//! MC-Trin's independent implementation of the public MCTier v3 wire protocol.
//! The EasyTier data plane runs as an ordinary child process in no-TUN mode.

use eyre::{Context, bail};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::{net::Ipv4Addr, sync::LazyLock};
use tokio::sync::{Mutex, mpsc, oneshot, watch};
use tokio_util::sync::CancellationToken;

use super::multiplayer::{self, MultiplayerProvider};

mod binary;
mod direct;

pub use direct::DirectRequest;
mod invite;
mod protocol;
mod runtime;

#[cfg(test)]
mod tests;

pub const DEFAULT_SIGNALING: &str = "wss://mctier.pmhs.top/signaling";
pub const DEFAULT_NODE: &str = "tcp://easytier.weiai.org.cn:11010";

#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum EntryMode {
    Create,
    Join,
}

#[derive(Clone, Deserialize)]
pub struct StartRequest {
    pub mode: EntryMode,
    pub lobby_name: String,
    pub password: String,
    pub player_name: String,
    pub server_node: String,
    pub signaling_server: String,
    #[serde(default)]
    pub is_public: bool,
    /// Optional description advertised in the public lobby listing.
    #[serde(default)]
    pub description: Option<String>,
    /// Optional player limit advertised in the public lobby listing.
    #[serde(default)]
    pub max_players: Option<u32>,
    /// Host Minecraft port, or the destination Minecraft port when joining.
    pub game_port: u16,
}

impl StartRequest {
    fn validate(&self) -> eyre::Result<()> {
        for (value, name, minimum, maximum) in [
            (&self.lobby_name, "lobby name", 4, 32),
            (&self.player_name, "player name", 1, 64),
            (&self.password, "password", 0, 128),
        ] {
            let length = value.chars().count();
            if length < minimum
                || length > maximum
                || value.chars().any(char::is_control)
            {
                bail!("invalid {name}");
            }
        }
        if self.lobby_name.trim() != self.lobby_name
            || !self.lobby_name.chars().any(char::is_alphanumeric)
            || !self.lobby_name.chars().all(|character| {
                character.is_alphanumeric() || matches!(character, '_' | '-')
            })
        {
            bail!("lobby names use letters, numbers, underscores, or hyphens");
        }
        validate_endpoint(&self.signaling_server, true)?;
        validate_endpoint(&self.server_node, false)?;
        if self.game_port == 0 {
            bail!("Minecraft port must be between 1 and 65535");
        }
        if self.is_public && self.mode != EntryMode::Create {
            bail!("only the room host can publish a lobby");
        }
        // The directory carries no password and the service isolates rooms by
        // name plus password, so a password-protected room cannot also be
        // listed. Refusing here keeps the UI from offering a combination the
        // service would silently ignore.
        if self.is_public && !self.password.is_empty() {
            bail!(
                "a public lobby cannot have a password; clear the password to publish, or keep the room private and share an invite code"
            );
        }
        let invalid_description =
            self.description.as_ref().is_some_and(|description| {
                description.chars().count() > 200
                    || description.chars().any(char::is_control)
            });
        if invalid_description {
            bail!("invalid lobby description");
        }
        if self.max_players.is_some_and(|players| players > 100) {
            bail!("the lobby player limit must not exceed 100");
        }
        Ok(())
    }
}

fn validate_endpoint(value: &str, signaling: bool) -> eyre::Result<()> {
    if value.len() > 2048 || value.chars().any(char::is_control) {
        bail!("invalid connection address");
    }
    let url = url::Url::parse(value).wrap_err("invalid connection address")?;
    let local =
        matches!(url.host_str(), Some("127.0.0.1" | "localhost" | "[::1]"));
    let allowed = if signaling {
        url.scheme() == "wss" || (url.scheme() == "ws" && local)
    } else {
        matches!(url.scheme(), "tcp" | "udp" | "ws" | "wss")
    };
    if !allowed
        || url.host_str().is_none()
        || !url.username().is_empty()
        || url.password().is_some()
        || url.fragment().is_some()
        || (!signaling
            && (url.query().is_some()
                || !matches!(url.path(), "" | "/")
                || url.port_or_known_default().is_none()))
    {
        bail!("unsupported connection address");
    }
    Ok(())
}

