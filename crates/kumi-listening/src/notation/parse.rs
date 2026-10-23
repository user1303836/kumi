//! Reading the notation into notes (the grammar is in the module's docs).
use super::{
    harmony::{self, Key},
    pitch,
    time::{self, Frame, EPSILON, TICKS},
    NotationError, Note,
};
use std::cmp::Ordering;

/// How far a glide (`~`) overlaps its note into the next: a 1/64 note.
pub(super) const GLIDE: f64 = 0.0625;
/// A sequence item's length until a line sets another: a quarter note.
pub(super) const LENGTH: f64 = 1.;
/// A lane's step until it sets another: a sixteenth.
pub(super) const STEP: f64 = 0.25;
/// A lane's velocities until it sets others; `x` plays at the velocity set (`v`), 100 until one is.
pub(super) const CLASSES: [(char, f64); 3] = [('x', 100.), ('X', 127.), ('o', 60.)];
/// The most notes a text writes: what one read of a clip's notes returns.
pub const MOST: usize = 100_000;
/// The characters a lane's pattern is made of.
pub(super) const PATTERN: &str = "xXo.-23456789";

/// One token of a line, and its column (from 1).
struct Token<'a> {
    text: &'a str,
    column: usize,
}
/// The most mistakes one read reports: enough to fix a text in one go, few enough to read.
const ERRORS: usize = 12;
/// What stays set from line to line.
pub(super) struct State {
    pub velocity: f64,
    pub deviation: f64,
    pub probability: f64,
    pub length: f64,
    pub key: Option<Key>,
    /// The octave of the last pitch named with one, which a pitch named without one takes.
    pub octave: Option<i32>,
    /// What Kumi read for itself, to say with the notes (`Reading::fixed`).
    pub fixed: Vec<String>,
}
impl Default for State {
    fn default() -> Self {
        Self { velocity: 100., deviation: 0., probability: 1., length: LENGTH, key: None, octave: None, fixed: vec![] }
    }
}
/// What a text writes, read as forgivingly as is safe: the notes, and the slips Kumi read for itself, each said.
#[derive(Debug, Clone, PartialEq)]
pub struct Reading {
    pub notes: Vec<Note>,
    pub fixed: Vec<String>,
}
/// A note in the text's time, and where it was written.
#[derive(Clone)]
struct Written {
    note: Note,
    line: usize,
    column: usize,
}
/// How far a lane's pattern goes.
enum Fill {
    Once,
    Times(usize),
    To(f64),
    End,
}

