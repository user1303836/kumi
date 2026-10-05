//! The listen tool: files or the Set, alone or against a reference.

use super::{
    analyze::{Analysis, AnalyzeOptions, HeardNote},
    audio_path, compare, hear,
    structure::{hear_form, FormOptions},
    AudioError,
};
use crate::core::{
    contracts::{HearRequest, HeardComparison, HeardEvent, HeardTake, JsonObject, KernelTool, ToolResult},
    errors::RuntimeError,
};
use async_trait::async_trait;
use futures::{future::try_join_all, future::LocalBoxFuture};
use kumi_common::{
    abort::{Signal, SignalExt},
    js::{
        json::stringify,
        number::{round, to_string},
        string::head,
    },
};
use serde_json::{json, Value};
use std::rc::Rc;

pub const LISTEN_TOOL: &str = "listen";
const DESCRIPTION:&str="Hear audio the producer points you to: a reference track, a sample, a bounce or a recording, by its file path (find_sounds finds audio files by words in folders the producer names, such as ~/Downloads) or, for an audio clip in the Set, its clipRef. Hear the Set itself with track (or tracks, to hear several together and what clashes between them) or mix: true. Kumi listens in Live directly, after each track's devices: while Live plays, to what's playing now (seconds, 8 by default); while it's stopped, quietly, to the loop or from the playhead (or from_beat and beats). No recording or bouncing first. Measures loudness (integrated LUFS, true peak, loudness range), tonal balance in named bands, stereo width per band, dynamics, tempo, key and the energy over time as a small text spectrogram; for a single sound (a note, a hit, a short sample), also its pitch, harmonics (which waveform it's like), envelope and movement (filter opening or closing, wobble or tremolo rate, at the tempo when known). With compare_to, sets file against the reference with loudness matched and lists what differs most (bands, brightness, width, compression, loudness). Use it before matching a mix to a reference (EQ, compression, width, loudness moves) or rebuilding a sound (harmonics to oscillators and filter, envelope to the amp and filter envelopes, movement to an LFO's rate and target). With form, hears a song's sections instead (in bars, with their energy and which are alike), to arrange like it. Say what you heard in the producer's terms, not as a data dump.";
pub type OnHeard = Rc<dyn Fn(HeardEvent)>;
pub type ResolveAudio = Rc<dyn Fn(String, Signal) -> LocalBoxFuture<'static, Result<Option<String>, RuntimeError>>>;
pub type HearSet = Rc<dyn Fn(HearRequest, Signal) -> LocalBoxFuture<'static, Result<Result<Vec<HeardTake>, String>, RuntimeError>>>;
#[derive(Clone)]
pub struct ListeningOptions {
    pub on_event: OnHeard,
    pub resolve: Option<ResolveAudio>,
    pub hear: Option<HearSet>,
}
impl Default for ListeningOptions {
    fn default() -> Self {
        Self { on_event: Rc::new(|_| {}), resolve: None, hear: None }
    }
}
struct ListenTool {
    options: ListeningOptions,
}
pub fn listening_tools(options: ListeningOptions) -> Vec<Rc<dyn KernelTool>> {
    vec![Rc::new(ListenTool { options })]
}
/// Notes in MIDI rows, retaining performed timing instead of snapping starts to a grid.
pub fn transcription(notes: &[HeardNote], tempo: Option<f64>) -> JsonObject {
    let tempo = tempo.filter(|v| *v != 0.0);
    let unit = tempo.map_or(1.0, |t| t / 60.0);
    let at = |seconds: f64| if tempo.is_some() { round(seconds * unit * 100.0) / 100.0 } else { round(seconds * 1000.0) / 1000.0 };
    let rows: Vec<_> = notes
        .iter()
        .take(400)
        .map(|n| json!([at(n.time), n.midi, n.velocity, (if tempo.is_some() { 0.1f64 } else { 0.02 }).max(at(n.duration))]))
        .collect();
    let pitched: Vec<_> = notes.iter().filter(|n| n.midi.is_some()).collect();
    let mut counts: Vec<(f64, usize)> = Vec::new();
    for note in &pitched {
        let midi = note.midi.unwrap();
        if let Some((_, count)) = counts.iter_mut().find(|(m, _)| *m == midi) {
            *count += 1;
        } else {
            counts.push((midi, 1));
        }
    }
    counts.sort_by(|a, b| b.1.cmp(&a.1));
    let most: Vec<_> = counts.into_iter().take(6).map(|(midi, count)| json!({"midi":midi,"count":count})).collect();
    let mut out=json!({"unit":tempo.map_or_else(||"seconds".into(),|t|format!("beats at {} BPM (a 16th is 0.25; starts as played, not on the grid)",to_string(t))),"columns":["start","pitch (MIDI; null: a hit with no clear pitch)","velocity","length"],"rows":rows}).as_object().unwrap().clone();
    if notes.len() > 400 {
        out.insert("more".into(), json!(notes.len() - 400));
    }
    out.insert("pitched".into(), json!(pitched.len()));
    out.insert("unpitched".into(), json!(notes.len() - pitched.len()));
    out.insert("mostPlayed".into(), json!(most));
    out.insert("note".into(),json!(format!("Monophonic: the strongest line. Write it with write_midi_clip at these starts as they are{} (rounding them to the grid moves the rhythm away from the reference's), unpitched hits as a drum or percussive voice; then audition it against the reference.",tempo.map_or_else(String::new,|t|format!(", with the Set at {} BPM",to_string(t))))));
    out
}
fn trim_sound(analysis: &Analysis, tempo: Option<f64>) -> Value {
    let mut result = serde_json::to_value(analysis).unwrap();
    let object = result.as_object_mut().unwrap();
    object.shift_remove("timeline");
    object.shift_remove("notes");
    if let Some(notes) = &analysis.notes {
        object.insert("notes".into(), Value::Object(transcription(notes, tempo)));
    }
    result
}
impl ListenTool {
    async fn locate(&self, named: &str, signal: Signal) -> Result<String, RuntimeError> {
        if let Some(resolve) = &self.options.resolve {
            if let Some(found) = resolve(named.into(), signal).await? {
                return Ok(found);
            }
        }
        Ok(audio_path(named))
    }
    fn event(&self, analysis: &Analysis, compared: Option<HeardComparison>) {
        (self.options.on_event)(HeardEvent {
            file: analysis.file.clone(),
            summary: summary(analysis),
            bands: analysis.balance.bands.iter().map(|b| b.db).collect(),
            compared,
        });
    }
}
enum ListenError {
    Audio(AudioError),
    Other(RuntimeError),
}
impl From<AudioError> for ListenError {
    fn from(e: AudioError) -> Self {
        Self::Audio(e)
    }
}
impl From<RuntimeError> for ListenError {
    fn from(e: RuntimeError) -> Self {
        Self::Other(e)
    }
}
fn error_result(error: ListenError, prefix: &str) -> ToolResult {
    ToolResult::error(match error {
        ListenError::Audio(error) => error.0,
        ListenError::Other(error) => format!("{prefix}{}", head(&error.to_string(), 200)),
    })
}
#[async_trait(?Send)]
impl KernelTool for ListenTool {
    fn name(&self) -> &str {
        LISTEN_TOOL
    }
    fn description(&self) -> &str {
        DESCRIPTION
    }
    fn input_schema(&self) -> JsonObject {
        serde_json::from_str(include_str!("listen-schema.json")).expect("listen schema")
    }
    async fn execute(&self, input: JsonObject, signal: Signal) -> Result<ToolResult, RuntimeError> {
        let number = |key: &str| input.get(key).and_then(Value::as_f64);
        let text = |key: &str| input.get(key).and_then(Value::as_str);
        let yes = |key: &str| input.get(key).and_then(Value::as_bool) == Some(true);
        if yes("form") {
            let file = text("file").unwrap_or("");
            let result:Result<_,ListenError>=async{let path=self.locate(file,signal.clone()).await?;let form=hear_form(&path,FormOptions{tempo:number("tempo"),beats_per_bar:number("beats_per_bar"),signal:Some(signal.clone())}).await?;Ok(ToolResult::text(stringify(&json!({"form":form,"note":"Bars are the reference's own, counted at its tempo. To mirror it, arrange with these sections (their bars, names fitting the genre) and pick which tracks play in each by its energy, density and low end."}))))}.await;
            return match result {
                Ok(result) => Ok(result),
                Err(error) => {
                    signal.check()?;
                    Ok(error_result(error, "Kumi couldn't hear its form: "))
                }
            };
        }
        let focus = text("focus").filter(|v| *v == "mix" || *v == "sound").map(str::to_string);
        let tempo = number("tempo");
        let mut named = Vec::new();
        if let Some(track) = text("track") {
            named.push(track.to_string());
        }
        if let Some(tracks) = input.get("tracks").and_then(Value::as_array) {
            named.extend(tracks.iter().filter_map(Value::as_str).map(str::to_string));
        }
        let mut file = text("file").unwrap_or("").to_string();
        let (mut from, mut seconds) = (None, None);
        if !named.is_empty() || yes("mix") {
            let Some(hear_set) = &self.options.hear else {
                return Ok(ToolResult::error("Kumi isn't connected to Live, so it can't hear the Set."));
            };
            let mut tracks = Vec::new();
            for track in named {
                if !tracks.contains(&track) {
                    tracks.push(track);
                }
            }
            let request = HearRequest {
                tracks,
                mix: if yes("mix") { Some(true) } else { None },
                from_beat: number("from_beat"),
                beats: number("beats"),
                seconds: number("seconds"),
            };
            let takes = match hear_set(request, signal.clone()).await? {
                Ok(takes) => takes,
                Err(message) => return Ok(ToolResult::error(message)),
            };
            if takes.len() > 1 {
                return heard_together(&takes, focus, self.options.on_event.clone(), signal).await;
            }
            let Some(take) = takes.first() else {
                return Ok(ToolResult::error("Nothing came through to hear."));
            };
            file = take.file.clone();
            from = Some(take.start);
            seconds = take.seconds;
        }
        if file.is_empty() {
            return Ok(ToolResult::error("Name what to hear: a file, an audio clip's clipRef, a track (or tracks), or mix: true."));
        }
        let from_set = from.is_some();
        let common = AnalyzeOptions {
            focus,
            transcribe: yes("transcribe"),
            start: if from_set { from } else { number("from_seconds") },
            seconds: if from_set { seconds } else { number("seconds") },
            signal: Some(signal.clone()),
        };
        let result:Result<ToolResult,ListenError>=async{let path=self.locate(&file,signal.clone()).await?;let mine=hear(&path,common.clone()).await?;let Some(compare_to)=text("compare_to")else{self.event(&mine,None);return Ok(ToolResult::text(stringify(&trim_sound(&mine,tempo))));};let path=self.locate(compare_to,signal.clone()).await?;let reference=hear(&path,AnalyzeOptions{start:number("compare_from_seconds"),transcribe:false,focus:Some(mine.analyzed.focus.clone()),..common}).await?;let comparison=compare(&mine,&reference);self.event(&mine,Some(HeardComparison{reference:reference.file.clone(),summary:summary(&reference),differences:comparison.balance.iter().map(|b|b.difference).collect(),headlines:comparison.headlines.clone()}));let mut mine_value=json!({"loudness":mine.loudness,"tempo":mine.tempo,"key":mine.key});if let Some(notes)=&mine.notes{mine_value["notes"]=Value::Object(transcription(notes,tempo));}Ok(ToolResult::text(stringify(&json!({"comparison":comparison,"mine":mine_value,"reference":{"loudness":reference.loudness,"tempo":reference.tempo,"key":reference.key}}))))}.await;
        match result {
            Ok(result) => Ok(result),
            Err(error) => {
                signal.check()?;
                Ok(error_result(error, "Kumi couldn't listen to that: "))
            }
        }
    }
}
async fn heard_together(takes: &[HeardTake], focus: Option<String>, on_event: OnHeard, signal: Signal) -> Result<ToolResult, RuntimeError> {
    let heard = try_join_all(takes.iter().map(|take| {
        let signal = signal.clone();
        let focus = focus.clone();
        async move {
            let analysis = hear(
                &take.file,
                AnalyzeOptions {
                    start: Some(take.start),
                    seconds: take.seconds,
                    focus: Some(focus.unwrap_or_else(|| "mix".into())),
                    signal: Some(signal),
                    ..Default::default()
                },
            )
            .await
            .map_err(|e| RuntimeError::plain(e.0))?;
            Ok::<_, RuntimeError>((take, analysis))
        }
    }))
    .await?;
    for (_, analysis) in &heard {
        on_event(HeardEvent {
            file: analysis.file.clone(),
            summary: summary(analysis),
            bands: analysis.balance.bands.iter().map(|b| b.db).collect(),
            compared: None,
        });
    }
    let level = |a: &Analysis, index: usize| a.loudness.integrated_lufs.map_or(f64::NEG_INFINITY, |l| l + a.balance.bands[index].db);
    let strong = |a: &Analysis, index: usize| {
        a.balance.bands[index].db >= a.balance.bands.iter().map(|b| b.db).fold(f64::NEG_INFINITY, f64::max) - 6.0
    };
    let mut clashes = Vec::new();
    for a in 0..heard.len() {
        for b in a + 1..heard.len() {
            let (first, second) = (&heard[a], &heard[b]);
            for (index, band) in first.1.balance.bands.iter().enumerate() {
                let x = level(&first.1, index);
                let y = level(&second.1, index);
                if x.is_finite() && y.is_finite() && strong(&first.1, index) && strong(&second.1, index) && (x - y).abs() <= 6.0 {
                    let x = round(x * 10.0) / 10.0;
                    let y = round(y * 10.0) / 10.0;
                    clashes.push((
                        format!(
                            "{} and {} both sit in the {} ({} Hz), at {} and {} dB",
                            first.0.label,
                            second.0.label,
                            band.name,
                            band.hz,
                            to_string(x),
                            to_string(y)
                        ),
                        x.max(y),
                    ));
                }
            }
        }
    }
    clashes.sort_by(|a, b| b.1.total_cmp(&a.1));
    let tracks:Vec<_>=heard.iter().map(|(take,a)|json!({"track":take.label,"heard":if take.live{"as it played"}else{"quietly"},"summary":summary(a),"loudness":a.loudness,"balance":a.balance.bands.iter().map(|b|format!("{} {} dB",b.name,to_string(b.db))).collect::<Vec<_>>(),"width":a.balance.bands.iter().map(|b|b.width).collect::<Vec<_>>(),"dynamics":a.dynamics})).collect();
    Ok(ToolResult::text(stringify(
        &json!({"tracks":tracks,"clashes":clashes.into_iter().take(8).map(|c|c.0).collect::<Vec<_>>(),"note":"A clash is two sounds strong in the same band at similar levels: carve one (EQ Eight), sidechain it (a compressor keyed from the other), or move one up or down an octave."}),
    )))
}
/// The short line an app shows for a mix or a single sound.
pub fn summary(analysis: &Analysis) -> String {
    let mut parts = Vec::new();
    if let Some(sound) = &analysis.sound {
        parts.push(sound.pitch.as_ref().map_or_else(|| "unpitched".into(), |p| p.note.clone()));
        if let Some(h) = &sound.harmonics {
            parts.push(h.shape.split(" (").next().unwrap_or(&h.shape).into());
        }
        parts.push(format!("attack {} ms", to_string(sound.envelope.attack_ms)));
        if let Some(lfo) = &sound.movement.lfo {
            parts.push(format!("{} Hz {} LFO", to_string(lfo.hz), lfo.on.split(' ').next().unwrap_or(&lfo.on)));
        }
    } else {
        parts.push(analysis.loudness.integrated_lufs.map_or_else(|| "silent".into(), |v| format!("{} LUFS", to_string(v))));
        if let Some(tempo) = &analysis.tempo {
            parts.push(format!("{} BPM", to_string(round(tempo.bpm))));
        }
        if let Some(key) = &analysis.key {
            parts.push(key.name.clone());
        }
    }
    parts.join(" · ")
}