#[derive(Clone, Copy, Debug, Default, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum Status {
    #[default]
    Idle,
    Downloading,
    Connecting,
    Hosting,
    Joined,
    Error,
}

/// Stage of the EasyTier runtime preparation shown while a lobby starts.
#[derive(Clone, Copy, Debug, Default, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum PreparationStage {
    #[default]
    Downloading,
    Extracting,
    Verifying,
}

/// Non-fatal notices surfaced while a room is running.
#[derive(Clone, Copy, Debug, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum HostHint {
    /// Hosting started before Minecraft opened its LAN port.
    GamePortClosed,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Player {
    pub player_id: String,
    pub player_name: String,
    pub virtual_ip: Option<Ipv4Addr>,
    pub session_generation: u64,
}

/// How the host's public listing request resolved.
///
/// The lobby service broadcasts `lobby-options-changed` to every member, so an
/// acknowledgement is not guaranteed within any useful deadline. A host whose
/// request was accepted but not echoed must not lose the room, so the outcome
/// distinguishes "confirmed" from "sent".
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum PublishOutcome {
    /// The room is not listed publicly.
    #[default]
    Private,
    /// The service broadcast the new visibility.
    Published,
    /// The request was sent and not rejected, but no broadcast arrived.
    SentUnconfirmed,
    /// The directory was asked directly and does not list this room.
    NotListed,
}

#[derive(Clone, Debug, Default, Serialize)]
pub struct MctierState {
    pub status: Status,
    pub mode: Option<EntryMode>,
    pub lobby_name: Option<String>,
    pub virtual_ip: Option<Ipv4Addr>,
    pub game_port: Option<u16>,
    pub connect_address: Option<String>,
    pub players: Vec<Player>,
    pub local_player_id: Option<String>,
    pub voice_ice_server: Option<String>,
    pub error_message: Option<String>,
    pub is_public: bool,
    /// How the current public listing resolved, for host-facing status.
    pub publish_outcome: PublishOutcome,
    /// Server-side player capacity when the listing advertises one.
    pub max_players: Option<u32>,
    pub has_password: bool,
    /// Preparation stage, only meaningful while `status` is `downloading`.
    pub download_stage: Option<PreparationStage>,
    /// Download percentage (`0..=100`), or `None` when the size is unknown.
    pub download_progress: Option<u8>,
    /// Actionable notice for the host, cleared once it no longer applies.
    pub host_hint: Option<HostHint>,
}

#[derive(Default)]
struct Service {
    generation: u64,
    session: Option<Session>,
    state: MctierState,
    invite_context: Option<InviteContext>,
}

struct InviteContext {
    lobby_name: String,
    password: String,
    server_node: String,
    signaling_server: String,
    /// The invite is minted once per session. Rebuilding it would produce a
    /// different envelope (fresh key and nonce), so an invite that is fetched
    /// on every state poll would appear to change on screen.
    invite: Option<String>,
}

struct Session {
    generation: u64,
    cancellation: CancellationToken,
    done: watch::Receiver<bool>,
    control: mpsc::Sender<ControlMessage>,
}

pub(super) enum ControlMessage {
    SetPublic {
        is_public: bool,
        response: oneshot::Sender<Result<(), String>>,
    },
    SendVoiceSignal {
        message: Value,
        response: oneshot::Sender<Result<(), String>>,
    },
}

pub type VoiceSignalEmitter = std::sync::Arc<dyn Fn(Value) + Send + Sync>;

static SERVICE: LazyLock<Mutex<Service>> =
    LazyLock::new(|| Mutex::new(Service::default()));

/// Preparation progress lives outside `SERVICE` so the download loop can report
/// it synchronously, without awaiting the service lock between chunks.
#[derive(Default)]
struct Preparation {
    stage: Option<PreparationStage>,
    progress: Option<u8>,
}

