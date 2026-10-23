//! The helper's process tap, tried on `afplay` (never on Logic Pro): writes a test tone (a second of silence, then 3 s
//! of a 440 Hz sine at -30 dBFS, a 32-bit float stereo WAV), plays it with `afplay`, taps afplay's process with
//! `mute: true` as soon as Core Audio has a process object for it (the silence lets the mute begin before the tone
//! does), stops the tap when afplay ends, and checks the capture: at least 2 s long, its peak within 1 dB of -30 dBFS,
//! 440 Hz within 2 Hz by its zero crossings, its anchors in order and agreeing with the sample rate within 1 ms, no
//! frame dropped. It prints how long `tap.start` and `tap.stop` took to answer, then deletes its files.
//!
//! cargo run -p logic-kumi --example tap_probe -- [--dir <folder>] [--helper <path>] [--seconds <n>] [--keep]
//!
//! `--dir` is where the tone and the capture go (the system's temporary folder by default), `--seconds` makes the
//! tone longer than 3 s (a longer capture shows drops and drift the short one can't), `--keep` leaves the files
//! there. The tone is heard only if the mute fails, and quietly: run the probe when nobody is listening anyway.

use std::path::{Path, PathBuf};
use std::process::{Child, Command};
use std::time::{Duration, Instant};

use logic_kumi::helper::{tap_start, tap_status, tap_stop, ChildHelper, Helper, HelperError, TapCapture};

const RATE: u32 = 48_000;
const LEAD_SECONDS: f64 = 1.0;
const FREQUENCY: f64 = 440.0;
const LEVEL_DB: f64 = -30.0;

fn main() {
    let mut args = std::env::args().skip(1);
    let (mut dir, mut helper_path, mut keep, mut seconds) = (std::env::temp_dir(), None, false, 3.0);
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--dir" => dir = PathBuf::from(args.next().expect("--dir <folder>")),
            "--helper" => helper_path = Some(PathBuf::from(args.next().expect("--helper <path>"))),
            "--seconds" => {
                seconds = args.next().and_then(|s| s.parse::<f64>().ok()).filter(|s| *s >= 3.0).expect("--seconds <n>, 3 or more")
            }
            "--keep" => keep = true,
            other => panic!("unknown argument {other}; usage: tap_probe [--dir <folder>] [--helper <path>] [--seconds <n>] [--keep]"),
        }
    }
    let helper_path = helper_path
        .or_else(|| std::env::var("KUMI_LOGIC_HANDS").ok().map(PathBuf::from))
        .or_else(|| {
            let dir = kumi_testkit::workspace_path("target/logic-hands");
            std::fs::read_dir(dir)
                .ok()?
                .flatten()
                .map(|e| e.path())
                .find(|p| p.file_name().is_some_and(|n| n.to_string_lossy().starts_with("kumi-logic-hands-")))
        })
        .expect("the helper: build it with python3 scripts/build-logic-hands.py, or name it with --helper");
    let tone = dir.join(format!("kumi-tap-probe-{}-tone.wav", std::process::id()));
    let capture_path = dir.join(format!("kumi-tap-probe-{}-capture.wav", std::process::id()));
    write_tone(&tone, seconds);

    let mut helper = ChildHelper::spawn(&helper_path).expect("the helper starts");
    let mut player = Command::new("afplay").arg(&tone).spawn().expect("afplay plays the tone");
    // Room for the whole tone and its silence, and a margin for afplay's start and end.
    let outcome = run(&mut helper, &mut player, &capture_path, LEAD_SECONDS + seconds + 30.0);
    let _ = player.kill();
    let _ = player.wait();
    helper.quit();
    let failures = match outcome {
        Ok(failures) => failures,
        Err(why) => vec![why],
    };
    if !keep {
        for file in [&tone, &capture_path] {
            let _ = std::fs::remove_file(file);
        }
    }
    if failures.is_empty() {
        println!("tap probe: every check passed");
    } else {
        for failure in &failures {
            println!("FAILED: {failure}");
        }
        std::process::exit(1);
    }
}

