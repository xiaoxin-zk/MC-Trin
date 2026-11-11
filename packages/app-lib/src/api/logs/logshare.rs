use std::time::{Duration, SystemTime, UNIX_EPOCH};

use futures::StreamExt;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use super::crash_analysis::collect_crash_logs;
use crate::State;

const LOGSHARE_BASE_URL: &str = "https://api.logshare.cn";
/// Analysis source that keeps crash analysis on this machine.
pub(crate) const LOCAL_AI_SOURCE: &str = "custom";
/// Analysis source that sends crash logs to LogShare.
pub(crate) const REMOTE_AI_SOURCE: &str = "logshare";
const LOGSHARE_AI_READ_TIMEOUT_SECS: u64 = 300;
const LOGSHARE_SOURCE_PREFIX: &str = "axolotl";

#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct LogShareSettings {
    pub share_provider: String,
    pub ai_source: String,
    pub auto_upload: bool,
    pub multi_file: bool,
    pub no_storage: bool,
    pub show_progress: bool,
    /// Whether the user has explicitly allowed crash logs to be sent to
    /// LogShare. Output only: `skip_deserializing` means echoing this struct
    /// back can never grant permission.
    #[serde(default, skip_deserializing)]
    pub remote_consent: bool,
    /// Explicit consent action carried by an input payload: `"grant"` or
    /// `"revoke"`. Absent in every ordinary settings round-trip, so saving the
    /// settings cannot change upload permission by accident.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub consent_action: Option<String>,
}

impl Default for LogShareSettings {
    fn default() -> Self {
        Self {
            share_provider: "logshare".to_string(),
            // Analysis is local until the user explicitly opts into the
            // remote service.
            ai_source: LOCAL_AI_SOURCE.to_string(),
            auto_upload: false,
            multi_file: true,
            no_storage: false,
            show_progress: true,
            remote_consent: false,
            consent_action: None,
        }
    }
}

#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct SharedLog {
    pub id: String,
    pub url: String,
    pub raw: String,
    pub token: String,
    pub provider: String,
    pub instance_id: Option<String>,
    pub instance_name: Option<String>,
    pub truncated: bool,
    pub created_at: i64,
}

#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct LogShareUploadResponse {
    pub success: bool,
    pub id: String,
    pub url: String,
    pub raw: String,
    pub token: String,
}

#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct LogShareDeleteResponse {
    pub success: bool,
    pub deleted: Vec<String>,
    pub failed: Vec<String>,
}

#[derive(Serialize, Debug, Clone)]
#[serde(rename_all = "snake_case")]
struct UploadFile {
    name: String,
    content: String,
}

#[derive(Serialize, Debug, Clone)]
struct UploadBody {
    content: String,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    files: Vec<UploadFile>,
    source: String,
}

/// Marker recording that the user chose to send crash logs to LogShare.
///
/// Until it exists, a stored `ai_source` of `logshare` is treated as not
/// consented: that value can predate the opt-in requirement, so it alone must
/// never authorize an upload.
const REMOTE_CONSENT_METADATA_KEY: &str = "crash_analysis_remote_consent";

async fn record_remote_consent<'e, E>(executor: E) -> crate::Result<()>
where
    E: sqlx::Executor<'e, Database = sqlx::Sqlite>,
{
    sqlx::query(
        "INSERT INTO app_metadata (key, value, updated_at)
         VALUES (?, '1', unixepoch())
         ON CONFLICT(key) DO UPDATE SET
             value = excluded.value,
             updated_at = excluded.updated_at",
    )
    .bind(REMOTE_CONSENT_METADATA_KEY)
    .execute(executor)
    .await?;
    Ok(())
}

/// Removes the marker so every LogShare content path is refused again.
async fn revoke_remote_consent<'e, E>(executor: E) -> crate::Result<()>
where
    E: sqlx::Executor<'e, Database = sqlx::Sqlite>,
{
    sqlx::query("DELETE FROM app_metadata WHERE key = ?")
        .bind(REMOTE_CONSENT_METADATA_KEY)
        .execute(executor)
        .await?;
    Ok(())
}

/// The whole consent decision, as a pure function of the stored marker.
///
/// Kept separate from the query so the decision itself can be tested directly:
/// absent, NULL, empty, foreign or revoked markers all deny, and only the
/// literal `"1"` allows a remote call.
fn remote_consent_allowed(marker: Option<&str>) -> bool {
    matches!(marker, Some("1"))
}

