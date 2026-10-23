//! Hearing what REAPER plays (`capture_start`): the device's output, taken from REAPER's hardware hook while REAPER plays.
//! The producer hears it because REAPER is playing it.
//!
//! The hook is registered for the capture only. On the audio thread it copies the output buffers (the master's hardware
//! output pair) into a lock-free single-producer ring: no locks, no allocation, no REAPER call but the hook's own
//! `GetBuffer`. The main thread drains the ring into a 32-bit float WAV in Kumi's folder each tick, and notes where the
//! project was at each jump (a loop, a seek, a stop) by pairing the frames the hook has had with the position of the next
//! block REAPER processes.

use std::ffi::{c_int, c_void};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicI32, AtomicU32, AtomicU64, AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use serde_json::{json, Value};

use super::wav;
use crate::read::Reads;
use crate::reaper::{Project, Reaper};

/// The longest capture.
pub const MAX_SECONDS: f64 = 60.0;
/// The ring holds this many seconds at the device's rate: what the main thread can fall behind by (a dialog, a long job)
/// before the capture ends there.
const RING_SECONDS: f64 = 12.0;
/// No audio for this long after the start: REAPER's device isn't sending any.
const SILENT_START: Duration = Duration::from_secs(3);
/// A hook taken out is freed this long after, so a block REAPER was in the middle of never touches freed memory.
pub const RETIRE_AFTER: Duration = Duration::from_secs(2);
/// A position this far from where playing on would have taken it is a jump.
const JUMP: f64 = 0.02;
/// Options: Toggle metronome.
const METRONOME: i32 = 40364;
/// The most marks one capture keeps.
const MAX_MARKS: usize = 512;

/// `audio_hook_register_t`, as reaper_plugin.h declares it.
#[repr(C)]
pub struct Hook {
    on_audio_buffer: extern "C" fn(is_post: bool, len: c_int, srate: f64, reg: *mut Hook),
    userdata1: *mut c_void,
    userdata2: *mut c_void,
    /// Set by REAPER: only to be read inside `on_audio_buffer`.
    input_nch: c_int,
    output_nch: c_int,
    get_buffer: Option<unsafe extern "C" fn(is_output: bool, idx: c_int) -> *mut f64>,
}

/// The samples the audio thread hands the main thread: stereo, interleaved, as f32 bits.
pub struct Ring {
    samples: Box<[AtomicU32]>,
    mask: usize,
    /// Samples written and read, counting up for ever (they wrap only past 2^64).
    head: AtomicUsize,
    tail: AtomicUsize,
    /// Frames the hook was given while open (written or dropped): where the stream is.
    frames: AtomicU64,
    /// Frames dropped because the ring was full, and the frame the first was (where the take must end: the audio after it
    /// has a gap).
    dropped: AtomicU64,
    gap: AtomicU64,
    /// The device's rate, as the first block gave it (f64 bits; 0 before any block), and whether a later block differed.
    rate: AtomicU64,
    rate_changed: AtomicBool,
    /// The output channels taken as left and right.
    left: AtomicI32,
    right: AtomicI32,
    /// The hook writes only while open.
    open: AtomicBool,
    /// What the hook saw, for the log: the device's output channels, blocks whose buffer REAPER gave none of, the peak.
    outputs: AtomicI32,
    missing: AtomicU64,
    peak: AtomicU32,
}

impl Ring {
    /// A ring of at least `samples` (rounded up to a power of two), taking these output channels.
    pub fn new(samples: usize, left: i32, right: i32) -> Ring {
        let capacity = samples.max(1024).next_power_of_two();
        Ring {
            samples: (0..capacity).map(|_| AtomicU32::new(0)).collect(),
            mask: capacity - 1,
            head: AtomicUsize::new(0),
            tail: AtomicUsize::new(0),
            frames: AtomicU64::new(0),
            dropped: AtomicU64::new(0),
            gap: AtomicU64::new(u64::MAX),
            rate: AtomicU64::new(0),
            rate_changed: AtomicBool::new(false),
            left: AtomicI32::new(left),
            right: AtomicI32::new(right),
            open: AtomicBool::new(true),
            outputs: AtomicI32::new(-1),
            missing: AtomicU64::new(0),
            peak: AtomicU32::new(0),
        }
    }

