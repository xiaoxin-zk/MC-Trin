//! A minimal OAuth 2.0 authorization code grant flow redirection/reply loopback URI HTTP
//! server implementation, compliant with [RFC 6749]'s authorization code grant flow and
//! [RFC 8252]'s best current practices for OAuth 2.0 in native apps.
//!
//! This server is needed for the step 4 of the OAuth authentication dance represented in
//! figure 1 of [RFC 8252].
//!
//! Further reading: https://www.oauth.com/oauth2-servers/oauth-native-apps/redirect-urls-for-native-apps/
//!
//! [RFC 6749]: https://datatracker.ietf.org/doc/html/rfc6749
//! [RFC 8252]: https://datatracker.ietf.org/doc/html/rfc8252

use std::{
    net::SocketAddr,
    sync::{LazyLock, Mutex},
    time::Duration,
};

use hyper::body::Incoming;
use hyper_util::rt::{TokioIo, TokioTimer};
use theseus::ErrorKind;
use theseus::prelude::tcp_listen_any_loopback;
use tokio::sync::{broadcast, oneshot};

static SERVER_SHUTDOWN: LazyLock<broadcast::Sender<()>> =
    LazyLock::new(|| broadcast::channel(1024).0);

/// Starts a temporary HTTP server to receive OAuth 2.0 authorization code grant flow redirects
/// on a loopback interface with an ephemeral port. The caller can know the bound socket address
/// by listening on the counterpart channel for `listen_socket_tx`.
///
/// If the server is stopped before receiving an authorization code, `Ok(None)` is returned.
pub async fn listen(
    listen_socket_tx: oneshot::Sender<Result<SocketAddr, theseus::Error>>,
) -> Result<Option<AuthorizationCodeReply>, theseus::Error> {
    let listener = tcp_listen_any_loopback().await;
    listen_with_listener(listener, listen_socket_tx, None).await
}

/// Starts a temporary HTTP server on a registered loopback callback address.
pub async fn listen_fixed(
    address: SocketAddr,
    listen_socket_tx: oneshot::Sender<Result<SocketAddr, theseus::Error>>,
    expected_path: String,
    expected_state: String,
) -> Result<Option<AuthorizationCodeReply>, theseus::Error> {
    let listener = tokio::net::TcpListener::bind(address).await;
    listen_with_listener(
        listener,
        listen_socket_tx,
        Some(CallbackExpectation {
            path: expected_path,
            state: expected_state,
        }),
    )
    .await
}

struct CallbackExpectation {
    path: String,
    state: String,
}

async fn listen_with_listener(
    listener: Result<tokio::net::TcpListener, std::io::Error>,
    listen_socket_tx: oneshot::Sender<Result<SocketAddr, theseus::Error>>,
    expected: Option<CallbackExpectation>,
) -> Result<Option<AuthorizationCodeReply>, theseus::Error> {
    let listener = match listener {
        Ok(listener) => {
            listen_socket_tx
                .send(listener.local_addr().map_err(|e| {
                    ErrorKind::OtherError(format!(
                        "Failed to get auth code reply socket address: {e}"
                    ))
                    .into()
                }))
                .ok();

            listener
        }
        Err(e) => {
            let error_msg =
                format!("Failed to bind auth code reply socket: {e}");

            listen_socket_tx
                .send(Err(ErrorKind::OtherError(error_msg.clone()).into()))
                .ok();

            return Err(ErrorKind::OtherError(error_msg).into());
        }
    };

    let mut auth_code = Mutex::new(None);
    let mut shutdown_notification = SERVER_SHUTDOWN.subscribe();

    while auth_code.get_mut().unwrap().is_none() {
        let client_socket = tokio::select! {
            biased;
            _ = shutdown_notification.recv() => {
                break;
            }
            conn_accept_result = listener.accept() => {
                match conn_accept_result {
                    Ok((socket, _)) => socket,
                    Err(e) => {
                        tracing::warn!("Failed to accept auth code reply: {e}");
                        continue;
                    }
                }
            }
        };

        if let Err(e) = hyper::server::conn::http1::Builder::new()
            .keep_alive(false)
            .header_read_timeout(Duration::from_secs(5))
            .timer(TokioTimer::new())
            .auto_date_header(false)
            .serve_connection(
                TokioIo::new(client_socket),
                hyper::service::service_fn(|req| {
                    handle_reply(req, &auth_code, expected.as_ref())
                }),
            )
            .await
        {
            tracing::warn!("Failed to handle auth code reply: {e}");
        }
    }

    auth_code.into_inner().unwrap().transpose()
}