pub(super) async fn remote_consent_recorded<'e, E>(executor: E) -> bool
where
    E: sqlx::Executor<'e, Database = sqlx::Sqlite>,
{
    let marker = sqlx::query_scalar::<_, String>(
        "SELECT value FROM app_metadata WHERE key = ?",
    )
    .bind(REMOTE_CONSENT_METADATA_KEY)
    .fetch_optional(executor)
    .await
    .ok()
    .flatten();
    remote_consent_allowed(marker.as_deref())
}

/// Fails unless the user explicitly opted into sending crash logs to LogShare.
///
/// Takes the pool directly so the fail-closed decision can be exercised without
/// the global `State`, and so every network path can call it before doing any
/// work at all.
/// Every function that can put log content on the wire must call this before
/// it builds a client, serializes a body, or formats log text. Call sites:
/// `upload_crash`, `analyse_crash_direct`, `analyze_snapshot`, and
/// `logagent::cloud` (before it materializes the snapshot).
pub(super) async fn ensure_remote_consent(
    pool: &sqlx::SqlitePool,
) -> crate::Result<()> {
    if remote_consent_recorded(pool).await {
        return Ok(());
    }
    Err(crate::ErrorKind::InputError(
        "Sending crash logs to LogShare requires your explicit approval. \
         Allow LogShare upload in crash analysis settings first."
            .to_string(),
    )
    .into())
}

/// Consent check for call sites that only hold the global state.
async fn ensure_remote_consent_here() -> crate::Result<()> {
    let state = State::get().await?;
    ensure_remote_consent(&state.pool).await
}

/// Applies the consent gate to persisted settings.
///
/// Rows written before the opt-in existed, and rows left behind after a revoke,
/// still say `ai_source = "logshare"` and `auto_upload = true`. Returning those
/// verbatim would present a remote default the user never chose, so the
/// effective value stays local until consent is recorded.
fn effective_settings(
    mut settings: LogShareSettings,
    consent: bool,
) -> LogShareSettings {
    settings.remote_consent = consent;
    if !consent {
        settings.ai_source = LOCAL_AI_SOURCE.to_string();
        settings.auto_upload = false;
    }
    settings
}

pub async fn get_log_share_settings() -> crate::Result<LogShareSettings> {
    let state = State::get().await?;
    let row = sqlx::query_as::<_, (String, String, i64, i64, i64, i64)>(
        "SELECT s.share_provider, a.ai_source, s.auto_upload, s.multi_file, s.no_storage, s.show_progress FROM log_share_settings s JOIN crash_analysis_ai_settings a ON a.id = s.id WHERE s.id = 0",
    )
    .fetch_one(&state.pool)
    .await?;
    let consent = remote_consent_recorded(&state.pool).await;
    Ok(effective_settings(
        LogShareSettings {
            share_provider: row.0,
            ai_source: row.1,
            auto_upload: row.2 != 0,
            multi_file: row.3 != 0,
            no_storage: row.4 != 0,
            show_progress: row.5 != 0,
            remote_consent: consent,
        },
        consent,
    ))
}

pub async fn update_log_share_settings(
    settings: LogShareSettings,
) -> crate::Result<()> {
    let settings = normalize_settings(settings);
    let consent_action = match settings.consent_action.as_deref() {
        None | Some("") => None,
        Some("grant") => Some(true),
        Some("revoke") => Some(false),
        Some(_) => {
            return Err(crate::ErrorKind::InputError(
                "Unknown consent action".to_string(),
            )
            .into());
        }
    };
    let state = State::get().await?;
    // Settings and consent move together, so a partial write cannot leave the
    // UI showing one state and the gate enforcing another.
    let mut transaction = state.pool.begin().await?;
    sqlx::query(
        "UPDATE log_share_settings SET share_provider = ?, auto_upload = ?, multi_file = ?, no_storage = ?, show_progress = ? WHERE id = 0",
    )
    .bind(&settings.share_provider)
    .bind(settings.auto_upload)
    .bind(settings.multi_file)
    .bind(settings.no_storage)
    .bind(settings.show_progress)
    .execute(&mut *transaction)
    .await?;
    // Only an explicit `consent_action` changes permission. An ordinary
    // settings round-trip carries none, so saving the page can neither grant
    // nor revoke it. Revoking also drops the stored preference to local with
    // automatic upload off, so the row and the enforced state agree.
    match consent_action {
        Some(true) => record_remote_consent(&mut *transaction).await?,
        Some(false) => {
            revoke_remote_consent(&mut *transaction).await?;
            sqlx::query(
                "UPDATE log_share_settings SET ai_source = ?, auto_upload = 0 WHERE id = 0",
            )
            .bind(LOCAL_AI_SOURCE)
            .execute(&mut *transaction)
            .await?;
            sqlx::query(
                "UPDATE crash_analysis_ai_settings SET ai_source = ? WHERE id = 0",
            )
            .bind(LOCAL_AI_SOURCE)
            .execute(&mut *transaction)
            .await?;
        }
        None => {}
    }
    transaction.commit().await?;
    Ok(())
}

