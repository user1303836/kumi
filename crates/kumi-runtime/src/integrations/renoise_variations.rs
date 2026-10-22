//! Renoise's backend for the variations tool (M7): only Renoise's facts. Its material is a pattern track's lines or a phrase's
//! (`renoise_lines`), its operators shuffle slices, roll, reverse and move ghosts, and its takes are phrases of an instrument or
//! slots of their own in a "Variations of …" section (a copy of the source pattern whose other tracks are aliases of the source's).
//! Takes are found by the places the tool's logs follow, else by the names Kumi gave them; the source pattern, track and
//! instrument the same way. Pattern takes render on the source's track from their slots, phrase takes through players of their
//! phrases (scratch tracks, gone after). Core's tool does the rest.
use super::own_changes::OwnPlan;
use super::own_listening::OwnRenderer;
use super::renoise_lines::{note_text, note_value, Lines, Op};
use crate::core::{
    contracts::JsonObject,
    refs::RefScheme,
    variations::{
        backend::{Rendered, Rewritten, Source, VariationsBackend, Words},
        heard::{heard_of, Heard},
        tool::names,
        Rng, TakeSet, TakeState,
    },
};
use async_trait::async_trait;
use kumi_common::abort::Signal;
use serde_json::{json, Map, Value};
use std::{collections::BTreeMap, collections::BTreeSet, rc::Rc};

/// Pages of lines read for a source (200 lines a page: a 512-line pattern is three).
const LINE_PAGES: usize = 8;
/// An instrument holds 126 phrases (0Z01 to 0Z7E).
const MOST_PHRASES: u64 = 126;
/// The longest a phrase take plays in its render (beats).
const PHRASE_BEATS_MOST: f64 = 32.;

pub static WORDS: Words = Words {
    app: "Renoise",
    sources: "a pattern track's lines (patternRef, or a slot for its pattern and its matrix mutes; trackRef; first and last, as Renoise numbers lines, from 00), a phrase (phraseRef), or an instrument (instrumentRef: new phrases from lines you write)",
    anchors: "the lines (as Renoise numbers them) every take keeps as they are: the kick and snare that make it the producer's",
    ops: "shuffle, retrig, reverse, ghosts, thin, fill, accent, octave, transpose, swing, offsets, speed (phrase takes only: a pattern has no LPB of its own); amount 0 to 1",
    lines: "lines you write, as notes_write takes them, numbered as the source's lines",
    places: "patterns (each take a slot of its own in a \"Variations of …\" section at the sequence's end, its other tracks playing the source pattern's, so playing it plays the take in context: a play or render of the whole song plays them too, and drop removes the ones not kept; the default for a pattern's lines) or phrases (new phrases of the instrument; the default otherwise)",
    place_names: &["patterns", "phrases"],
    at: "a phrase on keys [low, high], as the instrument's program, or switchAt {patternRef, trackRef, line, col}: a 0Zxx there plays it (how a pattern switches to a faster copy, since a phrase's LPB is fixed); a pattern take over the source's lines, after the source's slot, or at index",
    options: "a phrase take's instrumentRef (a pattern's lines become phrases of the instrument their notes play by default), keyTracking, baseNote (48 is C-4: the note that plays it as written), lines, lpb and name",
    extract: Some("one phrase of a pattern track's lines (source), relative to a base note so key tracking plays it from any key (options.baseNote: its first note by default; keyTracking none for a chop)."),
    idioms: "Renoise numbers lines from 00 as it shows them; effect commands as Renoise writes them: 0Sxx sample offset, 0Rxy retrig, 0B00 reverse, 0Zxx phrase.",
    directions: &["denser", "sparser", "more reverses", "shifted accents", "faster rolls", "swapped slices", "moved ghosts", "an octave up"],
    source_schema: r#"{"type":"object","additionalProperties":false,"properties":{"patternRef":{"type":"string","description":"The pattern, or a sequence slot for its pattern (its mutes go with the takes)"},"trackRef":{"type":"string"},"first":{"type":"integer","minimum":0},"last":{"type":"integer","minimum":0},"phraseRef":{"type":"string"},"instrumentRef":{"type":"string"}}}"#,
    op_schema: r#"{"type":"object","additionalProperties":false,"required":["op"],"properties":{"op":{"type":"string","enum":["shuffle","retrig","reverse","ghosts","thin","fill","accent","octave","transpose","swing","offsets","speed"]},"amount":{"type":"number","minimum":0,"maximum":1},"bar":{"type":"boolean"},"faster":{"type":"boolean"},"up":{"type":"boolean"},"volume":{"type":"string"},"semitones":{"type":"integer"},"factor":{"type":"number"}}}"#,
    options_schema: r#"{"type":"object","additionalProperties":false,"properties":{"instrumentRef":{"type":"string","description":"The instrument a pattern's lines become phrases of (the one its notes play by default)"},"keyTracking":{"type":"string","enum":["none","transpose","offset"]},"baseNote":{"type":"integer","minimum":0,"maximum":119},"lines":{"type":"integer","minimum":1,"maximum":512},"lpb":{"type":"integer","minimum":1,"maximum":256},"name":{"type":"string","maxLength":64}}}"#,
    at_schema: r#"{"type":"object","additionalProperties":false,"properties":{"keys":{"type":"array","items":{"type":"integer","minimum":0,"maximum":119},"minItems":2,"maxItems":2},"program":{"type":"boolean"},"switchAt":{"type":"object","additionalProperties":false,"required":["patternRef","trackRef","line"],"properties":{"patternRef":{"type":"string"},"trackRef":{"type":"string"},"line":{"type":"integer","minimum":0},"col":{"type":"integer","minimum":1,"maximum":8}}},"over":{"type":"boolean"},"after":{"type":"boolean"},"index":{"type":"integer","minimum":1}}}"#,
};

/// Where a set's source is, and where its takes go.
#[derive(Debug, Clone, PartialEq)]
pub struct RenoiseData {
    kind: SourceKind,
    placement: Placement,
    /// The instrument a phrase set's takes are in, by name (found by it when its place is lost).
    instrument_name: String,
}

#[derive(Debug, Clone, PartialEq)]
enum SourceKind {
    /// A pattern track's lines first..=last: the pattern by its place in the pool, the track by its ref at `layout` (and its name),
    /// the source slot when the model named one (its mutes go with each take).
    Range {
        pattern: Value,
        track: String,
        track_name: String,
        layout: Option<u64>,
        first: u32,
        last: u32,
        slot: Option<u64>,
    },
    Phrase {
        place: Value,
    },
    Instrument,
}

#[derive(Debug, Clone, PartialEq)]
enum Placement {
    /// New phrases of the instrument (its place), made with these settings.
    Phrases { instrument: Value, settings: JsonObject },
    /// Slots of their own; the section the first one starts.
    Patterns { section: String },
}

/// Renoise's sets (the backend's `Set`, whatever its refs).
type RenoiseSet = TakeSet<Source<Lines, RenoiseData>, Value, Lines>;

/// Renoise's backend, through Kumi's change engine (its changes) and renderer (its renders).
pub struct RenoiseBackend<S: RefScheme + 'static> {
    plan: Rc<OwnPlan<S>>,
    renderer: Rc<OwnRenderer<S>>,
}