/// Stops any active OAuth 2.0 authorization code grant flow reply listening HTTP servers.
pub fn stop_listeners() {
    SERVER_SHUTDOWN.send(()).ok();
}

#[derive(Debug, Eq, PartialEq)]
pub struct AuthorizationCodeReply {
    pub code: String,
    pub state: Option<String>,
}

struct ReplyPageCopy {
    language: &'static str,
    success_title: &'static str,
    success_message: &'static str,
    error_title: &'static str,
    error_message: &'static str,
}

fn reply_page_copy(
    accept_language: Option<&hyper::header::HeaderValue>,
) -> ReplyPageCopy {
    let is_chinese = accept_language
        .and_then(|value| value.to_str().ok())
        .is_some_and(|value| {
            value
                .split(',')
                .any(|language| language.trim_start().starts_with("zh"))
        });

    if is_chinese {
        ReplyPageCopy {
            language: "zh-CN",
            success_title: "已收到授权",
            success_message: "请返回 MC-Trin 完成登录。现在可以关闭此页面。",
            error_title: "发生错误",
            error_message: "未能完成本次授权。请返回 MC-Trin 查看结果或重试。",
        }
    } else {
        ReplyPageCopy {
            language: "en",
            success_title: "Authorization received",
            success_message: "Return to MC-Trin to finish signing in. You can close this page now.",
            error_title: "Error",
            error_message: "Authorization could not be completed. Return to MC-Trin to check the result or retry.",
        }
    }
}

fn render_reply_page(language: &str, title: &str, message: &str) -> String {
    include_str!("auth_code_reply/page.html")
        .replace("lang=en", &format!("lang={language}"))
        .replace("{{title}}", title)
        .replace("{{message}}", message)
}

async fn handle_reply(
    req: hyper::Request<Incoming>,
    auth_code_out: &Mutex<
        Option<Result<AuthorizationCodeReply, theseus::Error>>,
    >,
    expected: Option<&CallbackExpectation>,
) -> Result<hyper::Response<String>, hyper::http::Error> {
    if req.method() != hyper::Method::GET {
        return hyper::Response::builder()
            .status(hyper::StatusCode::METHOD_NOT_ALLOWED)
            .header("Allow", "GET")
            .body("".into());
    }

    let copy =
        reply_page_copy(req.headers().get(hyper::header::ACCEPT_LANGUAGE));

    let accepted = match parse_reply(req.uri(), expected) {
        Some(Ok(auth_code)) => {
            *auth_code_out.lock().unwrap() = Some(Ok(auth_code));
            true
        }
        Some(Err(error)) => {
            *auth_code_out.lock().unwrap() =
                Some(Err(ErrorKind::InputError(error).as_error()));
            false
        }
        None => false,
    };

    let response = if accepted {
        hyper::Response::builder()
            .status(hyper::StatusCode::OK)
            .header("Content-Type", "text/html;charset=utf-8")
            .body(render_reply_page(
                copy.language,
                copy.success_title,
                copy.success_message,
            ))
    } else {
        hyper::Response::builder()
            .status(hyper::StatusCode::BAD_REQUEST)
            .header("Content-Type", "text/html;charset=utf-8")
            .body(render_reply_page(
                copy.language,
                copy.error_title,
                copy.error_message,
            ))
    }?;

    Ok(response)
}

