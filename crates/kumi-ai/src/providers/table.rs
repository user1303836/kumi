//! The model providers Kumi signs in to, one row each: who they are, how a producer signs in, where their models are
//! listed and how a request to them is written. Saved settings, credentials and model IDs name a provider by its ID,
//! so a row's ID, credential and key variables never change once they've shipped.
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use std::{cell::Cell, fmt};

/// How a producer signs in to a provider.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum SignIn {
    /// A ChatGPT plan, in the browser.
    Chatgpt,
    /// An API key, pasted or from the environment.
    ApiKey,
    // Kinds not built yet take their place here: AWS credentials, Google's application default credentials, an Azure
    // key with its resource. Each needs a credential the last release still reads in auth.json, which refuses the
    // whole file over a `type` it doesn't know.
}

/// How a request to a model is written.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Format {
    /// OpenAI's Responses API.
    #[serde(rename = "responses")]
    Responses,
    /// Anthropic's Messages API.
    #[serde(rename = "anthropic")]
    Messages,
    /// Chat Completions, which most providers take.
    #[serde(rename = "chat-completions")]
    ChatCompletions,
}

/// A provider's own exception to its request format for some of its models, by the start of their names.
#[derive(Debug, Clone, Copy)]
pub struct FormatRule {
    pub prefix: &'static str,
    /// The format, or why Kumi can't send these models a request (and leaves them out of the list).
    pub format: Result<Format, &'static str>,
}

/// How a provider's list of models reads.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Listing {
    /// ChatGPT's: names, effort levels and the default, the faster tier and which models it hides, in its order.
    Chatgpt,
    /// Anthropic's: names and effort levels.
    Anthropic,
    /// OpenAI's: every model by ID, of which the chat models count, newest first.
    OpenaiChat,
    /// Every model by ID (`data[].id`), in the provider's order.
    Ids,
    /// OpenRouter's: what each model takes and does (`supported_parameters`, `reasoning`, modalities, limits, prices).
    OpenRouter,
    /// Venice's: each model's `model_spec` (capabilities, effort levels, limits, prices, privacy).
    Venice,
    /// Gemini's OpenAI-compatible list: IDs as `models/<model>`.
    Gemini,
    /// Hugging Face's router: each model with the providers that serve it and whether they take tools.
    HuggingFace,
}

/// How a key goes with a request.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum KeyHeader {
    /// `Authorization: Bearer <key>`.
    Bearer,
    /// Anthropic's `x-api-key`, with the API version its list needs.
    Anthropic,
}

/// How a key is checked before Kumi saves it: an answer that says it's refused, as opposed to one that says nothing.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum KeyCheck {
    /// Listing the provider's models with it.
    List,
    /// A GET of this path under the base URL, or of this address, which needs the key (for a provider that lists its
    /// models to anyone).
    Get(&'static str),
    /// An empty request to this path, for a provider with no such GET: it turns a key down (401, 403) before it reads
    /// the request, and an empty one down (400, 422) after. It costs nothing.
    Empty(&'static str),
}
/// How a Chat Completions request carries the effort level.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ChatEffort {
    /// `reasoning_effort: "<level>"`.
    ReasoningEffort,
    /// `reasoning: {"effort": "<level>"}`.
    Reasoning,
}

/// What a provider's requests carry beyond the format's own.
#[derive(Debug, Clone, Copy, Default)]
pub struct Extras {
    /// The header that carries the conversation's ID, for the provider's prompt cache and routing.
    pub session_header: Option<&'static str>,
    /// The body field that carries the conversation's ID on Chat Completions (a prompt cache's key, sticky routing).
    pub session_field: Option<&'static str>,
    /// How wordy the answers are (Responses' `text.verbosity`).
    pub verbosity: Option<&'static str>,
    /// Asks for the faster tier the model's list names, while `/fast` is on.
    pub service_tier: bool,
    /// How its Chat Completions requests carry the effort level; none: they carry none, so a model there offers none.
    pub chat_effort: Option<ChatEffort>,
    /// Headers every request carries (who's asking, as the provider wants it said).
    pub headers: &'static [(&'static str, &'static str)],
    /// Anthropic's models through it (`anthropic/…`) get Anthropic's prompt cache, asked for in the body.
    pub anthropic_cache: bool,
    /// Chat Completions streams ask for the usage at their end (`stream_options`); false for an API that refuses any
    /// field it doesn't know.
    pub stream_usage: bool,
}
/// No extras.
pub const NO_EXTRAS: Extras = Extras {
    session_header: None,
    session_field: None,
    verbosity: None,
    service_tier: false,
    chat_effort: None,
    headers: &[],
    anthropic_cache: false,
    stream_usage: true,
};

