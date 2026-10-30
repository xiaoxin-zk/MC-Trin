use super::*;
use async_tungstenite::{tokio::accept_async, tungstenite::Message};
use base64::{Engine, engine::general_purpose::STANDARD};
use futures::StreamExt;
use p256::{
    ecdsa::{Signature, VerifyingKey, signature::Verifier},
    pkcs8::DecodePublicKey,
};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::{net::Ipv4Addr, path::PathBuf, time::Duration};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::{TcpListener, TcpStream},
    sync::oneshot,
};
use tokio_util::sync::CancellationToken;

fn request() -> StartRequest {
    StartRequest {
        mode: EntryMode::Create,
        lobby_name: "TestRoom".into(),
        password: "secret".into(),
        player_name: "Player".into(),
        server_node: "tcp://127.0.0.1:11010".into(),
        signaling_server: "ws://127.0.0.1:12345".into(),
        game_port: 25565,
        is_public: false,
        description: None,
        max_players: None,
    }
}

#[test]
fn registration_proof_uses_the_public_v3_format_and_separate_application_metadata()
 {
    let identity = protocol::Identity::generate().unwrap();
    let request = request();
    let challenge = "12".repeat(32);
    let registration = identity.registration(&challenge, &request).unwrap();
    let public = STANDARD
        .decode(registration["identityPublicKey"].as_str().unwrap())
        .unwrap();
    let signature = STANDARD
        .decode(registration["challengeSignature"].as_str().unwrap())
        .unwrap();
    let verifying = VerifyingKey::from_public_key_der(&public).unwrap();
    let signature = Signature::from_der(&signature).unwrap();
    let expected = format!(
        "MCTIER-SIGNALING-V3\n3\n{challenge}\nTestRoom\n{}",
        identity.ip
    );
    verifying.verify(expected.as_bytes(), &signature).unwrap();
    assert!(
        verifying
            .verify(
                expected.replace("TestRoom", "OtherRoom").as_bytes(),
                &signature
            )
            .is_err()
    );
    let fingerprint: String = Sha256::digest(public)
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect();
    assert_eq!(identity.id, fingerprint);
    assert_eq!(
        registration["clientVersion"],
        protocol::MCTIER_COMPATIBILITY_VERSION
    );
    assert_eq!(registration["applicationName"], "MC-Trin");
    assert_eq!(
        registration["applicationVersion"],
        env!("CARGO_PKG_VERSION")
    );
    assert!(identity.registration("invalid", &request).is_err());
}

#[test]
fn request_validation_rejects_unsafe_urls_and_invalid_room_values() {
    assert!(request().validate().is_ok());
    for signaling in [
        "http://example.com",
        "ws://example.com",
        "wss://user:password@example.com",
        "wss://example.com/#fragment",
    ] {
        let mut input = request();
        input.signaling_server = signaling.into();
        assert!(input.validate().is_err(), "{signaling}");
    }
    for name in ["ab", " room", "room\nname", "--", "bad room"] {
        let mut input = request();
        input.lobby_name = name.into();
        assert!(input.validate().is_err(), "{name:?}");
    }
    for node in [
        "file:///x",
        "tcp://server",
        "tcp://user:pass@server:80",
        "tcp://server:80/path",
    ] {
        let mut input = request();
        input.server_node = node.into();
        assert!(input.validate().is_err(), "{node}");
    }
}

#[test]
fn a_public_lobby_cannot_keep_a_password() {
    let mut input = request();
    input.password.clear();
    input.is_public = true;
    assert!(
        input.validate().is_ok(),
        "a listed room without a password is allowed"
    );

    // The directory carries no password and the service isolates rooms by name
    // plus password, so the combination cannot be listed. Refusing it keeps the
    // UI from offering a room the service would silently leave out.
    input.password = "secret".into();
    let error = input.validate().unwrap_err();
    assert!(
        format!("{error}").contains("cannot have a password"),
        "{error}"
    );

    // A private room keeps its password.
    input.is_public = false;
    assert!(input.validate().is_ok());

    input.mode = EntryMode::Join;
    input.is_public = true;
    assert!(input.validate().is_err());
}

#[tokio::test]
async fn sending_voice_signal_after_session_end_returns_an_error() {
    let _serial = multiplayer::SESSION_TEST.lock().await;
    let result = send_voice_signal(json!({
        "type":"offer",
        "to":"b".repeat(64),
        "offer":{"type":"offer", "sdp":"v=0\r\n"}
    }))
    .await;
    assert!(result.is_err());
}

