use async_tungstenite::{
    WebSocketStream,
    tokio::{ConnectStream, connect_async},
    tungstenite::Message,
};
use base64::{Engine, engine::general_purpose::STANDARD};
use eyre::{Context, bail};
use futures::StreamExt;
use p256::{
    ecdsa::{Signature, SigningKey, signature::Signer},
    pkcs8::EncodePublicKey,
};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::{net::Ipv4Addr, time::Duration};

use super::{Player, StartRequest};

const MAX_SIGNAL_FRAME_BYTES: usize = 128 * 1024;
const MAX_SDP_BYTES: usize = 64 * 1024;
const MAX_ICE_CANDIDATE_BYTES: usize = 4 * 1024;

// This is the implemented MCTier v3 compatibility level, independently of the
// MC-Trin application version. The latter is sent as separate metadata.
pub const MCTIER_COMPATIBILITY_VERSION: &str = "3.8.0";
pub type Socket = WebSocketStream<ConnectStream>;

pub struct Identity {
    key: SigningKey,
    pub id: String,
    public_key: String,
    pub ip: Ipv4Addr,
    address_start: usize,
    address_step: usize,
}

impl Identity {
    pub fn generate() -> eyre::Result<Self> {
        let key = SigningKey::random(&mut rand::rngs::OsRng);
        Self::from_key(key)
    }

    fn from_key(key: SigningKey) -> eyre::Result<Self> {
        let der = key.verifying_key().to_public_key_der()?;
        let digest = Sha256::digest(der.as_bytes());
        let id = digest.iter().map(|byte| format!("{byte:02x}")).collect();
        // The signaling service accepts only 10.126.126.1-254. Use the full
        // identity digest to choose a bounded permutation of that pool, rather
        // than truncating to two bytes and silently reusing addresses.
        let address_start = address_slot(digest.as_slice());
        let mut step_hash = Sha256::new();
        step_hash.update(digest);
        step_hash.update(b"MC-TRIN-EASYTIER-ADDRESS-STEP");
        let address_step =
            usable_address_step(address_slot(&step_hash.finalize()));
        let ip = candidate_address(address_start, address_step, 0).unwrap();
        Ok(Self {
            key,
            id,
            public_key: STANDARD.encode(der.as_bytes()),
            ip,
            address_start,
            address_step,
        })
    }

    /// Return one identity-specific candidate. The coprime step visits every
    /// usable server address exactly once before reporting pool exhaustion.
    pub fn candidate_ip(&self, attempt: usize) -> Option<Ipv4Addr> {
        candidate_address(self.address_start, self.address_step, attempt)
    }

    pub fn registration(
        &self,
        challenge: &str,
        request: &StartRequest,
    ) -> eyre::Result<Value> {
        if challenge.len() != 64
            || !challenge.bytes().all(|byte| {
                byte.is_ascii_hexdigit() && !byte.is_ascii_uppercase()
            })
        {
            bail!("signaling returned an invalid challenge");
        }
        let canonical = format!(
            "MCTIER-SIGNALING-V3\n3\n{challenge}\n{}\n{}",
            request.lobby_name, self.ip
        );
        let signature: Signature = self.key.sign(canonical.as_bytes());
        Ok(json!({
            "type": "register-v3", "protocolVersion": 3,
            "identityPublicKey": self.public_key,
            "challengeSignature": STANDARD.encode(signature.to_der().as_bytes()),
            "playerName": request.player_name, "virtualIp": self.ip,
            "lobbyName": request.lobby_name, "lobbyPassword": request.password,
            "entryMode": request.mode, "useDomain": false,
            "clientVersion": MCTIER_COMPATIBILITY_VERSION,
            "applicationName": "MC-Trin", "applicationVersion": env!("CARGO_PKG_VERSION"),
        }))
    }
}

const ADDRESS_PREFIX: [u8; 3] = [10, 126, 126];