impl<S: RefScheme + 'static> RenoiseBackend<S> {
    pub fn new(plan: Rc<OwnPlan<S>>, renderer: Rc<OwnRenderer<S>>) -> Self {
        RenoiseBackend { plan, renderer }
    }

    // Reading

    fn lengthen(&self, reference: &str) -> Result<String, String> {
        self.plan.lengthen(reference)
    }

    async fn read(&self, args: Value, layout: Option<u64>, signal: &Signal) -> Result<Value, String> {
        self.plan.read_through(args, layout, signal).await
    }

    async fn rows(&self, args: Value, signal: &Signal) -> Result<Vec<Value>, String> {
        self.plan.rows_through(args, LINE_PAGES, signal).await
    }

    /// Where each item is now (`read {kind: "where"}`): {pattern}, {instrument}, {phrase} (a place or a ref) or {track} (a ref
    /// from `layout`).
    async fn locate(&self, items: Vec<Value>, layout: Option<u64>, signal: &Signal) -> Result<Vec<Value>, String> {
        let answer = self.read(json!({"kind": "where", "items": items}), layout, signal).await?;
        Ok(answer.get("items").and_then(Value::as_array).cloned().unwrap_or_default())
    }

    async fn song(&self, signal: &Signal) -> Option<Value> {
        let song = self.read(json!({"kind": "song"}), None, signal).await.ok()?;
        song.pointer("/items/0").cloned()
    }

    async fn song_lpb(&self, signal: &Signal) -> u32 {
        self.song(signal).await.and_then(|song| song.get("lpb").and_then(Value::as_u64)).unwrap_or(4) as u32
    }

    /// The ref of the instrument a phrase set's takes are in: by its place, else by its name.
    async fn instrument_now(&self, set: &RenoiseSet, signal: &Signal) -> Result<String, String> {
        let Placement::Phrases { instrument, .. } = &set.source.data.placement else {
            return Err("this set's takes are no phrases".into());
        };
        let found = self.locate(vec![json!({"instrument": instrument})], None, signal).await?;
        if let Some(reference) = found.first().and_then(|row| row.get("ref")).and_then(Value::as_str) {
            return Ok(reference.to_string());
        }
        let name = &set.source.data.instrument_name;
        let rows = self.rows(json!({"kind": "instrument", "limit": 200}), signal).await?;
        only_named(&rows, name).ok_or(format!("the instrument “{name}” isn't in the song where Kumi left it: read again"))
    }

    /// The source pattern's row now (a pattern-track set): by its place, else by its name when one pattern has it.
    async fn pattern_now(&self, set: &RenoiseSet, signal: &Signal) -> Result<Value, String> {
        let SourceKind::Range { pattern, .. } = &set.source.data.kind else { return Err("this set has no source pattern".into()) };
        let found = self.locate(vec![json!({"pattern": pattern})], None, signal).await?;
        if let Some(row) = found.into_iter().next().filter(|row| row.get("ref").is_some()) {
            return Ok(row);
        }
        let rows = self.rows(json!({"kind": "pattern", "limit": 200}), signal).await?;
        let reference = only_named(&rows, &set.source.short)
            .ok_or(format!("the source pattern “{}” isn't in the song where Kumi left it: read again", set.source.short))?;
        let again = self.locate(vec![json!({"pattern": reference})], None, signal).await?;
        again.into_iter().next().filter(|row| row.get("ref").is_some()).ok_or("the source pattern isn't in the song any more".into())
    }

    /// The source track's ref now (a pattern-track set): carried from the layout it was read at, else by its name.
    async fn track_now(&self, set: &RenoiseSet, signal: &Signal) -> Result<String, String> {
        let SourceKind::Range { track, track_name, layout, .. } = &set.source.data.kind else {
            return Err("this set has no source track".into());
        };
        if let Ok(found) = self.locate(vec![json!({"track": track})], *layout, signal).await {
            if let Some(reference) = found.first().and_then(|row| row.get("ref")).and_then(Value::as_str) {
                return Ok(reference.to_string());
            }
        }
        let rows = self.rows(json!({"kind": "track", "limit": 200}), signal).await?;
        only_named(&rows, track_name).ok_or(format!("the source's track “{track_name}” isn't in the song where Kumi left it: read again"))
    }

    /// The instrument's phrases (or the song's patterns), for takes found by their names.
    async fn named_rows(&self, set: &RenoiseSet, signal: &Signal) -> Result<Vec<Value>, String> {
        match &set.source.data.placement {
            Placement::Phrases { .. } => {
                let instrument = self.instrument_now(set, signal).await?;
                self.rows(json!({"kind": "phrase", "parent": instrument, "limit": 200}), signal).await
            }
            Placement::Patterns { .. } => self.rows(json!({"kind": "pattern", "limit": 200}), signal).await,
        }
    }

    /// Where the source's lines and each take's lines start in the song (beats), and the track to render them on.
    async fn where_slots(
        &self,
        set: &mut RenoiseSet,
        numbers: &[usize],
        signal: &Signal,
    ) -> Result<(f64, String, BTreeMap<usize, f64>), String> {
        let source_row = self.pattern_now(set, signal).await?;
        let track = self.track_now(set, signal).await?;
        let found = self.find(set, numbers, signal).await?;
        let rows = self.rows(json!({"kind": "sequence", "limit": 200}), signal).await?;
        let starts: BTreeMap<String, f64> =
            rows.iter().filter_map(|row| Some((row.get("ref")?.as_str()?.to_string(), row.get("positionBeats")?.as_f64()?))).collect();
        let start = |row: &Value| row.get("seqRef").and_then(Value::as_str).and_then(|reference| starts.get(reference)).copied();
        let source = start(&source_row).ok_or("no slot plays the source's pattern now")?;
        let takes = numbers.iter().zip(&found).filter_map(|(number, row)| Some((*number, start(row)?))).collect();
        Ok((source, track, takes))
    }

    /// One render of the track over `beats` from `from` (beats from the song's start), heard.
    async fn hear(&self, from: f64, beats: f64, track: &str, signal: &Signal) -> Result<Heard, String> {
        let sources = vec![json!({"id": "t", "what": "track", "ref": track, "stage": "pre-fader"})];
        let window = crate::listening::renderer::Window { from, beats };
        match self.renderer.render_sources(sources, window, signal).await {
            Ok(Ok((files, _))) => match files.get("t") {
                Some((render, _)) => heard_of(render, signal).await,
                None => Err("the render made no file".to_string()),
            },
            Ok(Err(why)) => Err(why),
            Err(error) => Err(error.to_string()),
        }
    }

    /// Phrase takes heard through players of their phrases: scratch tracks, one per phrase, made, rendered at once and dropped.
    async fn hear_phrases(&self, set: &mut RenoiseSet, numbers: &[usize], with_source: bool, signal: &Signal) -> Result<Rendered, String> {
        let found = self.find(set, numbers, signal).await?;
        let mut phrases: Vec<(Option<usize>, u64)> = Vec::new();
        if let (true, SourceKind::Phrase { place }) = (with_source, &set.source.data.kind) {
            let row = self.locate(vec![json!({"phrase": place})], None, signal).await?;
            if let Some(index) = row.first().and_then(|row| row.get("index")).and_then(Value::as_u64) {
                phrases.push((None, index));
            }
        }
        for (number, row) in numbers.iter().zip(&found) {
            if let Some(index) = row.get("index").and_then(Value::as_u64) {
                phrases.push((Some(*number), index));
            }
        }
        if phrases.is_empty() {
            return Err("the phrases weren't found".into());
        }
        let instrument = self.instrument_now(set, signal).await?;
        let note = match &set.source.data.placement {
            Placement::Phrases { settings, .. } => settings.get("baseNote").and_then(Value::as_u64).unwrap_or(48),
            Placement::Patterns { .. } => 48,
        };
        let beats = numbers
            .iter()
            .filter_map(|n| set.take(*n))
            .map(|take| take.material.length as f64 / take.material.lpb.max(1) as f64)
            .fold(1., f64::max)
            .min(PHRASE_BEATS_MOST);
        let list: Vec<u64> = phrases.iter().map(|(_, index)| *index).collect();
        let args = json!({"action": "make", "count": list.len(), "label": "variations", "range": {"fromBeat": 0, "beats": beats},
            "phrases": {"instrument": instrument, "phrases": list, "note": note}});
        let made = match self.renderer.ask("scratch", args, self.renderer.cleanup_signal()).await {
            Ok(Ok(made)) => made,
            Ok(Err(refused)) => return Err(refused.why),
            Err(error) => return Err(error.to_string()),
        };
        let id = made.get("scratchId").and_then(Value::as_str).unwrap_or("").to_string();
        let steps = made.pointer("/undo/steps").and_then(Value::as_u64).unwrap_or(0);
        self.renderer.tell_scratch_steps(steps);
        let sources: Vec<Value> = made
            .get("copies")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .enumerate()
            .filter_map(|(k, copy)| {
                Some(json!({"id": format!("p{k}"), "what": "track", "ref": copy.get("ref")?.as_str()?, "stage": "pre-fader"}))
            })
            .collect();
        let window = crate::listening::renderer::Window { from: 0., beats };
        let rendered = self.renderer.render_sources(sources, window, signal).await;
        self.renderer.drop_make(&id, steps).await;
        let files = match rendered {
            Ok(Ok((files, _))) => files,
            Ok(Err(why)) => return Err(why),
            Err(error) => return Err(error.to_string()),
        };
        let mut heard = BTreeMap::new();
        let mut source = None;
        for (k, (number, _)) in phrases.iter().enumerate() {
            let result = match files.get(&format!("p{k}")) {
                Some((render, _)) => heard_of(render, signal).await,
                None => Err("the render made no file".to_string()),
            };
            match number {
                Some(number) => {
                    heard.insert(*number, result);
                }
                None => source = result.ok(),
            }
        }
        Ok((source, heard))
    }

    /// One change of Kumi's by its step's name: its change id and what the DAW answered, or why it wasn't made (and whether the
    /// request must stop there).
    async fn change(&self, tool: &str, input: Value, signal: &Signal) -> Result<(String, Value), (String, bool)> {
        *self.plan.last_applied.borrow_mut() = None;
        let outcome = self.plan.change_by(tool, input.as_object().cloned().unwrap_or_default(), signal.clone()).await;
        let answer: Value = serde_json::from_str(&outcome.text).unwrap_or(Value::Null);
        if outcome.is_error || outcome.stops {
            let why = answer.get("changed").and_then(Value::as_str).map(str::to_owned).unwrap_or(outcome.text);
            return Err((why, outcome.stops));
        }
        let id = answer.get("change").and_then(Value::as_str).unwrap_or("").to_string();
        Ok((id, self.plan.last_applied.borrow_mut().take().unwrap_or(Value::Null)))
    }

    /// A phrase source's own settings (key tracking, base note), for its takes; what `options` gives wins.
    async fn phrase_row_settings(
        &self,
        instrument: &str,
        reference: &str,
        given: &JsonObject,
        lines: u32,
        lpb: u32,
        signal: &Signal,
    ) -> JsonObject {
        let mut settings = phrase_settings(given, lines, lpb);
        if let Ok(rows) = self.rows(json!({"kind": "phrase", "parent": instrument, "limit": 200}), signal).await {
            if let Some(row) = rows.iter().find(|row| row.get("ref").and_then(Value::as_str) == Some(reference)) {
                for key in ["keyTracking", "baseNote"] {
                    if let Some(value) = row.get(key) {
                        settings.entry(key).or_insert(value.clone());
                    }
                }
            }
        }
        settings
    }
}

