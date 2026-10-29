//! Static AI provider and model catalogs.

use std::collections::HashMap;
use std::sync::LazyLock;

use serde::Deserialize;

use super::{AiAuthType, AiProtocol, AiProviderDefinition, input_error};

macro_rules! provider {
	($id:literal, $name:literal, $protocol:ident, $endpoint:literal, $model:literal, $fetch:literal) => {
		AiProviderDefinition {
			id: $id,
			name: $name,
			protocol: AiProtocol::$protocol,
			auth_type: AiAuthType::ApiKey,
			default_endpoint: $endpoint,
			check_model: $model,
			show_model_fetcher: $fetch,
			required_settings: &[],
			sponsored: false,
		}
	};
	($id:literal, $name:literal, $protocol:ident, $endpoint:literal, $model:literal, $fetch:literal, $auth:ident) => {
		AiProviderDefinition {
			id: $id,
			name: $name,
			protocol: AiProtocol::$protocol,
			auth_type: AiAuthType::$auth,
			default_endpoint: $endpoint,
			check_model: $model,
			show_model_fetcher: $fetch,
			required_settings: &[],
			sponsored: false,
		}
	};
	($id:literal, $name:literal, $protocol:ident, $endpoint:literal, $model:literal, $fetch:literal, [$($setting:literal),*]) => {
		AiProviderDefinition {
			id: $id,
			name: $name,
			protocol: AiProtocol::$protocol,
			auth_type: AiAuthType::ApiKey,
			default_endpoint: $endpoint,
			check_model: $model,
			show_model_fetcher: $fetch,
			required_settings: &[$($setting),*],
			sponsored: false,
		}
	};
}