/// The notes a text writes, in the clip's own time and in order; its first mistake when it has one (`read` has them all).
pub fn parse(text: &str, frame: &Frame) -> Result<Vec<Note>, NotationError> {
    read(text, frame).map(|reading| reading.notes).map_err(|mut errors| errors.swap_remove(0))
}
/// The notes a text writes, and what Kumi read for itself in it; or every mistake in it (up to a dozen), so one fix
/// mends them all, where the first used to come back alone and cost a rewrite each (#257).
///
/// Kumi reads for itself, and says so: a pitch without its octave takes the octave of the pitch named before it (`D`
/// after `C3` is D3); a length written as a bare note value (`l1`, `l8.`) is that value (`l/1`, `l/8.`); and a note
/// at or past the clip's end is left out, and one running past it is cut there (Live keeps notes past a clip's end;
/// the bridge writes them only inside it).
pub fn read(text: &str, frame: &Frame) -> Result<Reading, Vec<NotationError>> {
    let mut state = State { key: frame.key, ..State::default() };
    let mut written: Vec<Written> = vec![];
    let mut errors = vec![];
    for (index, line) in text.lines().enumerate() {
        let number = index + 1;
        if let Err(error) = read_line(line, number, frame, &mut state, &mut written) {
            errors.push(error);
            // Past the notes a text may write, or a dozen mistakes in, the rest says nothing more.
            if errors.len() >= ERRORS || written.len() >= MOST {
                break;
            }
        }
    }
    let mut notes = Vec::with_capacity(written.len());
    let (mut left_out, mut cut) = (vec![], 0);
    for Written { note, line, column } in written {
        let start = note.start - frame.origin;
        if start < -EPSILON {
            if errors.len() < ERRORS {
                errors.push(NotationError {
                    line,
                    column,
                    message: format!(
                        "a note at {} is before the clip, which starts at {}",
                        frame.position(note.start),
                        frame.position(frame.origin)
                    ),
                });
            }
            continue;
        }
        let mut duration = note.duration;
        if let Some(length) = frame.length {
            if start > length - EPSILON {
                left_out.push(frame.position(note.start));
                continue;
            }
            if start + duration > length + EPSILON {
                duration = length - start;
                cut += 1;
            }
        }
        notes.push(Note { start: start.max(0.), duration, ..note });
    }
    if !errors.is_empty() {
        return Err(errors);
    }
    notes.sort_by(order);
    let mut fixed = state.fixed;
    if let Some(length) = frame.length {
        fixed.extend(past_the_end(&left_out, cut, &frame.position(frame.origin + length)));
    }
    Ok(Reading { notes, fixed })
}
/// What leaving out the notes at or past a clip's end (by where they started) and cutting those that ran past it
/// says, if anything.
pub(crate) fn past_the_end(left_out: &[String], cut: usize, end: &str) -> Vec<String> {
    let mut said = vec![];
    if !left_out.is_empty() {
        let mut at: Vec<&str> = left_out.iter().map(String::as_str).collect();
        at.dedup();
        let more = at.len().saturating_sub(4);
        at.truncate(4);
        said.push(format!(
            "{} at or past the clip's end ({end}) {} left out: at {}{}",
            plural(left_out.len(), "note"),
            if left_out.len() == 1 { "was" } else { "were" },
            at.join(", "),
            if more > 0 { format!(" and {more} more places") } else { String::new() }
        ));
    }
    if cut > 0 {
        said.push(format!(
            "{} ran past the clip's end ({end}) and {} cut there",
            plural(cut, "note"),
            if cut == 1 { "was" } else { "were" }
        ));
    }
    said
}
fn plural(count: usize, what: &str) -> String {
    if count == 1 {
        format!("1 {what}")
    } else {
        format!("{} {what}s", thousands(count))
    }
}
/// One line of a text into the notes written so far.
fn read_line(line: &str, number: usize, frame: &Frame, state: &mut State, written: &mut Vec<Written>) -> Result<(), NotationError> {
    let tokens = tokens(line, number)?;
    let Some(first) = tokens.first() else { return Ok(()) };
    let at = |token: &Token, message: String| NotationError { line: number, column: token.column, message };
    match first.text {
        "copy" => copy(&tokens, number, frame, written),
        "key" => {
            let rest = tokens[1..].iter().map(|token| token.text).collect::<Vec<_>>().join(" ");
            state.key = Some(Key::parse(&rest).ok_or_else(|| {
                at(first, format!("“key {rest}” isn't a key: a tonic and a mode, like key C major, key D dorian or key A harmonic minor"))
            })?);
            Ok(())
        }
        text if looks_like_setting(text) || frame.parse_position(text).is_some() => sequence(&tokens, number, frame, state, written),
        _ => lane(&tokens, number, frame, state, written),
    }
}
/// Every mistake a read found, as the change's error says them: the first in full, then one a line.
pub fn errors_text(errors: &[NotationError]) -> String {
    let mut text = errors.first().map(ToString::to_string).unwrap_or_default();
    for error in errors.iter().skip(1) {
        text.push_str(&format!("\nline {}, column {}: {}", error.line, error.column, error.message));
    }
    if errors.len() >= ERRORS {
        text.push_str("\n(there may be more after these)");
    }
    text
}
/// Whether a text names a lane for a drum rather than a pitch, which the track's Drum Rack pads should answer.
pub fn names_drums(text: &str) -> bool {
    text.lines().filter_map(|line| line.split_whitespace().next()).any(|first| {
        !first.starts_with('#')
            && !matches!(first, "copy" | "key")
            && !looks_like_setting(first)
            && !first.contains('|')
            && pitch::parse(first).is_none()
    })
}
/// Notes in time order, then by pitch.
pub(super) fn order(a: &Note, b: &Note) -> Ordering {
    a.start.total_cmp(&b.start).then(a.pitch.cmp(&b.pitch)).then(a.duration.total_cmp(&b.duration))
}

