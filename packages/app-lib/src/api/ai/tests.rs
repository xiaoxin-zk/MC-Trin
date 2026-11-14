use super::*;

/// A migrated in-memory database with the AI tables the loader reads.
///
/// `get_state_with_pool` touches `ai_settings`, `ai_provider_configs`,
/// `ai_provider_models` and `translation_settings`, so the real migrations are
/// used rather than hand-written DDL that could drift from production schema.
async fn ai_test_pool() -> sqlx::SqlitePool {
    let pool = sqlx::sqlite::SqlitePoolOptions::new()
        .max_connections(1)
        .connect("sqlite::memory:")
        .await
        .unwrap();
    sqlx::migrate!().run(&pool).await.unwrap();
    pool
}

fn ai_test_request(provider_id: &str, model_id: &str) -> AiTextRequest {
    AiTextRequest {
        provider_id: provider_id.to_string(),
        model_id: model_id.to_string(),
        system_prompt: "system".to_string(),
        user_prompt: "user".to_string(),
        mode: AiTextMode::Default,
        response_format: AiTextResponseFormat::Text,
    }
}

#[tokio::test]
async fn a_stored_protocol_this_build_does_not_know_falls_back_to_the_catalog()
{
    let pool = ai_test_pool().await;
    sqlx::query(
        "INSERT INTO ai_provider_configs
             (provider_id, custom_name, protocol, enabled, endpoint)
             VALUES ('openai', 'Renamed', 'openai-next', TRUE,
                     'https://api.openai.com/v1')",
    )
    .execute(&pool)
    .await
    .unwrap();

    let config =
        load_provider_record(&pool, provider_definition("openai").unwrap())
            .await
            .unwrap();

    // The catalog protocol is what every downstream match arm understands.
    assert_eq!(config.protocol, AiProtocol::Openai);
    assert_eq!(config.custom_name.as_deref(), Some("Renamed"));
    assert_eq!(config.endpoint, "https://api.openai.com/v1");
    // Failing would have been the old behaviour; this asserts the row is used,
    // not merely that no panic happened.
    assert!(config.enabled);
}

#[tokio::test]
async fn one_garbage_protocol_row_does_not_abort_the_provider_list() {
    let pool = ai_test_pool().await;
    sqlx::query(
        "INSERT INTO ai_provider_configs
             (provider_id, custom_name, protocol, enabled, endpoint)
             VALUES ('ollama', NULL, 'not-a-protocol', TRUE,
                     'http://127.0.0.1:11434')",
    )
    .execute(&pool)
    .await
    .unwrap();

    let state = get_state_with_pool(&pool, false).await.unwrap();

    // Every catalog provider is still present, and the damaged one is returned
    // with its catalog protocol rather than dropped or turned into an error.
    assert_eq!(state.providers.len(), PROVIDERS.len());
    let ollama = state
        .providers
        .iter()
        .find(|provider| provider.provider_id == "ollama")
        .expect("the damaged provider must still be listed");
    assert_eq!(ollama.protocol, AiProtocol::Ollama);
    assert!(ollama.enabled);
    assert!(
        state
            .providers
            .iter()
            .any(|provider| provider.provider_id == "openai")
    );
}

#[tokio::test]
async fn a_provider_that_fails_to_load_does_not_abort_the_provider_list() {
    let pool = ai_test_pool().await;
    // An invalid endpoint is a provider-scoped failure that survives the
    // protocol fallback, so it exercises the per-provider resilience directly.
    sqlx::query(
        "INSERT INTO ai_provider_configs
             (provider_id, custom_name, protocol, enabled, endpoint)
             VALUES ('ollama', 'Broken', 'ollama', TRUE, 'not a url')",
    )
    .execute(&pool)
    .await
    .unwrap();

    let state = get_state_with_pool(&pool, false).await.unwrap();

    assert_eq!(state.providers.len(), PROVIDERS.len());
    let ollama = state
        .providers
        .iter()
        .find(|provider| provider.provider_id == "ollama")
        .expect("the failing provider must still be listed");
    // Catalog defaults, not the unusable row.
    assert_eq!(ollama.custom_name, None);
    assert_eq!(ollama.endpoint, "http://127.0.0.1:11434");
    assert!(!ollama.enabled);
    assert!(
        state
            .providers
            .iter()
            .any(|provider| provider.provider_id == "openai")
    );
}