fn normalize_settings(mut settings: LogShareSettings) -> LogShareSettings {
    if settings.share_provider != "logshare" {
        settings.share_provider = "mclogs".to_string();
    }
    if settings.ai_source != REMOTE_AI_SOURCE {
        settings.ai_source = LOCAL_AI_SOURCE.to_string();
    }
    settings
}

pub async fn list_shared_logs() -> crate::Result<Vec<SharedLog>> {
    let state = State::get().await?;
    let rows = sqlx::query_as::<_, (String, String, String, String, String, Option<String>, Option<String>, i64, i64)>(
        "SELECT id, url, raw, token, provider, instance_id, instance_name, truncated, created_at FROM shared_logs ORDER BY created_at DESC",
    )
    .fetch_all(&state.pool)
    .await?;
    Ok(rows
        .into_iter()
        .map(|row| SharedLog {
            id: row.0,
            url: row.1,
            raw: row.2,
            token: row.3,
            provider: row.4,
            instance_id: row.5,
            instance_name: row.6,
            truncated: row.7 != 0,
            created_at: row.8,
        })
        .collect())
}

pub async fn record_shared_log(log: SharedLog) -> crate::Result<()> {
    let state = State::get().await?;
    let created_at = log.created_at.max(now_seconds());
    sqlx::query(
        "INSERT INTO shared_logs (id, url, raw, token, provider, instance_id, instance_name, truncated, created_at) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?)
         ON CONFLICT(id) DO UPDATE SET url = excluded.url, raw = excluded.raw, token = excluded.token, provider = excluded.provider, instance_id = excluded.instance_id, instance_name = excluded.instance_name, truncated = excluded.truncated",
    )
    .bind(&log.id)
    .bind(&log.url)
    .bind(&log.raw)
    .bind(&log.token)
    .bind(&log.provider)
    .bind(&log.instance_id)
    .bind(&log.instance_name)
    .bind(log.truncated)
    .bind(created_at)
    .execute(&state.pool)
    .await?;
    Ok(())
}

async fn remove_shared_log_record(id: &str) -> crate::Result<()> {
    let state = State::get().await?;
    sqlx::query("DELETE FROM shared_logs WHERE id = ?")
        .bind(id)
        .execute(&state.pool)
        .await?;
    Ok(())
}

pub async fn delete_shared_log(id: String, token: String) -> crate::Result<()> {
    let remote_result = delete_log(&id, &token).await;
    if let Err(error) = remote_result {
        tracing::warn!(
            "Remote LogShare deletion failed for {id}; removing the local record anyway: {error}"
        );
    }
    remove_shared_log_record(&id).await
}

fn now_seconds() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs() as i64
}

fn source_identifier() -> String {
    format!("{LOGSHARE_SOURCE_PREFIX}/{}", env!("CARGO_PKG_VERSION"))
}

async fn prepare_upload(instance_id: &str) -> crate::Result<UploadBody> {
    let settings = get_log_share_settings().await?;
    let logs = collect_crash_logs(instance_id).await?;

    let combined = logs.combined_log.as_str().to_string();
    let files = if settings.multi_file {
        logs.sources
            .iter()
            .map(|source| UploadFile {
                name: source.name.clone(),
                content: source.content.as_str().to_string(),
            })
            .collect::<Vec<_>>()
    } else {
        Vec::new()
    };
    Ok(UploadBody {
        content: combined,
        files,
        source: source_identifier(),
    })
}