fn address_at(index: usize) -> Ipv4Addr {
    debug_assert!(index < 254);
    Ipv4Addr::new(
        ADDRESS_PREFIX[0],
        ADDRESS_PREFIX[1],
        ADDRESS_PREFIX[2],
        (index + 1) as u8,
    )
}

fn address_slot(bytes: &[u8]) -> usize {
    bytes.iter().fold(0usize, |value, byte| {
        (value * 256 + usize::from(*byte)) % 254
    })
}

fn greatest_common_divisor(mut left: usize, mut right: usize) -> usize {
    while right != 0 {
        (left, right) = (right, left % right);
    }
    left
}

fn usable_address_step(mut step: usize) -> usize {
    while greatest_common_divisor(step, 254) != 1 {
        step = (step + 1) % 254;
    }
    step.max(1)
}

fn candidate_address(
    start: usize,
    step: usize,
    attempt: usize,
) -> Option<Ipv4Addr> {
    if attempt >= 254 {
        return None;
    }
    Some(address_at((start + attempt * step) % 254))
}

#[derive(Debug, thiserror::Error)]
#[error("virtual address already in use")]
pub struct VirtualAddressInUse;

pub struct Registered {
    pub socket: Socket,
    pub host_id: String,
    pub players: Vec<Player>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashSet;

    #[test]
    fn colliding_initial_addresses_have_identity_specific_retry_candidates() {
        let start = 42;
        let first_step = usable_address_step(1);
        let second_step = usable_address_step(3);
        assert_eq!(
            candidate_address(start, first_step, 0),
            candidate_address(start, second_step, 0)
        );
        assert_ne!(
            candidate_address(start, first_step, 1),
            candidate_address(start, second_step, 1)
        );
    }

    #[test]
    fn a_probe_permutation_visits_all_server_addresses_then_exhausts() {
        let mut addresses = HashSet::new();
        for attempt in 0..254 {
            let address =
                candidate_address(17, usable_address_step(0), attempt).unwrap();
            assert_eq!(address.octets()[..3], ADDRESS_PREFIX);
            assert!((1..=254).contains(&address.octets()[3]));
            assert!(addresses.insert(address));
        }
        assert_eq!(addresses.len(), 254);
        assert!(candidate_address(17, usable_address_step(0), 254).is_none());
    }

    fn peer(id: &str) -> Player {
        Player {
            player_id: id.into(),
            player_name: "Peer".into(),
            virtual_ip: Some(Ipv4Addr::new(10, 126, 126, 2)),
            session_generation: 1,
        }
    }

    #[test]
    fn outgoing_voice_signals_inject_identity_and_require_a_roster_target() {
        let local_id = "a".repeat(64);
        let remote_id = "b".repeat(64);
        let players = vec![peer(&local_id), peer(&remote_id)];
        let message = outgoing_voice_signal(
            &json!({
                "type":"offer",
                "to":remote_id,
                "offer":{"type":"offer", "sdp":"v=0\r\n"}
            }),
            &local_id,
            &players,
        )
        .unwrap();
        assert_eq!(message["from"], local_id);
        assert_eq!(message["to"], remote_id);
        assert_eq!(message["offer"]["type"], "offer");

        let spoofed = json!({
            "type":"answer",
            "from":"c".repeat(64),
            "to":remote_id,
            "answer":{"type":"answer", "sdp":"v=0\r\n"}
        });
        assert!(outgoing_voice_signal(&spoofed, &local_id, &players).is_err());

        let outsider = json!({
            "type":"ice-candidate",
            "to":"c".repeat(64),
            "candidate":{"candidate":"candidate:1 1 UDP 1 10.0.0.1 1234 typ host"}
        });
        assert!(outgoing_voice_signal(&outsider, &local_id, &players).is_err());
    }