#[tokio::test]
async fn a_fresh_ollama_provider_offers_a_selectable_model() {
    let pool = ai_test_pool().await;

    let config =
        load_provider_record(&pool, provider_definition("ollama").unwrap())
            .await
            .unwrap();

    // The bundled catalog lists 52 Ollama models with none enabled; without the
    // fallback every toggle would be off and the provider unusable.
    assert!(
        config.models.len() > 1,
        "the catalog list must be non-empty"
    );
    let selectable = config
        .models
        .iter()
        .filter(|model| model.enabled)
        .collect::<Vec<_>>();
    assert_eq!(selectable.len(), 1, "{selectable:#?}");
    assert_eq!(selectable[0].id, "deepseek-r1");
}

#[tokio::test]
async fn the_catalog_default_never_overrules_a_deliberate_choice() {
    let pool = ai_test_pool().await;
    sqlx::query(
        "INSERT INTO ai_provider_models
             (provider_id, model_id, display_name, enabled, source)
             VALUES ('ollama', 'gpt-oss:20b', 'GPT-OSS 20B', TRUE, 'builtin')",
    )
    .execute(&pool)
    .await
    .unwrap();

    let config =
        load_provider_record(&pool, provider_definition("ollama").unwrap())
            .await
            .unwrap();

    let enabled = config
        .models
        .iter()
        .filter(|model| model.enabled)
        .map(|model| model.id.as_str())
        .collect::<Vec<_>>();
    // The user's pick stays the only enabled row: the default is not injected
    // alongside it, and the check-model row is not promoted behind their back.
    assert_eq!(enabled, vec!["gpt-oss:20b"]);
}

#[tokio::test]
async fn a_provider_without_a_check_model_is_not_given_a_phantom_row() {
    // azure/azureai/lmstudio/vllm/xinference have no `check_model`, so nothing
    // may be invented for them; this is what keeps the predicate from becoming
    // "always inject one".
    let pool = ai_test_pool().await;
    for provider in ["azure", "azureai", "lmstudio", "vllm", "xinference"] {
        let definition = provider_definition(provider).unwrap();
        assert!(definition.check_model.is_empty(), "{provider}");
        let mut models = Vec::new();
        ensure_selectable_model(&pool, &mut models, definition)
            .await
            .unwrap();
        assert!(models.is_empty(), "{provider}");
    }
}

#[tokio::test]
async fn a_model_the_user_turned_off_stays_off_across_reloads() {
    let pool = ai_test_pool().await;
    sqlx::query(
        "INSERT INTO ai_provider_models
             (provider_id, model_id, display_name, enabled, source)
             VALUES ('ollama', 'deepseek-r1', 'DeepSeek R1', FALSE, 'builtin')",
    )
    .execute(&pool)
    .await
    .unwrap();

    let config =
        load_provider_record(&pool, provider_definition("ollama").unwrap())
            .await
            .unwrap();

    // Without the stored-decision check the fresh-install default would switch
    // this back on every load, and A-5 could never reject it.
    assert!(
        config
            .models
            .iter()
            .all(|model| !(model.id == "deepseek-r1" && model.enabled))
    );
}

