//! Printing notes as the notation. Drums become lanes where they fit one exactly, and the rest become sequences.
//! Every print is read back and compared with the notes. When it wouldn't read back the same, a plain print (a note
//! to a line) is used instead: nothing is ever snapped to a grid or rounded.
use super::{
    parse::{order, parse, setting, State, CLASSES, PATTERN},
    pitch,
    time::{self, Frame, EPSILON, TICKS},
    Note,
};
use std::collections::BTreeMap;

/// A print: the text, and whether it holds everything the notes do (channel and release velocity aren't written).
#[derive(Debug, Clone, PartialEq)]
pub struct Printed {
    pub text: String,
    pub exact: bool,
}

/// The notes (in the clip's own time) as the notation, in the frame's time.
pub fn print(notes: &[Note], frame: &Frame) -> Printed {
    let mut sorted = notes.to_vec();
    sorted.sort_by(order);
    let compact = compact(&sorted, frame);
    let text = if reads_back(&compact, &sorted, frame) { compact } else { plain(&sorted, frame) };
    let kept = sorted
        .iter()
        .all(|note| note.release_velocity.is_none_or(|velocity| velocity == 64.) && note.channel.is_none_or(|channel| channel <= 1));
    let exact = kept && reads_back(&text, &sorted, frame);
    Printed { text, exact }
}
/// Whether a text reads back as the notes. Times may differ by float noise; nothing else may.
fn reads_back(text: &str, notes: &[Note], frame: &Frame) -> bool {
    parse(text, &Frame { length: None, ..frame.clone() }).is_ok_and(|read| same(&read, notes))
}
/// Whether two lists hold the same notes, in any order: times to float noise, the rest exactly.
fn same(read: &[Note], notes: &[Note]) -> bool {
    // By pitch first: float noise mustn't swap two pitches that start together.
    let by_pitch = |a: &Note, b: &Note| a.pitch.cmp(&b.pitch).then(a.start.total_cmp(&b.start)).then(a.duration.total_cmp(&b.duration));
    let (mut read, mut notes) = (read.to_vec(), notes.to_vec());
    notes.sort_by(by_pitch);
    read.sort_by(by_pitch);
    let close = |a: f64, b: f64| (a - b).abs() < EPSILON;
    read.len() == notes.len()
        && read.iter().zip(&notes).all(|(a, b)| {
            a.pitch == b.pitch
                && close(a.start, b.start)
                && close(a.duration, b.duration)
                && close(a.velocity, b.velocity)
                && a.mute == b.mute
                && close(a.probability, b.probability)
                && close(a.velocity_deviation, b.velocity_deviation)
        })
}

