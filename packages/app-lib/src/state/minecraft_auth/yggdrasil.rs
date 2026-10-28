use super::{
    Credentials, MinecraftAccountType, MinecraftCharacterExpressionState,
    MinecraftProfile, MinecraftSkin, MinecraftSkinVariant,
};
use crate::ErrorKind;
use crate::util::fetch::authentication_client;
use base64::Engine;
use base64::prelude::{BASE64_STANDARD, BASE64_STANDARD_NO_PAD};
use chrono::{Duration, Utc};
use serde::{Deserialize, Serialize};
use serde_json::json;
use sqlx::Sqlite;
use std::collections::HashMap;
use std::sync::{Arc, LazyLock};
use std::time::{Duration as StdDuration, Instant};
use tokio::sync::Mutex;
use url::Url;
use uuid::Uuid;

const PENDING_LOGIN_LIFETIME: StdDuration = StdDuration::from_secs(600);

static PENDING_LOGINS: LazyLock<Mutex<HashMap<Uuid, PendingYggdrasilLogin>>> =
    LazyLock::new(|| Mutex::new(HashMap::new()));

#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct YggdrasilAccount {
    pub api_root: String,
    pub server_name: String,
    pub login: String,
    #[serde(skip_serializing)]
    pub client_token: String,
}

#[derive(Serialize, Debug)]
#[serde(tag = "status", rename_all = "snake_case")]
pub enum YggdrasilLoginResult {
    Complete {
        credentials: Box<Credentials>,
    },
    SelectProfile {
        flow_id: Uuid,
        profiles: Vec<YggdrasilProfile>,
    },
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq)]
pub struct YggdrasilProfile {
    #[serde(with = "simple_uuid")]
    pub id: Uuid,
    pub name: String,
}

mod simple_uuid {
    use serde::{Deserialize, Deserializer, Serializer};
    use uuid::Uuid;

    pub fn serialize<S: Serializer>(
        value: &Uuid,
        serializer: S,
    ) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(&value.simple().to_string())
    }

    pub fn deserialize<'de, D: Deserializer<'de>>(
        deserializer: D,
    ) -> Result<Uuid, D::Error> {
        let value = String::deserialize(deserializer)?;
        Uuid::parse_str(&value).map_err(serde::de::Error::custom)
    }
}

#[derive(Debug, Clone)]
pub struct YggdrasilMetadata {
    pub api_root: String,
    pub server_name: String,
    pub raw: String,
}

#[derive(Deserialize)]
struct MetadataDocument {
    #[serde(default)]
    meta: Metadata,
}