/// The tap on the player, the capture and its checks: what failed.
fn run(helper: &mut ChildHelper, player: &mut Child, capture_path: &Path, max_seconds: f64) -> Result<Vec<String>, String> {
    // Core Audio makes the player's process object once it starts its audio, a few tens of milliseconds in.
    let launched = Instant::now();
    let (started, start_ms) = loop {
        let asked = Instant::now();
        match tap_start(helper, player.id(), capture_path, true, Some(max_seconds)) {
            Ok(started) => break (started, asked.elapsed().as_secs_f64() * 1000.0),
            Err(HelperError::Refused { reply, .. }) if reply["step"] == "process" && launched.elapsed() < Duration::from_secs(2) => {
                std::thread::sleep(Duration::from_millis(10));
            }
            Err(error) => return Err(format!("tap.start: {error}")),
        }
    };
    println!(
        "tap.start answered in {start_ms:.1} ms, {:.0} ms after afplay was launched: {} Hz, {} channels, host time {}",
        launched.elapsed().as_secs_f64() * 1000.0,
        started.sample_rate,
        started.channels,
        started.host_time
    );
    let status = player.wait().map_err(|e| format!("afplay: {e}"))?;
    if !status.success() {
        return Err(format!("afplay exited with {status}"));
    }
    let running = tap_status(helper).map_err(|e| format!("tap.status: {e}"))?;
    let asked = Instant::now();
    let capture = tap_stop(helper).map_err(|e| format!("tap.stop: {e}"))?;
    let stop_ms = asked.elapsed().as_secs_f64() * 1000.0;
    println!("tap.stop answered in {stop_ms:.1} ms (tap.status said {} frames, running {})", running.frames, running.running);
    Ok(check(&capture, capture_path))
}

/// The tone: silence first, then the sine with 10 ms fades, the same in both channels.
fn write_tone(path: &Path, seconds: f64) {
    let amplitude = 10f64.powf(LEVEL_DB / 20.0);
    let lead = (LEAD_SECONDS * f64::from(RATE)) as usize;
    let length = (seconds * f64::from(RATE)) as usize;
    let fade = (0.01 * f64::from(RATE)) as usize;
    let mut samples = vec![0f32; 2 * lead];
    for index in 0..length {
        let gain = 1f64.min(index as f64 / fade as f64).min((length - 1 - index) as f64 / fade as f64);
        let value = (amplitude * gain * (std::f64::consts::TAU * FREQUENCY * index as f64 / f64::from(RATE)).sin()) as f32;
        samples.extend([value, value]);
    }
    let data: Vec<u8> = samples.iter().flat_map(|s| s.to_le_bytes()).collect();
    let mut bytes = Vec::with_capacity(44 + data.len());
    bytes.extend(b"RIFF");
    bytes.extend((36 + data.len() as u32).to_le_bytes());
    bytes.extend(b"WAVEfmt ");
    bytes.extend(16u32.to_le_bytes());
    bytes.extend(3u16.to_le_bytes());
    bytes.extend(2u16.to_le_bytes());
    bytes.extend(RATE.to_le_bytes());
    bytes.extend((RATE * 8).to_le_bytes());
    bytes.extend(8u16.to_le_bytes());
    bytes.extend(32u16.to_le_bytes());
    bytes.extend(b"data");
    bytes.extend((data.len() as u32).to_le_bytes());
    bytes.extend(data);
    std::fs::write(path, bytes).expect("the tone is written");
}

/// A 32-bit float WAV as the helper writes it: rate, channels and the samples.
fn read_wav(path: &Path) -> Result<(u32, u16, Vec<f32>), String> {
    let bytes = std::fs::read(path).map_err(|e| format!("reading {}: {e}", path.display()))?;
    let u16_at = |at: usize| bytes.get(at..at + 2).map(|b| u16::from_le_bytes([b[0], b[1]]));
    let u32_at = |at: usize| bytes.get(at..at + 4).map(|b| u32::from_le_bytes([b[0], b[1], b[2], b[3]]));
    if bytes.get(0..4) != Some(b"RIFF") || bytes.get(8..16) != Some(b"WAVEfmt ") || bytes.get(36..40) != Some(b"data") {
        return Err("the capture's header isn't RIFF WAVE fmt … data".into());
    }
    let (format, channels, rate, bits) = (u16_at(20), u16_at(22), u32_at(24), u16_at(34));
    if format != Some(3) || bits != Some(32) {
        return Err(format!("the capture is format {format:?} with {bits:?} bits, not 3 and 32"));
    }
    let size = u32_at(40).unwrap_or(0) as usize;
    if u32_at(4) != Some(36 + size as u32) || bytes.len() != 44 + size {
        return Err(format!("the header's sizes ({:?}, {size}) don't match the file's {} bytes", u32_at(4), bytes.len()));
    }
    let samples = bytes[44..].as_chunks::<4>().0.iter().map(|b| f32::from_le_bytes(*b)).collect();
    Ok((rate.unwrap_or(0), channels.unwrap_or(0), samples))
}