#[async_trait(?Send)]
impl<S: RefScheme + 'static> VariationsBackend for RenoiseBackend<S> {
    type Material = Lines;
    type Data = RenoiseData;

    fn words(&self) -> &'static Words {
        &WORDS
    }

    async fn project_key(&self, signal: &Signal) -> Option<String> {
        let song = self.song(signal).await?;
        ["id", "identity"].iter().find_map(|key| song.get(*key).and_then(Value::as_str).filter(|text| !text.is_empty()).map(str::to_owned))
    }

    async fn source(&self, input: &JsonObject, signal: &Signal) -> Result<Source<Lines, RenoiseData>, String> {
        let source = input
            .get("source")
            .and_then(Value::as_object)
            .ok_or("make needs a source: patternRef and trackRef (with first and last), phraseRef, or instrumentRef")?;
        let text = |key: &str| source.get(key).and_then(Value::as_str);
        let options = input.get("options").and_then(Value::as_object).cloned().unwrap_or_default();
        let anchors: Vec<u32> =
            input.get("anchors").and_then(Value::as_array).into_iter().flatten().filter_map(Value::as_u64).map(|n| n as u32).collect();
        let place = input.get("place").and_then(Value::as_str);
        if let (Some(pattern), Some(track)) = (text("patternRef"), text("trackRef")) {
            let (pattern, track) = (self.lengthen(pattern)?, self.lengthen(track)?);
            // (a slot named for the pattern is the source slot: its mutes go with each take)
            let slot = (pattern.split(':').nth(1) == Some("seq"))
                .then(|| pattern.rsplit(':').next().and_then(|n| n.parse::<u64>().ok()))
                .flatten();
            let found = self.locate(vec![json!({"pattern": pattern}), json!({"track": track})], None, signal).await?;
            let (pattern_row, track_row) = (&found[0], &found[1]);
            let (Some(pattern_ref), Some(track_ref)) =
                (pattern_row.get("ref").and_then(Value::as_str), track_row.get("ref").and_then(Value::as_str))
            else {
                return Err("that pattern or track isn't in the song any more: read again".into());
            };
            let length = pattern_row.get("lines").and_then(Value::as_u64).unwrap_or(64) as u32;
            let first = source.get("first").and_then(Value::as_u64).unwrap_or(0) as u32;
            let last = source.get("last").and_then(Value::as_u64).map_or(length - 1, |n| n as u32);
            if first > last || last >= length {
                return Err(format!("first and last are lines from 00 to {:02} of this pattern, first not after last", length - 1));
            }
            let lpb = self.song_lpb(signal).await;
            let rows =
                self.rows(json!({"kind": "line", "parent": pattern_ref, "track": track_ref, "hidden": true, "limit": 200}), signal).await?;
            let mut material = Lines::from_rows(&rows, first, last, lpb);
            material.anchors = anchors_in(&anchors, first, last)?;
            let number = pattern_row.get("number").and_then(Value::as_u64).unwrap_or(0);
            let pattern_name = named(pattern_row, &format!("Pattern {number:02}"));
            let track_name = named(track_row, "the track");
            let label = format!("lines {first:02}–{last:02} of “{pattern_name}” on “{track_name}”");
            let key = format!("range:{number}:{}:{first}:{last}", track_row.get("index").and_then(Value::as_u64).unwrap_or(0));
            let kind = SourceKind::Range {
                pattern: pattern_row.get("place").cloned().unwrap_or(Value::Null),
                track: track_ref.to_string(),
                track_name: track_row.get("name").and_then(Value::as_str).unwrap_or("").to_string(),
                layout: self.plan.link().layout.get(),
                first,
                last,
                slot,
            };
            let (placement, instrument_name) = match place.unwrap_or("patterns") {
                "patterns" => (Placement::Patterns { section: format!("Variations of {pattern_name}") }, String::new()),
                _ => {
                    // (a pattern's lines become phrases of the instrument they play: its notes lose their instrument column)
                    let instrument = match options.get("instrumentRef").and_then(Value::as_str) {
                        Some(reference) => self.lengthen(reference)?,
                        None => {
                            let value =
                                most_played(&material).ok_or("those lines play no instrument: name one with options.instrumentRef")?;
                            let epoch = pattern_ref.split(':').next().unwrap_or("");
                            format!("{epoch}:instrument:{}", value as u32 + 1)
                        }
                    };
                    let found = self.locate(vec![json!({"instrument": instrument})], None, signal).await?;
                    let row = found.first().filter(|row| row.get("ref").is_some()).ok_or("that instrument isn't in the song any more")?;
                    let index = row.get("index").and_then(Value::as_u64).unwrap_or(1) as u8 - 1;
                    material = for_phrase(&material, index)?;
                    let base = options.get("baseNote").cloned().or_else(|| first_note(&material).map(Value::from));
                    let mut settings = phrase_settings(&options, material.length, lpb);
                    settings.entry("keyTracking").or_insert_with(|| json!("transpose"));
                    if let Some(base) = base {
                        settings.entry("baseNote").or_insert(base);
                    }
                    (Placement::Phrases { instrument: row.get("place").cloned().unwrap_or(Value::Null), settings }, named(row, ""))
                }
            };
            // (phrase takes of a range are a set of their own: a make with another place starts another set)
            let key = match placement {
                Placement::Patterns { .. } => key,
                Placement::Phrases { .. } => format!("{key}:phrases"),
            };
            let data = RenoiseData { kind, placement, instrument_name };
            return Ok(Source { label, short: pattern_name, key, material: Some(material), data });
        }
        if place == Some("patterns") {
            return Err("place patterns is for a pattern track's lines (patternRef and trackRef)".into());
        }
        if let Some(phrase_ref) = text("phraseRef") {
            let long = self.lengthen(phrase_ref)?;
            let instrument = instrument_of(&long).ok_or("phraseRef must be a phrase")?;
            let found = self.locate(vec![json!({"phrase": long}), json!({"instrument": instrument})], None, signal).await?;
            let (row, instrument_row) = (&found[0], &found[1]);
            let reference = row.get("ref").and_then(Value::as_str).ok_or("that phrase isn't in the song any more: read again")?;
            let lines = row.get("lines").and_then(Value::as_u64).unwrap_or(16) as u32;
            let lpb = row.get("lpb").and_then(Value::as_u64).unwrap_or(4) as u32;
            let rows = self.rows(json!({"kind": "line", "parent": reference, "hidden": true, "limit": 200}), signal).await?;
            let mut material = Lines::from_rows(&rows, 0, lines.max(1) - 1, lpb);
            material.anchors = anchors_in(&anchors, 0, lines.max(1) - 1)?;
            let settings = self.phrase_row_settings(&instrument, reference, &options, lines, lpb, signal).await;
            let name = named(row, &format!("Phrase {:02}", row.get("index").and_then(Value::as_u64).unwrap_or(1)));
            let label = format!("phrase “{name}” of “{}”", named(instrument_row, "the instrument"));
            let data = RenoiseData {
                kind: SourceKind::Phrase { place: row.get("place").cloned().unwrap_or(Value::Null) },
                placement: Placement::Phrases { instrument: instrument_row.get("place").cloned().unwrap_or(Value::Null), settings },
                instrument_name: named(instrument_row, ""),
            };
            return Ok(Source {
                label,
                short: name,
                key: format!("phrase:{}", reference.splitn(3, ':').nth(2).unwrap_or("")),
                material: Some(material),
                data,
            });
        }
        if let Some(instrument_ref) = text("instrumentRef") {
            let long = self.lengthen(instrument_ref)?;
            let found = self.locate(vec![json!({"instrument": long})], None, signal).await?;
            let row =
                found.first().filter(|row| row.get("ref").is_some()).ok_or("that instrument isn't in the song any more: read again")?;
            let name = named(row, "the instrument");
            let lpb = match options.get("lpb").and_then(Value::as_u64) {
                Some(lpb) => lpb as u32,
                None => self.song_lpb(signal).await,
            };
            let lines = options.get("lines").and_then(Value::as_u64).map(|n| n as u32).unwrap_or_else(|| written_length(input, 16));
            let data = RenoiseData {
                kind: SourceKind::Instrument,
                placement: Placement::Phrases {
                    instrument: row.get("place").cloned().unwrap_or(Value::Null),
                    settings: phrase_settings(&options, lines, lpb),
                },
                instrument_name: named(row, ""),
            };
            let key = format!("instrument:{}", row.get("index").and_then(Value::as_u64).unwrap_or(0));
            return Ok(Source { label: format!("new phrases of “{name}”"), short: name, key, material: None, data });
        }
        Err("source is patternRef and trackRef (with first and last), phraseRef, or instrumentRef".into())
    }

    fn direction_ops(&self, direction: &str) -> Vec<Value> {
        match direction {
            "denser" => vec![json!({"op": "fill", "amount": 0.3}), json!({"op": "retrig", "amount": 0.2})],
            "sparser" => vec![json!({"op": "thin", "amount": 0.35})],
            "more reverses" => vec![json!({"op": "reverse", "amount": 0.4})],
            "shifted accents" => vec![json!({"op": "accent", "amount": 0.6})],
            "faster rolls" => vec![json!({"op": "retrig", "amount": 0.4})],
            "swapped slices" => vec![json!({"op": "shuffle", "amount": 0.6})],
            "moved ghosts" => vec![json!({"op": "ghosts", "amount": 0.8}), json!({"op": "fill", "amount": 0.15})],
            _ => vec![json!({"op": "octave", "amount": 0.4, "up": true})],
        }
    }

    fn check_ops(&self, source: &Source<Lines, RenoiseData>, ops: &[Value]) -> Result<(), String> {
        let parsed = ops.iter().map(Op::parse).collect::<Result<Vec<_>, _>>()?;
        if matches!(source.data.placement, Placement::Patterns { .. }) && parsed.iter().any(|op| matches!(op, Op::Speed { .. })) {
            return Err("speed is for phrase takes: a pattern has no LPB of its own (place: \"phrases\" makes phrase takes)".into());
        }
        Ok(())
    }

    fn apply_ops(&self, ops: &[Value], factor: f64, take: &mut Lines, source: &Lines, rng: &mut Rng) -> Result<Vec<String>, String> {
        let mut words = Vec::new();
        for op in ops {
            if let Some(done) = Op::parse(op)?.scaled(factor).apply(take, source, rng) {
                words.push(done);
            }
        }
        Ok(words)
    }

    fn written(&self, set: &RenoiseSet, lines: &[Value], base: &Lines) -> Result<Lines, String> {
        let offset = match &set.source.data.kind {
            SourceKind::Range { first, .. } => *first,
            _ => 0,
        };
        let mut written = Lines::from_cells(lines, offset, base.length, base.lpb)?;
        written.anchors = base.anchors.clone();
        Ok(written)
    }

    fn empty(&self, set: &RenoiseSet) -> Lines {
        match &set.source.data.placement {
            Placement::Phrases { settings, .. } => empty_lines(
                settings.get("lines").and_then(Value::as_u64).unwrap_or(16) as u32,
                settings.get("lpb").and_then(Value::as_u64).unwrap_or(4) as u32,
            ),
            Placement::Patterns { .. } => empty_lines(16, 4),
        }
    }

    fn anchors_kept(&self, source: &Lines, take: &Lines) -> Result<(), String> {
        source
            .anchors_kept_in(take)
            .map_err(|line| format!("line {line:02} (from the source's first) is an anchor: every take keeps it as it is"))
    }

    fn judge(&self, source: Option<&Lines>, take: &Lines, ops: &[Value], factor: f64) -> Vec<String> {
        judge(source, take, ops, factor)
    }

    fn digest(&self, material: &Lines) -> String {
        material.digest()
    }

    async fn write_new(
        &self,
        set: &RenoiseSet,
        number: usize,
        material: &Lines,
        signal: &Signal,
    ) -> Result<(String, Value, String), (String, bool)> {
        let name = names(number, &set.source.short)[0].clone();
        match (&set.source.data.placement, &set.source.data.kind) {
            (Placement::Phrases { settings, .. }, _) => {
                // (an instrument holds 126 phrases: 0Z01 to 0Z7E)
                let reference = self.instrument_now(set, signal).await.map_err(|why| (why, true))?;
                let found = self.locate(vec![json!({"instrument": reference})], None, signal).await.map_err(|why| (why, true))?;
                if found.first().and_then(|row| row.get("phrases")).and_then(Value::as_u64).is_some_and(|count| count >= MOST_PHRASES) {
                    return Err((
                        format!(
                            "“{}” holds {MOST_PHRASES} phrases, the most Renoise takes (0Z01 to 0Z7E)",
                            set.source.data.instrument_name
                        ),
                        true,
                    ));
                }
                let mut input = settings.clone();
                input.insert("ref".into(), json!(reference));
                input.insert("name".into(), json!(name));
                input.insert("lpb".into(), json!(material.lpb));
                input.insert("lines".into(), json!(material.length));
                input.insert("content".into(), Value::Array(material.cells(0)));
                let (change, applied) = self.change("phrase_add", Value::Object(input), signal).await?;
                let place = applied.pointer("/created/place").cloned().unwrap_or(Value::Null);
                let made = applied.pointer("/created/ref").and_then(Value::as_str).unwrap_or("").to_string();
                Ok((change, place, made))
            }
            (Placement::Patterns { section }, SourceKind::Range { first, last, slot, .. }) => {
                let source = self.pattern_now(set, signal).await.map_err(|why| (why, true))?;
                let track = self.track_now(set, signal).await.map_err(|why| (why, true))?;
                let pattern_ref = source.get("ref").and_then(Value::as_str).unwrap_or("").to_string();
                // (the source slot named, while it still plays the pattern: its mutes go with the take)
                let plays =
                    |s: u64| source.get("slots").and_then(Value::as_array).is_some_and(|slots| slots.iter().any(|v| v.as_u64() == Some(s)));
                let epoch = pattern_ref.split(':').next().unwrap_or("").to_string();
                let named_slot = slot.filter(|s| plays(*s)).map(|s| format!("{epoch}:seq:{s}")).unwrap_or_else(|| pattern_ref.clone());
                let mut input = json!({"patternRef": named_slot, "trackRef": track, "first": first, "last": last,
                    "content": material.cells(*first), "name": name});
                // (after the take before it: the takes stay together)
                let previous: Vec<usize> =
                    set.takes.iter().rev().filter(|take| take.state != TakeState::Dropped).map(|take| take.number).take(1).collect();
                let mut found_set = set.clone();
                let found = self.find(&mut found_set, &previous, signal).await.map_err(|why| (why, true))?;
                match found
                    .first()
                    .and_then(|row| row.get("slots"))
                    .and_then(Value::as_array)
                    .and_then(|slots| slots.last())
                    .and_then(Value::as_u64)
                {
                    Some(slot) => input["index"] = json!(slot + 1),
                    None => input["section"] = json!(section),
                }
                let (change, applied) = self.change("slot_variation", input, signal).await?;
                let place = applied.pointer("/created/place").cloned().unwrap_or(Value::Null);
                let made = applied.pointer("/created/pattern").and_then(Value::as_str).unwrap_or("").to_string();
                Ok((change, place, made))
            }
            _ => Err(("this set's takes can't go in patterns".into(), false)),
        }
    }

    /// By each take's place, else (a place from before Renoise started again, or the tool watched the song anew) by the name
    /// Kumi gave it, which then gets a fresh place.
    async fn find(&self, set: &mut RenoiseSet, numbers: &[usize], signal: &Signal) -> Result<Vec<Value>, String> {
        if numbers.is_empty() {
            return Ok(vec![]);
        }
        let item = |placement: &Placement, place: Value| match placement {
            Placement::Phrases { .. } => json!({"phrase": place}),
            Placement::Patterns { .. } => json!({"pattern": place}),
        };
        let items: Vec<Value> = numbers
            .iter()
            .map(|n| item(&set.source.data.placement, set.take(*n).map(|take| take.place.clone()).unwrap_or(Value::Null)))
            .collect();
        let mut found = self.locate(items, None, signal).await?;
        let lost = |row: &Value| row.get("ref").is_none() && row.get("gone") != Some(&Value::Bool(true));
        if !found.iter().any(lost) {
            return Ok(found);
        }
        // (a name only one phrase or pattern has: a copy the producer made of a take, named the same, is never taken for it)
        let rows = self.named_rows(set, signal).await?;
        for (k, number) in numbers.iter().enumerate() {
            if !found.get(k).is_some_and(lost) {
                continue;
            }
            let Some(reference) = names(*number, &set.source.short).iter().find_map(|name| only_named(&rows, name)) else { continue };
            let again = self.locate(vec![item(&set.source.data.placement, json!(reference))], None, signal).await?;
            if let Some(row) = again.into_iter().next().filter(|row| row.get("ref").is_some()) {
                if let Some(take) = set.take_mut(*number) {
                    take.place = row.get("place").cloned().unwrap_or(Value::Null);
                }
                found[k] = row;
            }
        }
        Ok(found)
    }

    async fn current(&self, set: &RenoiseSet, row: &Value, signal: &Signal) -> Result<Lines, String> {
        let reference = row.get("ref").and_then(Value::as_str).ok_or("the take wasn't found")?;
        // (by where the takes went: a range's takes may be phrases)
        match (&set.source.data.placement, &set.source.data.kind) {
            (Placement::Patterns { .. }, SourceKind::Range { first, last, .. }) => {
                let track = self.track_now(set, signal).await?;
                let rows =
                    self.rows(json!({"kind": "line", "parent": reference, "track": track, "hidden": true, "limit": 200}), signal).await?;
                let lpb = set.source.material.as_ref().map_or(4, |material| material.lpb);
                Ok(Lines::from_rows(&rows, *first, *last, lpb))
            }
            _ => {
                let rows = self.rows(json!({"kind": "line", "parent": reference, "hidden": true, "limit": 200}), signal).await?;
                let lines = row.get("lines").and_then(Value::as_u64).unwrap_or(16) as u32;
                let lpb = row.get("lpb").and_then(Value::as_u64).unwrap_or(4) as u32;
                Ok(Lines::from_rows(&rows, 0, lines.max(1) - 1, lpb))
            }
        }
    }

    async fn rewrite(
        &self,
        set: &RenoiseSet,
        row: &Value,
        before: &Lines,
        after: &Lines,
        signal: &Signal,
    ) -> Result<Rewritten<Lines>, (String, bool)> {
        let reference = row.get("ref").and_then(Value::as_str).unwrap_or("").to_string();
        match (&set.source.data.placement, &set.source.data.kind) {
            (Placement::Patterns { .. }, SourceKind::Range { first, .. }) => {
                let track = self.track_now(set, signal).await.map_err(|why| (why, true))?;
                let input = json!({"patternRef": reference, "trackRef": track, "lines": after.rewrite(before, *first)});
                self.change("notes_write", input, signal).await.map(|(change, _)| (change, None))
            }
            _ => {
                let lines = after.rewrite(before, 0);
                let write = |lines: Vec<Value>| self.change("phrase_write", json!({"ref": reference, "lines": lines}), signal);
                if after.lpb == before.lpb {
                    return write(lines).await.map(|(change, _)| (change, None));
                }
                // (a phrase's LPB is a setting of its own, a step of its own: the lines first, when any changed, so a refusal there
                // writes nothing; an LPB Renoise then refuses leaves lines that went in, told as what the take holds)
                let written = if lines.is_empty() { None } else { Some(write(lines).await?.0) };
                match self.change("phrase_set", json!({"ref": reference, "lpb": after.lpb}), signal).await {
                    Ok((change, _)) => Ok((change, None)),
                    Err((why, stops)) => match written {
                        Some(change) => {
                            let mut holds = after.clone();
                            holds.lpb = before.lpb;
                            Ok((change, Some((holds, format!("its LPB stayed {}: {why}", before.lpb)))))
                        }
                        None => Err((why, stops)),
                    },
                }
            }
        }
    }

    async fn mark_kept(&self, set: &RenoiseSet, number: usize, row: &Value, signal: &Signal) -> Result<Option<String>, String> {
        let [named_take, kept] = names(number, &set.source.short);
        let current = row.get("name").and_then(Value::as_str).unwrap_or("");
        if current != named_take {
            return Ok(None);
        }
        let reference = row.get("ref").and_then(Value::as_str).unwrap_or("");
        let (tool, input) = match set.source.data.placement {
            Placement::Phrases { .. } => ("phrase_set", json!({"ref": reference, "name": kept})),
            Placement::Patterns { .. } => ("pattern_name", json!({"ref": reference, "name": kept})),
        };
        self.change(tool, input, signal).await.map(|(change, _)| Some(change)).map_err(|(why, _)| why)
    }

    async fn place(&self, set: &mut RenoiseSet, number: usize, row: &Value, at: &JsonObject, signal: &Signal) -> Result<Value, String> {
        let reference = row.get("ref").and_then(Value::as_str).ok_or(format!("V{number} isn't in the song any more"))?.to_string();
        let (tool, input) = match &set.source.data.placement {
            Placement::Phrases { .. } => {
                if let Some(keys) = at.get("keys") {
                    ("phrase_map", json!({"ref": reference, "noteRange": keys}))
                } else if at.get("program") == Some(&Value::Bool(true)) {
                    let instrument = self.instrument_now(set, signal).await?;
                    ("phrase_playback", json!({"ref": instrument, "mode": "program", "programRef": reference}))
                } else if let Some(switch) = at.get("switchAt").and_then(Value::as_object) {
                    let pattern = self.lengthen(switch.get("patternRef").and_then(Value::as_str).unwrap_or(""))?;
                    let track = self.lengthen(switch.get("trackRef").and_then(Value::as_str).unwrap_or(""))?;
                    let line = switch.get("line").and_then(Value::as_u64).unwrap_or(0);
                    let col = switch.get("col").and_then(Value::as_u64).unwrap_or(1);
                    let index = row.get("index").and_then(Value::as_u64).unwrap_or(1);
                    (
                        "notes_write",
                        json!({"patternRef": pattern, "trackRef": track, "merge": true, "lines": [{"line": line, "effects": [{"col": col, "number": "0Z", "amount": index}]}]}),
                    )
                } else {
                    return Err("at, for a phrase take: keys, program or switchAt".into());
                }
            }
            Placement::Patterns { .. } => {
                let SourceKind::Range { first, last, .. } = &set.source.data.kind else {
                    return Err("a pattern take needs its source".into());
                };
                let source_row = self.pattern_now(set, signal).await?;
                let track = self.track_now(set, signal).await?;
                let source = source_row.get("ref").and_then(Value::as_str).unwrap_or("");
                if at.get("over") == Some(&Value::Bool(true)) {
                    (
                        "lines_copy",
                        json!({"patternRef": reference, "trackRef": track, "first": first, "last": last, "toPatternRef": source, "toTrackRef": track, "line": first}),
                    )
                } else if at.get("after") == Some(&Value::Bool(true)) {
                    let slot = source_row
                        .get("slots")
                        .and_then(Value::as_array)
                        .and_then(|slots| slots.first())
                        .and_then(Value::as_u64)
                        .ok_or("no slot plays the source pattern now")?;
                    ("slot_insert", json!({"index": slot + 1, "patternRef": reference}))
                } else if let Some(index) = at.get("index").and_then(Value::as_u64) {
                    ("slot_insert", json!({"index": index, "patternRef": reference}))
                } else {
                    return Err("at, for a pattern take: over, after or index".into());
                }
            }
        };
        let (change, applied) = self.change(tool, input, signal).await.map_err(|(why, _)| why)?;
        Ok(json!({"change": change, "title": applied.pointer("/display/title").cloned().unwrap_or(Value::Null)}))
    }

    fn undroppable(&self, set: &RenoiseSet, _number: usize, row: &Value) -> Option<String> {
        match set.source.data.placement {
            Placement::Phrases { .. } => None,
            Placement::Patterns { .. } => {
                let slots = row.get("slots").and_then(Value::as_array).map_or(0, Vec::len);
                (slots != 1).then(|| format!("plays in {slots} slots now"))
            }
        }
    }

    async fn drop(&self, set: &RenoiseSet, rows: &[(usize, Value)], signal: &Signal) -> Result<(String, Value), String> {
        let (tool, field, key) = match set.source.data.placement {
            Placement::Phrases { .. } => ("phrases_drop", "phraseRefs", "ref"),
            Placement::Patterns { .. } => ("slots_drop", "slotRefs", "seqRef"),
        };
        let refs: Vec<Value> = rows.iter().map(|(_, row)| row.get(key).cloned().unwrap_or(Value::Null)).collect();
        self.change(tool, json!({field: refs}), signal).await.map_err(|(why, _)| why)
    }

    fn cant_render(&self) -> Option<String> {
        self.renderer.why_not()
    }

    async fn playing(&self, signal: &Signal) -> bool {
        let focus = self.plan.link().call("focus", json!({}), signal.clone()).await.ok().and_then(Result::ok).unwrap_or(Value::Null);
        focus.pointer("/transport/playing") == Some(&Value::Bool(true))
    }

    async fn render(&self, set: &mut RenoiseSet, numbers: &[usize], with_source: bool, signal: &Signal) -> Result<Rendered, String> {
        if self.renderer.listening_now() {
            return Err("Kumi is listening to something else just now".into());
        }
        let _listening = self.renderer.begin_listen();
        let lpb = set.source.material.as_ref().map_or(4, |material| material.lpb) as f64;
        // (by where the takes went: pattern takes where their slots play, phrase takes through players of their phrases)
        let rendered = match (set.source.data.placement.clone(), set.source.data.kind.clone()) {
            (Placement::Patterns { .. }, SourceKind::Range { first, last, .. }) => {
                let (source_at, track, takes) = self.where_slots(set, numbers, signal).await?;
                let (offset, beats) = (first as f64 / lpb, (last - first + 1) as f64 / lpb);
                let source = if with_source { Some(self.hear(source_at + offset, beats, &track, signal).await?) } else { None };
                let mut heard = BTreeMap::new();
                for number in numbers {
                    if let Some(at) = takes.get(number) {
                        heard.insert(*number, self.hear(at + offset, beats, &track, signal).await);
                    }
                }
                Ok((source, heard))
            }
            (Placement::Phrases { .. }, SourceKind::Range { first, last, .. }) => {
                // (the range heard where a slot plays it, when one does: what the takes are judged against)
                let mut source = None;
                if with_source {
                    if let Ok((source_at, track, _)) = self.where_slots(set, &[], signal).await {
                        let (offset, beats) = (first as f64 / lpb, (last - first + 1) as f64 / lpb);
                        source = self.hear(source_at + offset, beats, &track, signal).await.ok();
                    }
                }
                self.hear_phrases(set, numbers, false, signal).await.map(|(_, heard)| (source, heard))
            }
            _ => self.hear_phrases(set, numbers, with_source, signal).await,
        };
        // (the tempo a render played into the transport goes back, as after every listen)
        self.renderer.put_tempo_back().await;
        rendered
    }

    fn short(&self, long: &str) -> String {
        self.plan.link().book.borrow_mut().short_ref(long)
    }

    fn end_note(&self, set: &RenoiseSet) -> Option<String> {
        let Placement::Patterns { section } = &set.source.data.placement else { return None };
        Some(format!(
            "The pattern takes play at the song's end, in “{section}”: playing or rendering the whole song plays them too. Drop removes the ones not kept."
        ))
    }

    async fn extract(&self, input: &JsonObject, signal: &Signal) -> Result<Value, String> {
        let source = input.get("source").and_then(Value::as_object);
        if source.and_then(|source| source.get("patternRef")).is_none() {
            return Err("extract takes a pattern track's lines: source with patternRef, trackRef, first and last".into());
        }
        let mut asked = input.clone();
        asked.insert("place".into(), json!("phrases"));
        let info = self.source(&asked, signal).await?;
        let Placement::Phrases { settings, .. } = &info.data.placement else { return Err("extract makes a phrase".into()) };
        let material = info.material.clone().unwrap_or_else(|| empty_lines(1, 4));
        if crate::core::variations::Material::silent(&material) {
            return Err("those lines have no notes to make a phrase of".into());
        }
        let set: RenoiseSet = TakeSet::new(0, info.clone(), 0);
        let reference = self.instrument_now(&set, signal).await?;
        let mut phrase = settings.clone();
        phrase.insert("ref".into(), json!(reference));
        let name = input
            .get("options")
            .and_then(|options| options.get("name"))
            .and_then(Value::as_str)
            .map(str::to_owned)
            .unwrap_or_else(|| info.short.clone());
        phrase.insert("name".into(), json!(name.chars().take(crate::core::variations::NAME_MOST).collect::<String>()));
        phrase.insert("content".into(), Value::Array(material.cells(0)));
        let (change, applied) = self.change("phrase_add", Value::Object(phrase), signal).await.map_err(|(why, _)| why)?;
        let made = applied.pointer("/created/ref").and_then(Value::as_str).map(|r| self.short(r));
        Ok(json!({"change": change, "ref": made, "title": applied.pointer("/display/title").cloned().unwrap_or(Value::Null),
            "keyTracking": settings.get("keyTracking"), "baseNote": settings.get("baseNote")}))
    }

    fn material_json(&self, material: &Lines) -> Value {
        lines_json(material)
    }

    fn material_of(&self, value: &Value) -> Option<Lines> {
        lines_of(value)
    }

    fn data_json(&self, data: &RenoiseData) -> Value {
        data_json(data)
    }

    fn data_of(&self, value: &Value) -> Option<RenoiseData> {
        data_of(value)
    }
}

