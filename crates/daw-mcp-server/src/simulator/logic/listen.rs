//! (M5) Listening, as the bridge's methods `hear`, `audition` and `render` make them over the real Logic
//! (`crates/logic-kumi/src/listen`, the contract's Listening section): a part played while the helper's process tap
//! captures what Logic outputs, cut to the part and written as a WAV file into the folder the request names (`hear`:
//! `kumi-hear-<n>.wav`); one such pass for each candidate, a track soloed alone or the whole mix (`audition`:
//! `kumi-audition-<n>-<k>.wav`); a part bounced offline through Logic's Bounce, a track soloed for it (`render`:
//! `kumi-render-<n>.wav`). None is a change: no row, no step of Logic's undo, and Logic is as it was after (its
//! playhead, solos, metronome and cycle). Logic playing is `busy`; a part is at least a sixteenth, a pass at most 600 s;
//! an audition hears 1 to 16 candidates; a request's `epoch`, when it gives one, must be the project's.
//!
//! The simulated Logic synthesizes what the real one would output. Each track (its own channel strip, not a Track
//! Stack's) plays a sine of its own, its pitch set by its place in the Tracks area (110 Hz times one more than its
//! place: 220 Hz for track 1, 330 Hz for track 2), at −20 dBFS, through its fader, its pan (Logic's −3 dB pan law: both
//! sides at unity in the middle) and its strip's plug-ins as far as they change the level (`Plugin::stage`: a Gain's
//! gain, balance, phase, swap and mono; a Limiter's and an Adaptive Limiter's gain, then their ceiling), then the
//! Stereo Out's plug-ins. A pass solos exactly the tracks it asks for (none: the whole mix, no track soloed), and a
//! muted track stays silent soloed or not, which its take's note says. A track sounds through the whole part whatever
//! regions it has, so every track has something to hear. A capture and a render are 32-bit float stereo at 44.1 kHz
//! (the default output's rate on the lab's Mac, and Scratch's own); each answer says what its file holds (its length,
//! rate, channels, and a capture's peak and RMS of full scale), and a capture is cut to the part, so its lead is 0.
use super::render::MOST_BEATS;
use super::{bad, failed, Answer, Fault, State};
use serde_json::{json, Value};
use std::io::Write;
use std::path::{Path, PathBuf};

/// The rate a capture and a render are written at, and their channels.
const RATE: u32 = 44_100;
const CHANNELS: u16 = 2;
/// A track's sine at its fader's 0 dB, centred: −20 dBFS.
const LEVEL: f64 = 0.1;
/// The lowest tone, which each track's place multiplies.
const BASE_HZ: f64 = 110.0;
/// The shortest part (a sixteenth), the longest pass, and the most candidates an audition hears: the bridge's.
const LEAST_BEATS: f64 = 0.25;
const MOST_SECONDS: f64 = 600.0;
const MOST_CANDIDATES: usize = 16;
/// What an answer says of the metronome, as the bridge's `metronome_word` says it: Scratch's is on (as the bridge's
/// scenes recorded it), so it is off for the pass and on again after; one the producer (or Kumi's `metronome-set`)
/// turned off stays off.
fn metronome_word(was_on: bool) -> &'static str {
    if was_on {
        "off for the pass"
    } else {
        "off"
    }
}

/// One pass: the tracks soloed alone for it, by index (none: the whole mix, no track soloed).
struct Pass {
    solo: Vec<usize>,
}

/// What a written file holds.
struct Written {
    file: PathBuf,
    frames: u64,
    peak: f64,
    rms: f64,
}

/// A number to a few places, as the bridge rounds what it answers.
fn round(value: f64, places: i32) -> f64 {
    let scale = 10f64.powi(places);
    (value * scale).round() / scale
}

