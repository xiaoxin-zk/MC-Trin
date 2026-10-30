//! AI provider catalog, credentials, and text-generation adapters.

mod catalog;
pub mod chat;
mod credentials;
mod protocol;
pub use catalog::catalog;
#[cfg(test)]
use catalog::{BUILTIN_MODEL_CATALOG, SPONSORED_PROVIDERS};
use catalog::{PROVIDERS, builtin_models, provider_definition};
use protocol::{
    ai_network_error, apply_openai_response_format,
    apply_openai_translation_options, apply_responses_translation_options,
    checked_openai_response_stream, google_generation_config,
    insert_json_option, openai_content, openai_response_stream_content,
    opencode_headers,
};

use std::collections::HashMap;
use std::sync::{LazyLock, Mutex};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use base64::Engine;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use hmac::{Hmac, Mac};
use jsonwebtoken::{Algorithm, EncodingKey, Header};
use keyring::Entry;
use reqwest::{Response, StatusCode};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use sqlx::Row;
use url::Url;
use uuid::Uuid;

use crate::{ErrorKind, State};

const CATALOG_SOURCE: &str = "LobeHub 49204982cab98bfa2f2fb6831a34d5018a2af6be";
const KEYRING_PREFIX: &str = "ai-provider";
const OAUTH_CREDENTIAL: &str = "oauth";
const API_KEY_CREDENTIAL: &str = "api-key";
const AWS_ACCESS_KEY_ID_CREDENTIAL: &str = "aws-access-key-id";
const AWS_SECRET_ACCESS_KEY_CREDENTIAL: &str = "aws-secret-access-key";
const AWS_SESSION_TOKEN_CREDENTIAL: &str = "aws-session-token";
const VERTEX_SERVICE_ACCOUNT_CREDENTIAL: &str = "vertex-service-account";
const CHATGPT_DEVICE_CODE_TTL_SECONDS: u64 = 15 * 60;
const CHATGPT_POLLING_SAFETY_MARGIN_SECONDS: u64 = 3;
const OAUTH_REFRESH_SKEW_SECONDS: u64 = 120;
const OAUTH_DEFAULT_TOKEN_TTL_SECONDS: u64 = 60 * 60;

static AI_CLIENT: LazyLock<reqwest::Client> = LazyLock::new(|| {
    reqwest::Client::builder()
        .timeout(Duration::from_secs(45))
        .user_agent(crate::launcher_user_agent())
        .build()
        .expect("AI client configuration should be valid")
});

static AI_TRANSLATION_CLIENT: LazyLock<reqwest::Client> = LazyLock::new(|| {
    reqwest::Client::builder()
        .timeout(Duration::from_secs(90))
        .user_agent(crate::launcher_user_agent())
        .build()
        .expect("AI translation client configuration should be valid")
});

static OAUTH_FLOWS: LazyLock<Mutex<HashMap<Uuid, PendingOAuthFlow>>> =
    LazyLock::new(|| Mutex::new(HashMap::new()));
static OAUTH_REFRESH_LOCK: LazyLock<tokio::sync::Mutex<()>> =
    LazyLock::new(|| tokio::sync::Mutex::new(()));
static OAUTH_POLL_LOCK: LazyLock<tokio::sync::Mutex<()>> =
    LazyLock::new(|| tokio::sync::Mutex::new(()));
static OAUTH_CREDENTIAL_LOCK: Mutex<()> = Mutex::new(());
static VERTEX_ACCESS_TOKEN: LazyLock<Mutex<Option<CachedVertexAccessToken>>> =
    LazyLock::new(|| Mutex::new(None));

#[derive(Debug, Clone, Copy, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum AiProtocol {
    Openai,
    Anthropic,
    Google,
    Ollama,
    Azure,
    AzureAi,
    Bedrock,
    Cloudflare,
    Huggingface,
    Router,
}

impl AiProtocol {
    fn as_str(self) -> &'static str {
        match self {
            Self::Openai => "openai",
            Self::Anthropic => "anthropic",
            Self::Google => "google",
            Self::Ollama => "ollama",
            Self::Azure => "azure",
            Self::AzureAi => "azure-ai",
            Self::Bedrock => "bedrock",
            Self::Cloudflare => "cloudflare",
            Self::Huggingface => "huggingface",
            Self::Router => "router",
        }
    }

    fn from_str(value: &str) -> crate::Result<Self> {
        match value {
            "openai" => Ok(Self::Openai),
            "anthropic" => Ok(Self::Anthropic),
            "google" => Ok(Self::Google),
            "ollama" => Ok(Self::Ollama),
            "azure" => Ok(Self::Azure),
            "azure-ai" | "azureai" => Ok(Self::AzureAi),
            "bedrock" => Ok(Self::Bedrock),
            "cloudflare" => Ok(Self::Cloudflare),
            "huggingface" => Ok(Self::Huggingface),
            "router" => Ok(Self::Router),
            _ => Err(input_error(format!("Unknown AI protocol: {value}"))),
        }
    }
}

#[derive(Debug, Clone, Copy, Eq, PartialEq, Serialize)]
pub enum AiAuthType {
    #[serde(rename = "apiKey")]
    ApiKey,
    #[serde(rename = "oauthDeviceFlow")]
    OAuthDeviceFlow,
    #[serde(rename = "none")]
    None,
}

