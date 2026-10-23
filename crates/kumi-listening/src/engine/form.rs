//! The form tool: the song played, heard and read bar by bar, which tracks play where (from the timeline), and a
//! reference's form beside it.

use super::{judge::JudgeHeard, Listening};
use crate::{
    form::{compare, file_grid, form, form_from, Form},
    measure::{measure_file, MeasureOptions},
    notes::Grid,
    references::tool::MEASURED,
    renderer::Window,
    tools::FormRequest,
};
use kumi_common::{
    abort::{self, Signal},
    js::string::head,
};
use kumi_types::errors::RuntimeError;
use serde_json::{json, Value};
use std::rc::Rc;

impl Listening {
    pub async fn form(self: &Rc<Self>, request: &FormRequest, original: Signal) -> Result<Result<Value, String>, RuntimeError> {
        let formed = self.form_now(request, original).await;
        // form hears through the judge's listen, which keeps its takes in judge/: they go now, a judged run's own stay.
        self.prune_kept().await;
        formed
    }

    async fn form_now(self: &Rc<Self>, request: &FormRequest, original: Signal) -> Result<Result<Value, String>, RuntimeError> {
        if let Some(why) = self.host.refusal() {
            return Ok(Err(why));
        }
        let Some(tempo) = self.host.tempo().filter(|tempo| *tempo > 0.) else {
            return Ok(Err(self.no_tempo()));
        };
        if self.host.flag().is_on() {
            return Ok(Err("Kumi is already listening to something; wait for it.".into()));
        }
        let signal = abort::any([original, self.host.lifetime()]);
        let meter = self.host.beats_per_bar().max(1.);
        // Whole bars: the form is read a bar at a time.
        let from = (request.from_beat.unwrap_or(0.) / meter).floor() * meter;
        let beats = match request.beats {
            Some(beats) => beats,
            None => match self.host.song_end(signal.clone()).await {
                Some(end) => end - from,
                None => return Ok(Err("Kumi couldn't tell where the song ends; give from_beat and beats.".into())),
            },
        };
        let span = Window { from, beats: (beats / meter).ceil().max(1.) * meter };
        if let Some(why) = super::too_long(span.beats, tempo, "read its form in parts: give from_beat and beats") {
            return Ok(Err(why));
        }
        let bar = meter * 60. / tempo;
        // The reference first: one that can't be heard says so before the song is.
        let reference = match &request.reference {
            Some(named) => {
                let file = match self.reference_file(named, signal.clone()).await? {
                    Ok(file) => file,
                    Err(why) => return Ok(Err(format!("The reference: {why}"))),
                };
                // Its first six minutes, as the reference tool measures a track.
                let options = MeasureOptions { seconds: Some(MEASURED), signal: Some(signal.clone()), ..Default::default() };
                let heard = match measure_file(&file, options).await {
                    Ok(heard) => heard,
                    Err(error) => return Ok(Err(format!("Kumi couldn't hear the reference: {}", head(&error.to_string(), 200)))),
                };
                // In its own bars: a reference at another tempo cut into this Set's bars gains or loses bars.
                let grid = file_grid(&file, meter, self.host.tempo(), signal.clone()).await;
                Some(match grid {
                    Some(grid) => (form_from(&heard, meter * 60. / grid.tempo, grid.downbeat), Some(grid)),
                    None => (form(&heard, bar), None),
                })
            }
            None => None,
        };
        let heard = match self.judge_hear(None, None, span, "the song, read bar by bar", signal.clone()).await? {
            Ok(JudgeHeard { silent: Some(why), .. }) | Err(why) => return Ok(Err(why)),
            Ok(heard) => heard,
        };
        let shape = form(&heard.main, bar);
        let first = (from / meter) as usize;
        let elements = self.elements(span, meter, signal.clone()).await;
        Ok(Ok(reply(self.host.profile().words.place, &shape, first, &elements, reference.as_ref())))
    }

    /// Which tracks play where in the span, from the timeline's clips: each track's runs of bars (song bars, from 1),
    /// clips that touch merged.
    async fn elements(&self, span: Window, meter: f64, signal: Signal) -> Vec<(String, Vec<(usize, usize)>)> {
        let mut found = vec![];
        for (name, clips) in self.host.clips_by_track(signal).await {
            let mut runs: Vec<(usize, usize)> = clips
                .iter()
                .filter_map(|(start, end)| {
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
                found.push((name, merged));
            }
        }
        found
    }
}

/// The form as the model reads it: bar numbers as the song's (from `first`, its first bar, counted from 0).
fn reply(
    place: &str,
    shape: &Form,
    first: usize,
    elements: &[(String, Vec<(usize, usize)>)],
    reference: Option<&(Form, Option<Grid>)>,
) -> Value {
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
        "onsetsPerSecondByBar": shape.bars.iter().map(|bar| (bar.density * 10.).round() / 10.).collect::<Vec<_>>(),
        "brightnessHzByBar": shape.bars.iter().map(|bar| (20. * bar.brightness.exp2() / 10.).round() * 10.).collect::<Vec<_>>(),
        "lowEndDbByBar": shape.bars.iter().map(|bar| (bar.low * 10.).round() / 10.).collect::<Vec<_>>(),
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
                None => format!("this {place}'s bars, from its start (its tempo couldn't be told)"),
            },
            "differences": compare(shape, reference),
        });
    }
    said
}