    pub fn capacity(&self) -> usize {
        self.mask + 1
    }

    /// The audio thread's side: a block's left and right samples (either may be missing: silence). Never blocks, never
    /// allocates; what doesn't fit is dropped and counted.
    pub fn push(&self, left: Option<&[f64]>, right: Option<&[f64]>, len: usize, rate: f64) {
        if !self.open.load(Ordering::Acquire) {
            return;
        }
        let bits = rate.to_bits();
        match self.rate.load(Ordering::Relaxed) {
            0 => self.rate.store(bits, Ordering::Relaxed),
            before if before != bits => self.rate_changed.store(true, Ordering::Relaxed),
            _ => {}
        }
        if left.is_none() || right.is_none() {
            self.missing.fetch_add(1, Ordering::Relaxed);
        }
        let head = self.head.load(Ordering::Relaxed);
        let tail = self.tail.load(Ordering::Acquire);
        let free = self.capacity().saturating_sub(head.wrapping_sub(tail));
        let fits = (free / 2).min(len);
        let mut peak = f32::from_bits(self.peak.load(Ordering::Relaxed));
        for i in 0..fits {
            let l = left.and_then(|l| l.get(i)).copied().unwrap_or(0.0) as f32;
            let r = right.and_then(|r| r.get(i)).copied().unwrap_or(0.0) as f32;
            peak = peak.max(l.abs()).max(r.abs());
            self.samples[head.wrapping_add(2 * i) & self.mask].store(l.to_bits(), Ordering::Relaxed);
            self.samples[head.wrapping_add(2 * i + 1) & self.mask].store(r.to_bits(), Ordering::Relaxed);
        }
        self.peak.store(peak.to_bits(), Ordering::Relaxed);
        self.head.store(head.wrapping_add(2 * fits), Ordering::Release);
        let at = self.frames.fetch_add(len as u64, Ordering::Release);
        if fits < len {
            self.dropped.fetch_add((len - fits) as u64, Ordering::Relaxed);
            let _ = self.gap.compare_exchange(u64::MAX, at + fits as u64, Ordering::Relaxed, Ordering::Relaxed);
        }
    }

    /// The main thread's side: up to `max` frames, appended to `out`.
    pub fn drain(&self, out: &mut Vec<f32>, max: usize) -> usize {
        let tail = self.tail.load(Ordering::Relaxed);
        let head = self.head.load(Ordering::Acquire);
        let samples = head.wrapping_sub(tail).min(max * 2) & !1;
        out.reserve(samples);
        for i in 0..samples {
            out.push(f32::from_bits(self.samples[tail.wrapping_add(i) & self.mask].load(Ordering::Relaxed)));
        }
        self.tail.store(tail.wrapping_add(samples), Ordering::Release);
        samples / 2
    }

    pub fn frames(&self) -> u64 {
        self.frames.load(Ordering::Acquire)
    }

    pub fn dropped(&self) -> u64 {
        self.dropped.load(Ordering::Relaxed)
    }

    /// The frame the first dropped one was, if any was.
    pub fn gap(&self) -> Option<u64> {
        let gap = self.gap.load(Ordering::Relaxed);
        (gap != u64::MAX).then_some(gap)
    }

    /// The device's rate, once a block has come.
    pub fn rate(&self) -> Option<f64> {
        let bits = self.rate.load(Ordering::Relaxed);
        (bits != 0).then(|| f64::from_bits(bits)).filter(|rate| *rate > 0.0)
    }

    pub fn close(&self) {
        self.open.store(false, Ordering::Release);
    }
}

/// REAPER's audio thread calls this twice a block: before its processing and after (`is_post`), when the output buffers
/// hold what goes to the device.
extern "C" fn on_audio_buffer(is_post: bool, len: c_int, srate: f64, reg: *mut Hook) {
    if !is_post || len <= 0 || reg.is_null() {
        return;
    }
    // SAFETY: REAPER passes back the Hook Kumi registered, which stays alive until after it is unregistered (and a while
    // longer, see RETIRE_AFTER); its userdata1 is the Ring it was made with, kept alive as long.
    let reg = unsafe { &*reg };
    let ring = unsafe { &*(reg.userdata1 as *const Ring) };
    let Some(get) = reg.get_buffer else { return };
    ring.outputs.store(reg.output_nch, Ordering::Relaxed);
    let len = len as usize;
    let channel = |which: &AtomicI32| -> Option<&[f64]> {
        let index = which.load(Ordering::Relaxed);
        if index < 0 || index >= reg.output_nch {
            return None;
        }
        // SAFETY: GetBuffer may be called inside this callback; it gives `len` samples of that output channel.
        let buffer = unsafe { get(true, index) };
        (!buffer.is_null()).then(|| unsafe { std::slice::from_raw_parts(buffer, len) })
    };
    ring.push(channel(&ring.left), channel(&ring.right), len, srate);
}