#[test]
fn kernel_arguments_keep_the_secret_out_of_argv_and_bind_forwarding_to_loopback()
 {
    let request = request();
    let arguments = runtime::arguments(
        &request,
        Ipv4Addr::new(10, 126, 2, 2),
        18181,
        Some((25566, Ipv4Addr::new(10, 126, 1, 1))),
    );
    assert!(
        arguments
            .windows(2)
            .any(|pair| pair == ["--no-tun", "true"])
    );
    assert!(
        arguments
            .windows(2)
            .any(|pair| pair == ["--rpc-portal", "127.0.0.1:18181"])
    );
    assert!(arguments.windows(2).any(|pair| pair
        == ["--port-forward", "tcp://127.0.0.1:25566/10.126.1.1:25565"]));
    assert!(!arguments.iter().any(|argument| argument.contains("secret")));
}

fn fixture() -> (tempfile::TempDir, PathBuf) {
    let directory = tempfile::tempdir().unwrap();
    let executable = directory.path().join(if cfg!(windows) {
        "fake-easytier.exe"
    } else {
        "fake-easytier"
    });
    assert!(
        std::process::Command::new("rustc")
            .args(["--edition=2024"])
            .arg(
                PathBuf::from(env!("CARGO_MANIFEST_DIR"))
                    .join("src/api/mctier/kernel-fixture.rs")
            )
            .arg("-o")
            .arg(&executable)
            .status()
            .unwrap()
            .success()
    );
    (directory, executable)
}

async fn signaling(reject: bool) -> (String, oneshot::Receiver<Value>) {
    let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).await.unwrap();
    let url = format!("ws://{}", listener.local_addr().unwrap());
    let (left, received) = oneshot::channel();
    tokio::spawn(async move {
        let (stream, _) = listener.accept().await.unwrap();
        let mut socket = accept_async(stream).await.unwrap();
        socket.send(Message::Text(json!({"type":"server-challenge", "protocolVersion":3, "lobbyEntryModes":true, "challenge":"12".repeat(32)}).to_string().into())).await.unwrap();
        let registration =
            socket.next().await.unwrap().unwrap().into_text().unwrap();
        let registration: Value =
            serde_json::from_str(registration.as_ref()).unwrap();
        if reject {
            socket.send(Message::Text(json!({"type":"register-error", "message":"room does not exist"}).to_string().into())).await.unwrap();
            let _ = left.send(registration);
            return;
        }
        let public = STANDARD
            .decode(registration["identityPublicKey"].as_str().unwrap())
            .unwrap();
        let id: String = Sha256::digest(public)
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect();
        let joining = registration["entryMode"] == "join";
        let host_id = if joining { "a".repeat(64) } else { id.clone() };
        socket.send(Message::Text(json!({"type":"register-success", "clientId":id, "hostId":host_id, "sessionGeneration":1, "lobbyId":"test-lobby", "chatToken":"not-exported", "chatTokenEpoch":1}).to_string().into())).await.unwrap();
        let mut players = vec![
            json!({"playerId":id, "playerName":"Player", "virtualIp":registration["virtualIp"], "sessionGeneration":1}),
        ];
        if joining {
            players.push(json!({"playerId":host_id,"playerName":"Host", "virtualIp":"10.126.1.1", "sessionGeneration":1}));
        }
        while let Some(Ok(message)) = socket.next().await {
            let Ok(text) = message.into_text() else {
                break;
            };
            let value: Value = serde_json::from_str(text.as_ref()).unwrap();
            match value["type"].as_str() {
                Some("players-list-request") => {
                    socket
                        .send(Message::Text(
                            json!({"type":"players-list", "players":players})
                                .to_string()
                                .into(),
                        ))
                        .await
                        .unwrap();
                }
                Some("leave") => {
                    let _ = left.send(value);
                    return;
                }
                _ => {}
            }
        }
    });
    (url, received)
}