async fn request_client() -> crate::Result<reqwest::Client> {
    let state = State::get().await?;
    let proxy = state.proxy_config().await?;
    let builder = reqwest::Client::builder()
        .connect_timeout(Duration::from_secs(30))
        .read_timeout(Duration::from_secs(LOGSHARE_AI_READ_TIMEOUT_SECS))
        .user_agent(crate::launcher_user_agent());
    proxy.apply(builder)?.build().map_err(Into::into)
}

pub async fn upload_crash(
    instance_id: &str,
) -> crate::Result<LogShareUploadResponse> {
    // Guard first: it must run before `prepare_upload` reads any log file.
    ensure_remote_consent_here().await?;
    let prepared = prepare_upload(instance_id).await?;
    upload_prepared(&prepared).await
}

async fn upload_prepared(
    prepared: &UploadBody,
) -> crate::Result<LogShareUploadResponse> {
    let client = request_client().await?;
    let body_value = serde_json::to_value(prepared)?;
    let request = client
        .post(format!("{LOGSHARE_BASE_URL}/v1/log"))
        .header("Accept", "application/json")
        .header("Accept-Encoding", "br, gzip");

    let response = request.json(&body_value).send().await?;
    let status = response.status();
    let text = response.text().await?;
    if !status.is_success() {
        return Err(crate::ErrorKind::OtherError(format!(
            "LogShare upload failed with HTTP {status}: {}",
            text.chars().take(500).collect::<String>()
        ))
        .into());
    }
    let value: Value = serde_json::from_str(&text).map_err(|_| {
        crate::ErrorKind::OtherError(
            "LogShare returned invalid JSON".to_string(),
        )
    })?;
    if value.get("success").and_then(Value::as_bool) != Some(true) {
        return Err(crate::ErrorKind::OtherError(format!(
            "LogShare upload failed: {}",
            value
                .get("error")
                .and_then(Value::as_str)
                .unwrap_or("unknown error")
        ))
        .into());
    }
    Ok(LogShareUploadResponse {
        success: true,
        id: value
            .get("id")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string(),
        url: value
            .get("url")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string(),
        raw: value
            .get("raw")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string(),
        token: value
            .get("token")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string(),
    })
}

pub async fn get_insights(id: &str) -> crate::Result<Value> {
    let client = request_client().await?;
    let response = client
        .get(format!("{LOGSHARE_BASE_URL}/v1/insights/{id}"))
        .header("Accept", "application/json")
        .header("Accept-Encoding", "br, gzip")
        .send()
        .await?;
    let status = response.status();
    let text = response.text().await?;
    if !status.is_success() {
        return Err(crate::ErrorKind::OtherError(format!(
            "LogShare analysis failed with HTTP {status}: {}",
            text.chars().take(500).collect::<String>()
        ))
        .into());
    }
    serde_json::from_str(&text).map_err(|_| {
        crate::ErrorKind::OtherError(
            "LogShare returned invalid insights JSON".to_string(),
        )
        .into()
    })
}

pub async fn analyse_crash_direct(instance_id: &str) -> crate::Result<Value> {
    // Guard first: no log is read, formatted or serialized before it passes.
    ensure_remote_consent_here().await?;
    let prepared = prepare_upload(instance_id).await?;
    let client = request_client().await?;
    let response = client
        .post(format!("{LOGSHARE_BASE_URL}/v1/analyse"))
        .header("Accept", "application/json")
        .header("Accept-Encoding", "br, gzip")
        .json(&prepared)
        .send()
        .await?;
    let status = response.status();
    let text = response.text().await?;
    if !status.is_success() {
        return Err(crate::ErrorKind::OtherError(format!(
            "LogShare analysis failed with HTTP {status}: {}",
            text.chars().take(500).collect::<String>()
        ))
        .into());
    }
    serde_json::from_str(&text).map_err(|_| {
        crate::ErrorKind::OtherError(
            "LogShare returned invalid analysis JSON".to_string(),
        )
        .into()
    })
}

pub async fn delete_log(
    id: &str,
    token: &str,
) -> crate::Result<LogShareDeleteResponse> {
    let client = request_client().await?;
    let response = client
        .delete(format!("{LOGSHARE_BASE_URL}/v1/log/{id}"))
        .bearer_auth(token)
        .header("Accept", "application/json")
        .header("Accept-Encoding", "br, gzip")
        .send()
        .await?;
    let status = response.status();
    let text = response.text().await?;
    if !status.is_success() {
        return Err(crate::ErrorKind::OtherError(format!(
            "LogShare deletion failed with HTTP {status}: {}",
            text.chars().take(500).collect::<String>()
        ))
        .into());
    }
    serde_json::from_str(&text).map_err(|_| {
        crate::ErrorKind::OtherError(
            "LogShare returned invalid deletion JSON".to_string(),
        )
        .into()
    })
}

