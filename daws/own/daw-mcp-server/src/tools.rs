//! The host's tools that read the DAW: `read`, `snapshot`, `focus` and `focus_body`. One implementation for every DAW:
//! each checks its arguments, asks the chosen bridge the method of the same name and hands the answer back, keeping
//! it inside the frame bound. What the rows and bodies hold is the DAW's own (see the M2 contract).
use crate::host::{CallError, Host, Pick, MAX_ANSWER_BYTES};
use daw_bridge::Daw;
use serde_json::{json, Map, Value};
use std::time::Duration;

/// A read, a snapshot or a body waits this long: the bridge answers a busy DAW at once, so this is for a slow project.
const READ_TIMEOUT: Duration = Duration::from_secs(15);
/// `focus` is polled; it never waits for the DAW.
const FOCUS_TIMEOUT: Duration = Duration::from_secs(3);
/// Rows in a page by default, and at most: the contract's.
pub const DEFAULT_LIMIT: u64 = 50;
pub const MAX_LIMIT: u64 = 200;
/// How many times a page the bridge made too big is asked for again with half the rows.
const SMALLER_PAGES: usize = 4;

/// What a DAW's `read` takes as a kind, and its `focus_body` as a body; and the change kinds the DAW's own undo never takes
/// back (they make no step in its history), so that its undo of a step never counts them, answered or not.
pub struct Kinds {
    pub read: &'static [&'static str],
    pub bodies: &'static [&'static str],
    pub outside_undo: &'static [&'static str],
}

pub fn kinds(daw: Daw) -> Kinds {
    match daw {
        Daw::Reaper => Kinds {
            read: &[
                "project", "track", "item", "take", "fx", "param", "send", "marker", "region", "note", "event", "envelope", "point",
                "autoitem", "call", "spans",
            ],
            bodies: &["arrange", "midi", "fx", "routing", "mixer"],
            // (r-tracks) The project's notes, grid and timebase make no step in REAPER's history either (7.82 lab).
            outside_undo: &[
                "loop-repeat",
                "fx-eq-band",
                "automation-override",
                "project-notes",
                "project-info",
                "project-grid",
                "project-timebase",
                // (m6-places) The metronome is a project setting REAPER keeps no history of.
                "metronome-switch",
            ],
        },
        Daw::Renoise => Kinds {
            // (an instrument's modulation sets and sample FX chains: n-instruments)
            read: &[
                "song",
                "track",
                "sequence",
                "pattern",
                "line",
                "automation",
                "instrument",
                "sample",
                "phrase",
                "device",
                "param",
                "modset",
                "chain",
                "spans",
            ],
            bodies: &["pattern", "matrix", "phrase", "instrument", "dsp", "mixer"],
            outside_undo: &[
                "track-mute",
                "track-solo",
                "loop-range",
                "loop-pattern",
                // M4 (n-song): the metronome switch and the editing settings (measured in 3.5.4)
                "metronome-switch",
                "edit-settings",
                "plugin-suspend",
            ],
        },
    }
}

fn text_arg(arguments: &Value, name: &str) -> Result<Option<String>, String> {
    match arguments.get(name) {
        None | Some(Value::Null) => Ok(None),
        Some(Value::String(text)) if !text.is_empty() => Ok(Some(text.clone())),
        Some(_) => Err(format!("{name} must be a non-empty string")),
    }
}

fn limit_arg(arguments: &Value) -> Result<Option<u64>, String> {
    match arguments.get("limit") {
        None | Some(Value::Null) => Ok(None),
        Some(value) => match value.as_u64() {
            // Over the most a page holds is refused, as the bridges refuse it, not quietly cut.
            Some(n) if (1..=MAX_LIMIT).contains(&n) => Ok(Some(n)),
            _ => Err(format!("limit must be a whole number from 1 to {MAX_LIMIT}")),
        },
    }
}

fn window_arg(arguments: &Value, daw: Daw) -> Result<Option<Value>, String> {
    let Some(window) = arguments.get("window").filter(|w| !w.is_null()) else { return Ok(None) };
    let number = |name: &str| window.get(name).and_then(Value::as_f64).filter(|n| n.is_finite());
    match (number("from"), number("to")) {
        (Some(from), Some(to)) if to > from || (daw == Daw::Renoise && to == from) => Ok(Some(json!({"from": from, "to": to}))),
        // (a pattern's one line is a window in Renoise, whose lines are whole numbers; REAPER's are seconds)
        _ => Err(format!("window must be {{from, to}}: two numbers, to {} from", if daw == Daw::Renoise { "not before" } else { "after" })),
    }
}