// Text-capable providers from LobeHub's model bank. Image-only providers and
// LobeHub's account/credit proxy are intentionally excluded.
pub(super) const PROVIDERS: &[AiProviderDefinition] = &[
    provider!(
        "ai21",
        "Ai21Labs",
        Openai,
        "https://api.ai21.com/studio/v1",
        "jamba-mini",
        false
    ),
    provider!(
        "ai302",
        "302.AI",
        Openai,
        "https://api.302.ai/v1",
        "gpt-4o",
        true
    ),
    provider!(
        "ai360",
        "360 AI",
        Openai,
        "https://api.360.cn/v1",
        "360gpt-turbo",
        true
    ),
    provider!(
        "aihubmix",
        "AIHubMix",
        Router,
        "https://aihubmix.com/v1",
        "gpt-4.1-nano",
        true
    ),
    provider!(
        "akashchat",
        "AkashChat",
        Openai,
        "https://chatapi.akash.network/api/v1",
        "Meta-Llama-3-1-8B-Instruct-FP8",
        true
    ),
    provider!(
        "antgroup",
        "AntGroup",
        Openai,
        "https://api.ant-ling.com/v1",
        "Ling-2.6-flash",
        false
    ),
    provider!(
        "anthropic",
        "Anthropic",
        Anthropic,
        "https://api.anthropic.com",
        "claude-opus-4-5-20251101",
        true
    ),
    provider!(
        "azure",
        "Azure OpenAI",
        Azure,
        "",
        "",
        false,
        ["deployment", "api_version"]
    ),
    provider!("azureai", "Azure AI", AzureAi, "", "", false),
    provider!(
        "baichuan",
        "Baichuan",
        Openai,
        "https://api.baichuan-ai.com/v1",
        "Baichuan3-Turbo",
        true
    ),
    provider!(
        "bailiancodingplan",
        "Aliyun Bailian Coding Plan",
        Openai,
        "https://coding.dashscope.aliyuncs.com/v1",
        "qwen3-coder-plus",
        false
    ),
    provider!(
        "bedrock",
        "Bedrock",
        Bedrock,
        "",
        "anthropic.claude-instant-v1",
        false,
        ["region"]
    ),
    provider!(
        "cerebras",
        "Cerebras",
        Openai,
        "https://api.cerebras.ai/v1",
        "llama3.1-8b",
        true
    ),
    provider!(
        "chatgpt",
        "ChatGPT",
        Openai,
        "https://chatgpt.com/backend-api/codex",
        "gpt-5.5",
        false,
        OAuthDeviceFlow
    ),
    provider!(
        "cloudflare",
        "Cloudflare Workers AI",
        Cloudflare,
        "https://api.cloudflare.com/client/v4",
        "@hf/meta-llama/meta-llama-3-8b-instruct",
        true,
        ["account_id"]
    ),
    // Sponsored listing; kept out of LobeHub syncs by
    // scripts/axolotl/sync-lobehub-models.mjs. See SPONSORED_PROVIDERS.
    provider!(
        "codeflow",
        "CodeFlow",
        Openai,
        "https://codeflow.sh/v1",
        "gpt-6.1-sol",
        true
    ),
    provider!(
        "cohere",
        "Cohere",
        Openai,
        "https://api.cohere.ai/compatibility/v1",
        "command-r7b-12-2024",
        false
    ),
    provider!(
        "cometapi",
        "CometAPI",
        Openai,
        "https://api.cometapi.com/v1",
        "gpt-5-mini",
        true
    ),
    provider!(
        "deepseek",
        "DeepSeek",
        Openai,
        "https://api.deepseek.com",
        "deepseek-v4-flash",
        true
    ),
    provider!(
        "fireworksai",
        "Fireworks AI",
        Openai,
        "https://api.fireworks.ai/inference/v1",
        "accounts/fireworks/models/llama-v3p2-3b-instruct",
        true
    ),
    provider!(
        "giteeai",
        "Gitee AI",
        Openai,
        "https://ai.gitee.com/v1",
        "Qwen2.5-72B-Instruct",
        true
    ),
    provider!(
        "github",
        "GitHub Models",
        AzureAi,
        "https://models.github.ai/inference",
        "microsoft/Phi-3-mini-4k-instruct",
        true
    ),
    provider!(
        "githubcopilot",
        "GitHub Copilot",
        Openai,
        "https://api.githubcopilot.com",
        "gpt-5-mini",
        false,
        OAuthDeviceFlow
    ),
    provider!(
        "glmcodingplan",
        "GLM Coding Plan",
        Openai,
        "https://open.bigmodel.cn/api/coding/paas/v4",
        "GLM-4.7",
        false
    ),
    provider!(
        "google",
        "Google",
        Google,
        "https://generativelanguage.googleapis.com",
        "gemini-3-flash-preview",
        true
    ),
    provider!(
        "groq",
        "Groq",
        Openai,
        "https://api.groq.com/openai/v1",
        "llama-3.1-8b-instant",
        true
    ),
    provider!(
        "higress",
        "Higress",
        Openai,
        "http://127.0.0.1:8080/v1",
        "qwen-max",
        true
    ),
    provider!(
        "huggingface",
        "Hugging Face",
        Huggingface,
        "https://router.huggingface.co/v1",
        "mistralai/Mistral-7B-Instruct-v0.2",
        true
    ),
    provider!(
        "hunyuan",
        "Hunyuan",
        Openai,
        "https://tokenhub.tencentmaas.com/v1",
        "hunyuan-role-latest",
        false
    ),
    provider!(
        "infiniai",
        "InfiniAI",
        Openai,
        "https://cloud.infini-ai.com/maas/v1",
        "qwen3-8b",
        true
    ),
    provider!(
        "internlm",
        "InternLM",
        Openai,
        "https://chat.intern-ai.org.cn/api/v1",
        "intern-latest",
        true
    ),
    provider!(
        "jina",
        "Jina AI",
        Openai,
        "https://deepsearch.jina.ai/v1",
        "jina-deepsearch-v1",
        true
    ),
    provider!(
        "kimicodingplan",
        "Kimi Code",
        Anthropic,
        "https://api.kimi.com/coding",
        "kimi-k2.5",
        false
    ),
    provider!(
        "lmstudio",
        "LM Studio",
        Openai,
        "http://127.0.0.1:1234/v1",
        "",
        true,
        None
    ),
    provider!(
        "longcat",
        "LongCat",
        Openai,
        "https://api.longcat.chat/openai/v1",
        "LongCat-2.0",
        true
    ),
    provider!(
        "minimax",
        "MiniMax",
        Openai,
        "https://api.minimaxi.com/v1",
        "MiniMax-M2.1",
        false
    ),
    provider!(
        "minimaxcodingplan",
        "MiniMax Token Plan",
        Openai,
        "https://api.minimaxi.com/v1",
        "MiniMax-M2.7",
        false
    ),
    provider!(
        "mistral",
        "Mistral",
        Openai,
        "https://api.mistral.ai/v1",
        "ministral-3b-latest",
        true
    ),
    provider!(
        "modelscope",
        "ModelScope",
        Openai,
        "https://api-inference.modelscope.cn/v1",
        "Qwen/Qwen3-4B",
        true
    ),
    provider!(
        "moonshot",
        "Moonshot",
        Openai,
        "https://api.moonshot.cn/v1",
        "kimi-k2.6",
        true
    ),
    provider!(
        "nebius",
        "Nebius",
        Openai,
        "https://api.studio.nebius.com/v1",
        "Qwen/Qwen2.5-Coder-7B",
        true
    ),
    provider!("newapi", "New API", Router, "", "gpt-4o-mini", true),
    provider!(
        "novita",
        "Novita",
        Openai,
        "https://api.novita.ai/v3/openai",
        "meta-llama/llama-3.1-8b-instruct",
        true
    ),
    provider!(
        "nvidia",
        "NVIDIA",
        Openai,
        "https://integrate.api.nvidia.com/v1",
        "meta/llama-3.2-1b-instruct",
        true
    ),
    provider!(
        "ollama",
        "Ollama",
        Ollama,
        "http://127.0.0.1:11434",
        "deepseek-r1",
        true,
        None
    ),
    provider!(
        "ollamacloud",
        "Ollama Cloud",
        Openai,
        "https://ollama.com/v1",
        "gpt-oss:20b",
        true
    ),
    provider!(
        "openai",
        "OpenAI",
        Openai,
        "https://api.openai.com/v1",
        "gpt-5.2",
        true
    ),
    provider!(
        "opencodecodingplan",
        "OpenCode Go",
        Openai,
        "https://opencode.ai/zen/go/v1",
        "glm-5.1",
        true
    ),
    provider!(
        "opencodezen",
        "OpenCode Zen",
        Openai,
        "https://opencode.ai/zen/v1",
        "claude-sonnet-4-5",
        true
    ),
    provider!(
        "openrouter",
        "OpenRouter",
        Openai,
        "https://openrouter.ai/api/v1",
        "google/gemma-2-9b-it:free",
        true
    ),
    provider!(
        "perplexity",
        "Perplexity",
        Openai,
        "https://api.perplexity.ai",
        "sonar",
        false
    ),
    provider!(
        "ppio",
        "PPIO",
        Openai,
        "https://api.ppinfra.com/v3/openai",
        "deepseek/deepseek-r1-distill-qwen-32b",
        true
    ),
    provider!(
        "qiniu",
        "Qiniu",
        Openai,
        "https://openai.qiniu.com/v1",
        "deepseek-r1",
        true
    ),
    provider!(
        "qwen",
        "Aliyun Bailian",
        Openai,
        "https://dashscope.aliyuncs.com/compatible-mode/v1",
        "qwen-flash",
        true
    ),
    provider!(
        "sambanova",
        "SambaNova",
        Openai,
        "https://api.sambanova.ai/v1",
        "Meta-Llama-3.2-1B-Instruct",
        false
    ),
    provider!(
        "search1api",
        "Search1API",
        Openai,
        "https://api.search1api.com/v1",
        "deepseek-r1-70b-fast-online",
        true
    ),
    provider!(
        "sensenova",
        "SenseNova",
        Openai,
        "https://token.sensenova.cn/v1",
        "sensenova-6.7-flash-lite",
        true
    ),
    provider!(
        "siliconcloud",
        "SiliconCloud",
        Openai,
        "https://api.siliconflow.cn/v1",
        "Pro/zai-org/GLM-4.7",
        true
    ),
    provider!(
        "spark",
        "Spark",
        Openai,
        "https://spark-api-open.xf-yun.com/v1",
        "lite",
        false
    ),
    provider!(
        "stepfun",
        "Stepfun",
        Openai,
        "https://api.stepfun.com/v1",
        "step-2-mini",
        true
    ),
    provider!(
        "straico",
        "Straico",
        Openai,
        "https://api.straico.com/v0",
        "microsoft/phi-4",
        true
    ),
    provider!(
        "streamlake",
        "StreamLake",
        Openai,
        "https://wanqing.streamlakeapi.com/api/gateway/v1/endpoints",
        "KAT-Coder-Air-V1",
        false
    ),
    provider!(
        "supergrok",
        "SuperGrok",
        Openai,
        "https://api.x.ai/v1",
        "grok-4.5",
        false,
        OAuthDeviceFlow
    ),
    provider!(
        "taichu",
        "Taichu",
        Openai,
        "https://cloud.zidongtaichu.com/maas/v1",
        "taichu_llm",
        false
    ),
    provider!(
        "tencentcloud",
        "TencentCloud",
        Openai,
        "https://api.lkeap.cloud.tencent.com/v1",
        "deepseek-v3",
        true
    ),
    provider!(
        "togetherai",
        "Together AI",
        Openai,
        "https://api.together.xyz/v1",
        "meta-llama/Llama-Vision-Free",
        true
    ),
    provider!(
        "upstage",
        "Upstage",
        Openai,
        "https://api.upstage.ai/v1/solar",
        "solar-1-mini-chat",
        false
    ),
    provider!(
        "v0",
        "Vercel (v0)",
        Openai,
        "https://api.v0.dev/v1",
        "v0-1.5-md",
        false
    ),
    provider!(
        "vercelaigateway",
        "Vercel AI Gateway",
        Openai,
        "https://ai-gateway.vercel.sh/v1",
        "openai/gpt-5-nano",
        true
    ),
    provider!(
        "vertexai",
        "Vertex AI",
        Google,
        "",
        "gemini-3-flash-preview",
        false,
        ["region"]
    ),
    provider!(
        "vllm",
        "vLLM",
        Openai,
        "http://localhost:8000/v1",
        "",
        true,
        None
    ),
    provider!(
        "volcengine",
        "Volcengine",
        Openai,
        "https://ark.cn-beijing.volces.com/api/v3",
        "doubao-seed-1.8",
        false
    ),
    provider!(
        "volcenginecodingplan",
        "Volcengine Coding Plan",
        Openai,
        "https://ark.cn-beijing.volces.com/api/coding/v3",
        "doubao-seed-code",
        false
    ),
    provider!(
        "wenxin",
        "Wenxin",
        Openai,
        "https://qianfan.baidubce.com/v2",
        "ernie-4.5-turbo-latest",
        false
    ),
    provider!(
        "xai",
        "xAI",
        Openai,
        "https://api.x.ai/v1",
        "grok-4.3",
        true
    ),
    provider!(
        "xiaomimimo",
        "Xiaomi MiMo",
        Openai,
        "https://api.xiaomimimo.com/v1",
        "mimo-v2.5",
        true
    ),
    provider!(
        "xinference",
        "Xinference",
        Openai,
        "http://localhost:9997/v1",
        "",
        false,
        None
    ),
    provider!(
        "zenmux",
        "ZenMux",
        Router,
        "https://zenmux.ai/api/v1",
        "openai/gpt-5-nano",
        true
    ),
    provider!(
        "zeroone",
        "01.AI",
        Openai,
        "https://api.lingyiwanwu.com/v1",
        "yi-lightning",
        true
    ),
    provider!(
        "zhipu",
        "ZhiPu",
        Openai,
        "https://open.bigmodel.cn/api/paas/v4",
        "glm-4.5-flash",
        true
    ),
];