/// A line's tokens: split at spaces, except inside `[…]`, `{…}` and `(…)`; `#` at a token's start ends the line.
fn tokens(line: &str, number: usize) -> Result<Vec<Token<'_>>, NotationError> {
    let mut tokens = vec![];
    let mut chars = line.char_indices().peekable();
    // The column of the next character, counted as they're taken.
    let mut next = 1;
    while let Some(&(start, first)) = chars.peek() {
        if first.is_whitespace() {
            chars.next();
            next += 1;
            continue;
        }
        if first == '#' {
            break;
        }
        let column = next;
        let mut closing = match first {
            '[' => Some(']'),
            '{' => Some('}'),
            '(' => Some(')'),
            _ => None,
        };
        let mut end = start;
        chars.next();
        next += 1;
        end += first.len_utf8();
        while let Some(&(at, c)) = chars.peek() {
            match closing {
                Some(close) if c == close => closing = None,
                Some(_) => {}
                None if c.is_whitespace() => break,
                None => {}
            }
            end = at + c.len_utf8();
            chars.next();
            next += 1;
        }
        if let Some(close) = closing {
            return Err(NotationError {
                line: number,
                column,
                message: format!("“{}” is never closed: end it with {close}", &line[start..end]),
            });
        }
        tokens.push(Token { text: &line[start..end], column });
    }
    Ok(tokens)
}
fn looks_like_setting(text: &str) -> bool {
    let mut chars = text.chars();
    match (chars.next(), chars.next()) {
        (Some('v'), Some(c)) => c.is_ascii_digit(),
        (Some('p'), Some(c)) => c.is_ascii_digit() || c == '.',
        (Some('l'), Some(c)) => c.is_ascii_digit() || c == '/' || c == '.',
        _ => false,
    }
}
/// Applies a setting (`v100`, `v80-110`, `p0.8`, `l/8`); false when the token isn't one.
pub(super) fn setting(state: &mut State, text: &str) -> Result<bool, String> {
    if !looks_like_setting(text) {
        return Ok(false);
    }
    let value = &text[1..];
    match &text[..1] {
        "v" => {
            let velocity = |text: &str| number(text).filter(|velocity| (1.0..=127.).contains(velocity));
            let wrong = || format!("“{text}” isn't a velocity: 1–127, or a range like v80-110 (its ends at most 127 apart)");
            // A range's other end is the velocity plus Live's deviation (up to 127 either way), so it may pass 1–127.
            let (low, high) = match value.split_once('-') {
                Some((low, high)) => {
                    let low = velocity(low).ok_or_else(wrong)?;
                    (low, Some(number(high).filter(|high| (high - low).abs() <= 127.).ok_or_else(wrong)?))
                }
                None => (velocity(value).ok_or_else(wrong)?, None),
            };
            state.velocity = low;
            state.deviation = high.map_or(0., |high| high - low);
        }
        "p" => {
            state.probability = number(value)
                .filter(|probability| (0.0..=1.).contains(probability))
                .ok_or_else(|| format!("“{text}” isn't a probability: 0 to 1, like p0.8"))?;
        }
        _ => {
            state.length = match time::parse_length(value) {
                Some(length) => length,
                // A note value without its slash, as other notations write it (l1 a whole note, l8. a dotted eighth).
                None => {
                    let bare = value.starts_with(|c: char| c.is_ascii_digit()) && !value.contains('/');
                    let length = time::parse_length(&format!("/{value}"))
                        .filter(|_| bare)
                        .ok_or_else(|| format!("“{text}” isn't a length: a note value (l/8, l/8., l/8t, l3/8) or beats (l0.37b)"))?;
                    state.fixed.push(format!("“{text}” was read as l/{value}"));
                    length
                }
            };
        }
    }
    Ok(true)
}
fn number(text: &str) -> Option<f64> {
    text.parse::<f64>().ok().filter(|number| number.is_finite())
}