/// Sequences first, a phrase the bars repeat written once with a copy after it, then the drums that fit lanes (after
/// the copy, so it doesn't lay them again).
fn compact(notes: &[Note], frame: &Frame) -> String {
    let mut lines = vec![];
    let mut state = State::default();
    let mut rest = notes.to_vec();
    let mut lanes = vec![];
    if frame.drums {
        let mut pitches: Vec<u8> = notes.iter().map(|note| note.pitch).collect();
        pitches.sort_unstable();
        pitches.dedup();
        for pitch in pitches {
            let hits: Vec<Note> = rest.iter().filter(|note| note.pitch == pitch).cloned().collect();
            if let Some(line) = lane(&hits, frame) {
                lanes.push((hits[0].clone(), line));
                rest.retain(|note| note.pitch != pitch);
            }
        }
    }
    let (phrase, copy) = repeat(&rest, frame);
    sequences(&phrase, frame, &mut state, &mut lines);
    lines.extend(copy);
    for (first, line) in lanes {
        lane_settings(&first, &mut state, &mut lines);
        lines.push(line);
    }
    lines.join("\n")
}
/// When the notes repeat a phrase of whole bars from their first bar to their last: the phrase's notes, and the copy
/// that lays it again over the rest. Otherwise all of the notes, and no copy.
fn repeat(notes: &[Note], frame: &Frame) -> (Vec<Note>, Option<String>) {
    let time = |note: &Note| frame.origin + note.start;
    let (Some(first), Some(last)) =
        (notes.first().map(|note| frame.bar_of(time(note))), notes.iter().map(|note| frame.bar_of(time(note))).max())
    else {
        return (notes.to_vec(), None);
    };
    let span = last - first + 1;
    let (from, end) = (frame.bar_start(first), frame.bar_start(last + 1));
    for period in 1..=span / 2 {
        let head_end = frame.bar_start(first + period);
        let (head, rest): (Vec<Note>, Vec<Note>) = notes.iter().cloned().partition(|note| time(note) < head_end - EPSILON);
        let mut tiled = vec![];
        let mut base = head_end;
        while base < end - EPSILON {
            for note in &head {
                let at = time(note) - from + base;
                if at < end - EPSILON {
                    tiled.push(Note { start: at - frame.origin, ..note.clone() });
                }
            }
            base += period as f64 * frame.bar();
        }
        if same(&tiled, &rest) {
            let bars = |a: u32, b: u32| if a == b { a.to_string() } else { format!("{a}-{b}") };
            return (head, Some(format!("copy {} {}", bars(first, first + period - 1), bars(first + period, last))));
        }
    }
    (notes.to_vec(), None)
}
/// One note to a line, with the settings it needs: what a print falls back on.
fn plain(notes: &[Note], frame: &Frame) -> String {
    let mut state = State::default();
    let lines: Vec<String> = notes
        .iter()
        .map(|note| {
            let mut line = vec![];
            settings(note, &mut state, &mut line);
            line.push(frame.position(frame.origin + note.start));
            line.push(item(&[note]));
            line.join(" ")
        })
        .collect();
    lines.join("\n")
}