#[tokio::test]
async fn discovered_models_are_stored_disabled_and_a_refresh_keeps_the_choice()
{
    let pool = ai_test_pool().await;
    let discovered = vec![
        ("fresh-model".to_string(), "Fresh Model".to_string()),
        ("premium-preview".to_string(), "Premium Preview".to_string()),
    ];

    store_discovered_models(&pool, "openai", &discovered)
        .await
        .unwrap();

    let stored = sqlx::query_as::<_, (String, i64)>(
        "SELECT model_id, enabled FROM ai_provider_models
             WHERE provider_id = 'openai' ORDER BY model_id",
    )
    .fetch_all(&pool)
    .await
    .unwrap();
    assert_eq!(
        stored,
        vec![
            ("fresh-model".to_string(), 0),
            ("premium-preview".to_string(), 0),
        ]
    );

    // The user enables exactly one of them, then refreshes again.
    sqlx::query(
        "UPDATE ai_provider_models SET enabled = TRUE
             WHERE provider_id = 'openai' AND model_id = 'fresh-model'",
    )
    .execute(&pool)
    .await
    .unwrap();
    store_discovered_models(
        &pool,
        "openai",
        &[
            ("fresh-model".to_string(), "Fresh Model v2".to_string()),
            ("premium-preview".to_string(), "Premium Preview".to_string()),
            ("new-model".to_string(), "New Model".to_string()),
        ],
    )
    .await
    .unwrap();

    let stored = sqlx::query_as::<_, (String, i64, String)>(
        "SELECT model_id, enabled, display_name FROM ai_provider_models
             WHERE provider_id = 'openai' ORDER BY model_id",
    )
    .fetch_all(&pool)
    .await
    .unwrap();
    assert_eq!(
        stored,
        vec![
            ("fresh-model".to_string(), 1, "Fresh Model v2".to_string()),
            ("new-model".to_string(), 0, "New Model".to_string()),
            (
                "premium-preview".to_string(),
                0,
                "Premium Preview".to_string()
            ),
        ]
    );
}

#[tokio::test]
async fn a_disabled_model_is_rejected_with_its_own_error_before_dispatch() {
    let pool = ai_test_pool().await;
    sqlx::query("UPDATE ai_settings SET enabled = TRUE WHERE id = 0")
        .execute(&pool)
        .await
        .unwrap();
    sqlx::query(
        "INSERT INTO ai_provider_configs
             (provider_id, custom_name, protocol, enabled, endpoint)
             VALUES ('ollama', NULL, 'ollama', TRUE,
                     'http://127.0.0.1:11434')",
    )
    .execute(&pool)
    .await
    .unwrap();
    sqlx::query(
        "INSERT INTO ai_provider_models
             (provider_id, model_id, display_name, enabled, source)
             VALUES ('ollama', 'deepseek-r1', 'DeepSeek R1', FALSE, 'builtin')",
    )
    .execute(&pool)
    .await
    .unwrap();
    // Enabling any other model suppresses the catalog default, so `deepseek-r1`
    // stays exactly the row the user turned off.
    sqlx::query(
        "INSERT INTO ai_provider_models
             (provider_id, model_id, display_name, enabled, source)
             VALUES ('ollama', 'qwen3-coder:480b', 'Qwen3 Coder 480B', TRUE,
                     'builtin')",
    )
    .execute(&pool)
    .await
    .unwrap();

    let error =
        resolve_text_request(&pool, &ai_test_request("ollama", "deepseek-r1"))
            .await
            .unwrap_err();

    let message = error.to_string();
    assert!(message.contains(AI_MODEL_DISABLED_ERROR), "{message}");
    assert!(!message.contains("is disabled"), "{message}");
    assert!(!message.contains("Select an AI model"), "{message}");
}

#[tokio::test]
async fn an_enabled_model_passes_the_gate_and_unknown_ids_are_left_alone() {
    let pool = ai_test_pool().await;
    sqlx::query("UPDATE ai_settings SET enabled = TRUE WHERE id = 0")
        .execute(&pool)
        .await
        .unwrap();
    sqlx::query(
        "INSERT INTO ai_provider_configs
             (provider_id, custom_name, protocol, enabled, endpoint)
             VALUES ('ollama', NULL, 'ollama', TRUE,
                     'http://127.0.0.1:11434')",
    )
    .execute(&pool)
    .await
    .unwrap();

    // The catalog default injected for a fresh provider is usable.
    let (definition, config) =
        resolve_text_request(&pool, &ai_test_request("ollama", "deepseek-r1"))
            .await
            .unwrap();
    assert_eq!(definition.id, "ollama");
    assert!(model_is_enabled(&config, "deepseek-r1"));

    // An id that is not part of the provider at all keeps going: Azure
    // deployments and local servers legitimately use undiscovered ids.
    assert!(
        resolve_text_request(
            &pool,
            &ai_test_request("ollama", "some-local-tag")
        )
        .await
        .is_ok()
    );
}