/// A sequence: items laid one after another from a position.
fn sequence(tokens: &[Token], line: usize, frame: &Frame, state: &mut State, written: &mut Vec<Written>) -> Result<(), NotationError> {
    let mut cursor: Option<f64> = None;
    // The notes of the item before, which `_` holds on.
    let mut last: Vec<usize> = vec![];
    for token in tokens {
        let at = |message: String| NotationError { line, column: token.column, message };
        if setting(state, token.text).map_err(at)? {
            continue;
        }
        if let Some(time) = frame.parse_position(token.text) {
            cursor = Some(time);
            last.clear();
            continue;
        }
        let Some(time) = cursor else {
            return Err(at("a sequence starts with a position (bar|beat, like 1|1) before its notes".into()));
        };
        let (body, suffix) = split_item(token.text);
        let (length, glide) = suffix_of(suffix, state.length).map_err(|message| at(format!("“{}”: {message}", token.text)))?;
        match body {
            "." => last.clear(),
            "_" => {
                if last.is_empty() {
                    return Err(at("“_” holds on the item before it, and there's none since the line's last position".into()));
                }
                for index in &last {
                    written[*index].note.duration += length;
                }
            }
            _ => {
                last.clear();
                let chord = pitches(body, state).map_err(at)?;
                room(written, chord.len(), line, token.column)?;
                for (pitch, mute) in chord {
                    last.push(written.len());
                    let duration = length + if glide { GLIDE } else { 0. };
                    let note = Note {
                        mute,
                        probability: state.probability,
                        velocity_deviation: state.deviation,
                        ..Note::new(pitch, time, duration, state.velocity)
                    };
                    written.push(Written { note, line, column: token.column });
                }
            }
        }
        if glide && matches!(body, "." | "_") {
            return Err(at("“~” glides a note into the next; a rest or a hold has nothing to glide".into()));
        }
        cursor = Some(time + length);
    }
    Ok(())
}
/// An item's body (a pitch, `[…]`, `{…}`, `(…)`, `.` or `_`) and its suffix (`/8`, `:3/8`, `~`).
fn split_item(text: &str) -> (&str, &str) {
    let closing = match text.chars().next() {
        Some('[') => Some(']'),
        Some('{') => Some('}'),
        Some('(') => Some(')'),
        _ => None,
    };
    let end = match closing {
        Some(close) => text.find(close).map_or(text.len(), |at| at + close.len_utf8()),
        None => text.find(['/', ':', '~']).unwrap_or(text.len()),
    };
    text.split_at(end)
}
/// An item's length and whether it glides, from its suffix (the length set before when it has none).
fn suffix_of(mut suffix: &str, mut length: f64) -> Result<(f64, bool), String> {
    let mut glide = false;
    while !suffix.is_empty() {
        if let Some(rest) = suffix.strip_prefix('~') {
            glide = true;
            suffix = rest;
            continue;
        }
        let literal = suffix.strip_prefix(':').or_else(|| suffix.starts_with('/').then_some(suffix)).ok_or_else(|| {
            format!("“{suffix}” isn't a length or a glide: write /8, /8., /8t, :3/8 or :0.37b after a note, and ~ to glide")
        })?;
        let end = literal.find('~').unwrap_or(literal.len());
        length = time::parse_length(&literal[..end])
            .ok_or_else(|| format!("“{}” isn't a length: a note value (/8, /8., /8t, :3/8) or beats (:0.37b)", &literal[..end]))?;
        suffix = &literal[end..];
    }
    Ok((length, glide))
}
/// The pitches an item plays, each with whether it's muted.
fn pitches(body: &str, state: &mut State) -> Result<Vec<(u8, bool)>, String> {
    // Parentheses mute what's inside, however deep they go: unwrapped in a loop, as a chord's part can nest them by
    // the thousand.
    let mut inner = body;
    while let Some(within) = inner.strip_prefix('(').and_then(|inner| inner.strip_suffix(')')) {
        inner = within;
    }
    if inner.len() < body.len() {
        return Ok(pitches(inner, state)?.into_iter().map(|(pitch, _)| (pitch, true)).collect());
    }
    if let Some(inner) = body.strip_prefix('[').and_then(|body| body.strip_suffix(']')) {
        let mut chord = vec![];
        for part in inner.split_whitespace() {
            chord.extend(pitches(part, state)?);
        }
        return if chord.is_empty() { Err("“[]” has no notes: write them inside, like [C3 E3 G3]".into()) } else { Ok(chord) };
    }
    if let Some(inner) = body.strip_prefix('{').and_then(|body| body.strip_suffix('}')) {
        return Ok(harmony::chord(inner.trim(), state.key.as_ref())?.into_iter().map(|pitch| (pitch, false)).collect());
    }
    if let Some(pitch) = pitch::parse(body) {
        if !body.bytes().all(|b| b.is_ascii_digit()) {
            state.octave = Some(i32::from(pitch) / 12 - 2);
        }
        return Ok(vec![(pitch, false)]);
    }
    // A pitch named without its octave takes the octave of the one named before it: D after C3 is D3.
    if let (Some((_, "")), Some(octave)) = (pitch::class(body), state.octave) {
        if let Some(pitch) = pitch::parse(&format!("{body}{octave}")) {
            let read = format!("“{body}” has no octave, so it was read as {body}{octave}, the octave of the pitch before it");
            if !state.fixed.contains(&read) {
                state.fixed.push(read);
            }
            return Ok(vec![(pitch, false)]);
        }
    }
    Err(format!("“{body}” isn't a pitch: write Live's names (C3 is middle C, F#2, Bb1) or a MIDI number (0–127)"))
}

