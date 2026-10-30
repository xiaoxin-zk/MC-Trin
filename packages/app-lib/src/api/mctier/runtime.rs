use eyre::{Context, bail};
use serde_json::json;
use std::{
    net::{Ipv4Addr, SocketAddrV4},
    path::Path,
    time::Duration,
};
use tokio::{
    net::{TcpListener, TcpStream, UdpSocket},
    process::Child,
    sync::oneshot,
    task::JoinHandle,
};
use tokio_util::sync::CancellationToken;

use super::{
    ControlMessage, EntryMode, HostHint, PublishOutcome, StartRequest, Status,
    VoiceSignalEmitter, binary, protocol, update,
};

/// How many times a dropped signaling link is retried before the room ends.
const RECONNECT_ATTEMPTS: u32 = 5;
pub(super) const REGISTRATION_ATTEMPT_LIMIT: usize = 16;
const REGISTRATION_TIME_LIMIT: Duration = if cfg!(test) {
    Duration::from_secs(2)
} else {
    Duration::from_secs(60)
};
/// Delay between reconnect attempts; it grows linearly per attempt. Tests use a
/// much shorter budget so the reconnect path stays cheap to exercise.
const RECONNECT_BACKOFF: Duration = if cfg!(test) {
    Duration::from_millis(50)
} else {
    Duration::from_secs(2)
};
/// Final, user-facing explanation once reconnecting has failed.
const SIGNALING_LOST: &str = "Lost the connection to the MCTier signaling server and could not reconnect. The room has ended - check your connection, then start or join the room again.";
/// How long the host waits for the service to echo a visibility change before
/// treating the request as sent. The echo is a broadcast that some deployments
/// never send, so this must stay short and its absence must not be an error.
const VISIBILITY_CONFIRMATION_WINDOW: Duration = if cfg!(test) {
    Duration::from_millis(150)
} else {
    Duration::from_millis(1500)
};

fn stun_binding_response(
    request: &[u8],
    source: std::net::SocketAddr,
    local_ip: Ipv4Addr,
) -> Option<[u8; 32]> {
    let std::net::SocketAddr::V4(source) = source else {
        return None;
    };
    if *source.ip() != local_ip
        || request.len() < 20
        || request.len() > 2048
        || request[..2] != [0, 1]
        || request[4..8] != [0x21, 0x12, 0xa4, 0x42]
    {
        return None;
    }
    let message_length = u16::from_be_bytes([request[2], request[3]]) as usize;
    if !message_length.is_multiple_of(4) || message_length + 20 != request.len()
    {
        return None;
    }
    let mut response = [0u8; 32];
    response[..4].copy_from_slice(&[1, 1, 0, 12]);
    response[4..20].copy_from_slice(&request[4..20]);
    response[20..26].copy_from_slice(&[0, 0x20, 0, 8, 0, 1]);
    response[26..28].copy_from_slice(&(source.port() ^ 0x2112).to_be_bytes());
    for (index, byte) in source.ip().octets().iter().enumerate() {
        response[28 + index] = byte ^ request[4 + index];
    }
    Some(response)
}

struct VoiceDiscovery(Option<JoinHandle<()>>);

impl VoiceDiscovery {
    async fn stop(mut self) {
        if let Some(worker) = self.0.take() {
            worker.abort();
            let _ = worker.await;
        }
    }
}

impl Drop for VoiceDiscovery {
    fn drop(&mut self) {
        if let Some(worker) = self.0.as_ref() {
            worker.abort();
        }
    }
}