/// The part a request asks for: `fromBeats`, `beats` (at least a sixteenth), `folder` (a whole path to a folder that
/// is there) and `mute`, in the bridge's words.
fn part(params: &Value) -> Result<(f64, f64, PathBuf, bool), Fault> {
    let number = |key: &str| -> Result<Option<f64>, Fault> {
        match params.get(key) {
            None | Some(Value::Null) => Ok(None),
            Some(value) => value
                .as_f64()
                .filter(|n| n.is_finite() && (0.0..=1.0e6).contains(n))
                .map(Some)
                .ok_or_else(|| bad(format!("{key} must be a number from 0"))),
        }
    };
    let from = number("fromBeats")?.ok_or_else(|| bad("fromBeats: where the part starts, in beats from the project's start"))?;
    let length = number("beats")?.ok_or_else(|| bad("beats: how long the part is, in beats"))?;
    if length < LEAST_BEATS {
        return Err(bad("beats must be at least 0.25"));
    }
    let text = params
        .get("folder")
        .and_then(Value::as_str)
        .filter(|folder| !folder.is_empty())
        .ok_or_else(|| bad("folder: a whole path to the folder the audio files go into (it must be there)"))?;
    let folder = PathBuf::from(text);
    if !folder.is_absolute() || folder.components().any(|c| matches!(c, std::path::Component::ParentDir)) {
        return Err(bad("folder must be a whole path, with no \"..\" in it"));
    }
    if !folder.is_dir() {
        return Err(bad(format!("the folder {} isn't there (or Kumi can't read it); Kumi makes no folders", folder.display())));
    }
    let mute = match params.get("mute") {
        None | Some(Value::Null) => false,
        Some(Value::Bool(mute)) => *mute,
        Some(_) => return Err(bad("mute must be true or false")),
    };
    Ok((from, length, folder, mute))
}

/// The files of a run of `passes` passes named `stem`, numbered past those in the folder: a pass's alone is
/// `kumi-<stem>-<n>.wav`, several are `kumi-<stem>-<n>-<k>.wav` (and the capture they're cut from, `kumi-tap-<n>.wav`,
/// is never left behind).
fn files(folder: &Path, stem: &str, passes: usize) -> Vec<PathBuf> {
    let named = |n: usize| -> Vec<PathBuf> {
        if passes == 1 {
            vec![folder.join(format!("kumi-{stem}-{n}.wav"))]
        } else {
            (1..=passes).map(|k| folder.join(format!("kumi-{stem}-{n}-{k}.wav"))).collect()
        }
    };
    (1..)
        .map(|n| (n, named(n)))
        .find(|(n, parts)| !folder.join(format!("kumi-tap-{n}.wav")).exists() && parts.iter().all(|part| !part.exists()))
        .map(|(_, parts)| parts)
        .expect("a free name")
}

/// How a capture was cut to the beats, as the bridge says it: the simulated capture is exact.
fn aligned() -> Value {
    json!({"by": "timecode+click", "latencyMs": 0.0, "errorMs": 0.0})
}

impl State {
    /// A request's `epoch`, when it gives one, against the project's.
    fn same_epoch_if_given(&self, params: &Value) -> Result<(), Fault> {
        match params.get("epoch") {
            None | Some(Value::Null) => Ok(()),
            Some(Value::String(epoch)) if *epoch == self.epoch_text() => Ok(()),
            Some(epoch) => Err((
                "stale".into(),
                format!("the epoch {epoch} is an earlier one (now {}): the tracks or regions changed, read again", self.epoch),
            )),
        }
    }

    /// A track ref of this epoch: its index.
    fn track_of(&self, reference: &Value) -> Result<usize, Fault> {
        let text = reference.as_str().ok_or_else(|| bad("ref must be a track ref"))?;
        self.track_at(Some(text), false)
    }

    /// Logic as a pass finds it: stopped (playing or recording is `busy`).
    fn stopped(&self) -> Result<(), Fault> {
        if self.playing {
            return Err(("busy".into(), "Logic Pro is playing: Kumi listens only while it is stopped; nothing changed".into()));
        }
        Ok(())
    }

    /// How long a part plays, through the Tempo List; a pass of more than 600 s is refused.
    fn pass_seconds(&self, from: f64, length: f64) -> Result<f64, Fault> {
        let seconds = self.seconds_to(from + length) - self.seconds_to(from);
        if seconds > MOST_SECONDS {
            return Err(bad(format!(
                "Kumi hears at most {MOST_SECONDS} s at once; {length} beats at {} BPM is {seconds:.0} s",
                self.tempo
            )));
        }
        Ok(seconds)
    }