/// A hook and its ring, owned together: the hook points at the ring.
pub struct Registered {
    hook: Box<Hook>,
    ring: Arc<Ring>,
}

impl Registered {
    pub fn new(ring: Arc<Ring>) -> Registered {
        let hook = Box::new(Hook {
            on_audio_buffer,
            userdata1: Arc::as_ptr(&ring) as *mut c_void,
            userdata2: std::ptr::null_mut(),
            input_nch: 0,
            output_nch: 0,
            get_buffer: None,
        });
        Registered { hook, ring }
    }

    pub fn as_ptr(&mut self) -> *mut c_void {
        (&mut *self.hook as *mut Hook).cast()
    }

    pub fn ring(&self) -> &Arc<Ring> {
        &self.ring
    }

    /// REAPER's audio thread, for a test: a block of `len` frames whose output channel n holds `value(n, i)`.
    #[cfg(test)]
    pub fn feed(&mut self, len: usize, rate: f64, channels: usize, value: impl Fn(usize, usize) -> f64) {
        thread_local! {
            static BUFFERS: std::cell::RefCell<Vec<Vec<f64>>> = const { std::cell::RefCell::new(Vec::new()) };
        }
        unsafe extern "C" fn get(is_output: bool, idx: c_int) -> *mut f64 {
            if !is_output {
                return std::ptr::null_mut();
            }
            BUFFERS.with(|b| b.borrow_mut().get_mut(idx as usize).map_or(std::ptr::null_mut(), |b| b.as_mut_ptr()))
        }
        BUFFERS.with(|b| *b.borrow_mut() = (0..channels).map(|n| (0..len).map(|i| value(n, i)).collect()).collect());
        self.hook.output_nch = channels as c_int;
        self.hook.get_buffer = Some(get);
        let reg = &mut *self.hook as *mut Hook;
        on_audio_buffer(false, len as c_int, rate, reg);
        on_audio_buffer(true, len as c_int, rate, reg);
    }
}

/// How a capture ended, in the status's words.
pub struct Ended {
    pub state: super::board::State,
    pub result: serde_json::Map<String, Value>,
}

/// A capture running: the hook registered, the file being written.
pub struct Capture {
    pub job: String,
    proj: Project,
    epoch: String,
    pub registered: Registered,
    writer: Option<wav::Writer>,
    file: PathBuf,
    target: f64,
    marks: Vec<Value>,
    /// The last pairing of a frame with the project position it plays: (frame, seconds, playing).
    last: Option<(u64, f64, bool)>,
    started: Instant,
    notes: Vec<String>,
    buffer: Vec<f32>,
}

/// Why REAPER can't capture now, in its words; None when it can.
pub fn refusal(reaper: &Reaper, proj: Project) -> Option<String> {
    let state = reaper.play_state(proj);
    if !(state.playing || state.recording) || state.paused {
        return Some("REAPER isn't playing: give from_beat and beats, and Kumi renders the part".into());
    }
    if !reaper.audio_running() {
        return Some(
            "REAPER's audio isn't running, so Kumi can't hear what it plays: give from_beat and beats, and Kumi renders the part".into(),
        );
    }
    if !reaper.audio_heard() {
        return Some(NO_OUTPUT.into());
    }
    None
}

/// Why a device without outputs can't be heard.
const NO_OUTPUT: &str =
    "REAPER's audio device has no output Kumi can hear (REAPER is on its dummy device): give from_beat and beats, and Kumi renders the part";
/// Blocks after which a hook that has had no output channel gives up.
const NO_OUTPUT_BLOCKS: u64 = 16;