pub(super) async fn analyze_snapshot(
    files: Vec<(String, String)>,
    settings: &LogShareSettings,
    ticket: Option<LogShareUploadResponse>,
    emit: impl Fn(&str, Value) + Send + Sync,
) -> crate::Result<super::logagent::CloudResult> {
    // Guard first: the combined body is only assembled after it passes.
    ensure_remote_consent_here().await?;
    let content = files
        .iter()
        .map(|(name, content)| format!("===== {name} =====\n{content}"))
        .collect::<Vec<_>>()
        .join("\n\n");
    let prepared = UploadBody {
        content,
        files: if settings.multi_file {
            files
                .into_iter()
                .map(|(name, content)| UploadFile { name, content })
                .collect()
        } else {
            Vec::new()
        },
        source: source_identifier(),
    };
    let (url, body, ticket) = if settings.no_storage {
        (
            format!("{LOGSHARE_BASE_URL}/v1/ai/analyse"),
            Some(serde_json::to_value(&prepared)?),
            None,
        )
    } else {
        let ticket = match ticket {
            Some(ticket)
                if !ticket.id.is_empty()
                    && ticket.id.chars().all(|ch| {
                        ch.is_ascii_alphanumeric() || ch == '-' || ch == '_'
                    }) =>
            {
                ticket
            }
            _ => upload_prepared(&prepared).await?,
        };
        (
            format!("{LOGSHARE_BASE_URL}/v1/ai/{}", ticket.id),
            None,
            Some(ticket),
        )
    };
    if let Some(ticket) = &ticket {
        emit("uploaded", serde_json::to_value(ticket)?);
    }
    let content = stream_ai(url, body, settings.show_progress, emit).await?;
    Ok(super::logagent::CloudResult { content, ticket })
}

async fn stream_ai(
    url: String,
    body: Option<Value>,
    show_progress: bool,
    emit: impl Fn(&str, Value) + Send + Sync,
) -> crate::Result<String> {
    let client = request_client().await?;
    let mut request = client
        .request(
            if body.is_some() {
                reqwest::Method::POST
            } else {
                reqwest::Method::GET
            },
            url.as_str(),
        )
        .header("Accept", "text/event-stream")
        .header("Accept-Encoding", "br, gzip");
    if let Some(payload) = body {
        request = request.json(&payload);
    }

    let response = request.send().await?;
    let status = response.status();
    if !status.is_success() {
        if status == reqwest::StatusCode::TOO_MANY_REQUESTS {
            return Err(crate::ErrorKind::OtherError(
                "LogAgent queue is full, please retry later".to_string(),
            )
            .into());
        }
        let text = response.text().await?;
        return Err(crate::ErrorKind::OtherError(format!(
            "LogAgent analysis failed with HTTP {status}: {}",
            text.chars().take(500).collect::<String>()
        ))
        .into());
    }

    let mut stream = response.bytes_stream();
    let mut parser = SseParser::new();
    let mut output = String::new();
    let mut received_bytes = 0usize;

    loop {
        let maybe = tokio::time::timeout(
            Duration::from_secs(LOGSHARE_AI_READ_TIMEOUT_SECS),
            stream.next(),
        )
        .await
        .map_err(|_| {
            crate::ErrorKind::OtherError(
                "LogAgent analysis timed out".to_string(),
            )
        })?;
        let Some(chunk) = maybe else {
            break;
        };
        let chunk = chunk?;
        received_bytes = received_bytes.saturating_add(chunk.len());
        if received_bytes > 8 * 1024 * 1024 {
            return Err(crate::ErrorKind::OtherError(
                "LogShare analysis exceeded its response budget".to_string(),
            )
            .into());
        }
        for event in parser.feed(&chunk) {
            if let Some(delta) = event.content {
                output.push_str(&delta);
                if show_progress {
                    emit("delta", json!({ "content": delta }));
                }
            }
            if let Some(status_event) = event.status_event
                && (show_progress || status_event.event_type == "queued")
            {
                emit(&status_event.event_type, status_event.data);
            }
            if let Some(message) = event.error {
                return Err(crate::ErrorKind::OtherError(format!(
                    "LogAgent analysis failed: {message}"
                ))
                .into());
            }
            if event.done {
                return Ok(output);
            }
        }
    }

    // A stream that ended without `event: done` is acceptable as long as text
    // was produced; otherwise it is a protocol error worth surfacing.
    if output.is_empty() {
        return Err(crate::ErrorKind::OtherError(
            "LogAgent analysis ended without output".to_string(),
        )
        .into());
    }
    Ok(output)
}