    /// What a soloed take's note says of the tracks it soloed that the producer muted.
    fn muted_note(&self, pass: &Pass) -> Option<String> {
        let muted: Vec<&str> = pass.solo.iter().filter(|at| self.tracks[**at].mute).map(|at| self.tracks[*at].name.as_str()).collect();
        (!muted.is_empty()).then(|| {
            format!(
                "{} muted in Logic Pro, so soloing doesn't make it heard",
                muted.join(", ") + if muted.len() == 1 { " is" } else { " are" }
            )
        })
    }

    /// A stereo frame of the mix at `t` seconds into the part, as the pass hears it.
    fn frame(&self, voices: &[(f64, f64, f64, Vec<super::mixer::Stage>)], out: &[super::mixer::Stage], t: f64) -> (f64, f64) {
        let mut mix = (0.0, 0.0);
        for (hz, left, right, stages) in voices {
            let sine = LEVEL * (std::f64::consts::TAU * hz * t).sin();
            let voice = stages.iter().fold((sine * left, sine * right), |frame, stage| stage.apply(frame));
            mix = (mix.0 + voice.0, mix.1 + voice.1);
        }
        out.iter().fold(mix, |frame, stage| stage.apply(frame))
    }

    /// The tracks that sound in a pass (exactly those it solos, or every one for the whole mix; a muted one never), each
    /// as its tone, its gains left and right, and its strip's stages.
    fn voices(&self, pass: &Pass) -> Vec<(f64, f64, f64, Vec<super::mixer::Stage>)> {
        self.tracks
            .iter()
            .enumerate()
            .filter(|(at, track)| {
                !track.is_stack() && !track.mute && track.volume_db > super::MIN_DB && (pass.solo.is_empty() || pass.solo.contains(at))
            })
            .map(|(at, track)| {
                let gain = 10f64.powf(track.volume_db / 20.0);
                // Logic's pan law, −3 dB compensated: both sides at unity in the middle, the far side silent at the end.
                let pan = (track.pan_steps as f64 / super::PAN_CENTRE as f64).clamp(-1.0, 1.0);
                let angle = (pan + 1.0) * std::f64::consts::FRAC_PI_4;
                let (left, right) = (gain * std::f64::consts::SQRT_2 * angle.cos(), gain * std::f64::consts::SQRT_2 * angle.sin());
                let stages = track.channel.plugins.iter().map(|plugin| plugin.stage()).collect();
                (BASE_HZ * (at + 2) as f64, left, right, stages)
            })
            .collect()
    }

    /// One pass written as `file`: `seconds` of the mix as `pass` hears it, 32-bit float stereo.
    fn write_pass(&self, file: PathBuf, pass: &Pass, seconds: f64) -> Result<Written, Fault> {
        let voices = self.voices(pass);
        let out: Vec<_> =
            self.outputs.first().map(|output| output.channel.plugins.iter().map(|plugin| plugin.stage()).collect()).unwrap_or_default();
        let frames = (seconds * f64::from(RATE)).round() as u64;
        let block = u32::from(CHANNELS) * 4;
        let data = frames * u64::from(block);
        let written = (|| -> std::io::Result<(f64, f64)> {
            let handle = std::fs::OpenOptions::new().write(true).create_new(true).open(&file)?;
            let mut writer = std::io::BufWriter::with_capacity(1 << 16, handle);
            writer.write_all(b"RIFF")?;
            writer.write_all(&((36 + data) as u32).to_le_bytes())?;
            writer.write_all(b"WAVEfmt ")?;
            writer.write_all(&16u32.to_le_bytes())?;
            writer.write_all(&3u16.to_le_bytes())?;
            writer.write_all(&CHANNELS.to_le_bytes())?;
            writer.write_all(&RATE.to_le_bytes())?;
            writer.write_all(&(RATE * block).to_le_bytes())?;
            writer.write_all(&(block as u16).to_le_bytes())?;
            writer.write_all(&32u16.to_le_bytes())?;
            writer.write_all(b"data")?;
            writer.write_all(&(data as u32).to_le_bytes())?;
            let (mut peak, mut squares) = (0.0f64, 0.0f64);
            for n in 0..frames {
                let (left, right) = self.frame(&voices, &out, n as f64 / f64::from(RATE));
                for sample in [left as f32, right as f32] {
                    peak = peak.max(f64::from(sample.abs()));
                    squares += f64::from(sample) * f64::from(sample);
                    writer.write_all(&sample.to_le_bytes())?;
                }
            }
            writer.flush()?;
            let count = (frames * u64::from(CHANNELS)).max(1) as f64;
            Ok((peak, (squares / count).sqrt()))
        })();
        let (peak, rms) = written.map_err(|error| failed(format!("Kumi couldn't write {}: {error}", file.display())))?;
        Ok(Written { file, frames, peak, rms })
    }

