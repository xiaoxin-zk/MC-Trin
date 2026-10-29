use super::*;

pub(super) fn text_client(request: &AiTextRequest) -> &'static reqwest::Client {
    match request.mode {
        AiTextMode::Default => &AI_CLIENT,
        AiTextMode::Translation => &AI_TRANSLATION_CLIENT,
    }
}

// OpenCode routes and caches requests by conversation, not by API key.
pub(super) fn opencode_headers(
    builder: reqwest::RequestBuilder,
    provider_id: &str,
    session_id: Uuid,
) -> reqwest::RequestBuilder {
    if matches!(provider_id, "opencodecodingplan" | "opencodezen") {
        builder.header("x-opencode-session", session_id.to_string())
    } else {
        builder
    }
}

fn lowest_openai_reasoning_effort(model_id: &str) -> Option<&'static str> {
    let model = model_id
        .rsplit('/')
        .next()
        .unwrap_or(model_id)
        .to_ascii_lowercase();
    if model.contains("chat-latest") {
        return None;
    }
    match model.as_str() {
        "gpt-5.4-pro" | "gpt-5.2-pro" => Some("medium"),
        "gpt-5-pro" => Some("high"),
        "gpt-5" | "gpt-5-mini" | "gpt-5-nano" | "gpt-5-codex" => {
            Some("minimal")
        }
        _ if model.starts_with("o1-")
            || model.starts_with("o3-")
            || model == "o1"
            || model == "o3"
            || model.starts_with("o4-mini") =>
        {
            Some("minimal")
        }
        _ if model.starts_with("gpt-5.1")
            || model.starts_with("gpt-5.2")
            || model.starts_with("gpt-5.3")
            || model.starts_with("gpt-5.4")
            || model.starts_with("gpt-5.5")
            || model.starts_with("gpt-5.6")
            || model.starts_with("gpt-oss-") =>
        {
            Some("none")
        }
        _ => None,
    }
}

pub(super) fn insert_json_option(body: &mut Value, key: &str, value: Value) {
    if let Some(object) = body.as_object_mut() {
        object.insert(key.to_string(), value);
    }
}

pub(super) fn apply_openai_translation_options(
    body: &mut Value,
    request: &AiTextRequest,
) {
    if request.mode != AiTextMode::Translation {
        return;
    }
    if let Some(effort) = lowest_openai_reasoning_effort(&request.model_id) {
        insert_json_option(body, "reasoning_effort", json!(effort));
        return;
    }

    let model = request.model_id.to_ascii_lowercase();
    let model_name = model.rsplit('/').next().unwrap_or(&model);
    let explicit_thinking = model.contains("thinking") || model.contains("qwq");
    if model.contains("/qwen3") && !explicit_thinking {
        insert_json_option(body, "reasoning_effort", json!("none"));
    } else if matches!(
        request.provider_id.as_str(),
        "qwen" | "bailiancodingplan"
    ) && model.contains("qwen")
        && !explicit_thinking
    {
        insert_json_option(body, "enable_thinking", json!(false));
    } else if matches!(
        model_name,
        "deepseek-reasoner" | "deepseek-v4-flash" | "deepseek-v4-pro"
    ) || ((request.provider_id == "moonshot"
        || request.provider_id == "fireworksai")
        && model.contains("kimi-k2")
        && !model.contains("instruct"))
        || ((request.provider_id == "minimax"
            || request.provider_id == "minimaxcodingplan")
            && model.contains("minimax-m"))
        || model_name.starts_with("glm-")
    {
        insert_json_option(body, "thinking", json!({ "type": "disabled" }));
    }
}

pub(super) fn apply_openai_response_format(
    body: &mut Value,
    request: &AiTextRequest,
) {
    if request.response_format == AiTextResponseFormat::JsonObject {
        insert_json_option(
            body,
            "response_format",
            json!({ "type": "json_object" }),
        );
    }
}

pub(super) fn apply_responses_translation_options(
    body: &mut Value,
    request: &AiTextRequest,
) {
    if request.response_format == AiTextResponseFormat::JsonObject {
        insert_json_option(
            body,
            "text",
            json!({ "format": { "type": "json_object" } }),
        );
    }
    if request.mode != AiTextMode::Translation {
        return;
    }
    if let Some(effort) = lowest_openai_reasoning_effort(&request.model_id) {
        insert_json_option(body, "reasoning", json!({ "effort": effort }));
        if let Some(object) = body.as_object_mut() {
            object.remove("include");
        }
    }
}