/// The most points a parameter's text is read at across its range (`grid`, M5): Kumi asks 49, as Live's scan does.
pub const MAX_GRID: u64 = 64;

/// The arguments a `read` forwards: only the ones it knows, each checked.
fn read_params(arguments: &Value) -> Result<Value, String> {
    let mut params = Map::new();
    for name in ["kind", "ref", "parent", "cursor", "epoch"] {
        if let Some(text) = text_arg(arguments, name)? {
            params.insert(name.into(), text.into());
        }
    }
    // The track whose lines of a pattern are read (Renoise's `line`): a number or a ref.
    match arguments.get("track") {
        None | Some(Value::Null) => {}
        Some(Value::Number(n)) if n.as_u64().is_some_and(|n| n >= 1) => {
            params.insert("track".into(), n.clone().into());
        }
        Some(Value::String(text)) if !text.is_empty() => {
            params.insert("track".into(), text.clone().into());
        }
        Some(_) => return Err("track must be a track number from 1, or a track ref".into()),
    }
    if !params.contains_key("kind") && !params.contains_key("ref") {
        return Err("kind: what to read (or ref: one object)".into());
    }
    layout_arg(arguments, &mut params)?;
    hidden_arg(arguments, &mut params)?;
    grid_arg(arguments, &mut params)?;
    call_args(arguments, &mut params)?;
    items_arg(arguments, &mut params)?;
    switches_arg(arguments, &mut params)?;
    if let Some(limit) = limit_arg(arguments)? {
        params.insert("limit".into(), limit.into());
    }
    Ok(Value::Object(params))
}

/// (#401) A Renoise read's switches, forwarded only when true: `prints` (a page of patterns, each track's lines summed up,
/// the jam's comparison) and `byTrack` (tidy-notes' counts per track, layering's players).
fn switches_arg(arguments: &Value, params: &mut Map<String, Value>) -> Result<(), String> {
    for name in ["prints", "byTrack"] {
        match arguments.get(name) {
            None | Some(Value::Null) | Some(Value::Bool(false)) => {}
            Some(Value::Bool(true)) => {
                params.insert(name.into(), true.into());
            }
            Some(_) => return Err(format!("{name} must be true or false")),
        }
    }
    Ok(())
}

/// A parameter's text read at this many points across its range (kind `param`, M5's tune): 2 to `MAX_GRID`.
fn grid_arg(arguments: &Value, params: &mut Map<String, Value>) -> Result<(), String> {
    match arguments.get("grid") {
        None | Some(Value::Null) => Ok(()),
        Some(value) => match value.as_u64().filter(|n| (2..=MAX_GRID).contains(n)) {
            Some(points) => {
                params.insert("grid".into(), points.into());
                Ok(())
            }
            None => Err(format!("grid must be a whole number from 2 to {MAX_GRID}")),
        },
    }
}

/// REAPER's read of an API function by name (`kind: call`): the function and its arguments, in order or by name.
fn call_args(arguments: &Value, params: &mut Map<String, Value>) -> Result<(), String> {
    if let Some(name) = text_arg(arguments, "fn")? {
        params.insert("fn".into(), name.into());
    }
    match arguments.get("args") {
        None | Some(Value::Null) => Ok(()),
        Some(args @ (Value::Array(_) | Value::Object(_))) => {
            params.insert("args".into(), args.clone());
            Ok(())
        }
        Some(_) => Err("args must be the function's arguments: a list in order, or an object by their names".into()),
    }
}

/// (M7) What a Renoise `where` read finds again: the places and refs Kumi kept (a list of objects, at most a page).
fn items_arg(arguments: &Value, params: &mut Map<String, Value>) -> Result<(), String> {
    match arguments.get("items") {
        None | Some(Value::Null) => Ok(()),
        Some(Value::Array(items)) if items.len() <= MAX_LIMIT as usize && items.iter().all(Value::is_object) => {
            params.insert("items".into(), Value::Array(items.clone()));
            Ok(())
        }
        Some(_) => Err(format!("items must be a list of at most {MAX_LIMIT} objects")),
    }
}

/// The arguments a `focus_body` forwards.
fn body_params(arguments: &Value, daw: Daw) -> Result<Value, String> {
    let mut params = Map::new();
    let Some(kind) = text_arg(arguments, "kind")? else { return Err("kind: which body".into()) };
    params.insert("kind".into(), kind.into());
    for name in ["ref", "cursor", "around", "epoch"] {
        if let Some(text) = text_arg(arguments, name)? {
            params.insert(name.into(), text.into());
        }
    }
    if let Some(window) = window_arg(arguments, daw)? {
        params.insert("window".into(), window);
    }
    if let Some(limit) = limit_arg(arguments)? {
        params.insert("limit".into(), limit.into());
    }
    layout_arg(arguments, &mut params)?;
    hidden_arg(arguments, &mut params)?;
    Ok(Value::Object(params))
}

