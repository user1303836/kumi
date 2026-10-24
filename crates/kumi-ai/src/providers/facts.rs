//! A listed model's facts, the same way for every provider: what its provider's list says first, then what models.dev
//! says (`catalog`), then, for a model neither describes, what its name suggests, marked a guess. A model that doesn't
//! chat (it makes pictures, embeds, speaks), or that Kumi can't send a request to, isn't offered.
use super::{
    catalog::{Catalog, CatalogModel},
    models::{Cost, EffortInfo, FactsFrom, Input, ModelInfo},
    table::{Format, ProviderRow},
    Effort,
};
use kumi_common::js::string::head;

/// A model as its provider's list has it, and how much of it the list said.
pub struct Listed {
    pub info: ModelInfo,
    /// The list gave its effort levels (none, when it said it has none).
    pub efforts: bool,
    /// The list described the model beyond its name: its effort levels, tools or what it takes.
    pub described: bool,
}
impl Listed {
    /// A model the list only names.
    pub fn named(info: ModelInfo) -> Self {
        Listed { info, efforts: false, described: false }
    }
}

/// The effort levels a model's name suggests: every level for Claude (but Haiku) and GPT-6 on; low to high for GPT-5
/// and the o-series. Only for a model neither its provider nor models.dev describes.
pub fn guessed_efforts(model: &str) -> Vec<Effort> {
    static GPT: std::sync::LazyLock<regex::Regex> = std::sync::LazyLock::new(|| regex::Regex::new(r"^gpt-(\d+)").unwrap());
    let gpt = GPT.captures(model).and_then(|m| m[1].parse::<u32>().ok()).unwrap_or(0);
    let o = model.starts_with('o') && model.as_bytes().get(1).is_some_and(u8::is_ascii_digit);
    if model.starts_with("claude-haiku") {
        vec![]
    } else if model.starts_with("claude-") || gpt >= 6 {
        vec![Effort::Low, Effort::Medium, Effort::High, Effort::Xhigh, Effort::Max]
    } else if gpt >= 5 || o {
        vec![Effort::Low, Effort::Medium, Effort::High]
    } else {
        vec![]
    }
}
fn levels(efforts: impl IntoIterator<Item = Effort>) -> Vec<EffortInfo> {
    efforts.into_iter().map(|effort| EffortInfo { effort, description: None }).collect()
}
/// The levels models.dev names, in Kumi's order, each once.
fn catalog_efforts(known: &CatalogModel) -> Vec<EffortInfo> {
    let named = known.efforts().unwrap_or_default();
    levels(super::EFFORTS.into_iter().filter(|effort| named.iter().any(|name| name == effort.as_str())))
}

/// The format a provider's SDK writes requests in, by its npm name: Err for one Kumi can't send yet.
fn sdk_format(npm: &str) -> Option<Result<Format, ()>> {
    match npm {
        "@ai-sdk/openai" => Some(Ok(Format::Responses)),
        "@ai-sdk/anthropic" => Some(Ok(Format::Messages)),
        "@ai-sdk/openai-compatible" => Some(Ok(Format::ChatCompletions)),
        "@ai-sdk/google" | "@ai-sdk/google-vertex" => Some(Err(())),
        _ => None,
    }
}
/// How a request to `model` is written: as models.dev routes it (a model of its own API on a gateway such as Zen),
/// the provider's way for a model it knows otherwise, and by the provider's rules for its names (a guess) for one it
/// doesn't. Err says why Kumi can't send it one.
pub fn format_for(row: &ProviderRow, model: &str, catalog: &Catalog) -> Result<Format, String> {
    let Some(known) = row.catalog.and_then(|provider| catalog.model(provider, model)) else {
        return row.format_for(model).map_err(str::to_string);
    };
    match known.provider.as_ref().and_then(|route| route.npm.as_deref()).and_then(sdk_format) {
        Some(Ok(format)) => Ok(format),
        Some(Err(())) => {
            Err(format!("{model} takes Google's own request format, which Kumi can't send yet: choose another model with /model."))
        }
        None => Ok(row.format),
    }
}

/// `listed` with what models.dev says and, failing both, what its name suggests; None when it doesn't chat or Kumi
/// can't send it a request.
pub fn with_facts(row: &ProviderRow, listed: Listed, catalog: &Catalog) -> Option<ModelInfo> {
    let Listed { mut info, efforts, described } = listed;
    let known = row.catalog.and_then(|provider| catalog.model(provider, &info.model));
    if known.is_some_and(|known| !known.chats()) {
        return None;
    }
    let format = format_for(row, &info.model, catalog).ok()?;
    info.format = (format != row.format).then_some(format);
    match known {
        Some(known) => {
            if !efforts {
                info.efforts = catalog_efforts(known);
            }
            if info.name == info.model {
                if let Some(name) = known.name.as_deref().map(str::trim).filter(|name| !name.is_empty()) {
                    info.name = head(name, 60);
                }
            }
            info.tools = info.tools.or(known.tool_call);
            info.reasoning = info.reasoning.or(known.reasoning);
            if info.inputs.is_empty() {
                info.inputs =
                    known.modalities.iter().flat_map(|modalities| &modalities.input).filter_map(|input| Input::parse(input)).collect();
            }
            info.context = info.context.or(known.limit.as_ref().and_then(|limit| limit.context));
            info.max_output = info.max_output.or(known.limit.as_ref().and_then(|limit| limit.output));
            info.cost = info.cost.or_else(|| {
                known.cost.as_ref().map(|cost| Cost {
                    input: cost.input,
                    output: cost.output,
                    cache_read: cost.cache_read,
                    cache_write: cost.cache_write,
                })
            });
            info.status = info.status.or_else(|| known.status.clone());
        }
        None if !efforts => info.efforts = levels(guessed_efforts(&info.model)),
        None => {}
    }
    // A provider whose Chat Completions requests carry no effort level: a model there offers none Kumi couldn't send.
    if format == Format::ChatCompletions && row.extras.chat_effort.is_none() {
        info.efforts.clear();
        info.default_effort = None;
    }
    info.facts_from = Some(if described {
        FactsFrom::Provider
    } else if known.is_some() {
        FactsFrom::Catalog
    } else {
        FactsFrom::Guess
    });
    Some(info)
}
