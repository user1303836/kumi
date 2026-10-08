//! The form tool: the song heard quietly and read bar by bar, which tracks play where (from the Arrangement), and a
//! reference's form beside it.

use super::super::connection::NO_CURRENT_LIVE;
use super::judge::JudgeHeard;
use super::rig::Window;
use super::*;
use crate::listening::{
    form::{compare, file_grid, form, form_from, Form},
    measure::{measure_file, MeasureOptions},
    notes::Grid,
};
use kumi_common::js::string::head;

/// What the model asks of the form tool: a stretch (the whole song without one) and a reference to compare with.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct FormRequest {
    pub from_beat: Option<f64>,
    pub beats: Option<f64>,
    pub reference: Option<String>,
}

impl Rendering {
    pub async fn form(self: &Rc<Self>, request: &FormRequest, original: Signal) -> Result<Result<Value, String>, RuntimeError> {
        if !self.available() {
            return Ok(Err(NO_CURRENT_LIVE.into()));
        }
        let Some(tempo) = self.observer.tempo.get().filter(|tempo| *tempo > 0.) else {
            return Ok(Err("Kumi doesn't know the Set's tempo yet; try again.".into()));
        };
        if self.rendering.get() {
            return Ok(Err("Kumi is already listening to something; wait for it.".into()));
        }
        let signal = abort::any([original, self.connection().lifetime.clone()]);
        let meter = self.observer.beats_per_bar.get().max(1.);
        // Whole bars: the form is read a bar at a time.
        let from = (request.from_beat.unwrap_or(0.) / meter).floor() * meter;
        let beats = match request.beats {
            Some(beats) => beats,
            None => match self.song_end(signal.clone()).await {
                Some(end) => end - from,
                None => return Ok(Err("Kumi couldn't tell where the song ends; give from_beat and beats.".into())),
            },
        };
        let span = Window { from, beats: (beats / meter).ceil().max(1.) * meter };
        let bar = meter * 60. / tempo;
        // The reference first: one that can't be heard says so before the song is.
        let reference = match &request.reference {
            Some(named) => {
                let file = match self.reference_file(named, signal.clone()).await? {
                    Ok(file) => file,
                    Err(why) => return Ok(Err(format!("The reference: {why}"))),
                };
                let heard = match measure_file(&file, MeasureOptions { signal: Some(signal.clone()), ..Default::default() }).await {
                    Ok(heard) => heard,
                    Err(error) => return Ok(Err(format!("Kumi couldn't hear the reference: {}", head(&error.to_string(), 200)))),
                };
                // In its own bars: a reference at another tempo cut into this Set's bars gains or loses bars.
                let grid = file_grid(&file, meter, self.observer.tempo.get(), signal.clone()).await;
                Some(match grid {
                    Some(grid) => (form_from(&heard, meter * 60. / grid.tempo, grid.downbeat), Some(grid)),
                    None => (form(&heard, bar), None),
                })
            }
            None => None,
        };
        let heard = match self.judge_hear(None, None, span, signal.clone()).await? {
            Ok(JudgeHeard { silent: Some(why), .. }) | Err(why) => return Ok(Err(why)),
            Ok(heard) => heard,
        };
        let shape = form(&heard.main, bar);
        let first = (from / meter) as usize;
        let elements = self.elements(span, meter, signal.clone()).await;
        Ok(Ok(reply(&shape, first, &elements, reference.as_ref())))
    }

    /// Which tracks play where in the span, from the Arrangement's clips: each track's runs of bars (song bars, from
    /// 1), clips that touch merged.
    async fn elements(&self, span: Window, meter: f64, signal: Signal) -> Vec<(String, Vec<(usize, usize)>)> {
        let Ok(tracks) = self.rows("track", json!({"fields":["name"]}), signal.clone()).await else { return vec![] };
        let mut found = vec![];
        for track in tracks.iter().take(64) {
            let (Some(reference), Some(name)) = (track.get("ref").and_then(Value::as_str), track.get("name").and_then(Value::as_str))
            else {
                continue;
            };
            let Ok(clips) = self
                .rows("arrangement-clip", json!({"parent":reference,"fields":["start","endTime","length","muted"]}), signal.clone())
                .await
            else {
                continue;
            };
            let mut runs: Vec<(usize, usize)> = clips
                .iter()
                // A muted clip doesn't play.
                .filter(|clip| clip.get("muted").and_then(Value::as_bool) != Some(true))
                .filter_map(|clip| {
                    let start = clip.get("start").and_then(Value::as_f64)?;
                    // Where it stops playing: a looped clip's length is its loop's, not how far it runs.
                    let end = match clip.get("endTime").and_then(Value::as_f64) {
                        Some(end) => end,
                        None => start + clip.get("length").and_then(Value::as_f64)?,
                    };
                    let (start, end) = (start.max(span.from), end.min(span.from + span.beats));
                    (end > start).then(|| ((start / meter).floor() as usize + 1, (end / meter).ceil() as usize))
                })
                .collect();
            runs.sort_unstable();
            let mut merged: Vec<(usize, usize)> = vec![];
            for (from, to) in runs {
                match merged.last_mut() {
                    Some(last) if from <= last.1 + 1 => last.1 = last.1.max(to),
                    _ => merged.push((from, to)),
                }
            }
            if !merged.is_empty() {
                found.push((name.to_string(), merged));
            }
        }
        found
    }
}

/// The form as the model reads it: bar numbers as the song's (from `first`, its first bar, counted from 0).
fn reply(shape: &Form, first: usize, elements: &[(String, Vec<(usize, usize)>)], reference: Option<&(Form, Option<Grid>)>) -> Value {
    let bars = |from: usize, to: usize| format!("{}–{}", first + from + 1, first + to);
    let sections: Vec<Value> = shape
        .sections
        .iter()
        .map(|section| json!({"bars": bars(section.from, section.to), "role": section.role, "letter": section.letter.to_string(), "loudness": section.loudness}))
        .collect();
    let turns: Vec<Value> =
        shape.transitions.iter().map(|turn| json!({"bar": first + turn.at + 1, "kind": turn.kind, "prepared": turn.prepared})).collect();
    let plays: serde_json::Map<String, Value> = elements
        .iter()
        .map(|(name, runs)| {
            let said: Vec<String> =
                runs.iter().map(|(from, to)| if from == to { format!("{from}") } else { format!("{from}–{to}") }).collect();
            (name.clone(), json!(format!("bars {}", said.join(", "))))
        })
        .collect();
    let mut said = json!({
        "bars": shape.bars.len(),
        "form": shape.sections.iter().map(|section| section.letter).collect::<String>(),
        "sections": sections,
        "turns": turns,
        "intro": format!("{} bars", shape.intro),
        "outro": format!("{} bars", shape.outro),
        "firstHook": shape.hook.map(|bar| format!("bar {}", first + bar + 1)),
        "loudnessByBar": shape.bars.iter().map(|bar| (bar.loudness * 10.).round() / 10.).collect::<Vec<_>>(),
        "problems": shape.problems,
    });
    if !plays.is_empty() {
        said["playing"] = Value::Object(plays);
    }
    if let Some((reference, grid)) = reference {
        said["reference"] = json!({
            "form": reference.sections.iter().map(|section| section.letter).collect::<String>(),
            "bars": reference.bars.len(),
            "readIn": match grid {
                Some(grid) => format!("its own bars: {} BPM, beat one {} s in", (grid.tempo * 10.).round() / 10., (grid.downbeat * 100.).round() / 100.),
                None => "this Set's bars, from its start (its tempo couldn't be told)".into(),
            },
            "differences": compare(shape, reference),
        });
    }
    said
}
