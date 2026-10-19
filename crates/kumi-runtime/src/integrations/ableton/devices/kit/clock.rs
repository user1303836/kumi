//! Kumi's timing building blocks: [kumi.transport], [kumi.clock] and [kumi.notes]. Each is a subpatcher of Max's own
//! objects that the producer can open, with its timing decided in gen~ at every sample.
//!
//! What Live gives a Max for Live device, as recorded in Live 12:
//! - Live starts, stops, moves the song position and changes tempo on a signal vector's first sample.
//! - plugsync~ says then whether Live plays, its tempo and where the song is in beats: while Live plays, where it is
//!   less a vector at the tempo (to a sample); on the vector Live starts, moves or changes tempo, already where it went.
//! - plugphasor~ is the beat's phase, exact while Live plays steadily. It only takes Live's word on a vector's first
//!   sample, so it runs on past Live's loop until then; when Live's phase is behind its own (a jump, a loop, a start,
//!   a tempo that falls) it holds still for up to a vector; while Live is stopped it holds (its reference says 0).
//! - A tempo change moves the song position on or back by up to an audio buffer's worth of the change in its advance
//!   (a few hundredths of a beat at Live's 512-sample buffer, more at its largest).
//! - Live takes a device's MIDI once per audio buffer (several vectors), and Max's messages from a buffer's last vector
//!   come too late for it: a note due there goes out at the next buffer's start, up to a vector late.
//!
//! Kumi's transport takes where Live is from plugsync~, and the fraction of a sample from the phase while the two
//! agree. It foresees Live's loop from its loop settings, so the steps go back on the sample Live does.
//!
//! Kumi's clock takes its steps from that position, on the grid of its division. The rules:
//! - A step starts on the sample it's due.
//! - After a start or a jump, the steps follow the song position.
//! - A change of division, swing or length lands on the next step of the new grid. It never doubles a step or puts
//!   one off the grid, and two steps are never closer than the shorter of the old and new grids' shortest steps.
//!
//! Its notes end in milliseconds worked out from the tempo, so no note waits on a transport that's stopped or has
//! jumped. They all end when Live stops, when the device is turned off and when it's deleted.

/// The transport's GenExpr: in1 is plugphasor~, in2 to in4 plugsync~'s song position in beats, play flag and tempo;
/// Params say Live's loop. out1: the song position in beats; out2 and out3: 1 while Live plays; out4: how many samples
/// back a step's line can be and still start now; out5: the tempo (BPM); out6: 1 where Live started, moved the song
/// position or looped, 2 where a tempo change moved it on (the steps follow it, and none starts twice).
pub const TRANSPORT: &str = r#"// Made by Kumi: Live's transport at every sample (fixed).
// Live starts, stops, moves the song position and changes tempo on a signal vector's first sample.
// in2 to in4: plugsync~'s song position (beats), whether Live plays, and its tempo, on each vector's first sample. While
// Live plays, the position is where Live is on that sample less a vector at the tempo, to a sample; on the vector Live
// starts, moves or changes tempo it's already where Live went, and when Live stops it's where Live stopped.
// in1: plugphasor~, the beat's phase: exact while Live plays steadily, but it runs on past Live's loop and holds still
// for up to a vector after Live moves or changes tempo, and while Live is stopped it holds or reads 0. So plugsync~ says
// where Live is, and the phase, while it agrees, says it to a fraction of a sample.
Param loop_on(0);
Param loop_start(0);
Param loop_length(4);
History t_position(0);
History t_advance(0);
History t_playing(0);
History t_heard(0);
History t_flag(0);
History t_bpm(0);
History t_phase(0);
phase = in1 - floor(in1);
heard = in2;
flag = in3;
bpm = in4;
position = t_position;
advance = t_advance;
playing = t_playing;
fresh = heard != t_heard || flag != t_flag || bpm != t_bpm;
moved = 0;
late = 0;
// The song position's advance per sample at Live's tempo.
told = advance;
if (bpm > 0) {
    told = bpm / 60 / samplerate;
}
if (told <= 0) {
    told = 2 / samplerate;
}
loop_end = loop_start + loop_length;
if (fresh > 0.5 && flag < 0.5) {
    // Live stops on this sample, or is stopped where plugsync~ says.
    playing = 0;
    position = heard;
} else if (fresh > 0.5 && playing < 0.5) {
    // Live starts on this sample: a step on a line here or just before starts now.
    playing = 1;
    advance = told;
    position = heard + vectorsize * advance;
    moved = 1;
    late = vectorsize;
} else if (playing > 0.5) {
    position = position + advance;
    anchored = 0;
    if (fresh > 0.5) {
        said = heard + vectorsize * told;
        gap = said - position;
        if (abs(gap) > 1.5 * max(advance, told)) {
            position = said;
            anchored = 1;
            if (bpm != t_bpm && abs(gap) < 8192 * abs(told - advance) + 2 * told) {
                // A tempo change moves Live's song position on or back by up to an audio buffer's worth of the change
                // (Live's buffer is up to 2048 samples, 8192 to spare). On: the steps follow it (2), and the step it
                // moved into starts now if it passed that step's line. Back: a line it goes back over doesn't start
                // again.
                late = max(gap, 0) / told + 1;
                moved = 2 * (gap > 0);
            } else {
                // Live moved the song position.
                moved = 1;
                late = vectorsize;
            }
        }
        advance = told;
    }
    // The phase, while it moves and agrees, puts the song position to a fraction of a sample.
    off = phase - (position - floor(position));
    off = off - floor(off + 0.5);
    if (in1 != t_phase && abs(off) < 1.5 * advance) {
        position = position + off;
    }
    // Live's arrangement loop goes back to its start on the sample the song position reaches its end, though
    // plugphasor~ and plugsync~ only say so on a later vector: the steps go back on that sample. The loop's end is
    // its start, however near it rounding leaves the song position; where plugsync~ just said where Live is, it's so.
    if (anchored < 0.5 && loop_on > 0.5 && loop_length > 0 && t_position >= loop_start && t_position < loop_end && position >= loop_end - 0.000000001) {
        position = max(position - loop_length, loop_start);
        moved = 1;
        late = 1;
    }
}
t_position = position;
t_advance = advance;
t_playing = playing;
t_heard = heard;
t_flag = flag;
t_bpm = bpm;
t_phase = in1;
out1 = position;
out2 = playing;
out3 = playing;
out4 = late;
out5 = advance * 60 * samplerate;
out6 = moved;
"#;