/// A lane: one drum or pitch, a pattern on a grid.
fn lane(tokens: &[Token], line: usize, frame: &Frame, state: &mut State, written: &mut Vec<Written>) -> Result<(), NotationError> {
    let at = |token: &Token, message: String| NotationError { line, column: token.column, message };
    let name = &tokens[0];
    let pitch = match pitch::parse(name.text) {
        Some(pitch) => pitch,
        None => pitch::drum_pitch(name.text, &frame.pads).map_err(|message| at(name, message))?,
    };
    let (mut step, mut start, mut fill, mut shift) = (STEP, frame.origin, Fill::Once, 0.);
    let mut classes = CLASSES;
    // Whether the line sets `x=`; when it doesn't, `x` plays at the velocity set.
    let mut x_set = false;
    let mut pattern: Vec<char> = vec![];
    let mut rest = tokens[1..].iter();
    while let Some(token) = rest.next() {
        let text = token.text;
        if setting(state, text).map_err(|message| at(token, message))? {
            continue;
        }
        if let Some(time) = frame.parse_position(text) {
            start = time;
        } else if text.len() > 1 && text.starts_with('-') && text[1..].bytes().all(|b| (b'2'..=b'9').contains(&b)) {
            // A hold then ratchets, or a shift back in ticks: neither is taken for the other.
            return Err(at(
                token,
                format!("“{text}” could be a shift or a pattern: write {text}t to shift the lane back by ticks, or join it to the pattern before it"),
            ));
        } else if text.chars().all(|c| PATTERN.contains(c)) {
            pattern.extend(text.chars());
        } else if text.starts_with('/') {
            step = time::parse_length(text)
                .ok_or_else(|| at(token, format!("“{text}” isn't a step: a note value like /16, /16t, /8t or /32")))?;
        } else if text == "*" {
            fill = Fill::End;
        } else if let Some(to) = text.strip_prefix("*to") {
            let to = if to.is_empty() { rest.next().map(|token| token.text) } else { Some(to) };
            let end =
                to.and_then(|to| frame.parse_position(to)).ok_or_else(|| at(token, "“*to” takes a position, like *to 17|1".into()))?;
            fill = Fill::To(end);
        } else if let Some(times) = text.strip_prefix('*') {
            let times = times.parse::<usize>().ok().filter(|times| *times > 0);
            fill = Fill::Times(
                times.ok_or_else(|| at(token, format!("“{text}” isn't a fill: * (to the clip's end), *8 (times) or *to 17|1")))?,
            );
        } else if let Some((class, value)) = text.split_once('=') {
            let velocity = number(value).filter(|velocity| (1.0..=127.).contains(velocity));
            match (classes.iter_mut().find(|(name, _)| class.len() == 1 && class.starts_with(*name)), velocity) {
                (Some((name, kept)), Some(velocity)) => {
                    *kept = velocity;
                    x_set |= *name == 'x';
                }
                _ => return Err(at(token, format!("“{text}” isn't a lane velocity: x=100, X=127 or o=60 (1–127)"))),
            }
        } else if let Some(by) = shift_of(text, frame.tempo) {
            shift = by;
        } else {
            return Err(at(
                token,
                format!("“{text}” isn't part of a lane: a pattern (x..x), a step (/16), a position (1|1), a fill (*, *8, *to 17|1), velocities (x=100) or a shift (+12, -8ms)"),
            ));
        }
    }
    if pattern.is_empty() {
        return Err(at(name, format!("the lane “{}” has no pattern: write one after it, like {} x..x..x...x..x..", name.text, name.text)));
    }
    if !x_set {
        classes[0].1 = state.velocity;
    }
    let steps = match fill {
        Fill::Once => pattern.len(),
        Fill::Times(times) => pattern.len().saturating_mul(times),
        Fill::To(end) => ((end - start) / step + EPSILON).floor().max(0.) as usize,
        Fill::End => {
            let length = frame.length.ok_or_else(|| {
                at(
                    name,
                    "“*” fills to the clip's end, and this clip's length isn't known: say how many times (*8) or until where (*to 17|1)"
                        .into(),
                )
            })?;
            ((frame.origin + length - start) / step + EPSILON).floor().max(0.) as usize
        }
    };
    if steps > MOST {
        return Err(at(
            name,
            format!("the lane “{}” runs {} steps: a lane takes at most {}", name.text, thousands(steps), thousands(MOST)),
        ));
    }
    // The notes it lays, before laying them: one a hit, a ratchet's count.
    let hits = |steps: &[char]| -> usize {
        steps.iter().map(|c| c.to_digit(10).map_or(usize::from(!matches!(c, '.' | '-')), |n| n as usize)).sum()
    };
    room(written, steps / pattern.len() * hits(&pattern) + hits(&pattern[..steps % pattern.len()]), line, name.column)?;
    let velocity = |class: char| classes.iter().find(|(name, _)| *name == class).map_or(100., |(_, velocity)| *velocity);
    let mut push = |time: f64, duration: f64, velocity: f64| {
        let note =
            Note { probability: state.probability, velocity_deviation: state.deviation, ..Note::new(pitch, time, duration, velocity) };
        written.push(Written { note, line, column: name.column });
        written.len() - 1
    };
    // The note the last hit made, which `-` holds on.
    let mut held: Option<usize> = None;
    let mut holds = vec![];
    for index in 0..steps {
        let time = start + index as f64 * step + shift;
        match pattern[index % pattern.len()] {
            '.' => held = None,
            '-' => match held {
                Some(note) => holds.push(note),
                None => return Err(at(name, "“-” holds on the hit before it, and there's none there: start with x, X or o".into())),
            },
            digit @ '2'..='9' => {
                let count = digit.to_digit(10).unwrap() as usize;
                let part = step / count as f64;
                for hit in 0..count {
                    held = Some(push(time + hit as f64 * part, part, velocity('x')));
                }
            }
            class => held = Some(push(time, step, velocity(class))),
        }
    }
    for note in holds {
        written[note].note.duration += step;
    }
    Ok(())
}
/// Refuses what would take the text past the notes it may write.
fn room(written: &[Written], adding: usize, line: usize, column: usize) -> Result<(), NotationError> {
    let total = written.len().saturating_add(adding);
    if total <= MOST {
        return Ok(());
    }
    Err(NotationError {
        line,
        column,
        message: format!("that makes {} notes: notation writes at most {} to a clip", thousands(total), thousands(MOST)),
    })
}
/// A count with its thousands marked: 100,000.
pub fn thousands(count: usize) -> String {
    let digits = count.to_string();
    let mut text = String::new();
    for (index, digit) in digits.chars().enumerate() {
        if index > 0 && (digits.len() - index).is_multiple_of(3) {
            text.push(',');
        }
        text.push(digit);
    }
    text
}