struct ParsedEvent {
    content: Option<String>,
    status_event: Option<ParsedStatusEvent>,
    done: bool,
    error: Option<String>,
}

struct ParsedStatusEvent {
    event_type: String,
    data: Value,
}

struct SseParser {
    buffer: Vec<u8>,
}

impl SseParser {
    fn new() -> Self {
        Self { buffer: Vec::new() }
    }

    fn feed(&mut self, chunk: &[u8]) -> Vec<ParsedEvent> {
        self.buffer.extend_from_slice(chunk);
        let mut events = Vec::new();
        loop {
            let lf_pos = find_bytes(&self.buffer, b"\n\n");
            let crlf_pos = find_bytes(&self.buffer, b"\r\n\r\n");
            let (pos, separator_len) = match (lf_pos, crlf_pos) {
                (Some(lf_pos), Some(crlf_pos)) if crlf_pos < lf_pos => {
                    (crlf_pos, 4)
                }
                (Some(lf_pos), _) => (lf_pos, 2),
                (None, Some(crlf_pos)) => (crlf_pos, 4),
                (None, None) => break,
            };
            let block =
                String::from_utf8_lossy(&self.buffer[..pos]).into_owned();
            self.buffer.drain(..pos + separator_len);
            if let Some(event) = parse_sse_block(&block) {
                events.push(event);
            }
        }
        events
    }
}

fn find_bytes(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    haystack.windows(needle.len()).position(|w| w == needle)
}

