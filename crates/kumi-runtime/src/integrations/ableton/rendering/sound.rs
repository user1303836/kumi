//! The sound tool: tracks heard quietly side by side in one pass, each sound measured, a kit judged as one, how each
//! part ducks under and interlocks with another, and how far its notes sit from a key.

use super::super::connection::NO_CURRENT_LIVE;
use super::rig::Window;
use super::*;
use crate::listening::{
    effects::{self, effects, note_value},
    measure::Heard,
    sound::{against_key, bandwidth, ducking, harmonics, interlock, key_classes, kit, noise_floor, problems, tail, width},
};

/// What the model asks of the sound tool.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct SoundRequest {
    pub tracks: Vec<String>,
    pub against: Option<String>,
    pub key: Option<String>,
    pub from_beat: Option<f64>,
    pub beats: Option<f64>,
}

impl Rendering {
    pub async fn sound(self: &Rc<Self>, request: &SoundRequest, original: Signal) -> Result<Result<Value, String>, RuntimeError> {
        if !self.available() {
            return Ok(Err(NO_CURRENT_LIVE.into()));
        }
        let Some(tempo) = self.observer.tempo.get().filter(|tempo| *tempo > 0.) else {
            return Ok(Err("Kumi doesn't know the Set's tempo yet; try again.".into()));
        };
        if self.rendering.get() {
            return Ok(Err("Kumi is already listening to something; wait for it.".into()));
        }
        let key = match &request.key {
            Some(words) => match key_classes(words) {
                Some(classes) => Some(classes),
                None => {
                    return Ok(Err(format!("“{words}” doesn't read as a key; say it like \"F# minor\", \"Bb major\" or \"D dorian\".")))
                }
            },
            None => None,
        };
        let signal = abort::any([original, self.connection().lifetime.clone()]);
        let meter = self.observer.beats_per_bar.get().max(1.);
        // Eight bars from where it starts, unless asked.
        let window = Window { from: request.from_beat.unwrap_or(0.).max(0.), beats: request.beats.unwrap_or(8. * meter).max(meter) };
        if let Some(why) = super::listen::too_long(window.beats, self.observer.tempo.get().unwrap_or(120.), "hear a shorter part") {
            return Ok(Err(why));
        }
        // The tracks by name, and the one the others duck under (heard in the same pass).
        let mut names: Vec<String> = vec![];
        for named in &request.tracks {
            let name = self.track_name(named, signal.clone()).await.unwrap_or_else(|| named.clone());
            if !names.contains(&name) {
                names.push(name);
            }
        }
        let against = match &request.against {
            Some(named) => Some(self.track_name(named, signal.clone()).await.unwrap_or_else(|| named.clone())),
            None => None,
        };
        let mut heard_names = names.clone();
        if let Some(against) = against.as_ref().filter(|against| !names.contains(against)) {
            heard_names.push(against.clone());
        }
        let heard = match self.hear_takes(&heard_names, window, signal).await? {
            Ok(heard) => heard,
            Err(why) => return Ok(Err(why)),
        };
        let trigger = against.as_ref().and_then(|name| heard.get(name)).map(|(heard, _)| heard);
        let beat_ms = 60_000. / tempo;
        let mut sounds = serde_json::Map::new();
        let mut pieces: Vec<(String, Heard)> = vec![];
        for name in &names {
            let Some((sound, lead)) = heard.get(name) else {
                sounds.insert(name.clone(), json!("nothing came through from it there"));
                continue;
            };
            let mut said = described(sound, tempo, window.from, meter, *lead);
            if let (Some(trigger), Some(against)) = (trigger.filter(|_| against.as_ref() != Some(name)), against.as_ref()) {
                if let Some((depth, recovery)) = ducking(sound, trigger) {
                    said["ducks"] =
                        json!(format!("{depth} dB under {against}, back in {recovery} ms ({} of a beat)", round2(recovery / beat_ms)));
                }
                if let Some((colliding, between)) = interlock(sound, trigger) {
                    said["hits"] = json!(format!("{colliding}% land on {against}'s, {between}% between them"));
                }
            }
            if let Some((cents, off)) = key.as_ref().and_then(|key| against_key(sound, key)) {
                said["key"] = json!(format!("its notes sit {cents} cents from the key (median); {off}% of the time a quarter tone off"));
            }
            sounds.insert(name.clone(), said);
            pieces.push((name.clone(), sound.clone()));
        }
        let mut reply = json!({"sounds": sounds, "bars": format!("{}–{}", (window.from / meter) as usize + 1, ((window.from + window.beats) / meter).ceil() as usize)});
        if pieces.len() >= 2 {
            let (_, found) = kit(&pieces, tempo);
            reply["kit"] = json!(if found.is_empty() {
                vec!["the pieces hang together: no noise floor, top or grit apart, no pile-ups or gaps".to_string()]
            } else {
                found
            });
        }
        Ok(Ok(reply))
    }
}