/// What the capture fails of the probe's checks.
fn check(capture: &TapCapture, path: &Path) -> Vec<String> {
    let mut failures = vec![];
    let (rate, channels, samples) = match read_wav(path) {
        Ok(read) => read,
        Err(why) => return vec![why],
    };
    let width = usize::from(channels.max(1));
    let frames = samples.len() / width;
    if (f64::from(rate), u32::from(channels), frames as u64) != (capture.sample_rate.round(), capture.channels, capture.frames) {
        failures.push(format!(
            "the file says {rate} Hz, {channels} channels, {frames} frames; tap.stop said {} Hz, {} channels, {} frames",
            capture.sample_rate, capture.channels, capture.frames
        ));
    }
    let peak = samples.iter().fold(0f32, |top, s| top.max(s.abs())) as f64;
    let peak_db = 20.0 * peak.max(1e-12).log10();
    let rms_db = 20.0 * capture.rms.max(1e-12).log10();
    println!(
        "capture: {:.3} s ({} frames), peak {peak_db:.2} dBFS (tap.stop said {:.2}), rms {rms_db:.2} dBFS, dropped {}, truncated {}",
        capture.seconds,
        capture.frames,
        20.0 * capture.peak.max(1e-12).log10(),
        capture.dropped,
        capture.truncated
    );
    if capture.seconds < 2.0 {
        failures.push(format!("the capture is {:.3} s, under 2 s", capture.seconds));
    }
    if (peak_db - LEVEL_DB).abs() > 1.0 {
        failures.push(format!("the peak is {peak_db:.2} dBFS, not within 1 dB of {LEVEL_DB}"));
    }
    if (capture.peak - peak).abs() > 1e-6 {
        failures.push(format!("tap.stop's peak {} isn't the file's {peak}", capture.peak));
    }
    match frequency(&samples, width, f64::from(rate)) {
        Some(heard) => {
            println!("frequency by zero crossings: {heard:.3} Hz");
            if (heard - FREQUENCY).abs() > 2.0 {
                failures.push(format!("the tone is {heard:.3} Hz, not within 2 Hz of {FREQUENCY}"));
            }
        }
        None => failures.push("the capture holds no tone".into()),
    }
    let tick = |ticks: i128| capture.timebase.seconds(ticks);
    let in_order = capture.anchors.windows(2).all(|pair| pair[1].0 > pair[0].0 && pair[1].1 > pair[0].1);
    let worst = capture
        .anchors
        .iter()
        .map(|&(frame, host)| {
            let (first_frame, first_host) = capture.anchors[0];
            let by_clock = tick(i128::from(host) - i128::from(first_host));
            let by_frames = (frame - first_frame) as f64 / capture.sample_rate;
            (by_clock - by_frames).abs()
        })
        .fold(0f64, f64::max);
    let span = capture.anchors.last().map(|&(frame, _)| frame as f64 / capture.sample_rate).unwrap_or(0.0);
    println!(
        "anchors: {} over {span:.3} s, in order {in_order}, the clock and the frames agree within {:.3} ms (timebase {}/{})",
        capture.anchors.len(),
        worst * 1000.0,
        capture.timebase.numer,
        capture.timebase.denom
    );
    if capture.anchors.len() < 2 || !in_order {
        failures.push(format!("the anchors aren't in order, or too few: {:?}", capture.anchors));
    }
    if worst > 0.001 {
        failures.push(format!("an anchor's host time is {:.3} ms off its frame at the sample rate", worst * 1000.0));
    }
    if capture.dropped != 0 {
        failures.push(format!("{} frames dropped", capture.dropped));
    }
    failures
}

/// The tone's frequency from its upward zero crossings (interpolated between samples) where the first channel is above
/// half its peak: the fades and the silence around it are left out.
fn frequency(samples: &[f32], width: usize, rate: f64) -> Option<f64> {
    let left: Vec<f32> = samples.iter().step_by(width).copied().collect();
    let peak = left.iter().fold(0f32, |top, s| top.max(s.abs()));
    let loud = |s: &f32| s.abs() > peak / 2.0;
    let first = left.iter().position(loud)?;
    let last = left.iter().rposition(loud)?;
    let crossings: Vec<f64> = (first..last)
        .filter(|&i| left[i] < 0.0 && left[i + 1] >= 0.0)
        .map(|i| i as f64 + f64::from(-left[i]) / f64::from(left[i + 1] - left[i]))
        .collect();
    if crossings.len() < 3 {
        return None;
    }
    Some((crossings.len() - 1) as f64 * rate / (crossings[crossings.len() - 1] - crossings[0]))
}