#[derive(Deserialize, Default)]
struct Metadata {
    #[serde(rename = "serverName")]
    server_name: Option<String>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct AuthenticateResponse {
    access_token: String,
    client_token: String,
    #[serde(default)]
    available_profiles: Vec<YggdrasilProfile>,
    selected_profile: Option<YggdrasilProfile>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct RefreshResponse {
    access_token: String,
    client_token: String,
    selected_profile: Option<YggdrasilProfile>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct ErrorResponse {
    error: Option<String>,
    error_message: Option<String>,
    cause: Option<String>,
}

#[derive(Deserialize)]
struct SessionProfile {
    #[serde(with = "simple_uuid")]
    id: Uuid,
    name: String,
    #[serde(default)]
    properties: Vec<SessionProperty>,
}

#[derive(Deserialize)]
struct SessionProperty {
    name: String,
    value: String,
}

#[derive(Deserialize)]
struct TexturePayload {
    #[serde(default)]
    textures: SessionTextures,
}

#[derive(Deserialize, Default)]
struct SessionTextures {
    #[serde(rename = "SKIN")]
    skin: Option<SessionTexture>,
}

#[derive(Deserialize)]
struct SessionTexture {
    url: String,
    #[serde(default)]
    metadata: SessionTextureMetadata,
}

#[derive(Deserialize, Default)]
struct SessionTextureMetadata {
    model: Option<String>,
}

struct PendingYggdrasilLogin {
    created: Instant,
    api_root: String,
    server_name: String,
    login: String,
    access_token: String,
    client_token: String,
    profiles: Vec<YggdrasilProfile>,
}

pub async fn begin_yggdrasil_login(
    api_root: &str,
    login: &str,
    password: &str,
    exec: impl sqlx::Executor<'_, Database = Sqlite> + Copy,
) -> crate::Result<YggdrasilLoginResult> {
    let login = login.trim();
    if login.is_empty() {
        return Err(ErrorKind::InputError(
            "The Yggdrasil account name cannot be empty".to_string(),
        )
        .as_error());
    }
    if password.is_empty() {
        return Err(ErrorKind::InputError(
            "The Yggdrasil account password cannot be empty".to_string(),
        )
        .as_error());
    }

    let metadata = fetch_yggdrasil_metadata(api_root).await?;
    let client_token = Uuid::new_v4().simple().to_string();
    let response = post_json::<AuthenticateResponse>(
        &metadata.api_root,
        "authenticate",
        json!({
            "agent": { "name": "Minecraft", "version": 1 },
            "username": login,
            "password": password,
            "clientToken": client_token,
            "requestUser": true,
        }),
    )
    .await?;

    if let Some(profile) = response.selected_profile {
        let credentials = create_credentials(
            profile,
            response.access_token,
            response.client_token,
            metadata,
            login,
        );
        credentials.upsert(exec).await?;
        return Ok(YggdrasilLoginResult::Complete {
            credentials: Box::new(credentials),
        });
    }

    if response.available_profiles.is_empty() {
        return Err(ErrorKind::OtherError(
            "The Yggdrasil account does not have a Minecraft profile"
                .to_string(),
        )
        .as_error());
    }

    if response.available_profiles.len() == 1 {
        let profile = response.available_profiles[0].clone();
        let refreshed = refresh_selected_profile(
            &metadata.api_root,
            &response.access_token,
            &response.client_token,
            &profile,
        )
        .await?;
        let selected_profile = refreshed.selected_profile.ok_or_else(|| {
            ErrorKind::OtherError(
                "The Yggdrasil service did not select the requested profile"
                    .to_string(),
            )
            .as_error()
        })?;
        let credentials = create_credentials(
            selected_profile,
            refreshed.access_token,
            refreshed.client_token,
            metadata,
            login,
        );
        credentials.upsert(exec).await?;
        return Ok(YggdrasilLoginResult::Complete {
            credentials: Box::new(credentials),
        });
    }

    let flow_id = Uuid::new_v4();
    let profiles = response.available_profiles.clone();
    let mut pending = PENDING_LOGINS.lock().await;
    pending.retain(|_, login| login.created.elapsed() < PENDING_LOGIN_LIFETIME);
    pending.insert(
        flow_id,
        PendingYggdrasilLogin {
            created: Instant::now(),
            api_root: metadata.api_root,
            server_name: metadata.server_name,
            login: login.to_string(),
            access_token: response.access_token,
            client_token: response.client_token,
            profiles: response.available_profiles,
        },
    );

    Ok(YggdrasilLoginResult::SelectProfile { flow_id, profiles })
}

pub async fn finish_yggdrasil_login(
    flow_id: Uuid,
    profile_id: Uuid,
    exec: impl sqlx::Executor<'_, Database = Sqlite> + Copy,
) -> crate::Result<Credentials> {
    let login =
        PENDING_LOGINS
            .lock()
            .await
            .remove(&flow_id)
            .ok_or_else(|| {
                ErrorKind::InputError(
                    "The Yggdrasil profile selection has expired".to_string(),
                )
                .as_error()
            })?;

    if login.created.elapsed() >= PENDING_LOGIN_LIFETIME {
        return Err(ErrorKind::InputError(
            "The Yggdrasil profile selection has expired".to_string(),
        )
        .as_error());
    }

    let profile = login
        .profiles
        .into_iter()
        .find(|profile| profile.id == profile_id)
        .ok_or_else(|| {
            ErrorKind::InputError(
                "The selected Yggdrasil profile is not available".to_string(),
            )
            .as_error()
        })?;
    let refreshed = refresh_selected_profile(
        &login.api_root,
        &login.access_token,
        &login.client_token,
        &profile,
    )
    .await?;
    let selected_profile = refreshed.selected_profile.ok_or_else(|| {
        ErrorKind::OtherError(
            "The Yggdrasil service did not select the requested profile"
                .to_string(),
        )
        .as_error()
    })?;
    let credentials = create_credentials(
        selected_profile,
        refreshed.access_token,
        refreshed.client_token,
        YggdrasilMetadata {
            api_root: login.api_root,
            server_name: login.server_name,
            raw: String::new(),
        },
        &login.login,
    );
    credentials.upsert(exec).await?;
    Ok(credentials)
}

pub async fn refresh_yggdrasil_credentials(
    credentials: &mut Credentials,
    exec: impl sqlx::Executor<'_, Database = Sqlite> + Copy,
) -> crate::Result<()> {
    if credentials.expires > Utc::now() {
        return Ok(());
    }

    let account = credentials.yggdrasil.clone().ok_or_else(|| {
        ErrorKind::OtherError(
            "Yggdrasil credentials are missing provider information"
                .to_string(),
        )
        .as_error()
    })?;
    let validate_url = endpoint(&account.api_root, "validate")?;
    let response = authentication_client()
        .post(validate_url)
        .json(&json!({
            "accessToken": credentials.access_token,
            "clientToken": account.client_token,
        }))
        .send()
        .await?;

    if response.status().is_success() {
        credentials.expires = Utc::now() + Duration::minutes(5);
        credentials.upsert(exec).await?;
        return Ok(());
    }
    if response.status() == reqwest::StatusCode::TOO_MANY_REQUESTS {
        tracing::warn!(
            "Yggdrasil token validation for {} was rate limited",
            account.server_name
        );
        credentials.expires = Utc::now() + Duration::minutes(1);
        credentials.upsert(exec).await?;
        return Ok(());
    }
    if response.status().as_u16() != 403 && response.status().as_u16() != 401 {
        return Err(response_error(response).await);
    }

    let refreshed = refresh_selected_profile(
        &account.api_root,
        &credentials.access_token,
        &account.client_token,
        &YggdrasilProfile {
            id: credentials.offline_profile.id,
            name: credentials.offline_profile.name.clone(),
        },
    )
    .await?;
    let profile = refreshed.selected_profile.ok_or_else(|| {
        ErrorKind::OtherError(
            "The Yggdrasil service rejected the selected profile".to_string(),
        )
        .as_error()
    })?;

    credentials.access_token = refreshed.access_token;
    credentials.offline_profile.id = profile.id;
    credentials.offline_profile.name = profile.name;
    if let Some(account) = credentials.yggdrasil.as_mut() {
        account.client_token = refreshed.client_token;
    }
    credentials.expires = Utc::now() + Duration::minutes(5);
    credentials.upsert(exec).await?;
    Ok(())
}

pub async fn fetch_yggdrasil_profile(
    account: &YggdrasilAccount,
    profile_id: Uuid,
) -> crate::Result<Option<MinecraftProfile>> {
    let mut url = Url::parse(&format!(
        "{}/sessionserver/session/minecraft/profile/{}",
        account.api_root,
        profile_id.simple()
    ))?;
    url.query_pairs_mut().append_pair("unsigned", "false");
    let response = authentication_client().get(url).send().await?;
    if response.status() == reqwest::StatusCode::NO_CONTENT {
        return Ok(None);
    }
    if !response.status().is_success() {
        return Err(response_error(response).await);
    }

    let raw = response.text().await?;
    let profile: SessionProfile =
        serde_json::from_str(&raw).map_err(|error| {
            ErrorKind::OtherError(format!(
                "The Yggdrasil session profile is not valid JSON: {error}"
            ))
            .as_error()
        })?;
    let skin = profile
        .properties
        .iter()
        .find(|property| property.name == "textures")
        .and_then(|property| match decode_session_skin(profile.id, &property.value) {
            Ok(skin) => skin,
            Err(error) => {
                tracing::warn!(
                    "Unable to decode the Yggdrasil skin for profile {}: {error}",
                    profile.id
                );
                None
            }
        });

    Ok(Some(MinecraftProfile {
        id: profile.id,
        name: profile.name,
        skins: skin.into_iter().collect(),
        capes: Vec::new(),
        fetch_time: Some(Instant::now()),
    }))
}

fn decode_session_skin(
    profile_id: Uuid,
    encoded_textures: &str,
) -> Result<Option<MinecraftSkin>, String> {
    let decoded = BASE64_STANDARD
        .decode(encoded_textures)
        .or_else(|_| BASE64_STANDARD_NO_PAD.decode(encoded_textures))
        .map_err(|error| {
            format!("invalid Base64 textures property: {error}")
        })?;
    let payload: TexturePayload = serde_json::from_slice(&decoded)
        .map_err(|error| format!("invalid textures JSON: {error}"))?;
    let Some(skin) = payload.textures.skin else {
        return Ok(None);
    };
    let url = Url::parse(&skin.url)
        .map_err(|error| format!("invalid skin texture URL: {error}"))?;
    if !matches!(url.scheme(), "http" | "https") {
        return Err("skin texture URL must use HTTP or HTTPS".to_string());
    }
    let variant = if skin.metadata.model.as_deref() == Some("slim") {
        MinecraftSkinVariant::Slim
    } else {
        MinecraftSkinVariant::Classic
    };

    Ok(Some(MinecraftSkin {
        id: profile_id,
        state: MinecraftCharacterExpressionState::Active,
        url: Arc::new(url),
        texture_key: None,
        variant,
        name: None,
    }))
}

pub async fn fetch_yggdrasil_metadata(
    api_root: &str,
) -> crate::Result<YggdrasilMetadata> {
    let api_root = normalize_api_root(api_root)?;
    let response = authentication_client().get(&api_root).send().await?;
    if !response.status().is_success() {
        return Err(response_error(response).await);
    }
    let raw = response.text().await?;
    let document: MetadataDocument =
        serde_json::from_str(&raw).map_err(|error| {
            ErrorKind::InputError(format!(
                "The Yggdrasil service metadata is not valid JSON: {error}"
            ))
            .as_error()
        })?;
    let fallback_name = Url::parse(&api_root)
        .ok()
        .and_then(|url| url.host_str().map(str::to_string))
        .unwrap_or_else(|| "Yggdrasil".to_string());
    let server_name = document
        .meta
        .server_name
        .filter(|name| !name.trim().is_empty())
        .unwrap_or(fallback_name);

    Ok(YggdrasilMetadata {
        api_root,
        server_name,
        raw,
    })
}

pub fn normalize_api_root(api_root: &str) -> crate::Result<String> {
    let mut url = Url::parse(api_root.trim()).map_err(|error| {
        ErrorKind::InputError(format!("Invalid Yggdrasil API URL: {error}"))
            .as_error()
    })?;
    if !matches!(url.scheme(), "http" | "https") || url.host_str().is_none() {
        return Err(ErrorKind::InputError(
            "The Yggdrasil API URL must be an HTTP or HTTPS URL".to_string(),
        )
        .as_error());
    }
    if url.username() != "" || url.password().is_some() {
        return Err(ErrorKind::InputError(
            "The Yggdrasil API URL cannot contain credentials".to_string(),
        )
        .as_error());
    }
    url.set_query(None);
    url.set_fragment(None);
    let path = url.path().trim_end_matches('/').to_string();
    url.set_path(&path);
    Ok(url.to_string().trim_end_matches('/').to_string())
}

fn endpoint(api_root: &str, action: &str) -> crate::Result<Url> {
    Url::parse(&format!("{api_root}/authserver/{action}")).map_err(|error| {
        ErrorKind::InputError(format!("Invalid Yggdrasil endpoint: {error}"))
            .as_error()
    })
}

async fn refresh_selected_profile(
    api_root: &str,
    access_token: &str,
    client_token: &str,
    profile: &YggdrasilProfile,
) -> crate::Result<RefreshResponse> {
    post_json(
        api_root,
        "refresh",
        json!({
            "accessToken": access_token,
            "clientToken": client_token,
            "selectedProfile": profile,
            "requestUser": true,
        }),
    )
    .await
}

async fn post_json<T: for<'de> Deserialize<'de>>(
    api_root: &str,
    action: &str,
    body: serde_json::Value,
) -> crate::Result<T> {
    let response = authentication_client()
        .post(endpoint(api_root, action)?)
        .header(reqwest::header::ACCEPT_LANGUAGE, "zh-CN")
        .json(&body)
        .send()
        .await?;
    if !response.status().is_success() {
        return Err(response_error(response).await);
    }
    Ok(response.json().await?)
}

async fn response_error(response: reqwest::Response) -> crate::Error {
    let status = response.status();
    let body = response.text().await.unwrap_or_default();
    let message = serde_json::from_str::<ErrorResponse>(&body)
        .ok()
        .and_then(|error| {
            error
                .error_message
                .or(error.cause)
                .or(error.error)
                .filter(|message| !message.trim().is_empty())
        })
        .unwrap_or_else(|| {
            if body.trim().is_empty() {
                format!("The Yggdrasil service returned HTTP {status}")
            } else {
                format!("The Yggdrasil service returned HTTP {status}: {body}")
            }
        });
    ErrorKind::OtherError(message).as_error()
}

fn create_credentials(
    profile: YggdrasilProfile,
    access_token: String,
    client_token: String,
    metadata: YggdrasilMetadata,
    login: &str,
) -> Credentials {
    let account_id = format!(
        "yggdrasil:{}:{}",
        metadata.api_root,
        profile.id.as_hyphenated()
    );
    Credentials {
        account_id: Some(account_id),
        offline_profile: MinecraftProfile {
            id: profile.id,
            name: profile.name,
            ..MinecraftProfile::default()
        },
        account_type: MinecraftAccountType::Yggdrasil,
        access_token,
        refresh_token: String::new(),
        expires: Utc::now() + Duration::minutes(5),
        active: true,
        yggdrasil: Some(YggdrasilAccount {
            api_root: metadata.api_root,
            server_name: metadata.server_name,
            login: login.to_string(),
            client_token,
        }),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalizes_api_roots() {
        assert_eq!(
            normalize_api_root(" https://littleskin.cn/api/yggdrasil/ ")
                .unwrap(),
            "https://littleskin.cn/api/yggdrasil"
        );
        assert!(normalize_api_root("file:///tmp/yggdrasil").is_err());
        assert!(
            normalize_api_root("https://user:pass@example.com/api").is_err()
        );
    }

    #[test]
    fn serializes_profile_ids_without_hyphens() {
        let profile = YggdrasilProfile {
            id: Uuid::parse_str("01234567-89ab-cdef-0123-456789abcdef")
                .unwrap(),
            name: "Player".to_string(),
        };
        let serialized = serde_json::to_value(&profile).unwrap();
        assert_eq!(serialized["id"], "0123456789abcdef0123456789abcdef");
        assert_eq!(
            serde_json::from_value::<YggdrasilProfile>(serialized)
                .unwrap()
                .id,
            profile.id
        );
    }

    #[test]
    fn decodes_classic_and_slim_session_skins() {
        let profile_id = Uuid::new_v4();
        let classic = BASE64_STANDARD.encode(
            br#"{"textures":{"SKIN":{"url":"https://textures.example/skin.png"}}}"#,
        );
        let slim = BASE64_STANDARD.encode(
            br#"{"textures":{"SKIN":{"url":"https://textures.example/slim.png","metadata":{"model":"slim"}}}}"#,
        );

        let classic =
            decode_session_skin(profile_id, &classic).unwrap().unwrap();
        let slim = decode_session_skin(profile_id, &slim).unwrap().unwrap();
        assert_eq!(classic.variant, MinecraftSkinVariant::Classic);
        assert_eq!(slim.variant, MinecraftSkinVariant::Slim);
        assert_eq!(classic.id, profile_id);
        assert_eq!(classic.url.as_str(), "https://textures.example/skin.png");
    }

    #[test]
    fn safely_handles_missing_or_invalid_session_skins() {
        let profile_id = Uuid::new_v4();
        let missing = BASE64_STANDARD.encode(br#"{"textures":{}}"#);
        let invalid_url = BASE64_STANDARD
            .encode(br#"{"textures":{"SKIN":{"url":"file:///tmp/skin.png"}}}"#);

        assert!(decode_session_skin(profile_id, &missing).unwrap().is_none());
        assert!(decode_session_skin(profile_id, "not-base64").is_err());
        assert!(decode_session_skin(profile_id, &invalid_url).is_err());
    }

    #[tokio::test]
    async fn persists_yggdrasil_account_metadata() {
        let pool = sqlx::sqlite::SqlitePoolOptions::new()
            .max_connections(1)
            .connect("sqlite::memory:")
            .await
            .unwrap();
        sqlx::migrate!().run(&pool).await.unwrap();
        let profile_id = Uuid::new_v4();
        let credentials = create_credentials(
            YggdrasilProfile {
                id: profile_id,
                name: "Player".to_string(),
            },
            "access-token".to_string(),
            "client-token".to_string(),
            YggdrasilMetadata {
                api_root: "https://littleskin.cn/api/yggdrasil".to_string(),
                server_name: "LittleSkin".to_string(),
                raw: "{}".to_string(),
            },
            "player@example.com",
        );
        credentials.upsert(&pool).await.unwrap();

        let stored = Credentials::get_active_without_refresh(&pool)
            .await
            .unwrap()
            .unwrap();
        assert!(stored.is_yggdrasil());
        assert_eq!(stored.offline_profile.id, profile_id);
        let account = stored.yggdrasil.unwrap();
        assert_eq!(account.server_name, "LittleSkin");
        assert_eq!(account.login, "player@example.com");
        assert_eq!(account.client_token, "client-token");
    }

    /// Minimal Yggdrasil service that answers from a routing table on loopback.
    struct MockYggdrasil {
        base: String,
        requests: std::sync::Arc<std::sync::Mutex<Vec<(String, String)>>>,
    }

    impl MockYggdrasil {
        async fn start(routes: Vec<(String, u16, serde_json::Value)>) -> Self {
            let listener =
                tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
            let address = listener.local_addr().unwrap();
            let requests =
                std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
            let recorded = requests.clone();
            let routes = std::sync::Arc::new(routes);
            tokio::spawn(async move {
                loop {
                    let Ok((mut stream, _)) = listener.accept().await else {
                        return;
                    };
                    let recorded = recorded.clone();
                    let routes = routes.clone();
                    tokio::spawn(async move {
                        let Some((path, body)) =
                            read_http_request(&mut stream).await
                        else {
                            return;
                        };
                        recorded.lock().unwrap().push((path.clone(), body));
                        let (status, payload) = routes
                            .iter()
                            .find(|(route, _, _)| *route == path)
                            .map(|(_, status, body)| (*status, body.clone()))
                            .unwrap_or((404, json!({"error": "not found"})));
                        write_http_response(&mut stream, status, &payload)
                            .await;
                    });
                }
            });
            Self {
                base: format!("http://{address}"),
                requests,
            }
        }

        fn body(&self, path: &str) -> Option<serde_json::Value> {
            self.requests
                .lock()
                .unwrap()
                .iter()
                .find(|(recorded, _)| recorded == path)
                .and_then(|(_, body)| serde_json::from_str(body).ok())
        }
    }

    async fn read_http_request(
        stream: &mut tokio::net::TcpStream,
    ) -> Option<(String, String)> {
        use tokio::io::AsyncReadExt;
        let mut buffer = Vec::new();
        let mut chunk = [0u8; 4096];
        let head_end = loop {
            let count = stream.read(&mut chunk).await.ok()?;
            if count == 0 {
                return None;
            }
            buffer.extend_from_slice(&chunk[..count]);
            if let Some(index) =
                buffer.windows(4).position(|window| window == b"\r\n\r\n")
            {
                break index + 4;
            }
            if buffer.len() > 65536 {
                return None;
            }
        };
        let head = String::from_utf8_lossy(&buffer[..head_end]).to_string();
        let path = head
            .split_whitespace()
            .nth(1)?
            .split('?')
            .next()?
            .to_string();
        let content_length = head
            .lines()
            .find_map(|line| {
                let (name, value) = line.split_once(':')?;
                name.eq_ignore_ascii_case("content-length")
                    .then(|| value.trim().parse::<usize>().ok())?
            })
            .unwrap_or(0);
        let mut body = buffer[head_end..].to_vec();
        while body.len() < content_length {
            let count = stream.read(&mut chunk).await.ok()?;
            if count == 0 {
                break;
            }
            body.extend_from_slice(&chunk[..count]);
        }
        Some((path, String::from_utf8_lossy(&body).to_string()))
    }

    async fn write_http_response(
        stream: &mut tokio::net::TcpStream,
        status: u16,
        payload: &serde_json::Value,
    ) {
        use tokio::io::AsyncWriteExt;
        let payload = serde_json::to_vec(payload).unwrap();
        let head = format!(
            "HTTP/1.1 {status} Mock\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
            payload.len()
        );
        let _ = stream.write_all(head.as_bytes()).await;
        let _ = stream.write_all(&payload).await;
        let _ = stream.flush().await;
    }

    async fn auth_test_pool() -> sqlx::SqlitePool {
        let pool = sqlx::sqlite::SqlitePoolOptions::new()
            .max_connections(1)
            .connect("sqlite::memory:")
            .await
            .unwrap();
        sqlx::migrate!().run(&pool).await.unwrap();
        pool
    }

    fn account_id(profile: &YggdrasilProfile) -> String {
        profile.id.simple().to_string()
    }

    fn profile(name: &str) -> YggdrasilProfile {
        YggdrasilProfile {
            id: Uuid::new_v4(),
            name: name.to_string(),
        }
    }

    fn metadata_root(server: &MockYggdrasil) -> YggdrasilMetadata {
        YggdrasilMetadata {
            api_root: server.base.clone(),
            server_name: "Mock Yggdrasil".to_string(),
            raw: "{}".to_string(),
        }
    }

    #[tokio::test]
    async fn yggdrasil_login_stores_the_selected_profile_and_server_name() {
        let player = profile("MockPlayer");
        let server = MockYggdrasil::start(vec![
            (
                "/".to_string(),
                200,
                json!({"meta": {"serverName": "Mock Yggdrasil"}}),
            ),
            (
                "/authserver/authenticate".to_string(),
                200,
                json!({
                    "accessToken": "access-1",
                    "clientToken": "client-1",
                    "selectedProfile": {"id": account_id(&player), "name": player.name},
                    "availableProfiles": [{"id": account_id(&player), "name": player.name}],
                }),
            ),
        ])
        .await;
        let pool = auth_test_pool().await;

        let result = begin_yggdrasil_login(
            &server.base,
            "player@example.com",
            "secret",
            &pool,
        )
        .await
        .unwrap();
        let YggdrasilLoginResult::Complete { credentials } = result else {
            panic!("a selected profile should complete the login");
        };
        assert_eq!(credentials.access_token, "access-1");
        assert_eq!(credentials.offline_profile.id, player.id);
        assert_eq!(credentials.offline_profile.name, "MockPlayer");
        let account = credentials.yggdrasil.clone().unwrap();
        assert_eq!(account.server_name, "Mock Yggdrasil");
        assert_eq!(account.api_root, server.base);

        let stored = Credentials::get_active_without_refresh(&pool)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(stored.access_token, "access-1");
        assert_eq!(stored.offline_profile.id, player.id);

        let request = server.body("/authserver/authenticate").unwrap();
        assert_eq!(request["username"], "player@example.com");
        assert_eq!(request["agent"]["name"], "Minecraft");
        assert_eq!(request["password"], "secret");
    }

    #[tokio::test]
    async fn yggdrasil_login_surfaces_service_errors_without_storing_credentials()
     {
        let server = MockYggdrasil::start(vec![
            ("/".to_string(), 200, json!({"meta": {"serverName": "Mock"}})),
            (
                "/authserver/authenticate".to_string(),
                403,
                json!({
                    "error": "ForbiddenOperationException",
                    "errorMessage": "Invalid credentials. Invalid username or password.",
                }),
            ),
        ])
        .await;
        let pool = auth_test_pool().await;

        let error = begin_yggdrasil_login(
            &server.base,
            "player@example.com",
            "hunter2",
            &pool,
        )
        .await
        .expect_err("invalid credentials must fail the login");
        let message = format!("{error:#}");
        assert!(
            message.contains("Invalid credentials"),
            "unexpected error: {message}"
        );
        assert!(
            !message.contains("hunter2"),
            "the failure must not echo the password"
        );
        assert!(
            Credentials::get_all_without_refresh(&pool)
                .await
                .unwrap()
                .is_empty()
        );
    }

    #[tokio::test]
    async fn selecting_a_profile_completes_the_login_with_a_refreshed_token() {
        let first = profile("First");
        let second = profile("Second");
        let server = MockYggdrasil::start(vec![
            ("/".to_string(), 200, json!({"meta": {"serverName": "Mock"}})),
            (
                "/authserver/authenticate".to_string(),
                200,
                json!({
                    "accessToken": "access-before",
                    "clientToken": "client-before",
                    "availableProfiles": [
                        {"id": account_id(&first), "name": first.name},
                        {"id": account_id(&second), "name": second.name},
                    ],
                }),
            ),
            (
                "/authserver/refresh".to_string(),
                200,
                json!({
                    "accessToken": "access-selected",
                    "clientToken": "client-selected",
                    "selectedProfile": {"id": account_id(&second), "name": second.name},
                }),
            ),
        ])
        .await;
        let pool = auth_test_pool().await;

        let result = begin_yggdrasil_login(
            &server.base,
            "player@example.com",
            "secret",
            &pool,
        )
        .await
        .unwrap();
        let YggdrasilLoginResult::SelectProfile { flow_id, profiles } = result
        else {
            panic!("two profiles should require a selection");
        };
        assert_eq!(profiles.len(), 2);

        let credentials = finish_yggdrasil_login(flow_id, second.id, &pool)
            .await
            .unwrap();
        assert_eq!(credentials.offline_profile.id, second.id);
        assert_eq!(credentials.offline_profile.name, "Second");
        assert_eq!(credentials.access_token, "access-selected");
        let stored = Credentials::get_active_without_refresh(&pool)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(stored.access_token, "access-selected");

        let request = server.body("/authserver/refresh").unwrap();
        assert_eq!(request["accessToken"], "access-before");
        assert_eq!(request["selectedProfile"]["id"], account_id(&second));
    }

    #[tokio::test]
    async fn expired_yggdrasil_tokens_refresh_and_persist_the_new_identity() {
        let original = profile("OldName");
        let renamed = profile("NewName");
        let server = MockYggdrasil::start(vec![
            (
                "/authserver/validate".to_string(),
                403,
                json!({"error": "ForbiddenOperationException"}),
            ),
            (
                "/authserver/refresh".to_string(),
                200,
                json!({
                    "accessToken": "access-2",
                    "clientToken": "client-2",
                    "selectedProfile": {"id": account_id(&renamed), "name": renamed.name},
                }),
            ),
        ])
        .await;
        let pool = auth_test_pool().await;
        let mut credentials = create_credentials(
            original.clone(),
            "access-1".to_string(),
            "client-1".to_string(),
            metadata_root(&server),
            "player@example.com",
        );
        credentials.expires = Utc::now() - Duration::minutes(1);
        credentials.upsert(&pool).await.unwrap();

        refresh_yggdrasil_credentials(&mut credentials, &pool)
            .await
            .unwrap();

        assert_eq!(credentials.access_token, "access-2");
        assert_eq!(credentials.offline_profile.id, renamed.id);
        assert_eq!(credentials.offline_profile.name, "NewName");
        assert!(credentials.expires > Utc::now());
        assert_eq!(
            credentials.yggdrasil.clone().unwrap().client_token,
            "client-2"
        );
        let stored = Credentials::get_active_without_refresh(&pool)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(stored.access_token, "access-2");
        assert_eq!(stored.offline_profile.id, renamed.id);

        let request = server.body("/authserver/refresh").unwrap();
        assert_eq!(request["accessToken"], "access-1");
        assert_eq!(request["selectedProfile"]["id"], account_id(&original));
    }

    #[tokio::test]
    async fn rejected_yggdrasil_refresh_keeps_the_stored_credentials() {
        let player = profile("Player");
        let server = MockYggdrasil::start(vec![
            (
                "/authserver/validate".to_string(),
                403,
                json!({"error": "ForbiddenOperationException"}),
            ),
            (
                "/authserver/refresh".to_string(),
                403,
                json!({
                    "error": "ForbiddenOperationException",
                    "errorMessage": "Invalid token.",
                }),
            ),
        ])
        .await;
        let pool = auth_test_pool().await;
        let mut credentials = create_credentials(
            player.clone(),
            "access-1".to_string(),
            "client-1".to_string(),
            metadata_root(&server),
            "player@example.com",
        );
        credentials.expires = Utc::now() - Duration::minutes(1);
        credentials.upsert(&pool).await.unwrap();

        let error = refresh_yggdrasil_credentials(&mut credentials, &pool)
            .await
            .expect_err("a rejected refresh must fail");
        assert!(format!("{error:#}").contains("Invalid token"));

        assert_eq!(credentials.access_token, "access-1");
        let stored = Credentials::get_active_without_refresh(&pool)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(stored.access_token, "access-1");
        assert_eq!(stored.offline_profile.id, player.id);
        assert!(server.body("/authserver/refresh").is_some());
    }

    #[tokio::test]
    async fn rate_limited_yggdrasil_validation_keeps_the_token_without_refreshing()
     {
        let player = profile("Player");
        let server = MockYggdrasil::start(vec![(
            "/authserver/validate".to_string(),
            429,
            json!({"error": "TooManyRequests"}),
        )])
        .await;
        let pool = auth_test_pool().await;
        let mut credentials = create_credentials(
            player.clone(),
            "access-1".to_string(),
            "client-1".to_string(),
            metadata_root(&server),
            "player@example.com",
        );
        credentials.expires = Utc::now() - Duration::minutes(1);
        credentials.upsert(&pool).await.unwrap();

        refresh_yggdrasil_credentials(&mut credentials, &pool)
            .await
            .unwrap();

        assert_eq!(credentials.access_token, "access-1");
        assert!(credentials.expires > Utc::now());
        assert!(
            server.body("/authserver/refresh").is_none(),
            "a rate limited validation must not force a refresh"
        );
    }
}