/// The clock's GenExpr: in1–in6 are the transport's outlets; Params set a step's length in beats (division), how late
/// every second step comes as a fraction of a step (swing, 0 to 0.5) and the steps before the pattern starts again
/// (length). out1: 1 on the sample a step starts; out2: that step's index, from 0; out3: its length in milliseconds at
/// the tempo then; out4: 1 while Live plays.
///
/// Each step's index and length also go into the Buffer `memory` (samples 0 and 1) on the sample it starts, for the
/// patch to read with [peek~] when edge~ answers the pulse. snapshot~ can't be used for them: in Max for Live it reads
/// the first sample of its vector, before a step that starts inside it.
pub const CLOCK: &str = r#"// Made by Kumi: the clock's steps, each on the sample it's due (fixed).
// in1 to in6: Kumi's transport (the song position in beats, playing, playing, how late, the tempo, moved).
Param division(0.25);
Param swing(0);
Param length(16);
Buffer memory;
History c_count(-1);
History c_index(0);
History c_shortest(0.25);
History c_follow(1);
History c_pair(-1);
History c_second(0);
History c_length(0);
History c_from(-1000000000);
History c_each(0);
History c_fired_pair(-1);
History c_reached(-1000000000);
position = in1;
playing = in2;
sure = in3;
late = in4;
tempo = in5;
moved = in6;
was_count = c_count;
index = c_index;
was_shortest = c_shortest;
follow = c_follow;
was_pair = c_pair;
was_second = c_second;
step_ms = c_length;
was_each = c_each;
fired_pair = c_fired_pair;
reached = c_reached;
from = c_from;
if (playing < 0.5) {
    follow = 1;
    reached = -1000000000;
}
if (moved > 0.5) {
    follow = 1;
    // A tempo change that moves the song on (2) keeps what's started: the step just played doesn't start again.
    if (moved < 1.5) {
        fired_pair = -1;
        reached = -1000000000;
    }
}
each = max(division, 0.001);
amount = clamp(swing, 0, 0.5);
steps = max(floor(length + 0.5), 1);
pair = each * 2;
offbeat = each + each * amount;
shortest = pair - offbeat;
pairs = floor(position / pair);
into = position - pairs * pair;
second = into >= offbeat;
count = pairs * 2 + second;
begun = pairs * pair + second * offbeat;
since = position - begun;
per_sample = max(tempo, 1) / 60 / samplerate;
tolerance = (late + 1.5) * per_sample;
// How far apart in beats steps are kept, and how far the song has gone since the last: in song position, so a tempo
// change between two steps, and the little way Live moves the song position with it, doesn't hold the second back.
since_beats = position - from;
fewest = min(shortest, was_shortest) - 1.5 * per_sample;
same_grid = abs(each - was_each) < 0.0000001;
if (same_grid < 0.5) {
    fired_pair = -1;
}
// A swing change can move the second step of a pair to before the sample now: it starts now, not never. Only with
// the division as it was: another division's pairs are other pairs.
swung = sure > 0.5 && same_grid > 0.5 && pairs == was_pair && second > 0.5 && was_second < 0.5;
due = since < tolerance || swung;
// A pair's second step starts once, however the swing moves it, and a line the song position went back over (a
// tempo change can move it back a little) doesn't start again.
again = (second > 0.5 && pairs == fired_pair) || begun <= reached + 0.0000001;
fire = 0;
if (playing > 0.5 && sure > 0.5 && due && again < 0.5 && (count != was_count || follow > 0.5)) {
    if (follow > 0.5 || since_beats >= fewest) {
        fire = 1;
    }
}
if (fire > 0.5) {
    if (follow > 0.5) {
        index = count - floor(count / steps) * steps;
    } else {
        index = index + 1;
        if (index >= steps) {
            index = 0;
        }
    }
    // Measured from the step's line, not the sample it started on (a step after a start or a jump can start late);
    // a second step the swing moved behind the sample now counts from now.
    from = position;
    if (since < tolerance) {
        from = begun;
    }
    was_shortest = shortest;
    follow = 0;
    if (second > 0.5) {
        fired_pair = pairs;
    }
    reached = begun;
    step_ms = offbeat * 60000 / max(tempo, 1);
    if (second > 0.5) {
        step_ms = shortest * 60000 / max(tempo, 1);
    }
    poke(memory, index, 0, 0);
    poke(memory, step_ms, 1, 0);
}
if (sure > 0.5) {
    was_count = count;
    was_pair = pairs;
    was_second = second;
}
c_count = was_count;
c_index = index;
c_shortest = was_shortest;
c_follow = follow;
c_pair = was_pair;
c_second = was_second;
c_length = step_ms;
c_from = from;
c_each = each;
c_fired_pair = fired_pair;
c_reached = reached;
out1 = fire;
out2 = index;
out3 = step_ms;
out4 = playing;
"#;

#[cfg(test)]
mod tests {
    use super::*;
    use kumi_genexpr::live::{run, run_tracing, Change, Device, Live, Step, RATE, VECTOR};

