use super::*;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;

async fn mock_server(
    status: u16,
    body: &'static str,
) -> (String, tokio::task::JoinHandle<String>) {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let handle = tokio::spawn(async move {
        let (mut stream, _) = listener.accept().await.unwrap();
        let mut request = Vec::new();
        let mut buffer = [0; 1024];
        loop {
            let read = stream.read(&mut buffer).await.unwrap();
            if read == 0 {
                break;
            }
            request.extend_from_slice(&buffer[..read]);
            if request.windows(4).any(|window| window == b"\r\n\r\n") {
                break;
            }
        }

        let reason = if status < 400 { "OK" } else { "Error" };
        let response = format!(
            "HTTP/1.1 {status} {reason}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
            body.len()
        );
        stream.write_all(response.as_bytes()).await.unwrap();
        String::from_utf8(request).unwrap()
    });
    (format!("http://{address}/v3/"), handle)
}

async fn invoke(
    method: Method,
    user_id: &str,
    api_base: &str,
    pool: &sqlx::SqlitePool,
    semaphore: &FetchSemaphore,
) -> crate::Result<()> {
    let client = reqwest::Client::builder().no_proxy().build().unwrap();
    mutate_friend(
        method,
        user_id,
        api_base,
        Some(("authorization", "Bearer test-token")),
        Some(&client),
        semaphore,
        pool,
    )
    .await
}

async fn test_pool() -> sqlx::SqlitePool {
    sqlx::sqlite::SqlitePoolOptions::new()
        .max_connections(1)
        .connect("sqlite::memory:")
        .await
        .unwrap()
}

#[tokio::test]
async fn add_friend_posts_encoded_user_id_with_authorization() {
    let (api_base, server) = mock_server(204, "").await;
    let pool = test_pool().await;
    let semaphore = FetchSemaphore(tokio::sync::Semaphore::new(1));

    invoke(
        Method::POST,
        "Player Name/extra",
        &api_base,
        &pool,
        &semaphore,
    )
    .await
    .unwrap();

    let request = server.await.unwrap().to_ascii_lowercase();
    assert!(
        request
            .starts_with("post /v3/friend/player%20name%2fextra http/1.1\r\n")
    );
    assert!(request.contains("authorization: bearer test-token\r\n"));
}

#[tokio::test]
async fn add_friend_maps_modrinth_not_found_to_username_message() {
    let body = r#"{"error":"not_found","description":"User not found"}"#;
    let (api_base, server) = mock_server(404, body).await;
    let pool = test_pool().await;
    let semaphore = FetchSemaphore(tokio::sync::Semaphore::new(1));

    let error =
        invoke(Method::POST, "missing-player", &api_base, &pool, &semaphore)
            .await
            .unwrap_err();

    assert!(format!("{error:#}").contains("No user found with username"));
    let request = server.await.unwrap().to_ascii_lowercase();
    assert!(request.starts_with("post /v3/friend/missing-player http/1.1\r\n"));
}

#[tokio::test]
async fn remove_friend_sends_delete_and_propagates_forbidden_response() {
    let body = r#"{"error":"forbidden","description":"Cannot remove friend"}"#;
    let (api_base, server) = mock_server(403, body).await;
    let pool = test_pool().await;
    let semaphore = FetchSemaphore(tokio::sync::Semaphore::new(1));

    let error =
        invoke(Method::DELETE, "friend-id", &api_base, &pool, &semaphore)
            .await
            .unwrap_err();

    assert!(format!("{error:#}").contains("Cannot remove friend"));
    let request = server.await.unwrap().to_ascii_lowercase();
    assert!(request.starts_with("delete /v3/friend/friend-id http/1.1\r\n"));
    assert!(request.contains("authorization: bearer test-token\r\n"));
}

#[test]
fn friend_endpoint_keeps_base_path_and_encodes_path_segment() {
    assert_eq!(
        friend_endpoint("https://example.test/api/v3/", "name/with space")
            .unwrap(),
        "https://example.test/api/v3/friend/name%2Fwith%20space"
    );
}
