//! Bounded diagnostic sessions with immutable, sanitized log snapshots.

mod tools;
mod window;

use super::{
    CensoredString, LogShareSettings, LogShareUploadResponse, analyze_crash,
    get_crash_analysis_ai_settings, get_log_share_settings,
};
use crate::{
    State,
    api::ai::chat::{ChatMessage, ChatModel, ChatTurn},
    prelude::Credentials,
};
use serde::Serialize;
use serde_json::{Value, json};
use std::{
    collections::{HashMap, HashSet},
    sync::{Arc, LazyLock, Mutex},
    time::{Duration, Instant},
};
use tokio_util::sync::CancellationToken;
use tools::LogFile;
pub use tools::ToolRequest;

const SESSION_TTL: Duration = Duration::from_secs(10 * 60);
static SESSIONS: LazyLock<Mutex<HashMap<String, Arc<Session>>>> =
    LazyLock::new(|| Mutex::new(HashMap::new()));

struct Snapshot {
    model: Option<ChatModel>,
    files: Vec<LogFile>,
    share_settings: LogShareSettings,
}
struct Session {
    created: Instant,
    cancel: CancellationToken,
    snapshot: tokio::sync::OnceCell<Snapshot>,
    source: String,
    budget: Mutex<Budget>,
    turn_lock: tokio::sync::Mutex<()>,
}

#[derive(Default)]
struct Budget {
    turns: usize,
    tools: usize,
    reads: HashSet<String>,
}

#[derive(Serialize)]
pub struct PreparedSession {
    pub run_id: String,
    pub provider_id: String,
    pub model_id: String,
    pub locale: String,
    pub context: String,
}

#[derive(Clone, Serialize)]
pub struct AnalysisEvent {
    pub run_id: String,
    pub event_type: String,
    pub data: Value,
}

#[derive(Serialize)]
pub struct CloudResult {
    pub content: String,
    pub ticket: Option<LogShareUploadResponse>,
}

fn error(message: &str) -> crate::Error {
    crate::ErrorKind::OtherError(message.to_string()).into()
}

fn sessions()
-> crate::Result<std::sync::MutexGuard<'static, HashMap<String, Arc<Session>>>>
{
    SESSIONS
        .lock()
        .map_err(|_| error("LogAgent session store unavailable"))
}

fn session(run_id: &str) -> crate::Result<Arc<Session>> {
    let session = sessions()?
        .get(run_id)
        .cloned()
        .ok_or_else(|| error("LogAgent session expired or cancelled"))?;
    if session.cancel.is_cancelled() || session.created.elapsed() > SESSION_TTL
    {
        return Err(error("LogAgent session expired or cancelled"));
    }
    Ok(session)
}

fn sanitize(value: &mut Value, credentials: &[Credentials]) {
    match value {
        Value::String(text) => {
            *text = CensoredString::censor(std::mem::take(text), credentials)
                .as_str()
                .to_string()
        }
        Value::Array(items) => {
            for item in items {
                sanitize(item, credentials);
            }
        }
        Value::Object(items) => {
            for item in items.values_mut() {
                sanitize(item, credentials);
            }
        }
        _ => {}
    }
}

pub async fn prepare(
    instance_id: &str,
    run_id: String,
    source: String,
    provider_id: Option<String>,
    model_id: Option<String>,
) -> crate::Result<PreparedSession> {
    uuid::Uuid::parse_str(&run_id)
        .map_err(|_| error("Invalid LogAgent run ID"))?;
    if !matches!(source.as_str(), "custom" | "logshare" | "assistant") {
        return Err(error("Unknown analysis source"));
    }
    let session = Arc::new(Session {
        created: Instant::now(),
        cancel: CancellationToken::new(),
        snapshot: tokio::sync::OnceCell::new(),
        source,
        budget: Mutex::new(Budget::default()),
        turn_lock: tokio::sync::Mutex::new(()),
    });
    {
        let mut sessions = sessions()?;
        sessions.retain(|_, session| {
            let live = session.created.elapsed() < SESSION_TTL
                && !session.cancel.is_cancelled();
            if !live {
                session.cancel.cancel();
            }
            live
        });
        if sessions.len() >= 8 || sessions.contains_key(&run_id) {
            return Err(error(
                "Too many diagnostic sessions; cancel an existing run",
            ));
        }
        sessions.insert(run_id.clone(), session.clone());
    }
    let result = tokio::select! {
        biased;
        _ = session.cancel.cancelled() => Err(error("LogAgent cancelled")),
        _ = tokio::time::sleep(SESSION_TTL) => Err(error("LogAgent time budget exceeded")),
        result = prepare_snapshot(
            instance_id,
            &run_id,
            &session,
            provider_id.as_deref(),
            model_id.as_deref(),
        ) => result,
    };
    if result.is_err() {
        cancel(&run_id)?;
    }
    result
}

