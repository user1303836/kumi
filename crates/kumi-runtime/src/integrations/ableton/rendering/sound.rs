//! The sound tool: tracks heard quietly side by side in one pass, each sound measured, a kit judged as one, how each
//! part ducks under and interlocks with another, and how far its notes sit from a key.

use super::super::connection::NO_CURRENT_LIVE;
use super::rig::Window;
use super::*;
use crate::listening::{
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
        let heard = match self.hear_tracks(&heard_names, window, signal).await? {
            Ok(heard) => heard,
            Err(why) => return Ok(Err(why)),
        };
        let trigger = against.as_ref().and_then(|name| heard.get(name));
        let beat_ms = 60_000. / tempo;
        let mut sounds = serde_json::Map::new();
        let mut pieces: Vec<(String, Heard)> = vec![];
        for name in &names {
            let Some(sound) = heard.get(name) else {
                sounds.insert(name.clone(), json!("nothing came through from it there"));
                continue;
            };
            let mut said = described(sound, beat_ms);
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

/// One sound's measures and problems, as the model reads them.
fn described(heard: &Heard, beat_ms: f64) -> Value {
    let m = &heard.measures;
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
    if m.crackle > 0. {
        said.insert("crackle".into(), json!(format!("{} a second", m.crackle)));
    }
    let found = problems(heard);
    if !found.is_empty() {
        said.insert("problems".into(), json!(found));
    }
    Value::Object(said)
}

fn round2(value: f64) -> f64 {
    (value * 100.).round() / 100.
}