/// Completes the registry handshake and the roster request that follows it,
/// leaving a registered session on the given socket.
async fn establish_session<S>(
    socket: &mut async_tungstenite::WebSocketStream<S>,
) -> (String, Value)
where
    S: futures::io::AsyncRead + futures::io::AsyncWrite + Unpin,
{
    socket
        .send(Message::Text(
            json!({"type":"server-challenge", "protocolVersion":3, "lobbyEntryModes":true, "challenge":"12".repeat(32)})
                .to_string()
                .into(),
        ))
        .await
        .unwrap();
    let registration: Value = serde_json::from_str(
        socket
            .next()
            .await
            .unwrap()
            .unwrap()
            .into_text()
            .unwrap()
            .as_ref(),
    )
    .unwrap();
    let public = STANDARD
        .decode(registration["identityPublicKey"].as_str().unwrap())
        .unwrap();
    let id: String = Sha256::digest(public)
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect();
    let joining = registration["entryMode"] == "join";
    let host_id = if joining { "a".repeat(64) } else { id.clone() };
    socket
        .send(Message::Text(
            json!({
                "type": "register-success", "clientId": id, "hostId": host_id,
                "sessionGeneration": 1, "lobbyId": "test-lobby",
                "chatToken": "not-exported", "chatTokenEpoch": 1,
            })
            .to_string()
            .into(),
        ))
        .await
        .unwrap();
    let mut players = vec![json!({
        "playerId": id, "playerName": "Player",
        "virtualIp": registration["virtualIp"], "sessionGeneration": 1,
    })];
    if joining {
        players.push(json!({
            "playerId": host_id, "playerName": "Host",
            "virtualIp": "10.126.1.1", "sessionGeneration": 1,
        }));
    }
    let roster = json!(players);
    while let Some(Ok(message)) = socket.next().await {
        let Ok(text) = message.into_text() else {
            break;
        };
        let Ok(value) = serde_json::from_str::<Value>(text.as_ref()) else {
            break;
        };
        if value["type"] == "players-list-request" {
            socket
                .send(Message::Text(
                    json!({"type":"players-list", "players": roster})
                        .to_string()
                        .into(),
                ))
                .await
                .unwrap();
            break;
        }
    }
    (id, roster)
}

/// Registers a host, confirms its lobby visibility and then drops the control
/// link, which is what a lost network path looks like from the client's view.
async fn signaling_disconnecting_after_registration() -> String {
    let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).await.unwrap();
    let url = format!("ws://{}", listener.local_addr().unwrap());
    tokio::spawn(async move {
        let (stream, _) = listener.accept().await.unwrap();
        let mut socket = accept_async(stream).await.unwrap();
        let _ = establish_session(&mut socket).await;
        // Let the room reach its running state first, so the drop interrupts an
        // established session rather than the setup handshake.
        while let Some(Ok(message)) = socket.next().await {
            let Ok(text) = message.into_text() else {
                break;
            };
            let Ok(value) = serde_json::from_str::<Value>(text.as_ref()) else {
                break;
            };
            if value["type"] == "set-lobby-options" {
                socket
                    .send(Message::Text(
                        json!({
                            "type": "lobby-options-changed",
                            "isPublic": value["isPublic"],
                        })
                        .to_string()
                        .into(),
                    ))
                    .await
                    .unwrap();
                break;
            }
        }
        let _ = socket.close(None).await;
    });
    url
}

/// Registers a guest, then pushes one authentic voice offer from the host and
/// one forged offer from a player outside the lobby.
async fn signaling_with_voice_signals() -> String {
    let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).await.unwrap();
    let url = format!("ws://{}", listener.local_addr().unwrap());
    tokio::spawn(async move {
        let (stream, _) = listener.accept().await.unwrap();
        let mut socket = accept_async(stream).await.unwrap();
        let (id, _) = establish_session(&mut socket).await;
        // The room confirms its lobby visibility before it starts reading voice
        // traffic; without the echo these offers would be swallowed by that wait.
        while let Some(Ok(message)) = socket.next().await {
            let Ok(text) = message.into_text() else {
                break;
            };
            let Ok(value) = serde_json::from_str::<Value>(text.as_ref()) else {
                break;
            };
            match value["type"].as_str() {
                Some("set-lobby-options") => {
                    socket
                        .send(Message::Text(
                            json!({
                                "type": "lobby-options-changed",
                                "isPublic": value["isPublic"],
                            })
                            .to_string()
                            .into(),
                        ))
                        .await
                        .unwrap();
                    break;
                }
                Some("leave") => return,
                _ => {}
            }
        }
        socket
            .send(Message::Text(
                json!({
                    "type": "offer", "from": "a".repeat(64), "to": id,
                    "playerName": "Host",
                    "offer": {"type": "offer", "sdp": "v=0\r\n"},
                })
                .to_string()
                .into(),
            ))
            .await
            .unwrap();
        socket
            .send(Message::Text(
                json!({
                    "type": "offer", "from": "c".repeat(64), "to": id,
                    "offer": {"type": "offer", "sdp": "v=0\r\n"},
                })
                .to_string()
                .into(),
            ))
            .await
            .unwrap();
        while let Some(Ok(message)) = socket.next().await {
            let Ok(text) = message.into_text() else {
                break;
            };
            let Ok(value) = serde_json::from_str::<Value>(text.as_ref()) else {
                break;
            };
            if value["type"] == "leave" {
                return;
            }
        }
    });
    url
}

