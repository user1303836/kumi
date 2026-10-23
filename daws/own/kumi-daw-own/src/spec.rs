//! Delivery to a spec through Kumi's own bridges (#401), the same for REAPER and Renoise but for their facts (`OwnDaw::spec`):
//! the mix rendered offline from the master (the whole song, or a stretch), the master's fader read from its track row and
//! set by the DAW's track_volume change.
use super::own_changes::OwnPlan;
use super::own_listening::OwnRenderer;
use crate::core::contracts::Render;
use crate::core::refs::RefScheme;
use crate::core::spec::backend::{SpecBackend, Words};
use crate::listening::renderer::Renderer;
use async_trait::async_trait;
use kumi_common::abort::Signal;
use serde_json::{json, Value};
use std::rc::Rc;

/// A DAW's facts for delivery to a spec: its words, which track row is the master's, and the field its fader is in.
pub struct OwnSpecFacts {
    pub words: Words,
    pub is_master: fn(&Value) -> bool,
    pub fader: &'static str,
}

/// Pages of track rows read for the master (200 a page).
const PAGES: usize = 10;

pub struct OwnSpec<S: RefScheme + 'static> {
    plan: Rc<OwnPlan<S>>,
    renderer: Rc<OwnRenderer<S>>,
    facts: &'static OwnSpecFacts,
}

impl<S: RefScheme + 'static> OwnSpec<S> {
    pub fn new(plan: Rc<OwnPlan<S>>, renderer: Rc<OwnRenderer<S>>, facts: &'static OwnSpecFacts) -> Self {
        OwnSpec { plan, renderer, facts }
    }

    async fn master(&self, signal: &Signal) -> Result<Value, String> {
        let rows = self.plan.rows_through(json!({"kind": "track", "limit": 200}), PAGES, signal).await?;
        rows.into_iter().find(|row| (self.facts.is_master)(row)).ok_or_else(|| "the master's track row wasn't found".to_string())
    }
}

#[async_trait(?Send)]
impl<S: RefScheme + 'static> SpecBackend for OwnSpec<S> {
    fn words(&self) -> &'static Words {
        &self.facts.words
    }

    async fn render_mix(&self, stretch: Option<(f64, f64)>, signal: &Signal) -> Result<Render, String> {
        if self.renderer.listening_now() {
            return Err("Kumi is listening to something else just now".into());
        }
        if let Some(why) = self.renderer.why_not() {
            return Err(why);
        }
        let window = match stretch {
            Some((from, beats)) => crate::listening::renderer::Window { from, beats },
            None => {
                let end = self
                    .renderer
                    .song_end(signal.clone())
                    .await
                    .ok_or("Kumi couldn't tell where the song ends: give fromBeat and beats")?;
                crate::listening::renderer::Window { from: 0., beats: end }
            }
        };
        let _listening = self.renderer.begin_listen();
        let sources = vec![json!({"id": "m", "what": "master"})];
        let rendered = self.renderer.render_sources(sources, window, signal).await;
        self.renderer.put_tempo_back().await;
        match rendered {
            Ok(Ok((files, _))) => files.get("m").map(|(render, _)| render.clone()).ok_or_else(|| "the render made no file".to_string()),
            Ok(Err(why)) => Err(why),
            Err(error) => Err(error.to_string()),
        }
    }

    async fn master_db(&self, signal: &Signal) -> Result<f64, String> {
        let master = self.master(signal).await?;
        master.get(self.facts.fader).and_then(Value::as_f64).ok_or_else(|| "the master's fader wasn't read".to_string())
    }

    async fn set_master_db(&self, db: f64, signal: &Signal) -> Result<Value, String> {
        let master = self.master(signal).await?;
        let reference = master.get("ref").and_then(Value::as_str).ok_or("the master has no ref")?;
        let input = json!({"ref": reference, "volumeDb": (db * 100.).round() / 100.});
        let outcome = self.plan.change_by("track_volume", input.as_object().cloned().unwrap_or_default(), signal.clone()).await;
        let answer: Value = serde_json::from_str(&outcome.text).unwrap_or_else(|_| json!({"said": outcome.text}));
        if outcome.is_error || outcome.stops {
            return Err(answer.get("changed").and_then(Value::as_str).map(str::to_owned).unwrap_or(outcome.text));
        }
        Ok(answer)
    }
}