    /// A take as the bridge answers it: its file and what it holds, and its note.
    fn take(&self, written: &Written, note: Option<String>) -> Value {
        let mut take = json!({
            "file": written.file.to_string_lossy(), "seconds": round(written.frames as f64 / f64::from(RATE), 4),
            "sampleRate": RATE, "channels": CHANNELS, "lead": 0.0, "peak": round(written.peak, 6), "rms": round(written.rms, 6),
        });
        if let Some(note) = note {
            take["note"] = note.into();
        }
        take
    }

    /// `hear {fromBeats, beats, folder, mute?, solo?: [track refs], epoch?}`: one pass, the tracks in `solo` soloed
    /// alone (none: the whole mix), captured into `kumi-hear-<n>.wav`.
    pub(super) fn hear(&mut self, params: &Value) -> Answer {
        self.same_epoch_if_given(params)?;
        let (from, length, folder, mute) = part(params)?;
        let refs: Vec<Value> = match params.get("solo") {
            None | Some(Value::Null) => vec![],
            Some(Value::Array(refs)) => refs.clone(),
            Some(_) => return Err(bad("solo must be a list of track refs")),
        };
        let solo = refs.iter().map(|reference| self.track_of(reference)).collect::<Result<Vec<usize>, Fault>>()?;
        self.stopped()?;
        let seconds = self.pass_seconds(from, length)?;
        let pass = Pass { solo };
        let file = files(&folder, "hear", 1).remove(0);
        let written = self.write_pass(file, &pass, seconds)?;
        let mut answer = self.take(&written, self.muted_note(&pass));
        answer["aligned"] = aligned();
        answer["fromBeats"] = from.into();
        answer["beats"] = length.into();
        answer["mute"] = mute.into();
        answer["solo"] = Value::Array(refs);
        answer["metronome"] = metronome_word(self.metronome).into();
        answer["epoch"] = self.epoch_text().into();
        Ok(answer)
    }

    /// `audition {candidates: [{ref} | {mix: true}], fromBeats, beats, folder, mute?, epoch?}`: one pass a candidate,
    /// each a track soloed alone or the whole mix; the takes in the candidates' order, each its candidate and name with
    /// what a hear's take holds.
    pub(super) fn audition(&mut self, params: &Value) -> Answer {
        self.same_epoch_if_given(params)?;
        let (from, length, folder, mute) = part(params)?;
        let candidates = params
            .get("candidates")
            .and_then(Value::as_array)
            .filter(|list| !list.is_empty())
            .ok_or_else(|| bad("candidates: a list of {\"ref\": <track ref>} or {\"mix\": true}"))?;
        if candidates.len() > MOST_CANDIDATES {
            return Err(bad(format!("Kumi auditions at most {MOST_CANDIDATES} candidates at once")));
        }
        let mut passes = vec![];
        for candidate in candidates {
            if candidate.get("mix") == Some(&Value::Bool(true)) {
                passes.push(Pass { solo: vec![] });
            } else if let Some(reference) = candidate.get("ref") {
                passes.push(Pass { solo: vec![self.track_of(reference)?] });
            } else {
                return Err(bad("each candidate is {\"ref\": <track ref>} or {\"mix\": true}"));
            }
        }
        self.stopped()?;
        let seconds = self.pass_seconds(from, length)?;
        let mut listed = vec![];
        for ((candidate, pass), file) in candidates.iter().zip(&passes).zip(files(&folder, "audition", passes.len())) {
            let written = self.write_pass(file, pass, seconds)?;
            let mut take = self.take(&written, self.muted_note(pass));
            match pass.solo.first() {
                Some(at) => {
                    take["ref"] = candidate["ref"].clone();
                    take["name"] = self.tracks[*at].name.clone().into();
                }
                None => {
                    take["mix"] = true.into();
                    take["name"] = "the whole mix".into();
                }
            }
            listed.push(take);
        }
        Ok(json!({
            "epoch": self.epoch_text(), "fromBeats": from, "beats": length, "mute": mute, "aligned": aligned(),
            "metronome": metronome_word(self.metronome), "candidates": listed,
        }))
    }

