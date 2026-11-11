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
use rand::RngCore;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::{
    collections::{HashMap, HashSet},
    sync::{Arc, LazyLock, Mutex},
    time::{Duration, Instant},
};
use tokio_util::sync::CancellationToken;
use tools::LogFile;
pub use tools::ToolRequest as ToolCall;

/// A tool request plus the approval grant a write tool must present.
///
/// `approval_request` asks the backend to mint a grant for this exact call; the
/// renderer sends it only after the user accepted the approval dialog. The
/// minted `approval` token is then sent back to execute. Neither field is part
/// of the model-facing tool schema, and the model's arguments are validated by
/// [`assistant_request_shape_valid`], which rejects unknown keys.
#[derive(Deserialize)]
pub struct ToolRequest {
    #[serde(flatten)]
    pub(crate) call: ToolCall,
    #[serde(default)]
    approval: Option<String>,
    #[serde(default)]
    approval_request: Option<bool>,
}

/// Response shape for the mint step: no launcher state is touched.
pub const APPROVAL_REQUIRED: &str = "approval_required";
pub const APPROVAL_TOKEN: &str = "approval_token";

const SESSION_TTL: Duration = Duration::from_secs(10 * 60);
static SESSIONS: LazyLock<Mutex<HashMap<String, Arc<Session>>>> =
    LazyLock::new(|| Mutex::new(HashMap::new()));

struct Snapshot {
    model: Option<ChatModel>,
    files: Vec<LogFile>,
    share_settings: LogShareSettings,
}
/// One diagnostic run. Owns the per-run approval grants.
struct Session {
    created: Instant,
    cancel: CancellationToken,
    snapshot: tokio::sync::OnceCell<Snapshot>,
    source: String,
    budget: Mutex<Budget>,
    turn_lock: tokio::sync::Mutex<()>,
    /// Write approvals issued by this run and not yet redeemed.
    approvals: Mutex<HashMap<String, PendingApproval>>,
}

/// A write approval the backend issued for one exact tool call.
///
/// The token is only ever produced by this module after the caller asked to
/// run a write tool; it is bound to the run, the tool name and the exact
/// arguments, expires quickly and is removed the moment it is redeemed.
/// A model can neither guess one nor reuse one it has already spent.
struct PendingApproval {
    tool: String,
    arguments: String,
    expires: Instant,
}

/// How long a user approval stays redeemable before it must be asked again.
const APPROVAL_TTL: Duration = Duration::from_secs(120);
/// Cap the number of unredeemed approvals one run can hold.
const MAX_PENDING_APPROVALS: usize = 16;

fn approval_token() -> String {
    let mut bytes = [0u8; 32];
    rand::rngs::OsRng.fill_bytes(&mut bytes);
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}