    #[test]
    fn incoming_voice_signals_are_scoped_to_local_identity_and_room_members() {
        let local_id = "a".repeat(64);
        let remote_id = "b".repeat(64);
        let players = vec![peer(&local_id), peer(&remote_id)];
        let incoming = json!({
            "type":"offer",
            "from":remote_id,
            "to":local_id,
            "playerName":"Peer",
            "offer":{"type":"offer", "sdp":"v=0\r\n"}
        });
        let checked =
            incoming_voice_signal(&incoming, &local_id, &players).unwrap();
        assert_eq!(checked["from"], remote_id);
        assert_eq!(checked["to"], local_id);
        assert!(checked.get("playerName").is_none());

        let wrong_recipient = json!({
            "type":"answer",
            "from":remote_id,
            "to":"c".repeat(64),
            "answer":{"type":"answer", "sdp":"v=0\r\n"}
        });
        assert!(
            incoming_voice_signal(&wrong_recipient, &local_id, &players)
                .is_err()
        );

        let outsider = json!({
            "type":"answer",
            "from":"c".repeat(64),
            "to":local_id,
            "answer":{"type":"answer", "sdp":"v=0\r\n"}
        });
        assert!(incoming_voice_signal(&outsider, &local_id, &players).is_err());
    }

    #[test]
    fn voice_signal_validation_rejects_oversized_and_malformed_payloads() {
        let local_id = "a".repeat(64);
        let remote_id = "b".repeat(64);
        let players = vec![peer(&remote_id)];
        let oversized_sdp = json!({
            "type":"offer",
            "to":remote_id,
            "offer":{"type":"offer", "sdp":"x".repeat(MAX_SDP_BYTES + 1)}
        });
        assert!(
            outgoing_voice_signal(&oversized_sdp, &local_id, &players).is_err()
        );

        let bad_candidate = json!({
            "type":"ice-candidate",
            "to":remote_id,
            "candidate":{"candidate":"not-a-candidate"}
        });
        assert!(
            outgoing_voice_signal(&bad_candidate, &local_id, &players).is_err()
        );

        let unsupported = json!({"type":"ping", "to":remote_id});
        assert!(
            outgoing_voice_signal(&unsupported, &local_id, &players).is_err()
        );
    }
}

pub fn parse_players(value: &Value) -> eyre::Result<Vec<Player>> {
    let list =
        value
            .get("players")
            .and_then(Value::as_array)
            .ok_or_else(|| {
                eyre::eyre!("signaling returned an invalid player list")
            })?;
    if list.len() > 512 {
        bail!("player list exceeds the supported size");
    }
    list.iter()
        .map(|item| {
            let player: Player = serde_json::from_value(item.clone())?;
            if player.player_id.len() != 64
                || !player
                    .player_id
                    .bytes()
                    .all(|byte| byte.is_ascii_hexdigit())
                || player.player_name.is_empty()
                || player.player_name.chars().count() > 64
                || player.player_name.chars().any(char::is_control)
                || player.session_generation == 0
                || player
                    .virtual_ip
                    .is_some_and(|ip| !ip.is_private() || ip.is_loopback())
            {
                bail!("signaling returned invalid player information");
            }
            Ok(player)
        })
        .collect()
}

/// Shown verbatim in the room UI when the signaling link dies, so it must name
/// the cause and the way back in rather than sounding like a dead button.
pub const SIGNALING_DISCONNECTED: &str = "Lost the connection to the MCTier signaling server. The room has ended - check your connection and start or join the room again (the default server is wss://mctier.pmhs.top/signaling; a self-hosted or alternate address can be set in the room request).";

/// Reads the public directory through a short-lived connection that never
/// registers an identity.
///
/// Publishing is otherwise one-way: the host asks to be listed and has no way
/// to learn whether the room actually reached the directory, which is how a
/// room can look published while the lobby list stays empty. Asking the
/// directory directly turns that guess into an answer.
pub async fn public_lobby_names(
    signaling_server: &str,
    timeout: Duration,
) -> eyre::Result<Vec<String>> {
    let read = async {
        let (mut socket, _) = connect_async(signaling_server).await?;
        socket
            .send(Message::Text(
                json!({ "type": "public-lobby-list-request" })
                    .to_string()
                    .into(),
            ))
            .await?;
        loop {
            let message = next_message(&mut socket).await?;
            if message["type"] != "public-lobby-list-response" {
                continue;
            }
            let mut names: Vec<String> = Vec::new();
            if let Some(lobbies) = message["lobbies"].as_array() {
                for lobby in lobbies {
                    if let Some(name) = lobby["lobbyName"].as_str() {
                        names.push(name.to_string());
                    }
                }
            }
            return Ok::<Vec<String>, eyre::Report>(names);
        }
    };
    tokio::time::timeout(timeout, read)
        .await
        .wrap_err("the lobby directory did not answer in time")?
}

