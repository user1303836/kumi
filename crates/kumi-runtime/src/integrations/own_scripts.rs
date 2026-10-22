//! Kept scripts through Kumi's own bridges (#401), the same for REAPER and Renoise but for their words and lint rules
//! (`OwnDaw::scripts`): whether a chunk ran in this session without an error (the change engine's record), the DAW's copy
//! installed as its action (`script_install`) or taken away (`script_remove`), and a kept script run as the DAW's run_lua
//! change: one step of its undo.
use super::own_changes::OwnPlan;
use crate::core::refs::RefScheme;
use crate::core::scripts::backend::{Installed, ScriptsBackend, Words};
use crate::core::scripts::store::Saved;
use async_trait::async_trait;
use kumi_common::abort::Signal;
use serde_json::{json, Value};
use std::rc::Rc;

pub struct OwnScripts<S: RefScheme + 'static> {
    plan: Rc<OwnPlan<S>>,
    words: &'static Words,
}

impl<S: RefScheme + 'static> OwnScripts<S> {
    pub fn new(plan: Rc<OwnPlan<S>>, words: &'static Words) -> Self {
        OwnScripts { plan, words }
    }

    async fn call(&self, method: &str, args: Value, signal: &Signal) -> Result<Value, String> {
        match self.plan.link().call(method, args, signal.clone()).await {
            Ok(Ok(answer)) => Ok(answer),
            Ok(Err(refused)) => Err(refused.why),
            Err(error) => Err(error.to_string()),
        }
    }
}

#[async_trait(?Send)]
impl<S: RefScheme + 'static> ScriptsBackend for OwnScripts<S> {
    fn words(&self) -> &'static Words {
        self.words
    }

    fn ran_cleanly(&self, code: &str) -> bool {
        self.plan.ran_cleanly(code)
    }

    async fn install(&self, saved: &Saved, replace: bool, signal: &Signal) -> Result<Installed, String> {
        let args =
            json!({"slug": saved.slug, "name": saved.name, "description": saved.description, "code": saved.code, "replace": replace});
        let answer = self.call("script_install", args, signal).await?;
        let text = |key: &str| answer.get(key).and_then(Value::as_str).map(str::to_owned);
        Ok(Installed {
            action: text("where").unwrap_or_else(|| format!("kept in {} as {}", self.words.app, saved.slug)),
            file: text("file").unwrap_or_default(),
        })
    }

    async fn uninstall(&self, slug: &str, signal: &Signal) -> Result<(), String> {
        match self.plan.link().call("script_remove", json!({"slug": slug}), signal.clone()).await {
            Ok(Ok(_)) => Ok(()),
            // (taken away in the DAW already: nothing left to take)
            Ok(Err(refused)) if refused.code.as_deref() == Some("gone") => Ok(()),
            Ok(Err(refused)) => Err(refused.why),
            Err(error) => Err(error.to_string()),
        }
    }

    async fn run(&self, code: &str, signal: &Signal) -> Result<Value, String> {
        let input = json!({"code": code});
        let outcome = self.plan.change_by("run_lua", input.as_object().cloned().unwrap_or_default(), signal.clone()).await;
        if outcome.is_error || outcome.stops {
            return Err(outcome.text);
        }
        Ok(serde_json::from_str(&outcome.text).unwrap_or_else(|_| json!({"said": outcome.text})))
    }
}