    /// How Live and Max can differ without changing a step: a History's new value read at once or at the next sample,
    /// plugphasor~ holding or reading 0 while stopped, and plugsync~'s position a sample off now and then.
    #[derive(Debug, Clone, Copy)]
    struct Way {
        delayed: bool,
        zero_when_stopped: bool,
        jitter: usize,
    }

    impl Way {
        fn live(&self) -> Live {
            let mut live = Live::new();
            live.zero_when_stopped = self.zero_when_stopped;
            live.jitter = self.jitter;
            live
        }

        fn device(&self, division: f64, length: f64) -> Device {
            Device::new(TRANSPORT, CLOCK, self.delayed, division, length)
        }
    }

    /// Every way gives the same steps, to the sample.
    fn each_way(test: impl Fn(Way) -> Vec<Step>) -> Vec<Step> {
        let plain = Way { delayed: false, zero_when_stopped: false, jitter: 0 };
        let first = test(plain);
        for delayed in [false, true] {
            for zero_when_stopped in [false, true] {
                for jitter in [0, 7] {
                    let way = Way { delayed, zero_when_stopped, jitter };
                    let again = test(way);
                    assert_eq!(first.len(), again.len(), "{way:?}: {first:?}\n{again:?}");
                    for (a, b) in first.iter().zip(&again) {
                        assert_eq!(a.index, b.index, "{way:?}");
                        assert!(a.sample.abs_diff(b.sample) <= 1, "{way:?}: {a:?} {b:?}");
                    }
                }
            }
        }
        first
    }

    /// How far a step's sample is past the sample its grid line falls in, in samples at `bpm`.
    fn off_grid(step: &Step, division: f64, bpm: f64) -> f64 {
        let line = (step.position / division).round() * division;
        (step.position - line) / (bpm / 60.0 / RATE)
    }

    #[test]
    fn steps_start_on_the_sample_their_line_falls_in_from_the_downbeat_at_play() {
        let steps = each_way(|way| {
            let mut live = way.live();
            let mut device = way.device(0.25, 16.0);
            run(&mut device, &mut live, 1000, |_, _, _| {});
            live.change(Change::Play);
            run(&mut device, &mut live, (RATE * 4.0) as usize, |_, _, _| {})
        });
        // Two bars at 120 BPM: 32 sixteenths, the first on the downbeat, indexed 0 to 15 twice, each on its sample.
        assert_eq!(steps.len(), 32, "{steps:?}");
        assert_eq!(steps.iter().map(|step| step.index).collect::<Vec<_>>(), (0..32).map(|n| n % 16).collect::<Vec<_>>());
        assert_eq!(steps[0].position, 0.0, "{:?}", steps[0]);
        for step in &steps {
            assert!((0.0..1.0).contains(&off_grid(step, 0.25, 120.0)), "{step:?}");
        }
    }

    #[test]
    fn a_start_inside_a_step_waits_for_the_next_and_one_on_a_line_takes_its_step_there() {
        for (from, first_index, first_line) in [(0.1, 1, 0.25), (1.0, 4, 1.0), (5.3, 6, 5.5)] {
            let steps = each_way(|way| {
                let mut live = way.live();
                live.position = from;
                let mut device = way.device(0.25, 16.0);
                run(&mut device, &mut live, 500, |_, _, _| {});
                live.change(Change::Play);
                run(&mut device, &mut live, 30_000, |_, _, _| {})
            });
            assert_eq!(steps[0].index, first_index, "from {from}: {steps:?}");
            assert!((steps[0].position - first_line).abs() < 0.0001, "from {from}: {:?}", steps[0]);
            assert!((0.0..1.0).contains(&off_grid(&steps[0], 0.25, 120.0)), "from {from}: {:?}", steps[0]);
        }
    }

    #[test]
    fn play_after_a_stop_starts_with_the_step_where_live_starts_on_its_sample() {
        // Stopped mid-step at 6.37 and on a beat's line at 6.0, then played from the start (Live's insert marker) or
        // on from where it stopped: plugsync~ says where on the vector Live starts, while the phase still holds the
        // stop's.
        for (stop, from, index, line) in
            [(6.37, Some(0.0), 0, 0.0), (6.0, Some(0.0), 0, 0.0), (6.37, None, 10, 6.5), (6.0, Some(2.0), 8, 2.0)]
        {
            let steps = each_way(|way| {
                let mut live = way.live();
                let mut device = way.device(0.25, 16.0);
                live.change(Change::Play);
                let mut stopped = false;
                let mut steps = Vec::new();
                while !stopped {
                    steps.extend(run(&mut device, &mut live, VECTOR, |_, live, _| {
                        if live.playing && live.position >= stop - live.advance() * VECTOR as f64 && live.pending.is_empty() {
                            live.change(Change::Stop);
                            stopped = true;
                        }
                    }));
                }
                let before = steps.len();
                steps.extend(run(&mut device, &mut live, 10_000, |_, _, _| {}));
                assert_eq!(steps.len(), before, "a step while stopped");
                if let Some(from) = from {
                    live.change(Change::Jump(from));
                }
                live.change(Change::Play);
                run(&mut device, &mut live, 12_000, |_, _, _| {})
            });
            assert_eq!(
                steps.iter().map(|step| step.index).take(2).collect::<Vec<_>>(),
                [index, (index + 1) % 16],
                "stop {stop}, from {from:?}: {steps:?}"
            );
            assert!((steps[0].position - line).abs() < 0.0001, "stop {stop}, from {from:?}: {:?}", steps[0]);
            for step in &steps {
                assert!((0.0..1.0).contains(&off_grid(step, 0.25, 120.0)), "stop {stop}, from {from:?}: {step:?}");
            }
        }
    }