fn parse_sse_block(block: &str) -> Option<ParsedEvent> {
    let mut event_name: Option<String> = None;
    let mut data_lines = Vec::new();

    for line in block.lines() {
        if let Some(rest) = line.strip_prefix("event:") {
            event_name = Some(rest.trim().to_string());
        } else if let Some(rest) = line.strip_prefix("data:") {
            data_lines.push(rest.trim_start().to_string());
        }
    }
    if event_name.is_none() && data_lines.is_empty() {
        return None;
    }

    let data_text = data_lines.join("\n");
    let event_name = event_name.as_deref().unwrap_or("message");

    match event_name {
        "done" => Some(ParsedEvent {
            content: None,
            status_event: None,
            done: true,
            error: None,
        }),
        "error" => Some(ParsedEvent {
            content: None,
            status_event: None,
            done: false,
            error: Some(
                serde_json::from_str::<Value>(&data_text)
                    .ok()
                    .and_then(|value| {
                        value
                            .get("error")
                            .and_then(Value::as_str)
                            .map(str::to_string)
                    })
                    .unwrap_or_else(|| data_text.clone()),
            ),
        }),
        "status" => {
            let Ok(value) = serde_json::from_str::<Value>(&data_text) else {
                return None;
            };
            let event_type = value
                .get("type")
                .and_then(Value::as_str)
                .unwrap_or("status")
                .to_string();
            Some(ParsedEvent {
                content: None,
                status_event: Some(ParsedStatusEvent {
                    event_type,
                    data: value,
                }),
                done: false,
                error: None,
            })
        }
        _ => {
            let Ok(value) = serde_json::from_str::<Value>(&data_text) else {
                return None;
            };
            let content = value
                .pointer("/choices/0/delta/content")
                .and_then(Value::as_str)
                .map(str::to_string);
            Some(ParsedEvent {
                content,
                status_event: None,
                done: false,
                error: None,
            })
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    async fn memory_pool() -> sqlx::SqlitePool {
        let pool = sqlx::SqlitePool::connect("sqlite::memory:").await.unwrap();
        sqlx::query(
            "CREATE TABLE app_metadata (key TEXT NOT NULL PRIMARY KEY, value TEXT NOT NULL, updated_at INTEGER NOT NULL DEFAULT 0)",
        )
        .execute(&pool)
        .await
        .unwrap();
        pool
    }

    #[tokio::test]
    async fn an_inherited_remote_source_never_counts_as_consent() {
        // A database upgraded from a build that shipped the remote default
        // still holds `ai_source = 'logshare'` and `auto_upload = 1`, but there
        // is no consent marker. That combination must not authorize an upload.
        let pool = memory_pool().await;
        sqlx::query(
            "CREATE TABLE log_share_settings (id INTEGER PRIMARY KEY, share_provider TEXT, ai_source TEXT, auto_upload INTEGER, multi_file INTEGER, metadata_enabled INTEGER, no_storage INTEGER, show_progress INTEGER)",
        )
        .execute(&pool)
        .await
        .unwrap();
        sqlx::query(
            "INSERT INTO log_share_settings VALUES (0, 'logshare', 'logshare', 1, 1, 1, 0, 1)",
        )
        .execute(&pool)
        .await
        .unwrap();

        assert!(!remote_consent_recorded(&pool).await);
    }

    /// The three public egress functions must open with the gate. This is a
    /// structural pin: it reads this file's own source, so moving the guard
    /// below a log read, a body serialization or a client build fails here
    /// instead of only in a live-credential environment.
    #[test]
    fn every_egress_function_starts_with_the_consent_guard() {
        const SOURCE: &str = include_str!("logshare.rs");
        for (signature, label) in [
            ("pub async fn upload_crash(", "upload_crash"),
            ("pub async fn analyse_crash_direct(", "analyse_crash_direct"),
            ("pub(super) async fn analyze_snapshot(", "analyze_snapshot"),
        ] {
            let start = SOURCE
                .find(signature)
                .unwrap_or_else(|| panic!("{label} not found"));
            let body = &SOURCE[start..];
            let brace = body.find('{').expect("fn body");
            // Skip the signature line, comments and blank lines.
            let first_statement = body[brace + 1..]
                .lines()
                .map(str::trim)
                .find(|line| {
                    !line.is_empty()
                        && !line.starts_with("//")
                        && !line.starts_with("///")
                })
                .unwrap_or_default();
            assert_eq!(
                first_statement, "ensure_remote_consent_here().await?;",
                "{label} must call the consent gate before doing any work"
            );
        }
    }

    #[test]
    fn the_consent_decision_allows_only_a_positive_marker() {
        assert!(!remote_consent_allowed(None));
        assert!(!remote_consent_allowed(Some("")));
        assert!(!remote_consent_allowed(Some("0")));
        assert!(!remote_consent_allowed(Some("true")));
        assert!(!remote_consent_allowed(Some("2")));
        assert!(remote_consent_allowed(Some("1")));
    }

    #[tokio::test]
    async fn the_gate_refuses_a_legacy_remote_row_before_any_request() {
        // The fail-closed assertion: the gate itself, not merely the absence of
        // a marker, must refuse the legacy combination. It runs before
        // `prepare_upload`, so no client is built and no byte is sent.
        let pool = memory_pool().await;
        sqlx::query(
            "CREATE TABLE log_share_settings (id INTEGER PRIMARY KEY, share_provider TEXT, ai_source TEXT, auto_upload INTEGER, multi_file INTEGER, metadata_enabled INTEGER, no_storage INTEGER, show_progress INTEGER)",
        )
        .execute(&pool)
        .await
        .unwrap();
        sqlx::query(
            "INSERT INTO log_share_settings VALUES (0, 'logshare', 'logshare', 1, 1, 1, 0, 1)",
        )
        .execute(&pool)
        .await
        .unwrap();

        let error = ensure_remote_consent(&pool).await.unwrap_err();
        assert!(
            format!("{error}").contains("explicit approval"),
            "got: {error}"
        );

        record_remote_consent(&pool).await.unwrap();
        assert!(ensure_remote_consent(&pool).await.is_ok());

        revoke_remote_consent(&pool).await.unwrap();
        assert!(
            ensure_remote_consent(&pool).await.is_err(),
            "revoking consent must close every LogShare content path again"
        );
    }

    #[tokio::test]
    async fn revoking_consent_removes_the_marker() {
        let pool = memory_pool().await;
        record_remote_consent(&pool).await.unwrap();
        assert!(remote_consent_recorded(&pool).await);

        revoke_remote_consent(&pool).await.unwrap();
        assert!(!remote_consent_recorded(&pool).await);
        let rows: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM app_metadata WHERE key = ?",
        )
        .bind(REMOTE_CONSENT_METADATA_KEY)
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!(rows, 0, "the marker row itself must be gone");
        // Revoking twice is harmless.
        revoke_remote_consent(&pool).await.unwrap();
    }

    #[tokio::test]
    async fn legacy_rows_are_reported_as_local_until_consent_exists() {
        let legacy = LogShareSettings {
            ai_source: REMOTE_AI_SOURCE.to_string(),
            auto_upload: true,
            ..LogShareSettings::default()
        };

        let without = effective_settings(legacy.clone(), false);
        assert_eq!(without.ai_source, LOCAL_AI_SOURCE);
        assert!(!without.auto_upload, "a legacy row must not auto-upload");
        assert!(!without.remote_consent);

        let with = effective_settings(legacy, true);
        assert_eq!(with.ai_source, REMOTE_AI_SOURCE);
        assert!(
            with.auto_upload,
            "an explicit opt-in keeps the user's choice"
        );
        assert!(with.remote_consent);
    }

    #[tokio::test]
    async fn an_explicit_save_records_consent() {
        let pool = memory_pool().await;
        record_remote_consent(&pool).await.unwrap();
        assert!(remote_consent_recorded(&pool).await);
        // A second save must not fail on the existing row.
        record_remote_consent(&pool).await.unwrap();
        assert!(remote_consent_recorded(&pool).await);
    }

    #[test]
    fn only_an_explicit_action_changes_consent() {
        // An ordinary round-trip: `consent_action` is absent, and echoing
        // `remote_consent = true` back must not be read as a grant.
        let echoed = LogShareSettings {
            remote_consent: true,
            consent_action: None,
            ..LogShareSettings::default()
        };
        let json = serde_json::to_value(&echoed).unwrap();
        assert_eq!(json.get("consent_action"), None);

        // Because the field is not deserialized, even a payload that tries to
        // set it is ignored.
        let parsed: LogShareSettings =
            serde_json::from_value(serde_json::json!({
                "share_provider": "logshare",
                "ai_source": "custom",
                "auto_upload": false,
                "multi_file": true,
                "no_storage": false,
                "show_progress": true,
                "remote_consent": true,
            }))
            .unwrap();
        assert!(
            !parsed.remote_consent,
            "a settings payload must not be able to grant consent"
        );
        assert!(parsed.consent_action.is_none());
    }

    #[tokio::test]
    async fn revoking_resets_the_stored_preference_to_local() {
        let pool = memory_pool().await;
        sqlx::query(
            "CREATE TABLE log_share_settings (id INTEGER PRIMARY KEY, share_provider TEXT, ai_source TEXT, auto_upload INTEGER, multi_file INTEGER, metadata_enabled INTEGER, no_storage INTEGER, show_progress INTEGER)",
        )
        .execute(&pool)
        .await
        .unwrap();
        sqlx::query(
            "INSERT INTO log_share_settings VALUES (0, 'logshare', 'logshare', 1, 1, 1, 0, 1)",
        )
        .execute(&pool)
        .await
        .unwrap();
        record_remote_consent(&pool).await.unwrap();

        // Mirror of the revoke branch in `update_log_share_settings`.
        let mut transaction = pool.begin().await.unwrap();
        revoke_remote_consent(&mut *transaction).await.unwrap();
        sqlx::query(
            "UPDATE log_share_settings SET ai_source = ?, auto_upload = 0 WHERE id = 0",
        )
        .bind(LOCAL_AI_SOURCE)
        .execute(&mut *transaction)
        .await
        .unwrap();
        transaction.commit().await.unwrap();

        let row: (String, i64) =
            sqlx::query_as("SELECT ai_source, auto_upload FROM log_share_settings WHERE id = 0")
                .fetch_one(&pool)
                .await
                .unwrap();
        assert_eq!(row, (LOCAL_AI_SOURCE.to_string(), 0));
        assert!(!remote_consent_recorded(&pool).await);
        assert!(ensure_remote_consent(&pool).await.is_err());
    }

    #[test]
    fn saving_settings_keeps_an_unknown_source_local() {
        let normalized = normalize_settings(LogShareSettings {
            ai_source: "not-a-source".to_string(),
            ..LogShareSettings::default()
        });
        assert_eq!(normalized.ai_source, LOCAL_AI_SOURCE);
    }

    #[test]
    fn the_shipped_defaults_do_not_upload_crash_logs() {
        let settings = LogShareSettings::default();
        assert_eq!(settings.ai_source, LOCAL_AI_SOURCE);
        assert!(!settings.auto_upload);
    }
}
