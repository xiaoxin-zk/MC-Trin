use eyre::{Context, bail};
use futures::SinkExt;
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
    ControlMessage, EntryMode, StartRequest, Status, VoiceSignalEmitter,
    binary, protocol, update,
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
    if message_length % 4 != 0 || message_length + 20 != request.len() {
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
) -> eyre::Result<(protocol::Identity, protocol::Registered)> {
    for attempt in 0..254 {
        identity.ip = identity.candidate_ip(attempt).ok_or_else(|| {
            eyre::eyre!("the signaling address pool is exhausted")
        })?;
        let result = tokio::select! {
            biased;
            _ = cancellation.cancelled() => bail!("lobby operation cancelled"),
            result = protocol::register(request, &identity) => result,
        };
        match result {
            Ok(session) => return Ok((identity, session)),
            Err(protocol::RegistrationError::AddressInUse) => continue,
            Err(protocol::RegistrationError::Failed(error)) => {
                return Err(error);
            }
        }
    }
    bail!("all 254 signaling virtual addresses are already in use")
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

async fn free_port() -> eyre::Result<u16> {
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
    let (identity, mut registration) =
        register_with_retry(&request, identity, cancellation).await?;
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
        if request.is_public {
            set_lobby_visibility(
                &mut registration.socket,
                &identity.id,
                &request.server_node,
                true,
                cancellation,
            )
            .await?;
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
            state.is_public = request.is_public;
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
                    } else if is_public && !request.password.is_empty() {
                        Err(eyre::eyre!(
                            "remove the room password before publishing it"
                        ))
                    } else {
                        set_lobby_visibility(
                            &mut registration.socket,
                            &identity.id,
                            &request.server_node,
                            is_public,
                            cancellation,
                        )
                        .await
                    };
                    match result {
                        Ok(()) => {
                            update(generation, |state| {
                                state.is_public = is_public
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
                Event::Message(message) => {
                    let message = message?;
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

async fn set_lobby_visibility(
    socket: &mut protocol::Socket,
    player_id: &str,
    server_node: &str,
    is_public: bool,
    cancellation: &CancellationToken,
) -> eyre::Result<()> {
    socket
        .send(async_tungstenite::tungstenite::Message::Text(
            json!({
                "type": "set-lobby-options",
                "from": player_id,
                "isPublic": is_public,
                "serverNode": server_node,
            })
            .to_string()
            .into(),
        ))
        .await
        .wrap_err("could not send lobby visibility settings")?;

    tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            let message = tokio::select! {
                biased;
                _ = cancellation.cancelled() => bail!("lobby operation cancelled"),
                message = protocol::next_message(socket) => message?,
            };
            match message["type"].as_str() {
                Some("lobby-options-changed") if message["isPublic"] == is_public => return Ok(()),
                Some("register-error") => bail!("the signaling server rejected lobby visibility changes"),
                Some("kicked" | "version-too-old") => bail!("the signaling server ended this lobby session"),
                _ => {}
            }
        }
    })
    .await
    .wrap_err("the signaling server did not confirm lobby visibility; it may not support MCTier lobby options")?
}

#[cfg(test)]
mod tests {
    use super::*;
    use async_tungstenite::{
        tokio::{accept_async, connect_async},
        tungstenite::Message,
    };
    use futures::{SinkExt, StreamExt};
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
        set_lobby_visibility(
            &mut socket,
            &"a".repeat(64),
            "tcp://127.0.0.1:11010",
            true,
            &cancellation,
        )
        .await
        .unwrap();

        let message = received.await.unwrap();
        assert_eq!(message["type"], "set-lobby-options");
        assert_eq!(message["from"], "a".repeat(64));
        assert_eq!(message["isPublic"], true);
        assert_eq!(message["serverNode"], "tcp://127.0.0.1:11010");
    }
}