/// Fingerprint of the exact tool arguments, so an approval for one call cannot
/// be repointed at another instance, file or settings patch.
fn arguments_digest(request: &ToolCall) -> String {
    let canonical =
        serde_json::to_string(request).unwrap_or_else(|_| request.name.clone());
    Sha256::digest(canonical.as_bytes())
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

impl Session {
    /// Issues a single-use approval for one write call and returns its token.
    fn issue_approval(
        &self,
        tool: &str,
        arguments: &str,
    ) -> crate::Result<String> {
        let mut approvals = self
            .approvals
            .lock()
            .map_err(|_| error("LogAgent approval store unavailable"))?;
        let now = Instant::now();
        approvals.retain(|_, approval| approval.expires > now);
        if approvals.len() >= MAX_PENDING_APPROVALS {
            return Err(error(
                "Too many pending tool approvals; finish or cancel the current run",
            ));
        }
        let token = approval_token();
        approvals.insert(
            token.clone(),
            PendingApproval {
                tool: tool.to_string(),
                arguments: arguments.to_string(),
                expires: now + APPROVAL_TTL,
            },
        );
        Ok(token)
    }

    /// Redeems an approval. Returns false when it is unknown, expired, already
    /// spent, or bound to a different tool or different arguments.
    fn redeem_approval(
        &self,
        token: &str,
        tool: &str,
        arguments: &str,
    ) -> bool {
        let Ok(mut approvals) = self.approvals.lock() else {
            return false;
        };
        // Drop grants nobody redeemed so an expired one cannot occupy the
        // pending cap and shut out the user's next legitimate approval.
        let now = Instant::now();
        approvals.retain(|_, approval| approval.expires > now);
        let Some(approval) = approvals.remove(token) else {
            return false;
        };
        approval.tool == tool && approval.arguments == arguments
    }
}

/// Gate for one assistant write call.
///
/// `Ok(Some(token))` means the caller asked for an approval: a grant was minted
/// and the write must not run yet. `Ok(None)` means the presented grant was
/// valid and has now been consumed. Every other outcome is an error the renderer
/// cannot turn into an execution.
fn check_write_approval(
    session: &Session,
    approval: Option<&str>,
    approval_request: bool,
    request: &ToolCall,
) -> crate::Result<Option<String>> {
    let arguments = arguments_digest(request);
    match approval {
        Some(token) => {
            if session.redeem_approval(token, &request.name, &arguments) {
                Ok(None)
            } else {
                Err(error("This launcher change needs a fresh user approval"))
            }
        }
        None if approval_request => {
            Ok(Some(session.issue_approval(&request.name, &arguments)?))
        }
        None => Err(error(
            "This launcher change needs an explicit user approval",
        )),
    }
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
    include_logs: bool,
) -> crate::Result<PreparedSession> {
    uuid::Uuid::parse_str(&run_id)
        .map_err(|_| error("Invalid LogAgent run ID"))?;
    if !matches!(source.as_str(), "custom" | "logshare" | "assistant") {
        return Err(error("Unknown analysis source"));
    }
    // A remote run reads and materializes crash logs during preparation, so the
    // opt-in is checked here as well as in `cloud`: without consent no log file
    // is even read. Local sources never reach LogShare and are unaffected.
    if source == "logshare" {
        let state = State::get().await?;
        super::logshare::ensure_remote_consent(&state.pool).await?;
    }
    let session = Arc::new(Session {
        created: Instant::now(),
        cancel: CancellationToken::new(),
        snapshot: tokio::sync::OnceCell::new(),
        source,
        budget: Mutex::new(Budget::default()),
        turn_lock: tokio::sync::Mutex::new(()),
        approvals: Mutex::new(HashMap::new()),
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
            include_logs,
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
    include_logs: bool,
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
    let (files, context) = if session.source != "logshare"
        && (!include_logs || instance_id.is_empty())
    {
        (
            Vec::new(),
            "Use enabled launcher context tools when local details materially improve the answer. No instance crash snapshot was selected for this run.".to_string(),
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
    let approval = request.approval;
    let approval_request = request.approval_request;
    let request = request.call;
    if session.source == "assistant" {
        if !assistant_request_shape_valid(&request) {
            return Err(error("Unsupported launcher assistant tool request"));
        }
    } else if session.source != "custom" {
        return Err(error("Local tools are unavailable for cloud sessions"));
    }
    // The backend refuses a launcher change unless the call carries a
    // single-use grant bound to this run, this tool and these exact arguments.
    // The grant is minted by the trusted renderer path (`useLogAgent.runTool`),
    // which is reached only after the approval dialog in `agent.ts`; the model
    // never sees the token, so a tool call it drives cannot redeem one. Minting
    // performs no work: without a grant the write stops here. The consent
    // prompt itself lives in the renderer, so this gate guarantees that an
    // unapproved write cannot execute, not that a hostile renderer cannot ask
    // for a grant.
    if session.source == "assistant" && assistant_write_tool(&request.name) {
        if let Some(token) = check_write_approval(
            &session,
            approval.as_deref(),
            approval_request.unwrap_or(false),
            &request,
        )? {
            return Ok(json!({
                APPROVAL_REQUIRED: true,
                APPROVAL_TOKEN: token,
                "tool": request.name,
            }));
        }
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
            "{}:{}",
            request.name,
            serde_json::to_string(&request).unwrap_or_default(),
        );
        if assistant_write_tool(&request.name)
            || request.name == "get_install_job"
        {
            false
        } else {
            !budget.reads.insert(key)
        }
    };
    if duplicate {
        return Ok(
            json!({ "notice": "This exact request was already answered. Reuse earlier evidence or inspect another range." }),
        );
    }
    if session.source == "assistant" {
        if matches!(
            request.name.as_str(),
            "list_log_files" | "read_log_file" | "grep_log_file"
        ) {
            Ok(tools::execute(&session.snapshot()?.files, &request))
        } else {
            let result =
                tools::execute_assistant(&request, &session.cancel).await;
            if assistant_write_tool(&request.name) {
                session
                    .budget
                    .lock()
                    .map_err(|_| error("LogAgent budget unavailable"))?
                    .reads
                    .clear();
            }
            Ok(result?)
        }
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
            | "update_launcher_settings"
            | "create_instance"
            | "rename_instance"
            | "delete_instance"
            | "discover_java_installations"
            | "list_java_vendors"
            | "inspect_instance_content"
            | "check_content_updates"
            | "search_resources"
            | "get_resource_releases"
            | "prepare_resource_install"
            | "prepare_resource_update"
            | "queue_resource_install"
            | "get_install_job"
            | "cancel_install_job"
            | "install_java_runtime"
            | "set_java_default_runtime"
            | "disable_content"
            | "enable_content"
            | "remove_content"
            | "list_log_files"
            | "read_log_file"
            | "grep_log_file"
    )
}

fn assistant_write_tool(name: &str) -> bool {
    matches!(
        name,
        "update_launcher_settings"
            | "create_instance"
            | "rename_instance"
            | "delete_instance"
            | "install_java_runtime"
            | "set_java_default_runtime"
            | "queue_resource_install"
            | "cancel_install_job"
            | "disable_content"
            | "enable_content"
            | "remove_content"
    )
}

fn assistant_request_shape_valid(request: &ToolCall) -> bool {
    let allowed_fields: &[&str] = match request.name.as_str() {
        "get_instance_overview"
        | "get_launcher_settings"
        | "get_platform_diagnostics"
        | "discover_java_installations"
        | "list_java_vendors"
        | "list_log_files" => &[],
        "update_launcher_settings" => &["patch"],
        "create_instance" => {
            &["instance_name", "game_version", "loader", "loader_version"]
        }
        "rename_instance" => &["instance_id", "instance_name"],
        "delete_instance"
        | "inspect_instance_content"
        | "check_content_updates" => &["instance_id"],
        "search_resources" => &[
            "query",
            "context",
            "game_version",
            "loader",
            "source",
            "resource_type",
        ],
        "get_resource_releases" => {
            &["source", "project_id", "game_version", "loader"]
        }
        "prepare_resource_install" => &[
            "instance_id",
            "source",
            "project_id",
            "version_id",
            "resource_type",
            "install_dependencies",
        ],
        "prepare_resource_update" => &["instance_id", "content_id"],
        "queue_resource_install" => &["plan_id", "plan_revision"],
        "get_install_job" | "cancel_install_job" => &["job_id"],
        "install_java_runtime" => &["java_version", "vendor"],
        "set_java_default_runtime" => &["java_version"],
        "disable_content" | "enable_content" | "remove_content" => {
            &["instance_id", "content_id"]
        }
        "read_log_file" => &["file_id", "start_line", "end_line"],
        "grep_log_file" => &["file_id", "query", "context"],
        _ => return false,
    };
    if !only_expected_assistant_fields(request, allowed_fields) {
        return false;
    }
    let no_log_fields = request.file_id.is_empty()
        && request.start_line.is_none()
        && request.end_line.is_none()
        && request.query.is_none()
        && request.context.is_none();
    match request.name.as_str() {
        "get_instance_overview"
        | "get_launcher_settings"
        | "get_platform_diagnostics"
        | "list_java_vendors"
        | "list_log_files" => {
            no_log_fields && no_assistant_extra_fields(request)
        }
        "update_launcher_settings" => {
            no_log_fields
                && request.instance_id.is_none()
                && request.instance_name.is_none()
                && request.game_version.is_none()
                && request.loader.is_none()
                && request.loader_version.is_none()
                && request.patch.is_some()
                && request.java_version.is_none()
                && request.vendor.is_none()
                && request.source.is_none()
                && request.project_id.is_none()
                && request.version_id.is_none()
                && request.resource_type.is_none()
                && request.job_id.is_none()
                && request.plan_id.is_none()
                && request.plan_revision.is_none()
                && request.content_id.is_none()
                && request.enabled.is_none()
                && request.install_dependencies.is_none()
        }
        "create_instance" => {
            no_log_fields
                && request.instance_id.is_none()
                && request.instance_name.is_some()
                && request.game_version.is_some()
                && request
                    .loader
                    .as_deref()
                    .is_none_or(|value| !value.is_empty())
                && request
                    .loader_version
                    .as_deref()
                    .is_none_or(|value| !value.is_empty())
                && request.patch.is_none()
        }
        "rename_instance" => {
            no_log_fields
                && request.instance_id.is_some()
                && request.instance_name.is_some()
                && request.game_version.is_none()
                && request.loader.is_none()
                && request.loader_version.is_none()
                && request.patch.is_none()
        }
        "delete_instance" => {
            no_log_fields
                && request.instance_id.is_some()
                && request.instance_name.is_none()
                && request.game_version.is_none()
                && request.loader.is_none()
                && request.loader_version.is_none()
                && request.patch.is_none()
        }
        "discover_java_installations" => no_log_fields,
        "inspect_instance_content" | "check_content_updates" => {
            no_log_fields
                && request.instance_id.is_some()
                && request.instance_name.is_none()
                && request.game_version.is_none()
                && request.loader.is_none()
                && request.loader_version.is_none()
                && request.patch.is_none()
                && request.java_version.is_none()
                && request.vendor.is_none()
                && request.source.is_none()
                && request.project_id.is_none()
                && request.version_id.is_none()
                && request.resource_type.is_none()
                && request.job_id.is_none()
                && request.plan_id.is_none()
                && request.plan_revision.is_none()
                && request.content_id.is_none()
                && request.enabled.is_none()
                && request.install_dependencies.is_none()
        }
        "search_resources" => {
            request.file_id.is_empty()
                && request.start_line.is_none()
                && request.end_line.is_none()
                && request.context.is_none()
                && request
                    .query
                    .as_ref()
                    .is_some_and(|value| !value.is_empty())
                && request.instance_id.is_none()
                && request.instance_name.is_none()
                && request
                    .game_version
                    .as_ref()
                    .is_none_or(|value| !value.is_empty())
                && request
                    .loader
                    .as_ref()
                    .is_none_or(|value| !value.is_empty())
                && request.patch.is_none()
                && request.java_version.is_none()
                && request.vendor.is_none()
                && request.project_id.is_none()
                && request.version_id.is_none()
                && request
                    .resource_type
                    .as_ref()
                    .is_none_or(|value| !value.is_empty())
                && request.job_id.is_none()
                && request.plan_id.is_none()
                && request.plan_revision.is_none()
                && request.content_id.is_none()
                && request.enabled.is_none()
                && request.install_dependencies.is_none()
        }
        "get_resource_releases" => {
            no_log_fields
                && request.source.is_some()
                && request.project_id.is_some()
                && request.instance_id.is_none()
                && request.instance_name.is_none()
                && request.patch.is_none()
                && request.java_version.is_none()
                && request.vendor.is_none()
                && request.version_id.is_none()
                && request.resource_type.is_none()
                && request.job_id.is_none()
                && request.plan_id.is_none()
                && request.plan_revision.is_none()
                && request.content_id.is_none()
                && request.enabled.is_none()
                && request.install_dependencies.is_none()
        }
        "prepare_resource_install" => {
            no_log_fields
                && request.instance_id.is_some()
                && request.source.is_some()
                && request.project_id.is_some()
                && request.resource_type.is_some()
                && request.plan_id.is_none()
                && request.plan_revision.is_none()
                && request.job_id.is_none()
                && request.content_id.is_none()
                && request.java_version.is_none()
                && request.vendor.is_none()
                && request.enabled.is_none()
        }
        "prepare_resource_update" => {
            no_log_fields
                && request.instance_id.is_some()
                && request.content_id.is_some()
                && request.plan_id.is_none()
                && request.plan_revision.is_none()
                && request.job_id.is_none()
                && request.java_version.is_none()
                && request.vendor.is_none()
                && request.source.is_none()
                && request.project_id.is_none()
                && request.version_id.is_none()
                && request.resource_type.is_none()
                && request.enabled.is_none()
                && request.install_dependencies.is_none()
        }
        "queue_resource_install" => {
            no_log_fields
                && request.plan_id.is_some()
                && request.plan_revision.is_some()
                && request.instance_id.is_none()
                && request.content_id.is_none()
                && request.java_version.is_none()
                && request.vendor.is_none()
                && request.source.is_none()
                && request.project_id.is_none()
                && request.resource_type.is_none()
                && request.job_id.is_none()
                && request.enabled.is_none()
                && request.install_dependencies.is_none()
        }
        "get_install_job" => {
            no_log_fields
                && request.job_id.is_some()
                && request.plan_id.is_none()
                && request.plan_revision.is_none()
                && request.instance_id.is_none()
                && request.content_id.is_none()
                && request.java_version.is_none()
                && request.vendor.is_none()
                && request.source.is_none()
                && request.project_id.is_none()
                && request.resource_type.is_none()
                && request.enabled.is_none()
                && request.install_dependencies.is_none()
        }
        "cancel_install_job" => {
            no_log_fields
                && request.job_id.is_some()
                && request.plan_id.is_none()
                && request.plan_revision.is_none()
                && request.instance_id.is_none()
                && request.content_id.is_none()
                && request.java_version.is_none()
                && request.vendor.is_none()
                && request.source.is_none()
                && request.project_id.is_none()
                && request.resource_type.is_none()
                && request.enabled.is_none()
                && request.install_dependencies.is_none()
        }
        "install_java_runtime" => {
            no_log_fields
                && request.java_version.is_some()
                && request
                    .vendor
                    .as_ref()
                    .is_some_and(|value| !value.is_empty())
                && request.instance_id.is_none()
                && request.plan_id.is_none()
                && request.plan_revision.is_none()
                && request.job_id.is_none()
                && request.content_id.is_none()
                && request.source.is_none()
                && request.project_id.is_none()
                && request.resource_type.is_none()
                && request.enabled.is_none()
                && request.install_dependencies.is_none()
        }
        "set_java_default_runtime" => {
            no_log_fields
                && request.java_version.is_some()
                && request.vendor.is_none()
                && request.instance_id.is_none()
                && request.plan_id.is_none()
                && request.plan_revision.is_none()
                && request.job_id.is_none()
                && request.content_id.is_none()
                && request.source.is_none()
                && request.project_id.is_none()
                && request.resource_type.is_none()
                && request.enabled.is_none()
                && request.install_dependencies.is_none()
        }
        "disable_content" | "enable_content" | "remove_content" => {
            no_log_fields
                && request.instance_id.is_some()
                && request.content_id.is_some()
                && request.patch.is_none()
                && request.java_version.is_none()
                && request.vendor.is_none()
                && request.source.is_none()
                && request.project_id.is_none()
                && request.resource_type.is_none()
                && request.job_id.is_none()
                && request.plan_id.is_none()
                && request.plan_revision.is_none()
                && request.enabled.is_none()
                && request.install_dependencies.is_none()
        }
        "read_log_file" => {
            no_assistant_extra_fields(request)
                && !request.file_id.is_empty()
                && request.query.is_none()
                && request.context.is_none()
        }
        "grep_log_file" => {
            no_assistant_extra_fields(request)
                && !request.file_id.is_empty()
                && request
                    .query
                    .as_ref()
                    .is_some_and(|value| !value.is_empty())
                && request.start_line.is_none()
                && request.end_line.is_none()
        }
        _ => false,
    }
}

fn only_expected_assistant_fields(
    request: &ToolCall,
    allowed: &[&str],
) -> bool {
    let Ok(Value::Object(mut fields)) = serde_json::to_value(request) else {
        return false;
    };
    fields.remove("name");
    fields.remove("file_id");
    fields.into_iter().all(|(key, value)| {
        value.is_null()
            || value.as_str() == Some("")
            || allowed.contains(&key.as_str())
    })
}

fn no_assistant_extra_fields(request: &ToolCall) -> bool {
    request.instance_id.is_none()
        && request.instance_name.is_none()
        && request.game_version.is_none()
        && request.loader.is_none()
        && request.loader_version.is_none()
        && request.patch.is_none()
        && request.java_version.is_none()
        && request.vendor.is_none()
        && request.source.is_none()
        && request.project_id.is_none()
        && request.version_id.is_none()
        && request.resource_type.is_none()
        && request.job_id.is_none()
        && request.plan_id.is_none()
        && request.plan_revision.is_none()
        && request.content_id.is_none()
        && request.enabled.is_none()
        && request.install_dependencies.is_none()
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
    // Refuse before the snapshot is materialized. The snapshot is only memory
    // and no request has been made yet, but checking first keeps the rule that
    // nothing about the logs is assembled for a remote service the user has
    // not allowed.
    let state = State::get().await?;
    super::logshare::ensure_remote_consent(&state.pool).await?;
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

    fn make_session(source: &str) -> Arc<Session> {
        Arc::new(Session {
            created: Instant::now(),
            cancel: CancellationToken::new(),
            snapshot: tokio::sync::OnceCell::new(),
            source: source.into(),
            budget: Mutex::new(Budget::default()),
            turn_lock: tokio::sync::Mutex::new(()),
            approvals: Mutex::new(HashMap::new()),
        })
    }

    fn write_request(name: &str, instance: &str) -> ToolCall {
        let mut request: ToolCall =
            serde_json::from_value(json!({ "name": name })).unwrap();
        request.instance_id = Some(instance.to_string());
        request
    }

    fn wrap(call: ToolCall, approval: Option<&str>) -> ToolRequest {
        ToolRequest {
            call,
            approval: approval.map(str::to_string),
            approval_request: None,
        }
    }

    /// Structural pin: `cloud` must consult the consent gate before it touches
    /// the snapshot, and `prepare` must gate a `logshare` run before any crash
    /// log is read. Both are read from this module's own source, so reordering
    /// the guard below `session.snapshot()` or below `analyze_crash` fails here.
    #[test]
    fn remote_analysis_checks_consent_before_reading_or_materializing_logs() {
        const SOURCE: &str = include_str!("mod.rs");

        fn body_after<'a>(source: &'a str, signature: &str) -> &'a str {
            let start = source
                .find(signature)
                .unwrap_or_else(|| panic!("{signature} not found"));
            &source[start..]
        }

        let cloud = body_after(SOURCE, "pub async fn cloud(");
        let cloud_gate = cloud
            .find("ensure_remote_consent")
            .expect("cloud must call the consent gate");
        let cloud_snapshot = cloud
            .find("session.snapshot()?")
            .expect("cloud must materialize the snapshot");
        assert!(
            cloud_gate < cloud_snapshot,
            "cloud must check consent before session.snapshot()"
        );

        let prepare = body_after(SOURCE, "pub async fn prepare(");
        let prepare_gate = prepare
            .find("ensure_remote_consent")
            .expect("prepare must gate a logshare run");
        let prepare_crash = prepare
            .find("prepare_snapshot(")
            .expect("prepare must delegate to prepare_snapshot");
        assert!(
            prepare_gate < prepare_crash,
            "prepare must check consent before prepare_snapshot reads crash logs"
        );
        // The gate must only apply to the remote source.
        assert!(
            prepare[..prepare_gate].contains("source == \"logshare\""),
            "the prepare-time gate must be conditional on the logshare source"
        );
    }

    #[tokio::test]
    async fn the_tool_entry_point_refuses_a_write_that_presents_no_grant() {
        let run_id = uuid::Uuid::new_v4().to_string();
        let session = make_session("assistant");
        sessions().unwrap().insert(run_id.clone(), session);

        // delete_instance is a real launcher write and its shape is valid here,
        // so the only thing that can stop it is the approval gate in `tool`.
        let call = write_request("delete_instance", "instance-1");
        let error = tool(&run_id, wrap(call, None)).await.unwrap_err();
        assert!(
            format!("{error}").contains("explicit user approval"),
            "the gate must refuse before any launcher state is touched: {error:#}"
        );

        // Asking for an approval mints a token but still performs no work.
        let call = write_request("delete_instance", "instance-1");
        let minted = tool(
            &run_id,
            ToolRequest {
                call,
                approval: None,
                approval_request: Some(true),
            },
        )
        .await
        .unwrap();
        assert_eq!(minted[APPROVAL_REQUIRED], json!(true));
        assert_eq!(minted[APPROVAL_TOKEN].as_str().map(str::len), Some(64));

        cancel(&run_id).unwrap();
    }

    #[test]
    fn a_forged_token_never_authorizes_a_write() {
        let session = make_session("assistant");
        let call = write_request("delete_instance", "instance-1");
        let arguments = arguments_digest(&call);
        assert!(!session.redeem_approval("forged", &call.name, &arguments));
        assert!(!session.redeem_approval("", &call.name, &arguments));
        assert_eq!(arguments.len(), 64);
    }

    #[test]
    fn an_approval_is_bound_to_the_tool_and_arguments_it_was_issued_for() {
        let session = make_session("assistant");
        let call = write_request("delete_instance", "instance-1");
        let arguments = arguments_digest(&call);

        // A grant for instance-1 must not delete instance-2.
        let token = session.issue_approval(&call.name, &arguments).unwrap();
        let other_instance = write_request("delete_instance", "instance-2");
        assert!(!session.redeem_approval(
            &token,
            &other_instance.name,
            &arguments_digest(&other_instance)
        ));

        // A grant for delete_instance must not remove content instead.
        let token = session.issue_approval(&call.name, &arguments).unwrap();
        let other_tool = write_request("remove_content", "instance-1");
        assert!(!session.redeem_approval(
            &token,
            &other_tool.name,
            &arguments_digest(&other_tool)
        ));

        // The matching call is the only one the grant authorizes.
        let token = session.issue_approval(&call.name, &arguments).unwrap();
        assert!(session.redeem_approval(&token, &call.name, &arguments));
    }

    #[test]
    fn a_valid_approval_is_accepted_exactly_once() {
        let session = make_session("assistant");
        let call = write_request("delete_instance", "instance-1");
        let arguments = arguments_digest(&call);
        let token = session.issue_approval(&call.name, &arguments).unwrap();

        assert!(session.redeem_approval(&token, &call.name, &arguments));
        assert!(
            !session.redeem_approval(&token, &call.name, &arguments),
            "a redeemed grant must not be reusable"
        );
    }

    #[test]
    fn expired_approvals_are_rejected() {
        let session = make_session("assistant");
        let call = write_request("delete_instance", "instance-1");
        let arguments = arguments_digest(&call);
        let token = session.issue_approval(&call.name, &arguments).unwrap();
        {
            let mut approvals = session.approvals.lock().unwrap();
            if let Some(approval) = approvals.get_mut(&token) {
                approval.expires = Instant::now() - Duration::from_secs(1);
            }
        }
        assert!(!session.redeem_approval(&token, &call.name, &arguments));
    }

    #[test]
    fn wrapping_a_read_only_call_keeps_its_fields_untouched() {
        let call: ToolCall = serde_json::from_value(
            json!({ "name": "get_install_job", "job_id": "job-1" }),
        )
        .unwrap();
        let request = wrap(call, None);
        assert_eq!(request.call.name, "get_install_job");
        assert_eq!(request.call.job_id.as_deref(), Some("job-1"));
        assert!(request.approval.is_none());
    }

    #[test]
    fn a_write_without_a_grant_is_refused_instead_of_running() {
        let session = make_session("assistant");
        let call = write_request("delete_instance", "instance-1");
        let outcome = check_write_approval(&session, None, false, &call);
        let message = format!("{}", outcome.unwrap_err());
        assert!(message.contains("explicit user approval"), "{message}");
        assert!(
            session.approvals.lock().unwrap().is_empty(),
            "a refused write must not leave a redeemable grant behind"
        );
    }

    #[test]
    fn only_an_explicit_request_mints_a_grant() {
        let session = make_session("assistant");
        let call = write_request("delete_instance", "instance-1");
        let token = check_write_approval(&session, None, true, &call)
            .unwrap()
            .expect("an explicit request must mint a token");
        assert_eq!(token.len(), 64);
        assert!(
            check_write_approval(&session, Some(&token), false, &call)
                .unwrap()
                .is_none(),
            "the minted grant must authorize exactly the call it was issued for"
        );
        assert!(
            check_write_approval(&session, Some(&token), false, &call).is_err(),
            "the same grant must not authorize a second call"
        );
    }

    #[test]
    fn cancellation_cannot_affect_another_run() {
        let first = uuid::Uuid::new_v4().to_string();
        let second = uuid::Uuid::new_v4().to_string();
        let old = make_session("custom");
        sessions().unwrap().insert(first.clone(), old.clone());
        sessions()
            .unwrap()
            .insert(second.clone(), make_session("custom"));
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
    fn assistant_tool_allowlist_includes_bounded_writes_and_remote_mcp_aliases()
    {
        assert!(allowed_tool_name("assistant", "get_instance_overview"));
        assert!(allowed_tool_name("assistant", "get_launcher_settings"));
        assert!(allowed_tool_name("assistant", "get_platform_diagnostics"));
        assert!(allowed_tool_name("assistant", "update_launcher_settings"));
        assert!(allowed_tool_name("assistant", "create_instance"));
        assert!(allowed_tool_name("assistant", "rename_instance"));
        assert!(allowed_tool_name("assistant", "delete_instance"));
        assert!(allowed_tool_name(
            "assistant",
            "discover_java_installations"
        ));
        assert!(allowed_tool_name("assistant", "install_java_runtime"));
        assert!(allowed_tool_name("assistant", "search_resources"));
        assert!(allowed_tool_name("assistant", "prepare_resource_install"));
        assert!(allowed_tool_name("assistant", "prepare_resource_update"));
        assert!(allowed_tool_name("assistant", "queue_resource_install"));
        assert!(allowed_tool_name("assistant", "disable_content"));
        assert!(allowed_tool_name("assistant", "mcp_0_lookup_server"));
        assert!(allowed_tool_name("assistant", "list_log_files"));
        assert!(!allowed_tool_name("assistant", "execute"));
        assert!(!allowed_tool_name("custom", "get_launcher_settings"));
        assert!(allowed_tool_name("custom", "read_log_file"));
        assert!(!remote_mcp_tool_name("mcp_x_lookup_server"));
        assert!(!remote_mcp_tool_name("mcp_0_execute;"));
    }
}