    #[test]
    fn stopping_ends_the_steps_and_says_live_stopped() {
        // Live stops mid-step, on a beat's line, and either side of one: no step from there, every line before its
        // step. (A line a sample before the stop is the last.)
        for (stop, last) in [(5.37, 5.25), (5.000_05, 5.0), (4.999_98, 4.75), (5.0, 4.75)] {
            let steps = each_way(|way| {
                let mut live = way.live();
                let vectors = 1000;
                live.position = stop - (vectors * VECTOR) as f64 * live.advance();
                let mut device = way.device(0.25, 16.0);
                live.change(Change::Play);
                let mut steps = run(&mut device, &mut live, vectors * VECTOR, |_, _, _| {});
                assert!((live.position - stop).abs() < 1e-9, "stop {stop}: {}", live.position);
                let stopped_at = live.sample;
                live.change(Change::Stop);
                steps.extend(run(&mut device, &mut live, 20_000, |_, _, _| {}));
                assert!(steps.iter().all(|step| step.sample < stopped_at), "stop {stop}: a step after the stop: {steps:?}");
                let (read, _) = live.tick();
                let transport = device.transport.tick(&read, 6);
                assert_eq!((transport[1], transport[2]), (0.0, 0.0), "stop {stop}: the transport says stopped");
                steps
            });
            let lines: Vec<f64> = steps.iter().map(|step| (step.position / 0.25).round() * 0.25).collect();
            assert_eq!(lines.last().copied(), Some(last), "stop {stop}: {steps:?}");
            assert!(lines.windows(2).all(|pair| (pair[1] - pair[0] - 0.25).abs() < 1e-9), "stop {stop}: {lines:?}");
        }
    }

    #[test]
    fn lives_loop_starts_its_steps_again_on_the_sample_it_goes_back() {
        // Loops of 3 and 3.5 beats from the downbeat, and one from beat 1 to 4 played from 0: sixteen steps of a
        // sixteenth, so each pass starts again at the loop's start step, not where the pattern would have reached.
        for (start, length, from) in [(0.0, 3.0, 0.0), (0.0, 3.5, 0.0), (1.0, 3.0, 0.0)] {
            let steps = each_way(|way| {
                let mut live = way.live();
                live.loop_on = true;
                live.loop_start = start;
                live.loop_length = length;
                live.position = from;
                let mut device = way.device(0.25, 16.0);
                device.told_loop(&live);
                live.change(Change::Play);
                run(&mut device, &mut live, (RATE * 7.0) as usize, |_, _, _| {})
            });
            let end = start + length;
            for (at, step) in steps.iter().enumerate() {
                assert!((0.0..1.0).contains(&off_grid(step, 0.25, 120.0)), "loop {start}+{length}: {step:?}");
                assert_eq!(step.index, ((step.position / 0.25).round() as usize) % 16, "loop {start}+{length}: {step:?}");
                if at > 0 {
                    assert!(step.sample - steps[at - 1].sample > 5000, "loop {start}+{length}: doubled: {step:?}");
                }
            }
            let passes = steps.iter().filter(|step| (step.position - start).abs() < 0.0001).count();
            let expected = ((7.0 * 2.0 - (start - from)) / length).floor() as usize + usize::from(from >= start);
            assert!(passes + 1 >= expected && passes <= expected + 1, "loop {start}+{length}: {passes} passes: {steps:?}");
            let lines = ((length / 0.25).round() as usize) * passes;
            assert!(steps.iter().all(|step| step.position < end), "{steps:?}");
            assert!(steps.len() + 16 >= lines, "loop {start}+{length}: {} steps", steps.len());
        }
    }

    #[test]
    fn a_jump_lands_on_its_step_on_time_and_never_doubles() {
        // Onto a line (9.0, 2.75, 8.0: its step on the vector Live lands) and into a step (9.1: the next line's).
        for (to, index, line) in [(9.0, 4, 9.0), (9.1, 5, 9.25), (2.75, 11, 2.75), (8.0, 0, 8.0)] {
            let steps = each_way(|way| {
                let mut live = way.live();
                let mut device = way.device(0.25, 16.0);
                live.change(Change::Play);
                let mut steps = run(&mut device, &mut live, 30_016, |_, _, _| {});
                live.change(Change::Jump(to));
                steps.extend(run(&mut device, &mut live, 30_000, |_, _, _| {}));
                steps
            });
            let landed = steps.iter().position(|step| step.position >= to - 0.0001 && step.position < to + 0.3).unwrap();
            assert_eq!(steps[landed].index, index, "to {to}: {steps:?}");
            assert!((steps[landed].position - line).abs() < 0.0001, "to {to}: {:?}", steps[landed]);
            for step in &steps {
                assert!((0.0..1.0).contains(&off_grid(step, 0.25, 120.0)), "to {to}: {step:?}");
            }
            for pair in steps.windows(2) {
                assert!(pair[1].sample - pair[0].sample > 1000, "to {to}: {pair:?}");
            }
        }
    }

    #[test]
    fn a_jump_by_whole_beats_is_followed_on_the_vector_it_lands_without_a_second_step() {
        // From about 6.0 beats back by 5: the phase goes on as if nothing happened; plugsync~ says so at once.
        let steps = each_way(|way| {
            let mut live = way.live();
            let mut device = way.device(0.25, 16.0);
            live.change(Change::Play);
            let mut steps = run(&mut device, &mut live, (3.0 * RATE) as usize / VECTOR * VECTOR, |_, _, _| {});
            let back = live.position - 5.0;
            live.change(Change::Jump(back));
            steps.extend(run(&mut device, &mut live, 30_000, |_, _, _| {}));
            steps
        });
        for pair in steps.windows(2) {
            assert!(pair[1].sample - pair[0].sample > 5000, "{pair:?}");
        }
        for step in &steps {
            assert!((0.0..1.0).contains(&off_grid(step, 0.25, 120.0)), "{step:?}");
            assert_eq!(step.index, ((step.position / 0.25).round() as usize) % 16, "{step:?}");
        }
    }