impl Capture {
    /// Registers the hook for a capture of `seconds` into `file`; the reason when REAPER won't.
    pub fn begin(reaper: &Reaper, reads: &Reads, job: &str, proj: Project, seconds: f64, file: PathBuf) -> Result<Capture, String> {
        if let Some(why) = refusal(reaper, proj) {
            return Err(why);
        }
        let (first, mono) = reaper.master_output(proj);
        let (left, right) = if mono { (first, first) } else { (first, first + 1) };
        let rate = reaper.audio_device("SRATE").and_then(|r| r.trim().parse::<f64>().ok()).filter(|r| *r >= 8000.0).unwrap_or(48000.0);
        let ring = Arc::new(Ring::new((RING_SECONDS * rate * 2.0) as usize, left, right));
        let mut registered = Registered::new(ring);
        if !reaper.register_audio_hook(true, registered.as_ptr()) {
            return Err("REAPER wouldn't let Kumi hear its output (its hardware hook refused)".into());
        }
        let mut notes = vec![];
        if reaper.action_on(METRONOME) {
            notes.push("REAPER's metronome is on: it is in the take".to_string());
        }
        Ok(Capture {
            job: job.into(),
            proj,
            epoch: reads.epoch(),
            registered,
            writer: None,
            file,
            target: seconds,
            marks: vec![],
            last: None,
            started: Instant::now(),
            notes,
            buffer: Vec::new(),
        })
    }

    pub fn answer(&self) -> Value {
        json!({
            "jobId": self.job,
            "state": "running",
            "seconds": self.target,
            "files": [{"id": "master", "file": self.file.to_string_lossy()}],
        })
    }

    /// The frame the hook is at now, paired with the position of the next block REAPER processes (which that frame will
    /// be): read again when a block slipped in between.
    fn pairing(&self, reaper: &Reaper) -> (u64, f64) {
        let ring = self.registered.ring();
        let mut frames = ring.frames();
        let mut position = reaper.next_block_position(self.proj);
        for _ in 0..3 {
            let after = ring.frames();
            if after == frames {
                break;
            }
            frames = after;
            position = reaper.next_block_position(self.proj);
        }
        (frames, position)
    }

    fn mark(&mut self, reaper: &Reaper, frame: u64, seconds: f64, playing: bool) {
        if self.marks.len() >= MAX_MARKS {
            return;
        }
        let beats = reaper.quarter_notes_at(self.proj, seconds).map(crate::rows::seconds);
        self.marks.push(json!([frame, crate::rows::seconds(seconds), beats, playing]));
    }

    /// Notes a jump since the last tick: a loop's wrap (placed exactly, at the loop's end), a seek, a stop or a start.
    fn follow(&mut self, reaper: &Reaper, rate: f64) {
        let (frame, position) = self.pairing(reaper);
        let state = reaper.play_state(self.proj);
        let playing = (state.playing || state.recording) && !state.paused;
        let speed = reaper.play_rate(self.proj);
        match self.last {
            None => {
                // The stream's first frame, where playing at this speed from it reached this position.
                let at = (position - frame as f64 / rate * speed).max(0.0);
                self.mark(reaper, 0, at, playing);
            }
            Some((last_frame, last_position, last_playing)) => {
                let expected = last_position + (frame.saturating_sub(last_frame)) as f64 / rate * speed;
                let moved = (position - expected).abs() > JUMP.max(frame.saturating_sub(last_frame) as f64 / rate * 0.01);
                if playing != last_playing || (playing && moved) {
                    let (loop_start, loop_end) = reaper.loop_points(self.proj);
                    let wrapped = playing && last_playing && reaper.repeat(self.proj) && loop_end > loop_start && position < last_position;
                    if wrapped && position >= loop_start {
                        let back = ((position - loop_start) / speed.max(1e-6) * rate).round() as u64;
                        let at = frame.saturating_sub(back).max(last_frame);
                        self.mark(reaper, at, loop_start, true);
                    } else {
                        self.mark(reaper, frame, position, playing);
                    }
                }
            }
        }
        self.last = Some((frame, position, playing));
    }