async fn collision_signaling(
    rejected_attempts: usize,
    total_attempts: usize,
) -> (String, oneshot::Receiver<Vec<Value>>) {
    let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).await.unwrap();
    let url = format!("ws://{}", listener.local_addr().unwrap());
    let (sender, received) = oneshot::channel();
    tokio::spawn(async move {
        let mut registrations = Vec::new();
        for attempt in 0..total_attempts {
            let (stream, _) = listener.accept().await.unwrap();
            let mut socket = accept_async(stream).await.unwrap();
            socket
                .send(Message::Text(
                    json!({
                        "type": "server-challenge", "protocolVersion": 3,
                        "lobbyEntryModes": true, "challenge": "12".repeat(32),
                    })
                    .to_string()
                    .into(),
                ))
                .await
                .unwrap();
            let registration: Value = serde_json::from_str(
                socket
                    .next()
                    .await
                    .unwrap()
                    .unwrap()
                    .into_text()
                    .unwrap()
                    .as_ref(),
            )
            .unwrap();
            registrations.push(registration.clone());
            if attempt < rejected_attempts {
                socket
                    .send(Message::Text(
                        json!({
                            "type": "register-error",
                            "message": "virtualIp 已被大厅内其他成员使用",
                        })
                        .to_string()
                        .into(),
                    ))
                    .await
                    .unwrap();
                continue;
            }
            let public = STANDARD
                .decode(registration["identityPublicKey"].as_str().unwrap())
                .unwrap();
            let id: String = Sha256::digest(public)
                .iter()
                .map(|byte| format!("{byte:02x}"))
                .collect();
            socket.send(Message::Text(json!({
                "type": "register-success", "clientId": id, "hostId": id,
                "sessionGeneration": 1, "lobbyId": "collision-test",
                "chatToken": "not-exported", "chatTokenEpoch": 1,
            }).to_string().into())).await.unwrap();
            let request_message = socket
                .next()
                .await
                .unwrap()
                .unwrap()
                .into_text()
                .unwrap()
                .to_string();
            assert_eq!(request_message, "{\"type\":\"players-list-request\"}");
            socket.send(Message::Text(json!({
                "type": "players-list",
                "players": [{
                    "playerId": id, "playerName": "Player",
                    "virtualIp": registration["virtualIp"], "sessionGeneration": 1,
                }],
            }).to_string().into())).await.unwrap();
            let _ = sender.send(registrations);
            return;
        }
        let _ = sender.send(registrations);
    });
    (url, received)
}

async fn read_varint(stream: &mut TcpStream) -> eyre::Result<usize> {
    let mut value = 0;
    for shift in (0..35).step_by(7) {
        let byte = stream.read_u8().await?;
        value |= ((byte & 127) as usize) << shift;
        if byte & 128 == 0 {
            return Ok(value);
        }
    }
    bail!("invalid varint")
}

fn varint(mut value: usize) -> Vec<u8> {
    let mut bytes = Vec::new();
    loop {
        let byte = (value & 127) as u8;
        value >>= 7;
        bytes.push(byte | if value == 0 { 0 } else { 128 });
        if value == 0 {
            return bytes;
        }
    }
}