pub async fn next_message(socket: &mut Socket) -> eyre::Result<Value> {
    loop {
        let message = socket
            .next()
            .await
            .ok_or_else(|| eyre::eyre!(SIGNALING_DISCONNECTED))??;
        match message {
            Message::Text(text) => {
                if text.len() > MAX_SIGNAL_FRAME_BYTES {
                    bail!("signaling frame is too large");
                }
                return serde_json::from_str(text.as_ref())
                    .wrap_err("invalid signaling message");
            }
            Message::Binary(bytes) if bytes.len() > MAX_SIGNAL_FRAME_BYTES => {
                bail!("signaling frame is too large");
            }
            Message::Close(_) => bail!(SIGNALING_DISCONNECTED),
            Message::Ping(data) => socket.send(Message::Pong(data)).await?,
            _ => {}
        }
    }
}

/// Validate a renderer-originated voice message and add the authenticated
/// local identity before it reaches the signaling server.
pub fn outgoing_voice_signal(
    signal: &Value,
    local_player_id: &str,
    players: &[Player],
) -> eyre::Result<Value> {
    if signal.as_object().is_none() {
        bail!("invalid voice signal");
    }
    reject_unknown_fields(
        signal,
        &["type", "to", "offer", "answer", "candidate"],
    )?;
    let kind = required_string(signal, "type")?;
    let to = required_string(signal, "to")?;
    validate_player_id(to)?;
    if to == local_player_id
        || !players.iter().any(|player| player.player_id == to)
    {
        bail!("voice signal target is not in the active lobby");
    }
    let message = match kind {
        "offer" => {
            reject_unknown_fields(signal, &["type", "to", "offer"])?;
            let offer = validate_description(
                signal
                    .get("offer")
                    .ok_or_else(|| eyre::eyre!("missing offer"))?,
                "offer",
            )?;
            json!({"type":"offer", "from":local_player_id, "to":to, "offer":offer})
        }
        "answer" => {
            reject_unknown_fields(signal, &["type", "to", "answer"])?;
            let answer = validate_description(
                signal
                    .get("answer")
                    .ok_or_else(|| eyre::eyre!("missing answer"))?,
                "answer",
            )?;
            json!({"type":"answer", "from":local_player_id, "to":to, "answer":answer})
        }
        "ice-candidate" => {
            reject_unknown_fields(signal, &["type", "to", "candidate"])?;
            let candidate = validate_candidate(
                signal
                    .get("candidate")
                    .ok_or_else(|| eyre::eyre!("missing candidate"))?,
            )?;
            json!({"type":"ice-candidate", "from":local_player_id, "to":to, "candidate":candidate})
        }
        _ => bail!("unsupported voice signal type"),
    };
    if serde_json::to_vec(&message)?.len() > MAX_SIGNAL_FRAME_BYTES {
        bail!("voice signal is too large");
    }
    Ok(message)
}