/// Whether to look through the columns a track doesn't show (Renoise), which costs about six times as much: off unless asked.
fn hidden_arg(arguments: &Value, params: &mut Map<String, Value>) -> Result<(), String> {
    match arguments.get("hidden") {
        None | Some(Value::Null) | Some(Value::Bool(false)) => Ok(()),
        Some(Value::Bool(true)) => {
            params.insert("hidden".into(), true.into());
            Ok(())
        }
        Some(_) => Err("hidden must be true or false".into()),
    }
}

/// The layout the request's refs were made in (Renoise's track shifts since): the bridge carries older refs through them.
fn layout_arg(arguments: &Value, params: &mut Map<String, Value>) -> Result<(), String> {
    match arguments.get("layout") {
        None | Some(Value::Null) => Ok(()),
        Some(Value::Number(n)) if n.as_u64().is_some() => {
            params.insert("layout".into(), n.clone().into());
            Ok(())
        }
        Some(_) => Err("layout must be a whole number from 0".into()),
    }
}

/// An answer's size as JSON, counted as it's written rather than written out: a page runs to MAX_ANSWER_BYTES, and is
/// written again as the tool's text.
fn size_of(value: &Value) -> usize {
    struct Counted(usize);
    impl std::io::Write for Counted {
        fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
            self.0 += bytes.len();
            Ok(bytes.len())
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }
    let mut counted = Counted(0);
    serde_json::to_writer(&mut counted, value).map(|()| counted.0).unwrap_or(usize::MAX)
}

impl Host {
    /// The answer of a call as a tool gives it: the JSON, and whether it is an error.
    async fn answer_of(&self, method: &str, params: Value, pick: &Pick, timeout: Duration) -> (Value, bool) {
        match self.call(method, params, pick, timeout).await {
            Ok(answer) => (answer, false),
            Err(error) => (self.call_error(&error), true),
        }
    }

    /// A call whose answer is a page: if the bridge sends one past the frame bound, the same page is asked for again
    /// with half the rows, down to one.
    async fn paged(&self, method: &str, mut params: Value, pick: &Pick, timeout: Duration) -> (Value, bool) {
        let mut before = usize::MAX;
        for attempt in 0..=SMALLER_PAGES {
            match self.call(method, params.clone(), pick, timeout).await {
                Ok(answer) if size_of(&answer) <= MAX_ANSWER_BYTES => return (answer, false),
                Ok(answer) => {
                    // (An answer that doesn't page, or a bridge that ignores the limit, is no smaller for asking again.)
                    let size = size_of(&answer);
                    let limit = params.get("limit").and_then(Value::as_u64).unwrap_or(DEFAULT_LIMIT);
                    if limit <= 1 || attempt == SMALLER_PAGES || size >= before {
                        return (self.call_error(&CallError::TooBig), true);
                    }
                    before = size;
                    params["limit"] = (limit / 2).into();
                }
                Err(error) => return (self.call_error(&error), true),
            }
        }
        (self.call_error(&CallError::TooBig), true)
    }

    fn pick_of(arguments: &Value) -> Result<Pick, (Value, bool)> {
        Pick::from_arguments(arguments).map_err(|why| (json!({"error": why}), true))
    }

    /// `read`: rows of one kind, a page, or one object by ref.
    pub async fn read(&self, arguments: &Value) -> (Value, bool) {
        let pick = match Self::pick_of(arguments) {
            Ok(pick) => pick,
            Err(answer) => return answer,
        };
        match read_params(arguments) {
            Ok(params) => {
                let by_ref = params.get("ref").is_some();
                let (mut answer, is_error) = self.paged("read", params, &pick, READ_TIMEOUT).await;
                // (M4 n-devices) A Renoise device read by ref: what a Doofer, Splitter or Notepad holds, from its preset data.
                if !is_error && by_ref && self.daw() == Daw::Renoise {
                    self.renoise_inside(&mut answer, &pick, arguments.get("presetData") == Some(&Value::Bool(true))).await;
                }
                (answer, is_error)
            }
            Err(why) => (json!({"error": why}), true),
        }
    }