/// A set's facts as kept between runs, and read back.
fn data_json(data: &RenoiseData) -> Value {
    let kind = match &data.kind {
        SourceKind::Range { pattern, track, track_name, layout, first, last, slot } => {
            json!({"kind": "range", "pattern": pattern, "track": track, "trackName": track_name, "layout": layout, "first": first, "last": last, "slot": slot})
        }
        SourceKind::Phrase { place } => json!({"kind": "phrase", "place": place}),
        SourceKind::Instrument => json!({"kind": "instrument"}),
    };
    let placement = match &data.placement {
        Placement::Phrases { instrument, settings } => json!({"kind": "phrases", "instrument": instrument, "settings": settings}),
        Placement::Patterns { section } => json!({"kind": "patterns", "section": section}),
    };
    json!({"source": kind, "placement": placement, "instrumentName": data.instrument_name})
}

fn data_of(value: &Value) -> Option<RenoiseData> {
    let source = value.get("source")?;
    let text = |value: &Value, key: &str| value.get(key).and_then(Value::as_str).unwrap_or("").to_string();
    let kind = match source.get("kind")?.as_str()? {
        "range" => SourceKind::Range {
            pattern: source.get("pattern").cloned().unwrap_or(Value::Null),
            track: text(source, "track"),
            track_name: text(source, "trackName"),
            layout: source.get("layout").and_then(Value::as_u64),
            first: source.get("first")?.as_u64()? as u32,
            last: source.get("last")?.as_u64()? as u32,
            slot: source.get("slot").and_then(Value::as_u64),
        },
        "phrase" => SourceKind::Phrase { place: source.get("place").cloned().unwrap_or(Value::Null) },
        _ => SourceKind::Instrument,
    };
    let placement = value.get("placement")?;
    let placement = match placement.get("kind")?.as_str()? {
        "phrases" => Placement::Phrases {
            instrument: placement.get("instrument").cloned().unwrap_or(Value::Null),
            settings: placement.get("settings").and_then(Value::as_object).cloned().unwrap_or_default(),
        },
        _ => Placement::Patterns { section: text(placement, "section") },
    };
    Some(RenoiseData { kind, placement, instrument_name: text(value, "instrumentName") })
}