/// Validate signaling-server messages before exposing them to the renderer.
pub fn incoming_voice_signal(
    message: &Value,
    local_player_id: &str,
    players: &[Player],
) -> eyre::Result<Value> {
    if message.as_object().is_none() {
        bail!("invalid voice signal");
    }
    let kind = required_string(message, "type")?;
    let from = required_string(message, "from")?;
    let to = required_string(message, "to")?;
    validate_player_id(from)?;
    if to != local_player_id
        || from == local_player_id
        || !players.iter().any(|player| player.player_id == from)
    {
        bail!("voice signal sender is not in the active lobby");
    }
    let validated = match kind {
        "offer" => {
            reject_unknown_fields(
                message,
                &["type", "from", "to", "offer", "playerName"],
            )?;
            if let Some(player_name) = message.get("playerName") {
                validate_player_name(player_name)?;
            }
            let offer = validate_description(
                message
                    .get("offer")
                    .ok_or_else(|| eyre::eyre!("missing offer"))?,
                "offer",
            )?;
            json!({"type":"offer", "from":from, "to":to, "offer":offer})
        }
        "answer" => {
            reject_unknown_fields(message, &["type", "from", "to", "answer"])?;
            let answer = validate_description(
                message
                    .get("answer")
                    .ok_or_else(|| eyre::eyre!("missing answer"))?,
                "answer",
            )?;
            json!({"type":"answer", "from":from, "to":to, "answer":answer})
        }
        "ice-candidate" => {
            reject_unknown_fields(
                message,
                &["type", "from", "to", "candidate"],
            )?;
            let candidate = validate_candidate(
                message
                    .get("candidate")
                    .ok_or_else(|| eyre::eyre!("missing candidate"))?,
            )?;
            json!({"type":"ice-candidate", "from":from, "to":to, "candidate":candidate})
        }
        _ => bail!("unsupported voice signal type"),
    };
    if serde_json::to_vec(&validated)?.len() > MAX_SIGNAL_FRAME_BYTES {
        bail!("voice signal is too large");
    }
    Ok(validated)
}

fn required_string<'a>(value: &'a Value, field: &str) -> eyre::Result<&'a str> {
    value
        .get(field)
        .and_then(Value::as_str)
        .ok_or_else(|| eyre::eyre!("invalid voice signal field: {field}"))
}

fn reject_unknown_fields(value: &Value, allowed: &[&str]) -> eyre::Result<()> {
    let object = value
        .as_object()
        .ok_or_else(|| eyre::eyre!("invalid voice signal"))?;
    if object
        .keys()
        .any(|field| !allowed.contains(&field.as_str()))
    {
        bail!("voice signal contains unsupported fields");
    }
    Ok(())
}

fn validate_player_id(value: &str) -> eyre::Result<()> {
    if value.len() != 64 || !value.bytes().all(|byte| byte.is_ascii_hexdigit())
    {
        bail!("invalid voice signal player id");
    }
    Ok(())
}

fn validate_player_name(value: &Value) -> eyre::Result<()> {
    let name = value
        .as_str()
        .ok_or_else(|| eyre::eyre!("invalid voice signal player name"))?;
    if name.is_empty()
        || name.chars().count() > 64
        || name.chars().any(char::is_control)
    {
        bail!("invalid voice signal player name");
    }
    Ok(())
}

fn validate_description(
    value: &Value,
    expected_type: &str,
) -> eyre::Result<Value> {
    reject_unknown_fields(value, &["type", "sdp"])?;
    if required_string(value, "type")? != expected_type {
        bail!("voice signal description type does not match");
    }
    let sdp = required_string(value, "sdp")?;
    if sdp.is_empty() || sdp.len() > MAX_SDP_BYTES || !sdp.is_ascii() {
        bail!("invalid or oversized voice SDP");
    }
    Ok(json!({"type":expected_type, "sdp":sdp}))
}