/// Sponsored provider ids. These listings are part of paid partnerships and
/// must survive LobeHub model syncing; scripts/axolotl/sync-lobehub-models.mjs
/// refuses to sync the provider catalog when any of them is missing.
pub(super) const SPONSORED_PROVIDERS: &[&str] = &["codeflow"];

#[derive(Debug, Clone, Deserialize)]
pub(super) struct BuiltinModel {
    pub(super) id: String,
    pub(super) name: String,
    pub(super) enabled: bool,
}

#[derive(Debug, Deserialize)]
pub(super) struct BuiltinModelCatalog {
    pub(super) providers: HashMap<String, Vec<BuiltinModel>>,
}

pub(super) static BUILTIN_MODEL_CATALOG: LazyLock<BuiltinModelCatalog> =
    LazyLock::new(|| {
        serde_json::from_str(include_str!("../lobehub_text_models.json"))
            .expect("bundled LobeHub text model catalog should be valid")
    });

pub(super) fn provider_definition(
    provider_id: &str,
) -> crate::Result<&'static AiProviderDefinition> {
    PROVIDERS
        .iter()
        .find(|provider| provider.id == provider_id)
        .ok_or_else(|| {
            input_error(format!("Unknown AI provider: {provider_id}"))
        })
}

pub fn catalog() -> Vec<AiProviderDefinition> {
    PROVIDERS
        .iter()
        .map(|definition| {
            let mut definition = *definition;
            if SPONSORED_PROVIDERS.contains(&definition.id) {
                definition.sponsored = true;
            }
            definition
        })
        .collect()
}

pub(super) fn builtin_models(provider_id: &str) -> &[BuiltinModel] {
    BUILTIN_MODEL_CATALOG
        .providers
        .get(provider_id)
        .map(Vec::as_slice)
        .unwrap_or_default()
}