pub(super) fn google_generation_config(request: &AiTextRequest) -> Value {
    let mut config = json!({ "temperature": 0 });
    if request.response_format == AiTextResponseFormat::JsonObject {
        insert_json_option(
            &mut config,
            "responseMimeType",
            json!("application/json"),
        );
    }
    if request.mode != AiTextMode::Translation {
        return config;
    }
    let model = request.model_id.to_ascii_lowercase();
    if !model.starts_with("gemini-") {
        return config;
    }
    let thinking = if model == "gemini-pro-latest" {
        json!({ "thinkingLevel": "low", "includeThoughts": false })
    } else if model.starts_with("gemini-3")
        || model.starts_with("gemini-flash-latest")
        || model.starts_with("gemini-flash-lite-latest")
    {
        json!({ "thinkingLevel": "minimal", "includeThoughts": false })
    } else {
        json!({ "thinkingBudget": 0, "includeThoughts": false })
    };
    insert_json_option(&mut config, "thinkingConfig", thinking);
    config
}

pub(super) fn openai_response_stream_content(
    text: &str,
    provider: &str,
) -> crate::Result<String> {
    let mut output = String::new();
    let mut completed_output = None;
    let mut done_output = None;
    let mut saw_event = false;
    for line in text.lines() {
        let Some(data) = line.strip_prefix("data:") else {
            continue;
        };
        let data = data.trim();
        if data.is_empty() || data == "[DONE]" {
            continue;
        }
        saw_event = true;
        let event: Value = serde_json::from_str(data).map_err(|_| {
            ErrorKind::OtherError(format!(
                "AI_PROVIDER_FAILED: {provider} returned an invalid event stream"
            ))
        })?;
        match event.get("type").and_then(Value::as_str) {
            Some("response.output_text.delta") => {
                if let Some(delta) = event.get("delta").and_then(Value::as_str)
                {
                    output.push_str(delta);
                }
            }
            Some("response.output_text.done") => {
                done_output = event
                    .get("text")
                    .and_then(Value::as_str)
                    .map(str::to_string);
            }
            Some("response.completed") => {
                completed_output =
                    event.get("response").and_then(openai_content);
            }
            Some("error" | "response.failed" | "response.incomplete") => {
                let message = event
                    .pointer("/response/error/message")
                    .or_else(|| event.pointer("/error/message"))
                    .or_else(|| event.get("message"))
                    .and_then(Value::as_str)
                    .unwrap_or("Provider response failed");
                return Err(ErrorKind::OtherError(format!(
                    "AI_PROVIDER_FAILED: {message}"
                ))
                .into());
            }
            _ => {}
        }
    }
    if !output.is_empty() {
        return Ok(output);
    }
    if let Some(output) = done_output.or(completed_output) {
        return Ok(output);
    }
    if !saw_event
        && let Ok(value) = serde_json::from_str::<Value>(text)
        && let Some(output) = openai_content(&value)
    {
        return Ok(output);
    }
    Err(ErrorKind::OtherError(format!(
        "AI_PROVIDER_FAILED: {provider} returned no text output"
    ))
    .into())
}

pub(super) async fn checked_openai_response_stream(
    response: Response,
    provider: &str,
) -> crate::Result<String> {
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
    openai_response_stream_content(&text, provider)
}

pub(super) fn ai_network_error(
    provider: &str,
    error: reqwest::Error,
) -> crate::Error {
    ErrorKind::OtherError(format!(
        "AI_NETWORK_FAILED: Could not reach {provider}: {error}"
    ))
    .into()
}

pub(super) fn openai_content(value: &Value) -> Option<String> {
    if let Some(text) = value.get("output_text").and_then(Value::as_str) {
        return Some(text.to_string());
    }
    if let Some(text) = value
        .get("choices")
        .and_then(Value::as_array)
        .and_then(|choices| choices.first())
        .and_then(|choice| choice.get("message"))
        .and_then(|message| message.get("content"))
        .and_then(Value::as_str)
    {
        return Some(text.to_string());
    }
    if let Some(text) = value
        .get("choices")
        .and_then(Value::as_array)
        .and_then(|choices| choices.first())
        .and_then(|choice| choice.get("message"))
        .and_then(|message| message.get("content"))
        .and_then(Value::as_array)
        .and_then(|content| {
            content
                .iter()
                .find_map(|item| item.get("text").and_then(Value::as_str))
        })
    {
        return Some(text.to_string());
    }
    value
        .get("output")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|item| item.get("content").and_then(Value::as_array))
        .flatten()
        .find_map(|item| item.get("text").and_then(Value::as_str))
        .map(str::to_string)
}
