use super::*;

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