fn parse_reply(
    uri: &hyper::Uri,
    expected: Option<&CallbackExpectation>,
) -> Option<Result<AuthorizationCodeReply, String>> {
    if expected.is_some_and(|expected| expected.path != uri.path()) {
        return None;
    }
    let pairs: Vec<_> =
        url::form_urlencoded::parse(uri.query()?.as_bytes()).collect();
    for key in ["code", "state", "error"] {
        if pairs.iter().filter(|(name, _)| name == key).count() > 1 {
            return None;
        }
    }
    let unique = |key: &str| {
        let mut values = pairs.iter().filter(|(name, _)| name == key);
        let (_, value) = values.next()?;
        if values.next().is_some() {
            return None;
        }
        Some(value.to_string())
    };
    let state = unique("state");
    if expected.is_some_and(|expected| {
        state.as_deref() != Some(expected.state.as_str())
    }) {
        return None;
    }
    let code = unique("code");
    let error = unique("error");
    match (code, error) {
        (Some(code), None) if !code.trim().is_empty() => {
            Some(Ok(AuthorizationCodeReply { code, state }))
        }
        (None, Some(error)) if expected.is_some() && !error.is_empty() => Some(
            Err(format!("Microsoft sign-in was not completed ({error})")),
        ),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    fn expected() -> CallbackExpectation {
        CallbackExpectation {
            path: "/oauth/microsoft/callback".into(),
            state: "expected-state".into(),
        }
    }

    #[test]
    fn callback_requires_the_exact_path_state_and_nonempty_unique_code() {
        let expected = expected();
        for uri in [
            "/favicon.ico?code=unused&state=expected-state",
            "/oauth/microsoft/callback?code=unused",
            "/oauth/microsoft/callback?code=unused&state=wrong-state",
            "/oauth/microsoft/callback?code=&state=expected-state",
            "/oauth/microsoft/callback?code=a&code=b&state=expected-state",
            "/oauth/microsoft/callback?code=a&state=expected-state&state=wrong-state",
            "/oauth/microsoft/callback?code=a&error=access_denied&state=expected-state",
        ] {
            assert!(
                parse_reply(&uri.parse().unwrap(), Some(&expected)).is_none(),
                "{uri}"
            );
        }
        let reply = parse_reply(&"/oauth/microsoft/callback?code=encoded%2Bcode&state=expected-state".parse().unwrap(), Some(&expected)).unwrap().unwrap();
        assert_eq!(reply.code, "encoded+code");
        assert_eq!(reply.state.as_deref(), Some("expected-state"));
    }

    #[test]
    fn matching_oauth_denial_finishes_the_flow_as_an_error() {
        let reply = parse_reply(&"/oauth/microsoft/callback?error=access_denied&state=expected-state".parse().unwrap(), Some(&expected())).unwrap();
        assert!(reply.unwrap_err().contains("access_denied"));
    }

    #[tokio::test]
    async fn listener_survives_unrelated_requests_then_accepts_the_matching_callback()
     {
        let (address_tx, address_rx) = oneshot::channel();
        let task = tokio::spawn(listen_fixed(
            "127.0.0.1:0".parse().unwrap(),
            address_tx,
            expected().path,
            expected().state,
        ));
        let address = address_rx.await.unwrap().unwrap();
        for path in [
            "/favicon.ico",
            "/oauth/microsoft/callback?code=unused&state=wrong-state",
        ] {
            let response = request(address, path).await;
            assert!(response.starts_with("HTTP/1.1 400"));
            assert!(!task.is_finished());
        }
        let response = request(
            address,
            "/oauth/microsoft/callback?code=accepted&state=expected-state",
        )
        .await;
        assert!(response.starts_with("HTTP/1.1 200"));
        let reply = tokio::time::timeout(Duration::from_secs(2), task)
            .await
            .unwrap()
            .unwrap()
            .unwrap()
            .unwrap();
        assert_eq!(reply.code, "accepted");
        assert!(tokio::net::TcpListener::bind(address).await.is_ok());
    }

    async fn request(address: SocketAddr, path: &str) -> String {
        tokio::time::timeout(Duration::from_secs(2), async {
            let mut stream = tokio::net::TcpStream::connect(address).await.unwrap();
            stream.write_all(format!("GET {path} HTTP/1.1\r\nHost: {address}\r\nConnection: close\r\n\r\n").as_bytes()).await.unwrap();
            let mut response = Vec::new();
            stream.read_to_end(&mut response).await.unwrap();
            String::from_utf8(response).unwrap()
        }).await.unwrap()
    }
}