    /// `render {ref?, fromBeats, beats, folder, epoch?}`: the part bounced offline (a bounce: 32-bit float WAV at the
    /// project's rate), the track `ref` names soloed alone for it (so its audio passed the Stereo Out's plug-ins, which
    /// its note says); without one, the whole mix.
    pub(super) fn render(&mut self, params: &Value) -> Answer {
        self.same_epoch_if_given(params)?;
        let (from, length, folder, _) = part(params)?;
        let track = match params.get("ref") {
            None | Some(Value::Null) => None,
            Some(reference) => Some(self.track_of(reference)?),
        };
        self.stopped()?;
        if length > MOST_BEATS {
            return Err(bad(format!("Kumi bounces at most {MOST_BEATS} beats at once")));
        }
        let seconds = self.seconds_to(from + length) - self.seconds_to(from);
        let file = (1..).map(|n| folder.join(format!("kumi-render-{n}.wav"))).find(|path| !path.exists()).expect("a free name");
        let written = self.write_pass(file, &Pass { solo: track.into_iter().collect() }, seconds)?;
        let mut answer = json!({
            "epoch": self.epoch_text(), "file": written.file.to_string_lossy(), "seconds": written.frames as f64 / f64::from(RATE),
            "sampleRate": RATE, "channels": CHANNELS, "bits": 32, "lead": 0.0, "fromBeats": from, "beats": length,
            "metronome": metronome_word(self.metronome),
        });
        if let Some(reference) = params.get("ref").filter(|reference| !reference.is_null()) {
            answer["ref"] = reference.clone();
            answer["note"] = "The track was soloed alone for the bounce, so its audio passed the Stereo Out's plug-ins.".into();
        }
        Ok(answer)
    }
}

#[cfg(test)]
mod tests {
    use super::super::LogicScript;
    use serde_json::{json, Value};

    /// A WAV file's frames as numbers: its rate, its channels and each sample (32-bit float), from its header.
    fn samples(path: &str) -> (u32, u16, Vec<f64>) {
        let bytes = std::fs::read(path).unwrap();
        let rate = u32::from_le_bytes(bytes[24..28].try_into().unwrap());
        let channels = u16::from_le_bytes(bytes[22..24].try_into().unwrap());
        let (code, bits) = (u16::from_le_bytes(bytes[20..22].try_into().unwrap()), u16::from_le_bytes(bytes[34..36].try_into().unwrap()));
        assert_eq!((code, bits), (3, 32), "32-bit float");
        (rate, channels, bytes[44..].chunks(4).map(|c| f64::from(f32::from_le_bytes(c.try_into().unwrap()))).collect())
    }

    fn rms(values: &[f64]) -> f64 {
        (values.iter().map(|v| v * v).sum::<f64>() / values.len() as f64).sqrt()
    }