/// The one row named `name` (none when none or several are), its ref.
fn only_named(rows: &[Value], name: &str) -> Option<String> {
    if name.is_empty() {
        return None;
    }
    let mut same = rows.iter().filter(|row| row.get("name").and_then(Value::as_str) == Some(name));
    match (same.next(), same.next()) {
        (Some(row), None) => row.get("ref").and_then(Value::as_str).map(str::to_owned),
        _ => None,
    }
}

fn lines_json(lines: &Lines) -> Value {
    json!({"length": lines.length, "lpb": lines.lpb, "anchors": lines.anchors, "cells": lines.cells(0)})
}

fn lines_of(value: &Value) -> Option<Lines> {
    let length = value.get("length")?.as_u64()? as u32;
    let lpb = value.get("lpb")?.as_u64()? as u32;
    let mut lines = Lines::from_rows(value.get("cells")?.as_array()?, 0, length.max(1) - 1, lpb);
    lines.anchors =
        value.get("anchors").and_then(Value::as_array).into_iter().flatten().filter_map(Value::as_u64).map(|n| n as u32).collect();
    Some(lines)
}

/// What a take breaks that the source keeps: notes outside the source's pitches (shifted by a transpose asked for; an octave
/// keeps them; the anchors are the source's own), and delays past the source's latest one (or the swing as it was applied,
/// `factor` times the amount asked).
fn judge(source: Option<&Lines>, take: &Lines, ops: &[Value], factor: f64) -> Vec<String> {
    let Some(source) = source else { return vec![] };
    let mut issues = Vec::new();
    let notes = |lines: &Lines| -> Vec<(u32, u8, u8)> {
        lines
            .rows
            .iter()
            .flat_map(|(number, line)| line.notes.iter().filter_map(move |note| Some((*number, note_value(&note.note)?, note.delay))))
            .collect()
    };
    let shift: i32 = ops
        .iter()
        .filter(|op| op.get("op").and_then(Value::as_str) == Some("transpose"))
        .filter_map(|op| op.get("semitones")?.as_i64())
        .sum::<i64>() as i32;
    let pitches: BTreeSet<i32> = notes(source).iter().map(|(_, value, _)| (*value as i32 + shift).rem_euclid(12)).collect();
    if !pitches.is_empty() {
        let mut outside: Vec<String> = notes(take)
            .iter()
            .filter(|(number, value, _)| !source.anchors.contains(number) && !pitches.contains(&(*value as i32).rem_euclid(12)))
            .map(|(number, value, _)| format!("{} on line {number:02}", note_text(*value)))
            .collect();
        outside.dedup();
        if !outside.is_empty() {
            issues.push(format!("notes outside the source's key: {}", outside.into_iter().take(4).collect::<Vec<_>>().join(", ")));
        }
    }
    let swing = ops
        .iter()
        .filter_map(|op| match Op::parse(op).ok()?.scaled(factor) {
            Op::Swing { amount } => Some((amount * 96.).round() as u8),
            _ => None,
        })
        .max()
        .unwrap_or(0);
    let allowed = notes(source).iter().map(|(_, _, delay)| *delay).max().unwrap_or(0).max(swing);
    let late: Vec<String> = notes(take)
        .iter()
        .filter(|(_, _, delay)| *delay > allowed)
        .map(|(number, _, delay)| format!("line {number:02} ({delay}/256)"))
        .collect();
    if !late.is_empty() {
        issues.push(format!("off the grid past the source's feel: {}", late.into_iter().take(4).collect::<Vec<_>>().join(", ")));
    }
    issues
}

