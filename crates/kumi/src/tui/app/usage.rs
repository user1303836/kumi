//! What the model thinks and what each answer costs: the reasoning's headline while it thinks (in NOW and on its own
//! step row, one line, never the reasoning itself), how full the answering model's context is (the header's meter),
//! and the quiet line under a finished answer (its time, its changes, its tokens on an API key or its share of the
//! plan on a subscription). The headline is shown only: it never reaches the answer's text, /copy, HISTORY or the
//! model.

use super::super::{
    style::{palette, Style},
    transcript::{headline, Thinking},
    wrap::Span,
};
use super::drawing::{sp, st};
use super::*;
use kumi_ai::providers::{usage::PlanWindow, SignIn};

/// The reasoning kept for its headline: past this, only its latest part.
const REASONING_KEPT: usize = 16 * 1024;

impl TuiApp {
    /// A piece of the model's reasoning: the answer's thinking row and NOW say its headline.
    pub(super) fn reasoned(&self, text: &str) {
        let mut state = self.0.state.borrow_mut();
        let Some(entry) = state.current.clone().filter(|_| !state.suppress) else { return };
        state.reasoning.push_str(text);
        if state.reasoning.len() > REASONING_KEPT {
            let from = state.reasoning.len() - REASONING_KEPT / 2;
            let from = (from..state.reasoning.len()).find(|at| state.reasoning.is_char_boundary(*at)).unwrap_or(0);
            state.reasoning.drain(..from);
        }
        let said = head(&sanitize_text(&headline(&state.reasoning), &state.secrets), 200);
        let busy_since = state.busy_since;
        if let Entry::Assistant { thinking, steps, started_at, .. } = &mut *entry.borrow_mut() {
            // Thinking since the last step ended, or since the answer began.
            let since = thinking
                .as_ref()
                .map(|t| t.since)
                .or_else(|| steps.last().and_then(|step| step.ended_at))
                .or(*started_at)
                .unwrap_or(busy_since);
            *thinking = (!said.is_empty()).then_some(Thinking { headline: said, since });
        }
        state.transcript.touch(&entry);
    }

    /// The model spoke, began a step or finished: what it thought is no longer what it's doing.
    pub(super) fn thought_ends(&self) {
        let mut state = self.0.state.borrow_mut();
        state.reasoning.clear();
        let Some(entry) = state.current.clone() else { return };
        let ended = match &mut *entry.borrow_mut() {
            Entry::Assistant { thinking, .. } => thinking.take().is_some(),
            _ => false,
        };
        if ended {
            state.transcript.touch(&entry);
        }
    }

    /// A model call finished: its tokens, and the model that answered it, for the context meter.
    pub(super) fn called(&self, usage: Usage) {
        let answering = self.answering_model();
        self.0.state.borrow_mut().last_call = Some((usage, answering));
    }

    /// The model answering now: the producer's fallback while it answers for their model, else their model.
    fn answering_model(&self) -> Option<String> {
        self.0.options.controller.answering_fallback().or_else(|| self.0.options.models.as_ref().and_then(|models| models.current().model))
    }

    /// The answering model's provider: its sign-in says what an answer's cost is told in.
    fn answering_provider(&self) -> Option<String> {
        match self.0.options.controller.answering_fallback() {
            Some(model) => model.split('/').next().map(str::to_string),
            None => self.0.options.models.as_ref()?.current().provider,
        }
    }

    /// The plan window behind the answering model nearest its limit, as its provider last said (none on an API key).
    pub(super) fn answering_plan(&self) -> Option<PlanWindow> {
        kumi_ai::providers::usage::nearest(&self.answering_provider()?, 0.)
    }

    /// The quiet line under a finished answer: how long it took, what it changed, and what it cost.
    pub(super) fn answer_footer(&self, elapsed: f64, changes: usize, usage: Option<&Usage>) -> String {
        // (a minute or more as a clock: 3:20)
        let took = if elapsed < 60_000. { super::super::transcript::seconds(elapsed) } else { helpers::clock_of(elapsed) };
        let mut said = vec![took];
        if changes > 0 {
            said.push(format!("{changes} {}", if changes == 1 { "change" } else { "changes" }));
        }
        said.extend(self.answer_cost(usage));
        said.join(" · ")
    }

    /// Tokens on an API key; on a subscription, the share of the plan's window the answer took (when the provider said).
    fn answer_cost(&self, usage: Option<&Usage>) -> Option<String> {
        let provider = self.answering_provider()?;
        let cloud = self.0.options.models.as_ref()?.cloud_provider(&provider)?;
        if cloud.row().sign_in == SignIn::ApiKey {
            let usage = usage.filter(|usage| usage.input_tokens + usage.output_tokens > 0.)?;
            return Some(format!("{} in · {} out", tokens(usage.input_tokens), tokens(usage.output_tokens)));
        }
        let after = kumi_ai::providers::usage::nearest(&provider, 0.)?;
        let before = self.0.state.borrow().plan_before.clone().filter(|before| before.window_minutes == after.window_minutes);
        let share = after.used_percent - before.map_or(after.used_percent, |before| before.used_percent);
        (share > 0.).then(|| if share < 1. { "under 1% of your plan".into() } else { format!("{}% of your plan", share.round()) })
    }

    /// How full the answering model's context is, for the header: "context ▰▰▰▰▱▱▱▱▱▱ 41%", from half full (or always,
    /// or never, as `contextMeter` says); none while that model's window isn't known.
    pub(super) fn context_meter(&self) -> Option<Vec<Span>> {
        let mode = self.0.options.context_meter;
        if mode == ContextMeter::Off {
            return None;
        }
        let (usage, model) = self.0.state.borrow().last_call.clone()?;
        let window = self.0.options.models.as_ref()?.context_window(model.as_deref()?).filter(|window| *window > 0.)?;
        let full = ((usage.input_tokens + usage.output_tokens) / window).clamp(0., 1.);
        if mode == ContextMeter::Auto && full < 0.5 {
            return None;
        }
        let filled = (full * 10.).round() as usize;
        // Near the end of the window, in the warning colour.
        let style = if full >= 0.8 { st::WARN } else { st::DIM };
        Some(vec![
            sp("context ", st::FAINT),
            sp("▰".repeat(filled), style),
            sp("▱".repeat(10 - filled), Style::fg(palette::FAINT)),
            sp(format!(" {}%", (full * 100.).round()), style),
        ])
    }
}

/// A count of tokens as the footer says it: 830, 18.4k, 1.25M.
fn tokens(n: f64) -> String {
    if n < 1000. {
        number::to_string(n)
    } else if n < 1_000_000. {
        format!("{}k", number::to_fixed(n / 1000., 1))
    } else {
        format!("{}M", number::to_fixed(n / 1_000_000., 2))
    }
}

/// What NOW says while the model thinks with a headline: "thinking · 31s", the answer's changes after it.
pub(super) fn thinking_label(thinking: &Thinking, now: f64, changes: usize) -> String {
    let seconds = ((now - thinking.since) / 1000.).floor().max(0.);
    let took = if seconds < 60. { format!("{seconds}s") } else { helpers::clock_of(now - thinking.since) };
    let changed = if changes > 0 { format!(" · {changes} {}", if changes == 1 { "change" } else { "changes" }) } else { String::new() };
    format!("thinking · {took}{changed}")
}