async fn prepare_snapshot(
    instance_id: &str,
    run_id: &str,
    session: &Session,
    requested_provider_id: Option<&str>,
    requested_model_id: Option<&str>,
) -> crate::Result<PreparedSession> {
    let settings = get_crash_analysis_ai_settings().await?;
    let (provider_id, model_id, model) = match session.source.as_str() {
        "custom" => {
            if !settings.enabled {
                return Err(error(
                    "Enable local LogAgent in analysis settings",
                ));
            }
            let model =
                ChatModel::prepare(&settings.provider_id, &settings.model_id)
                    .await?;
            (
                settings.provider_id.clone(),
                settings.model_id.clone(),
                Some(model),
            )
        }
        "assistant" => {
            let provider_id = requested_provider_id
                .filter(|value| !value.trim().is_empty())
                .ok_or_else(|| {
                    error("Select an AI provider for the assistant")
                })?;
            let model_id = requested_model_id
                .filter(|value| !value.trim().is_empty())
                .ok_or_else(|| error("Select an AI model for the assistant"))?;
            let model = ChatModel::prepare(provider_id, model_id).await?;
            (provider_id.to_string(), model_id.to_string(), Some(model))
        }
        _ => (
            settings.provider_id.clone(),
            settings.model_id.clone(),
            None,
        ),
    };
    let share_settings = get_log_share_settings().await?;
    let state = State::get().await?;
    let locale = crate::state::Settings::get(&state.pool).await?.locale;
    let (files, context) = if session.source == "assistant" {
        (
            Vec::new(),
            "Use the read-only launcher context tools only when they help answer the user's question. Do not infer details that those tools do not provide.".to_string(),
        )
    } else {
        let analysis = analyze_crash(instance_id).await?;
        let credentials = Credentials::get_all(&state.pool)
            .await?
            .into_iter()
            .collect::<Vec<_>>();
        let mut remaining = 16 * 1024 * 1024usize;
        let files = analysis
            .sources
            .iter()
            .enumerate()
            .map(|(index, source)| {
                let mut lines = Vec::new();
                for line in source.content.as_str().lines() {
                    if line.len() + 1 > remaining {
                        break;
                    }
                    remaining -= line.len() + 1;
                    lines.push(line.to_string());
                }
                LogFile {
                    id: format!("log-{}", index + 1),
                    name: CensoredString::censor(
                        source.filename.clone(),
                        &credentials,
                    )
                    .as_str()
                    .to_string(),
                    source_type: source.source_type.clone(),
                    first_line: source.first_line,
                    truncated: source.first_line > 1
                        || lines.len() < source.line_count,
                    line_count: lines.len(),
                    lines,
                }
            })
            .collect::<Vec<_>>();
        if files.iter().all(|file| file.lines.is_empty()) {
            return Err(error("No crash logs available for analysis"));
        }
        let mut metadata = json!({ "findings": analysis.findings, "matched_mods": analysis.mods, "mod_changes": analysis.mod_changes, "windows_events": analysis.windows_events });
        sanitize(&mut metadata, &credentials);
        let context = format!(
            "[Diagnostic metadata; may be shortened]\n{}\n[Snapshot file IDs and original line ranges]\n{}\n[Focused initial log excerpts; use tools to expand]\n{}",
            metadata
                .to_string()
                .chars()
                .take(20_000)
                .collect::<String>(),
            serde_json::to_string(&files)?,
            window::initial_window(&files)
        );
        (files, context)
    };
    session
        .snapshot
        .set(Snapshot {
            model,
            files,
            share_settings,
        })
        .map_err(|_| error("Diagnostic snapshot already prepared"))?;
    Ok(PreparedSession {
        run_id: run_id.to_string(),
        provider_id,
        model_id,
        locale,
        context,
    })
}