#[test]
fn opencode_session_headers_are_stable_and_provider_scoped() {
    let client = reqwest::Client::builder()
        .user_agent(crate::launcher_user_agent())
        .build()
        .unwrap();
    let session = Uuid::new_v4();
    for provider in ["opencodecodingplan", "opencodezen"] {
        for _ in 0..2 {
            let request = opencode_headers(
                client.post("https://opencode.ai/zen/go/v1/chat/completions"),
                provider,
                session,
            )
            .build()
            .unwrap();
            assert_eq!(
                request.headers()["x-opencode-session"],
                session.to_string()
            );
        }
    }
    let other_session = Uuid::new_v4();
    let other_request = opencode_headers(
        client.post("https://opencode.ai/zen/go/v1/chat/completions"),
        "opencodecodingplan",
        other_session,
    )
    .build()
    .unwrap();
    assert_eq!(
        other_request.headers()["x-opencode-session"],
        other_session.to_string()
    );
    assert_ne!(
        other_request.headers()["x-opencode-session"],
        session.to_string()
    );
    let request = opencode_headers(
        client.post("https://example.com/v1/chat/completions"),
        "openai",
        session,
    )
    .build()
    .unwrap();
    assert!(!request.headers().contains_key("x-opencode-session"));
}

#[tokio::test]
async fn retries_persistence_without_redeeming_the_authorization_code() {
    let flow_id = Uuid::new_v4();
    OAUTH_FLOWS.lock().unwrap().insert(
        flow_id,
        PendingOAuthFlow {
            provider_id: "chatgpt".to_string(),
            // Invalid device state makes accidental re-exchange fail before
            // making any network request.
            device_code: "already-consumed".to_string(),
            expires_at: now_seconds() + 900,
            credentials: Some(OAuthCredentials {
                access_token: "exchanged-access-token".to_string(),
                refresh_token: Some("exchanged-refresh-token".to_string()),
                ..Default::default()
            }),
        },
    );
    let result = poll_oauth_with_storage(flow_id, |provider_id, value| {
        assert_eq!(provider_id, "chatgpt");
        assert_eq!(value.unwrap().access_token, "exchanged-access-token");
        Err(input_error("Simulated persistence failure".to_string()))
    })
    .await;
    assert!(result.is_err());
    assert!(OAUTH_FLOWS.lock().unwrap().contains_key(&flow_id));
    let result = poll_oauth_with_storage(flow_id, |_, value| {
        let value = value.unwrap();
        assert_eq!(value.access_token, "exchanged-access-token");
        assert_eq!(
            value.refresh_token.as_deref(),
            Some("exchanged-refresh-token")
        );
        Ok(())
    })
    .await
    .unwrap();
    assert!(matches!(result, OAuthPollStatus::Success));
    assert!(!OAUTH_FLOWS.lock().unwrap().contains_key(&flow_id));
}

#[test]
fn auth_type_uses_lobehub_wire_values() {
    assert_eq!(
        serde_json::to_value(AiAuthType::ApiKey).unwrap(),
        json!("apiKey")
    );
    assert_eq!(
        serde_json::to_value(AiAuthType::OAuthDeviceFlow).unwrap(),
        json!("oauthDeviceFlow")
    );
    assert_eq!(
        serde_json::to_value(AiAuthType::None).unwrap(),
        json!("none")
    );
}

#[test]
fn parses_chatgpt_account_and_expiry_claims() {
    let claims = URL_SAFE_NO_PAD.encode(
        serde_json::to_vec(&json!({
            "exp": 1_800_000_000_u64,
            "https://api.openai.com/auth": {
                "chatgpt_account_id": "account-123"
            }
        }))
        .unwrap(),
    );
    let token = format!("header.{claims}.signature");
    assert_eq!(jwt_account_id(&token).as_deref(), Some("account-123"));
    assert_eq!(jwt_expiry(&token), Some(1_800_000_000));
}

#[test]
fn parses_openai_response_event_stream() {
    let stream = concat!(
        "event: response.output_text.delta\n",
        "data: {\"type\":\"response.output_text.delta\",\"delta\":\"Hello\"}\n\n",
        "data: {\"type\":\"response.output_text.delta\",\"delta\":\" world\"}\n\n",
        "data: {\"type\":\"response.completed\",\"response\":{\"status\":\"completed\"}}\n\n",
        "data: [DONE]\n",
    );
    assert_eq!(
        openai_response_stream_content(stream, "ChatGPT").unwrap(),
        "Hello world"
    );
}