    /// The lines a song reached, as (sample, line): each the first time the song position is at or past it, so a line
    /// the song goes back over isn't due again, and where it moves on past several at once only the one it's in is.
    /// A song that starts inside a step owes that step nothing.
    fn lines_reached(positions: &[(usize, f64)], division: f64) -> Vec<(usize, f64)> {
        let mut lines = Vec::new();
        let mut reached = f64::MIN;
        for (at, &(sample, now)) in positions.iter().enumerate() {
            let line = (now / division + 1e-9).floor() * division;
            if at == 0 && now - line > 1e-9 {
                reached = line;
            }
            if line > reached + 1e-9 {
                lines.push((sample, line));
                reached = line;
            }
        }
        lines
    }

    /// The steps of a song played from `start` whose tempo `tempo` sets before each vector (by its first sample),
    /// checked line by line: every line the song reaches gets one step, its own, on its sample or (where a tempo
    /// change moved the song on past it) on the vector it moved.
    fn every_line_once(label: &str, shift: f64, from: f64, start: f64, samples: usize, tempo: impl Fn(usize) -> Option<f64>) {
        for delayed in [false, true] {
            let mut live = Live::new();
            live.bpm = from;
            live.tempo_shift = shift;
            live.position = start;
            let mut device = Device::new(TRANSPORT, CLOCK, delayed, 0.25, 16.0);
            live.change(Change::Play);
            let (steps, positions) = run_tracing(&mut device, &mut live, samples, |_, live, sample| {
                if let Some(bpm) = tempo(sample) {
                    if bpm != live.bpm {
                        live.change(Change::Tempo(bpm));
                    }
                }
            });
            let lines = lines_reached(&positions, 0.25);
            assert_eq!(steps.len(), lines.len(), "{label}, shift {shift}: {steps:?}\n{lines:?}");
            for (step, (sample, line)) in steps.iter().zip(&lines) {
                assert!(step.position - line >= -1e-9, "{label}, shift {shift}: early: {step:?} for {line}");
                assert!(
                    step.sample >= *sample && step.sample - sample <= 1,
                    "{label}, shift {shift}: {step:?} for line {line} at {sample}"
                );
                assert_eq!(step.index, ((line / 0.25).round() as usize) % 16, "{label}, shift {shift}: {step:?} for line {line}");
            }
        }
    }

    #[test]
    fn a_tempo_change_of_any_size_gives_every_line_its_step() {
        // Typed in mid-step: halved, doubled, and the far ends of Live's range. Live moves the song position a little
        // each way when its tempo changes, by how much it changes, up to its audio buffer's worth (512 samples here,
        // up to 2048 at Live's largest). A thread a change.
        let changes = [
            (120.0_f64, 60.0_f64),
            (120.0, 240.0),
            (140.0, 70.0),
            (120.0, 59.0),
            (100.0, 160.0),
            (80.0, 170.0),
            (20.0, 999.0),
            (999.0, 20.0),
        ];
        std::thread::scope(|scope| {
            for (from, to) in changes {
                scope.spawn(move || {
                    for shift in [0.0, 400.0, -400.0, 2048.0, -2048.0, 4096.0] {
                        let at = 1.13 * 60.0 / from;
                        every_line_once(
                            &format!("{from} to {to}"),
                            shift,
                            from,
                            0.0,
                            (RATE * (at + 4.0 * 60.0 / to.min(from) + 0.5)) as usize,
                            |sample| Some(if sample as f64 / RATE >= at { to } else { from }),
                        );
                    }
                });
            }
        });
    }

    #[test]
    fn a_tempo_ramp_gives_every_line_its_step() {
        // Down from 160 to 100 BPM in steps every tenth of a second, as automation does, and up again.
        std::thread::scope(|scope| {
            for (from, to) in [(160.0, 100.0), (100.0, 160.0)] {
                for shift in [0.0, 400.0, 2048.0, -2048.0] {
                    scope.spawn(move || {
                        every_line_once(&format!("ramp {from} to {to}"), shift, from, 0.0, (RATE * 6.0) as usize, |sample| {
                            let t = sample as f64 / RATE;
                            Some(from + (to - from) * ((t / 0.1).floor() * 0.1 / 4.0).min(1.0))
                        });
                    });
                }
            }
        });
    }

    #[test]
    fn a_tempo_rise_just_after_a_line_starts_that_step_once() {
        // Beat 1's line crossed 1 to 40 samples before the vector a tempo rise lands on: Live's shift moves the song
        // on, past the next line or not, and the step just played doesn't start again. A thread a rise.
        std::thread::scope(|scope| {
            for (from, to) in [(60.0_f64, 240.0_f64), (120.0, 121.0), (20.0, 999.0)] {
                for shift in [400.0, 2048.0] {
                    scope.spawn(move || {
                        let rise = 8 * VECTOR;
                        let advance = from / 60.0 / RATE;
                        for before in 1..=40 {
                            let start = 1.0 + 0.5 * advance - (rise - before) as f64 * advance;
                            let samples = rise + (0.6 * 60.0 / to * RATE) as usize + VECTOR;
                            every_line_once(&format!("{from} to {to}, {before} after the line"), shift, from, start, samples, |sample| {
                                Some(if sample >= rise { to } else { from })
                            });
                        }
                    });
                }
            }
        });
    }