/// One provider.
#[derive(Debug, Clone, Copy)]
pub struct ProviderRow {
    /// What settings, credentials and model IDs (`<id>/<model>`) name it by.
    pub id: &'static str,
    /// As /login and the picker list it ("OpenAI API").
    pub name: &'static str,
    /// As a sentence names it ("OpenAI's plan limit is reached").
    pub short_name: &'static str,
    pub sign_in: SignIn,
    /// The entry in auth.json its sign-in is kept under; providers one key opens share it.
    pub credential: &'static str,
    /// Every provider that shares the credential, as a sentence names them ("OpenCode Zen and Go").
    pub shared_by: Option<&'static str>,
    /// The environment variables its key may be in, the first one set used (a saved key comes before any).
    pub key_env: &'static [&'static str],
    /// Where a producer makes a key.
    pub key_page: Option<&'static str>,
    /// Where its API is; requests and its list go under it.
    pub base_url: &'static str,
    /// How a request to its models is written, unless a rule says otherwise for a model.
    pub format: Format,
    pub format_rules: &'static [FormatRule],
    pub key_header: KeyHeader,
    /// Its list of models: the path under `base_url`, and how the list reads.
    pub list: (&'static str, Listing),
    pub key_check: KeyCheck,
    /// Its provider in models.dev's catalog.
    pub catalog: Option<&'static str>,
    pub extras: Extras,
    /// Its place among the providers /login and the picker offer, the lowest first.
    pub offer: u8,
    /// Its name where Kumi first asks how to sign in (`kumi login` alone, Kumi's first start); None leaves it to /login.
    pub first_choice: Option<&'static str>,
    /// Custom servers' IDs never take this one: the five providers Kumi had before servers were named. A provider added
    /// later is hidden, for a producer, behind a custom server that already has its ID.
    pub reserved: bool,
}

/// What most providers' rows are, the rest of a row saying who the provider is: an API key sent as a Bearer token,
/// Chat Completions, every model listed at `/models` (which also checks a key), offered after the rest, and not one of
/// the first five.
pub const API_KEY_ROW: ProviderRow = ProviderRow {
    id: "",
    name: "",
    short_name: "",
    sign_in: SignIn::ApiKey,
    credential: "",
    shared_by: None,
    key_env: &[],
    key_page: None,
    base_url: "",
    format: Format::ChatCompletions,
    format_rules: &[],
    key_header: KeyHeader::Bearer,
    list: ("/models", Listing::Ids),
    key_check: KeyCheck::List,
    catalog: None,
    extras: NO_EXTRAS,
    offer: u8::MAX,
    first_choice: None,
    reserved: false,
};
/// Who's asking, as OpenRouter lists apps in its rankings (its app attribution).
const OPENROUTER_HEADERS: [(&str, &str); 2] = [("http-referer", "https://github.com/user1303836/kumi"), ("x-openrouter-title", "Kumi")];
const RESPONSES_ON_ZEN: [FormatRule; 5] = [
    FormatRule { prefix: "gpt-", format: Ok(Format::Responses) },
    FormatRule { prefix: "grok-", format: Ok(Format::Responses) },
    FormatRule { prefix: "muse-", format: Ok(Format::Responses) },
    FormatRule { prefix: "claude-", format: Ok(Format::Messages) },
    FormatRule { prefix: "gemini-", format: Err("Gemini models through OpenCode are not supported yet.") },
];
const ZEN_EXTRAS: Extras = Extras { session_header: Some("x-opencode-session"), ..NO_EXTRAS };

