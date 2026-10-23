//! Following Logic's playhead while it plays. The first channel is the Mackie Control's timecode display: Logic sends
//! the digits that changed about every 33 ms, a CoreMIDI packet each, from the most significant down, and the helper
//! stamps each packet with the host time it came (Logic's own packets carry no time stamp). A run of descending digits
//! is one update of the display, dated by its first packet, and while the surface's PLAY LED is lit those updates are
//! the playhead's positions, which lie on a line of host time against position to a fraction of a millisecond (the
//! lab's spread: 0.4 ms, with an update now and then a few milliseconds late, which the fit leaves out). The second
//! channel, when the surface sends nothing, is the control bar's bar and beat polled: each change of beat between two
//! polls is a position at the polls' middle, good to half the time between them. Everything here is pure over what
//! was seen, so a recorded scene follows the same way.

use serde_json::Value;

/// Logic's ticks a quarter note.
pub const TICKS_PER_QUARTER: f64 = 960.0;
/// The timecode display's digits: controllers 0x40 (the last) to 0x49 (the first).
const FIRST_DIGIT: u8 = 0x49;
const LAST_DIGIT: u8 = 0x40;
const PLAY_LED: u8 = 0x5E;
/// The LEDs that say what the timecode display shows: SMPTE time or bars and beats.
const SMPTE_LED: u8 = 0x71;
const BEATS_LED: u8 = 0x72;

/// What the timecode display showed, and when its update came (host time, `mach_absolute_time` ticks).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Shown {
    pub host: u64,
    /// Bar, beat, division and tick, as the display counts them (from 1).
    pub bbt: [i64; 4],
}

/// Reads the surface's events as they come: the PLAY LED, and the timecode display's updates while it is lit.
#[derive(Debug, Clone)]
pub struct Follower {
    digits: [char; 10],
    /// The update being received: its first packet's host time and the last digit it wrote.
    burst: Option<(u64, u8)>,
    /// Whether the PLAY LED is lit now, whether it has been lit since this follower began, and whether it went out
    /// after that.
    pub playing: bool,
    pub played: bool,
    pub stopped: bool,
    /// Whether the display shows bars and beats (its BEATS LED lit), so its digits are positions.
    pub beats: bool,
    /// The updates seen while Logic played, in order.
    pub shown: Vec<Shown>,
}

impl Default for Follower {
    fn default() -> Self {
        Follower::new(true)
    }
}

/// A character of the surface's seven-segment set (as `mcu::Surface` reads it).
fn segment(value: u8) -> char {
    let c = value & 0x3F;
    if c < 0x20 {
        char::from(0x40 + c)
    } else {
        char::from(c)
    }
}

impl Follower {
    /// A follower that takes the display's updates as positions while it shows bars and beats (`beats`, as the
    /// surface's BEATS LED says now; the LED's changes come in the events).
    pub fn new(beats: bool) -> Follower {
        Follower { digits: [' '; 10], burst: None, playing: false, played: false, stopped: false, beats, shown: vec![] }
    }

    /// One of the helper's events: a `midi` event's bytes read in order; anything else is passed over.
    pub fn feed(&mut self, event: &Value) {
        if event.get("event").and_then(Value::as_str) != Some("midi") {
            return;
        }
        let host = event.get("hostTime").and_then(Value::as_u64).unwrap_or(0);
        let bytes = crate::mcu::from_hex(event.get("hex").and_then(Value::as_str).unwrap_or(""));
        let mut at = 0;
        while at < bytes.len() {
            let status = bytes[at];
            let length = match status {
                0x80..=0xBF | 0xE0..=0xEF => 3,
                0xC0..=0xDF => 2,
                0xF0 => bytes[at..].iter().position(|b| *b == 0xF7).map_or(bytes.len() - at, |end| end + 1),
                _ => 1,
            };
            if at + length > bytes.len() {
                break;
            }
            match status {
                0x90 | 0x80 if bytes[at + 1] == PLAY_LED => {
                    // The update under way belongs to the state before: kept if Logic was playing, else dropped.
                    let on = status == 0x90 && bytes[at + 2] != 0;
                    self.close();
                    if !on && self.playing {
                        self.stopped = true;
                    }
                    self.playing = on;
                    self.played |= on;
                }
                0x90 | 0x80 if matches!(bytes[at + 1], SMPTE_LED | BEATS_LED) => {
                    let on = status == 0x90 && bytes[at + 2] != 0;
                    self.close();
                    if bytes[at + 1] == BEATS_LED {
                        self.beats = on;
                    } else if on {
                        self.beats = false;
                    }
                }
                0xB0 if (LAST_DIGIT..=FIRST_DIGIT).contains(&bytes[at + 1]) => self.digit(host, bytes[at + 1], bytes[at + 2]),
                _ => {}
            }
            at += length;
        }
    }

