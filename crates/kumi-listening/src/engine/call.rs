//! The listening tools as the model calls them: a tool's input becomes a request, the engine answers, and the reply is
//! what the model reads.
use super::Listening;
use crate::tools::{
    audition_request, form_request, groove_request, judge_reply, judge_request, sound_request, tune_request, AUDITION_TOOL, FORM_TOOL,
    GROOVE_TOOL, JUDGE_TOOL, RENDER_TOOL, SOUND_TOOL, TUNE_TOOL,
};
use kumi_common::{abort::Signal, js::json::stringify};
use kumi_types::{
    contracts::{JsonObject, ToolResult},
    errors::RuntimeError,
};
use serde_json::{json, Value};
use std::rc::Rc;

fn object(value: Value) -> JsonObject {
    value.as_object().cloned().unwrap_or_default()
}

impl Listening {
    /// Runs one of the listening tools (judge, tune, sound, form, groove, audition, render); None for any other tool.
    pub async fn call(self: &Rc<Self>, name: &str, input: JsonObject, signal: Signal) -> Option<Result<ToolResult, RuntimeError>> {
        Some(match name {
            JUDGE_TOOL => self.judge_call(input, signal).await,
            TUNE_TOOL => self.tune_call(input, signal).await,
            SOUND_TOOL => self.sound_call(input, signal).await,
            FORM_TOOL => self.form_call(input, signal).await,
            GROOVE_TOOL => self.groove_call(input, signal).await,
            AUDITION_TOOL => self.audition_call(input, signal).await,
            RENDER_TOOL => self.render_call(input, signal).await,
            _ => return None,
        })
    }

    async fn judge_call(self: &Rc<Self>, input: JsonObject, signal: Signal) -> Result<ToolResult, RuntimeError> {
        let request = match judge_request(&input) {
            Ok(request) => request,
            Err(why) => return Ok(ToolResult::error(why)),
        };
        match self.judge(&request, signal.clone()).await? {
            Ok(mut round) => {
                self.name_plugin_roles(&mut round, signal).await;
                Ok(ToolResult::text(stringify(&judge_reply(&round))))
            }
            Err(why) => Ok(ToolResult::error(why)),
        }
    }

    async fn tune_call(self: &Rc<Self>, input: JsonObject, signal: Signal) -> Result<ToolResult, RuntimeError> {
        let request = match tune_request(&input) {
            Ok(request) => request,
            Err(why) => return Ok(ToolResult::error(why)),
        };
        match self.tune(&request, signal.clone()).await? {
            Ok(mut round) => {
                self.name_plugin_roles(&mut round, signal).await;
                Ok(ToolResult::text(stringify(&judge_reply(&round))))
            }
            Err(why) => Ok(ToolResult::error(why)),
        }
    }

    async fn sound_call(self: &Rc<Self>, input: JsonObject, signal: Signal) -> Result<ToolResult, RuntimeError> {
        let request = match sound_request(&input) {
            Ok(request) => request,
            Err(why) => return Ok(ToolResult::error(why)),
        };
        match self.sound(&request, signal).await? {
            Ok(said) => Ok(ToolResult::text(stringify(&said))),
            Err(why) => Ok(ToolResult::error(why)),
        }
    }

    async fn form_call(self: &Rc<Self>, input: JsonObject, signal: Signal) -> Result<ToolResult, RuntimeError> {
        match self.form(&form_request(&input), signal).await? {
            Ok(said) => Ok(ToolResult::text(stringify(&said))),
            Err(why) => Ok(ToolResult::error(why)),
        }
    }

    async fn groove_call(self: &Rc<Self>, input: JsonObject, signal: Signal) -> Result<ToolResult, RuntimeError> {
        let request = match groove_request(self.host.profile(), &input) {
            Ok(request) => request,
            Err(why) => return Ok(ToolResult::error(why)),
        };
        match self.groove(&request, signal).await? {
            Ok(round) => Ok(ToolResult::text(stringify(&judge_reply(&round)))),
            Err(why) => Ok(ToolResult::error(why)),
        }
    }

    async fn audition_call(self: &Rc<Self>, input: JsonObject, signal: Signal) -> Result<ToolResult, RuntimeError> {
        let request = match audition_request(self.host.profile(), &input) {
            Ok(request) => request,
            Err(why) => return Ok(ToolResult::error(why)),
        };
        let result = match self.audition(&request, signal).await? {
            Ok(result) => result,
            Err(why) => return Ok(ToolResult::error(why)),
        };
        let mut reply = object(json!({"round":self.round_count()}));
        if let Some(best) = result.best.filter(|s| !s.is_empty()) {
            reply.insert("best".into(), json!(best));
        }
        let takes: Vec<_> = result
            .takes
            .iter()
            .map(|take| {
                let mut row = object(json!({"label":take.label,"track":self.host.short_ref(&take.track)}));
                if take.silent == Some(true) {
                    row.insert("silent".into(), json!(true));
                }
                if let Some(heard) = &take.heard {
                    row.insert("heard".into(), json!(heard.summary));
                }
                if let Some(close) = &take.closeness {
                    row.insert("score".into(), json!(close.score));
                    row.insert("gaps".into(), json!(close.gaps));
                    let features: JsonObject =
                        close.features.iter().map(|f| (json!(f.name).as_str().unwrap().to_owned(), json!(f.similarity))).collect();
                    row.insert("features".into(), json!(features));
                    if let Some(structural) = &close.structural {
                        row.insert("knobsCantCloseThis".into(), json!(format!("{}: {}", structural.gap, structural.r#move)));
                    }
                }
                row
            })
            .collect();
        reply.insert("takes".into(), json!(takes));
        if let Some(reference) = result.reference {
            reply.insert("reference".into(), json!(reference.summary));
        }
        reply.insert("seconds".into(), json!(result.seconds));
        if !result.notes.is_empty() {
            reply.insert("notes".into(), json!(result.notes));
        }
        Ok(ToolResult {
            text: stringify(&json!(reply)),
            is_error: result.takes.iter().all(|take| take.silent == Some(true) || take.heard.is_none()),
            ..Default::default()
        })
    }

    async fn render_call(&self, input: JsonObject, signal: Signal) -> Result<ToolResult, RuntimeError> {
        let from = input.get("from_beat").and_then(Value::as_f64).filter(|n| n.is_finite() && *n >= 0.);
        let beats = input.get("beats").and_then(Value::as_f64).filter(|n| n.is_finite() && *n > 0.);
        let (Some(track), Some(from), Some(beats)) = (input.get("track").and_then(Value::as_str), from, beats) else {
            return Ok(ToolResult::error("Give the track (an audio track's reference from this turn), from_beat and beats."));
        };
        match self.host.render(track, from, beats, signal).await? {
            Err(why) => Ok(ToolResult::error(why)),
            Ok(mut reply) => {
                let clips = self.host.profile().vocab.clips;
                reply.insert(
                    "note".into(),
                    json!(format!(
                        "The track's own {clips}, before its devices. Hear it with listen (file), against a reference with compare_to."
                    )),
                );
                Ok(ToolResult::text(stringify(&json!(reply))))
            }
        }
    }
}
