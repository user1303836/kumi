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
/// A lane's velocities until it sets others.
pub(super) const CLASSES: [(char, f64); 3] = [('x', 100.), ('X', 127.), ('o', 60.)];
/// The characters a lane's pattern is made of.
pub(super) const PATTERN: &str = "xXo.-23456789";

/// One token of a line, and its column (from 1).
struct Token<'a> {
    text: &'a str,
    column: usize,
}
/// What stays set from line to line.
pub(super) struct State {
    pub velocity: f64,
    pub deviation: f64,
    pub probability: f64,
    pub length: f64,
    pub key: Option<Key>,
}
impl Default for State {
    fn default() -> Self {
        Self { velocity: 100., deviation: 0., probability: 1., length: LENGTH, key: None }
    }
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

/// The notes a text writes, in the clip's own time and in order.
pub fn parse(text: &str, frame: &Frame) -> Result<Vec<Note>, NotationError> {
    let mut state = State::default();
    let mut written: Vec<Written> = vec![];
    for (index, line) in text.lines().enumerate() {
        let number = index + 1;
        let tokens = tokens(line, number)?;
        let Some(first) = tokens.first() else { continue };
        let at = |token: &Token, message: String| NotationError { line: number, column: token.column, message };
        match first.text {
            "copy" => copy(&tokens, number, frame, &mut written)?,
            "key" => {
                let rest = tokens[1..].iter().map(|token| token.text).collect::<Vec<_>>().join(" ");
                state.key = Some(Key::parse(&rest).ok_or_else(|| {
                    at(
                        first,
                        format!("“key {rest}” isn't a key: a tonic and a mode, like key C major, key D dorian or key A harmonic minor"),
                    )
                })?);
            }
            text if looks_like_setting(text) || frame.parse_position(text).is_some() => {
                sequence(&tokens, number, frame, &mut state, &mut written)?
            }
            _ => lane(&tokens, number, frame, &mut state, &mut written)?,
        }
    }
    let mut notes = Vec::with_capacity(written.len());
    for Written { note, line, column } in written {
        let start = note.start - frame.origin;
        let at = |message: String| NotationError { line, column, message };
        if start < -EPSILON {
            return Err(at(format!(
                "a note at {} is before the clip, which starts at {}",
                frame.position(note.start),
                frame.position(frame.origin)
            )));
        }
        if let Some(length) = frame.length {
            if start + note.duration > length + EPSILON {
                return Err(at(format!(
                    "a note at {} ends after the clip, which ends at {}: shorten it, or make the clip longer",
                    frame.position(note.start),
                    frame.position(frame.origin + length)
                )));
            }
        }
        notes.push(Note { start: start.max(0.), ..note });
    }
    notes.sort_by(order);
    Ok(notes)
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
    while let Some(&(start, first)) = chars.peek() {
        if first.is_whitespace() {
            chars.next();
            continue;
        }
        if first == '#' {
            break;
        }
        let column = line[..start].chars().count() + 1;
        let mut closing = match first {
            '[' => Some(']'),
            '{' => Some('}'),
            '(' => Some(')'),
            _ => None,
        };
        let mut end = start;
        chars.next();
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
            let wrong = || format!("“{text}” isn't a velocity: 1–127, or a range like v80-110");
            let (low, high) = match value.split_once('-') {
                Some((low, high)) => (velocity(low).ok_or_else(wrong)?, Some(velocity(high).ok_or_else(wrong)?)),
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
            state.length = time::parse_length(value)
                .ok_or_else(|| format!("“{text}” isn't a length: a note value (l/8, l/8., l/8t, l3/8) or beats (l0.37b)"))?;
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
                for (pitch, mute) in pitches(body, state).map_err(at)? {
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
fn pitches(body: &str, state: &State) -> Result<Vec<(u8, bool)>, String> {
    if let Some(inner) = body.strip_prefix('(').and_then(|body| body.strip_suffix(')')) {
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
    pitch::parse(body)
        .map(|pitch| vec![(pitch, false)])
        .ok_or_else(|| format!("“{body}” isn't a pitch: write Live's names (C3 is middle C, F#2, Bb1) or a MIDI number (0–127)"))
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
    let mut pattern: Vec<char> = vec![];
    let mut rest = tokens[1..].iter();
    while let Some(token) = rest.next() {
        let text = token.text;
        if setting(state, text).map_err(|message| at(token, message))? {
            continue;
        }
        if let Some(time) = frame.parse_position(text) {
            start = time;
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
                (Some((_, kept)), Some(velocity)) => *kept = velocity,
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
    let steps = match fill {
        Fill::Once => pattern.len(),
        Fill::Times(times) => pattern.len() * times,
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
    let (source, end) = (frame.bar_start(first), frame.bar_start(last + 1));
    let (length, fill_end) = (end - source, frame.bar_start(until + 1));
    let copied: Vec<Written> =
        written.iter().filter(|w| w.note.start >= source - EPSILON && w.note.start < end - EPSILON).cloned().collect();
    if copied.is_empty() {
        return Err(at(from, format!("bars {} have no notes above this line to copy", from.text)));
    }
    let mut base = frame.bar_start(into);
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
        assert!(parse("6|4 C3/2", &frame).unwrap_err().to_string().contains("ends after the clip"));
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