#[derive(Debug, Clone, Copy, Serialize)]
pub struct AiProviderDefinition {
    pub id: &'static str,
    pub name: &'static str,
    pub protocol: AiProtocol,
    pub auth_type: AiAuthType,
    pub default_endpoint: &'static str,
    pub check_model: &'static str,
    pub show_model_fetcher: bool,
    pub required_settings: &'static [&'static str],
    #[serde(default)]
    pub sponsored: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AiSettings {
    pub enabled: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AiProviderModel {
    pub id: String,
    pub name: String,
    pub enabled: bool,
    pub source: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AiProviderConfig {
    pub provider_id: String,
    pub custom_name: Option<String>,
    pub protocol: AiProtocol,
    pub enabled: bool,
    pub endpoint: String,
    #[serde(default)]
    pub settings: HashMap<String, String>,
    pub has_api_key: bool,
    pub configured_credentials: Vec<String>,
    pub oauth_connected: bool,
    pub models: Vec<AiProviderModel>,
}

#[derive(Debug, Clone, Serialize)]
pub struct AiState {
    pub settings: AiSettings,
    pub catalog_source: &'static str,
    pub providers: Vec<AiProviderConfig>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AiProviderConfigUpdate {
    pub provider_id: String,
    pub custom_name: Option<String>,
    pub enabled: bool,
    pub endpoint: String,
    #[serde(default)]
    pub settings: HashMap<String, String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AiModelUpdate {
    pub provider_id: String,
    pub model_id: String,
    pub display_name: String,
    pub enabled: bool,
}

#[derive(Debug, Clone)]
pub struct AiTextRequest {
    pub provider_id: String,
    pub model_id: String,
    pub system_prompt: String,
    pub user_prompt: String,
    pub mode: AiTextMode,
    pub response_format: AiTextResponseFormat,
}

#[derive(Debug, Clone, Copy, Default, Eq, PartialEq)]
pub enum AiTextMode {
    #[default]
    Default,
    Translation,
}

#[derive(Debug, Clone, Copy, Default, Eq, PartialEq)]
pub enum AiTextResponseFormat {
    #[default]
    Text,
    JsonObject,
}

#[derive(Debug, Clone, Serialize)]
pub struct OAuthDeviceCode {
    pub flow_id: Uuid,
    pub user_code: String,
    pub verification_uri: String,
    pub verification_uri_complete: Option<String>,
    pub expires_in: u64,
    pub interval: u64,
}

#[derive(Debug, Clone, Copy, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum OAuthPollStatus {
    Pending,
    Success,
    Expired,
    Denied,
    SlowDown,
}

#[derive(Debug, Clone)]
struct PendingOAuthFlow {
    provider_id: String,
    device_code: String,
    expires_at: u64,
    credentials: Option<OAuthCredentials>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
struct OAuthCredentials {
    access_token: String,
    refresh_token: Option<String>,
    expires_at: Option<u64>,
    account_id: Option<String>,
    bearer_token: Option<String>,
    bearer_token_expires_at: Option<u64>,
}

#[derive(Debug, Clone)]
struct CachedVertexAccessToken {
    credential_fingerprint: String,
    access_token: String,
    expires_at: u64,
}

#[derive(Debug, Deserialize)]
struct VertexServiceAccount {
    client_email: String,
    private_key: String,
    project_id: String,
    #[serde(default = "default_google_token_uri")]
    token_uri: String,
}

#[derive(Debug, Serialize)]
struct VertexJwtClaims<'a> {
    iss: &'a str,
    scope: &'static str,
    aud: &'a str,
    iat: u64,
    exp: u64,
}

fn default_google_token_uri() -> String {
    "https://oauth2.googleapis.com/token".to_string()
}

fn parse_vertex_service_account(
    service_account_json: &str,
) -> crate::Result<VertexServiceAccount> {
    let account: VertexServiceAccount =
        serde_json::from_str(service_account_json).map_err(|_| {
            input_error(
                "Vertex AI credentials must be a service-account JSON document"
                    .to_string(),
            )
        })?;
    if account.client_email.trim().is_empty()
        || account.private_key.trim().is_empty()
        || account.project_id.trim().is_empty()
    {
        return Err(input_error(
            "Vertex AI service-account credentials are incomplete".to_string(),
        ));
    }
    let token_uri = Url::parse(&account.token_uri).map_err(|_| {
        input_error("Vertex AI token URI must be a valid HTTPS URL".to_string())
    })?;
    if token_uri.scheme() != "https" {
        return Err(input_error(
            "Vertex AI token URI must be a valid HTTPS URL".to_string(),
        ));
    }
    Ok(account)
}

fn vertex_jwt_claims(
    account: &VertexServiceAccount,
    issued_at: u64,
) -> VertexJwtClaims<'_> {
    VertexJwtClaims {
        iss: &account.client_email,
        scope: "https://www.googleapis.com/auth/cloud-platform",
        aud: &account.token_uri,
        iat: issued_at,
        exp: issued_at + 3600,
    }
}

#[derive(Debug, Clone, Copy)]
struct OAuthDefinition {
    client_id: &'static str,
    device_code_endpoint: &'static str,
    token_endpoint: &'static str,
    token_exchange_endpoint: Option<&'static str>,
    scope: &'static str,
    interval: u64,
}

fn oauth_definition(provider_id: &str) -> Option<OAuthDefinition> {
    match provider_id {
        "githubcopilot" => Some(OAuthDefinition {
            client_id: "Iv1.b507a08c87ecfe98",
            device_code_endpoint: "https://github.com/login/device/code",
            token_endpoint: "https://github.com/login/oauth/access_token",
            token_exchange_endpoint: Some(
                "https://api.github.com/copilot_internal/v2/token",
            ),
            scope: "read:user",
            interval: 5,
        }),
        "chatgpt" => Some(OAuthDefinition {
            client_id: "app_EMoamEEZ73f0CkXaXp7hrann",
            device_code_endpoint: "https://auth.openai.com/api/accounts/deviceauth/usercode",
            token_endpoint: "https://auth.openai.com/oauth/token",
            token_exchange_endpoint: Some(
                "https://auth.openai.com/api/accounts/deviceauth/token",
            ),
            scope: "",
            interval: 8,
        }),
        "supergrok" => Some(OAuthDefinition {
            client_id: "b1a00492-073a-47ea-816f-4c329264a828",
            device_code_endpoint: "https://auth.x.ai/oauth2/device/code",
            token_endpoint: "https://auth.x.ai/oauth2/token",
            token_exchange_endpoint: None,
            scope: "openid profile email offline_access grok-cli:access api:access",
            interval: 5,
        }),
        _ => None,
    }
}

fn input_error(message: String) -> crate::Error {
    ErrorKind::InputError(message).into()
}

fn normalize_endpoint(endpoint: &str, fallback: &str) -> crate::Result<String> {
    let endpoint = if endpoint.trim().is_empty() {
        fallback.trim()
    } else {
        endpoint.trim()
    };
    if endpoint.is_empty() {
        return Ok(String::new());
    }
    let url = Url::parse(endpoint).map_err(|_| {
        input_error("AI provider endpoint must be a valid URL".to_string())
    })?;
    if !matches!(url.scheme(), "http" | "https") {
        return Err(input_error(
            "AI provider endpoint must use HTTP or HTTPS".to_string(),
        ));
    }
    Ok(endpoint.trim_end_matches('/').to_string())
}

fn now_seconds() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

fn jwt_claims(token: &str) -> Option<Value> {
    let mut parts = token.split('.');
    let _header = parts.next()?;
    let payload = parts.next()?;
    parts.next()?;
    if parts.next().is_some() {
        return None;
    }
    let decoded = URL_SAFE_NO_PAD.decode(payload).ok()?;
    serde_json::from_slice(&decoded).ok()
}

fn jwt_expiry(token: &str) -> Option<u64> {
    jwt_claims(token)?.get("exp")?.as_u64()
}

fn json_u64(value: &Value, key: &str) -> Option<u64> {
    value.get(key).and_then(|value| {
        value
            .as_u64()
            .or_else(|| value.as_str().and_then(|value| value.parse().ok()))
    })
}

fn oauth_token_expiry(value: &Value, access_token: &str) -> Option<u64> {
    json_u64(value, "expires_in")
        .map(|seconds| now_seconds() + seconds)
        .or_else(|| jwt_expiry(access_token))
}

fn jwt_account_id(token: &str) -> Option<String> {
    let claims = jwt_claims(token)?;
    claims
        .get("chatgpt_account_id")
        .or_else(|| {
            claims.pointer("/https:~1~1api.openai.com~1auth/chatgpt_account_id")
        })
        .and_then(Value::as_str)
        .map(str::to_string)
        .or_else(|| {
            claims
                .get("organizations")
                .and_then(Value::as_array)
                .and_then(|items| items.first())
                .and_then(|item| item.get("id"))
                .and_then(Value::as_str)
                .map(str::to_string)
        })
}

fn credential_entry(
    provider_id: &str,
    credential: &str,
) -> crate::Result<Entry> {
    provider_definition(provider_id)?;
    // Same name the data directory uses, so a build with its own directory
    // (AXOLOTL_DATA_DIR_SUFFIX) also keeps its own credentials. A release
    // resolves to the plain identifier and is unaffected.
    Entry::new(
        &crate::brand::app_data_dir_identifier(crate::brand::BUNDLE_IDENTIFIER),
        &format!("{KEYRING_PREFIX}:{provider_id}:{credential}"),
    )
    .map_err(|error| {
        ErrorKind::OtherError(format!(
            "Could not open the system credential store: {error}"
        ))
        .as_error()
    })
}

fn credential_names(provider_id: &str) -> &'static [&'static str] {
    match provider_id {
        "bedrock" => &[
            API_KEY_CREDENTIAL,
            AWS_ACCESS_KEY_ID_CREDENTIAL,
            AWS_SECRET_ACCESS_KEY_CREDENTIAL,
            AWS_SESSION_TOKEN_CREDENTIAL,
        ],
        "vertexai" => &[VERTEX_SERVICE_ACCOUNT_CREDENTIAL],
        _ => &[API_KEY_CREDENTIAL],
    }
}

fn validate_credential_name(
    provider_id: &str,
    credential: &str,
) -> crate::Result<()> {
    let definition = provider_definition(provider_id)?;
    if definition.auth_type == AiAuthType::OAuthDeviceFlow {
        return Err(input_error(
            "This provider uses OAuth authorization instead of stored credentials"
                .to_string(),
        ));
    }
    if !credential_names(provider_id).contains(&credential) {
        return Err(input_error(format!(
            "Unsupported credential for {}",
            definition.name
        )));
    }
    Ok(())
}

fn read_credential(
    provider_id: &str,
    credential: &str,
) -> crate::Result<Option<String>> {
    Ok(read_stored_credential(provider_id, credential)?
        .filter(|value| !value.trim().is_empty()))
}

fn read_stored_credential(
    provider_id: &str,
    credential: &str,
) -> crate::Result<Option<String>> {
    match credential_entry(provider_id, credential)?.get_password() {
        Ok(value) => Ok(Some(value)),
        Err(keyring::Error::NoEntry) => Ok(None),
        Err(error) => Err(ErrorKind::OtherError(format!(
            "Could not read the system credential store: {error}"
        ))
        .into()),
    }
}

fn write_credential(
    provider_id: &str,
    credential: &str,
    value: Option<&str>,
) -> crate::Result<()> {
    write_stored_credential(
        provider_id,
        credential,
        value.map(str::trim).filter(|value| !value.is_empty()),
    )
}

fn write_stored_credential(
    provider_id: &str,
    credential: &str,
    value: Option<&str>,
) -> crate::Result<()> {
    let entry = credential_entry(provider_id, credential)?;
    match value {
        Some(value) => entry.set_password(value).map_err(|error| {
            ErrorKind::OtherError(format!(
                "Could not write the system credential store: {error}"
            ))
            .as_error()
        }),
        None => match entry.delete_credential() {
            Ok(()) | Err(keyring::Error::NoEntry) => Ok(()),
            Err(error) => Err(ErrorKind::OtherError(format!(
                "Could not update the system credential store: {error}"
            ))
            .into()),
        },
    }
}

fn read_oauth_credentials(
    provider_id: &str,
) -> crate::Result<Option<OAuthCredentials>> {
    let _guard = OAUTH_CREDENTIAL_LOCK.lock().map_err(|_| {
        ErrorKind::OtherError(
            "OAuth credential storage is unavailable".to_string(),
        )
    })?;
    credentials::read(&OAuthCredentialStore(provider_id))?
        .map(|value| {
            serde_json::from_str(&value).map_err(|_| {
                ErrorKind::OtherError(
					"Stored AI OAuth credentials are invalid; reconnect the provider"
						.to_string(),
				)
				.as_error()
            })
        })
        .transpose()
}

fn write_oauth_credentials(
    provider_id: &str,
    credentials: Option<&OAuthCredentials>,
) -> crate::Result<()> {
    let _guard = OAUTH_CREDENTIAL_LOCK.lock().map_err(|_| {
        ErrorKind::OtherError(
            "OAuth credential storage is unavailable".to_string(),
        )
    })?;
    let serialized = credentials
        .map(serde_json::to_string)
        .transpose()
        .map_err(|error| ErrorKind::OtherError(error.to_string()))?;
    credentials::write(
        &OAuthCredentialStore(provider_id),
        serialized.as_deref(),
    )
}

struct OAuthCredentialStore<'a>(&'a str);

impl credentials::Store for OAuthCredentialStore<'_> {
    fn read(&self, name: &str) -> crate::Result<Option<String>> {
        read_stored_credential(self.0, name)
    }

    fn write(&self, name: &str, value: Option<&str>) -> crate::Result<()> {
        write_stored_credential(self.0, name, value)
    }
}

async fn discard_legacy_openai_secret() -> crate::Result<()> {
    let state = State::get().await?;
    let cleanup = sqlx::query_scalar::<_, i64>(
        "SELECT legacy_openai_credential_cleanup FROM ai_settings WHERE id = 0",
    )
    .fetch_one(&state.pool)
    .await?
        != 0;
    if !cleanup {
        return Ok(());
    }
    write_credential("openai", API_KEY_CREDENTIAL, None)?;
    sqlx::query(
        "UPDATE ai_settings SET legacy_openai_credential_cleanup = FALSE WHERE id = 0",
    )
    .execute(&state.pool)
    .await?;
    Ok(())
}

