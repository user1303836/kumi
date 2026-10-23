//! Kumi's building blocks for the devices the model patches: [kumi.transport], [kumi.clock] and [kumi.notes]. Kumi makes
//! each as a subpatcher of Max's own objects, [p kumi.clock] and so on, that the producer can open. Their timing is
//! worked out once, in [`clock`], and tested sample by sample.

pub mod clock;

use clock::{CLOCK, TRANSPORT};

/// The building blocks, as the guide lists them.
pub const NAMES: [&str; 3] = ["kumi.transport", "kumi.clock", "kumi.notes"];

/// A note value's length in beats (quarter notes): 16n is 0.25, 8nt a third, 4nd 1.5. None for anything else.
pub fn note_value(text: &str) -> Option<f64> {
    let (number, kind) = text.split_once('n')?;
    let count: u32 = number.parse().ok()?;
    if !count.is_power_of_two() || count > 128 {
        return None;
    }
    let straight = 4.0 / count as f64;
    match kind {
        "" => Some(straight),
        "t" => Some(straight * 2.0 / 3.0),
        "d" => Some(straight * 1.5),
        _ => None,
    }
}

/// The divisions a [kumi.clock] with no divisions of its own chooses from: 1n to 32n, index 4 (16n) to start.
const DIVISIONS: [&str; 6] = ["1n", "2n", "4n", "8n", "16n", "32n"];

/// A number as GenExpr and Max read it, with a point so Max keeps it a float.
fn decimal(value: f64) -> String {
    let text = format!("{value}");
    if text.contains('.') || text.contains('e') {
        text
    } else {
        format!("{text}.")
    }
}

/// What a building block's box (its text, as the model wrote it) is made of: the [p]'s text and the patch in it, in
/// Kumi's notation; or why it can't be made, said by the box's line.
pub fn expand(text: &str, line: usize) -> Result<(String, String), String> {
    let words: Vec<&str> = text.split_whitespace().collect();
    let (class, rest) = words.split_first().map(|(class, rest)| (*class, rest)).unwrap_or(("", &[]));
    match class {
        "kumi.transport" => {
            if !rest.is_empty() {
                return Err(format!("[{class}] takes no arguments"));
            }
            Ok((format!("p {class}"), transport_patch(&["position", "playing", "tempo"])))
        }
        "kumi.clock" => clock_patch(rest, line).map(|patch| (format!("p {class}"), patch)),
        "kumi.notes" => {
            let channel = match rest {
                [] => 1,
                [channel] => channel
                    .parse::<u32>()
                    .ok()
                    .filter(|channel| (1..=16).contains(channel))
                    .ok_or_else(|| format!("[{class}]'s argument is the MIDI channel, 1 to 16, not {channel}"))?,
                _ => return Err(format!("[{class}] takes one argument at most, the MIDI channel")),
            };
            Ok((format!("p {class}"), notes_patch(channel)))
        }
        _ => Err(format!(
            "[{class}] isn't one of Kumi's building blocks: they're {}",
            NAMES.iter().map(|name| format!("[{name}]")).collect::<Vec<_>>().join(", ")
        )),
    }
}

/// Live's transport into the transport's gen~ (`song`): the beat's phase, plugsync~'s song position, play flag and
/// tempo, and Live's loop settings (read through the Live API, whose answers come on Max's low-priority thread; the loop
/// rarely changes).
/// `outlets` names the song's outlets the patch sends on, by what they are: position, playing, tempo.
fn transport_patch(outlets: &[&str]) -> String {
    let mut patch = String::from(concat!(
        "# Live's song position at every sample, its tempo, and whether it plays (Kumi's transport).\n",
        "beat = [plugphasor~]\n",
        "sync = [plugsync~]\n",
        "heard = [sig~]\n",
        "flag = [sig~]\n",
        "bpm = [sig~]\n",
        "sync.6 -> heard\n",
        "sync -> flag\n",
        "sync.5 -> bpm\n",
    ));
    patch.push_str(&format!("song = [gen~] {{\n{TRANSPORT}}}\n"));
    patch.push_str(concat!(
        "beat -> song\n",
        "heard -> song.1\n",
        "flag -> song.2\n",
        "bpm -> song.3\n",
        "# Live's loop, so the steps go back with it on the sample it jumps.\n",
        "ready = [live.thisdevice]\n",
        "find = [live.path]\n",
        "ready -> (goto live_set) -> find\n",
        "each = [t l l l]\n",
        "find -> each\n",
        "looping = [live.observer loop]\n",
        "begins = [live.observer loop_start]\n",
        "lasts = [live.observer loop_length]\n",
        "each.2 -> looping.1\n",
        "each.1 -> begins.1\n",
        "each -> lasts.1\n",
        "looping -> [prepend loop_on] -> song\n",
        "begins -> [prepend loop_start] -> song\n",
        "lasts -> [prepend loop_length] -> song\n",
    ));
    for name in outlets {
        let outlet = match *name {
            "position" => 0,
            "playing" => 1,
            "sure" => 2,
            "late" => 3,
            "tempo" => 4,
            _ => 5,
        };
        patch.push_str(&format!("{name} = [outlet]\nsong.{outlet} -> {name}\n"));
    }
    patch
}