    /// A tick's work: drains the ring into the file and follows the position; the end when it is over.
    pub fn step(&mut self, reaper: &Reaper, reads: &Reads, cancelled: bool) -> Option<Ended> {
        let ring = self.registered.ring().clone();
        let still = reaper.current_project().is_some_and(|info| info.ptr == self.proj)
            && (reads.epoch() == self.epoch || reads.epoch_is_of(&self.epoch, self.proj));
        if !still {
            return Some(self.fail(reaper, "stale", "the project in the tab changed while Kumi listened: read again"));
        }
        // A device whose blocks come with no output channel can't be heard (REAPER's dummy device: measured).
        if ring.outputs.load(Ordering::Relaxed) == 0
            && ring.missing.load(Ordering::Relaxed) >= NO_OUTPUT_BLOCKS
            && !crate::reaper::lab_dummy()
        {
            return Some(self.fail(reaper, "failed", NO_OUTPUT));
        }
        let Some(rate) = ring.rate() else {
            if cancelled {
                return Some(self.fail(reaper, "failed", "REAPER's audio sent nothing before Kumi stopped listening"));
            }
            if self.started.elapsed() > SILENT_START {
                return Some(self.fail(reaper, "failed", "REAPER's audio sent nothing: is its audio device running?"));
            }
            return None;
        };
        // The take ends at a gap the audio thread had to leave (the ring was full).
        let target = ((self.target * rate).round() as u64).min(ring.gap().unwrap_or(u64::MAX));
        if self.writer.is_none() {
            match wav::Writer::create(&self.file, 2, rate.round() as u32) {
                Ok(writer) => self.writer = Some(writer),
                Err(error) => return Some(self.fail(reaper, "failed", &format!("Kumi couldn't write {} ({error})", self.file.display()))),
            }
        }
        self.follow(reaper, rate);
        let written = self.writer.as_ref().map_or(0, wav::Writer::frames);
        self.buffer.clear();
        ring.drain(&mut self.buffer, (target.saturating_sub(written)) as usize);
        if let Some(writer) = self.writer.as_mut() {
            if let Err(error) = writer.write(&self.buffer) {
                return Some(self.fail(reaper, "failed", &format!("Kumi couldn't write the capture ({error})")));
            }
        }
        let written = self.writer.as_ref().map_or(0, wav::Writer::frames);
        let heard = written as f64 / rate;
        let state = reaper.play_state(self.proj);
        let partial_note = if ring.gap().is_some_and(|gap| written >= gap) {
            Some(format!("Kumi fell behind REAPER's audio after {heard:.1} s (REAPER's main thread was held): the take ends there"))
        } else if written >= target {
            None
        } else if cancelled {
            Some(format!("Kumi stopped listening after {heard:.1} s"))
        } else if ring.rate_changed.load(Ordering::Relaxed) {
            Some(format!("REAPER's sample rate changed after {heard:.1} s: the take ends there"))
        } else if !reaper.audio_running() {
            Some(format!("REAPER's audio stopped after {heard:.1} s"))
        } else if !(state.playing || state.recording) || state.paused {
            Some(format!("REAPER stopped after {heard:.1} s"))
        } else {
            return None;
        };
        Some(self.finish(reaper, rate, partial_note))
    }

    fn stop_hook(&mut self, reaper: &Reaper) {
        self.registered.ring().close();
        reaper.register_audio_hook(false, self.registered.as_ptr());
    }