#[test]
fn detects_github_copilot_response_models() {
    for model in [
        "gpt-5.4",
        "gpt-5.2-codex",
        "gpt-5-mini",
        "oswe-vscode-prime",
    ] {
        assert!(github_copilot_uses_responses(model), "{model}");
    }
    for model in [
        "gpt-5.1",
        "gpt-5-chat-latest",
        "gpt-4.1",
        "gemini-3.1-pro-preview",
    ] {
        assert!(!github_copilot_uses_responses(model), "{model}");
    }
}

#[test]
fn catalog_matches_lobehub_text_provider_scope() {
    assert_eq!(PROVIDERS.len(), 80);
    for excluded in ["bfl", "comfyui", "fal", "replicate", "lobehub"] {
        assert!(!PROVIDERS.iter().any(|provider| provider.id == excluded));
    }
    for oauth in ["githubcopilot", "chatgpt", "supergrok"] {
        assert_eq!(
            provider_definition(oauth).unwrap().auth_type,
            AiAuthType::OAuthDeviceFlow
        );
    }
}

#[test]
fn sponsored_providers_are_marked_in_catalog() {
    let catalog = catalog();
    assert!(
        catalog
            .iter()
            .filter(|provider| provider.sponsored)
            .all(|provider| SPONSORED_PROVIDERS.contains(&provider.id))
    );
    assert!(
        catalog
            .iter()
            .filter(|provider| SPONSORED_PROVIDERS.contains(&provider.id))
            .all(|provider| provider.sponsored)
    );
    assert!(
        catalog
            .iter()
            .any(|provider| provider.id == "codeflow" && provider.sponsored)
    );
}

#[test]
fn bundled_catalog_contains_supported_lobehub_chat_models() {
    let model_count = BUILTIN_MODEL_CATALOG
        .providers
        .values()
        .map(Vec::len)
        .sum::<usize>();
    assert!(model_count > 0);
    assert_eq!(BUILTIN_MODEL_CATALOG.providers.len(), PROVIDERS.len());
    for provider in PROVIDERS {
        let models = BUILTIN_MODEL_CATALOG.providers.get(provider.id).expect(
            "every supported provider should be present in the catalog",
        );
        let mut model_ids = std::collections::HashSet::new();
        assert!(models.iter().all(|model| {
            !model.id.trim().is_empty()
                && !model.name.trim().is_empty()
                && model_ids.insert(model.id.as_str())
        }));
    }
}

#[test]
fn extracts_openai_chat_and_responses_text() {
    assert_eq!(
        openai_content(
            &json!({ "choices": [{ "message": { "content": "chat" } }] })
        ),
        Some("chat".to_string())
    );
    assert_eq!(
        openai_content(
            &json!({ "output": [{ "content": [{ "text": "response" }] }] })
        ),
        Some("response".to_string())
    );
}