    #[test]
    fn a_capture_holds_each_tracks_tone_through_its_fader_and_the_stereo_outs_gain() {
        let dir = tempfile::tempdir().unwrap();
        let folder = dir.path().to_string_lossy().into_owned();
        let script = LogicScript::new();
        let answer = |method: &str, params: Value| script.answer(method, &params).expect("a method the simulator has");
        let heard = answer("hear", json!({"epoch": "1", "fromBeats": 0, "beats": 16, "folder": folder})).unwrap();
        // 16 beats at 160 BPM: 6 s at 44.1 kHz, two sines of −20 dBFS.
        assert_eq!(
            (heard["seconds"].as_f64(), heard["sampleRate"].as_u64(), heard["channels"].as_u64()),
            (Some(6.0), Some(44_100), Some(2))
        );
        assert_eq!(
            (heard["lead"].as_f64(), heard["epoch"].as_str(), heard["metronome"].as_str(), heard["solo"].clone(), heard["mute"].clone()),
            (Some(0.0), Some("1"), Some("off for the pass"), json!([]), json!(false))
        );
        assert_eq!(heard["aligned"]["by"], "timecode+click");
        assert!(heard["file"].as_str().unwrap().ends_with("/kumi-hear-1.wav"), "{heard}");
        let (rate, channels, values) = samples(heard["file"].as_str().unwrap());
        assert_eq!((rate, channels, values.len()), (44_100, 2, 6 * 44_100 * 2));
        let measured = rms(&values);
        assert!((measured - heard["rms"].as_f64().unwrap()).abs() < 1e-5 && (measured - 0.1).abs() < 0.002, "{measured} {heard}");
        assert!(heard["peak"].as_f64().unwrap() <= 0.2 + 1e-6, "{heard}");

        // Keys alone, soloed for the pass: one sine, at half the power; the solo put back.
        let keys = answer("hear", json!({"fromBeats": 0, "beats": 4, "folder": folder, "solo": ["1:track:2"]})).unwrap();
        let alone = rms(&samples(keys["file"].as_str().unwrap()).2);
        assert!((alone - 0.1 / 2f64.sqrt()).abs() < 0.001, "{alone}");
        assert!(keys["file"].as_str().unwrap().ends_with("/kumi-hear-2.wav") && keys["solo"] == json!(["1:track:2"]));
        assert!(!script.track(2)["solo"].as_bool().unwrap(), "the solo is put back");

        // A Gain on the Stereo Out at -6 dB turns the whole capture down 6 dB; Vocal muted leaves Keys alone, and a pass
        // that solos Vocal says it stays silent.
        let change = |kind: &str, input: Value| {
            let preview = answer("change_preview", json!({"kind": kind, "input": input, "epoch": "1"})).unwrap();
            let apply = json!({"kind": kind, "input": input, "fence": preview["fence"], "desc": format!("Kumi: {kind}"), "epoch": "1"});
            answer("change_apply", apply).unwrap()
        };
        let added = change("fx-add", json!({"ref": "1:output:1", "name": "Gain"}));
        let gain = added["created"]["ref"].as_str().unwrap().to_string();
        change("fx-param", json!({"ref": gain, "param": "Gain", "value": -6}));
        let quieter = answer("hear", json!({"epoch": "1", "fromBeats": 0, "beats": 4, "folder": folder})).unwrap();
        let ratio = 20.0 * (rms(&samples(quieter["file"].as_str().unwrap()).2) / measured).log10();
        assert!((ratio + 6.0).abs() < 0.05, "{ratio} dB");
        script.set_track(1, "mute", json!(true));
        let muted = answer("hear", json!({"epoch": "1", "fromBeats": 0, "beats": 4, "folder": folder})).unwrap();
        let level = rms(&samples(muted["file"].as_str().unwrap()).2);
        assert!((20.0 * (level / alone).log10() + 6.0).abs() < 0.05, "Keys alone, 6 dB down: {level}");
        let vocal = answer("hear", json!({"fromBeats": 0, "beats": 4, "folder": folder, "solo": ["1:track:1"]})).unwrap();
        assert_eq!(vocal["note"], "Vocal is muted in Logic Pro, so soloing doesn't make it heard");
        assert_eq!(vocal["peak"].as_f64(), Some(0.0), "silent");
    }