    fn finish(&mut self, reaper: &Reaper, rate: f64, partial: Option<String>) -> Ended {
        // How far REAPER's position strayed from where the frames counted since the last mark put it (for the log).
        let drift = match (self.marks.last(), self.last) {
            (Some(mark), Some((frame, position, true))) => mark[0].as_u64().zip(mark[1].as_f64()).map(|(mark_frame, mark_seconds)| {
                let counted = frame.saturating_sub(mark_frame) as f64 / rate;
                ((position - mark_seconds - counted * reaper.play_rate(self.proj)) * 1000.0, counted)
            }),
            _ => None,
        };
        self.stop_hook(reaper);
        let ring = self.registered.ring();
        crate::log::line(&format!(
            "capture {}: {} frames, {} lost (the ring was full), device outputs {}, {} blocks without a buffer, peak {:.4}, drift {}",
            self.job,
            ring.frames(),
            ring.dropped(),
            ring.outputs.load(Ordering::Relaxed),
            ring.missing.load(Ordering::Relaxed),
            f32::from_bits(ring.peak.load(Ordering::Relaxed)),
            drift.map_or("unknown".into(), |(ms, over)| format!("{ms:+.1} ms over {over:.1} s"))
        ));
        let frames = match self.writer.take().map(wav::Writer::finish) {
            Some(Ok(frames)) => frames,
            Some(Err(error)) => return self.fail(reaper, "failed", &format!("Kumi couldn't finish the capture ({error})")),
            None => 0,
        };
        let mut notes = std::mem::take(&mut self.notes);
        let is_partial = partial.is_some();
        notes.extend(partial);
        let mut result = serde_json::Map::new();
        result.insert(
            "files".into(),
            json!([{
                "id": "master",
                "file": self.file.to_string_lossy(),
                "start": 0.0,
                "seconds": crate::rows::seconds(frames as f64 / rate),
                "channels": 2,
                "sampleRate": rate.round() as u32,
                "stage": "master",
                "throughMaster": false,
                "partial": is_partial,
            }]),
        );
        result.insert("marks".into(), Value::Array(std::mem::take(&mut self.marks)));
        if !notes.is_empty() {
            result.insert("notes".into(), json!(notes));
        }
        Ended { state: super::board::State::Done, result }
    }

    fn fail(&mut self, reaper: &Reaper, code: &str, error: &str) -> Ended {
        self.stop_hook(reaper);
        self.writer = None;
        let _ = std::fs::remove_file(&self.file);
        let mut result = serde_json::Map::new();
        result.insert("error".into(), error.into());
        result.insert("code".into(), code.into());
        if !self.notes.is_empty() {
            result.insert("notes".into(), json!(self.notes));
        }
        Ended { state: super::board::State::Failed, result }
    }

    /// The bridge is going: the hook comes out at once, the file is deleted.
    pub fn abandon(&mut self, reaper: &Reaper) {
        self.fail(reaper, "failed", "the bridge unloaded");
    }

    /// Progress, for the status: the part of the target the file has.
    pub fn progress(&self) -> f64 {
        let rate = self.registered.ring().rate().unwrap_or(48000.0);
        let written = self.writer.as_ref().map_or(0, wav::Writer::frames) as f64;
        (written / (self.target * rate)).clamp(0.0, 0.99)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_ring_hands_over_whole_frames_in_order_and_counts_what_didnt_fit() {
        let ring = Arc::new(Ring::new(1024, 0, 1));
        assert_eq!(ring.capacity(), 1024);
        let mut registered = Registered::new(ring.clone());
        registered.feed(100, 48000.0, 2, |channel, i| if channel == 0 { i as f64 } else { -(i as f64) });
        assert_eq!(ring.rate(), Some(48000.0));
        let mut out = Vec::new();
        assert_eq!(ring.drain(&mut out, 1000), 100);
        assert_eq!(&out[..6], &[0.0, -0.0, 1.0, -1.0, 2.0, -2.0]);
        // 512 frames fit; a block of 600 with nothing drained loses 88, counted, and the take's gap is where they began.
        registered.feed(600, 48000.0, 2, |_, _| 0.5);
        assert_eq!((ring.frames(), ring.dropped(), ring.gap()), (700, 88, Some(612)));
        out.clear();
        assert_eq!(ring.drain(&mut out, 10_000), 512);
        // A channel past what the device has is silence; a closed ring takes nothing.
        let mono = Arc::new(Ring::new(1024, 0, 5));
        let mut registered = Registered::new(mono.clone());
        registered.feed(4, 44100.0, 2, |_, _| 0.25);
        out.clear();
        mono.drain(&mut out, 4);
        assert_eq!(out, [0.25, 0.0, 0.25, 0.0, 0.25, 0.0, 0.25, 0.0]);
        mono.close();
        registered.feed(4, 44100.0, 2, |_, _| 0.25);
        assert_eq!(mono.frames(), 4);
        // A later block at another rate is noticed.
        let changing = Arc::new(Ring::new(1024, 0, 1));
        let mut registered = Registered::new(changing.clone());
        registered.feed(4, 44100.0, 2, |_, _| 0.0);
        registered.feed(4, 48000.0, 2, |_, _| 0.0);
        assert!(changing.rate_changed.load(Ordering::Relaxed));
        assert_eq!(changing.rate(), Some(44100.0));
    }
}