fn spawn_voice_discovery(
    generation: u64,
    local_ip: Ipv4Addr,
    cancellation: CancellationToken,
) -> JoinHandle<()> {
    tokio::spawn(async move {
        let deadline = tokio::time::Instant::now() + Duration::from_secs(15);
        let socket = loop {
            if cancellation.is_cancelled() {
                return;
            }
            match UdpSocket::bind((local_ip, 0)).await {
                Ok(socket) => break socket,
                Err(error)
                    if (error.kind()
                        == std::io::ErrorKind::AddrNotAvailable
                        || error.raw_os_error() == Some(10049))
                        && tokio::time::Instant::now() < deadline =>
                {
                    tokio::select! {
                        _ = cancellation.cancelled() => return,
                        _ = tokio::time::sleep(Duration::from_millis(200)) => {}
                    }
                }
                Err(error) => {
                    tracing::warn!(%local_ip, %error, "MCTier voice STUN socket could not bind");
                    return;
                }
            }
        };
        let address = match socket.local_addr() {
            Ok(address) => address,
            Err(error) => {
                tracing::warn!(%error, "MCTier voice STUN socket has no local address");
                return;
            }
        };
        let url = format!("stun:{address}");
        update(generation, |state| state.voice_ice_server = Some(url)).await;
        let mut buffer = [0u8; 2049];
        loop {
            tokio::select! {
                biased;
                _ = cancellation.cancelled() => return,
                received = socket.recv_from(&mut buffer) => {
                    let Ok((length, source)) = received else { return };
                    if let Some(response) = stun_binding_response(&buffer[..length], source, local_ip) {
                        let _ = socket.send_to(&response, source).await;
                    }
                }
            }
        }
    })
}

pub(super) async fn register_with_retry(
    request: &StartRequest,
    mut identity: protocol::Identity,
    cancellation: &CancellationToken,
) -> eyre::Result<(protocol::Identity, protocol::Registered, StartRequest)> {
    /// The signaling server owns the public lobby list and rejects a name that
    /// another host already registered, whatever the password is. Suffixing the
    /// room once keeps the host in control instead of failing the whole room.
    fn lobby_name_taken(message: &str) -> bool {
        let message = message.to_lowercase();
        [
            "已被占用",
            "已被使用",
            "已存在",
            "occupied",
            "already taken",
            "already in use",
            "duplicate",
        ]
        .iter()
        .any(|needle| message.contains(needle))
    }

    let mut request = request.clone();
    let requested_name = request.lobby_name.clone();
    let mut renamed = false;
    let deadline = tokio::time::Instant::now() + REGISTRATION_TIME_LIMIT;
    for attempt in 0..REGISTRATION_ATTEMPT_LIMIT {
        identity.ip = identity.candidate_ip(attempt).ok_or_else(|| {
            eyre::eyre!("the signaling address pool is exhausted")
        })?;
        let result = tokio::select! {
            biased;
            _ = cancellation.cancelled() => bail!("lobby operation cancelled"),
            result = tokio::time::timeout_at(
                deadline,
                protocol::register(&request, &identity),
            ) => result.wrap_err("MCTier registration exceeded its overall time limit")?,
        };
        match result {
            Ok(session) => return Ok((identity, session, request)),
            Err(protocol::RegistrationError::AddressInUse) => continue,
            Err(protocol::RegistrationError::Failed(error)) => {
                if !renamed && lobby_name_taken(&format!("{error:#}")) {
                    request.lobby_name = format!(
                        "{requested_name}-{:04x}",
                        rand::random::<u16>()
                    );
                    renamed = true;
                    continue;
                }
                return Err(error);
            }
        }
    }
    bail!(
        "all {REGISTRATION_ATTEMPT_LIMIT} MCTier virtual address attempts were rejected"
    )
}

pub fn arguments(
    request: &StartRequest,
    ip: Ipv4Addr,
    rpc_port: u16,
    forwarding: Option<(u16, Ipv4Addr)>,
) -> Vec<String> {
    let mut arguments = vec![
        "--no-tun".into(),
        "true".into(),
        "--use-smoltcp".into(),
        "true".into(),
        "--ipv4".into(),
        ip.to_string(),
        "--network-name".into(),
        format!("MCTier-{}", request.lobby_name),
        "--peers".into(),
        request.server_node.clone(),
        "--listeners".into(),
        "tcp://0.0.0.0:0".into(),
        "--rpc-portal".into(),
        format!("127.0.0.1:{rpc_port}"),
        "--console-log-level".into(),
        "off".into(),
        "--file-log-level".into(),
        "off".into(),
        "--hostname".into(),
        format!("MC-Trin-{ip}"),
    ];
    if let Some((port, remote)) = forwarding {
        arguments.extend([
            "--port-forward".into(),
            format!("tcp://127.0.0.1:{port}/{remote}:{}", request.game_port),
        ]);
    }
    arguments
}