async fn load_provider_models(
    provider_id: &str,
) -> crate::Result<Vec<AiProviderModel>> {
    let state = State::get().await?;
    let rows = sqlx::query(
		"SELECT model_id, display_name, enabled, source FROM ai_provider_models WHERE provider_id = ? ORDER BY display_name, model_id",
	)
	.bind(provider_id)
	.fetch_all(&state.pool)
	.await?;
    let mut stored = rows
        .into_iter()
        .map(|row| {
            let model = AiProviderModel {
                id: row.get("model_id"),
                name: row.get::<String, _>("display_name"),
                enabled: row.get::<i64, _>("enabled") != 0,
                source: row.get("source"),
            };
            (model.id.clone(), model)
        })
        .collect::<HashMap<_, _>>();
    let mut models = builtin_models(provider_id)
        .iter()
        .map(|builtin| {
            let saved = stored.remove(&builtin.id);
            AiProviderModel {
                id: builtin.id.clone(),
                name: saved
                    .as_ref()
                    .map(|model| model.name.clone())
                    .filter(|name| !name.is_empty())
                    .unwrap_or_else(|| builtin.name.clone()),
                enabled: saved
                    .as_ref()
                    .map_or(builtin.enabled, |model| model.enabled),
                source: "builtin".to_string(),
            }
        })
        .collect::<Vec<_>>();
    let mut additional = stored.into_values().collect::<Vec<_>>();
    additional.sort_by(|left, right| {
        left.name
            .cmp(&right.name)
            .then_with(|| left.id.cmp(&right.id))
    });
    models.extend(additional);
    Ok(models)
}

async fn load_provider_config(
    definition: &AiProviderDefinition,
) -> crate::Result<AiProviderConfig> {
    let state = State::get().await?;
    let row = sqlx::query(
		"SELECT custom_name, protocol, enabled, endpoint, settings FROM ai_provider_configs WHERE provider_id = ?",
	)
	.bind(definition.id)
	.fetch_optional(&state.pool)
	.await?;
    let (custom_name, protocol, enabled, endpoint, settings) = match row {
        Some(row) => {
            let settings = serde_json::from_str::<HashMap<String, String>>(
                &row.get::<String, _>("settings"),
            )
            .unwrap_or_default();
            (
                row.get("custom_name"),
                AiProtocol::from_str(&row.get::<String, _>("protocol"))?,
                row.get::<i64, _>("enabled") != 0,
                normalize_endpoint(
                    &row.get::<String, _>("endpoint"),
                    definition.default_endpoint,
                )?,
                settings,
            )
        }
        None => (
            None,
            definition.protocol,
            false,
            definition.default_endpoint.to_string(),
            HashMap::new(),
        ),
    };
    let mut models = load_provider_models(definition.id).await?;
    if models.is_empty() && !definition.check_model.is_empty() {
        models.push(AiProviderModel {
            id: definition.check_model.to_string(),
            name: definition.check_model.to_string(),
            enabled: true,
            source: "builtin".to_string(),
        });
    }
    let mut configured_credentials = Vec::new();
    for credential in credential_names(definition.id) {
        if read_credential(definition.id, credential)?.is_some() {
            configured_credentials.push((*credential).to_string());
        }
    }
    Ok(AiProviderConfig {
        provider_id: definition.id.to_string(),
        custom_name,
        protocol,
        enabled,
        endpoint,
        settings,
        has_api_key: read_credential(definition.id, API_KEY_CREDENTIAL)?
            .is_some(),
        configured_credentials,
        oauth_connected: read_oauth_credentials(definition.id)?.is_some(),
        models,
    })
}

#[tracing::instrument]
pub async fn get_state() -> crate::Result<AiState> {
    discard_legacy_openai_secret().await?;
    let state = State::get().await?;
    let enabled = sqlx::query_scalar::<_, i64>(
        "SELECT enabled FROM ai_settings WHERE id = 0",
    )
    .fetch_one(&state.pool)
    .await?
        != 0;
    let mut providers = Vec::with_capacity(PROVIDERS.len());
    for definition in PROVIDERS {
        providers.push(load_provider_config(definition).await?);
    }
    Ok(AiState {
        settings: AiSettings { enabled },
        catalog_source: CATALOG_SOURCE,
        providers,
    })
}

#[tracing::instrument]
pub async fn update_settings(settings: AiSettings) -> crate::Result<()> {
    let state = State::get().await?;
    sqlx::query("UPDATE ai_settings SET enabled = ? WHERE id = 0")
        .bind(settings.enabled)
        .execute(&state.pool)
        .await?;
    Ok(())
}

#[tracing::instrument(skip(update))]
pub async fn update_provider(
    update: AiProviderConfigUpdate,
) -> crate::Result<()> {
    let definition = provider_definition(&update.provider_id)?;
    let endpoint =
        normalize_endpoint(&update.endpoint, definition.default_endpoint)?;
    if update.enabled
        && endpoint.is_empty()
        && definition.protocol != AiProtocol::Bedrock
        && definition.id != "vertexai"
    {
        return Err(input_error(
            "Configure the provider endpoint before enabling it".to_string(),
        ));
    }
    let settings = serde_json::to_string(&update.settings)
        .map_err(|error| ErrorKind::OtherError(error.to_string()))?;
    let state = State::get().await?;
    sqlx::query(
		"INSERT INTO ai_provider_configs (provider_id, custom_name, protocol, enabled, endpoint, settings) VALUES (?, ?, ?, ?, ?, ?) ON CONFLICT(provider_id) DO UPDATE SET custom_name = excluded.custom_name, protocol = excluded.protocol, enabled = excluded.enabled, endpoint = excluded.endpoint, settings = excluded.settings",
	)
	.bind(definition.id)
	.bind(update.custom_name.map(|value| value.trim().to_string()))
	.bind(definition.protocol.as_str())
	.bind(update.enabled)
	.bind(endpoint)
	.bind(settings)
	.execute(&state.pool)
	.await?;
    Ok(())
}

#[tracing::instrument(skip(secret))]
pub fn set_api_key(
    provider_id: String,
    secret: Option<String>,
) -> crate::Result<()> {
    set_credential(provider_id, API_KEY_CREDENTIAL.to_string(), secret)
}

#[tracing::instrument(skip(secret))]
pub fn set_credential(
    provider_id: String,
    credential: String,
    secret: Option<String>,
) -> crate::Result<()> {
    validate_credential_name(&provider_id, &credential)?;
    write_credential(&provider_id, &credential, secret.as_deref())?;
    if provider_id == "vertexai" {
        *VERTEX_ACCESS_TOKEN.lock().map_err(|_| {
            ErrorKind::OtherError(
                "Vertex AI token cache is unavailable".to_string(),
            )
        })? = None;
    }
    Ok(())
}

#[tracing::instrument(skip(update))]
pub async fn update_model(update: AiModelUpdate) -> crate::Result<()> {
    provider_definition(&update.provider_id)?;
    let model_id = update.model_id.trim();
    if model_id.is_empty() {
        return Err(input_error("AI model ID cannot be empty".to_string()));
    }
    let display_name = if update.display_name.trim().is_empty() {
        model_id
    } else {
        update.display_name.trim()
    };
    let source = if builtin_models(&update.provider_id)
        .iter()
        .any(|model| model.id == model_id)
    {
        "builtin"
    } else {
        "custom"
    };
    let state = State::get().await?;
    sqlx::query(
		"INSERT INTO ai_provider_models (provider_id, model_id, display_name, enabled, source) VALUES (?, ?, ?, ?, ?) ON CONFLICT(provider_id, model_id) DO UPDATE SET display_name = excluded.display_name, enabled = excluded.enabled, source = CASE WHEN excluded.source = 'builtin' THEN 'builtin' ELSE ai_provider_models.source END",
	)
	.bind(&update.provider_id)
	.bind(model_id)
	.bind(display_name)
	.bind(update.enabled)
	.bind(source)
	.execute(&state.pool)
	.await?;
    Ok(())
}

#[tracing::instrument]
pub async fn remove_model(
    provider_id: String,
    model_id: String,
) -> crate::Result<()> {
    provider_definition(&provider_id)?;
    let state = State::get().await?;
    sqlx::query(
        "DELETE FROM ai_provider_models WHERE provider_id = ? AND model_id = ?",
    )
    .bind(provider_id)
    .bind(model_id)
    .execute(&state.pool)
    .await?;
    Ok(())
}

fn endpoint_with_path(base: &str, path: &str) -> String {
    if base.ends_with(path) {
        base.to_string()
    } else {
        format!("{}{path}", base.trim_end_matches('/'))
    }
}

async fn checked_json(
    response: Response,
    provider: &str,
) -> crate::Result<Value> {
    let status = response.status();
    let text = response.text().await.map_err(|error| {
        ErrorKind::OtherError(format!(
            "Could not read {provider} response: {error}"
        ))
    })?;
    if !status.is_success() {
        let category = if status == StatusCode::TOO_MANY_REQUESTS {
            "AI_RATE_LIMITED"
        } else if matches!(
            status,
            StatusCode::UNAUTHORIZED | StatusCode::FORBIDDEN
        ) {
            "AI_AUTHENTICATION_FAILED"
        } else {
            "AI_PROVIDER_FAILED"
        };
        return Err(ErrorKind::OtherError(format!(
            "{category}: {provider} returned HTTP {status}: {}",
            text.chars().take(500).collect::<String>()
        ))
        .into());
    }
    serde_json::from_str(&text).map_err(|_| {
        ErrorKind::OtherError(format!(
            "AI_PROVIDER_FAILED: {provider} returned invalid JSON"
        ))
        .into()
    })
}