    /// A digit: a new update when it isn't below the last digit written (each update writes its digits downwards).
    fn digit(&mut self, host: u64, controller: u8, value: u8) {
        match self.burst {
            Some((_, last)) if controller < last => {}
            _ => {
                self.close();
                self.burst = Some((host, controller));
            }
        }
        if let Some((_, last)) = self.burst.as_mut() {
            *last = controller;
        }
        self.digits[usize::from(FIRST_DIGIT - controller)] = segment(value);
    }

    /// The update received so far is whole: kept when Logic plays and the display reads as a position.
    pub fn close(&mut self) {
        if let Some((host, _)) = self.burst.take() {
            if self.playing && self.beats {
                if let Some(bbt) = crate::mcu::bbt(&self.digits.iter().collect::<String>()) {
                    self.shown.push(Shown { host, bbt });
                }
            }
        }
    }

    /// The last whole update seen while playing.
    pub fn last(&self) -> Option<&Shown> {
        self.shown.last()
    }
}

/// How the display counts: beats a bar, ticks a beat (960 a quarter note: 480 in x/8), and ticks a division.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Grid {
    pub beats_per_bar: u32,
    pub ticks_per_beat: f64,
}

impl Grid {
    /// From the time signature as the control bar shows it ("4/4", "6/8").
    pub fn of(signature: &str) -> Grid {
        let mut parts = signature.split('/').map(|part| part.trim().parse::<u32>().ok());
        let beats_per_bar = parts.next().flatten().filter(|n| *n > 0).unwrap_or(4);
        let unit = parts.next().flatten().filter(|n| *n > 0).unwrap_or(4);
        Grid { beats_per_bar, ticks_per_beat: TICKS_PER_QUARTER * 4.0 / f64::from(unit) }
    }

    /// A position as ticks from the project's start, with `per_division` ticks a division.
    pub fn ticks(&self, bbt: [i64; 4], per_division: f64) -> f64 {
        let [bar, beat, division, tick] = bbt;
        ((bar - 1) * i64::from(self.beats_per_bar) + (beat - 1)) as f64 * self.ticks_per_beat
            + (division - 1) as f64 * per_division
            + (tick - 1) as f64
    }

    /// Beats (the time signature's) as ticks.
    pub fn of_beats(&self, beats: f64) -> f64 {
        beats * self.ticks_per_beat
    }
}

/// Where a line of host time against position runs: the host time (in nanoseconds) of tick 0, nanoseconds a tick, how
/// far the positions it was fitted to lie from it, and how many it kept.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Line {
    pub origin: f64,
    pub ns_per_tick: f64,
    /// The kept positions' spread around the line (their RMS distance), in milliseconds.
    pub spread_ms: f64,
    pub kept: usize,
}

impl Line {
    /// The host time (nanoseconds) at a position in ticks.
    pub fn at(&self, ticks: f64) -> f64 {
        self.origin + ticks * self.ns_per_tick
    }
}

/// How far from the line's middle a position may lie and still be kept (a late update is left out).
const KEEP_NS: f64 = 1.0e6;

/// The line through `(host ns, ticks)` points: at the tempo's slope when the points agree with it, else at their own
/// (a tempo the control bar rounds); its place from the points' median, then the mean of those within a millisecond of
/// it. None for fewer than three points.
pub fn fit(points: &[(f64, f64)], ns_per_tick: f64) -> Option<Line> {
    if points.len() < 3 || !ns_per_tick.is_finite() || ns_per_tick <= 0.0 {
        return None;
    }
    let place = |slope: f64| -> (f64, Vec<(f64, f64)>) {
        let mut offsets: Vec<f64> = points.iter().map(|(host, ticks)| host - ticks * slope).collect();
        offsets.sort_by(f64::total_cmp);
        let median = offsets[offsets.len() / 2];
        let kept: Vec<(f64, f64)> =
            points.iter().copied().filter(|(host, ticks)| (host - ticks * slope - median).abs() <= KEEP_NS).collect();
        let origin = kept.iter().map(|(host, ticks)| host - ticks * slope).sum::<f64>() / kept.len().max(1) as f64;
        (origin, kept)
    };
    let (_, kept) = place(ns_per_tick);
    // The points' own slope, by least squares over the kept ones.
    let slope = if kept.len() >= 3 {
        let n = kept.len() as f64;
        let (mean_t, mean_h) = (kept.iter().map(|p| p.1).sum::<f64>() / n, kept.iter().map(|p| p.0).sum::<f64>() / n);
        let spread: f64 = kept.iter().map(|p| (p.1 - mean_t).powi(2)).sum();
        if spread > 0.0 {
            kept.iter().map(|p| (p.1 - mean_t) * (p.0 - mean_h)).sum::<f64>() / spread
        } else {
            ns_per_tick
        }
    } else {
        ns_per_tick
    };
    let slope = if (slope / ns_per_tick - 1.0).abs() > 0.003 && slope > 0.0 { slope } else { ns_per_tick };
    let (origin, kept) = place(slope);
    if kept.len() < 3 {
        return None;
    }
    let spread = (kept.iter().map(|(host, ticks)| (host - ticks * slope - origin).powi(2)).sum::<f64>() / kept.len() as f64).sqrt();
    Some(Line { origin, ns_per_tick: slope, spread_ms: spread / 1.0e6, kept: kept.len() })
}

