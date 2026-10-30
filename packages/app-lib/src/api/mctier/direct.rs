//! A room that needs no lobby service.
//!
//! The lobby protocol deliberately couples a room's password to the network
//! secret, which is what makes a listed room passwordless: a published room
//! must let strangers derive the same secret. Users still want a password, so
//! this mode keeps the EasyTier network and drops the directory: both sides
//! agree on a room name and secret out of band, exactly like the other EasyTier
//! shells, and no server has to know the room exists.
//!
//! Consequence worth stating plainly in the UI: a direct room is never listed.
//! It is joined by pasting the room code or typing the virtual address.

use std::{net::Ipv4Addr, path::Path, process::Stdio, time::Duration};

use eyre::{Context, bail};
use serde::{Deserialize, Serialize};
use tokio::{net::TcpStream, process::Child};
use tokio_util::sync::CancellationToken;

use super::{
    HostHint, PublishOutcome, StartRequest, Status, binary, runtime, update,
};

/// What the room form collects. Mirrors the fields the lobby path uses so the
/// two modes share validation and the kernel arguments stay in one place.
#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct DirectRequest {
    pub room_name: String,
    pub password: String,
    pub player_name: String,
    pub server_node: String,
    /// Host Minecraft port, or the port to reach on the host when joining.
    pub game_port: u16,
    /// Virtual address of the host, required when joining.
    #[serde(default)]
    pub host_address: Option<String>,
}

impl DirectRequest {
    /// The network name that both sides must agree on. Prefixed so a direct
    /// room never collides with a lobby room of the same name.
    pub fn network_name(&self) -> String {
        format!("MCTRIN-{}", self.room_name)
    }

    pub fn validate(&self) -> eyre::Result<()> {
        let name_length = self.room_name.chars().count();
        if !(4..=32).contains(&name_length)
            || self.room_name.trim() != self.room_name
            || self.room_name.chars().any(char::is_control)
            || !self.room_name.chars().all(|character| {
                character.is_alphanumeric() || matches!(character, '_' | '-')
            })
        {
            bail!(
                "room names use 4-32 letters, numbers, underscores, or hyphens"
            );
        }
        if self.player_name.trim().is_empty()
            || self.player_name.chars().count() > 64
            || self.player_name.chars().any(char::is_control)
        {
            bail!("invalid player name");
        }
        // A direct room exists because of its password, so an empty one is a
        // configuration mistake rather than a valid open network.
        if self.password.is_empty() {
            bail!("a direct room needs a password");
        }
        if self.password.chars().count() > 128
            || self.password.chars().any(char::is_control)
        {
            bail!("invalid password");
        }
        if self.game_port == 0 {
            bail!("Minecraft port must be between 1 and 65535");
        }
        if let Some(address) = self.host_address.as_deref() {
            address
                .parse::<Ipv4Addr>()
                .wrap_err("the host address must be an IPv4 address")?;
        }
        Ok(())
    }

    /// Reuses the lobby request so the kernel arguments and port forwarding stay
    /// defined in one place instead of drifting between the two modes.
    fn as_start_request(&self) -> StartRequest {
        StartRequest {
            mode: if self.host_address.is_some() {
                super::EntryMode::Join
            } else {
                super::EntryMode::Create
            },
            lobby_name: self.room_name.clone(),
            password: self.password.clone(),
            player_name: self.player_name.clone(),
            server_node: self.server_node.clone(),
            // There is no signaling server, but the field is part of the shared
            // request shape; an empty value can never be dialed.
            signaling_server: String::new(),
            is_public: false,
            description: None,
            max_players: None,
            game_port: self.game_port,
        }
    }
}