    #[test]
    fn a_line_on_a_vectors_first_sample_takes_its_step_there_once() {
        // plugphasor~ can read 1 and then hold for the vector when a beat's line falls just before one starts.
        for into in [0.4, 0.0, 0.999] {
            let steps = each_way(|way| {
                let mut live = way.live();
                let advance = live.advance();
                live.position = 1.0 - (640.0 - into) * advance;
                let mut device = way.device(0.25, 16.0);
                run(&mut device, &mut live, 1024, |_, _, _| {});
                live.change(Change::Play);
                run(&mut device, &mut live, 30_000, |_, _, _| {})
            });
            let line = steps.iter().find(|step| (step.position - 1.0).abs() < 0.001).expect("a step on beat 1");
            assert_eq!((line.sample, line.index), (1024 + 640, 4), "{into}: {steps:?}");
            assert_eq!(steps.iter().filter(|step| step.index == 4).count(), 1, "{into}: {steps:?}");
        }
    }

    #[test]
    fn a_device_loaded_while_live_plays_takes_up_its_steps_from_the_next_vector() {
        let mut live = Live::new();
        live.late_first_vector = true;
        live.playing = true;
        live.played = true;
        live.position = 3.1;
        let mut device = Device::new(TRANSPORT, CLOCK, false, 0.25, 16.0);
        let steps = run(&mut device, &mut live, 30_000, |_, _, _| {});
        assert_eq!(steps[0].index, 13, "{steps:?}");
        assert!((steps[0].position - 3.25).abs() < 0.0001, "{steps:?}");
        assert!(steps.windows(2).all(|pair| pair[1].index == (pair[0].index + 1) % 16), "{steps:?}");
    }

    #[test]
    fn a_loop_the_device_wasnt_told_of_is_followed_on_the_next_vector() {
        // Live loops at 3.5 beats but the device's loop settings say no loop (they come late on Max's low-priority
        // thread): until plugsync~ says so the steps go on to 3.5, then they're back with Live within a vector, with
        // the loop's first step once.
        let mut live = Live::new();
        live.loop_on = true;
        live.loop_length = 3.5;
        let mut device = Device::new(TRANSPORT, CLOCK, false, 0.25, 16.0);
        live.change(Change::Play);
        let (steps, positions) = run_tracing(&mut device, &mut live, (RATE * 3.0) as usize, |_, _, _| {});
        let wraps: Vec<usize> = positions.windows(2).filter(|pair| pair[1].1 < pair[0].1).map(|pair| pair[1].0).collect();
        assert_eq!(wraps.len(), 1, "{wraps:?}");
        let firsts: Vec<&Step> = steps.iter().filter(|step| step.index == 0).collect();
        assert_eq!(firsts.len(), 2, "the downbeat and the loop's start once: {steps:?}");
        assert!(firsts[1].sample >= wraps[0] && firsts[1].sample - wraps[0] <= VECTOR, "{:?} {wraps:?}", firsts[1]);
        let after: Vec<&Step> = steps.iter().filter(|step| step.sample > firsts[1].sample).collect();
        assert!(after.len() >= 8 && after.iter().all(|step| (0.0..1.0).contains(&off_grid(step, 0.25, 120.0))), "{steps:?}");
        assert!(after.windows(2).all(|pair| pair[1].index == pair[0].index + 1), "{steps:?}");
        // And told of a loop Live doesn't have: the steps go back at its end, then on with Live within a vector.
        let mut live = Live::new();
        let mut device = Device::new(TRANSPORT, CLOCK, false, 0.25, 16.0);
        device.transport.set("loop_on", 1.0);
        device.transport.set("loop_length", 3.5);
        live.change(Change::Play);
        let steps = run(&mut device, &mut live, (RATE * 3.0) as usize, |_, _, _| {});
        let late: Vec<&Step> = steps.iter().filter(|step| step.sample > (3.5 / 2.0 * RATE) as usize + VECTOR).collect();
        assert!(late.len() >= 8, "{steps:?}");
        for step in &late {
            assert_eq!(step.index, ((step.position / 0.25).round() as usize) % 16, "{steps:?}");
            assert!((0.0..1.0).contains(&off_grid(step, 0.25, 120.0)), "{step:?}");
        }
    }

    #[test]
    fn a_division_or_swing_moved_every_vector_never_doubles_crowds_or_starves_the_steps() {
        let divisions = [4.0, 2.0, 1.0, 0.5, 1.0 / 3.0, 0.375, 0.25, 1.0 / 6.0, 0.1875, 0.125];
        let mut seed: u64 = 7;
        let mut random = move || {
            seed ^= seed << 13;
            seed ^= seed >> 7;
            seed ^= seed << 17;
            seed
        };
        let picks: Vec<(f64, f64)> =
            (0..20_000).map(|_| (divisions[(random() % divisions.len() as u64) as usize], (random() % 51) as f64 / 100.0)).collect();
        let mut live = Live::new();
        live.jitter = 5;
        let mut device = Device::new(TRANSPORT, CLOCK, false, 0.25, 16.0);
        live.change(Change::Play);
        let steps = run(&mut device, &mut live, 64 * 20_000, |device, _, sample| {
            let (division, swing) = picks[sample / 64];
            device.clock.set("division", division);
            device.clock.set("swing", swing);
        });
        assert!(steps.len() > 100, "{}", steps.len());
        let per_beat = RATE / 2.0;
        // The shortest gap a grid makes is the second step of a swung pair: (1 − swing) × division.
        let shortest = divisions.iter().fold(f64::MAX, |low, d| low.min(d * 0.5));
        for pair in steps.windows(2) {
            let gap = (pair[1].sample - pair[0].sample) as f64 / per_beat;
            assert!(gap >= shortest - 0.0002, "{pair:?}: {gap} beats");
            assert!(gap <= 4.0 + 0.0002, "starved: {pair:?}: {gap} beats");
        }
        // Every step is on a line of some grid in play: on a division's line, or its swung second step.
        for step in &steps {
            let on = divisions.iter().any(|d| {
                let pair = d * 2.0;
                let into = step.position - (step.position / pair).floor() * pair;
                into < 0.0002 || (into > *d - 0.0002 && into < d * 1.5 + 0.0002)
            });
            assert!(on, "{step:?}");
        }
    }