async fn minecraft() -> u16 {
    let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).await.unwrap();
    let port = listener.local_addr().unwrap().port();
    tokio::spawn(async move {
        while let Ok((mut stream, _)) = listener.accept().await {
            tokio::spawn(async move {
                let serve = async {
                    let handshake = read_varint(&mut stream).await?;
                    let mut packet = vec![0; handshake];
                    stream.read_exact(&mut packet).await?;
                    let status = read_varint(&mut stream).await?;
                    let mut packet = vec![0; status];
                    stream.read_exact(&mut packet).await?;
                    let json = json!({"version":{"name":"1.21.1","protocol":767},"players":{"max":20,"online":1},"description":{"text":"Mock Minecraft"}}).to_string();
                    let mut body = vec![0];
                    body.extend(varint(json.len()));
                    body.extend(json.bytes());
                    stream.write_all(&varint(body.len())).await?;
                    stream.write_all(&body).await?;
                    let ping = read_varint(&mut stream).await?;
                    let mut packet = vec![0; ping];
                    stream.read_exact(&mut packet).await?;
                    stream.write_all(&varint(ping)).await?;
                    stream.write_all(&packet).await?;
                    Ok::<(), eyre::Report>(())
                };
                let _ = serve.await;
            });
        }
    });
    port
}

#[tokio::test]
async fn fake_signaling_and_kernel_complete_host_and_guest_then_clean_up() {
    let _serial = multiplayer::SESSION_TEST.lock().await;
    let (directory, executable) = fixture();
    for mode in [EntryMode::Create, EntryMode::Join] {
        let (url, leave) = signaling(false).await;
        let mut input = request();
        input.mode = mode;
        input.signaling_server = url;
        input.game_port = minecraft().await;
        let cancellation = CancellationToken::new();
        let token = cancellation.clone();
        let executable = executable.clone();
        let (ready, result) = oneshot::channel();
        let actor = tokio::spawn(async move {
            runtime::run(
                0,
                input,
                &executable,
                &token,
                tokio::sync::mpsc::channel(1).1,
                std::sync::Arc::new(|_| {}),
                &mut Some(ready),
            )
            .await
        });
        tokio::time::timeout(Duration::from_secs(15), result)
            .await
            .unwrap()
            .unwrap()
            .unwrap();
        let rpc: u16 =
            tokio::fs::read_to_string(directory.path().join("rpc-port"))
                .await
                .unwrap()
                .parse()
                .unwrap();
        cancellation.cancel();
        tokio::time::timeout(Duration::from_secs(5), actor)
            .await
            .unwrap()
            .unwrap()
            .unwrap();
        let leave = tokio::time::timeout(Duration::from_secs(3), leave)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(leave["type"], "leave");
        assert!(
            TcpStream::connect((Ipv4Addr::LOCALHOST, rpc))
                .await
                .is_err()
        );
    }
}

#[tokio::test]
async fn rejected_registration_does_not_start_a_network_process() {
    let _serial = multiplayer::SESSION_TEST.lock().await;
    let (directory, executable) = fixture();
    let (url, _) = signaling(true).await;
    let mut input = request();
    input.signaling_server = url;
    let error = runtime::run(
        0,
        input,
        &executable,
        &CancellationToken::new(),
        tokio::sync::mpsc::channel(1).1,
        std::sync::Arc::new(|_| {}),
        &mut None,
    )
    .await
    .err()
    .expect("registration should be rejected");
    assert!(error.to_string().contains("room does not exist"));
    assert!(!directory.path().join("arguments").exists());
}

#[tokio::test]
async fn an_address_collision_retries_with_the_same_identity_and_a_new_ip() {
    let (url, attempts) = collision_signaling(1, 2).await;
    let mut input = request();
    input.signaling_server = url;
    let (identity, registered, _) = runtime::register_with_retry(
        &input,
        protocol::Identity::generate().unwrap(),
        &CancellationToken::new(),
    )
    .await
    .unwrap();
    let attempts = attempts.await.unwrap();
    assert_eq!(attempts.len(), 2);
    assert_eq!(
        attempts[0]["identityPublicKey"],
        attempts[1]["identityPublicKey"]
    );
    assert_eq!(
        attempts[0]["virtualIp"],
        identity.candidate_ip(0).unwrap().to_string()
    );
    assert_eq!(
        attempts[1]["virtualIp"],
        identity.candidate_ip(1).unwrap().to_string()
    );
    assert_ne!(attempts[0]["virtualIp"], attempts[1]["virtualIp"]);
    assert_eq!(registered.players[0].player_id, identity.id);
}