/// A lane's shift: `+12` or `-12t` in ticks (960 to a quarter), or `+8ms` at the tempo.
fn shift_of(text: &str, tempo: f64) -> Option<f64> {
    let sign = match text.chars().next()? {
        '+' => 1.,
        '-' => -1.,
        _ => return None,
    };
    let body = &text[1..];
    if let Some(ms) = body.strip_suffix("ms") {
        return number(ms).map(|ms| sign * ms * tempo / 60_000.);
    }
    number(body.strip_suffix('t').unwrap_or(body)).map(|ticks| sign * ticks / TICKS)
}

/// `copy 1-2 3-16`: the notes written so far in bars 1–2, laid again and again over bars 3–16.
fn copy(tokens: &[Token], line: usize, frame: &Frame, written: &mut Vec<Written>) -> Result<(), NotationError> {
    let at = |token: &Token, message: String| NotationError { line, column: token.column, message };
    let (Some(from), Some(to), None) = (tokens.get(1), tokens.get(2), tokens.get(3)) else {
        return Err(at(&tokens[0], "copy takes the bars to copy, then the bars to fill: copy 1-2 3-16".into()));
    };
    let range = |token: &Token| -> Result<(u32, u32), NotationError> {
        let (first, last) = token.text.split_once('-').unwrap_or((token.text, token.text));
        match (first.parse::<u32>(), last.parse::<u32>()) {
            (Ok(first), Ok(last)) if first >= 1 && first <= last => Ok((first, last)),
            _ => Err(at(token, format!("“{}” isn't bars: one bar (3) or a range (3-16)", token.text))),
        }
    };
    let ((first, last), (into, until)) = (range(from)?, range(to)?);
    if into <= last && first <= until {
        return Err(at(to, format!("bars {} overlap the bars copied ({})", to.text, from.text)));
    }
    let (source, end) = (frame.bar_start(first), frame.bar_start(last.saturating_add(1)));
    let (length, fill_end) = (end - source, frame.bar_start(until.saturating_add(1)));
    let copied: Vec<Written> =
        written.iter().filter(|w| w.note.start >= source - EPSILON && w.note.start < end - EPSILON).cloned().collect();
    if copied.is_empty() {
        return Err(at(from, format!("bars {} have no notes above this line to copy", from.text)));
    }
    let mut base = frame.bar_start(into);
    // The notes it lays, before laying them: whole laps, then the part of the last that fits.
    let laps = ((fill_end - base) / length + EPSILON).floor().max(0.);
    let last_lap = base + laps * length;
    let part = copied.iter().filter(|w| w.note.start - source + last_lap < fill_end - EPSILON).count();
    room(written, (laps as usize).saturating_mul(copied.len()).saturating_add(part), line, tokens[0].column)?;
    while base < fill_end - EPSILON {
        for w in &copied {
            let start = w.note.start - source + base;
            if start < fill_end - EPSILON {
                written.push(Written { note: Note { start, ..w.note.clone() }, line, column: tokens[0].column });
            }
        }
        base += length;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn notes(text: &str) -> Vec<(u8, f64, f64, f64)> {
        parse(text, &Frame::default()).unwrap().into_iter().map(|n| (n.pitch, n.start, n.duration, n.velocity)).collect()
    }
    fn error(text: &str) -> String {
        parse(text, &Frame { length: Some(16.), ..Frame::default() }).unwrap_err().to_string()
    }

    #[test]
    fn a_sequence_lays_its_items_one_after_another() {
        assert_eq!(
            notes("l/8 1|1 C3 D3 . E3/4 [C3 G3]\nv80 2|1 C3:3/8 _"),
            [
                (60, 0., 0.5, 100.),
                (62, 0.5, 0.5, 100.),
                (64, 1.5, 1., 100.),
                (60, 2.5, 0.5, 100.),
                (67, 2.5, 0.5, 100.),
                (60, 4., 1.5 + 0.5, 80.),
            ]
        );
        // A position inside the line, a decimal one, a glide, a muted note and a chord symbol.
        let parsed = parse("1|1 C3 1|3.25 D3~ (E3) 2|1 {Am}/1", &Frame::default()).unwrap();
        let rows: Vec<_> = parsed.iter().map(|n| (n.pitch, n.start, n.duration, n.mute)).collect();
        assert_eq!(
            rows,
            [
                (60, 0., 1., false),
                (62, 2.25, 1.0625, false),
                (64, 3.25, 1., true),
                (57, 4., 4., false),
                (60, 4., 4., false),
                (64, 4., 4., false)
            ]
        );
    }

    #[test]
    fn a_chords_note_in_parentheses_is_muted_however_deep_they_go() {
        // Unwrapped in a loop: 100,000 deep ran the stack out before.
        let deep = format!("1|1 [{}C3{} E3]", "(".repeat(100_000), ")".repeat(100_000));
        let parsed = parse(&deep, &Frame::default()).unwrap();
        assert_eq!(parsed.iter().map(|n| (n.pitch, n.mute)).collect::<Vec<_>>(), [(60, true), (64, false)]);
        assert!(error(&format!("1|1 [{}H3{}]", "(".repeat(1000), ")".repeat(1000))).contains("\u{201c}H3\u{201d} isn't a pitch"));
    }

    #[test]
    fn settings_stay_until_changed() {
        let parsed = parse("v80-110 p0.5\n1|1 C3 D3\nv100 p1 1|3 E3", &Frame::default()).unwrap();
        let rows: Vec<_> = parsed.iter().map(|n| (n.velocity, n.velocity_deviation, n.probability)).collect();
        assert_eq!(rows, [(80., 30., 0.5), (80., 30., 0.5), (100., 0., 1.)]);
    }

    #[test]
    fn a_lane_lays_a_pattern_on_its_grid() {
        // Hits, an accent, a ghost, a hold and a ratchet, twice.
        let lane = notes("kick 1|1 xX.o x-.3 *2");
        assert_eq!(lane.len(), 2 * 7);
        assert_eq!(
            &lane[..7],
            &[
                (36, 0., 0.25, 100.),
                (36, 0.25, 0.25, 127.),
                (36, 0.75, 0.25, 60.),
                (36, 1., 0.5, 100.),
                (36, 1.75, 0.25 / 3., 100.),
                (36, 1.75 + 0.25 / 3., 0.25 / 3., 100.),
                (36, 1.75 + 0.5 / 3., 0.25 / 3., 100.)
            ]
        );
        // Triplets, other velocities, a shift, and a five-step pattern filling four bars of 4/4 (polymeter).
        assert_eq!(notes("hat /8t x.. x=90 +12")[0], (42, 0.0125, 1. / 3., 90.));
        let five = notes("C1 x.... *to 5|1");
        assert_eq!((five.len(), five[1].1, five.last().unwrap().1), (13, 1.25, 15.));
        // `*` fills to the clip's end.
        let filled = parse("hat x. *", &Frame { length: Some(4.), ..Frame::default() }).unwrap();
        assert_eq!(filled.len(), 8);
    }

    #[test]
    fn song_time_is_turned_into_the_clips_and_bars_are_copied() {
        // A clip at bar 5 (16 beats in): its notes start at 5|1.
        let frame = Frame { origin: 16., length: Some(8.), ..Frame::default() };
        let parsed = parse("5|1 C3 . E3\ncopy 5 6", &frame).unwrap();
        assert_eq!(parsed.iter().map(|n| (n.pitch, n.start)).collect::<Vec<_>>(), [(60, 0.), (64, 2.), (60, 4.), (64, 6.)]);
        assert!(parse("1|1 C3", &frame).unwrap_err().to_string().contains("before the clip, which starts at 5|1"));
        // Kumi's bridge writes notes inside a clip: one running past its end is cut there, one starting at it is left
        // out, and the read says so.
        let past = read("6|4 C3/2 D3", &frame).unwrap();
        assert_eq!(past.notes.iter().map(|n| (n.pitch, n.start, n.duration)).collect::<Vec<_>>(), [(60, 7., 1.)]);
        assert_eq!(
            past.fixed,
            ["1 note at or past the clip's end (7|1) was left out: at 7|2", "1 note ran past the clip's end (7|1) and was cut there"]
        );
    }

    #[test]
    fn a_read_reports_every_mistake_and_reads_the_obvious_slips_for_itself() {
        // Every line's mistake, each with its place (#257), so one fix mends them all.
        let errors = read(
            "1|1 C3 Q3
2|1 E3
3|1 H2 G3
v200 4|1 C3",
            &Frame::default(),
        )
        .unwrap_err();
        assert_eq!(errors.iter().map(|e| (e.line, e.column)).collect::<Vec<_>>(), [(1, 8), (3, 5), (4, 1)]);
        let text = errors_text(&errors);
        assert!(text.starts_with("Notation line 1, column 8: “Q3” isn't a pitch"), "{text}");
        assert!(
            text.contains(
                "
line 3, column 5: “H2” isn't a pitch"
            ) && text.contains(
                "
line 4, column 1: “v200” isn't a velocity"
            )
        );
        // A pitch without its octave takes the one named before it, in a sequence or a chord; with none before, it's
        // still a mistake.
        let octaves = read("1|1 C3 D E [G2 B D3] F", &Frame::default()).unwrap();
        assert_eq!(octaves.notes.iter().map(|n| n.pitch).collect::<Vec<_>>(), [60, 62, 64, 55, 59, 62, 65]);
        assert_eq!(octaves.fixed.len(), 4, "said once a name: {:?}", octaves.fixed);
        assert!(octaves.fixed[0].starts_with("“D” has no octave, so it was read as D3"));
        assert!(read("1|1 D E3", &Frame::default()).is_err());
        // A length that's a bare note value is that value: l1 a whole note, l8. a dotted eighth; l3 is no note value.
        let lengths = read(
            "l1 1|1 C3
l8. 2|1 D3",
            &Frame::default(),
        )
        .unwrap();
        assert_eq!(lengths.notes.iter().map(|n| n.duration).collect::<Vec<_>>(), [4., 0.75]);
        assert_eq!(lengths.fixed, ["“l1” was read as l/1", "“l8.” was read as l/8."]);
        assert!(read("l3 1|1 C3", &Frame::default()).unwrap_err()[0].message.contains("isn't a length"));
    }

    #[test]
    fn a_lanes_x_plays_at_the_velocity_set_and_a_shift_back_takes_its_t() {
        let velocities = |text: &str| notes(text).into_iter().map(|note| note.3).collect::<Vec<_>>();
        assert_eq!(velocities("v80\nkick xXo."), [80., 127., 60.]);
        assert_eq!(velocities("kick v70 x."), [70.]);
        assert_eq!(velocities("kick v80 x x=90"), [90.]);
        let ranged = parse("v80-110\nkick x.", &Frame::default()).unwrap();
        assert_eq!((ranged[0].velocity, ranged[0].velocity_deviation), (80., 30.));
        // A range's other end may pass 1–127: Live's deviation runs to 127 either way.
        let wide = parse("v100-150 1|1 C3\nv20--40 1|2 D3", &Frame::default()).unwrap();
        assert_eq!(wide.iter().map(|n| (n.velocity, n.velocity_deviation)).collect::<Vec<_>>(), [(100., 50.), (20., -60.)]);
        assert!(error("v100-300 1|1 C3").contains("at most 127 apart"));
        // -24 could be a hold and two ratchets: a shift back in ticks says so with its t. -12 can't be a pattern.
        assert!((notes("hat 1|2 x -24t")[0].1 - (1. - 24. / 960.)).abs() < 1e-12);
        assert!(error("hat 1|2 x -24").contains("“-24” could be a shift or a pattern: write -24t"));
        assert!((notes("hat 1|2 x -12")[0].1 - (1. - 12. / 960.)).abs() < 1e-12);
    }

    #[test]
    fn roman_numerals_start_in_the_frames_key_until_a_key_line() {
        let frame = Frame { key: Key::parse("D dorian"), ..Frame::default() };
        let pitches = |text: &str| parse(text, &frame).unwrap().into_iter().map(|note| note.pitch).collect::<Vec<_>>();
        // IV in D Dorian is G major, the root from F2 to E3.
        assert_eq!(pitches("1|1 {IV}"), [55, 59, 62]);
        // A key line names another: IV in C major is F major.
        assert_eq!(pitches("key C major\n1|1 {IV}"), [53, 57, 60]);
    }

    #[test]
    fn a_text_writes_at_most_so_many_notes() {
        assert!(error("kick x *100000000").contains("runs 100,000,000 steps: a lane takes at most 100,000"));
        assert!(error("kick x... *to 4000000000|1").contains("a lane takes at most 100,000"));
        assert!(error("kick 4444 *10000").contains("that makes 160,000 notes: notation writes at most 100,000"));
        assert!(error("1|1 C3\ncopy 1 2-4000000000").contains("notation writes at most 100,000"));
        assert!(error("1|1 C3\ncopy 1 2-4294967295").contains("notation writes at most 100,000"));
        assert_eq!(notes("1|1 C3\ncopy 1 2-1000").len(), 1000);
        // Columns count characters, however long the line.
        assert!(error("1|1 F♯2 Q3").starts_with("Notation line 1, column 9:"));
    }

    #[test]
    fn errors_name_the_line_the_column_and_a_fix() {
        assert_eq!(
            error("1|1 C3\n1|1 H3"),
            "Notation line 2, column 5: “H3” isn't a pitch: write Live's names (C3 is middle C, F#2, Bb1) or a MIDI number (0–127)"
        );
        assert!(error("C3 D3").contains("isn't part of a lane"));
        assert!(error("v200 1|1 C3").contains("isn't a velocity"));
        assert!(error("1|1 C3/7").contains("isn't a length"));
        assert!(error("snr x...").contains("no drum called “snr”"));
        assert!(parse("kick x... *", &Frame::default()).unwrap_err().to_string().contains("length isn't known"));
        assert!(error("1|1 {IV}").contains("needs a key"));
        assert!(error("1|1 [C3 E3").contains("never closed"));
        assert!(error("copy 1-2 2-4").contains("overlap"));
        assert!(error("1|1 _").contains("holds on the item before it"));
        // A comment, and a sharp that isn't one.
        assert_eq!(notes("# a riff\n1|1 F#2 # the root"), [(54, 0., 1., 100.)]);
    }
}