    #[test]
    fn a_division_change_on_the_sample_a_line_falls_lands_one_step_there() {
        // From sixteenths to eighths at the start of beat 2, where both grids have a line: one step there, then eighths.
        let mut live = Live::new();
        let mut device = Device::new(TRANSPORT, CLOCK, false, 0.25, 16.0);
        live.change(Change::Play);
        let change = (1.0 / 2.0 * RATE) as usize / 64 * 64;
        let steps = run(&mut device, &mut live, (2.0 * RATE) as usize, |device, _, sample| {
            if sample == change {
                device.clock.set("division", 0.5);
            }
        });
        let after: Vec<f64> = steps.iter().filter(|step| step.sample >= change).map(|step| (step.position * 2.0).round() / 2.0).collect();
        assert_eq!(after.first().copied(), Some(1.0), "{steps:?}");
        assert!(after.windows(2).all(|pair| (pair[1] - pair[0] - 0.5).abs() < 1e-9), "{after:?}");
    }

    #[test]
    fn a_shorter_length_starts_the_pattern_again_at_its_next_step() {
        let mut live = Live::new();
        let mut device = Device::new(TRANSPORT, CLOCK, false, 0.25, 16.0);
        live.change(Change::Play);
        let steps = run(&mut device, &mut live, (RATE * 4.0) as usize, |device, _, sample| {
            if sample == (2.6 / 2.0 * RATE) as usize / 64 * 64 {
                device.clock.set("length", 8.0);
            }
        });
        let indices: Vec<usize> = steps.iter().map(|step| step.index).collect();
        assert_eq!(&indices[..11], &[0, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10]);
        assert_eq!(indices[11], 0, "{indices:?}");
        assert!(indices[11..].windows(2).all(|pair| pair[1] == (pair[0] + 1) % 8), "{indices:?}");
    }

    #[test]
    fn a_swing_raised_after_the_off_beat_doesnt_play_it_again() {
        let mut live = Live::new();
        let mut device = Device::new(TRANSPORT, CLOCK, false, 0.25, 16.0);
        live.change(Change::Play);
        let raise = (0.26 / 2.0 * RATE) as usize / VECTOR * VECTOR;
        let steps = run(&mut device, &mut live, (RATE * 1.0) as usize, |device, _, sample| {
            if sample == raise {
                device.clock.set("swing", 0.5);
            }
        });
        let first_pair: Vec<f64> = steps.iter().filter(|step| step.position < 0.49).map(|step| step.position).collect();
        assert_eq!(first_pair.len(), 2, "the downbeat and the off-beat, once each: {steps:?}");
        assert!(steps.windows(2).all(|pair| pair[1].index == pair[0].index + 1), "{steps:?}");
        // The next pairs swing: their second steps three quarters of the way in.
        let later: Vec<f64> = steps.iter().filter(|step| step.position > 0.49).map(|step| (step.position * 8.0).round() / 8.0).collect();
        assert_eq!(&later[..4], &[0.5, 0.875, 1.0, 1.375], "{steps:?}");
    }

    #[test]
    fn a_division_change_inside_a_pair_waits_for_the_new_grids_line() {
        // From quarter notes to eighth-note triplets at 0.4 beats: nothing at once, the next step at 2/3.
        let mut live = Live::new();
        let mut device = Device::new(TRANSPORT, CLOCK, false, 1.0, 16.0);
        live.change(Change::Play);
        let change = (0.4 / 2.0 * RATE) as usize / VECTOR * VECTOR;
        let steps = run(&mut device, &mut live, (RATE * 0.6) as usize, |device, _, sample| {
            if sample == change {
                device.clock.set("division", 1.0 / 3.0);
            }
        });
        let positions: Vec<f64> = steps.iter().map(|step| step.position).collect();
        assert_eq!(positions.len(), 3, "the downbeat, 2/3 and 1: {positions:?}");
        assert!(positions[0] < 0.001 && (positions[1] - 2.0 / 3.0).abs() < 0.0002 && (positions[2] - 1.0).abs() < 0.0002, "{positions:?}");
    }

    /// Live's own transport, recorded in Live 12 at every sample through eight scenes (tests/support's
    /// live-transport.json, made by the device_timing_live example): each scene's name, Live's loop if it had one, and
    /// what a device reads at each sample (plugphasor~, and plugsync~'s position, play flag and tempo).
    fn recorded() -> Vec<(String, Option<(f64, f64)>, Vec<[f64; 4]>)> {
        let file: serde_json::Value = serde_json::from_str(include_str!("../../../../../tests/support/live-transport.json")).unwrap();
        let rate = file["rate"].as_f64().unwrap();
        assert_eq!((rate, file["vector"].as_u64()), (RATE, Some(VECTOR as u64)));
        let number = |value: &serde_json::Value| value.as_f64().unwrap();
        let mut scenes = Vec::new();
        for scene in file["scenes"].as_array().unwrap() {
            let samples = scene["samples"].as_u64().unwrap() as usize;
            let mut read = vec![[0.0; 4]; samples];
            let runs = scene["phase"].as_array().unwrap();
            for (at, run) in runs.iter().enumerate() {
                let from = run[0].as_u64().unwrap() as usize;
                let to = runs.get(at + 1).map_or(samples, |next| next[0].as_u64().unwrap() as usize);
                let (value, step) = (number(&run[1]), number(&run[2]));
                for (k, sample) in read[from..to].iter_mut().enumerate() {
                    let x = value + k as f64 * step;
                    sample[0] = if step == 0.0 || k == 0 { value } else { x - x.floor() };
                }
            }
            let mut said = scene["sync"].as_array().unwrap().iter().peekable();
            let (mut heard, mut flag, mut bpm) = (0.0, 0.0, 0.0);
            for vector in 0..samples.div_ceil(VECTOR) {
                if flag > 0.5 {
                    heard += VECTOR as f64 * bpm / 60.0 / rate;
                }
                if let Some(news) = said.next_if(|news| news[0].as_u64() == Some(vector as u64)) {
                    (heard, flag, bpm) = (number(&news[1]), number(&news[2]), number(&news[3]));
                }
                for sample in read.iter_mut().skip(vector * VECTOR).take(VECTOR) {
                    (sample[1], sample[2], sample[3]) = (heard, flag, bpm);
                }
            }
            let looped = scene["loop"].as_array().map(|pair| (number(&pair[0]), number(&pair[1])));
            scenes.push((scene["name"].as_str().unwrap().to_string(), looped, read));
        }
        scenes
    }