/// The anchors given (as Renoise numbers lines) within first..=last, from 0 there.
fn anchors_in(anchors: &[u32], first: u32, last: u32) -> Result<BTreeSet<u32>, String> {
    anchors
        .iter()
        .map(|line| {
            if *line < first || *line > last {
                Err(format!("anchor line {line:02} is outside the source's lines ({first:02} to {last:02})"))
            } else {
                Ok(line - first)
            }
        })
        .collect()
}

/// The instrument most of the lines' notes play (from 00), if any.
fn most_played(lines: &Lines) -> Option<u8> {
    let mut counts: BTreeMap<u8, usize> = BTreeMap::new();
    for line in lines.rows.values() {
        for note in &line.notes {
            if let (Some(instrument), true) = (note.instrument, note_value(&note.note).is_some()) {
                *counts.entry(instrument).or_default() += 1;
            }
        }
    }
    counts.into_iter().max_by_key(|(instrument, count)| (*count, std::cmp::Reverse(*instrument))).map(|(instrument, _)| instrument)
}

/// A pattern's lines as a phrase of instrument `index` (from 00) holds them: its notes play the phrase's instrument, so the
/// instrument column goes; a note of another instrument can't be in it.
fn for_phrase(lines: &Lines, index: u8) -> Result<Lines, String> {
    let mut out = lines.clone();
    for (number, line) in out.rows.iter_mut() {
        for note in &mut line.notes {
            match note.instrument {
                Some(instrument) if instrument != index => {
                    return Err(format!(
                        "line {number:02} plays instrument {instrument:02X}, not {index:02X}: a phrase plays its own instrument (or name the instrument with options.instrumentRef)"
                    ))
                }
                _ => note.instrument = None,
            }
        }
    }
    Ok(out)
}