/// [kumi.clock]: its arguments are the divisions its first inlet's index picks from (note values: 16n, 8nt, 4nd…, 16n
/// by default), with @length (steps) and @swing (percent, 50 straight to 75) to start with.
fn clock_patch(arguments: &[&str], line: usize) -> Result<String, String> {
    let mut divisions: Vec<(String, f64)> = Vec::new();
    let (mut length, mut swing) = (16.0, 50.0);
    let mut words = arguments.iter();
    while let Some(word) = words.next() {
        match *word {
            "@length" | "@swing" => {
                let value = words.next().and_then(|value| value.parse::<f64>().ok()).filter(|value| value.is_finite());
                let Some(value) = value else { return Err(format!("[kumi.clock]'s {word} needs a number after it")) };
                if *word == "@length" {
                    length = value.round().clamp(1.0, 1024.0);
                } else {
                    swing = value.clamp(50.0, 75.0);
                }
            }
            word => match note_value(word) {
                Some(beats) => divisions.push((word.to_string(), beats)),
                None => {
                    return Err(format!(
                        "[kumi.clock]'s arguments are note values (1n to 128n, with t for a triplet and d for dotted: 16n, 8nt, 4nd), @length and @swing, not {word}"
                    ))
                }
            },
        }
    }
    let (divisions, first) = if divisions.is_empty() {
        (DIVISIONS.iter().map(|word| (word.to_string(), note_value(word).unwrap())).collect::<Vec<_>>(), 0.25)
    } else {
        let first = divisions[0].1;
        (divisions, first)
    };
    let clock_code = CLOCK
        .replace("Param division(0.25);", &format!("Param division({});", decimal(first)))
        .replace("Param length(16);", &format!("Param length({});", decimal(length)))
        .replace("Param swing(0);", &format!("Param swing({});", decimal((swing - 50.0) / 50.0)));
    let beats: Vec<String> = divisions.iter().map(|(_, beats)| decimal(*beats)).collect();
    // The clock's buffer: its own in each copy of the device (---) and for each clock in it (its line).
    let memory = format!("---kumi.clock.{line}");
    let mut patch = String::from(concat!(
        "# Kumi's clock: steps on Live's grid, each on the sample it's due. In: the division (an index into ",
        "the list below, or beats followed by a step's length in beats), swing (percent: 50 straight to 75) and the ",
        "steps in the pattern. Out: each step's index (from 0) on the step, its length in ms just before, and 1 or 0 ",
        "as Live starts and stops.\n",
        "division = [inlet]\n",
        "swing = [inlet]\n",
        "length = [inlet]\n",
    ));
    patch.push_str(&transport_patch(&[]));
    patch.push_str(&format!("clock = [gen~] {{\n{clock_code}}}\n"));
    patch.push_str(concat!(
        "song -> clock\n",
        "song.1 -> clock.1\n",
        "song.2 -> clock.2\n",
        "song.3 -> clock.3\n",
        "song.4 -> clock.4\n",
        "song.5 -> clock.5\n",
        "pick = [route beats]\n",
        "division -> pick\n",
        "pick -> [prepend division] -> clock\n",
    ));
    patch.push_str(&format!(
        "# The divisions, in beats: {}.\ngrid = [zl.lookup {}]\n",
        divisions.iter().map(|(word, _)| word.as_str()).collect::<Vec<_>>().join(" "),
        beats.join(" ")
    ));
    patch.push_str(concat!(
        "pick.1 -> grid\n",
        "grid -> [prepend division] -> clock\n",
        "swing -> [- 50.] -> [* 0.02] -> [prepend swing] -> clock\n",
        "length -> [prepend length] -> clock\n",
        "# A step: its length first, then its index, on the sample it starts. The clock writes both to its buffer\n",
        "# then, and peek~ reads them when edge~ answers.\n",
        "starts = [edge~]\n",
        "clock -> starts\n",
        "take = [t b b]\n",
        "starts -> take\n",
        "step = [outlet]\n",
        "long = [outlet]\n",
        "plays = [outlet]\n",
    ));
    patch.push_str(&format!(
        concat!(
            "memory = [buffer~ {memory} @samps 2]\n",
            "[loadmess memory {memory}] -> clock\n",
            "index = [peek~ {memory}]\n",
            "ms = [peek~ {memory}]\n",
        ),
        memory = memory
    ));
    patch.push_str(concat!(
        "take.1 -> (1) -> ms\n",
        "take -> (0) -> index\n",
        "index -> [i] -> step\n",
        "ms -> long\n",
        "runs = [edge~]\n",
        "clock.3 -> runs\n",
        "runs -> (1) -> plays\n",
        "runs.1 -> (0) -> plays\n",
    ));
    Ok(patch)
}