    /// `snapshot`: the project on one screen.
    pub async fn snapshot(&self, arguments: &Value) -> (Value, bool) {
        match Self::pick_of(arguments) {
            Ok(pick) => self.answer_of_bounded("snapshot", json!({}), &pick, READ_TIMEOUT).await,
            Err(answer) => answer,
        }
    }

    /// `focus`: what the producer is looking at, cheap enough to poll.
    pub async fn focus(&self, arguments: &Value) -> (Value, bool) {
        match Self::pick_of(arguments) {
            Ok(pick) => self.answer_of("focus", json!({}), &pick, FOCUS_TIMEOUT).await,
            Err(answer) => answer,
        }
    }

    /// `focus_body`: one FOCUS body.
    pub async fn focus_body(&self, arguments: &Value) -> (Value, bool) {
        let pick = match Self::pick_of(arguments) {
            Ok(pick) => pick,
            Err(answer) => return answer,
        };
        match body_params(arguments, self.daw()) {
            Ok(params) => {
                let dsp = params.get("kind").and_then(Value::as_str) == Some("dsp");
                let (mut answer, is_error) = self.paged("focus_body", params, &pick, READ_TIMEOUT).await;
                // (M4 n-devices) Renoise's DSP chain: each Doofer's and Splitter's devices as its branches.
                if !is_error && dsp && self.daw() == Daw::Renoise {
                    self.renoise_dsp_children(&mut answer, &pick).await;
                }
                (answer, is_error)
            }
            Err(why) => (json!({"error": why}), true),
        }
    }