/// One sound's measures, its effects and their problems, as the model reads them (heard from beat `from`, the take
/// starting `lead` seconds before it).
fn described(heard: &Heard, tempo: f64, from: f64, meter: f64, lead: f64) -> Value {
    let m = &heard.measures;
    let beat_ms = 60_000. / tempo;
    let bar = |seconds: f64| format!("bar {}", ((from + (seconds - lead).max(0.) * tempo / 60.) / meter).floor() as i64 + 1);
    let fx = effects(heard, Some(tempo), Some(lead + (from.ceil() - from) * beat_ms / 1000.));
    let mut said = serde_json::Map::new();
    let mut put = |key: &str, value: Option<String>| {
        if let Some(value) = value {
            said.insert(key.into(), json!(value));
        }
    };
    put("attack", m.attack.map(|ms| format!("{ms} ms")));
    put("decay", m.decay.map(|ms| format!("{ms} ms to 20 dB under ({} of a beat)", round2(ms / beat_ms))));
    put("sustain", m.sustain.map(|db| format!("{db} dB under its peak a quarter second on")));
    put("brightness", m.centroid.map(|hz| format!("{hz} Hz (centroid)")));
    put("noisiness", m.noise.map(|db| format!("{db} dB (0 is noise)")));
    put("pitch drop", m.pitch_drop.map(|st| format!("{st} semitones over its first 200 ms")));
    put("modulation", m.modulation.map(|hz| format!("{hz} Hz ({} beats a cycle)", round2(1000. / (hz * beat_ms)))));
    put(
        "warmth",
        harmonics(heard).map(|h| {
            format!(
                "2nd and 3rd harmonics {} dB against the fundamental ({} Hz); even against odd {} dB",
                h.warmth, h.fundamental, h.even_odd
            )
        }),
    );
    put("width", width(heard).map(|db| format!("{db} dB side against mid")));
    put("top", bandwidth(heard).map(|hz| format!("{} kHz", round2(hz / 1000.))));
    put("noise floor", noise_floor(heard).map(|db| format!("{db} dB under its loud parts")));
    put("tail", tail(heard).map(|db| format!("{db} dB under each hit 300 ms on")));
    // A reverb only when its tails fall in two slopes, the second slower; else only how fast it dies away.
    put(
        if fx.reverb { "reverb" } else { "decay (RT60)" },
        fx.decay_time.map(|seconds| {
            let mut words = if fx.reverb {
                format!("its tails decay in {seconds} s (RT60), slower than the sound's own fall")
            } else {
                format!("it falls 60 dB in {seconds} s")
            };
            if let Some(octaves) = fx.darkening.filter(|octaves| octaves.abs() >= 0.1) {
                words += &format!(", {} {} octaves", if octaves > 0. { "darkening" } else { "brightening" }, octaves.abs());
            }
            if let Some(db) = fx.widening.filter(|db| db.abs() >= 1.) {
                words += &format!(", {} {} dB", if db > 0. { "widening" } else { "narrowing" }, db.abs());
            }
            words
        }),
    );
    put(
        "echoes",
        fx.echo.map(|echo| {
            let (name, off) = note_value(echo.ms / 1000., tempo);
            let value = if off.abs() <= 0.03 { format!("a {name}") } else { format!("{} % off a {name}", (off * 100.).round()) };
            let darkens =
                echo.darkens.filter(|octaves| *octaves >= 0.1).map(|octaves| format!(" and {octaves} octaves darker")).unwrap_or_default();
            format!("every {} ms ({value}), {} repeats, each {} dB down{darkens}", echo.ms, echo.repeats, echo.falls)
        }),
    );
    put("swing", fx.swing.filter(|db| *db > 0.).map(|db| format!("{db} dB at its modulation rate")));
    put(
        "brightness moves",
        fx.sweep.filter(|octaves| *octaves >= 0.25).map(|octaves| {
            let cycle =
                fx.sweep_cycle.map(|seconds| format!(", once every {} beats", round2(seconds * 1000. / beat_ms))).unwrap_or_default();
            format!("over {octaves} octaves{cycle}")
        }),
    );
    put(
        "pumping",
        fx.pump.map(|pump| {
            let lowest = pump.lowest.map(|beats| format!(", deepest {} % into the beat", (beats * 100.).round())).unwrap_or_default();
            format!("dips {} dB once a beat{lowest}, back within {} % of it", pump.depth, (pump.back * 100.).round())
        }),
    );
    put(
        "tail share",
        fx.tail_share.map(|share| {
            let mut words = format!("{share} % of its energy comes after each hit's first 60 ms");
            let bars: Vec<(usize, f64)> = effects::tail_shares(heard, meter * beat_ms / 1000.)
                .iter()
                .enumerate()
                .filter_map(|(index, share)| Some((index, (*share)?)))
                .collect();
            let wettest = bars.iter().copied().max_by(|a, b| a.1.total_cmp(&b.1));
            let driest = bars.iter().copied().min_by(|a, b| a.1.total_cmp(&b.1));
            if let (Some((wet_at, wet)), Some((dry_at, dry))) = (wettest, driest) {
                if wet - dry >= 15. {
                    let first = (from / meter).floor() as usize + 1;
                    words += &format!(" (from {dry} % in bar {} to {wet} % in bar {})", first + dry_at, first + wet_at);
                }
            }
            words
        }),
    );
    if m.crackle > 0. {
        said.insert("crackle".into(), json!(format!("{} a second", m.crackle)));
    }
    let mut found = problems(heard);
    found.extend(effects::problems(heard, &fx, Some(tempo), &bar));
    if !found.is_empty() {
        said.insert("problems".into(), json!(found));
    }
    Value::Object(said)
}

fn round2(value: f64) -> f64 {
    (value * 100.).round() / 100.
}