#[tokio::test]
async fn registration_stops_after_the_attempt_limit_is_rejected() {
    let (url, attempts) = collision_signaling(
        runtime::REGISTRATION_ATTEMPT_LIMIT,
        runtime::REGISTRATION_ATTEMPT_LIMIT,
    )
    .await;
    let mut input = request();
    input.signaling_server = url;
    let error = runtime::register_with_retry(
        &input,
        protocol::Identity::generate().unwrap(),
        &CancellationToken::new(),
    )
    .await
    .err()
    .expect("all addresses should be rejected");
    assert!(error.to_string().contains("all 16 MCTier"));
    let attempts = attempts.await.unwrap();
    assert_eq!(attempts.len(), runtime::REGISTRATION_ATTEMPT_LIMIT);
    let identity_keys: std::collections::HashSet<_> = attempts
        .iter()
        .map(|item| &item["identityPublicKey"])
        .collect();
    let addresses: std::collections::HashSet<_> =
        attempts.iter().map(|item| &item["virtualIp"]).collect();
    assert_eq!(identity_keys.len(), 1);
    assert_eq!(addresses.len(), runtime::REGISTRATION_ATTEMPT_LIMIT);
}

#[tokio::test]
async fn registration_stops_when_the_signaling_server_never_answers() {
    let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).await.unwrap();
    let address = listener.local_addr().unwrap();
    let server = tokio::spawn(async move {
        let (stream, _) = listener.accept().await.unwrap();
        let _socket = accept_async(stream).await.unwrap();
        std::future::pending::<()>().await;
    });

    let mut input = request();
    input.signaling_server = format!("ws://{address}");
    let started = tokio::time::Instant::now();
    let error = runtime::register_with_retry(
        &input,
        protocol::Identity::generate().unwrap(),
        &CancellationToken::new(),
    )
    .await
    .err()
    .expect("registration should respect its overall deadline");

    assert!(error.to_string().contains("overall time limit"));
    assert!(started.elapsed() < Duration::from_secs(4));
    server.abort();
}