static PREPARATION: LazyLock<std::sync::Mutex<Preparation>> =
    LazyLock::new(|| std::sync::Mutex::new(Preparation::default()));

fn report_preparation(stage: PreparationStage, percent: Option<u8>) {
    if let Ok(mut preparation) = PREPARATION.lock() {
        preparation.stage = Some(stage);
        preparation.progress = percent;
    }
}

fn reset_preparation() {
    if let Ok(mut preparation) = PREPARATION.lock() {
        *preparation = Preparation::default();
    }
}

pub struct PendingStart(oneshot::Receiver<eyre::Result<()>>);

impl PendingStart {
    pub async fn wait(self) -> eyre::Result<()> {
        self.0
            .await
            .wrap_err("lobby session stopped before returning a result")?
    }
}

pub fn is_supported() -> bool {
    binary::release_for(std::env::consts::OS, std::env::consts::ARCH).is_ok()
}

pub async fn get_state() -> MctierState {
    let mut state = SERVICE.lock().await.state.clone();
    if state.status == Status::Downloading
        && let Ok(preparation) = PREPARATION.lock()
    {
        state.download_stage = preparation.stage;
        state.download_progress = preparation.progress;
    }
    state
}

pub(super) async fn update(
    generation: u64,
    change: impl FnOnce(&mut MctierState),
) {
    let mut service = SERVICE.lock().await;
    if service
        .session
        .as_ref()
        .is_some_and(|session| session.generation == generation)
    {
        change(&mut service.state);
    }
}

pub async fn begin_start(
    request: StartRequest,
    voice_signal_emitter: VoiceSignalEmitter,
) -> eyre::Result<PendingStart> {
    let mut request = request;
    request.password = invite::resolve_password(&request.password)?;
    request.validate()?;
    if !is_supported() {
        bail!(
            "EasyTier is unavailable on this operating system or architecture"
        );
    }
    let mut service = SERVICE.lock().await;
    if service.session.is_some() {
        bail!("a lobby is already connecting or running");
    }
    multiplayer::claim_provider(MultiplayerProvider::Mctier).await?;
    service.generation += 1;
    let generation = service.generation;
    let cancellation = CancellationToken::new();
    let (control, control_receiver) = mpsc::channel(4);
    let (done, receiver) = watch::channel(false);
    service.session = Some(Session {
        generation,
        cancellation: cancellation.clone(),
        done: receiver,
        control,
    });
    service.invite_context =
        (request.mode == EntryMode::Create).then(|| InviteContext {
            lobby_name: request.lobby_name.clone(),
            password: request.password.clone(),
            server_node: request.server_node.clone(),
            signaling_server: request.signaling_server.clone(),
            invite: None,
        });
    service.state = MctierState {
        status: Status::Downloading,
        mode: Some(request.mode),
        lobby_name: Some(request.lobby_name.clone()),
        game_port: Some(request.game_port),
        has_password: !request.password.is_empty(),
        ..MctierState::default()
    };
    report_preparation(PreparationStage::Downloading, None);
    let (ready, result) = oneshot::channel();
    tokio::spawn(async move {
        use futures::FutureExt;
        let mut ready = Some(ready);
        let reporter: fn(PreparationStage, Option<u8>) = report_preparation;
        let outcome = std::panic::AssertUnwindSafe(async {
            let binary = binary::prepare(&cancellation, &reporter).await?;
            runtime::run(
                generation,
                request,
                &binary,
                &cancellation,
                control_receiver,
                voice_signal_emitter,
                &mut ready,
            )
            .await
        })
        .catch_unwind()
        .await
        .unwrap_or_else(|_| {
            Err(eyre::eyre!("lobby session stopped unexpectedly"))
        });
        if let Some(ready) = ready.take() {
            let message = outcome.as_ref().err().map_or_else(
                || "lobby operation cancelled".to_string(),
                |error| format!("{error:#}"),
            );
            let _ = ready.send(Err(eyre::eyre!(message)));
        }
        let mut service = SERVICE.lock().await;
        if service
            .session
            .as_ref()
            .is_some_and(|session| session.generation == generation)
        {
            service.session = None;
            service.invite_context = None;
            service.state.connect_address = None;
            service.state.players.clear();
            service.state.local_player_id = None;
            service.state.voice_ice_server = None;
            service.state.download_stage = None;
            service.state.download_progress = None;
            service.state.host_hint = None;
            reset_preparation();
            if cancellation.is_cancelled() || outcome.is_ok() {
                service.state = MctierState::default();
            } else {
                service.state.status = Status::Error;
                service.state.error_message =
                    outcome.err().map(|error| format!("{error:#}"));
            }
            multiplayer::release_provider(MultiplayerProvider::Mctier).await;
        }
        let _ = done.send(true);
    });
    Ok(PendingStart(result))
}