/// Every provider, in the order its ID was added.
const TABLE: [ProviderRow; 26] = [
    ProviderRow {
        id: "openai-codex",
        name: "ChatGPT",
        short_name: "ChatGPT",
        sign_in: SignIn::Chatgpt,
        credential: "openai-codex",
        shared_by: None,
        key_env: &[],
        key_page: None,
        base_url: "https://chatgpt.com/backend-api/codex",
        format: Format::Responses,
        format_rules: &[],
        key_header: KeyHeader::Bearer,
        list: ("/models?client_version=1.0.0", Listing::Chatgpt),
        key_check: KeyCheck::List,
        catalog: Some("openai"),
        extras: Extras { session_header: Some("session-id"), verbosity: Some("low"), service_tier: true, ..NO_EXTRAS },
        offer: 1,
        first_choice: Some("ChatGPT"),
        reserved: true,
    },
    ProviderRow {
        id: "openai",
        name: "OpenAI API",
        short_name: "OpenAI",
        sign_in: SignIn::ApiKey,
        credential: "openai",
        shared_by: None,
        key_env: &["OPENAI_API_KEY"],
        key_page: Some("platform.openai.com/api-keys"),
        base_url: "https://api.openai.com/v1",
        format: Format::Responses,
        format_rules: &[],
        key_header: KeyHeader::Bearer,
        list: ("/models", Listing::OpenaiChat),
        key_check: KeyCheck::List,
        catalog: Some("openai"),
        extras: NO_EXTRAS,
        offer: 3,
        first_choice: Some("OpenAI"),
        reserved: true,
    },
    ProviderRow {
        id: "anthropic",
        name: "Anthropic",
        short_name: "Anthropic",
        sign_in: SignIn::ApiKey,
        credential: "anthropic",
        shared_by: None,
        key_env: &["ANTHROPIC_API_KEY"],
        key_page: Some("platform.claude.com (API keys)"),
        base_url: "https://api.anthropic.com/v1",
        format: Format::Messages,
        format_rules: &[],
        key_header: KeyHeader::Anthropic,
        list: ("/models?limit=100", Listing::Anthropic),
        key_check: KeyCheck::List,
        catalog: Some("anthropic"),
        extras: NO_EXTRAS,
        offer: 2,
        first_choice: Some("Anthropic"),
        reserved: true,
    },
    ProviderRow {
        id: "opencode",
        name: "OpenCode Zen",
        short_name: "OpenCode Zen",
        sign_in: SignIn::ApiKey,
        credential: "opencode",
        shared_by: Some("OpenCode Zen and Go"),
        key_env: &["OPENCODE_API_KEY"],
        key_page: Some("opencode.ai/auth"),
        base_url: "https://opencode.ai/zen/v1",
        format: Format::ChatCompletions,
        format_rules: &RESPONSES_ON_ZEN,
        key_header: KeyHeader::Bearer,
        list: ("/models", Listing::Ids),
        key_check: KeyCheck::List,
        catalog: Some("opencode"),
        extras: ZEN_EXTRAS,
        offer: 4,
        first_choice: Some("OpenCode"),
        reserved: true,
    },
    ProviderRow {
        id: "opencode-go",
        name: "OpenCode Go",
        short_name: "OpenCode Go",
        sign_in: SignIn::ApiKey,
        credential: "opencode",
        shared_by: Some("OpenCode Zen and Go"),
        key_env: &["OPENCODE_API_KEY"],
        key_page: Some("opencode.ai/auth"),
        base_url: "https://opencode.ai/zen/go/v1",
        format: Format::ChatCompletions,
        format_rules: &RESPONSES_ON_ZEN,
        key_header: KeyHeader::Bearer,
        list: ("/models", Listing::Ids),
        key_check: KeyCheck::List,
        catalog: Some("opencode-go"),
        extras: ZEN_EXTRAS,
        offer: 5,
        first_choice: None,
        reserved: true,
    },
    ProviderRow {
        id: "openrouter",
        name: "OpenRouter",
        short_name: "OpenRouter",
        credential: "openrouter",
        key_env: &["OPENROUTER_API_KEY"],
        key_page: Some("openrouter.ai/settings/keys"),
        base_url: "https://openrouter.ai/api/v1",
        list: ("/models", Listing::OpenRouter),
        key_check: KeyCheck::Get("/key"),
        catalog: Some("openrouter"),
        extras: Extras {
            session_field: Some("session_id"),
            chat_effort: Some(ChatEffort::Reasoning),
            headers: &OPENROUTER_HEADERS,
            anthropic_cache: true,
            ..NO_EXTRAS
        },
        offer: 6,
        ..API_KEY_ROW
    },
    ProviderRow {
        id: "venice",
        name: "Venice",
        short_name: "Venice",
        credential: "venice",
        key_env: &["VENICE_API_KEY"],
        key_page: Some("venice.ai/settings/api"),
        base_url: "https://api.venice.ai/api/v1",
        list: ("/models?type=text", Listing::Venice),
        key_check: KeyCheck::Get("/api_keys/rate_limits"),
        catalog: Some("venice"),
        extras: Extras { session_field: Some("prompt_cache_key"), chat_effort: Some(ChatEffort::ReasoningEffort), ..NO_EXTRAS },
        offer: 7,
        ..API_KEY_ROW
    },
    // One key for chat and listening (`gemini_key`): the saved one, then the environment's.
    ProviderRow {
        id: "gemini",
        name: "Google Gemini",
        short_name: "Gemini",
        credential: "gemini",
        key_env: &["GEMINI_API_KEY", "GOOGLE_API_KEY"],
        key_page: Some("aistudio.google.com/apikey"),
        base_url: "https://generativelanguage.googleapis.com/v1beta/openai",
        list: ("/models", Listing::Gemini),
        catalog: Some("google"),
        extras: Extras { chat_effort: Some(ChatEffort::ReasoningEffort), ..NO_EXTRAS },
        offer: 8,
        ..API_KEY_ROW
    },
    ProviderRow {
        id: "deepseek",
        name: "DeepSeek",
        short_name: "DeepSeek",
        credential: "deepseek",
        key_env: &["DEEPSEEK_API_KEY"],
        key_page: Some("platform.deepseek.com/api_keys"),
        base_url: "https://api.deepseek.com",
        catalog: Some("deepseek"),
        extras: Extras { chat_effort: Some(ChatEffort::ReasoningEffort), ..NO_EXTRAS },
        offer: 9,
        ..API_KEY_ROW
    },
    // Responses, which xAI recommends over its Chat Completions; `prompt_cache_key` keeps a conversation on its cache.
    ProviderRow {
        id: "xai",
        name: "xAI",
        short_name: "xAI",
        credential: "xai",
        key_env: &["XAI_API_KEY"],
        key_page: Some("console.x.ai"),
        base_url: "https://api.x.ai/v1",
        format: Format::Responses,
        catalog: Some("xai"),
        offer: 10,
        ..API_KEY_ROW
    },
    ProviderRow {
        id: "zai",
        name: "Z.ai (GLM)",
        short_name: "Z.ai",
        credential: "zai",
        key_env: &["ZAI_API_KEY", "ZHIPU_API_KEY"],
        key_page: Some("z.ai/manage-apikey/apikey-list"),
        base_url: "https://api.z.ai/api/paas/v4",
        catalog: Some("zai"),
        extras: Extras { chat_effort: Some(ChatEffort::ReasoningEffort), ..NO_EXTRAS },
        offer: 11,
        ..API_KEY_ROW
    },
    ProviderRow {
        id: "moonshot",
        name: "Moonshot (Kimi)",
        short_name: "Moonshot",
        credential: "moonshot",
        key_env: &["MOONSHOT_API_KEY"],
        key_page: Some("platform.kimi.ai/console/api-keys"),
        base_url: "https://api.moonshot.ai/v1",
        catalog: Some("moonshotai"),
        extras: Extras { chat_effort: Some(ChatEffort::ReasoningEffort), ..NO_EXTRAS },
        offer: 12,
        ..API_KEY_ROW
    },
    ProviderRow {
        id: "alibaba",
        name: "Alibaba Cloud (Qwen)",
        short_name: "Alibaba Cloud",
        credential: "alibaba",
        key_env: &["DASHSCOPE_API_KEY"],
        key_page: Some("modelstudio.console.alibabacloud.com (API keys)"),
        base_url: "https://dashscope-intl.aliyuncs.com/compatible-mode/v1",
        catalog: Some("alibaba"),
        offer: 13,
        ..API_KEY_ROW
    },
    // Anthropic's format, which MiniMax recommends (thinking between tool calls); a Bearer key.
    ProviderRow {
        id: "minimax",
        name: "MiniMax",
        short_name: "MiniMax",
        credential: "minimax",
        key_env: &["MINIMAX_API_KEY"],
        key_page: Some("platform.minimax.io/user-center/basic-information/interface-key"),
        base_url: "https://api.minimax.io/anthropic/v1",
        format: Format::Messages,
        list: ("/models", Listing::Anthropic),
        catalog: Some("minimax"),
        offer: 14,
        ..API_KEY_ROW
    },
    ProviderRow {
        id: "xiaomi",
        name: "Xiaomi MiMo",
        short_name: "Xiaomi MiMo",
        credential: "xiaomi",
        key_env: &["MIMO_API_KEY", "XIAOMI_API_KEY"],
        key_page: Some("platform.xiaomimimo.com (API keys)"),
        base_url: "https://api.xiaomimimo.com/v1",
        catalog: Some("xiaomi"),
        offer: 15,
        ..API_KEY_ROW
    },
    ProviderRow {
        id: "stepfun",
        name: "StepFun",
        short_name: "StepFun",
        credential: "stepfun",
        key_env: &["STEPFUN_API_KEY", "STEP_API_KEY"],
        key_page: Some("platform.stepfun.ai/interface-key"),
        base_url: "https://api.stepfun.ai/v1",
        catalog: Some("stepfun-ai"),
        extras: Extras { chat_effort: Some(ChatEffort::ReasoningEffort), ..NO_EXTRAS },
        offer: 16,
        ..API_KEY_ROW
    },
    // Tencent Cloud International's TokenHub (pay-as-you-go, Singapore).
    ProviderRow {
        id: "tencent",
        name: "Tencent TokenHub",
        short_name: "Tencent TokenHub",
        credential: "tencent",
        key_env: &["TENCENT_TOKENHUB_API_KEY"],
        key_page: Some("console.tencentcloud.com/tokenhub/apikey"),
        base_url: "https://tokenhub-intl.tencentcloudmaas.com/v1",
        catalog: Some("tencent-tokenhub"),
        offer: 17,
        ..API_KEY_ROW
    },
    ProviderRow {
        id: "groq",
        name: "Groq",
        short_name: "Groq",
        credential: "groq",
        key_env: &["GROQ_API_KEY"],
        key_page: Some("console.groq.com/keys"),
        base_url: "https://api.groq.com/openai/v1",
        catalog: Some("groq"),
        extras: Extras { chat_effort: Some(ChatEffort::ReasoningEffort), ..NO_EXTRAS },
        offer: 18,
        ..API_KEY_ROW
    },
    ProviderRow {
        id: "cerebras",
        name: "Cerebras",
        short_name: "Cerebras",
        credential: "cerebras",
        key_env: &["CEREBRAS_API_KEY"],
        key_page: Some("cloud.cerebras.ai (API keys)"),
        base_url: "https://api.cerebras.ai/v1",
        catalog: Some("cerebras"),
        extras: Extras { session_field: Some("prompt_cache_key"), chat_effort: Some(ChatEffort::ReasoningEffort), ..NO_EXTRAS },
        offer: 19,
        ..API_KEY_ROW
    },
    ProviderRow {
        id: "fireworks",
        name: "Fireworks AI",
        short_name: "Fireworks AI",
        credential: "fireworks",
        key_env: &["FIREWORKS_API_KEY"],
        key_page: Some("app.fireworks.ai/settings/users/api-keys"),
        base_url: "https://api.fireworks.ai/inference/v1",
        catalog: Some("fireworks-ai"),
        extras: Extras { session_field: Some("prompt_cache_key"), chat_effort: Some(ChatEffort::ReasoningEffort), ..NO_EXTRAS },
        offer: 20,
        ..API_KEY_ROW
    },
    ProviderRow {
        id: "together",
        name: "Together AI",
        short_name: "Together AI",
        credential: "together",
        key_env: &["TOGETHER_API_KEY"],
        key_page: Some("api.together.ai/settings/api-keys"),
        base_url: "https://api.together.ai/v1",
        catalog: Some("togetherai"),
        extras: Extras { session_field: Some("prompt_cache_key"), chat_effort: Some(ChatEffort::ReasoningEffort), ..NO_EXTRAS },
        offer: 21,
        ..API_KEY_ROW
    },
    ProviderRow {
        id: "deepinfra",
        name: "DeepInfra",
        short_name: "DeepInfra",
        credential: "deepinfra",
        key_env: &["DEEPINFRA_API_KEY"],
        key_page: Some("deepinfra.com/dash/api_keys"),
        base_url: "https://api.deepinfra.com/v1/openai",
        key_check: KeyCheck::Get("https://api.deepinfra.com/v1/me"),
        catalog: Some("deepinfra"),
        extras: Extras { session_field: Some("prompt_cache_key"), chat_effort: Some(ChatEffort::ReasoningEffort), ..NO_EXTRAS },
        offer: 22,
        ..API_KEY_ROW
    },
    // Its API turns down a field it doesn't know (`stream_options`), and thinks in chunks Kumi doesn't read yet: no effort.
    ProviderRow {
        id: "mistral",
        name: "Mistral",
        short_name: "Mistral",
        credential: "mistral",
        key_env: &["MISTRAL_API_KEY"],
        key_page: Some("console.mistral.ai/api-keys"),
        base_url: "https://api.mistral.ai/v1",
        catalog: Some("mistral"),
        extras: Extras { session_field: Some("prompt_cache_key"), stream_usage: false, ..NO_EXTRAS },
        offer: 23,
        ..API_KEY_ROW
    },
    // Ollama's cloud models, apart from the Ollama on this computer (`ollama`).
    ProviderRow {
        id: "ollama-cloud",
        name: "Ollama Cloud",
        short_name: "Ollama Cloud",
        credential: "ollama-cloud",
        key_env: &["OLLAMA_API_KEY"],
        key_page: Some("ollama.com/settings/keys"),
        base_url: "https://ollama.com/v1",
        key_check: KeyCheck::Get("https://ollama.com/api/balance"),
        catalog: Some("ollama-cloud"),
        extras: Extras { chat_effort: Some(ChatEffort::ReasoningEffort), ..NO_EXTRAS },
        offer: 24,
        ..API_KEY_ROW
    },
    // No key check of its own (its list answers anyone): an empty request. Some models' schemas take no unknown field.
    ProviderRow {
        id: "nvidia",
        name: "NVIDIA",
        short_name: "NVIDIA",
        credential: "nvidia",
        key_env: &["NVIDIA_API_KEY"],
        key_page: Some("build.nvidia.com (API keys)"),
        base_url: "https://integrate.api.nvidia.com/v1",
        key_check: KeyCheck::Empty("/chat/completions"),
        catalog: Some("nvidia"),
        extras: Extras { stream_usage: false, ..NO_EXTRAS },
        offer: 25,
        ..API_KEY_ROW
    },
    ProviderRow {
        id: "huggingface",
        name: "Hugging Face",
        short_name: "Hugging Face",
        credential: "huggingface",
        key_env: &["HF_TOKEN"],
        key_page: Some("huggingface.co/settings/tokens"),
        base_url: "https://router.huggingface.co/v1",
        list: ("/models", Listing::HuggingFace),
        key_check: KeyCheck::Get("https://huggingface.co/api/whoami-v2"),
        catalog: Some("huggingface"),
        offer: 26,
        ..API_KEY_ROW
    },
];