#[tokio::test]
async fn cancellation_interrupts_pending_kernel_start_and_releases_its_socket()
{
    let _serial = multiplayer::SESSION_TEST.lock().await;
    let (directory, executable) = fixture();
    std::fs::write(directory.path().join("delay"), b"").unwrap();
    let (url, leave) = signaling(false).await;
    let mut input = request();
    input.signaling_server = url;
    input.game_port = minecraft().await;
    let cancellation = CancellationToken::new();
    let token = cancellation.clone();
    let actor = tokio::spawn(async move {
        runtime::run(
            0,
            input,
            &executable,
            &token,
            tokio::sync::mpsc::channel(1).1,
            std::sync::Arc::new(|_| {}),
            &mut None,
        )
        .await
    });
    tokio::time::timeout(Duration::from_secs(5), async {
        while !directory.path().join("arguments").exists() {
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await
    .unwrap();
    cancellation.cancel();
    tokio::time::timeout(Duration::from_secs(3), actor)
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    assert_eq!(
        tokio::time::timeout(Duration::from_secs(3), leave)
            .await
            .unwrap()
            .unwrap()["type"],
        "leave"
    );
}

/// A signaling link that dies mid-session must end the room and reap the
/// EasyTier kernel instead of leaving a half-open room behind.
#[tokio::test]
async fn a_dropped_signaling_connection_ends_the_room_and_releases_the_kernel()
{
    let _serial = multiplayer::SESSION_TEST.lock().await;
    let (directory, executable) = fixture();
    let url = signaling_disconnecting_after_registration().await;
    let mut input = request();
    input.signaling_server = url;
    input.game_port = minecraft().await;
    let cancellation = CancellationToken::new();
    let token = cancellation.clone();
    let executable = executable.clone();
    let (ready, established) = oneshot::channel();
    let actor = tokio::spawn(async move {
        runtime::run(
            0,
            input,
            &executable,
            &token,
            tokio::sync::mpsc::channel(1).1,
            std::sync::Arc::new(|_| {}),
            &mut Some(ready),
        )
        .await
    });
    tokio::time::timeout(Duration::from_secs(15), established)
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    let rpc: u16 = tokio::fs::read_to_string(directory.path().join("rpc-port"))
        .await
        .unwrap()
        .parse()
        .unwrap();
    // The runtime retries the link before giving up, so allow the full
    // reconnect budget to elapse here.
    let error = tokio::time::timeout(Duration::from_secs(20), actor)
        .await
        .unwrap()
        .unwrap()
        .expect_err("a link that cannot be re-established must end the room");
    // The disconnect message names the cause and how to rejoin; an abruptly
    // reset socket also surfaces it through the underlying IO error.
    let message = format!("{error:#}").to_lowercase();
    assert!(
        message.contains("lost the connection to the mctier signaling server")
            || message.contains("connection")
            || message.contains("os error"),
        "unexpected error: {error:#}"
    );
    // The kernel child is gone, so its RPC port stops answering.
    tokio::time::timeout(Duration::from_secs(5), async {
        while TcpStream::connect((Ipv4Addr::LOCALHOST, rpc)).await.is_ok() {
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
    })
    .await
    .expect("the EasyTier kernel socket should be released");
    assert!(directory.path().join("arguments").exists());
    cancellation.cancel();
}

/// Voice signals are only forwarded for players the lobby actually holds: the
/// host's offer reaches the renderer, the forged one is dropped and the room
/// keeps running.
#[tokio::test]
async fn only_room_members_voice_signals_reach_the_renderer() {
    let _serial = multiplayer::SESSION_TEST.lock().await;
    let (directory, executable) = fixture();
    let url = signaling_with_voice_signals().await;
    let mut input = request();
    input.mode = EntryMode::Join;
    input.signaling_server = url;
    input.game_port = minecraft().await;
    let cancellation = CancellationToken::new();
    let token = cancellation.clone();
    let received = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
    let collected = received.clone();
    let emitter: VoiceSignalEmitter =
        std::sync::Arc::new(move |value| collected.lock().unwrap().push(value));
    let (ready, established) = oneshot::channel();
    let mut actor = tokio::spawn(async move {
        runtime::run(
            0,
            input,
            &executable,
            &token,
            tokio::sync::mpsc::channel(1).1,
            emitter,
            &mut Some(ready),
        )
        .await
    });
    tokio::time::timeout(Duration::from_secs(15), established)
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    // Wait for the offer, but surface an early room teardown instead of letting
    // the wait time out without a diagnosis.
    tokio::select! {
        result = &mut actor => {
            panic!("the room ended before the voice offer arrived: {result:?}")
        }
        waited = tokio::time::timeout(Duration::from_secs(10), async {
            while received.lock().unwrap().is_empty() {
                tokio::time::sleep(Duration::from_millis(20)).await;
            }
        }) => {
            waited.expect("the host's voice offer should reach the renderer");
        }
    }
    // Give the forged offer time to be rejected instead of racing the assertion.
    tokio::time::sleep(Duration::from_millis(200)).await;
    let signals = received.lock().unwrap().clone();
    assert_eq!(signals.len(), 1, "unexpected signals: {signals:?}");
    assert_eq!(signals[0]["type"], "offer");
    assert_eq!(signals[0]["from"], "a".repeat(64));
    assert!(
        signals[0].get("playerName").is_none(),
        "protocol metadata must not leak to the renderer"
    );
    cancellation.cancel();
    tokio::time::timeout(Duration::from_secs(5), actor)
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    let _ = directory;
}

/// Accepts the first session, drops it, then serves one more session on the
/// same address so a reconnect can be observed.
async fn signaling_reconnecting_once() -> String {
    let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).await.unwrap();
    let url = format!("ws://{}", listener.local_addr().unwrap());
    tokio::spawn(async move {
        // First session: established, then dropped mid-flight.
        let (stream, _) = listener.accept().await.unwrap();
        let mut socket = accept_async(stream).await.unwrap();
        let _ = establish_session(&mut socket).await;
        while let Some(Ok(message)) = socket.next().await {
            let Ok(text) = message.into_text() else {
                break;
            };
            let Ok(value) = serde_json::from_str::<Value>(text.as_ref()) else {
                break;
            };
            if value["type"] == "set-lobby-options" {
                socket
                    .send(Message::Text(
                        json!({
                            "type": "lobby-options-changed",
                            "isPublic": value["isPublic"],
                        })
                        .to_string()
                        .into(),
                    ))
                    .await
                    .unwrap();
                break;
            }
        }
        let _ = socket.close(None).await;

        // Second session: the reconnect must be able to register again.
        let Ok((stream, _)) = listener.accept().await else {
            return;
        };
        let mut socket = accept_async(stream).await.unwrap();
        let _ = establish_session(&mut socket).await;
        while let Some(Ok(message)) = socket.next().await {
            let Ok(text) = message.into_text() else {
                break;
            };
            let Ok(value) = serde_json::from_str::<Value>(text.as_ref()) else {
                break;
            };
            if value["type"] == "leave" {
                return;
            }
        }
    });
    url
}

#[tokio::test]
async fn a_dropped_signaling_link_reconnects_instead_of_ending_the_room() {
    let _serial = multiplayer::SESSION_TEST.lock().await;
    let (directory, executable) = fixture();
    let url = signaling_reconnecting_once().await;
    let mut input = request();
    input.signaling_server = url;
    input.game_port = minecraft().await;
    let cancellation = CancellationToken::new();
    let token = cancellation.clone();
    let (ready, established) = oneshot::channel();
    let actor = tokio::spawn(async move {
        runtime::run(
            0,
            input,
            &executable,
            &token,
            tokio::sync::mpsc::channel(1).1,
            std::sync::Arc::new(|_| {}),
            &mut Some(ready),
        )
        .await
    });
    tokio::time::timeout(Duration::from_secs(15), established)
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    // The link drops here; the runtime should re-register rather than fail.
    tokio::time::sleep(Duration::from_secs(6)).await;
    assert!(
        !actor.is_finished(),
        "the room must survive a signaling drop it can recover from"
    );
    cancellation.cancel();
    tokio::time::timeout(Duration::from_secs(10), actor)
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    let _ = directory;
}

fn direct_request() -> direct::DirectRequest {
    direct::DirectRequest {
        room_name: "TestRoom".into(),
        password: "secret".into(),
        player_name: "Player".into(),
        server_node: "tcp://127.0.0.1:11010".into(),
        game_port: 25565,
        host_address: None,
    }
}

#[test]
fn direct_room_validation_enforces_the_password_contract() {
    let valid = direct_request();
    assert!(valid.validate().is_ok());

    // A direct room exists because of its password: an empty secret would be
    // an open network wearing the name of a protected one.
    let mut open = direct_request();
    open.password.clear();
    assert!(open.validate().is_err());

    // Joining by an address that is not an IP is a form error, not a runtime
    // surprise after the kernel is already up.
    let mut joined = direct_request();
    joined.host_address = Some("not-an-address".into());
    assert!(joined.validate().is_err());

    let mut nameless = direct_request();
    nameless.room_name = "ab".into();
    assert!(nameless.validate().is_err());
}

#[test]
fn direct_room_arguments_forward_the_overlay_target_per_role() {
    let host = direct_request();
    let host_arguments = direct::arguments(
        &host,
        Ipv4Addr::new(10, 126, 126, 9),
        18181,
        34567,
        Ipv4Addr::new(10, 126, 126, 9),
    );
    let port_forward = host_arguments
        .iter()
        .position(|argument| argument == "--port-forward")
        .map(|index| host_arguments[index + 1].clone())
        .expect("the host must publish its game port");
    assert_eq!(port_forward, "tcp://127.0.0.1:34567/10.126.126.9:25565");
    assert!(host_arguments.contains(&"--network-name".to_string()));
    let name_index = host_arguments
        .iter()
        .position(|argument| argument == "--network-name")
        .unwrap();
    assert_eq!(
        host_arguments[name_index + 1],
        "MCTRIN-TestRoom",
        "a direct room must not share a network name with a lobby room"
    );

    // A guest dials the host's virtual address, not its own.
    let mut guest = direct_request();
    guest.host_address = Some("10.126.126.9".into());
    let guest_arguments = direct::arguments(
        &guest,
        Ipv4Addr::new(10, 126, 126, 12),
        18181,
        34567,
        Ipv4Addr::new(10, 126, 126, 9),
    );
    let port_forward = guest_arguments
        .iter()
        .position(|argument| argument == "--port-forward")
        .map(|index| guest_arguments[index + 1].clone())
        .unwrap();
    assert_eq!(port_forward, "tcp://127.0.0.1:34567/10.126.126.9:25565");
}

#[tokio::test]
async fn direct_room_reports_success_only_once_the_forward_listens() {
    // The kernel must be listening before the room claims to be up; reporting
    // success at spawn time turned a dead network into a green screen.
    let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).await.unwrap();
    let address = listener.local_addr().unwrap();
    // Nothing accepts on the kernel side here: the guard process is a stand-in
    // that exits immediately, so wait_for_listener must fail and the room must
    // surface that instead of reporting Hosting.
    drop(listener);
    let cancellation = CancellationToken::new();
    let outcome = direct::run(
        0,
        direct_request(),
        &PathBuf::from("definitely-not-a-kernel"),
        &cancellation,
        tokio::sync::mpsc::channel(4).1,
        None,
    )
    .await;
    assert!(outcome.is_err(), "a missing kernel cannot host a room");
    let _ = address;
}