fn first_note(lines: &Lines) -> Option<u8> {
    lines.rows.values().flat_map(|line| line.notes.iter()).find_map(|note| note_value(&note.note))
}

fn phrase_settings(given: &JsonObject, lines: u32, lpb: u32) -> JsonObject {
    let mut settings = Map::new();
    settings.insert("lines".into(), json!(given.get("lines").and_then(Value::as_u64).unwrap_or(lines as u64)));
    settings.insert("lpb".into(), json!(given.get("lpb").and_then(Value::as_u64).unwrap_or(lpb as u64)));
    for key in ["keyTracking", "baseNote"] {
        if let Some(value) = given.get(key) {
            settings.insert(key.into(), value.clone());
        }
    }
    settings
}

/// How long the lines the takes write need their phrases to be: past the last line any take writes, in bars of 16 lines.
fn written_length(input: &JsonObject, least: u32) -> u32 {
    let last = input
        .get("takes")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|take| take.get("lines").and_then(Value::as_array))
        .flatten()
        .filter_map(|line| line.get("line").and_then(Value::as_u64))
        .max();
    match last {
        Some(last) => ((last as u32 + 1).div_ceil(16) * 16).clamp(least, 512),
        None => least,
    }
}

fn empty_lines(length: u32, lpb: u32) -> Lines {
    Lines { length: length.max(1), lpb: lpb.max(1), rows: BTreeMap::new(), anchors: Default::default() }
}