pub fn cancel(run_id: &str) -> crate::Result<()> {
    if let Some(session) = sessions()?.remove(run_id) {
        session.cancel.cancel();
    }
    Ok(())
}

impl Session {
    fn snapshot(&self) -> crate::Result<&Snapshot> {
        self.snapshot
            .get()
            .ok_or_else(|| error("Diagnostic snapshot is not ready"))
    }
    fn remaining(&self) -> Duration {
        SESSION_TTL.saturating_sub(self.created.elapsed())
    }
}

pub async fn model_turn(
    run_id: &str,
    turn: ChatTurn,
    emit: impl Fn(AnalysisEvent) + Send + Sync,
) -> crate::Result<ChatMessage> {
    let session = session(run_id)?;
    let model = session
        .snapshot()?
        .model
        .as_ref()
        .ok_or_else(|| error("This is a cloud diagnostic session"))?;
    let _lock = session.turn_lock.lock().await;
    {
        let mut budget = session
            .budget
            .lock()
            .map_err(|_| error("LogAgent budget unavailable"))?;
        budget.turns += 1;
        if budget.turns > 16 {
            return Err(error("LogAgent reached the 16 model turn limit"));
        }
    }
    if turn.tools.iter().any(|tool| {
        !tool["function"]["name"]
            .as_str()
            .is_some_and(|name| allowed_tool_name(&session.source, name))
    }) {
        return Err(error("Unsupported diagnostic model tool"));
    }
    let stream_event = |kind: &str, text: &str| {
        emit(AnalysisEvent {
            run_id: run_id.to_string(),
            event_type: kind.to_string(),
            data: json!({ "content": text }),
        })
    };
    tokio::select! {
        biased;
        _ = session.cancel.cancelled() => Err(error("LogAgent cancelled")),
        _ = tokio::time::sleep(session.remaining()) => Err(error("LogAgent time budget exceeded")),
        result = model.turn(turn, &stream_event) => result,
    }
}

pub async fn tool(run_id: &str, request: ToolRequest) -> crate::Result<Value> {
    let session = session(run_id)?;
    if session.source == "assistant" {
        if !assistant_tool_name(&request.name)
            || !request.file_id.is_empty()
            || request.start_line.is_some()
            || request.end_line.is_some()
            || request.query.is_some()
            || request.context.is_some()
        {
            return Err(error("Unsupported launcher assistant tool request"));
        }
    } else if session.source != "custom" {
        return Err(error("Local tools are unavailable for cloud sessions"));
    }
    let duplicate = {
        let mut budget = session
            .budget
            .lock()
            .map_err(|_| error("LogAgent budget unavailable"))?;
        budget.tools += 1;
        if budget.tools > 40 {
            return Err(error("LogAgent reached the 40 tool call limit"));
        }
        let key = format!(
            "{}:{}:{:?}:{:?}:{:?}:{:?}",
            request.name,
            request.file_id,
            request.start_line,
            request.end_line,
            request.query,
            request.context
        );
        !budget.reads.insert(key)
    };
    if duplicate {
        return Ok(
            json!({ "notice": "This exact request was already answered. Reuse earlier evidence or inspect another range." }),
        );
    }
    if session.source == "assistant" {
        tools::execute_assistant(&request).await
    } else {
        Ok(tools::execute(&session.snapshot()?.files, &request))
    }
}

fn assistant_tool_name(name: &str) -> bool {
    matches!(
        name,
        "get_instance_overview"
            | "get_launcher_settings"
            | "get_platform_diagnostics"
    )
}