/// Starts a room that never contacts a lobby service.
///
/// Shares the session slot, kernel preparation and state reporting with the
/// lobby path; only the runtime differs, so a user cannot have both running.
pub async fn begin_start_direct(
    request: direct::DirectRequest,
) -> eyre::Result<PendingStart> {
    request.validate()?;
    if !is_supported() {
        bail!(
            "EasyTier is unavailable on this operating system or architecture"
        );
    }
    let mut service = SERVICE.lock().await;
    if service.session.is_some() {
        bail!("a room is already connecting or running");
    }
    multiplayer::claim_provider(MultiplayerProvider::Direct).await?;
    service.generation += 1;
    let generation = service.generation;
    let cancellation = CancellationToken::new();
    let (control, _control_receiver) = mpsc::channel(4);
    let (done, receiver) = watch::channel(false);
    service.session = Some(Session {
        generation,
        cancellation: cancellation.clone(),
        done: receiver,
        control,
    });
    // A direct room has no invite envelope to mint: the room code is built from
    // the same name and password the host typed.
    service.invite_context = None;
    service.state = MctierState {
        status: Status::Downloading,
        mode: Some(request.as_start_request().mode),
        lobby_name: Some(request.room_name.clone()),
        game_port: Some(request.game_port),
        has_password: !request.password.is_empty(),
        ..MctierState::default()
    };
    report_preparation(PreparationStage::Downloading, None);
    let (ready, result) = oneshot::channel();
    tokio::spawn(async move {
        use futures::FutureExt;
        let mut readiness = Some(ready);
        let reporter: fn(PreparationStage, Option<u8>) = report_preparation;
        let outcome = std::panic::AssertUnwindSafe(async {
            let binary = binary::prepare(&cancellation, &reporter).await?;
            direct::run(
                generation,
                request,
                &binary,
                &cancellation,
                readiness.take(),
            )
            .await
        })
        .catch_unwind()
        .await
        .unwrap_or_else(|_| {
            Err(eyre::eyre!("room session stopped unexpectedly"))
        });
        if let Some(ready) = readiness {
            let message = outcome.as_ref().err().map_or_else(
                || "room operation cancelled".to_string(),
                |error| format!("{error:#}"),
            );
            let _ = ready.send(Err(eyre::eyre!(message)));
        }
        let mut service = SERVICE.lock().await;
        if service
            .session
            .as_ref()
            .is_some_and(|session| session.generation == generation)
        {
            service.session = None;
            service.invite_context = None;
            reset_preparation();
            if cancellation.is_cancelled() || outcome.is_ok() {
                service.state = MctierState::default();
            } else {
                service.state.status = Status::Error;
                service.state.error_message =
                    outcome.err().map(|error| format!("{error:#}"));
            }
            multiplayer::release_provider(MultiplayerProvider::Direct).await;
        }
        let _ = done.send(true);
    });
    Ok(PendingStart(result))
}