    /// Where Live's song truly was at each sample (None while it's stopped), with hindsight: plugsync~'s word on each
    /// vector, on at its tempo, put to a fraction of a sample by the phase where that moves on steadily, and back at
    /// Live's loop's end (each recorded loop plays from inside it).
    fn truth(read: &[[f64; 4]], looped: Option<(f64, f64)>) -> Vec<Option<f64>> {
        let back = |x: f64| match looped {
            Some((start, length)) if x >= start + length - 1e-9 => (x - length).max(start),
            _ => x,
        };
        let (mut said, mut advance) = (0.0, 0.0);
        let mut truth = Vec::with_capacity(read.len());
        for (i, &[phase, heard, flag, bpm]) in read.iter().enumerate() {
            if flag < 0.5 {
                truth.push(None);
                continue;
            }
            if i % VECTOR == 0 {
                advance = bpm / 60.0 / RATE;
                said = heard + VECTOR as f64 * advance;
            }
            let estimate = back(said + (i % VECTOR) as f64 * advance);
            let steady = i > 0 && read[i - 1][2] > 0.5 && {
                let step = phase - read[i - 1][0];
                (step - step.round() - advance).abs() < 1e-7
            };
            let fine = phase - phase.floor();
            let whole = (estimate - fine).round();
            truth.push(Some(if steady && (whole + fine - estimate).abs() < 2.0 * advance { back(whole + fine) } else { estimate }));
        }
        truth
    }

    /// One recorded scene through the transport and clock at each of two grids: every line the song reached has one
    /// step, its own (the steps follow the song), on its sample or the next (a line on a sample's edge), and no step
    /// falls on no line.
    fn replay(name: &str, looped: Option<(f64, f64)>, read: &[[f64; 4]]) {
        let truth = truth(read, looped);
        for division in [0.25, 1.0 / 6.0] {
            let mut device = Device::new(TRANSPORT, CLOCK, false, division, 16.0);
            if let Some((start, length)) = looped {
                device.transport.set("loop_on", 1.0);
                device.transport.set("loop_start", start);
                device.transport.set("loop_length", length);
            }
            let mut steps: Vec<(usize, f64)> = Vec::new();
            for (i, sample) in read.iter().enumerate() {
                let transport = device.transport.tick(sample, 6);
                let clock = device.clock.tick(&transport, 4);
                if clock[0] > 0.5 {
                    steps.push((i, clock[1]));
                }
            }
            // Each line the song reached, the first time it did; where the song jumped, from where it landed.
            let mut lines = Vec::new();
            let mut reached = f64::MIN;
            for i in 1..read.len() {
                let Some(now) = truth[i] else {
                    reached = f64::MIN;
                    continue;
                };
                let line = (now / division + 1e-9).floor() * division;
                match truth[i - 1] {
                    Some(was) if (now - was).abs() < 0.3 => {
                        if line > was.min(now) + 1e-12 && line <= now + 1e-12 && line > reached + 1e-9 {
                            lines.push((i, line));
                            reached = line;
                        }
                    }
                    _ => {
                        reached = f64::MIN;
                        if now - line < 2.0 * read[i][3] / 60.0 / RATE {
                            lines.push((i, line));
                            reached = line;
                        }
                    }
                }
            }
            let mut missing = Vec::new();
            for &(sample, line) in &lines {
                let index = ((line / division).round() as i64).rem_euclid(16) as f64;
                match steps.iter().position(|&(step, played)| step + 1 >= sample && step <= sample + 1 && played == index) {
                    Some(at) => {
                        steps.remove(at);
                    }
                    None => missing.push((sample, line)),
                }
            }
            assert!(lines.len() > 40, "{name}: {} lines", lines.len());
            assert!(
                missing.is_empty() && steps.is_empty(),
                "{name}, every {division} beats: no step for the lines at samples {missing:?}; steps on no line at {steps:?}"
            );
        }
    }

    #[test]
    fn on_lives_own_recordings_every_line_the_song_reaches_gets_one_step_on_its_sample() {
        // Two restarts, loops of 3.5 and 3 beats, jumps, tempos from 20 to 999 BPM typed in, stops and plays on
        // mid-beat, and a ramp, each as Live played it: a thread a scene.
        let scenes = recorded();
        assert_eq!(scenes.len(), 8);
        std::thread::scope(|scope| {
            for (name, looped, read) in &scenes {
                scope.spawn(move || replay(name, *looped, read));
            }
        });
    }

    #[test]
    fn no_name_the_building_blocks_use_is_one_gen_keeps() {
        let names = |code: &str| crate::devices::gen::names(code).declared.into_keys().chain(crate::devices::gen::names(code).read);
        for code in [TRANSPORT, CLOCK] {
            for name in names(code) {
                let allowed = name.starts_with("in")
                    || name.starts_with("out")
                    || ["samplerate", "vectorsize", "if", "else"].contains(&name.as_str());
                assert!(
                    allowed || crate::devices::gen::param_problem(&name).is_none() || name.starts_with("kumi_"),
                    "{name} is one of gen's own names"
                );
            }
        }
    }
}