fn remote_mcp_tool_name(name: &str) -> bool {
    let Some(rest) = name.strip_prefix("mcp_") else {
        return false;
    };
    let Some((index, tool)) = rest.split_once('_') else {
        return false;
    };
    !index.is_empty()
        && index.bytes().all(|byte| byte.is_ascii_digit())
        && !tool.is_empty()
        && tool.len() <= 48
        && tool.bytes().all(|byte| {
            byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-')
        })
}

fn allowed_tool_name(source: &str, name: &str) -> bool {
    let local_allowed = match source {
        "assistant" => assistant_tool_name(name),
        "custom" => {
            matches!(name, "list_log_files" | "read_log_file" | "grep_log_file")
        }
        _ => false,
    };
    local_allowed || remote_mcp_tool_name(name)
}

pub async fn cloud(
    run_id: &str,
    ticket: Option<LogShareUploadResponse>,
    emit: impl Fn(AnalysisEvent) + Send + Sync,
) -> crate::Result<CloudResult> {
    let session = session(run_id)?;
    if session.source != "logshare" {
        return Err(error("Cloud analysis was not selected for this run"));
    }
    let _lock = session.turn_lock.lock().await;
    let snapshot = session.snapshot()?;
    let files = snapshot
        .files
        .iter()
        .map(|file| {
            (format!("{}-{}", file.id, file.name), file.lines.join("\n"))
        })
        .collect::<Vec<_>>();
    tokio::select! {
        biased;
        _ = session.cancel.cancelled() => Err(error("LogAgent cancelled")),
        _ = tokio::time::sleep(session.remaining()) => Err(error("LogAgent time budget exceeded")),
        result = super::logshare::analyze_snapshot(files, &snapshot.share_settings, ticket, |event_type, data| emit(AnalysisEvent { run_id: run_id.to_string(), event_type: event_type.to_string(), data })) => result,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cancellation_cannot_affect_another_run() {
        let first = uuid::Uuid::new_v4().to_string();
        let second = uuid::Uuid::new_v4().to_string();
        let make_session = || {
            Arc::new(Session {
                created: Instant::now(),
                cancel: CancellationToken::new(),
                snapshot: tokio::sync::OnceCell::new(),
                source: "custom".into(),
                budget: Mutex::new(Budget::default()),
                turn_lock: tokio::sync::Mutex::new(()),
            })
        };
        let old = make_session();
        sessions().unwrap().insert(first.clone(), old.clone());
        sessions().unwrap().insert(second.clone(), make_session());
        cancel(&first).unwrap();
        assert!(old.cancel.is_cancelled());
        assert!(session(&first).is_err());
        assert!(session(&second).is_ok());
        cancel(&first).unwrap();
        cancel(&second).unwrap();
    }

    #[test]
    fn sanitizes_nested_metadata_without_changing_structure() {
        let username = whoami::username();
        let mut metadata = json!({ "windows_events": [{ "message": format!("C:\\Users\\{username}\\game") }], "count": 3 });
        sanitize(&mut metadata, &[]);
        assert_eq!(metadata["count"], 3);
        assert!(
            metadata["windows_events"][0]["message"]
                .as_str()
                .unwrap()
                .contains("{COMPUTER_USERNAME}")
        );
    }

    #[test]
    fn assistant_tool_allowlist_is_read_only_and_accepts_remote_mcp_aliases() {
        assert!(allowed_tool_name("assistant", "get_instance_overview"));
        assert!(allowed_tool_name("assistant", "get_launcher_settings"));
        assert!(allowed_tool_name("assistant", "get_platform_diagnostics"));
        assert!(allowed_tool_name("assistant", "mcp_0_lookup_server"));
        assert!(!allowed_tool_name("assistant", "list_log_files"));
        assert!(!allowed_tool_name("assistant", "execute"));
        assert!(!allowed_tool_name("custom", "get_launcher_settings"));
        assert!(allowed_tool_name("custom", "read_log_file"));
        assert!(!remote_mcp_tool_name("mcp_x_lookup_server"));
        assert!(!remote_mcp_tool_name("mcp_0_execute;"));
    }
}
