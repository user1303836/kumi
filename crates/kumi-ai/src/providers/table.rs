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
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Format {
    /// OpenAI's Responses API.
    Responses,
    /// Anthropic's Messages API.
    Messages,
    /// Chat Completions, which most providers take.
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
}

/// What a provider's requests carry beyond the format's own.
#[derive(Debug, Clone, Copy, Default)]
pub struct Extras {
    /// The header that carries the conversation's ID, for the provider's prompt cache and routing.
    pub session_header: Option<&'static str>,
    /// How wordy the answers are (Responses' `text.verbosity`).
    pub verbosity: Option<&'static str>,
    /// Asks for the faster tier the model's list names, while `/fast` is on.
    pub service_tier: bool,
}

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
    extras: Extras { session_header: None, verbosity: None, service_tier: false },
    offer: u8::MAX,
    first_choice: None,
    reserved: false,
};
const RESPONSES_ON_ZEN: [FormatRule; 5] = [
    FormatRule { prefix: "gpt-", format: Ok(Format::Responses) },
    FormatRule { prefix: "grok-", format: Ok(Format::Responses) },
    FormatRule { prefix: "muse-", format: Ok(Format::Responses) },
    FormatRule { prefix: "claude-", format: Ok(Format::Messages) },
    FormatRule { prefix: "gemini-", format: Err("Gemini models through OpenCode are not supported yet.") },
];
const ZEN_EXTRAS: Extras = Extras { session_header: Some("x-opencode-session"), verbosity: None, service_tier: false };

/// Every provider, in the order its ID was added.
const TABLE: [ProviderRow; 5] = [
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
        extras: Extras { session_header: Some("session-id"), verbosity: Some("low"), service_tier: true },
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
        extras: Extras { session_header: None, verbosity: None, service_tier: false },
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
        extras: Extras { session_header: None, verbosity: None, service_tier: false },
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