/// [kumi.notes]: makenote with its note-offs in milliseconds, which no transport holds up, and every held note ended
/// when Live stops, when the device is turned off and when it's deleted.
fn notes_patch(channel: u32) -> String {
    format!(
        concat!(
            "# Kumi's notes: each ends after its length in ms (a note-off no transport holds up), and every one ends ",
            "when Live stops, when the device is turned off and when it's deleted. In: a pitch, or a list of pitch, ",
            "velocity and length in ms; a velocity; a length in ms. Out: MIDI for out.\n",
            "note = [inlet]\n",
            "velocity = [inlet]\n",
            "duration = [inlet]\n",
            "parts = [unpack 0 0 0.]\n",
            "note -> parts\n",
            "# A pitch played again before it ended is ended first (repeatmode 1), so no earlier note-off cuts it short.\n",
            "make = [makenote 100 250 @repeatmode 1]\n",
            "parts.2 -> make.2\n",
            "parts.1 -> make.1\n",
            "parts -> make\n",
            "velocity -> make.1\n",
            "duration -> make.2\n",
            "pair = [pack 0 0]\n",
            "make.1 -> pair.1\n",
            "make -> pair\n",
            "form = [midiformat {channel}]\n",
            "pair -> form\n",
            "midi = [outlet]\n",
            "form -> midi\n",
            "stop = (stop)\n",
            "stop -> make\n",
            "sync = [plugsync~]\n",
            "sync -> [change] -> [sel 0] -> stop\n",
            "ready = [live.thisdevice]\n",
            "ready.1 -> [sel 0] -> stop\n",
            "[freebang] -> stop\n",
        ),
        channel = channel
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::devices::patch::notation::{parse, What};

    #[test]
    fn note_values_are_beats() {
        assert_eq!(note_value("4n"), Some(1.0));
        assert_eq!(note_value("16n"), Some(0.25));
        assert_eq!(note_value("1n"), Some(4.0));
        assert!((note_value("8nt").unwrap() - 1.0 / 3.0).abs() < 1e-12);
        assert_eq!(note_value("4nd"), Some(1.5));
        for wrong in ["3n", "16", "n", "16nx", "256n", "-4n"] {
            assert_eq!(note_value(wrong), None, "{wrong}");
        }
    }

    #[test]
    fn each_building_block_is_a_patch_that_reads_whole() {
        for text in ["kumi.transport", "kumi.clock", "kumi.clock 16n 8n 8nt @length 12 @swing 58", "kumi.notes", "kumi.notes 10"] {
            let (head, patch) = expand(text, 3).unwrap();
            assert!(head.starts_with("p kumi."), "{head}");
            let (notation, found) = parse(&patch);
            assert!(found.is_empty(), "{text}: {found:?}");
            assert!(notation.boxes.iter().any(|item| matches!(item.what, What::Object(ref t) if t == "inlet" || t == "outlet")));
        }
        let (_, clock) = expand("kumi.clock 16n 8n @length 12 @swing 62.5", 3).unwrap();
        assert!(clock.contains("Param division(0.25);") && clock.contains("Param length(12.);") && clock.contains("Param swing(0.25);"));
        assert!(clock.contains("[zl.lookup 0.25 0.5]"), "{clock}");
        let (_, plain) = expand("kumi.clock", 3).unwrap();
        assert!(plain.contains("[buffer~ ---kumi.clock.3 @samps 2]") && plain.contains("[loadmess memory ---kumi.clock.3]"), "{plain}");
        assert!(plain.contains("[zl.lookup 4. 2. 1. 0.5 0.25 0.125]") && plain.contains("Param division(0.25);"));
        assert!(expand("kumi.notes 10", 3).unwrap().1.contains("[midiformat 10]"));
    }

    #[test]
    fn a_building_block_given_what_it_cant_take_says_what_it_takes() {
        assert!(expand("kumi.clock 3n", 1).unwrap_err().contains("note values"));
        assert!(expand("kumi.clock @length", 1).unwrap_err().contains("@length needs a number"));
        assert!(expand("kumi.notes 17", 1).unwrap_err().contains("1 to 16"));
        assert!(expand("kumi.transport 4n", 1).unwrap_err().contains("no arguments"));
        assert!(expand("kumi.lfo", 1).unwrap_err().contains("[kumi.transport], [kumi.clock], [kumi.notes]"));
    }
}
