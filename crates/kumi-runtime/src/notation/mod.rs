//! Kumi's notation for notes: one compact text the model writes and reads, parsed and printed here, so the bridge
//! still sees the notes it always did. A print parses back to the same notes; nothing is snapped to a grid.
//!
//! A text is lines, and `#` at the start of a token begins a comment. A line is settings, a sequence, a lane, a copy
//! or a key:
//! - **Settings** stay until changed: `v100` (velocity; `v80-110` is 80 with Live's velocity deviation of +30),
//!   `p0.8` (probability) and `l/8` (length). A length is a note value (`/8`, `/8.` dotted, `/8t` triplet, `3/8`) or
//!   beats (`0.37b`).
//! - **A sequence** starts at a position, `bar|beat` with the beat in the meter's unit (`3|2.5`, `3|1+1/3`). Its
//!   items follow one after another, each taking the length:
//!   - a pitch, in Live's names (C3 is 60; `F#2`, `Bb1`) or as a MIDI number;
//!   - a chord `[C3 E3 G3]`, or a chord symbol `{Cm7}` `{Cm7/G}`, or a roman numeral after a key (`{IV}` `{ii7}`);
//!   - a rest `.`, or `_` to hold the item before for another length.
//!   - Suffixes: `/8` (or `:3/8`) sets the item's own length, and `~` glides a note into the next (a 1/64 overlap).
//!   - `(C3)` is muted. A position inside the line moves to it, and settings may change between items.
//! - **A lane** is one drum or pitch on a grid: `kick /16 1|1 x..x..x...x..x.. *`.
//!   - The pattern: `x` hits, `X` accents, `o` is a ghost, `-` holds the hit before for a step, and `2`–`9`
//!     ratchet that many even hits into the step.
//!   - The step is a note value (`/16`, `/16t`, `/8t`, `/32`…). `*` repeats the pattern to the clip's end, `*8`
//!     plays it 8 times, and `*to 17|1` until there.
//!   - `x=100 X=127 o=60` (the defaults) set the lane's velocities. `+12` (ticks, 960 to a quarter) or `-8ms`
//!     shifts every hit.
//! - **A copy** tiles bars: `copy 1-2 3-16` lays bars 1–2 over bars 3–16, again and again.
//! - **A key** names the tonic and mode that roman numerals use: `key D dorian`.
mod harmony;
mod parse;
mod pitch;
mod print;
mod time;

pub use harmony::{chord, Key};
pub use parse::parse;
pub use pitch::{drum_pitch, name as pitch_name, parse as parse_pitch};
pub use print::{print, Printed};
pub use time::{Frame, TICKS};

/// One note, as Live keeps it: start and length in beats (quarter notes) from the clip's start.
#[derive(Debug, Clone, PartialEq)]
pub struct Note {
    pub pitch: u8,
    pub start: f64,
    pub duration: f64,
    pub velocity: f64,
    pub mute: bool,
    pub probability: f64,
    pub velocity_deviation: f64,
    /// What the notation leaves out (a print says it isn't exact when a note has them).
    pub release_velocity: Option<f64>,
    pub channel: Option<u8>,
}
impl Note {
    pub fn new(pitch: u8, start: f64, duration: f64, velocity: f64) -> Self {
        Self {
            pitch,
            start,
            duration,
            velocity,
            mute: false,
            probability: 1.,
            velocity_deviation: 0.,
            release_velocity: None,
            channel: None,
        }
    }
}

/// Where a text went wrong: its line and column (from 1), what was expected, and how to fix it.
#[derive(Debug, Clone, PartialEq)]
pub struct NotationError {
    pub line: usize,
    pub column: usize,
    pub message: String,
}
impl std::fmt::Display for NotationError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "Notation line {}, column {}: {}", self.line, self.column, self.message)
    }
}
impl std::error::Error for NotationError {}