fn github_copilot_uses_responses(model: &str) -> bool {
    let model = model.to_ascii_lowercase();
    if model.starts_with("oswe-")
        || matches!(
            model.as_str(),
            "o1-pro"
                | "o1-pro-2025-03-19"
                | "o3-deep-research"
                | "o3-deep-research-2025-06-26"
                | "o3-pro"
                | "o3-pro-2025-06-10"
                | "o4-mini-deep-research"
                | "o4-mini-deep-research-2025-06-26"
                | "codex-mini-latest"
                | "computer-use-preview"
                | "computer-use-preview-2025-03-11"
                | "gpt-5-mini"
                | "gpt-5-mini-2025-08-07"
        )
    {
        return true;
    }
    if !model.starts_with("gpt-5") || model.contains("-chat") {
        return false;
    }
    if model.contains("-codex") || model.contains("-pro") {
        return true;
    }
    model
        .strip_prefix("gpt-5.")
        .and_then(|value| value.split(['-', ':']).next())
        .and_then(|value| value.parse::<u64>().ok())
        .is_some_and(|minor| minor >= 2)
}

#[derive(Debug, Deserialize)]
struct CopilotAutoSelection {
    session_token: String,
    selected_model: CopilotAutoModel,
}

#[derive(Debug, Deserialize)]
struct CopilotAutoModel {
    id: String,
}

fn copilot_headers(
    builder: reqwest::RequestBuilder,
    session_token: Option<&str>,
) -> reqwest::RequestBuilder {
    let builder = builder
        .header("Copilot-Integration-Id", "vscode-chat")
        .header("Editor-Version", "vscode/1.126.0")
        .header("Editor-Plugin-Version", "copilot-chat/0.54.0")
        .header("User-Agent", "GitHubCopilotChat/0.54.0")
        .header("OpenAI-Intent", "conversation-panel")
        .header("X-GitHub-Api-Version", "2026-08-01")
        .header("X-Initiator", "user");
    match session_token {
        Some(token) => builder.header("Copilot-Session-Token", token),
        None => builder,
    }
}

async fn fresh_oauth_credentials(
    provider_id: &str,
) -> crate::Result<OAuthCredentials> {
    let _refresh_guard = OAUTH_REFRESH_LOCK.lock().await;
    let mut credentials =
        read_oauth_credentials(provider_id)?.ok_or_else(|| {
            input_error(format!(
                "Connect {provider_id} with OAuth before using it"
            ))
        })?;
    if provider_id == "githubcopilot" {
        if credentials
            .bearer_token_expires_at
            .is_none_or(|expiry| expiry <= now_seconds() + 300)
        {
            let definition =
                oauth_definition(provider_id).expect("known OAuth provider");
            let value = checked_json(
                AI_CLIENT
                    .get(
                        definition
                            .token_exchange_endpoint
                            .expect("Copilot exchange endpoint"),
                    )
                    .header("Accept", "application/json")
                    .header(
                        "Authorization",
                        format!("token {}", credentials.access_token),
                    )
                    .send()
                    .await
                    .map_err(|error| ai_network_error(provider_id, error))?,
                "GitHub Copilot token exchange",
            )
            .await?;
            let bearer_token = value
                .get("token")
                .and_then(Value::as_str)
                .ok_or_else(|| {
                    input_error(
                        "GitHub Copilot returned no bearer token".to_string(),
                    )
                })?;
            let bearer_token_expires_at = value
                .get("expires_at")
                .and_then(Value::as_u64)
                .ok_or_else(|| {
                    input_error(
                        "GitHub Copilot returned no token expiry".to_string(),
                    )
                })?;
            credentials.bearer_token = Some(bearer_token.to_string());
            credentials.bearer_token_expires_at = Some(bearer_token_expires_at);
            write_oauth_credentials(provider_id, Some(&credentials))?;
        }
        return Ok(credentials);
    }
    let expires_at = credentials
        .expires_at
        .or_else(|| jwt_expiry(&credentials.access_token));
    if expires_at.is_none_or(|expiry| {
        expiry <= now_seconds() + OAUTH_REFRESH_SKEW_SECONDS
    }) && let Some(refresh_token) = credentials.refresh_token.clone()
    {
        let definition =
            oauth_definition(provider_id).expect("known OAuth provider");
        let value = checked_json(
            AI_CLIENT
                .post(definition.token_endpoint)
                .header("Accept", "application/json")
                .form(&[
                    ("client_id", definition.client_id),
                    ("grant_type", "refresh_token"),
                    ("refresh_token", refresh_token.as_str()),
                ])
                .send()
                .await
                .map_err(|error| ai_network_error(provider_id, error))?,
            "OAuth token refresh",
        )
        .await?;
        let access_token = value
            .get("access_token")
            .and_then(Value::as_str)
            .ok_or_else(|| {
                input_error(
                    "OAuth refresh returned no access token".to_string(),
                )
            })?;
        credentials.expires_at =
            Some(oauth_token_expiry(&value, access_token).unwrap_or_else(
                || now_seconds() + OAUTH_DEFAULT_TOKEN_TTL_SECONDS,
            ));
        credentials.account_id = value
            .get("id_token")
            .and_then(Value::as_str)
            .and_then(jwt_account_id)
            .or_else(|| jwt_account_id(access_token))
            .or(credentials.account_id);
        credentials.access_token = access_token.to_string();
        credentials.refresh_token = value
            .get("refresh_token")
            .and_then(Value::as_str)
            .map(str::to_string)
            .or(credentials.refresh_token);
        write_oauth_credentials(provider_id, Some(&credentials))?;
    }
    Ok(credentials)
}

async fn complete_openai(
    definition: &AiProviderDefinition,
    config: &AiProviderConfig,
    request: &AiTextRequest,
) -> crate::Result<String> {
    let (token, account_id) =
        if definition.auth_type == AiAuthType::OAuthDeviceFlow {
            let credentials = fresh_oauth_credentials(definition.id).await?;
            (
                if definition.id == "githubcopilot" {
                    credentials.bearer_token.unwrap_or_default()
                } else {
                    credentials.access_token
                },
                credentials.account_id,
            )
        } else {
            (
                read_credential(definition.id, API_KEY_CREDENTIAL)?
                    .unwrap_or_default(),
                None,
            )
        };
    if definition.auth_type != AiAuthType::None && token.is_empty() {
        return Err(input_error(format!(
            "Configure credentials for {} before using it",
            definition.name
        )));
    }
    let copilot_auto = if definition.id == "githubcopilot"
        && request.model_id.eq_ignore_ascii_case("auto")
    {
        let value = checked_json(
            copilot_headers(
                text_client(request)
                    .post(endpoint_with_path(&config.endpoint, "/auto"))
                    .bearer_auth(token.clone()),
                None,
            )
            .json(&json!({ "prompt": request.user_prompt }))
            .send()
            .await
            .map_err(|error| ai_network_error(definition.name, error))?,
            definition.name,
        )
        .await?;
        Some(serde_json::from_value::<CopilotAutoSelection>(value).map_err(
            |error| {
                crate::Error::from(ErrorKind::OtherError(format!(
                    "AI_PROVIDER_FAILED: GitHub Copilot /auto returned an invalid selection: {error}"
                )))
            },
        )?)
    } else {
        None
    };
    let model_id = copilot_auto
        .as_ref()
        .map(|selection| selection.selected_model.id.as_str())
        .unwrap_or(request.model_id.as_str());
    let copilot_session_token = copilot_auto
        .as_ref()
        .map(|selection| selection.session_token.as_str());
    if definition.id == "githubcopilot"
        && model_id.to_ascii_lowercase().contains("claude")
    {
        let mut body = json!({
            "model": model_id,
            "max_tokens": 4096,
            "temperature": 0,
            "system": request.system_prompt,
            "messages": [{ "role": "user", "content": request.user_prompt }]
        });
        if request.mode == AiTextMode::Translation {
            insert_json_option(
                &mut body,
                "thinking",
                json!({ "type": "disabled" }),
            );
        }
        let value = checked_json(
            copilot_headers(
                text_client(request)
                    .post(endpoint_with_path(&config.endpoint, "/v1/messages"))
                    .bearer_auth(token),
                copilot_session_token,
            )
            .header("anthropic-version", "2023-06-01")
            .json(&body)
            .send()
            .await
            .map_err(|error| ai_network_error(definition.name, error))?,
            definition.name,
        )
        .await?;
        return value
            .get("content")
            .and_then(Value::as_array)
            .and_then(|content| {
                content
                    .iter()
                    .find_map(|item| item.get("text").and_then(Value::as_str))
            })
            .map(str::to_string)
            .ok_or_else(|| {
                ErrorKind::OtherError(
                    "GitHub Copilot returned no text output".to_string(),
                )
                .into()
            });
    }
    if definition.id == "chatgpt" {
        let account_id = account_id.ok_or_else(|| {
            input_error(
                "ChatGPT OAuth credentials have no account id; reconnect the provider"
                    .to_string(),
            )
        })?;
        let responses_lite = matches!(
            request.model_id.as_str(),
            "gpt-5.6-luna" | "gpt-5.6-sol" | "gpt-5.6-terra"
        );
        let mut body = if responses_lite {
            let mut input = vec![json!({
                "role": "developer",
                "tools": [],
                "type": "additional_tools"
            })];
            if !request.system_prompt.is_empty() {
                input.push(json!({
                    "content": request.system_prompt,
                    "role": "developer",
                    "type": "message"
                }));
            }
            input.push(json!({
                "content": request.user_prompt,
                "role": "user",
                "type": "message"
            }));
            json!({
                "model": request.model_id,
                "input": input,
                "parallel_tool_calls": false,
                "reasoning": { "context": "all_turns" },
                "store": false,
                "stream": true,
                "tool_choice": "auto"
            })
        } else {
            json!({
                "model": request.model_id,
                "instructions": request.system_prompt,
                "input": request.user_prompt,
                "include": ["reasoning.encrypted_content"],
                "store": false,
                "stream": true
            })
        };
        apply_responses_translation_options(&mut body, request);
        let mut builder = text_client(request)
            .post(endpoint_with_path(&config.endpoint, "/responses"))
            .bearer_auth(token)
            .header("Accept", "text/event-stream")
            .header("ChatGPT-Account-Id", account_id)
            .header("originator", "lobehub")
            .header("session-id", Uuid::new_v4().to_string())
            .header("version", env!("CARGO_PKG_VERSION"))
            .json(&body);
        if responses_lite {
            builder = builder
                .header("x-openai-internal-codex-responses-lite", "true");
        }
        return checked_openai_response_stream(
            builder
                .send()
                .await
                .map_err(|error| ai_network_error(definition.name, error))?,
            definition.name,
        )
        .await;
    }
    if definition.id == "githubcopilot"
        && github_copilot_uses_responses(model_id)
    {
        let mut body = json!({
            "model": model_id,
            "input": [
                { "role": "system", "content": request.system_prompt },
                { "role": "user", "content": request.user_prompt }
            ],
            "store": false,
            "stream": true
        });
        apply_responses_translation_options(&mut body, request);
        let response = copilot_headers(
            text_client(request)
                .post(endpoint_with_path(&config.endpoint, "/responses"))
                .bearer_auth(token),
            copilot_session_token,
        )
        .header("Accept", "text/event-stream")
        .json(&body)
        .send()
        .await
        .map_err(|error| ai_network_error(definition.name, error))?;
        return checked_openai_response_stream(response, definition.name).await;
    }
    let endpoint = match config.protocol {
        AiProtocol::Azure => {
            let deployment = config
                .settings
                .get("deployment")
                .map(String::as_str)
                .filter(|value| !value.is_empty())
                .unwrap_or(model_id);
            let version = config
                .settings
                .get("api_version")
                .map(String::as_str)
                .unwrap_or("2024-10-21");
            format!(
                "{}/openai/deployments/{}/chat/completions?api-version={}",
                config.endpoint.trim_end_matches('/'),
                urlencoding::encode(deployment),
                urlencoding::encode(version)
            )
        }
        _ => endpoint_with_path(&config.endpoint, "/chat/completions"),
    };
    let mut body = json!({
        "model": model_id,
        "temperature": 0,
        "messages": [
            { "role": "system", "content": request.system_prompt },
            { "role": "user", "content": request.user_prompt }
        ]
    });
    apply_openai_translation_options(&mut body, request);
    apply_openai_response_format(&mut body, request);
    let mut builder = opencode_headers(
        text_client(request).post(endpoint).json(&body),
        definition.id,
        Uuid::new_v4(),
    );
    if !token.is_empty() {
        builder = if config.protocol == AiProtocol::Azure
            || (config.protocol == AiProtocol::AzureAi
                && definition.id != "github")
        {
            builder.header("api-key", token)
        } else {
            builder.bearer_auth(token)
        };
    }
    if definition.id == "githubcopilot" {
        builder = copilot_headers(builder, copilot_session_token);
    }
    let value = checked_json(
        builder
            .send()
            .await
            .map_err(|error| ai_network_error(definition.name, error))?,
        definition.name,
    )
    .await?;
    openai_content(&value).ok_or_else(|| {
        ErrorKind::OtherError(format!(
            "{} returned no text output",
            definition.name
        ))
        .into()
    })
}

