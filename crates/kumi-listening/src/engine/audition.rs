//! The audition tool: candidates heard (the first where the producer hears it, the rest on their own), each measured,
//! compared with a reference, and the best of the round kept. The DAW plays and records them; scoring is the same
//! whichever DAW it is.
use super::{too_long, Listening};
use crate::{
    audio::{
        self,
        analyze::{Analysis, AnalyzeOptions},
        matching::{closeness, Focus},
        tools::summary,
    },
    core::{contracts::*, errors::RuntimeError},
    listening::renderer::{Audible, AuditionOptions},
};
use kumi_common::{
    abort::{self, Signal, SignalExt},
    js::{
        json::stringify,
        number::{round, to_string},
        string::head,
    },
    time::now_ms,
};
use serde_json::json;
use std::{
    panic::{catch_unwind, AssertUnwindSafe},
    rc::Rc,
};

impl Listening {
    pub async fn audition(
        self: &Rc<Self>,
        request: &AuditionRequest,
        original: Signal,
    ) -> Result<Result<AuditionResult, String>, RuntimeError> {
        let began = now_ms();
        if let Some(why) = self.host.render_refusal() {
            return Ok(Err(why));
        }
        if self.host.flag().is_on() {
            return Ok(Err("An audition is already running; wait for it.".into()));
        }
        let Some(tempo) = self.host.tempo().filter(|value| *value != 0. && !value.is_nan()) else {
            return Ok(Err(self.no_tempo()));
        };
        let signal = abort::any([original.clone(), self.host.lifetime()]);
        let mut notes = vec![];
        self.rounds.set(self.rounds.get() + 1);
        let round = self.rounds.get();
        let mut files = vec![];
        let mut takes: Vec<_> = request
            .candidates
            .iter()
            .enumerate()
            .map(|(i, candidate)| AuditionTake {
                label: candidate.label.clone().unwrap_or_else(|| format!("Candidate {}", i + 1)),
                track: candidate.track.clone(),
                r#where: None,
                silent: None,
                closeness: None,
                render: None,
                heard: None,
            })
            .collect();
        let mut beats = request.beats.unwrap_or(8.);
        if let Some(why) = too_long(beats, tempo, "audition a part") {
            return Ok(Err(why));
        }
        let rendering = self.host.flag().begin();
        // The producer hears the first candidate where it plays; Kumi hears the others on their own.
        let pass = self
            .host
            .audition_pass(
                &request.candidates,
                request.from_beat,
                request.beats,
                AuditionOptions { audible: Audible::First, round, tempo, first: !self.told_length.get() },
                signal.clone(),
            )
            .await;
        // Told how long a round takes once, when the DAW got as far as saying so.
        if pass.opened {
            self.told_length.set(true);
        }
        match pass.rendered {
            Ok(rendered) => {
                beats = rendered.beats;
                // The producer hears an audition as Kumi does: a DAW that captured every candidate without playing it
                // says so, and the producer is told it wasn't played out loud.
                if !rendered.sources.is_empty() && rendered.sources.iter().all(|source| !source.audible) {
                    let profile = self.host.profile();
                    let how = if profile.caps.renders_to_listen { "rendered" } else { "captured" };
                    notes.push(format!("The producer didn't hear this round: {} {how} it without playing it out loud.", profile.words.app));
                }
                for (index, source) in rendered.sources.iter().enumerate() {
                    let at = takes.iter().position(|take| take.track == source.track || take.track == source.name).unwrap_or(index);
                    let take = &mut takes[at];
                    take.r#where = Some(Where { track: source.name.clone(), clip: source.scene.map(|scene| format!("scene:{scene}")) });
                    if let Some(found) = rendered.files.get(&source.name) {
                        files.push((at, found.clone()));
                    } else {
                        take.silent = Some(true);
                    }
                }
            }
            Err(error) => {
                notes.push(if original.is_cancelled() { "Stopped before it finished.".into() } else { head(&error.to_string(), 400) });
            }
        }
        notes.extend(pass.notes);
        drop(rendering);
        let listen: Result<AuditionResult, RuntimeError> = async {
            let reference = match request.reference.as_deref().filter(|s| !s.is_empty()) {
                Some(named) => Some(self.heard_reference(named, request, signal.clone()).await?),
                None => None,
            };
            for (index, render) in &files {
                let seconds = render.seconds.unwrap_or(beats * 60. / tempo);
                let heard =
                    audio::hear(&render.file, heard_options(render.start, seconds, request.focus, signal.clone())).await.map_err(plain)?;
                let take = &mut takes[*index];
                take.heard = Some(Heard { lufs: heard.loudness.integrated_lufs, summary: summary(&heard) });
                take.render = Some(render.clone());
                if silent_render(&heard) {
                    take.silent = Some(true);
                    continue;
                }
                if let Some(reference) = &reference {
                    take.closeness = Some(closeness(&heard, reference, request.focus));
                }
            }
            let mut scored: Vec<_> = takes.iter().filter(|take| take.closeness.is_some()).collect();
            scored.sort_by(|a, b| b.closeness.as_ref().unwrap().score.total_cmp(&a.closeness.as_ref().unwrap().score));
            let best = scored.first().copied();
            let silent = takes.iter().all(|take| take.silent == Some(true)) && !files.is_empty();
            if silent {
                notes.push(format!(
                    "The render was silent: is the source playing in the {} at that spot (its {} there, the track not muted)?",
                    self.host.profile().words.timeline,
                    self.host.profile().vocab.clips
                ));
            }
            let previous = self.best.get();
            if let Some(best) = best {
                let score = best.closeness.as_ref().unwrap().score;
                if previous.is_none_or(|previous| score > previous) {
                    self.best.set(Some(score));
                }
            }
            let title = if let Some(best) = best {
                format!(
                    "Auditioned {}",
                    if takes.len() == 1 { best.label.clone() } else { format!("{} candidates · best {}", takes.len(), best.label) }
                )
            } else if silent {
                "Auditioned: the render was silent".into()
            } else {
                format!("Auditioned {}", if takes.len() == 1 { takes[0].label.clone() } else { format!("{} candidates", takes.len()) })
            };
            self.host.emit(ChangeRecord {
                id: format!("a{}", &uuid::Uuid::new_v4().to_string()[..8]),
                family: ChangeFamily::Clip,
                title,
                state: ChangeState::Heard,
                score: best.and_then(|take| take.closeness.as_ref()).map(|close| close.score),
                at: self.host.now_ms(),
                track: None,
                from: None,
                to: None,
                range: None,
                colors: None,
                clip: None,
                devices: None,
                note: None,
                picture: None,
            });
            if let Some(tell) = &self.on_audition {
                let event = AuditionEvent {
                    round: round as u32,
                    best: best.map(|take| BestTake { label: take.label.clone(), score: take.closeness.as_ref().unwrap().score }),
                    previous,
                    takes: scored
                        .iter()
                        .map(|take| TakeScore {
                            label: take.label.clone(),
                            score: take.closeness.as_ref().map(|close| close.score),
                            silent: None,
                            r#where: take.r#where.clone(),
                        })
                        .chain(takes.iter().filter(|take| take.closeness.is_none()).map(|take| TakeScore {
                            label: take.label.clone(),
                            score: None,
                            silent: take.silent.filter(|s| *s),
                            r#where: take.r#where.clone(),
                        }))
                        .collect(),
                    gaps: best
                        .and_then(|take| take.closeness.as_ref())
                        .map(|close| close.gaps.iter().take(3).cloned().collect())
                        .unwrap_or_default(),
                    request: Some(request.clone()),
                    reference: reference.as_ref().map(summary),
                    structural: best
                        .and_then(|take| take.closeness.as_ref())
                        .and_then(|close| close.structural.as_ref())
                        .map(|s| StructuralMove { gap: s.gap.clone(), r#move: s.r#move.clone() }),
                };
                let _ = catch_unwind(AssertUnwindSafe(|| tell(event)));
            }
            self.tell(
                best.map(|take| format!("Auditioned · {}%", to_string(take.closeness.as_ref().unwrap().score)))
                    .unwrap_or_else(|| "Auditioned".into()),
                Some(false),
            );
            Ok(AuditionResult {
                best: best.map(|take| take.label.clone()),
                reference: reference.map(|heard| ReferenceHeard { summary: summary(&heard), file: heard.file }),
                seconds: round_number((now_ms() - began) as f64 / 100.) / 10.,
                takes,
                notes: notes.clone(),
            })
        }
        .await;
        match listen {
            Ok(result) => Ok(Ok(result)),
            Err(error) => {
                self.tell("Auditioned", Some(false));
                signal.check()?;
                Ok(Err(format!(
                    "{}Kumi couldn't listen to the render: {}",
                    if notes.is_empty() { String::new() } else { format!("{} ", notes.join(" ")) },
                    head(&error.to_string(), 300)
                )))
            }
        }
    }

    /// The reference an audition (or a goal) compares with: heard from the file a clip or a path stands for, over the
    /// stretch asked for, and kept for the next round.
    pub async fn heard_reference(&self, named: &str, request: &AuditionRequest, signal: Signal) -> Result<Analysis, RuntimeError> {
        let file = self.reference_file(named, signal.clone()).await?.map_err(RuntimeError::Observation)?;
        let key = stringify(&json!([file, request.reference_from.unwrap_or(0.), request.reference_seconds, request.focus]));
        if let Some(known) = self.reference_cache.borrow().get(&key) {
            return Ok(known.clone());
        }
        let heard = audio::hear(
            &file,
            AnalyzeOptions {
                start: request.reference_from,
                seconds: request.reference_seconds,
                focus: analysis_focus(request.focus),
                signal: Some(signal),
                ..Default::default()
            },
        )
        .await
        .map_err(|e| RuntimeError::plain(e.to_string()))?;
        let mut cache = self.reference_cache.borrow_mut();
        cache.insert(key, heard.clone());
        if cache.len() > 32 {
            cache.shift_remove_index(0);
        }
        Ok(heard)
    }
}

fn round_number(value: f64) -> f64 {
    round(value)
}

fn plain(error: impl std::fmt::Display) -> RuntimeError {
    RuntimeError::plain(error.to_string())
}

/// Whether a render held no sound.
pub fn silent_render(analysis: &Analysis) -> bool {
    analysis.loudness.integrated_lufs.is_none_or(|value| value < -60.0) || analysis.loudness.sample_peak_dbfs < -55.0
}

/// How a stretch is analysed for a focus: the whole section's mix, or one sound's.
pub fn analysis_focus(focus: Option<Focus>) -> Option<String> {
    focus.map(|focus| if focus == Focus::Section { "mix" } else { "sound" }.into())
}

/// What a heard stretch is analysed with: from `start`, `seconds` long (a little extra before a sound's, which has
/// its attack), for a focus.
pub fn heard_options(start: f64, seconds: f64, focus: Option<Focus>, signal: Signal) -> AnalyzeOptions {
    AnalyzeOptions {
        start: Some(start + if focus == Some(Focus::Section) { 0.1 } else { 0. }),
        seconds: Some(seconds + if focus == Some(Focus::Section) { 0. } else { 0.1 }),
        focus: analysis_focus(focus),
        signal: Some(signal),
        ..Default::default()
    }
}