    #[test]
    fn an_audition_takes_a_pass_per_candidate_and_a_render_is_a_bounce() {
        let dir = tempfile::tempdir().unwrap();
        let folder = dir.path().to_string_lossy().into_owned();
        let script = LogicScript::new();
        let answer = |method: &str, params: Value| script.answer(method, &params).expect("a method the simulator has");
        let audition = answer(
            "audition",
            json!({"epoch": "1", "fromBeats": 4, "beats": 8, "folder": folder, "mute": true, "candidates": [{"ref": "1:track:1"}, {"ref": "1:track:2"}, {"mix": true}]}),
        )
        .unwrap();
        let takes = audition["candidates"].as_array().unwrap();
        assert_eq!(
            takes.iter().map(|take| (take["ref"].as_str(), take["mix"].as_bool(), take["name"].as_str().unwrap())).collect::<Vec<_>>(),
            [(Some("1:track:1"), None, "Vocal"), (Some("1:track:2"), None, "Keys"), (None, Some(true), "the whole mix")]
        );
        assert!(takes.iter().all(|take| take["seconds"] == 3.0 && take["sampleRate"] == 44_100), "{audition}");
        assert_eq!((audition["mute"].clone(), audition["aligned"]["by"].clone()), (json!(true), json!("timecode+click")));
        let names: Vec<&str> = takes.iter().map(|take| take["file"].as_str().unwrap().rsplit('/').next().unwrap()).collect();
        assert_eq!(names, ["kumi-audition-1-1.wav", "kumi-audition-1-2.wav", "kumi-audition-1-3.wav"]);
        let render = answer("render", json!({"epoch": "1", "ref": "1:track:2", "fromBeats": 0, "beats": 16, "folder": folder})).unwrap();
        assert_eq!(
            (render["seconds"].as_f64(), render["sampleRate"].as_u64(), render["channels"].as_u64(), render["bits"].as_u64()),
            (Some(6.0), Some(44_100), Some(2), Some(32))
        );
        assert!(render["note"].as_str().is_some_and(|note| note.contains("the Stereo Out's plug-ins")), "{render}");
        assert!(render["file"].as_str().unwrap().ends_with("/kumi-render-1.wav"));
        let (rate, _, values) = samples(render["file"].as_str().unwrap());
        assert_eq!((rate, values.len()), (44_100, 6 * 44_100 * 2));
        assert!((rms(&values) - 0.1 / 2f64.sqrt()).abs() < 0.001);

        // Refused before anything is written, in the bridge's words.
        for (params, code, words) in [
            (json!({"epoch": "0", "fromBeats": 0, "beats": 4, "folder": folder}), "stale", "the epoch \"0\" is an earlier one (now 1)"),
            (json!({"fromBeats": 0, "beats": 4, "folder": format!("{folder}/missing")}), "bad-params", "Kumi makes no folders"),
            (json!({"fromBeats": 0, "beats": 0.1, "folder": folder}), "bad-params", "beats must be at least 0.25"),
            (json!({"fromBeats": 0, "beats": 4000, "folder": folder}), "bad-params", "Kumi hears at most 600 s at once"),
            (json!({"fromBeats": 0, "beats": 4, "folder": folder, "solo": ["1:track:9"]}), "failed", "isn't in the project"),
            (json!({"fromBeats": 0, "beats": 4, "folder": folder, "mute": "yes"}), "bad-params", "mute must be true or false"),
        ] {
            let (got, said) = answer("hear", params.clone()).unwrap_err();
            assert!(got == code && said.contains(words), "{params}: {got} {said}");
        }
        let many: Vec<Value> = (0..17).map(|_| json!({"mix": true})).collect();
        let refused = answer("audition", json!({"fromBeats": 0, "beats": 4, "folder": folder, "candidates": many})).unwrap_err();
        assert_eq!(refused.1, "Kumi auditions at most 16 candidates at once");
        answer("transport_play", json!({})).unwrap();
        for method in ["hear", "render"] {
            let busy = answer(method, json!({"epoch": "1", "fromBeats": 0, "beats": 4, "folder": folder})).unwrap_err();
            assert_eq!(
                busy,
                ("busy".to_string(), "Logic Pro is playing: Kumi listens only while it is stopped; nothing changed".to_string())
            );
        }
    }
}
