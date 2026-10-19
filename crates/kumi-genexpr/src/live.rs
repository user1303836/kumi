//! Live's transport as a Max for Live device sees it, one sample at a time, the way Live 12 was recorded doing it, and Kumi's
//! transport and clock run in it: the test bench of Kumi's timing building blocks (kumi-runtime's kit::clock tests say
//! what each rule is). Here so that it runs optimized in those tests, as the interpreter does.

use crate::Gen;

pub const RATE: f64 = 44_100.0;
pub const VECTOR: usize = 64;

/// A change Live makes on the next vector's first sample.
#[derive(Debug, Clone, Copy)]
pub enum Change {
    Play,
    Stop,
    Jump(f64),
    Tempo(f64),
}

/// Live as a Max for Live device sees it, one sample at a time, the way Live 12 was recorded doing it (the module's
/// notes say what). The song position moves at the tempo while Live plays and goes back at its loop's end on the
/// sample it reaches it.
pub struct Live {
    pub position: f64,
    pub bpm: f64,
    pub playing: bool,
    pub loop_on: bool,
    pub loop_start: f64,
    pub loop_length: f64,
    /// How far a tempo change moves the song position: this many samples of the change in a sample's advance.
    pub tempo_shift: f64,
    /// plugphasor~ reads 0 while Live is stopped, as its reference says, instead of holding.
    pub zero_when_stopped: bool,
    /// plugsync~'s position is a sample further on every this many vectors (0: never).
    pub jitter: usize,
    /// The sig~s after plugsync~ read 0 for the first vector, as when the device loads while Live plays.
    pub late_first_vector: bool,
    pub sample: usize,
    pub pending: Vec<Change>,
    pub heard: f64,
    pub flag: f64,
    pub said_bpm: f64,
    pub phase: f64,
    pub line: f64,
    pub held: Option<f64>,
    pub held_vector: bool,
    pub stepped: f64,
    pub played: bool,
}

impl Live {
    pub fn new() -> Live {
        Live {
            position: 0.0,
            bpm: 120.0,
            playing: false,
            loop_on: false,
            loop_start: 0.0,
            loop_length: 4.0,
            tempo_shift: 400.0,
            zero_when_stopped: false,
            jitter: 0,
            late_first_vector: false,
            sample: 0,
            pending: Vec::new(),
            heard: 0.0,
            flag: 0.0,
            said_bpm: 0.0,
            phase: 0.0,
            line: 0.0,
            held: None,
            held_vector: false,
            stepped: 0.0,
            played: false,
        }
    }

    pub fn advance(&self) -> f64 {
        self.bpm / 60.0 / RATE
    }

    pub fn change(&mut self, change: Change) {
        self.pending.push(change);
    }