async fn complete_anthropic(
    definition: &AiProviderDefinition,
    config: &AiProviderConfig,
    request: &AiTextRequest,
) -> crate::Result<String> {
    let api_key = read_credential(definition.id, API_KEY_CREDENTIAL)?
        .ok_or_else(|| {
            input_error(format!(
                "Configure credentials for {} before using it",
                definition.name
            ))
        })?;
    let mut body = json!({
        "model": request.model_id,
        "max_tokens": 4096,
        "temperature": 0,
        "system": request.system_prompt,
        "messages": [{ "role": "user", "content": request.user_prompt }]
    });
    if request.mode == AiTextMode::Translation {
        insert_json_option(
            &mut body,
            "thinking",
            json!({ "type": "disabled" }),
        );
    }
    let value = checked_json(
        opencode_headers(
            text_client(request)
                .post(endpoint_with_path(&config.endpoint, "/v1/messages")),
            definition.id,
            Uuid::new_v4(),
        )
        .header("x-api-key", api_key)
        .header("anthropic-version", "2023-06-01")
        .json(&body)
        .send()
        .await
        .map_err(|error| ai_network_error(definition.name, error))?,
        definition.name,
    )
    .await?;
    value
        .get("content")
        .and_then(Value::as_array)
        .and_then(|content| {
            content
                .iter()
                .find_map(|item| item.get("text").and_then(Value::as_str))
        })
        .map(str::to_string)
        .ok_or_else(|| {
            ErrorKind::OtherError(format!(
                "{} returned no text output",
                definition.name
            ))
            .into()
        })
}