fn validate_candidate(value: &Value) -> eyre::Result<Value> {
    reject_unknown_fields(
        value,
        &["candidate", "sdpMid", "sdpMLineIndex", "usernameFragment"],
    )?;
    let candidate = required_string(value, "candidate")?;
    if candidate.len() > MAX_ICE_CANDIDATE_BYTES
        || !candidate.is_ascii()
        || (!candidate.is_empty() && !candidate.starts_with("candidate:"))
    {
        bail!("invalid or oversized ICE candidate");
    }
    let sdp_mid = match value.get("sdpMid") {
        Some(Value::Null) | None => None,
        Some(Value::String(mid))
            if mid.len() <= 256 && !mid.chars().any(char::is_control) =>
        {
            Some(mid.as_str())
        }
        _ => bail!("invalid ICE candidate media id"),
    };
    let line_index = match value.get("sdpMLineIndex") {
        Some(Value::Null) | None => None,
        Some(value) => Some(
            value
                .as_u64()
                .filter(|index| *index <= u16::MAX as u64)
                .ok_or_else(|| {
                    eyre::eyre!("invalid ICE candidate line index")
                })? as u16,
        ),
    };
    let username_fragment = match value.get("usernameFragment") {
        Some(Value::Null) | None => None,
        Some(Value::String(fragment))
            if fragment.len() <= 256
                && !fragment.chars().any(char::is_control) =>
        {
            Some(fragment.as_str())
        }
        _ => bail!("invalid ICE username fragment"),
    };
    let mut result = json!({"candidate":candidate});
    if let Some(mid) = sdp_mid {
        result["sdpMid"] = json!(mid);
    }
    if let Some(index) = line_index {
        result["sdpMLineIndex"] = json!(index);
    }
    if let Some(fragment) = username_fragment {
        result["usernameFragment"] = json!(fragment);
    }
    Ok(result)
}

pub async fn register(
    request: &StartRequest,
    identity: &Identity,
) -> Result<Registered, RegistrationError> {
    let attempt = async {
        let (mut socket, _) = connect_async(&request.signaling_server).await?;
        let challenge = next_message(&mut socket).await?;
        if challenge["type"] != "server-challenge"
            || challenge["protocolVersion"] != 3
            || challenge["lobbyEntryModes"] != true
        {
            bail!("signaling does not support the required lobby protocol");
        }
        let proof = identity.registration(
            challenge["challenge"].as_str().unwrap_or_default(),
            request,
        )?;
        socket.send(Message::Text(proof.to_string().into())).await?;
        let accepted = next_message(&mut socket).await?;
        if matches!(
            accepted["type"].as_str(),
            Some("register-error" | "version-too-old")
        ) {
            // Never surface server-provided links, HTML, or unbounded exception text.
            let detail = accepted["message"]
                .as_str()
                .unwrap_or("registration rejected");
            let safe: String = detail
                .chars()
                .filter(|c| !c.is_control())
                .take(256)
                .collect();
            if safe == "virtualIp 已被大厅内其他成员使用" {
                return Err(eyre::Report::new(VirtualAddressInUse));
            }
            bail!("lobby registration rejected: {safe}");
        }
        if accepted["type"] != "register-success"
            || accepted["clientId"] != identity.id
            || accepted["sessionGeneration"].as_u64().unwrap_or(0) == 0
        {
            bail!("signaling did not confirm the registered identity");
        }
        let host_id = accepted["hostId"]
            .as_str()
            .ok_or_else(|| eyre::eyre!("lobby has no authoritative host"))?
            .to_string();
        socket
            .send(Message::Text(
                json!({ "type": "players-list-request" }).to_string().into(),
            ))
            .await?;
        loop {
            let roster = next_message(&mut socket).await?;
            if roster["type"] == "players-list" {
                let players = parse_players(&roster)?;
                return Ok(Registered {
                    socket,
                    host_id,
                    players,
                });
            }
            if matches!(
                roster["type"].as_str(),
                Some("register-error" | "kicked")
            ) {
                bail!("lobby registration was revoked");
            }
        }
    };
    match tokio::time::timeout(Duration::from_secs(15), attempt).await {
        Ok(Ok(registered)) => Ok(registered),
        Ok(Err(error))
            if error.downcast_ref::<VirtualAddressInUse>().is_some() =>
        {
            Err(RegistrationError::AddressInUse)
        }
        Ok(Err(error)) => Err(RegistrationError::Failed(error)),
        Err(_) => Err(RegistrationError::Failed(eyre::eyre!(
            "lobby registration timed out"
        ))),
    }
}

#[derive(Debug, thiserror::Error)]
pub enum RegistrationError {
    #[error("virtual address already in use")]
    AddressInUse,
    #[error(transparent)]
    Failed(#[from] eyre::Report),
}