thread_local! {
    /// Rows a test adds on its thread, as providers added after the first five (`add_test_rows`).
    static ADDED: Cell<&'static [ProviderRow]> = const { Cell::new(&[]) };
}
/// Tests only: `rows` join the table on this thread, as providers added later would.
#[doc(hidden)]
pub fn add_test_rows(rows: &'static [ProviderRow]) {
    ADDED.set(rows);
}
/// Every provider, in the order its ID was added.
pub fn rows() -> impl Iterator<Item = &'static ProviderRow> {
    TABLE.iter().chain(ADDED.get())
}
/// Every provider, in the order /login and the picker offer them.
pub fn offered() -> Vec<ProviderId> {
    let mut offered: Vec<_> = rows().collect();
    offered.sort_by_key(|row| row.offer);
    offered.into_iter().map(|row| ProviderId(row.id)).collect()
}
/// The providers Kumi first offers to sign in to (`kumi login` alone, Kumi's first start), with their names there.
pub fn first_choices() -> Vec<(ProviderId, &'static str)> {
    offered().into_iter().filter_map(|id| Some((id, id.row().first_choice?))).collect()
}

impl ProviderRow {
    /// How a request to `model` is written, or why Kumi can't send it one.
    pub fn format_for(&self, model: &str) -> Result<Format, &'static str> {
        self.format_rules.iter().find(|rule| model.starts_with(rule.prefix)).map_or(Ok(self.format), |rule| rule.format)
    }
    /// The first environment variable its key may be in.
    pub fn first_key_env(&self) -> Option<&'static str> {
        self.key_env.first().copied()
    }
}

/// A provider in the table, by its ID: settings, credentials and model IDs name it so.
#[derive(Clone, Copy, PartialEq, Eq, Hash)]
pub struct ProviderId(&'static str);
impl ProviderId {
    pub fn parse(value: &str) -> Option<Self> {
        rows().find(|row| row.id == value).map(|row| Self(row.id))
    }
    pub fn as_str(self) -> &'static str {
        self.0
    }
    pub fn row(self) -> &'static ProviderRow {
        rows().find(|row| row.id == self.0).expect("a provider's ID names its row")
    }
}
impl fmt::Display for ProviderId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.0)
    }
}
impl fmt::Debug for ProviderId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{:?}", self.0)
    }
}
impl Serialize for ProviderId {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(self.0)
    }
}
impl<'de> Deserialize<'de> for ProviderId {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let id = String::deserialize(deserializer)?;
        Self::parse(&id).ok_or_else(|| serde::de::Error::custom(format!("unknown provider {id}")))
    }
}