fn bytes_to_hex(bytes: impl AsRef<[u8]>) -> String {
    bytes
        .as_ref()
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

fn hmac_sha256(key: &[u8], value: &str) -> Vec<u8> {
    let mut mac = Hmac::<Sha256>::new_from_slice(key)
        .expect("HMAC-SHA256 accepts keys of any size");
    mac.update(value.as_bytes());
    mac.finalize().into_bytes().to_vec()
}

fn bedrock_body(request: &AiTextRequest) -> Value {
    if request.model_id.starts_with("meta.") {
        return json!({
            "prompt": format!(
                "<s>[INST] <<SYS>>\n{}\n<</SYS>>\n\n{} [/INST]",
                request.system_prompt,
                request.user_prompt
            ),
            "max_gen_len": 4096,
            "temperature": 0
        });
    }
    if request.model_id.contains("claude-v2")
        || request.model_id.contains("claude-instant")
    {
        return json!({
            "prompt": format!(
                "\n\nHuman: {}\n\n{}\n\nAssistant:",
                request.system_prompt,
                request.user_prompt
            ),
            "max_tokens_to_sample": 4096,
            "temperature": 0
        });
    }
    json!({
        "anthropic_version": "bedrock-2023-05-31",
        "max_tokens": 4096,
        "temperature": 0,
        "system": request.system_prompt,
        "messages": [{ "role": "user", "content": request.user_prompt }]
    })
}

fn bedrock_content(value: &Value) -> Option<String> {
    value
        .get("content")
        .and_then(Value::as_array)
        .and_then(|content| {
            content
                .iter()
                .find_map(|item| item.get("text").and_then(Value::as_str))
        })
        .or_else(|| value.get("completion").and_then(Value::as_str))
        .or_else(|| value.get("generation").and_then(Value::as_str))
        .map(str::to_string)
}

fn sign_bedrock_request(
    builder: reqwest::RequestBuilder,
    endpoint: &str,
    region: &str,
    body: &str,
    access_key_id: &str,
    secret_access_key: &str,
    session_token: Option<&str>,
) -> crate::Result<reqwest::RequestBuilder> {
    let url = Url::parse(endpoint).map_err(|_| {
        input_error("Bedrock endpoint must be a valid URL".to_string())
    })?;
    let mut host = url
        .host_str()
        .ok_or_else(|| input_error("Bedrock endpoint has no host".to_string()))?
        .to_string();
    if let Some(port) = url.port() {
        host.push(':');
        host.push_str(&port.to_string());
    }
    let now = chrono::Utc::now();
    let timestamp = now.format("%Y%m%dT%H%M%SZ").to_string();
    let date = now.format("%Y%m%d").to_string();
    let payload_hash = bytes_to_hex(Sha256::digest(body.as_bytes()));
    let mut canonical_headers = format!(
        "content-type:application/json\nhost:{host}\nx-amz-content-sha256:{payload_hash}\nx-amz-date:{timestamp}\n"
    );
    let mut signed_headers =
        "content-type;host;x-amz-content-sha256;x-amz-date".to_string();
    if let Some(token) = session_token {
        canonical_headers.push_str("x-amz-security-token:");
        canonical_headers.push_str(token);
        canonical_headers.push('\n');
        signed_headers.push_str(";x-amz-security-token");
    }
    let canonical_request = format!(
        "POST\n{}\n{}\n{canonical_headers}\n{signed_headers}\n{payload_hash}",
        url.path(),
        url.query().unwrap_or_default()
    );
    let scope = format!("{date}/{region}/bedrock/aws4_request");
    let string_to_sign = format!(
        "AWS4-HMAC-SHA256\n{timestamp}\n{scope}\n{}",
        bytes_to_hex(Sha256::digest(canonical_request.as_bytes()))
    );
    let date_key =
        hmac_sha256(format!("AWS4{secret_access_key}").as_bytes(), &date);
    let region_key = hmac_sha256(&date_key, region);
    let service_key = hmac_sha256(&region_key, "bedrock");
    let signing_key = hmac_sha256(&service_key, "aws4_request");
    let signature = bytes_to_hex(hmac_sha256(&signing_key, &string_to_sign));
    let authorization = format!(
        "AWS4-HMAC-SHA256 Credential={access_key_id}/{scope}, SignedHeaders={signed_headers}, Signature={signature}"
    );
    let builder = builder
        .header("Host", host)
        .header("x-amz-date", timestamp)
        .header("x-amz-content-sha256", payload_hash)
        .header("Authorization", authorization);
    Ok(if let Some(token) = session_token {
        builder.header("x-amz-security-token", token)
    } else {
        builder
    })
}

async fn complete_bedrock(
    definition: &AiProviderDefinition,
    config: &AiProviderConfig,
    request: &AiTextRequest,
) -> crate::Result<String> {
    let region = config
        .settings
        .get("region")
        .map(String::as_str)
        .filter(|value| !value.trim().is_empty())
        .unwrap_or("us-east-1");
    let base = if config.endpoint.is_empty() {
        format!("https://bedrock-runtime.{region}.amazonaws.com")
    } else {
        config.endpoint.clone()
    };
    let endpoint = format!(
        "{}/model/{}/invoke",
        base.trim_end_matches('/'),
        urlencoding::encode(&request.model_id)
    );
    let body = bedrock_body(request).to_string();
    let mut builder = text_client(request)
        .post(&endpoint)
        .header("Accept", "application/json")
        .header("Content-Type", "application/json")
        .body(body.clone());
    let api_key = read_credential(definition.id, API_KEY_CREDENTIAL)?;
    let use_api_key = match config.settings.get("auth_mode").map(String::as_str)
    {
        Some("api-key") => true,
        Some("aws-credentials") => false,
        _ => api_key.is_some(),
    };
    if use_api_key {
        builder = builder.bearer_auth(api_key.ok_or_else(|| {
            input_error("Configure the Bedrock API key".to_string())
        })?);
    } else {
        let access_key_id =
            read_credential(definition.id, AWS_ACCESS_KEY_ID_CREDENTIAL)?
                .ok_or_else(|| {
                    input_error(
                "Configure a Bedrock API key or AWS credentials before using it"
                    .to_string(),
            )
                })?;
        let secret_access_key =
            read_credential(definition.id, AWS_SECRET_ACCESS_KEY_CREDENTIAL)?
                .ok_or_else(|| {
                input_error(
                    "Configure the Bedrock AWS secret access key".to_string(),
                )
            })?;
        let session_token =
            read_credential(definition.id, AWS_SESSION_TOKEN_CREDENTIAL)?;
        builder = sign_bedrock_request(
            builder,
            &endpoint,
            region,
            &body,
            &access_key_id,
            &secret_access_key,
            session_token.as_deref(),
        )?;
    }
    let value = checked_json(
        builder
            .send()
            .await
            .map_err(|error| ai_network_error(definition.name, error))?,
        definition.name,
    )
    .await?;
    bedrock_content(&value).ok_or_else(|| {
        ErrorKind::OtherError("Bedrock returned no text output".to_string())
            .into()
    })
}

async fn vertex_access_token(
    service_account_json: &str,
) -> crate::Result<(VertexServiceAccount, String)> {
    let account = parse_vertex_service_account(service_account_json)?;
    let fingerprint =
        bytes_to_hex(Sha256::digest(service_account_json.as_bytes()));
    if let Some(cached) = VERTEX_ACCESS_TOKEN
        .lock()
        .map_err(|_| {
            ErrorKind::OtherError(
                "Vertex AI token cache is unavailable".to_string(),
            )
        })?
        .as_ref()
        .filter(|cached| {
            cached.credential_fingerprint == fingerprint
                && cached.expires_at > now_seconds() + 120
        })
        .cloned()
    {
        return Ok((account, cached.access_token));
    }
    let issued_at = now_seconds();
    let claims = vertex_jwt_claims(&account, issued_at);
    let assertion = jsonwebtoken::encode(
        &Header::new(Algorithm::RS256),
        &claims,
        &EncodingKey::from_rsa_pem(account.private_key.as_bytes()).map_err(
            |_| input_error("Vertex AI private key is invalid".to_string()),
        )?,
    )
    .map_err(|error| {
        ErrorKind::OtherError(format!(
            "Could not sign the Vertex AI access token request: {error}"
        ))
    })?;
    let value = checked_json(
        AI_CLIENT
            .post(&account.token_uri)
            .form(&[
                ("grant_type", "urn:ietf:params:oauth:grant-type:jwt-bearer"),
                ("assertion", assertion.as_str()),
            ])
            .send()
            .await
            .map_err(|error| ai_network_error("Vertex AI OAuth", error))?,
        "Vertex AI OAuth",
    )
    .await?;
    let access_token = value
        .get("access_token")
        .and_then(Value::as_str)
        .ok_or_else(|| {
            input_error("Vertex AI returned no access token".to_string())
        })?
        .to_string();
    let expires_at = issued_at
        + value
            .get("expires_in")
            .and_then(Value::as_u64)
            .unwrap_or(3600);
    *VERTEX_ACCESS_TOKEN.lock().map_err(|_| {
        ErrorKind::OtherError(
            "Vertex AI token cache is unavailable".to_string(),
        )
    })? = Some(CachedVertexAccessToken {
        credential_fingerprint: fingerprint,
        access_token: access_token.clone(),
        expires_at,
    });
    Ok((account, access_token))
}

fn google_content(value: &Value, provider: &str) -> crate::Result<String> {
    value
        .get("candidates")
        .and_then(Value::as_array)
        .and_then(|items| items.first())
        .and_then(|item| item.get("content"))
        .and_then(|content| content.get("parts"))
        .and_then(Value::as_array)
        .and_then(|parts| {
            parts
                .iter()
                .find_map(|part| part.get("text").and_then(Value::as_str))
        })
        .map(str::to_string)
        .ok_or_else(|| {
            ErrorKind::OtherError(format!("{provider} returned no text output"))
                .into()
        })
}

async fn complete_vertex(
    definition: &AiProviderDefinition,
    config: &AiProviderConfig,
    request: &AiTextRequest,
) -> crate::Result<String> {
    let service_account =
        read_credential(definition.id, VERTEX_SERVICE_ACCOUNT_CREDENTIAL)?
            .ok_or_else(|| {
                input_error(
            "Configure Vertex AI service-account credentials before using it"
                .to_string(),
        )
            })?;
    let (account, access_token) = vertex_access_token(&service_account).await?;
    let project = config
        .settings
        .get("project")
        .map(String::as_str)
        .filter(|value| !value.trim().is_empty())
        .unwrap_or(&account.project_id);
    let location = config
        .settings
        .get("location")
        .or_else(|| config.settings.get("region"))
        .map(String::as_str)
        .filter(|value| !value.trim().is_empty())
        .unwrap_or("global");
    let base = if config.endpoint.is_empty() {
        if location == "global" {
            "https://aiplatform.googleapis.com".to_string()
        } else {
            format!("https://{location}-aiplatform.googleapis.com")
        }
    } else {
        config.endpoint.clone()
    };
    let endpoint = format!(
        "{}/v1/projects/{}/locations/{}/publishers/google/models/{}:generateContent",
        base.trim_end_matches('/'),
        urlencoding::encode(project),
        urlencoding::encode(location),
        urlencoding::encode(&request.model_id)
    );
    let body = json!({
        "systemInstruction": { "parts": [{ "text": request.system_prompt }] },
        "contents": [{ "role": "user", "parts": [{ "text": request.user_prompt }] }],
        "generationConfig": google_generation_config(request)
    });
    let value = checked_json(
        text_client(request)
            .post(endpoint)
            .bearer_auth(access_token)
            .json(&body)
            .send()
            .await
            .map_err(|error| ai_network_error(definition.name, error))?,
        definition.name,
    )
    .await?;
    google_content(&value, definition.name)
}

async fn complete_google(
    definition: &AiProviderDefinition,
    config: &AiProviderConfig,
    request: &AiTextRequest,
) -> crate::Result<String> {
    let api_key = read_credential(definition.id, API_KEY_CREDENTIAL)?
        .ok_or_else(|| {
            input_error(format!(
                "Configure credentials for {} before using it",
                definition.name
            ))
        })?;
    let endpoint = format!(
        "{}/v1beta/models/{}:generateContent?key={}",
        config.endpoint.trim_end_matches('/'),
        urlencoding::encode(&request.model_id),
        urlencoding::encode(&api_key)
    );
    let body = json!({
        "systemInstruction": { "parts": [{ "text": request.system_prompt }] },
        "contents": [{ "role": "user", "parts": [{ "text": request.user_prompt }] }],
        "generationConfig": google_generation_config(request)
    });
    let value = checked_json(
        text_client(request)
            .post(endpoint)
            .json(&body)
            .send()
            .await
            .map_err(|error| ai_network_error(definition.name, error))?,
        definition.name,
    )
    .await?;
    google_content(&value, definition.name)
}

async fn complete_ollama(
    definition: &AiProviderDefinition,
    config: &AiProviderConfig,
    request: &AiTextRequest,
) -> crate::Result<String> {
    let mut body = json!({
        "model": request.model_id,
        "stream": false,
        "messages": [
            { "role": "system", "content": request.system_prompt },
            { "role": "user", "content": request.user_prompt }
        ]
    });
    if request.mode == AiTextMode::Translation {
        insert_json_option(&mut body, "think", json!(false));
    }
    if request.response_format == AiTextResponseFormat::JsonObject {
        insert_json_option(&mut body, "format", json!("json"));
    }
    let value = checked_json(
        text_client(request)
            .post(endpoint_with_path(&config.endpoint, "/api/chat"))
            .json(&body)
            .send()
            .await
            .map_err(|error| ai_network_error(definition.name, error))?,
        definition.name,
    )
    .await?;
    value
        .get("message")
        .and_then(|message| message.get("content"))
        .and_then(Value::as_str)
        .map(str::to_string)
        .ok_or_else(|| {
            ErrorKind::OtherError("Ollama returned no text output".to_string())
                .into()
        })
}

async fn complete_cloudflare(
    definition: &AiProviderDefinition,
    config: &AiProviderConfig,
    request: &AiTextRequest,
) -> crate::Result<String> {
    let token = read_credential(definition.id, API_KEY_CREDENTIAL)?
        .ok_or_else(|| {
            input_error(
                "Configure a Cloudflare API token before using it".to_string(),
            )
        })?;
    let account_id = config
        .settings
        .get("account_id")
        .filter(|value| !value.is_empty())
        .ok_or_else(|| {
            input_error(
                "Configure the Cloudflare account ID before using it"
                    .to_string(),
            )
        })?;
    let endpoint = format!(
        "{}/accounts/{}/ai/run/{}",
        config.endpoint.trim_end_matches('/'),
        urlencoding::encode(account_id),
        request.model_id
    );
    let body = json!({
        "messages": [
            { "role": "system", "content": request.system_prompt },
            { "role": "user", "content": request.user_prompt }
        ]
    });
    let value = checked_json(
        text_client(request)
            .post(endpoint)
            .bearer_auth(token)
            .json(&body)
            .send()
            .await
            .map_err(|error| ai_network_error(definition.name, error))?,
        definition.name,
    )
    .await?;
    value
        .get("result")
        .and_then(|result| result.get("response"))
        .and_then(Value::as_str)
        .map(str::to_string)
        .ok_or_else(|| {
            ErrorKind::OtherError(
                "Cloudflare returned no text output".to_string(),
            )
            .into()
        })
}

#[tracing::instrument(skip(request), fields(provider_id = %request.provider_id, model_id = %request.model_id))]
pub async fn complete_text(request: AiTextRequest) -> crate::Result<String> {
    let state = State::get().await?;
    let enabled = sqlx::query_scalar::<_, i64>(
        "SELECT enabled FROM ai_settings WHERE id = 0",
    )
    .fetch_one(&state.pool)
    .await?
        != 0;
    if !enabled {
        return Err(input_error("AI features are disabled".to_string()));
    }
    let definition = provider_definition(&request.provider_id)?;
    let config = load_provider_config(definition).await?;
    if !config.enabled {
        return Err(input_error(format!("{} is disabled", definition.name)));
    }
    if request.model_id.trim().is_empty() {
        return Err(input_error(
            "Select an AI model before using it".to_string(),
        ));
    }
    match config.protocol {
        AiProtocol::Anthropic => {
            complete_anthropic(definition, &config, &request).await
        }
        AiProtocol::Google if definition.id == "vertexai" => {
            complete_vertex(definition, &config, &request).await
        }
        AiProtocol::Google => {
            complete_google(definition, &config, &request).await
        }
        AiProtocol::Bedrock => {
            complete_bedrock(definition, &config, &request).await
        }
        AiProtocol::Ollama => {
            complete_ollama(definition, &config, &request).await
        }
        AiProtocol::Cloudflare => {
            complete_cloudflare(definition, &config, &request).await
        }
        AiProtocol::Openai
        | AiProtocol::Azure
        | AiProtocol::AzureAi
        | AiProtocol::Huggingface
        | AiProtocol::Router => {
            complete_openai(definition, &config, &request).await
        }
    }
}

#[tracing::instrument]
pub async fn test_provider(
    provider_id: String,
    model_id: String,
) -> crate::Result<String> {
    complete_text(AiTextRequest {
        provider_id,
        model_id,
        system_prompt: "Reply with a short plain-text greeting.".to_string(),
        user_prompt: "Hello from MC-Trin".to_string(),
        mode: AiTextMode::Default,
        response_format: AiTextResponseFormat::Text,
    })
    .await
}

fn is_text_model(model: &Value, id: &str, protocol: AiProtocol) -> bool {
    if protocol == AiProtocol::Google
        && let Some(methods) = model
            .get("supportedGenerationMethods")
            .and_then(Value::as_array)
        && !methods
            .iter()
            .filter_map(Value::as_str)
            .any(|method| method == "generateContent")
    {
        return false;
    }
    if let Some(model_type) = model.get("type").and_then(Value::as_str) {
        let model_type = model_type.to_ascii_lowercase();
        if matches!(
            model_type.as_str(),
            "embedding"
                | "image"
                | "tts"
                | "asr"
                | "audio"
                | "video"
                | "realtime"
                | "text2music"
        ) {
            return false;
        }
    }
    if let Some(task) = model
        .pointer("/task/name")
        .or_else(|| model.get("task"))
        .and_then(Value::as_str)
    {
        let task = task.to_ascii_lowercase();
        if !task.contains("text generation")
            && !task.contains("text-generation")
            && !task.contains("chat")
        {
            return false;
        }
    }
    let id = id.to_ascii_lowercase();
    ![
        "embedding",
        "embed-",
        "-embed",
        "/embed",
        "rerank",
        "dall-e",
        "gpt-image",
        "imagen",
        "image-generation",
        "stable-diffusion",
        "recraft",
        "flux-",
        "sdxl",
        "whisper",
        "transcri",
        "speech",
        "tts",
        "audio",
        "realtime",
        "moderation",
        "video",
        "sora",
        "veo-",
    ]
    .iter()
    .any(|marker| id.contains(marker))
}

fn discovered_models(
    value: &Value,
    protocol: AiProtocol,
) -> Vec<(String, String)> {
    let values = match protocol {
        AiProtocol::Google => value.get("models"),
        AiProtocol::Ollama => value.get("models"),
        AiProtocol::Cloudflare => value.get("result"),
        _ => value.get("data"),
    }
    .and_then(Value::as_array)
    .cloned()
    .unwrap_or_default();
    let mut models = values
        .iter()
        .filter_map(|model| {
            let id = if protocol == AiProtocol::Ollama {
                model.get("name").and_then(Value::as_str)
            } else {
                model
                    .get("id")
                    .or_else(|| model.get("name"))
                    .and_then(Value::as_str)
            }?;
            let id = id.strip_prefix("models/").unwrap_or(id);
            if !is_text_model(model, id, protocol) {
                return None;
            }
            let name = model
                .get("display_name")
                .or_else(|| model.get("displayName"))
                .or_else(|| model.get("name"))
                .and_then(Value::as_str)
                .map(|name| name.strip_prefix("models/").unwrap_or(name))
                .filter(|name| !name.is_empty())
                .unwrap_or(id);
            Some((id.to_string(), name.to_string()))
        })
        .collect::<Vec<_>>();
    models.sort_by(|left, right| left.0.cmp(&right.0));
    models.dedup_by(|left, right| left.0 == right.0);
    models
}

#[tracing::instrument]
pub async fn fetch_models(
    provider_id: String,
) -> crate::Result<Vec<AiProviderModel>> {
    let definition = provider_definition(&provider_id)?;
    if !definition.show_model_fetcher {
        return Err(input_error(format!(
            "{} does not expose model discovery",
            definition.name
        )));
    }
    let config = load_provider_config(definition).await?;
    let key = if definition.auth_type == AiAuthType::OAuthDeviceFlow {
        fresh_oauth_credentials(definition.id).await?.access_token
    } else {
        read_credential(definition.id, API_KEY_CREDENTIAL)?.unwrap_or_default()
    };
    let (endpoint, mut builder) = match config.protocol {
        AiProtocol::Google => {
            let endpoint = format!(
                "{}/v1beta/models?key={}",
                config.endpoint,
                urlencoding::encode(&key)
            );
            (endpoint.clone(), AI_CLIENT.get(endpoint))
        }
        AiProtocol::Ollama => {
            let endpoint = endpoint_with_path(&config.endpoint, "/api/tags");
            (endpoint.clone(), AI_CLIENT.get(endpoint))
        }
        AiProtocol::Anthropic => {
            let endpoint = endpoint_with_path(&config.endpoint, "/v1/models");
            let builder = AI_CLIENT
                .get(&endpoint)
                .header("x-api-key", &key)
                .header("anthropic-version", "2023-06-01");
            (endpoint, builder)
        }
        AiProtocol::Cloudflare => {
            let account_id = config
                .settings
                .get("account_id")
                .map(String::as_str)
                .filter(|value| !value.trim().is_empty())
                .ok_or_else(|| {
                    input_error(
                        "Configure the Cloudflare account ID before fetching models"
                            .to_string(),
                    )
                })?;
            let endpoint = format!(
                "{}/accounts/{}/ai/models/search",
                config.endpoint.trim_end_matches('/'),
                urlencoding::encode(account_id)
            );
            (endpoint.clone(), AI_CLIENT.get(endpoint))
        }
        _ => {
            let endpoint = endpoint_with_path(&config.endpoint, "/models");
            (endpoint.clone(), AI_CLIENT.get(endpoint))
        }
    };
    if !key.is_empty()
        && !matches!(
            config.protocol,
            AiProtocol::Google | AiProtocol::Anthropic
        )
    {
        builder = if config.protocol == AiProtocol::Azure
            || (config.protocol == AiProtocol::AzureAi
                && definition.id != "github")
        {
            builder.header("api-key", key)
        } else {
            builder.bearer_auth(key)
        };
    }
    let value = checked_json(
        builder
            .send()
            .await
            .map_err(|error| ai_network_error(definition.name, error))?,
        definition.name,
    )
    .await?;
    let models = discovered_models(&value, config.protocol);
    if models.is_empty() {
        return Err(ErrorKind::OtherError(format!(
            "{} returned no text models from {endpoint}",
            definition.name
        ))
        .into());
    }
    let state = State::get().await?;
    for (id, name) in &models {
        sqlx::query(
			"INSERT INTO ai_provider_models (provider_id, model_id, display_name, enabled, source) VALUES (?, ?, ?, TRUE, 'remote') ON CONFLICT(provider_id, model_id) DO UPDATE SET display_name = CASE WHEN ai_provider_models.source = 'custom' THEN ai_provider_models.display_name ELSE excluded.display_name END",
		)
		.bind(&provider_id)
		.bind(id)
		.bind(name)
		.execute(&state.pool)
		.await?;
    }
    load_provider_models(&provider_id).await
}

#[tracing::instrument]
pub async fn begin_oauth(
    provider_id: String,
) -> crate::Result<OAuthDeviceCode> {
    let definition = oauth_definition(&provider_id).ok_or_else(|| {
        input_error(format!(
            "{provider_id} does not support OAuth device authorization"
        ))
    })?;
    let value = if provider_id == "chatgpt" {
        checked_json(
            AI_CLIENT
                .post(definition.device_code_endpoint)
                .json(&json!({ "client_id": definition.client_id }))
                .send()
                .await
                .map_err(|error| ai_network_error("ChatGPT OAuth", error))?,
            "ChatGPT OAuth",
        )
        .await?
    } else {
        checked_json(
            AI_CLIENT
                .post(definition.device_code_endpoint)
                .header("Accept", "application/json")
                .form(&[
                    ("client_id", definition.client_id),
                    ("scope", definition.scope),
                ])
                .send()
                .await
                .map_err(|error| {
                    ai_network_error("OAuth device authorization", error)
                })?,
            "OAuth device authorization",
        )
        .await?
    };
    let (
        device_code,
        user_code,
        verification_uri,
        complete,
        expires_in,
        interval,
    ) = if provider_id == "chatgpt" {
        let device_auth_id = value
            .get("device_auth_id")
            .and_then(Value::as_str)
            .ok_or_else(|| {
                input_error(
                    "ChatGPT returned no device authorization ID".to_string(),
                )
            })?;
        let user_code =
            value.get("user_code").and_then(Value::as_str).ok_or_else(
                || input_error("ChatGPT returned no user code".to_string()),
            )?;
        let provider_interval = json_u64(&value, "interval")
            .map(|interval| {
                interval.max(1) + CHATGPT_POLLING_SAFETY_MARGIN_SECONDS
            })
            .unwrap_or(definition.interval);
        (
            json!({ "device_auth_id": device_auth_id, "user_code": user_code })
                .to_string(),
            user_code.to_string(),
            "https://auth.openai.com/codex/device".to_string(),
            None,
            CHATGPT_DEVICE_CODE_TTL_SECONDS,
            provider_interval,
        )
    } else {
        (
            value
                .get("device_code")
                .and_then(Value::as_str)
                .ok_or_else(|| {
                    input_error(
                        "OAuth provider returned no device code".to_string(),
                    )
                })?
                .to_string(),
            value
                .get("user_code")
                .and_then(Value::as_str)
                .ok_or_else(|| {
                    input_error(
                        "OAuth provider returned no user code".to_string(),
                    )
                })?
                .to_string(),
            value
                .get("verification_uri")
                .or_else(|| value.get("verification_url"))
                .and_then(Value::as_str)
                .ok_or_else(|| {
                    input_error(
                        "OAuth provider returned no verification URL"
                            .to_string(),
                    )
                })?
                .to_string(),
            value
                .get("verification_uri_complete")
                .and_then(Value::as_str)
                .map(str::to_string),
            json_u64(&value, "expires_in").unwrap_or(900),
            json_u64(&value, "interval").unwrap_or(definition.interval),
        )
    };
    let flow_id = Uuid::new_v4();
    OAUTH_FLOWS
        .lock()
        .map_err(|_| {
            ErrorKind::OtherError("OAuth flow state is unavailable".to_string())
        })?
        .insert(
            flow_id,
            PendingOAuthFlow {
                provider_id,
                device_code,
                expires_at: now_seconds() + expires_in,
                credentials: None,
            },
        );
    Ok(OAuthDeviceCode {
        flow_id,
        user_code,
        verification_uri,
        verification_uri_complete: complete,
        expires_in,
        interval,
    })
}

fn oauth_error_status(value: &Value) -> Option<OAuthPollStatus> {
    match value.get("error").and_then(Value::as_str) {
        Some("authorization_pending") => Some(OAuthPollStatus::Pending),
        Some("slow_down") => Some(OAuthPollStatus::SlowDown),
        Some("expired_token") => Some(OAuthPollStatus::Expired),
        Some("access_denied") => Some(OAuthPollStatus::Denied),
        _ => None,
    }
}

#[tracing::instrument]
pub async fn poll_oauth(flow_id: Uuid) -> crate::Result<OAuthPollStatus> {
    poll_oauth_with_storage(flow_id, write_oauth_credentials).await
}

async fn poll_oauth_with_storage(
    flow_id: Uuid,
    persist: impl Fn(&str, Option<&OAuthCredentials>) -> crate::Result<()>,
) -> crate::Result<OAuthPollStatus> {
    // A device authorization code can only be exchanged once.
    let _guard = OAUTH_POLL_LOCK.lock().await;
    let flow = OAUTH_FLOWS
        .lock()
        .map_err(|_| {
            ErrorKind::OtherError("OAuth flow state is unavailable".to_string())
        })?
        .get(&flow_id)
        .cloned()
        .ok_or_else(|| {
            input_error(
                "OAuth flow was not found or already finished".to_string(),
            )
        })?;
    if flow.expires_at <= now_seconds() {
        OAUTH_FLOWS
            .lock()
            .ok()
            .and_then(|mut flows| flows.remove(&flow_id));
        return Ok(OAuthPollStatus::Expired);
    }
    if let Some(credentials) = &flow.credentials {
        return complete_oauth(
            flow_id,
            &flow.provider_id,
            credentials,
            persist,
        );
    }
    let definition =
        oauth_definition(&flow.provider_id).expect("stored OAuth provider");
    let value = if flow.provider_id == "chatgpt" {
        let state: Value =
            serde_json::from_str(&flow.device_code).map_err(|_| {
                input_error("Invalid ChatGPT OAuth state".to_string())
            })?;
        let response = AI_CLIENT
            .post(
                definition
                    .token_exchange_endpoint
                    .expect("ChatGPT polling endpoint"),
            )
            .json(&json!({
                "device_auth_id": state.get("device_auth_id"),
                "user_code": state.get("user_code")
            }))
            .send()
            .await
            .map_err(|error| ai_network_error("ChatGPT OAuth", error))?;
        if matches!(
            response.status(),
            StatusCode::FORBIDDEN | StatusCode::NOT_FOUND
        ) {
            return Ok(OAuthPollStatus::Pending);
        }
        let authorization = checked_json(response, "ChatGPT OAuth").await?;
        let authorization_code = authorization
            .get("authorization_code")
            .and_then(Value::as_str)
            .ok_or_else(|| {
                input_error(
                    "ChatGPT returned no authorization code".to_string(),
                )
            })?;
        let code_verifier = authorization
            .get("code_verifier")
            .and_then(Value::as_str)
            .ok_or_else(|| {
                input_error("ChatGPT returned no PKCE verifier".to_string())
            })?;
        let token_response = AI_CLIENT
            .post(definition.token_endpoint)
            .header("Accept", "application/json")
            .form(&[
                ("client_id", definition.client_id),
                ("code", authorization_code),
                ("code_verifier", code_verifier),
                ("grant_type", "authorization_code"),
                (
                    "redirect_uri",
                    "https://auth.openai.com/deviceauth/callback",
                ),
            ])
            .send()
            .await
            .map_err(|error| {
                ai_network_error("ChatGPT OAuth token exchange", error)
            })?;
        checked_json(token_response, "ChatGPT OAuth token exchange").await?
    } else {
        let response = AI_CLIENT
            .post(definition.token_endpoint)
            .header("Accept", "application/json")
            .form(&[
                ("client_id", definition.client_id),
                ("device_code", flow.device_code.as_str()),
                ("grant_type", "urn:ietf:params:oauth:grant-type:device_code"),
            ])
            .send()
            .await
            .map_err(|error| ai_network_error("OAuth token exchange", error))?;
        let status = response.status();
        let value: Value = response
            .json()
            .await
            .map_err(|error| ErrorKind::OtherError(error.to_string()))?;
        if let Some(status) = oauth_error_status(&value) {
            return Ok(status);
        }
        if !status.is_success() {
            return Err(ErrorKind::OtherError(format!(
                "OAuth token endpoint returned HTTP {status}"
            ))
            .into());
        }
        value
    };
    if let Some(status) = oauth_error_status(&value) {
        return Ok(status);
    }
    let access_token = value
        .get("access_token")
        .and_then(Value::as_str)
        .ok_or_else(|| {
            input_error("OAuth provider returned no access token".to_string())
        })?
        .to_string();
    let account_id = value
        .get("id_token")
        .and_then(Value::as_str)
        .and_then(jwt_account_id)
        .or_else(|| jwt_account_id(&access_token));
    if flow.provider_id == "chatgpt" && account_id.is_none() {
        return Err(input_error(
            "ChatGPT token response is missing an account id".to_string(),
        ));
    }
    let mut credentials = OAuthCredentials {
        account_id,
        refresh_token: value
            .get("refresh_token")
            .and_then(Value::as_str)
            .map(str::to_string),
        expires_at: oauth_token_expiry(&value, &access_token),
        access_token,
        ..OAuthCredentials::default()
    };
    if flow.provider_id == "githubcopilot" {
        let exchange = checked_json(
            AI_CLIENT
                .get(
                    definition
                        .token_exchange_endpoint
                        .expect("Copilot exchange endpoint"),
                )
                .header("Accept", "application/json")
                .header(
                    "Authorization",
                    format!("token {}", credentials.access_token),
                )
                .send()
                .await
                .map_err(|error| {
                    ai_network_error("GitHub Copilot token exchange", error)
                })?,
            "GitHub Copilot token exchange",
        )
        .await?;
        let bearer_token = exchange
            .get("token")
            .and_then(Value::as_str)
            .ok_or_else(|| {
                input_error(
                    "GitHub Copilot returned no bearer token".to_string(),
                )
            })?;
        let bearer_token_expires_at = exchange
            .get("expires_at")
            .and_then(Value::as_u64)
            .ok_or_else(|| {
                input_error(
                    "GitHub Copilot returned no token expiry".to_string(),
                )
            })?;
        credentials.bearer_token = Some(bearer_token.to_string());
        credentials.bearer_token_expires_at = Some(bearer_token_expires_at);
    }
    // Retain exchanged tokens if persistence fails, so retrying the same flow
    // does not attempt to redeem the already consumed authorization code.
    OAUTH_FLOWS
        .lock()
        .map_err(|_| {
            ErrorKind::OtherError("OAuth flow state is unavailable".to_string())
        })?
        .get_mut(&flow_id)
        .expect("active OAuth flow")
        .credentials = Some(credentials.clone());
    complete_oauth(flow_id, &flow.provider_id, &credentials, persist)
}

fn complete_oauth(
    flow_id: Uuid,
    provider_id: &str,
    credentials: &OAuthCredentials,
    persist: impl FnOnce(&str, Option<&OAuthCredentials>) -> crate::Result<()>,
) -> crate::Result<OAuthPollStatus> {
    persist(provider_id, Some(credentials))?;
    OAUTH_FLOWS
        .lock()
        .ok()
        .and_then(|mut flows| flows.remove(&flow_id));
    Ok(OAuthPollStatus::Success)
}

#[tracing::instrument]
pub fn disconnect_oauth(provider_id: String) -> crate::Result<()> {
    oauth_definition(&provider_id).ok_or_else(|| {
        input_error(format!(
            "{provider_id} does not support OAuth device authorization"
        ))
    })?;
    write_oauth_credentials(&provider_id, None)
}

#[cfg(test)]
mod tests;
