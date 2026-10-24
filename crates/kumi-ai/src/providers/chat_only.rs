//! Kumi's one way with a model that can't use tools, cloud or local: it stays selectable, its requests go without
//! tools, the model hears what it can't do, and the producer hears it once. A model learns to be one when a call is
//! turned down for its tools: from then on, for the session, it's sent none.
use crate::{
    binding::{LanguageModel, ModelBinding, ModelRequest},
    error::LanguageModelError,
    types::{CallOptions, StreamParts},
};
use async_trait::async_trait;
use kumi_types::{
    daw::DawProfile,
    errors::{FailureKind, KumiError},
};
use std::{
    cell::{Cell, RefCell},
    collections::HashSet,
    rc::Rc,
    sync::LazyLock,
};

kumi_types::daw_text!(
    #[allow(dead_code)]
    NO_TOOLS, no_tools:
    "This model can't use tools here: you see the ", place, " only as it's described above, and can't read more of it or change anything. When asked for a change, say so plainly, and that /model chooses a model that can make it."
);
/// What the producer hears of a model that can't use tools.
pub fn talk_only(name: &str, profile: &DawProfile, instead: &str) -> String {
    format!("{name} can't use tools, so Kumi can talk with it about your {} but can't change anything. {instead}", profile.words.place)
}
/// Whether a provider's or server's words say the model can't take tools.
pub fn refuses_tools(said: &str) -> bool {
    static TOOLS: LazyLock<regex::Regex> = LazyLock::new(|| {
        regex::Regex::new(r"(?i)does not support tools|doesn't support tools|tools? (?:is |are )?not supported|(?:tool|function) (?:use|calling) is not (?:supported|enabled)|no endpoints found that support tool use").unwrap()
    });
    TOOLS.is_match(said)
}
/// `call` without its tools, its instructions telling the model it can't use any.
pub fn without_tools(mut call: CallOptions, profile: &DawProfile) -> CallOptions {
    if let Some(crate::types::Message::System { content, .. }) = call.prompt.first_mut() {
        content.push_str("\n\n");
        content.push_str(&no_tools(profile));
    }
    call.tools = None;
    call.tool_choice = None;
    call
}

/// The models found, this session, to turn down tools (by `<provider>/<model>`).
pub type Learned = Rc<RefCell<HashSet<String>>>;

/// `binding` as Kumi uses a model that can't use tools: `known` (its list says so) or learned this session (a call
/// turned down for its tools, which `learned` keeps). Its requests then go without tools; a refusal says so in words.
pub fn chat_only(binding: ModelBinding, known: bool, learned: Learned, profile: DawProfile, name: String) -> ModelBinding {
    let id = binding.id.clone();
    let profile = Rc::new(Cell::new(profile));
    let prepare = binding.prepare;
    let model = Rc::new(Watched { model: binding.model, id: id.clone(), learned: learned.clone(), profile: profile.clone(), name });
    ModelBinding {
        id: id.clone(),
        model,
        // Before each format places the instructions (Responses keeps them apart from the messages).
        prepare: Box::new(move |mut request: ModelRequest| {
            if known || learned.borrow().contains(&id) {
                request.instructions.push_str("\n\n");
                request.instructions.push_str(&no_tools(&profile.get()));
                request.tools.clear();
            }
            prepare(request)
        }),
        budget: binding.budget,
    }
}
struct Watched {
    model: Rc<dyn LanguageModel>,
    id: String,
    learned: Learned,
    profile: Rc<Cell<DawProfile>>,
    name: String,
}
#[async_trait(?Send)]
impl LanguageModel for Watched {
    async fn do_stream(&self, call: CallOptions) -> Result<StreamParts, LanguageModelError> {
        let had_tools = call.tools.as_ref().is_some_and(|tools| !tools.is_empty());
        match self.model.do_stream(call).await {
            Err(error) if had_tools && refuses_tools(&said(&error)) => {
                self.learned.borrow_mut().insert(self.id.clone());
                let provider = self.id.split('/').next().unwrap_or(&self.id);
                Err(KumiError::with_provider(
                    FailureKind::Model,
                    talk_only(
                        &self.name,
                        &self.profile.get(),
                        "Send your message again to talk with it, or choose one that can with /model.",
                    ),
                    provider,
                )
                .into())
            }
            other => other,
        }
    }
}
/// What a failed call says: the provider's answer, as far as Kumi read it.
fn said(error: &LanguageModelError) -> String {
    match error {
        LanguageModelError::ApiCall(call) => format!("{} {}", call.message, call.response_body.as_deref().unwrap_or("")),
        other => other.to_string(),
    }
}
