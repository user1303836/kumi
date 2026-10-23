//! (M5) Logic Pro heard through the host: `hear` (a part played, and what Logic outputs captured into a file),
//! `audition` (such a pass for each candidate, one after the other) and `render` (a part bounced offline), forwarded to
//! the bridge with the fields each takes. Audio travels as a file the bridge writes into the folder Kumi names: an
//! answer carries its path, never its samples (an answer is at most 256 KB). A pass plays in real time, so its call
//! lasts as long as the part does plus a margin: the host's bound is the part's length at the tempo Logic plays at now
//! (its focus says), with room for a slower stretch of the Tempo List, plus what each pass takes besides (the playhead
//! and the solos put in place and back, the tap started and stopped, the capture cut); a render's bounce is offline,
//! never slower than the part plays, plus its dialogs. It covers the bridge's own wait (`listen::bound_seconds`: a
//! hear's part and 28 s, an audition's parts and 8 s each and 20 s, a render's part and 45 s), which the request's
//! deadline then sets. None is a change: no journal, no undo.
use crate::host::{Host, Pick};
use serde_json::{json, Value};
use std::time::Duration;

/// What one pass takes besides the part itself.
const PASS_MARGIN: Duration = Duration::from_secs(20);
/// What a call takes besides its passes (Logic made ready, then put back).
const CALL_MARGIN: Duration = Duration::from_secs(15);
/// A bounce's dialog and save panel, and its file found on disk, besides the part.
const RENDER_MARGIN: Duration = Duration::from_secs(45);
/// How much slower than the tempo now a stretch of the part may play (a slower tempo further on).
const SLOWER: f64 = 1.25;
/// The tempo taken when Logic doesn't say: a slow one, so the bound is long enough.
const SLOWEST_TEMPO: f64 = 30.0;
/// The longest part Kumi hears at once (an hour), and the most candidates an audition hears (the bridge's 16).
const LONGEST_PART: f64 = 3600.0;
const MOST_PASSES: u32 = 16;
/// How long the focus that says the tempo may take (it answers from a cache).
const FOCUS_TIMEOUT: Duration = Duration::from_secs(2);
/// No call is bounded below a change's 30 s.
const SHORTEST: Duration = crate::changes::CHANGE_TIMEOUT;

/// How long a listening call may take: `passes` passes of a part `part` long at the tempo now (`hear`'s one, an
/// `audition`'s one per candidate), or a `render`'s bounce of it.
pub fn bound(method: &str, part: Duration, passes: u32) -> Duration {
    let part = Duration::from_secs_f64(part.as_secs_f64().min(LONGEST_PART) * SLOWER);
    let bound = match method {
        "render" => part + RENDER_MARGIN,
        _ => (part + PASS_MARGIN) * passes.clamp(1, MOST_PASSES) + CALL_MARGIN,
    };
    bound.max(SHORTEST)
}

impl Host {
    /// `hear`, `audition` or `render` (Logic's): the fields the method takes go on to the bridge, the instance pick and
    /// anything else don't; the call is bounded by the part's length (`bound`). Never asked again on a new connection:
    /// it plays and writes a file.
    pub async fn listen(&self, method: &str, arguments: &Value) -> (Value, bool) {
        if self.daw() != daw_bridge::Daw::Logic {
            return (json!({"error": format!("{method} is Logic Pro's"), "code": "bad-params"}), true);
        }
        let pick = match Pick::from_arguments(arguments) {
            Ok(pick) => pick,
            Err(why) => return (json!({"error": why, "code": "bad-params"}), true),
        };
        let fields: &[&str] = match method {
            "hear" => &["fromBeats", "beats", "folder", "mute", "solo", "epoch"],
            "audition" => &["candidates", "fromBeats", "beats", "folder", "mute", "epoch"],
            _ => &["ref", "fromBeats", "beats", "folder", "epoch"],
        };
        let mut params = json!({});
        for field in fields {
            if let Some(value) = arguments.get(*field) {
                params[*field] = value.clone();
            }
        }
        let Some(beats) = arguments.get("beats").and_then(Value::as_f64).filter(|beats| beats.is_finite() && *beats > 0.0) else {
            return (json!({"error": "beats must be a number above 0: how long the part is, in beats", "code": "bad-params"}), true);
        };
        let passes = match method {
            "audition" => arguments.get("candidates").and_then(Value::as_array).map_or(1, |candidates| candidates.len() as u32),
            _ => 1,
        };
        let tempo = self.tempo_now(&pick).await;
        let part = Duration::from_secs_f64((beats * 60.0 / tempo).min(LONGEST_PART));
        match self.call(method, params, &pick, bound(method, part, passes)).await {
            Ok(answer) => (answer, false),
            Err(error) => (self.call_error(&error), true),
        }
    }

    /// The tempo Logic plays at now, as its focus says; a slow one when it doesn't say.
    async fn tempo_now(&self, pick: &Pick) -> f64 {
        match self.call("focus", json!({}), pick, FOCUS_TIMEOUT).await {
            Ok(focus) => focus
                .pointer("/transport/tempo")
                .and_then(Value::as_f64)
                .filter(|tempo| tempo.is_finite() && *tempo >= 1.0)
                .unwrap_or(SLOWEST_TEMPO),
            Err(_) => SLOWEST_TEMPO,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_call_lasts_its_passes_and_a_margin_and_never_less_than_a_change() {
        let six = Duration::from_secs(6);
        // Bars 1 to 5 at 160 BPM: 6 s, played in at most 7.5 s, plus a pass's and a call's margins.
        assert_eq!(bound("hear", six, 1), Duration::from_secs_f64(7.5 + 20.0 + 15.0));
        assert_eq!(bound("audition", six, 2), Duration::from_secs_f64(2.0 * 27.5 + 15.0));
        assert_eq!(bound("render", six, 1), Duration::from_secs_f64(7.5 + 45.0));
        assert_eq!(bound("hear", Duration::from_secs(1), 1), Duration::from_secs_f64(1.25 + 35.0));
        assert_eq!(bound("render", Duration::ZERO, 1), Duration::from_secs(45), "a render's dialogs at least");
        assert_eq!(bound("audition", six, 40), bound("audition", six, 16), "sixteen candidates at most");
        assert_eq!(bound("hear", Duration::from_secs(10_000), 1), Duration::from_secs_f64(4500.0 + 35.0), "an hour at most");
        assert!(bound("hear", Duration::ZERO, 0) >= SHORTEST);
    }
}