    /// One sample: what plugphasor~ and plugsync~ (position, play flag, tempo) say, and where the song truly is;
    /// then the song moves on.
    pub fn tick(&mut self) -> ([f64; 4], f64) {
        let first = self.sample % VECTOR == 0;
        let was_playing = self.playing;
        if first {
            for change in std::mem::take(&mut self.pending) {
                match change {
                    Change::Play => self.playing = true,
                    Change::Stop => self.playing = false,
                    Change::Jump(to) => self.position = to,
                    Change::Tempo(bpm) => {
                        let before = self.advance();
                        self.bpm = bpm;
                        if self.playing {
                            self.position += self.tempo_shift * (self.advance() - before);
                        }
                    }
                }
            }
            let silent = self.late_first_vector && self.sample == 0;
            self.flag = if self.playing && !silent { 1.0 } else { 0.0 };
            self.said_bpm = if silent { 0.0 } else { self.bpm };
            self.heard = if silent {
                0.0
            } else if self.playing {
                let mut samples = (self.position / self.advance()).round() - VECTOR as f64;
                if self.jitter > 0 && (self.sample / VECTOR) % self.jitter == self.jitter - 1 {
                    samples += 1.0;
                }
                samples * self.advance()
            } else {
                self.position
            };
        }
        let fraction = |x: f64| x - x.floor();
        // plugphasor~: a line from where Live says the song is on the vector's first sample. Where that's behind
        // the phase it would have gone on to (unwrapped: a beat's line just before the vector counts), it holds
        // until the line passes it or the vector ends; the vector after a hold takes Live's phase as it is.
        let out = if !self.playing {
            if was_playing && first {
                self.phase = fraction(self.position);
            }
            if self.zero_when_stopped || !self.played {
                0.0
            } else {
                self.phase
            }
        } else if first {
            self.line = self.position;
            let own = self.phase + if was_playing { self.stepped } else { 0.0 };
            let live = fraction(self.position);
            if live >= own - 1e-9 || !self.played || self.held_vector {
                self.held = None;
                self.held_vector = false;
                live
            } else {
                self.held = Some(own);
                self.held_vector = true;
                own
            }
        } else {
            let live = fraction(self.line + (self.sample % VECTOR) as f64 * self.advance());
            match self.held {
                Some(held) if live < held => held,
                _ => {
                    self.held = None;
                    live
                }
            }
        };
        self.phase = out;
        self.played |= self.playing;
        let truth = self.position;
        self.stepped = 0.0;
        if self.playing {
            let end = self.loop_start + self.loop_length;
            let next = self.position + self.advance();
            let inside = self.loop_on && self.position >= self.loop_start && self.position < end;
            self.position = if inside && next >= end { next - self.loop_length } else { next };
            self.stepped = self.advance();
        }
        self.sample += 1;
        ([out, self.heard, self.flag, self.said_bpm], truth)
    }
}

/// Kumi's transport and clock, wired as [kumi.clock] wires them.
pub struct Device {
    pub transport: Gen,
    pub clock: Gen,
}

impl Device {
    pub fn new(transport: &str, clock: &str, delayed: bool, division: f64, length: f64) -> Device {
        let mut device = Device { transport: Gen::new(transport, delayed), clock: Gen::new(clock, delayed) };
        device.clock.set("division", division);
        device.clock.set("length", length);
        device
    }

    /// The device told Live's loop settings, as its live.observers tell it.
    pub fn told_loop(&mut self, live: &Live) {
        self.transport.set("loop_on", if live.loop_on { 1.0 } else { 0.0 });
        self.transport.set("loop_start", live.loop_start);
        self.transport.set("loop_length", live.loop_length);
    }

    /// One sample: whether a step starts, its index and its length in ms (read from the clock's buffer, as the
    /// patch's peek~ reads them once the vector is done), and where the true song position was.
    pub fn tick(&mut self, live: &mut Live) -> (bool, f64, f64, f64) {
        let (read, truth) = live.tick();
        let transport = self.transport.tick(&read, 6);
        let clock = self.clock.tick(&transport, 4);
        let memory = &self.clock.buffers[0].1;
        if clock[0] > 0.5 {
            assert_eq!((memory[0], memory[1]), (clock[1], clock[2]), "the buffer holds the step that starts");
        }
        (clock[0] > 0.5, memory[0], memory[1], truth)
    }
}

/// A step that started: the sample, its index, and the true song position then.
#[derive(Debug, Clone, PartialEq)]
pub struct Step {
    pub sample: usize,
    pub index: usize,
    pub position: f64,
}

/// `samples` of the song through Kumi's clock, `between` running before each vector (Live's settings moving),
/// with every sample's true song position.
pub fn run_tracing(
    device: &mut Device,
    live: &mut Live,
    samples: usize,
    mut between: impl FnMut(&mut Device, &mut Live, usize),
) -> (Vec<Step>, Vec<(usize, f64)>) {
    let mut steps = Vec::new();
    let mut positions = Vec::new();
    for _ in 0..samples {
        if live.sample % VECTOR == 0 {
            between(device, live, live.sample);
        }
        let sample = live.sample;
        let (fire, index, _, position) = device.tick(live);
        positions.push((sample, position));
        if fire {
            steps.push(Step { sample, index: index as usize, position });
        }
    }
    (steps, positions)
}

pub fn run(device: &mut Device, live: &mut Live, samples: usize, between: impl FnMut(&mut Device, &mut Live, usize)) -> Vec<Step> {
    run_tracing(device, live, samples, between).0
}