#[tokio::test]
async fn checked_openai_responses_keep_authentication_and_rate_limit_errors() {
    for (status, category) in [
        (http::StatusCode::UNAUTHORIZED, "AI_AUTHENTICATION_FAILED"),
        (http::StatusCode::TOO_MANY_REQUESTS, "AI_RATE_LIMITED"),
    ] {
        let response = reqwest::Response::from(
            http::Response::builder()
                .status(status)
                .body(reqwest::Body::from(r#"{"error":"denied"}"#))
                .unwrap(),
        );
        let error = checked_openai_response_stream(response, "OpenAI")
            .await
            .unwrap_err();
        assert!(error.to_string().contains(category), "{error:#}");
        assert!(
            error
                .to_string()
                .contains(status.as_u16().to_string().as_str())
        );
    }
}

#[tokio::test]
async fn checked_openai_responses_parse_server_sent_text_events() {
    let response = reqwest::Response::from(http::Response::new(
        reqwest::Body::from(concat!(
            "data: {\"type\":\"response.output_text.delta\",\"delta\":\"MC-\"}\n\n",
            "data: {\"type\":\"response.output_text.delta\",\"delta\":\"Trin\"}\n\n",
            "data: {\"type\":\"response.completed\",\"response\":{\"status\":\"completed\"}}\n\n",
        )),
    ));

    let text = checked_openai_response_stream(response, "OpenAI")
        .await
        .unwrap();

    assert_eq!(text, "MC-Trin");
}

#[test]
fn model_fetch_parsers_are_protocol_specific() {
    assert_eq!(
        discovered_models(
            &json!({ "data": [{ "id": "gpt", "display_name": "GPT" }] }),
            AiProtocol::Openai
        ),
        vec![("gpt".to_string(), "GPT".to_string())]
    );
    assert_eq!(
        discovered_models(
            &json!({
                "models": [{
                    "name": "models/gemini",
                    "displayName": "Gemini",
                    "supportedGenerationMethods": ["generateContent"]
                }]
            }),
            AiProtocol::Google
        ),
        vec![("gemini".to_string(), "Gemini".to_string())]
    );
}

#[test]
fn model_fetch_filters_non_text_models() {
    assert_eq!(
        discovered_models(
            &json!({
                "data": [
                    { "id": "text-embedding-3-small" },
                    { "id": "image-model", "type": "image" },
                    { "id": "chat-model", "type": "chat", "display_name": "Chat" },
                    { "id": "llama", "task": "text-generation" }
                ]
            }),
            AiProtocol::Openai
        ),
        vec![
            ("chat-model".to_string(), "Chat".to_string()),
            ("llama".to_string(), "llama".to_string()),
        ]
    );
    assert!(
        discovered_models(
            &json!({
                "models": [{
                    "name": "models/text-embedding-004",
                    "supportedGenerationMethods": ["embedContent"]
                }]
            }),
            AiProtocol::Google
        )
        .is_empty()
    );
}

#[test]
fn bedrock_payloads_and_text_responses_match_model_families() {
    let request = AiTextRequest {
        provider_id: "bedrock".to_string(),
        model_id: "meta.llama3-8b-instruct-v1:0".to_string(),
        system_prompt: "system".to_string(),
        user_prompt: "user".to_string(),
        mode: AiTextMode::Default,
        response_format: AiTextResponseFormat::Text,
    };
    let meta = bedrock_body(&request);
    assert_eq!(meta["max_gen_len"], 4096);
    assert!(meta["prompt"].as_str().unwrap().contains("<<SYS>>"));

    let mut legacy_claude = request.clone();
    legacy_claude.model_id = "anthropic.claude-v2:1".to_string();
    let legacy = bedrock_body(&legacy_claude);
    assert_eq!(legacy["max_tokens_to_sample"], 4096);
    assert!(legacy["prompt"].as_str().unwrap().ends_with("Assistant:"));

    let mut modern_claude = request;
    modern_claude.model_id = "anthropic.claude-3-haiku".to_string();
    let modern = bedrock_body(&modern_claude);
    assert_eq!(modern["anthropic_version"], "bedrock-2023-05-31");
    assert_eq!(modern["messages"][0]["content"], "user");

    assert_eq!(
        bedrock_content(&json!({ "content": [{ "text": "claude" }] })),
        Some("claude".to_string())
    );
    assert_eq!(
        bedrock_content(&json!({ "completion": "legacy" })),
        Some("legacy".to_string())
    );
    assert_eq!(
        bedrock_content(&json!({ "generation": "llama" })),
        Some("llama".to_string())
    );
}

#[test]
fn json_response_mode_uses_native_provider_constraints() {
    let request = AiTextRequest {
        provider_id: "openai".to_string(),
        model_id: "gpt-test".to_string(),
        system_prompt: "system".to_string(),
        user_prompt: "user".to_string(),
        mode: AiTextMode::Translation,
        response_format: AiTextResponseFormat::JsonObject,
    };
    let mut chat = json!({});
    apply_openai_response_format(&mut chat, &request);
    assert_eq!(chat["response_format"]["type"], "json_object");

    let mut responses = json!({});
    apply_responses_translation_options(&mut responses, &request);
    assert_eq!(responses["text"]["format"]["type"], "json_object");

    let google = google_generation_config(&request);
    assert_eq!(google["responseMimeType"], "application/json");
}

#[test]
fn bedrock_sigv4_request_contains_required_headers() {
    assert_eq!(
        bytes_to_hex(hmac_sha256(
            b"key",
            "The quick brown fox jumps over the lazy dog"
        )),
        "f7bc83f430538424b13298e6aa6fb143ef4d59a14946175997479dbc2d1a3cd8"
    );
    let request = sign_bedrock_request(
            reqwest::Client::new().post(
                "https://bedrock-runtime.us-east-1.amazonaws.com/model/example/invoke",
            ),
            "https://bedrock-runtime.us-east-1.amazonaws.com/model/example/invoke",
            "us-east-1",
            "{}",
            "AKIDEXAMPLE",
            "secret",
            Some("session"),
        )
        .unwrap()
        .build()
        .unwrap();
    let authorization = request
        .headers()
        .get("authorization")
        .unwrap()
        .to_str()
        .unwrap();
    assert!(
        authorization.starts_with("AWS4-HMAC-SHA256 Credential=AKIDEXAMPLE/")
    );
    assert!(authorization.contains("/us-east-1/bedrock/aws4_request"));
    assert!(authorization.contains(
            "SignedHeaders=content-type;host;x-amz-content-sha256;x-amz-date;x-amz-security-token"
        ));
    assert_eq!(
        request.headers().get("x-amz-security-token").unwrap(),
        "session"
    );
    assert_eq!(
        request.headers().get("x-amz-content-sha256").unwrap(),
        "44136fa355b3678a1146ad16f7e8649e94fb4fc21fe77e8310c060f61caaff8a"
    );
}

#[test]
fn vertex_service_account_defaults_and_claims_are_validated() {
    let account = parse_vertex_service_account(
        r#"{
                "client_email": "service@example.test",
                "private_key": "private-key",
                "project_id": "project"
            }"#,
    )
    .unwrap();
    assert_eq!(account.token_uri, default_google_token_uri());
    let claims = vertex_jwt_claims(&account, 100);
    assert_eq!(claims.iss, "service@example.test");
    assert_eq!(claims.aud, "https://oauth2.googleapis.com/token");
    assert_eq!((claims.iat, claims.exp), (100, 3700));

    assert!(
        parse_vertex_service_account(
            r#"{
                "client_email": "service@example.test",
                "private_key": "private-key",
                "project_id": "",
                "token_uri": "http://oauth.example.test/token"
            }"#,
        )
        .is_err()
    );
}