/// Kernel arguments for a direct room.
///
/// The lobby path builds the network name as `MCTier-{lobby}`; a direct room
/// must use its own prefix so the two never share a network by accident. The
/// forwarding target differs per role: the host publishes its local Minecraft
/// port into the overlay, the guest dials the host's virtual address.
pub fn arguments(
    request: &DirectRequest,
    ip: Ipv4Addr,
    rpc_port: u16,
) -> Vec<String> {
    let forwarding = request.host_address.as_deref().and_then(|address| {
        address
            .parse::<Ipv4Addr>()
            .ok()
            .map(|remote| (0u16, remote))
    });
    let mut arguments = vec![
        "--no-tun".into(),
        "true".into(),
        "--use-smoltcp".into(),
        "true".into(),
        "--ipv4".into(),
        ip.to_string(),
        "--network-name".into(),
        request.network_name(),
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
    match forwarding {
        Some((_, remote)) => arguments.extend([
            "--port-forward".into(),
            format!("tcp://127.0.0.1:0/{remote}:{}", request.game_port),
        ]),
        None => arguments.extend([
            "--port-forward".into(),
            format!("tcp://127.0.0.1:0/{}:{}", ip, request.game_port),
        ]),
    }
    arguments
}

/// The address a guest must reach for this room, as shown to the host.
pub fn host_address(ip: Ipv4Addr, game_port: u16) -> String {
    format!("{ip}:{game_port}")
}

/// Runs the room until cancelled, mirroring the lobby lifecycle minus signaling.
pub async fn run(
    generation: u64,
    request: DirectRequest,
    executable: &Path,
    cancellation: &CancellationToken,
    mut ready: Option<tokio::sync::oneshot::Sender<eyre::Result<()>>>,
) -> eyre::Result<()> {
    update(generation, |state| state.status = Status::Connecting).await;

    let identity = super::protocol::Identity::generate()?;
    let ip = identity.ip;
    let joining = request.host_address.is_some();

    // The host owns the LAN port; a closed port is a warning, never a failed
    // room, because players routinely open the world after creating the room.
    if !joining
        && TcpStream::connect((Ipv4Addr::LOCALHOST, request.game_port))
            .await
            .is_err()
    {
        update(generation, |state| {
            state.host_hint = Some(HostHint::GamePortClosed);
        })
        .await;
        let game_port = request.game_port;
        tokio::spawn(async move {
            for _ in 0..120 {
                tokio::time::sleep(Duration::from_secs(3)).await;
                if TcpStream::connect((Ipv4Addr::LOCALHOST, game_port))
                    .await
                    .is_ok()
                {
                    update(generation, |state| state.host_hint = None).await;
                    return;
                }
            }
        });
    }

    let rpc_port = runtime::free_port().await?;
    let mut child = binary::hidden_command(executable)
        .args(arguments(&request, ip, rpc_port))
        .env("ET_NETWORK_SECRET", &request.password)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .wrap_err("could not start EasyTier")?;

    update(generation, |state| {
        state.status = if joining {
            Status::Joined
        } else {
            Status::Hosting
        };
        state.mode = Some(request.as_start_request().mode);
        state.lobby_name = Some(request.room_name.clone());
        state.virtual_ip = Some(ip);
        state.game_port = Some(request.game_port);
        // A direct room is never listed, so it must not claim to be public.
        state.is_public = false;
        state.publish_outcome = PublishOutcome::Private;
        state.connect_address = Some(host_address(ip, request.game_port));
        state.players.clear();
    })
    .await;
    if let Some(ready) = ready.take() {
        let _ = ready.send(Ok(()));
    }

    let result = tokio::select! {
        biased;
        _ = cancellation.cancelled() => Ok(()),
        exit = child.wait() => match exit {
            Ok(status) => Err(eyre::eyre!(
                "the EasyTier process exited with {status}"
            )),
            Err(error) => Err(eyre::eyre!(
                "could not wait for the EasyTier process: {error}"
            )),
        },
    };

    let _ = child.kill().await;
    update(generation, |state| {
        state.status = Status::Idle;
        state.host_hint = None;
    })
    .await;
    result
}