/// The ticks a division of the display holds, from what it showed while playing: of the usual divisions of a beat,
/// the one whose positions lie closest to a line (a wrong one breaks it at every division); a sixteenth's when there
/// are too few to tell.
pub fn ticks_per_division(shown: &[Shown], grid: &Grid, ns_per_tick: f64, ns_per_host: f64) -> f64 {
    const DIVISIONS: [f64; 8] = [4.0, 2.0, 1.0, 3.0, 6.0, 8.0, 12.0, 16.0];
    let default = grid.ticks_per_beat / 4.0;
    if shown.len() < 6 {
        return default;
    }
    let mut best = (f64::INFINITY, default);
    for per_beat in DIVISIONS {
        let per_division = grid.ticks_per_beat / per_beat;
        // A division that the display shows past its count isn't this one.
        if shown.iter().any(|s| s.bbt[2] as f64 > per_beat || s.bbt[3] as f64 > per_division) {
            continue;
        }
        let points: Vec<(f64, f64)> = shown.iter().map(|s| (s.host as f64 * ns_per_host, grid.ticks(s.bbt, per_division))).collect();
        if let Some(line) = fit(&points, ns_per_tick) {
            let score = line.spread_ms + (shown.len() - line.kept) as f64;
            if score < best.0 {
                best = (score, per_division);
            }
        }
    }
    best.1
}

/// A poll of the control bar while playing: the host times before and after it (`mach_absolute_time` ticks) and the
/// beats from the project's start it showed (whole beats).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Poll {
    pub before: u64,
    pub after: u64,
    pub beats: f64,
    pub playing: bool,
}