pub(super) async fn free_port() -> eyre::Result<u16> {
    Ok(TcpListener::bind((Ipv4Addr::LOCALHOST, 0))
        .await?
        .local_addr()?
        .port())
}

/// One status handshake against the local end of the lobby port forward.
async fn ping_forwarded_game(port: u16) -> eyre::Result<()> {
    let connection = async_minecraft_ping::ConnectionConfig::build("127.0.0.1")
        .with_port(port)
        .with_timeout(Duration::from_secs(10))
        .connect()
        .await
        .wrap_err("the lobby network refused the Minecraft handshake")?
        .status()
        .await
        .wrap_err("the lobby network did not deliver a Minecraft status")?;
    connection
        .ping(1)
        .await
        .wrap_err("the lobby network did not deliver a Minecraft ping")?;
    Ok(())
}

fn error_chain(error: &eyre::Report) -> String {
    error
        .chain()
        .map(|cause| cause.to_string())
        .collect::<Vec<_>>()
        .join(": ")
}

/// The port forwarder binds its local socket as soon as EasyTier launches, which
/// happens well before the peer tunnel carries traffic. Early handshakes are
/// therefore reset, so retry until Minecraft answers instead of failing the
/// join on the first attempt.
async fn verify_forwarded_game(
    child: &mut Child,
    port: u16,
    cancellation: &CancellationToken,
) -> eyre::Result<()> {
    let address = SocketAddrV4::new(Ipv4Addr::LOCALHOST, port);
    let deadline = tokio::time::Instant::now() + Duration::from_secs(60);
    loop {
        if let Some(status) = child.try_wait()? {
            bail!(
                "EasyTier exited before the lobby network was ready: {status}"
            );
        }
        let failure = match ping_forwarded_game(port).await {
            Ok(()) => return Ok(()),
            Err(error) => error_chain(&error),
        };
        if tokio::time::Instant::now() >= deadline {
            bail!(
                "Minecraft did not answer through the lobby network at {address}: {failure}. Ask the host to keep the world open on LAN and to check the shared port."
            );
        }
        tokio::select! {
            _ = cancellation.cancelled() => bail!("lobby operation cancelled"),
            _ = tokio::time::sleep(Duration::from_secs(2)) => {}
        }
    }
}