#[test]
fn a_provider_error_never_repeats_the_api_key() {
    let key = "sk-live-very-secret-value";
    let endpoint = format!(
        "https://generativelanguage.googleapis.com/v1beta/models?key={key}"
    );
    let message = format!(
        "Google returned no text models from {}",
        redact_url(&endpoint)
    );
    assert!(!message.contains(key), "{message}");
    assert!(message.contains("generativelanguage.googleapis.com"));
}

#[test]
fn a_transport_error_summary_drops_the_request_url() {
    let key = "AIzaSy-example-leaked-key";
    let rendered = format!(
        "error sending request for url (https://example.test/v1beta/models?key={key})"
    );
    let summary = super::protocol::strip_error_url(&rendered);
    assert!(!summary.contains(key), "{summary}");
    assert!(!summary.contains("for url"), "{summary}");
    assert_eq!(summary, "error sending request");

    // A message that is only a URL must not survive either.
    assert_eq!(
        super::protocol::strip_error_url(&format!(
            "error following redirect for url (https://example.test/?key={key})"
        )),
        "error following redirect"
    );
}

#[test]
fn response_bodies_are_collapsed_and_bounded() {
    let body = "{\n  \"error\": \"denied\",\n  \"detail\": \"x\"\n}";
    let snippet = sanitized_body_snippet(body);
    assert!(!snippet.contains('\n'), "{snippet}");
    assert!(snippet.contains("denied"));

    let huge = "a".repeat(10_000);
    assert!(sanitized_body_snippet(&huge).chars().count() <= 160);
    assert_eq!(sanitized_body_snippet("   \n\n "), "no response body");
}

#[test]
fn redact_url_removes_query_and_user_information() {
    assert_eq!(
        redact_url("https://user:pass@example.test/v1/models?key=secret"),
        "https://example.test/v1/models"
    );
    assert_eq!(
        redact_url("https://example.test/v1/models"),
        "https://example.test/v1/models"
    );
    assert_eq!(redact_url("not a url"), "[invalid URL]");
}