/// Sequences, a line to a bar: items one after another, a position where they jump. The line's most common length is
/// set on it and other lengths go on their items; velocities and probabilities are set where they change.
fn sequences(notes: &[Note], frame: &Frame, state: &mut State, lines: &mut Vec<String>) {
    // Items: notes at one time that share a length and their settings (a chord), the rest on their own.
    let mut items: Vec<(f64, Vec<&Note>)> = vec![];
    let mut index = 0;
    while index < notes.len() {
        let start = notes[index].start;
        let onset: Vec<&Note> = notes[index..].iter().take_while(|note| note.start == start).collect();
        index += onset.len();
        let mut grouped: Vec<Vec<&Note>> = vec![];
        for note in onset {
            match grouped.iter_mut().find(|item| alike(item[0], note)) {
                Some(item) => item.push(note),
                None => grouped.push(vec![note]),
            }
        }
        items.extend(grouped.into_iter().map(|item| (frame.origin + start, item)));
    }
    let mut at = 0;
    while at < items.len() {
        let bar = frame.bar_of(items[at].0);
        let end = items[at..].iter().position(|(time, _)| frame.bar_of(*time) != bar).map_or(items.len(), |count| at + count);
        let length = most_common(items[at..end].iter().map(|(_, item)| item[0].duration));
        let (mut line, mut cursor) = (vec![], None::<f64>);
        for (index, (time, item)) in items[at..end].iter().enumerate() {
            match cursor.map(|cursor| time - cursor) {
                Some(gap) if gap.abs() < EPSILON => {}
                Some(gap) if gap > 0. && (1..=3).any(|rests| (gap - rests as f64 * state.length).abs() < EPSILON) => {
                    line.extend(std::iter::repeat_n(".".to_owned(), (gap / state.length).round() as usize));
                }
                _ => line.push(frame.position(*time)),
            }
            if index == 0 && (length - state.length).abs() >= EPSILON {
                let text = format!("l{}", time::length(length));
                let _ = setting(state, &text);
                line.push(text);
            }
            voice_settings(item[0], state, &mut line);
            let duration = item[0].duration;
            if (duration - state.length).abs() < EPSILON {
                line.push(self::item(item));
                cursor = Some(time + state.length);
            } else {
                // A length of its own: the parser takes the literal's, as written.
                let literal = time::length(duration);
                let own = time::parse_length(&literal).unwrap_or(duration);
                line.push(format!("{}{}", self::item(item), if literal.starts_with('/') { literal } else { format!(":{literal}") }));
                cursor = Some(time + own);
            }
        }
        lines.push(line.join(" "));
        at = end;
    }
}
/// The length most items have (the first of the most common, on a tie).
fn most_common(lengths: impl Iterator<Item = f64>) -> f64 {
    let mut counted: Vec<(f64, usize)> = vec![];
    for length in lengths {
        match counted.iter_mut().find(|(seen, _)| (seen - length).abs() < EPSILON) {
            Some((_, count)) => *count += 1,
            None => counted.push((length, 1)),
        }
    }
    counted.iter().fold((0., 0), |best, (length, count)| if *count > best.1 { (*length, *count) } else { best }).0
}
/// Whether two notes at one time can share an item: the same length and settings.
fn alike(a: &Note, b: &Note) -> bool {
    a.duration == b.duration && a.velocity == b.velocity && a.velocity_deviation == b.velocity_deviation && a.probability == b.probability
}
/// A pitch, or a chord of them; muted ones in parentheses.
fn item(notes: &[&Note]) -> String {
    let named = |note: &&Note| if note.mute { format!("({})", pitch::name(note.pitch)) } else { pitch::name(note.pitch) };
    match notes {
        [note] => named(note),
        _ => format!("[{}]", notes.iter().map(named).collect::<Vec<_>>().join(" ")),
    }
}
/// A note's velocity and probability, where they aren't set already.
fn voice_settings(note: &Note, state: &mut State, line: &mut Vec<String>) {
    // The length is the sequence's business: the probe has the one already set.
    let probe = Note { duration: state.length, ..note.clone() };
    settings(&probe, state, line);
}
/// The settings a note needs that aren't set already, added to the line (and kept, as the parser will).
fn settings(note: &Note, state: &mut State, line: &mut Vec<String>) {
    let mut add = |text: String, state: &mut State| {
        let _ = setting(state, &text);
        line.push(text);
    };
    if note.velocity != state.velocity || note.velocity_deviation != state.deviation {
        let text = if note.velocity_deviation == 0. {
            format!("v{}", time::decimal(note.velocity))
        } else {
            format!("v{}-{}", time::decimal(note.velocity), time::decimal(note.velocity + note.velocity_deviation))
        };
        add(text, state);
    }
    if note.probability != state.probability {
        add(format!("p{}", time::decimal(note.probability)), state);
    }
    if (note.duration - state.length).abs() >= EPSILON {
        add(format!("l{}", time::length(note.duration)), state);
    }
}