fn instrument_of(phrase: &str) -> Option<String> {
    let mut parts = phrase.splitn(3, ':');
    let (epoch, kind, numbers) = (parts.next()?, parts.next()?, parts.next()?);
    (kind == "phrase").then(|| format!("{epoch}:instrument:{}", numbers.split('.').next().unwrap_or("1")))
}

fn named(row: &Value, otherwise: &str) -> String {
    row.get("name").and_then(Value::as_str).filter(|name| !name.is_empty()).map(str::to_owned).unwrap_or_else(|| otherwise.into())
}

#[cfg(test)]
mod tests {
    use super::super::renoise_lines::OPS;
    use super::*;

    #[test]
    fn the_schemas_are_json_and_every_operator_offered_is_one_renoise_has() {
        for part in [WORDS.source_schema, WORDS.op_schema, WORDS.options_schema, WORDS.at_schema] {
            serde_json::from_str::<Value>(part).unwrap();
        }
        let op: Value = serde_json::from_str(WORDS.op_schema).unwrap();
        for name in op.pointer("/properties/op/enum").unwrap().as_array().unwrap() {
            let name = name.as_str().unwrap();
            let value = if name == "transpose" { json!({"op": name, "semitones": 2}) } else { json!({"op": name}) };
            assert!(Op::parse(&value).is_ok(), "{name}");
            assert!(OPS.contains(name), "{name} is named in the operators' words");
        }
    }

    /// (V9) The tool goes with every Renoise request: its words and schema stay small beside Renoise's other tools (~62 KB).
    #[test]
    fn the_tool_is_small_beside_renoises_other_tools() {
        use crate::core::variations::tool::{description, schema};
        let size = description(&WORDS).len() + kumi_common::js::json::stringify(&Value::Object(schema(&WORDS))).len();
        assert!(size < 9_000, "the variations tool is {size} bytes");
        assert!(description(&WORDS).contains("0Sxx sample offset"), "Renoise's idioms");
    }

    #[test]
    fn a_patterns_lines_become_a_phrase_of_the_instrument_they_play() {
        let rows = vec![
            json!({"line": 0, "notes": [{"col": 1, "note": "C-4", "instrument": "02"}]}),
            json!({"line": 2, "notes": [{"col": 1, "note": "E-4", "instrument": "02", "volume": "40"}]}),
        ];
        let lines = Lines::from_rows(&rows, 0, 7, 4);
        assert_eq!(most_played(&lines), Some(2));
        let phrase = for_phrase(&lines, 2).unwrap();
        assert!(phrase.rows.values().all(|line| line.notes.iter().all(|note| note.instrument.is_none())));
        assert_eq!(first_note(&phrase), Some(48));
        assert!(for_phrase(&lines, 1).unwrap_err().contains("plays instrument 02"));
        assert_eq!(instrument_of("5:phrase:2.3").as_deref(), Some("5:instrument:2"));
        assert_eq!(written_length(json!({"takes": [{"lines": [{"line": 20}]}]}).as_object().unwrap(), 16), 32);
    }

    #[test]
    fn a_take_keeps_the_sources_key_and_feel_unless_a_transpose_or_swing_was_asked() {
        let source = Lines::from_rows(
            &[
                json!({"line": 0, "notes": [{"col": 1, "note": "C-4"}]}),
                json!({"line": 4, "notes": [{"col": 1, "note": "E-4", "delay": "10"}]}),
            ],
            0,
            15,
            4,
        );
        let mut octave = source.clone();
        octave.rows.get_mut(&0).unwrap().notes[0].note = "C-5".into();
        assert!(judge(Some(&source), &octave, &[], 1.).is_empty(), "an octave keeps the key");
        let mut sharp = source.clone();
        sharp.rows.get_mut(&4).unwrap().notes[0].note = "F#4".into();
        let issues = judge(Some(&source), &sharp, &[], 1.);
        assert!(issues[0].contains("outside the source's key: F#4 on line 04"), "{issues:?}");
        let mut up = source.clone();
        up.rows.get_mut(&0).unwrap().notes[0].note = "D-4".into();
        up.rows.get_mut(&4).unwrap().notes[0].note = "F#4".into();
        assert!(
            judge(Some(&source), &up, &[json!({"op": "transpose", "semitones": 2})], 1.).is_empty(),
            "a transpose moves the key on purpose"
        );
        let mut late = source.clone();
        late.rows.get_mut(&0).unwrap().notes[0].delay = 0x60;
        assert!(judge(Some(&source), &late, &[], 1.)[0].contains("off the grid"), "past the source's feel");
        assert!(judge(Some(&source), &late, &[json!({"op": "swing", "amount": 1})], 1.).is_empty(), "within the swing asked for");
    }

    /// (review W7) A transpose take's anchors keep the source's notes: they aren't outside its key.
    #[test]
    fn a_transpose_take_keeps_its_anchors_notes() {
        let row = |line: u32, slice: &str| json!({"line": line, "notes": [{"col": 1, "note": "C-4", "fx": format!("0S{slice}")}]});
        let mut source = Lines::from_rows(&[row(0, "00"), row(2, "40"), row(4, "80"), row(6, "C0")], 0, 7, 4);
        source.anchors = [0, 4].into_iter().collect();
        let mut take = source.clone();
        for line in [2, 6] {
            take.rows.get_mut(&line).unwrap().notes[0].note = "D-4".into();
        }
        assert!(judge(Some(&source), &take, &[json!({"op": "transpose", "semitones": 2})], 1.).is_empty());
    }

    /// (review W8) The grid allows the swing as it was applied: a re-roll's 1.35 times as much.
    #[test]
    fn a_rerolled_swing_is_judged_as_it_was_applied() {
        let source = Lines::from_rows(
            &[json!({"line": 0, "notes": [{"col": 1, "note": "C-4"}]}), json!({"line": 2, "notes": [{"col": 1, "note": "E-4"}]})],
            0,
            7,
            4,
        );
        let mut take = source.clone();
        take.rows.get_mut(&2).unwrap().notes[0].delay = 26;
        let swing = [json!({"op": "swing", "amount": 0.2})];
        assert!(judge(Some(&source), &take, &swing, 1.35).is_empty(), "26/256 is 0.2 × 1.35 of a swing");
        assert!(!judge(Some(&source), &take, &swing, 1.).is_empty(), "past the swing as first asked");
    }

    #[test]
    fn material_and_facts_come_back_as_they_were_kept() {
        let mut lines = Lines::from_rows(
            &[
                json!({"line": 3, "notes": [{"col": 1, "note": "C-4", "volume": "20"}], "effects": [{"col": 1, "number": "0S", "amount": "40"}]}),
            ],
            0,
            15,
            4,
        );
        lines.anchors = [3].into_iter().collect();
        assert_eq!(lines_of(&lines_json(&lines)), Some(lines.clone()));
        let data = RenoiseData {
            kind: SourceKind::Range {
                pattern: json!({"n": 2, "mark": "p:4"}),
                track: "1:track:3".into(),
                track_name: "Drums".into(),
                layout: Some(2),
                first: 0,
                last: 15,
                slot: Some(4),
            },
            placement: Placement::Patterns { section: "Variations of B".into() },
            instrument_name: String::new(),
        };
        assert_eq!(data_of(&data_json(&data)), Some(data));
        let phrases = RenoiseData {
            kind: SourceKind::Instrument,
            placement: Placement::Phrases {
                instrument: json!({"of": "instrument"}),
                settings: json!({"lines": 16, "lpb": 4}).as_object().cloned().unwrap(),
            },
            instrument_name: "Synth".into(),
        };
        assert_eq!(data_of(&data_json(&phrases)), Some(phrases));
    }
}