/// The positions the polls date: each change of beat, at the middle between the poll before it and the poll that saw
/// it, as `(host ns, ticks)`; and the most time between two such polls (what a position may be off by, twice over).
pub fn poll_points(polls: &[Poll], grid: &Grid, ns_per_host: f64) -> (Vec<(f64, f64)>, f64) {
    let mut points = vec![];
    let mut widest: f64 = 0.0;
    for pair in polls.windows(2) {
        let (before, after) = (&pair[0], &pair[1]);
        if before.playing && after.playing && after.beats > before.beats {
            let (from, to) = (before.before as f64 * ns_per_host, after.after as f64 * ns_per_host);
            widest = widest.max(to - from);
            points.push(((from + to) / 2.0, grid.of_beats(after.beats)));
        }
    }
    (points, widest / 1.0e6)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn midi(hex: &str, host: u64) -> Value {
        json!({"event": "midi", "hex": hex, "hostTime": host, "timeStamp": 0})
    }

    #[test]
    fn the_displays_updates_are_read_while_the_play_led_is_lit() {
        let mut follower = Follower::default();
        // The playhead moved while stopped: shown, but not a playing position.
        for (hex, host) in [("B0 47 32", 10), ("B0 45 31", 11), ("B0 43 31", 12), ("B0 42 20 B0 41 20", 13), ("B0 40 31", 14)] {
            follower.feed(&midi(hex, host));
        }
        follower.feed(&midi("90 5E 7F", 100));
        assert!(follower.playing && follower.played && follower.shown.is_empty());
        // Two updates: tick 98 of bar 2, then tick 183 (each digit its own packet, the meters between).
        for (hex, host) in
            [("B0 41 39", 1000), ("D0 0F", 1001), ("B0 40 38", 1002), ("B0 42 31", 2000), ("B0 41 38", 2001), ("B0 40 33", 2002)]
        {
            follower.feed(&midi(hex, host));
        }
        assert_eq!(follower.shown, vec![Shown { host: 1000, bbt: [2, 1, 1, 98] }], "the second update isn't whole until the next");
        // The next begins with a digit no lower than the last one written: the second is whole.
        follower.feed(&midi("B0 40 34", 3000));
        assert_eq!(follower.shown[1], Shown { host: 2000, bbt: [2, 1, 1, 183] });
        // Stopped: the update under way is kept, and nothing after.
        follower.feed(&midi("90 5E 00", 4000));
        assert!(follower.stopped && !follower.playing);
        assert_eq!(follower.shown.len(), 3);
        follower.feed(&midi("B0 47 33", 5000));
        follower.close();
        assert_eq!(follower.shown.len(), 3);
        assert_eq!(follower.last().unwrap().bbt, [2, 1, 1, 184]);
        follower.feed(&json!({"event": "app", "state": "activated"}));
        // The display on SMPTE time: its digits aren't positions.
        let mut smpte = Follower::new(true);
        smpte.feed(&midi("90 5E 7F", 1));
        smpte.feed(&midi("90 71 7F", 2));
        for (hex, host) in [("B0 41 31", 10), ("B0 40 32", 11), ("B0 40 33", 50)] {
            smpte.feed(&midi(hex, host));
        }
        smpte.close();
        assert!(smpte.shown.is_empty() && !smpte.beats);
        assert!(!Follower::new(false).beats);
    }

    #[test]
    fn a_line_is_fitted_through_the_positions_leaving_late_ones_out() {
        let grid = Grid::of("4/4");
        assert_eq!(grid.ticks([2, 1, 1, 1], 240.0), 3840.0);
        assert_eq!(grid.ticks([1, 2, 3, 41], 240.0), 960.0 + 480.0 + 40.0);
        assert_eq!(Grid::of("6/8"), Grid { beats_per_bar: 6, ticks_per_beat: 480.0 });
        // 160 BPM: 960 ticks in 375 ms; positions every 33 ms with a little jitter, two of them late.
        let ns_per_tick = 375.0e6 / 960.0;
        let mut points: Vec<(f64, f64)> = (0..40)
            .map(|i| (5.0e9 + i as f64 * 33.0e6 + if i % 3 == 0 { 0.2e6 } else { -0.1e6 }, i as f64 * 33.0e6 / ns_per_tick))
            .collect();
        points[7].0 += 4.0e6;
        points[20].0 += 5.5e6;
        let line = fit(&points, ns_per_tick).unwrap();
        assert!((line.origin - 5.0e9).abs() < 0.2e6, "{line:?}");
        assert_eq!((line.kept, line.ns_per_tick), (38, ns_per_tick));
        assert!(line.spread_ms < 0.2);
        assert!((line.at(960.0) - (5.0e9 + 375.0e6)).abs() < 0.2e6);
        // A tempo the control bar rounded: the points' own slope.
        let off: Vec<(f64, f64)> = (0..40).map(|i| (i as f64 * 33.0e6, i as f64 * 33.0e6 / (ns_per_tick * 1.01))).collect();
        assert!((fit(&off, ns_per_tick).unwrap().ns_per_tick / (ns_per_tick * 1.01) - 1.0).abs() < 1e-9);
        assert_eq!(fit(&points[..2], ns_per_tick), None);
    }

    #[test]
    fn the_division_is_the_one_the_positions_line_up_with_and_polls_date_each_new_beat() {
        let grid = Grid::of("4/4");
        let ns_per_tick = 375.0e6 / 960.0;
        // Sixteenths (240 ticks a division) shown every 33 ms, the host clock in nanoseconds.
        let shown: Vec<Shown> = (0..30)
            .map(|i| {
                let ticks = (i as f64 * 33.0e6 / ns_per_tick) as i64;
                let (beat, rest) = (ticks / 960, ticks % 960);
                Shown { host: 1_000_000_000 + i * 33_000_000, bbt: [1 + beat / 4, 1 + beat % 4, 1 + rest / 240, 1 + rest % 240] }
            })
            .collect();
        assert_eq!(ticks_per_division(&shown, &grid, ns_per_tick, 1.0), 240.0);
        assert_eq!(ticks_per_division(&shown[..3], &grid, ns_per_tick, 1.0), 240.0, "too few: a sixteenth's");
        let polls = [
            Poll { before: 0, after: 20, beats: 4.0, playing: true },
            Poll { before: 30, after: 50, beats: 4.0, playing: true },
            Poll { before: 60, after: 80, beats: 5.0, playing: true },
            Poll { before: 90, after: 110, beats: 5.0, playing: false },
        ];
        let (points, widest) = poll_points(&polls, &grid, 1.0e6);
        assert_eq!(points, vec![(55.0e6, 4800.0)]);
        assert_eq!(widest, 50.0);
    }
}