async fn wait_for_listener(child: &mut Child, port: u16) -> eyre::Result<()> {
    tokio::time::timeout(Duration::from_secs(10), async {
        loop {
            if let Some(status) = child.try_wait()? {
                bail!("EasyTier exited before starting: {status}");
            }
            if TcpStream::connect((Ipv4Addr::LOCALHOST, port))
                .await
                .is_ok()
            {
                return Ok(());
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
    })
    .await
    .wrap_err("EasyTier local service did not become ready")?
}

pub async fn run(
    generation: u64,
    request: StartRequest,
    executable: &Path,
    cancellation: &CancellationToken,
    mut controls: tokio::sync::mpsc::Receiver<ControlMessage>,
    voice_signal_emitter: VoiceSignalEmitter,
    ready: &mut Option<oneshot::Sender<eyre::Result<()>>>,
) -> eyre::Result<()> {
    update(generation, |state| state.status = Status::Connecting).await;
    let identity = protocol::Identity::generate()?;
    let requested_lobby_name = request.lobby_name.clone();
    let (identity, mut registration, request) =
        register_with_retry(&request, identity, cancellation).await?;
    if request.lobby_name != requested_lobby_name {
        let lobby_name = request.lobby_name.clone();
        update(generation, |state| state.lobby_name = Some(lobby_name)).await;
    }
    let forwarding = if request.mode == EntryMode::Join {
        let remote = registration
            .players
            .iter()
            .find(|player| player.player_id == registration.host_id)
            .and_then(|player| player.virtual_ip)
            .ok_or_else(|| {
                eyre::eyre!("the host has no reachable virtual address")
            })?;
        if registration.host_id == identity.id {
            bail!("joining requires another player to be hosting the lobby");
        }
        Some((free_port().await?, remote))
    } else {
        // Hosting must not depend on the LAN world already being open: players
        // routinely create the room first and open the world afterwards. The
        // port forward only dials this socket per incoming connection, so a
        // closed port here is worth a warning, never a failed room.
        if let Err(error) =
            TcpStream::connect((Ipv4Addr::LOCALHOST, request.game_port)).await
        {
            tracing::warn!(
                port = request.game_port,
                %error,
                "Minecraft is not listening on the selected port yet; the room keeps waiting for it"
            );
            update(generation, |state| {
                state.host_hint = Some(HostHint::GamePortClosed);
            })
            .await;
            // Clear the notice as soon as the player opens the world to LAN.
            let game_port = request.game_port;
            tokio::spawn(async move {
                for _ in 0..120 {
                    tokio::time::sleep(Duration::from_secs(3)).await;
                    if TcpStream::connect((Ipv4Addr::LOCALHOST, game_port))
                        .await
                        .is_ok()
                    {
                        update(generation, |state| state.host_hint = None)
                            .await;
                        return;
                    }
                }
            });
        }
        None
    };
    let rpc_port = free_port().await?;
    let mut child = binary::hidden_command(executable)
        .args(arguments(&request, identity.ip, rpc_port, forwarding))
        .env("ET_NETWORK_SECRET", &request.password)
        .spawn()
        .wrap_err("could not start EasyTier")?;
    let mut voice_discovery: Option<VoiceDiscovery> = None;
    let operation = async {
        wait_for_listener(&mut child, rpc_port).await?;
        if let Some((port, _)) = forwarding {
            wait_for_listener(&mut child, port).await?;
            // A bound local socket alone does not prove that Minecraft can be reached.
            verify_forwarded_game(&mut child, port, cancellation).await?;
        }
        // The lobby service keeps a freshly registered room in the public list
        // until the host opts out, so private rooms have to state their
        // visibility explicitly instead of relying on the default. Public
        // rooms stay listed even with a password: the server validates the
        // password when players join, and a service that refuses the option
        // must not take the room down.
        let mut publish_outcome = PublishOutcome::Private;
        if request.is_public {
            match set_lobby_visibility(
                &mut registration.socket,
                &identity.id,
                &request.server_node,
                true,
                Some(&request),
                cancellation,
            )
            .await
            {
                Ok(outcome) => {
                    publish_outcome = outcome;
                    // The request is one-way, so confirm it against the
                    // directory itself. A host that is told "unconfirmed"
                    // otherwise has no way to know the room is missing.
                    if outcome != PublishOutcome::Private {
                        let listed = protocol::public_lobby_names(
                            &request.signaling_server,
                            VISIBILITY_CONFIRMATION_WINDOW * 4,
                        )
                        .await;
                        match listed {
                            Ok(names)
                                if names.iter().any(|name| {
                                    name == &request.lobby_name
                                }) =>
                            {
                                publish_outcome = PublishOutcome::Published
                            }
                            Ok(_) => {
                                tracing::warn!(
                                    lobby = %request.lobby_name,
                                    "the lobby service accepted the publish but does not list the room"
                                );
                                publish_outcome = PublishOutcome::NotListed;
                            }
                            Err(error) => {
                                tracing::warn!(
                                    %error,
                                    "could not read the lobby directory to confirm the listing"
                                );
                            }
                        }
                    }
                }
                Err(error) => {
                    // The room itself is healthy; only its listing failed.
                    tracing::warn!(%error, "the lobby could not be published publicly");
                }
            }
        } else if let Err(error) = set_lobby_visibility(
            &mut registration.socket,
            &identity.id,
            &request.server_node,
            false,
            None,
            cancellation,
        )
        .await
        {
            // A service that refuses the option must not stop the room: the
            // host still keeps the password, and the local state stays private.
            tracing::warn!(%error, "could not unlist the lobby from the public list");
        }
        voice_discovery = Some(VoiceDiscovery(Some(spawn_voice_discovery(
            generation,
            identity.ip,
            cancellation.clone(),
        ))));
        update(generation, |state| {
            state.status = if request.mode == EntryMode::Create {
                Status::Hosting
            } else {
                Status::Joined
            };
            state.virtual_ip = Some(identity.ip);
            state.local_player_id = Some(identity.id.clone());
            state.is_public = matches!(
                publish_outcome,
                PublishOutcome::Published | PublishOutcome::SentUnconfirmed
            );
            state.publish_outcome = publish_outcome;
            state.max_players = request.max_players;
            state.players = registration.players.clone();
            state.connect_address = forwarding.map(|(port, _)| {
                SocketAddrV4::new(Ipv4Addr::LOCALHOST, port).to_string()
            });
        })
        .await;
        if let Some(ready) = ready.take() {
            let _ = ready.send(Ok(()));
        }
        let mut heartbeat = tokio::time::interval(Duration::from_secs(15));
        let mut controls_open = true;
        loop {
            enum Event {
                Cancelled,
                Exited(std::io::Result<std::process::ExitStatus>),
                Heartbeat,
                Control(Option<ControlMessage>),
                Message(eyre::Result<serde_json::Value>),
            }
            let event = tokio::select! {
                biased;
                _ = cancellation.cancelled() => Event::Cancelled,
                exit = child.wait() => Event::Exited(exit),
                _ = heartbeat.tick() => Event::Heartbeat,
                control = controls.recv(), if controls_open => Event::Control(control),
                message = protocol::next_message(&mut registration.socket) => Event::Message(message),
            };
            match event {
                Event::Cancelled => return Ok(()),
                Event::Exited(exit) => {
                    bail!("EasyTier network process exited: {}", exit?)
                }
                Event::Heartbeat => {
                    registration
                        .socket
                        .send(async_tungstenite::tungstenite::Message::Text(
                            json!({"type":"ping"}).to_string().into(),
                        ))
                        .await?
                }
                Event::Control(Some(ControlMessage::SetPublic {
                    is_public,
                    response,
                })) => {
                    let result = if request.mode != EntryMode::Create {
                        Err(eyre::eyre!(
                            "only the active room host can change public visibility"
                        ))
                    } else {
                        set_lobby_visibility(
                            &mut registration.socket,
                            &identity.id,
                            &request.server_node,
                            is_public,
                            Some(&request),
                            cancellation,
                        )
                        .await
                    };
                    match result {
                        Ok(outcome) => {
                            update(generation, |state| {
                                state.is_public = matches!(
                                    outcome,
                                    PublishOutcome::Published
                                        | PublishOutcome::SentUnconfirmed
                                );
                                state.publish_outcome = outcome;
                            })
                            .await;
                            let _ = response.send(Ok(()));
                        }
                        Err(error) => {
                            let message = format!("{error:#}");
                            let _ = response.send(Err(message));
                        }
                    }
                }
                Event::Control(Some(ControlMessage::SendVoiceSignal {
                    message,
                    response,
                })) => {
                    let result = registration
                        .socket
                        .send(async_tungstenite::tungstenite::Message::Text(
                            message.to_string().into(),
                        ))
                        .await
                        .map_err(|error| {
                            format!("could not send voice signal: {error}")
                        });
                    let _ = response.send(result);
                }
                Event::Control(None) => controls_open = false,
                Event::Message(Err(error)) => {
                    // The signaling link died mid-session. Try to re-register
                    // with the same identity and address before giving up, so a
                    // blip does not end the room.
                    tracing::warn!(%error, "MCTier signaling link failed; reconnecting");
                    update(generation, |state| {
                        state.status = Status::Connecting;
                        state.error_message = Some(
                            "Reconnecting to the MCTier signaling server..."
                                .to_string(),
                        );
                    })
                    .await;
                    let mut reconnected = None;
                    for attempt in 0..RECONNECT_ATTEMPTS {
                        if cancellation.is_cancelled() {
                            return Ok(());
                        }
                        tokio::select! {
                            _ = cancellation.cancelled() => return Ok(()),
                            _ = tokio::time::sleep(RECONNECT_BACKOFF * (attempt + 1)) => {}
                        }
                        let Ok(result) = tokio::time::timeout(
                            Duration::from_secs(10),
                            protocol::register(&request, &identity),
                        )
                        .await
                        else {
                            tracing::warn!(
                                attempt,
                                "MCTier reconnect attempt timed out"
                            );
                            continue;
                        };
                        match result {
                            Ok(session) => {
                                reconnected = Some(session);
                                break;
                            }
                            Err(error) => {
                                tracing::warn!(
                                    attempt,
                                    %error,
                                    "MCTier reconnect attempt failed"
                                );
                            }
                        }
                    }
                    let Some(session) = reconnected else {
                        bail!(SIGNALING_LOST);
                    };
                    registration = session;
                    let players = registration.players.clone();
                    update(generation, |state| {
                        state.status = if request.mode == EntryMode::Create {
                            Status::Hosting
                        } else {
                            Status::Joined
                        };
                        state.error_message = None;
                        state.players = players;
                    })
                    .await;
                    continue;
                }
                Event::Message(Ok(message)) => {
                    match message["type"].as_str() {
                        Some("players-list") => {
                            let players = protocol::parse_players(&message)?;
                            registration.players = players.clone();
                            update(generation, |state| state.players = players)
                                .await;
                        }
                        Some(
                            "player-joined" | "player-left" | "host-changed",
                        ) => {
                            registration.socket.send(async_tungstenite::tungstenite::Message::Text(json!({"type":"players-list-request"}).to_string().into())).await?;
                        }
                        // The service broadcasts lobby option changes to every
                        // member, including the host that requested them. Keep
                        // the advertised state in step with the server so a
                        // listing changed from another session is not shown
                        // wrongly, and so an echoed request settles the host's
                        // publish status.
                        Some("lobby-options-changed") => {
                            if let Some(is_public) =
                                message["isPublic"].as_bool()
                            {
                                let max_players =
                                    message["maxPlayers"].as_u64().and_then(
                                        |value| u32::try_from(value).ok(),
                                    );
                                update(generation, |state| {
                                    state.is_public = is_public;
                                    state.publish_outcome = if is_public {
                                        PublishOutcome::Published
                                    } else {
                                        PublishOutcome::Private
                                    };
                                    if max_players.is_some() {
                                        state.max_players = max_players;
                                    }
                                })
                                .await;
                            }
                        }
                        Some(
                            "kicked" | "register-error" | "version-too-old",
                        ) => bail!(
                            "the signaling server ended this lobby session"
                        ),
                        Some("offer" | "answer" | "ice-candidate") => {
                            match protocol::incoming_voice_signal(
                                &message,
                                &identity.id,
                                &registration.players,
                            ) {
                                Ok(signal) => voice_signal_emitter(signal),
                                Err(error) => tracing::warn!(
                                    %error,
                                    "rejected invalid MCTier voice signal"
                                ),
                            }
                        }
                        _ => {}
                    }
                }
            }
        }
    };
    let result = tokio::select! {
        biased;
        _ = cancellation.cancelled() => Ok(()),
        result = operation => result,
    };
    if let Some(discovery) = voice_discovery.take() {
        discovery.stop().await;
    }
    let _ = child.kill().await;
    let _ = child.wait().await;
    let _ = tokio::time::timeout(Duration::from_secs(2), async {
        registration
            .socket
            .send(async_tungstenite::tungstenite::Message::Text(
                json!({"type":"leave", "clientId":identity.id})
                    .to_string()
                    .into(),
            ))
            .await?;
        registration.socket.close(None).await
    })
    .await;
    result
}

/// Asks the lobby service to change this room's public listing.
///
/// `lobby-options-changed` is a broadcast to every member, not a per-request
/// receipt, so waiting for it as if it were an acknowledgement made every
/// publish attempt fail after the timeout and left the room out of the public
/// list. The request is therefore optimistic: a rejection fails, a broadcast
/// confirms, and silence means the service accepted the frame without echoing
/// it — which must not take the host's room down.
async fn set_lobby_visibility(
    socket: &mut protocol::Socket,
    player_id: &str,
    server_node: &str,
    is_public: bool,
    request: Option<&StartRequest>,
    cancellation: &CancellationToken,
) -> eyre::Result<PublishOutcome> {
    let mut options = json!({
        "type": "set-lobby-options",
        "from": player_id,
        "isPublic": is_public,
        "serverNode": server_node,
    });
    if let Some(request) = request {
        if let Some(description) = request.description.as_deref() {
            options["description"] = json!(description);
        }
        if let Some(max_players) = request.max_players {
            options["maxPlayers"] = json!(max_players);
        }
    }
    socket
        .send(async_tungstenite::tungstenite::Message::Text(
            options.to_string().into(),
        ))
        .await
        .wrap_err("could not send lobby visibility settings")?;

    let confirmation = tokio::time::timeout(VISIBILITY_CONFIRMATION_WINDOW, async {
        loop {
            let message = tokio::select! {
                biased;
                _ = cancellation.cancelled() => bail!("lobby operation cancelled"),
                message = protocol::next_message(socket) => message?,
            };
            match message["type"].as_str() {
                Some("lobby-options-changed") if message["isPublic"] == is_public => {
                    return Ok(())
                }
                Some("register-error") => {
                    bail!("the signaling server rejected lobby visibility changes")
                }
                Some("kicked" | "version-too-old") => {
                    bail!("the signaling server ended this lobby session")
                }
                _ => {}
            }
        }
    })
    .await;

    match confirmation {
        Ok(Ok(())) if is_public => Ok(PublishOutcome::Published),
        // Unlisting needs no acknowledgement: the room simply stops being
        // advertised, and the host keeps hosting either way.
        Ok(Ok(())) => Ok(PublishOutcome::Private),
        Ok(Err(error)) => Err(error),
        Err(_) if is_public => {
            tracing::debug!(
                "the lobby service did not echo the visibility change; keeping the listing request"
            );
            Ok(PublishOutcome::SentUnconfirmed)
        }
        Err(_) => Ok(PublishOutcome::Private),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use async_tungstenite::{
        tokio::{accept_async, connect_async},
        tungstenite::Message,
    };
    use futures::StreamExt;
    use serde_json::Value;
    use tokio::{net::TcpListener, sync::oneshot};

    #[test]
    fn stun_binding_response_is_bounded_and_only_answers_the_overlay_source() {
        let ip = Ipv4Addr::new(10, 126, 126, 7);
        let source = std::net::SocketAddr::from((ip, 52341));
        let mut request = [0u8; 20];
        request[1] = 1;
        request[4..8].copy_from_slice(&[0x21, 0x12, 0xa4, 0x42]);
        request[8..20].fill(9);

        let response = stun_binding_response(&request, source, ip).unwrap();
        assert_eq!(&response[..4], &[1, 1, 0, 12]);
        assert_eq!(&response[4..20], &request[4..20]);
        assert_eq!(
            u16::from_be_bytes([response[26], response[27]]) ^ 0x2112,
            source.port()
        );
        let decoded_ip: Vec<_> = (0..4)
            .map(|index| response[28 + index] ^ request[4 + index])
            .collect();
        assert_eq!(decoded_ip, ip.octets());

        assert!(
            stun_binding_response(
                &request,
                std::net::SocketAddr::from((
                    Ipv4Addr::new(10, 126, 126, 8),
                    5000
                )),
                ip,
            )
            .is_none()
        );
        assert!(
            stun_binding_response(
                &request,
                std::net::SocketAddr::from((
                    std::net::Ipv6Addr::LOCALHOST,
                    5000
                )),
                ip,
            )
            .is_none()
        );
        assert!(stun_binding_response(&request[..19], source, ip).is_none());
        assert!(stun_binding_response(&[0; 2049], source, ip).is_none());

        request[3] = 4;
        assert!(stun_binding_response(&request, source, ip).is_none());
    }

    #[tokio::test]
    async fn visibility_change_uses_mctier_host_message_and_requires_echo() {
        let listener =
            TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).await.unwrap();
        let address = listener.local_addr().unwrap();
        let (sent, received) = oneshot::channel();
        tokio::spawn(async move {
            let (stream, _) = listener.accept().await.unwrap();
            let mut socket = accept_async(stream).await.unwrap();
            let text =
                socket.next().await.unwrap().unwrap().into_text().unwrap();
            let message: Value = serde_json::from_str(text.as_ref()).unwrap();
            let _ = sent.send(message);
            socket
                .send(Message::Text(
                    json!({ "type": "lobby-options-changed", "isPublic": true })
                        .to_string()
                        .into(),
                ))
                .await
                .unwrap();
        });

        let (mut socket, _) =
            connect_async(format!("ws://{address}")).await.unwrap();
        let cancellation = CancellationToken::new();
        let outcome = set_lobby_visibility(
            &mut socket,
            &"a".repeat(64),
            "tcp://127.0.0.1:11010",
            true,
            None,
            &cancellation,
        )
        .await
        .unwrap();
        assert_eq!(outcome, PublishOutcome::Published);

        let message = received.await.unwrap();
        assert_eq!(message["type"], "set-lobby-options");
        assert_eq!(message["from"], "a".repeat(64));
        assert_eq!(message["isPublic"], true);
        assert_eq!(message["serverNode"], "tcp://127.0.0.1:11010");
    }

    /// A service that accepts the frame without echoing it must not be treated
    /// as a failure: that turned every publish into the "did not confirm lobby
    /// visibility" error and kept the room out of the public list.
    #[tokio::test]
    async fn an_unanswered_visibility_change_stays_published() {
        let listener =
            TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).await.unwrap();
        let address = listener.local_addr().unwrap();
        tokio::spawn(async move {
            let (stream, _) = listener.accept().await.unwrap();
            let mut socket = accept_async(stream).await.unwrap();
            // Read the request, answer nothing, keep the socket open past the
            // confirmation window.
            let _ = socket.next().await;
            tokio::time::sleep(VISIBILITY_CONFIRMATION_WINDOW * 3).await;
        });

        let (mut socket, _) =
            connect_async(format!("ws://{address}")).await.unwrap();
        let outcome = set_lobby_visibility(
            &mut socket,
            &"b".repeat(64),
            "tcp://127.0.0.1:11010",
            true,
            None,
            &CancellationToken::new(),
        )
        .await
        .expect("silence is not a failure");
        assert_eq!(outcome, PublishOutcome::SentUnconfirmed);
    }

    /// Unlisting needs no echo either; the room simply stops being advertised.
    #[tokio::test]
    async fn an_unanswered_unlist_returns_to_private() {
        let listener =
            TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).await.unwrap();
        let address = listener.local_addr().unwrap();
        tokio::spawn(async move {
            let (stream, _) = listener.accept().await.unwrap();
            let mut socket = accept_async(stream).await.unwrap();
            let _ = socket.next().await;
            tokio::time::sleep(VISIBILITY_CONFIRMATION_WINDOW * 3).await;
        });

        let (mut socket, _) =
            connect_async(format!("ws://{address}")).await.unwrap();
        let outcome = set_lobby_visibility(
            &mut socket,
            &"c".repeat(64),
            "tcp://127.0.0.1:11010",
            false,
            None,
            &CancellationToken::new(),
        )
        .await
        .unwrap();
        assert_eq!(outcome, PublishOutcome::Private);
    }

    /// A rejection is a real failure and must surface to the host.
    #[tokio::test]
    async fn a_rejected_visibility_change_reports_an_error() {
        let listener =
            TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).await.unwrap();
        let address = listener.local_addr().unwrap();
        tokio::spawn(async move {
            let (stream, _) = listener.accept().await.unwrap();
            let mut socket = accept_async(stream).await.unwrap();
            let _ = socket.next().await;
            socket
                .send(Message::Text(
                    json!({ "type": "register-error" }).to_string().into(),
                ))
                .await
                .unwrap();
        });

        let (mut socket, _) =
            connect_async(format!("ws://{address}")).await.unwrap();
        let error = set_lobby_visibility(
            &mut socket,
            &"d".repeat(64),
            "tcp://127.0.0.1:11010",
            true,
            None,
            &CancellationToken::new(),
        )
        .await
        .unwrap_err();
        assert!(
            format!("{error:#}").contains("rejected lobby visibility"),
            "{error:#}"
        );
    }
}