/// A lane for one pitch's notes, when they fit one exactly (the shortest of the grids they fit).
fn lane(hits: &[Note], frame: &Frame) -> Option<String> {
    let first = hits.first()?;
    if hits.iter().any(|hit| hit.mute || hit.probability != first.probability || hit.velocity_deviation != first.velocity_deviation) {
        return None;
    }
    ["/16", "/8", "/16t", "/8t", "/32", "/32t", "/4", "/64"].iter().filter_map(|step| fit(hits, step, frame)).min_by_key(|line| line.len())
}
/// The settings a lane's hits need (probability, velocity deviation), on a line of their own before it.
fn lane_settings(first: &Note, state: &mut State, lines: &mut Vec<String>) {
    let mut needed = vec![];
    if first.probability != state.probability {
        needed.push(format!("p{}", time::decimal(first.probability)));
    }
    if first.velocity_deviation != state.deviation {
        // Lanes take their velocities from their classes; the setting is for the deviation.
        let low = state.velocity.clamp(1. - first.velocity_deviation.min(0.), 127. - first.velocity_deviation.max(0.));
        needed.push(format!("v{}-{}", time::decimal(low), time::decimal(low + first.velocity_deviation)));
    }
    for text in &needed {
        let _ = setting(state, text);
    }
    if !needed.is_empty() {
        lines.push(needed.join(" "));
    }
}
/// What one step of a lane holds.
#[derive(Clone, Copy, PartialEq)]
enum Step {
    Rest,
    Hit(f64),
    Hold,
    Ratchet(usize, f64),
}
/// A lane line for the hits on one grid, if every hit sits on it exactly (with one shift for all).
fn fit(hits: &[Note], step_text: &str, frame: &Frame) -> Option<String> {
    let step = time::parse_length(step_text)?;
    let times: Vec<f64> = hits.iter().map(|hit| frame.origin + hit.start).collect();
    let start = frame.bar_start(frame.bar_of(times[0]));
    let from = times[0] - start;
    let shift = from - (from / step).round() * step;
    let mut steps: BTreeMap<usize, Vec<(f64, &Note)>> = BTreeMap::new();
    for (time, hit) in times.iter().zip(hits) {
        let at = (time - start - shift) / step;
        let index = (at + EPSILON).floor();
        if index < 0. {
            return None;
        }
        steps.entry(index as usize).or_default().push(((at - index) * step, hit));
    }
    let mut pattern: Vec<Step> = vec![];
    for (&index, notes) in &steps {
        if index < pattern.len() {
            return None;
        }
        pattern.resize(index, Step::Rest);
        match notes[..] {
            [(offset, hit)] if offset.abs() < EPSILON => {
                let ratio = hit.duration / step;
                let held = ratio.round();
                if held < 1. || (ratio - held).abs() >= 1e-9 {
                    return None;
                }
                pattern.push(Step::Hit(hit.velocity));
                pattern.extend(std::iter::repeat_n(Step::Hold, held as usize - 1));
            }
            _ if (2..=9).contains(&notes.len()) => {
                let count = notes.len();
                let part = step / count as f64;
                let even = notes.iter().enumerate().all(|(at, (offset, hit))| {
                    (offset - at as f64 * part).abs() < EPSILON
                        && (hit.duration - part).abs() < EPSILON
                        && hit.velocity == notes[0].1.velocity
                });
                if !even {
                    return None;
                }
                pattern.push(Step::Ratchet(count, notes[0].1.velocity));
            }
            _ => return None,
        }
    }
    let classes = classes(&pattern)?;
    let class_of = |velocity: f64| classes.iter().find(|(_, at)| *at == velocity).map(|(class, _)| *class);
    let chars: Vec<char> = pattern
        .iter()
        .map(|step| match step {
            Step::Rest => Some('.'),
            Step::Hold => Some('-'),
            Step::Hit(velocity) => class_of(*velocity),
            Step::Ratchet(count, _) => char::from_digit(*count as u32, 10),
        })
        .collect::<Option<_>>()?;
    // The shortest period the steps repeat in, played as many times as it takes (or until where they end).
    let length = chars.len();
    let period = (1..=length).find(|period| (0..length).all(|at| chars[at] == chars[at % period]))?;
    let times = length.div_ceil(period);
    let fill = if period == length {
        String::new()
    } else if (length..times * period).all(|at| chars[at % period] == '.') {
        format!(" *{times}")
    } else {
        format!(" *to {}", frame.position(start + length as f64 * step))
    };
    let per_bar = frame.bar() / step;
    let shown: String = if (per_bar - per_bar.round()).abs() < EPSILON && per_bar >= 1. {
        let per_bar = per_bar.round() as usize;
        chars[..period].chunks(per_bar).map(|chunk| chunk.iter().collect::<String>()).collect::<Vec<_>>().join(" ")
    } else {
        chars[..period].iter().collect()
    };
    let mut line = pitch::drum_name(hits[0].pitch, &frame.pads);
    if step_text != "/16" {
        line += &format!(" {step_text}");
    }
    if (start - frame.origin).abs() >= EPSILON {
        line += &format!(" {}", frame.position(start));
    }
    line += &format!(" {shown}{fill}");
    for (class, velocity) in &classes {
        if CLASSES.iter().all(|(name, default)| name != class || default != velocity) {
            line += &format!(" {class}={}", time::decimal(*velocity));
        }
    }
    let ticks = shift * TICKS;
    if ticks.abs() >= 1e-6 {
        let text = format!("{}{}", if ticks > 0. { "+" } else { "-" }, time::decimal(ticks.abs()));
        line += &if text.chars().all(|c| PATTERN.contains(c)) { format!(" {text}t") } else { format!(" {text}") };
    }
    Some(line)
}
/// The lane's velocity classes: at most three velocities (a ratchet's must be `x`'s), the defaults where they serve.
fn classes(pattern: &[Step]) -> Option<Vec<(char, f64)>> {
    let mut velocities: Vec<f64> = vec![];
    let mut ratchet = None;
    for step in pattern {
        match step {
            Step::Hit(velocity) if !velocities.contains(velocity) => velocities.push(*velocity),
            Step::Ratchet(_, velocity) => match ratchet {
                Some(other) if other != *velocity => return None,
                _ => ratchet = Some(*velocity),
            },
            _ => {}
        }
    }
    if let Some(velocity) = ratchet.filter(|velocity| !velocities.contains(velocity)) {
        velocities.push(velocity);
    }
    if velocities.len() > 3 {
        return None;
    }
    let default = |velocity: f64| CLASSES.iter().find(|(_, at)| *at == velocity).map(|(class, _)| *class);
    if ratchet.is_none_or(|velocity| default(velocity) == Some('x')) && velocities.iter().all(|velocity| default(*velocity).is_some()) {
        return Some(CLASSES.to_vec());
    }
    // `x` takes the ratchets' velocity (or the most used), and the others take the rest, louder first.
    let x = ratchet.unwrap_or_else(|| {
        let count = |velocity: f64| pattern.iter().filter(|step| **step == Step::Hit(velocity)).count();
        velocities.iter().copied().max_by_key(|velocity| count(*velocity)).unwrap_or(100.)
    });
    let mut others: Vec<f64> = velocities.into_iter().filter(|velocity| *velocity != x).collect();
    others.sort_by(|a, b| b.total_cmp(a));
    let mut classes = vec![('x', x)];
    classes.extend(['X', 'o'].into_iter().zip(others));
    Some(classes)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_drum_loop_prints_as_lanes_and_reads_back() {
        let frame = Frame { drums: true, ..Frame::default() };
        let text = "kick x..x..x...x..x.. *4\nsnare ....x.......x... *4\nhat x.x.x.x.x.x.x.x. *4";
        let notes = parse(text, &frame).unwrap();
        let printed = print(&notes, &frame);
        assert_eq!(printed.text, "kick x..x..x...x..x.. *4\nsnare ....x... *8\nhat x. *32");
        assert!(printed.exact);
    }

    #[test]
    fn a_melody_prints_as_sequences_with_settings_where_they_change() {
        let frame = Frame::default();
        let notes = parse("l/8 1|1 C3 D3 E3 . G3/4 [C3 E3 G3]\nv80 2|1 C3:3/8 2|3.25 D3", &frame).unwrap();
        let printed = print(&notes, &frame);
        assert_eq!(printed.text, "1|1 l/8 C3 D3 E3 . G3/4 [C3 E3 G3]\n2|1 l/4. v80 C3 2|3.25 D3/8");
        assert_eq!(parse(&printed.text, &frame).unwrap(), notes);
    }

    #[test]
    fn notes_off_any_grid_print_as_decimals_and_nothing_is_rounded() {
        let frame = Frame { drums: true, ..Frame::default() };
        let mut notes = vec![Note::new(36, 0.0234, 0.2, 101.5), Note::new(38, 1.013, 0.31, 87.)];
        notes[1].probability = 0.42;
        let printed = print(&notes, &frame);
        assert!(printed.exact, "{}", printed.text);
        let read = parse(&printed.text, &frame).unwrap();
        assert!(read.iter().zip(&notes).all(|(a, b)| (a.start - b.start).abs() < EPSILON && a.velocity == b.velocity), "{}", printed.text);
    }

    #[test]
    fn what_the_notation_leaves_out_makes_a_print_inexact() {
        let mut note = Note::new(60, 0., 1., 100.);
        note.release_velocity = Some(20.);
        assert!(!print(&[note], &Frame::default()).exact);
    }
}