pub async fn send_voice_signal(signal: Value) -> eyre::Result<()> {
    let (control, message, response) = {
        let service = SERVICE.lock().await;
        if !matches!(service.state.status, Status::Hosting | Status::Joined) {
            bail!("voice signaling requires an active MCTier room");
        }
        let local_player_id =
            service.state.local_player_id.as_deref().ok_or_else(|| {
                eyre::eyre!("the local room identity is unavailable")
            })?;
        let message = protocol::outgoing_voice_signal(
            &signal,
            local_player_id,
            &service.state.players,
        )?;
        let session = service
            .session
            .as_ref()
            .filter(|session| !session.cancellation.is_cancelled())
            .ok_or_else(|| {
                eyre::eyre!("the room session is no longer active")
            })?;
        let control = session.control.clone();
        let (response, receiver) = oneshot::channel();
        (control, message, (response, receiver))
    };
    let (sender, receiver) = response;
    control
        .send(ControlMessage::SendVoiceSignal {
            message,
            response: sender,
        })
        .await
        .map_err(|_| eyre::eyre!("the room session is no longer active"))?;
    tokio::time::timeout(std::time::Duration::from_secs(5), receiver)
        .await
        .wrap_err("voice signal delivery timed out")?
        .wrap_err("the room session stopped before sending the voice signal")?
        .map_err(eyre::Report::msg)
}

pub async fn set_public(is_public: bool) -> eyre::Result<()> {
    let (control, response) = {
        let service = SERVICE.lock().await;
        if service.state.status != Status::Hosting
            || service.state.mode != Some(EntryMode::Create)
        {
            bail!("only the active room host can change public visibility");
        }
        let control = service
            .session
            .as_ref()
            .ok_or_else(|| eyre::eyre!("the room session is no longer active"))?
            .control
            .clone();
        let (response, receiver) = oneshot::channel();
        (control, (response, receiver))
    };
    let (sender, receiver) = response;
    control
        .send(ControlMessage::SetPublic {
            is_public,
            response: sender,
        })
        .await
        .map_err(|_| eyre::eyre!("the room session is no longer active"))?;
    tokio::time::timeout(std::time::Duration::from_secs(8), receiver)
        .await
        .wrap_err("the signaling server did not confirm the visibility change")?
        .wrap_err(
            "the room session stopped before confirming the visibility change",
        )?
        .map_err(eyre::Report::msg)
}

pub async fn create_invite() -> eyre::Result<String> {
    let mut service = SERVICE.lock().await;
    if service.state.status != Status::Hosting {
        bail!("create an active room before generating an invitation");
    }
    // The signaling server may have suffixed the room name after a collision,
    // so the invite must follow the name the lobby was actually registered with.
    let registered_lobby_name = service
        .state
        .lobby_name
        .clone()
        .filter(|name| !name.trim().is_empty());
    let context = service
        .invite_context
        .as_mut()
        .ok_or_else(|| eyre::eyre!("room invitation data is unavailable"))?;
    if let Some(invite) = context.invite.clone() {
        return Ok(invite);
    }
    let lobby_name =
        registered_lobby_name.unwrap_or_else(|| context.lobby_name.clone());
    let secret = invite::export_password(&context.password)?;
    let mut serializer = url::form_urlencoded::Serializer::new(String::new());
    serializer
        .append_pair("v", "3")
        .append_pair("name", &lobby_name)
        .append_pair("secret", &secret)
        .append_pair("node", &context.server_node)
        .append_pair("signal", &context.signaling_server);
    let query = serializer.finish();
    // `mctrin` is the scheme registered by the desktop bundle. Keep parsing
    // legacy `mctier://` links in the frontend for invites shared earlier.
    let invite = format!("mctrin://join?{query}");
    context.invite = Some(invite.clone());
    Ok(invite)
}

pub async fn stop() -> eyre::Result<()> {
    let pending = {
        let service = SERVICE.lock().await;
        service.session.as_ref().map(|session| {
            session.cancellation.cancel();
            session.done.clone()
        })
    };
    if let Some(mut done) = pending {
        tokio::time::timeout(std::time::Duration::from_secs(10), async {
            while !*done.borrow_and_update() {
                done.changed()
                    .await
                    .wrap_err("lobby cleanup stopped unexpectedly")?;
            }
            Ok::<(), eyre::Report>(())
        })
        .await
        .wrap_err("lobby cleanup timed out")??;
    }
    Ok(())
}