    /// An answer that has no page to shrink: past the frame bound it is refused.
    async fn answer_of_bounded(&self, method: &str, params: Value, pick: &Pick, timeout: Duration) -> (Value, bool) {
        let (answer, is_error) = self.answer_of(method, params, pick, timeout).await;
        if !is_error && size_of(&answer) > MAX_ANSWER_BYTES {
            return (self.call_error(&CallError::TooBig), true);
        }
        (answer, is_error)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_answers_size_is_its_json_counted_as_written() {
        for value in [
            json!({"items": [{"name": "Bass \"Sub\" ✓ \u{1}", "volume": -6.5, "n": 3, "big": 1e300, "tiny": 1.5e-7}], "next": null}),
            json!([]),
            json!("words"),
            json!(0.1),
        ] {
            assert_eq!(size_of(&value), serde_json::to_string(&value).unwrap().len(), "{value}");
        }
    }

    #[test]
    fn a_wheres_items_are_forwarded_and_anything_else_under_items_refused() {
        let items = json!([{"pattern": {"n": 4, "mark": "p:3"}}, {"track": "1:track:2"}]);
        let params = read_params(&json!({"kind": "where", "items": items})).unwrap();
        assert_eq!(params["items"], items);
        assert!(read_params(&json!({"kind": "where", "items": "all"})).is_err());
        assert!(read_params(&json!({"kind": "where", "items": [1]})).is_err());
        assert!(read_params(&json!({"kind": "song"})).unwrap().get("items").is_none());
    }

    #[test]
    fn a_reads_switches_are_forwarded_only_when_true() {
        let params = read_params(&json!({"kind": "pattern", "prints": true, "limit": 16})).unwrap();
        assert_eq!(params, json!({"kind": "pattern", "prints": true, "limit": 16}));
        assert_eq!(read_params(&json!({"kind": "tidy-notes", "byTrack": true})).unwrap()["byTrack"], true);
        assert!(read_params(&json!({"kind": "pattern", "prints": false})).unwrap().get("prints").is_none());
        assert!(read_params(&json!({"kind": "pattern", "prints": "yes"})).is_err());
    }

    #[test]
    fn hidden_is_forwarded_only_when_true() {
        assert_eq!(
            read_params(&json!({"kind": "line", "parent": "1:pattern:2", "track": 3, "hidden": true})).unwrap(),
            json!({"kind": "line", "parent": "1:pattern:2", "track": 3, "hidden": true})
        );
        assert_eq!(
            body_params(&json!({"kind": "pattern", "hidden": true}), Daw::Renoise).unwrap(),
            json!({"kind": "pattern", "hidden": true})
        );
        assert_eq!(read_params(&json!({"kind": "line", "hidden": false})).unwrap(), json!({"kind": "line"}));
        assert_eq!(
            read_params(&json!({"kind": "param", "parent": "r:fx:{A}", "grid": 49})).unwrap(),
            json!({"kind": "param", "parent": "r:fx:{A}", "grid": 49})
        );
        for grid in [json!(1), json!(65), json!(4.5), json!("49")] {
            assert!(read_params(&json!({"kind": "param", "grid": grid})).is_err(), "{grid}");
        }
        assert!(read_params(&json!({"kind": "line", "hidden": "yes"})).is_err());
        assert!(body_params(&json!({"kind": "pattern", "hidden": 1}), Daw::Renoise).is_err());
    }

    #[test]
    fn a_call_read_forwards_its_function_and_arguments() {
        assert_eq!(
            read_params(&json!({"kind": "call", "fn": "GetMediaTrackInfo_Value", "args": ["r:track:master", "D_VOL"]})).unwrap(),
            json!({"kind": "call", "fn": "GetMediaTrackInfo_Value", "args": ["r:track:master", "D_VOL"]})
        );
        assert_eq!(
            read_params(&json!({"kind": "call", "fn": "CountTracks", "args": {"proj": 0}})).unwrap(),
            json!({"kind": "call", "fn": "CountTracks", "args": {"proj": 0}})
        );
        for bad in
            [json!({"kind": "call", "fn": ""}), json!({"kind": "call", "fn": 3}), json!({"kind": "call", "fn": "CountTracks", "args": "0"})]
        {
            assert!(read_params(&bad).is_err(), "{bad}");
        }
    }

    #[test]
    fn a_read_forwards_only_what_it_knows_checked() {
        let params =
            read_params(&json!({"kind": "item", "parent": "r:track:master", "cursor": "c", "limit": 200, "pid": 4, "extra": true}))
                .unwrap();
        assert_eq!(params, json!({"kind": "item", "parent": "r:track:master", "cursor": "c", "limit": 200}));
        assert_eq!(read_params(&json!({"ref": "r:track:master"})).unwrap(), json!({"ref": "r:track:master"}));
        for bad in [
            json!({}),
            json!({"kind": 3}),
            json!({"kind": ""}),
            json!({"kind": "track", "limit": 0}),
            json!({"kind": "track", "limit": 201}),
            json!({"kind": "track", "limit": "9"}),
            json!({"kind": "track", "cursor": []}),
            json!({"kind": "line", "track": 0}),
            json!({"kind": "line", "track": 1.5}),
            json!({"kind": "line", "track": ""}),
            json!({"kind": "line", "track": ["1"]}),
        ] {
            assert!(read_params(&bad).is_err(), "{bad}");
        }
        // Renoise's lines are read for a track, given by number or by ref.
        assert_eq!(
            read_params(&json!({"kind": "line", "parent": "1:pattern:2", "track": 3})).unwrap(),
            json!({"kind": "line", "parent": "1:pattern:2", "track": 3})
        );
        assert_eq!(
            read_params(&json!({"kind": "line", "parent": "1:pattern:2", "track": "1:track:3"})).unwrap(),
            json!({"kind": "line", "parent": "1:pattern:2", "track": "1:track:3"})
        );
    }

    #[test]
    fn a_body_forwards_its_window_when_it_is_a_range() {
        let params =
            body_params(&json!({"kind": "arrange", "window": {"from": 0, "to": 32}, "limit": 10, "resource": "/x"}), Daw::Reaper).unwrap();
        assert_eq!(params, json!({"kind": "arrange", "window": {"from": 0.0, "to": 32.0}, "limit": 10}));
        let params = body_params(&json!({"kind": "arrange", "limit": 24, "around": "r:track:{A}"}), Daw::Reaper).unwrap();
        assert_eq!(params, json!({"kind": "arrange", "around": "r:track:{A}", "limit": 24}));
        assert!(body_params(&json!({"kind": "arrange", "around": 5}), Daw::Reaper).is_err());
        // (one line of a pattern is a window; a bridge that wants more says so)
        assert_eq!(
            body_params(&json!({"kind": "pattern", "window": {"from": 5, "to": 5}}), Daw::Renoise).unwrap(),
            json!({"kind": "pattern", "window": {"from": 5.0, "to": 5.0}})
        );
        for bad in [
            json!({}),
            json!({"kind": "arrange", "window": {"from": 6, "to": 5}}),
            json!({"kind": "arrange", "window": {"from": 5, "to": 5}}),
            json!({"kind": "arrange", "window": "all"}),
            json!({"kind": "fx", "ref": 2}),
        ] {
            assert!(body_params(&bad, Daw::Reaper).is_err(), "{bad}");
        }
    }

    #[test]
    fn each_daw_has_its_own_kinds() {
        assert!(kinds(Daw::Reaper).read.contains(&"envelope") && !kinds(Daw::Reaper).read.contains(&"pattern"));
        assert!(kinds(Daw::Renoise).bodies.contains(&"matrix") && !kinds(Daw::Renoise).bodies.contains(&"arrange"));
    }
}
