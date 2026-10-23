//! The audio of a listening pass: the helper's capture read back (a 32-bit float WAV file), a part of it cut into a
//! file of its own with its peak and RMS, and the onsets in a stretch of it (the metronome's clicks a pass is aligned
//! by, the notes the alignment was measured with). Files are read and written in blocks, so a long capture never sits
//! in memory whole.

use std::fs::File;
use std::io::{self, BufWriter, Read, Seek, SeekFrom, Write};
use std::path::Path;

/// A WAV file's format and where its samples are: 32-bit float (format 3, or extensible with the float sub-format)
/// is what the helper writes and what a cut is written as.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Wav {
    pub rate: u32,
    pub channels: u16,
    /// Where the `data` chunk's samples start in the file, and how many frames it holds.
    pub data: u64,
    pub frames: u64,
}

fn bad(why: impl Into<String>) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, why.into())
}

/// Reads a 32-bit float WAV's header: its rate, channels and samples' place. A header whose data size says more than
/// the file holds (a capture whose last patch didn't land) is taken at the file's length.
pub fn open(path: &Path) -> io::Result<(File, Wav)> {
    let mut file = File::open(path)?;
    let length = file.metadata()?.len();
    let mut head = [0u8; 12];
    file.read_exact(&mut head)?;
    if &head[0..4] != b"RIFF" || &head[8..12] != b"WAVE" {
        return Err(bad(format!("{} isn't a WAVE file", path.display())));
    }
    let (mut format, mut at) = (None, 12u64);
    for _ in 0..64 {
        file.seek(SeekFrom::Start(at))?;
        let mut chunk = [0u8; 8];
        file.read_exact(&mut chunk)?;
        let size = u64::from(u32::from_le_bytes([chunk[4], chunk[5], chunk[6], chunk[7]]));
        match &chunk[0..4] {
            b"fmt " => {
                let mut fmt = [0u8; 40];
                let wanted = size.min(40) as usize;
                file.read_exact(&mut fmt[..wanted])?;
                let tag = u16::from_le_bytes([fmt[0], fmt[1]]);
                let channels = u16::from_le_bytes([fmt[2], fmt[3]]);
                let rate = u32::from_le_bytes([fmt[4], fmt[5], fmt[6], fmt[7]]);
                let bits = u16::from_le_bytes([fmt[14], fmt[15]]);
                let float = tag == 3 || (tag == 0xFFFE && wanted >= 26 && u16::from_le_bytes([fmt[24], fmt[25]]) == 3);
                if !float || bits != 32 || channels == 0 || rate == 0 {
                    return Err(bad(format!("{} isn't 32-bit float audio (format {tag}, {bits} bits)", path.display())));
                }
                format = Some((rate, channels));
            }
            b"data" => {
                let (rate, channels) = format.ok_or_else(|| bad(format!("{} has its samples before its format", path.display())))?;
                let block = u64::from(channels) * 4;
                let held = size.min(length.saturating_sub(at + 8));
                return Ok((file, Wav { rate, channels, data: at + 8, frames: held / block }));
            }
            _ => {}
        }
        at += 8 + size + (size & 1);
    }
    Err(bad(format!("{} has no samples", path.display())))
}

/// Frames `[from, from + count)` of an open file, interleaved; fewer at the file's end.
pub fn read_frames(file: &mut File, wav: &Wav, from: u64, count: u64) -> io::Result<Vec<f32>> {
    let from = from.min(wav.frames);
    let count = count.min(wav.frames - from);
    let block = u64::from(wav.channels) * 4;
    file.seek(SeekFrom::Start(wav.data + from * block))?;
    let mut bytes = vec![0u8; (count * block) as usize];
    file.read_exact(&mut bytes)?;
    Ok(bytes.as_chunks::<4>().0.iter().map(|b| f32::from_le_bytes(*b)).collect())
}

/// A 32-bit float WAV header for `frames` frames.
pub fn header(rate: u32, channels: u16, frames: u64) -> Vec<u8> {
    let block = u32::from(channels) * 4;
    let bytes = u32::try_from(frames * u64::from(block)).unwrap_or(u32::MAX - 36);
    let mut out = Vec::with_capacity(44);
    out.extend_from_slice(b"RIFF");
    out.extend_from_slice(&(36 + bytes).to_le_bytes());
    out.extend_from_slice(b"WAVEfmt ");
    out.extend_from_slice(&16u32.to_le_bytes());
    out.extend_from_slice(&3u16.to_le_bytes());
    out.extend_from_slice(&channels.to_le_bytes());
    out.extend_from_slice(&rate.to_le_bytes());
    out.extend_from_slice(&(rate * block).to_le_bytes());
    out.extend_from_slice(&(block as u16).to_le_bytes());
    out.extend_from_slice(&32u16.to_le_bytes());
    out.extend_from_slice(b"data");
    out.extend_from_slice(&bytes.to_le_bytes());
    out
}

/// What a cut wrote: its length in frames, its format, and its largest sample and RMS (linear, every channel).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Cut {
    pub frames: u64,
    pub rate: u32,
    pub channels: u16,
    pub peak: f64,
    pub rms: f64,
}

/// Frames read and written a block at a time.
const BLOCK_FRAMES: u64 = 16384;

/// Writes frames `[from, from + frames)` of `source` into a new file `dest` (never over a file that is there), as
/// 32-bit float in the source's format, measuring them on the way. Frames the source doesn't have (past its end) are
/// left out.
pub fn cut(source: &Path, dest: &Path, from: u64, frames: u64) -> io::Result<Cut> {
    let (mut file, wav) = open(source)?;
    let from = from.min(wav.frames);
    let frames = frames.min(wav.frames - from);
    let mut out = BufWriter::new(File::options().write(true).create_new(true).open(dest)?);
    out.write_all(&header(wav.rate, wav.channels, frames))?;
    let (mut peak, mut squares, mut done) = (0f64, 0f64, 0u64);
    while done < frames {
        let samples = read_frames(&mut file, &wav, from + done, BLOCK_FRAMES.min(frames - done))?;
        if samples.is_empty() {
            break;
        }
        for sample in &samples {
            let value = f64::from(*sample);
            peak = peak.max(value.abs());
            squares += value * value;
            out.write_all(&sample.to_le_bytes())?;
        }
        done += samples.len() as u64 / u64::from(wav.channels);
    }
    out.flush()?;
    let count = done * u64::from(wav.channels);
    let rms = if count > 0 { (squares / count as f64).sqrt() } else { 0.0 };
    Ok(Cut { frames: done, rate: wav.rate, channels: wav.channels, peak, rms })
}

/// The envelope's hop and how far back an onset looks, in seconds; how much louder than the stretch before it a hop
/// must be (a piano note over the one before it rises three to four times: the lab's captures of Scratch), the
/// quietest an onset may be, and how long after an onset the next may come.
const HOP: f64 = 0.0005;
const LOOK_BACK: f64 = 0.020;
const RISE: f32 = 3.0;
const FLOOR: f32 = 0.005;
const REST: f64 = 0.050;
/// An onset's sample: the first one at a tenth of the peak of the 10 ms from the hop it rose in.
const REFINE: f64 = 0.010;
const SHARE: f32 = 0.1;

/// Onsets in interleaved samples, as frames (from the first sample given): where the sound rises threefold over the
/// loudest of the 20 ms before, above -46 dBFS, placed at the first sample reaching a tenth of the peak that
/// follows. A click and a struck note are found within a fraction of a millisecond; what rises slowly is placed late.
pub fn onsets(samples: &[f32], channels: usize, rate: f64) -> Vec<f64> {
    let channels = channels.max(1);
    let mono: Vec<f32> = samples.chunks(channels).map(|frame| frame.iter().sum::<f32>() / frame.len() as f32).collect();
    let hop = ((rate * HOP) as usize).max(1);
    let envelope: Vec<f32> = mono.chunks(hop).map(|chunk| chunk.iter().fold(0f32, |top, s| top.max(s.abs()))).collect();
    let (look, rest, refine) = (((LOOK_BACK / HOP) as usize).max(1), ((REST / HOP) as usize).max(1), ((rate * REFINE) as usize).max(1));
    let mut found = vec![];
    let mut i = look;
    while i < envelope.len() {
        let before = envelope[i - look..i].iter().fold(0f32, |top, e| top.max(*e));
        if envelope[i] > FLOOR && envelope[i] > RISE * before + 1e-4 {
            let start = (i - 1) * hop;
            let peak = mono[start..(start + refine).min(mono.len())].iter().fold(0f32, |top, s| top.max(s.abs()));
            let first = (start..mono.len()).find(|&k| mono[k].abs() >= SHARE * peak).unwrap_or(start);
            found.push(first as f64);
            i += rest;
        } else {
            i += 1;
        }
    }
    found
}

/// The onsets of frames `[from, to)` of a file, as frames of the file.
pub fn onsets_in(path: &Path, from: u64, to: u64) -> io::Result<Vec<f64>> {
    let (mut file, wav) = open(path)?;
    let samples = read_frames(&mut file, &wav, from, to.saturating_sub(from))?;
    Ok(onsets(&samples, usize::from(wav.channels), f64::from(wav.rate)).into_iter().map(|frame| frame + from as f64).collect())
}

/// A stand-in for a recorded part: `frames` frames of a quiet 220 Hz tone at `rate` and `channels`, written where a
/// replayed scene's answer names a file (the recording's audio isn't kept).
pub fn synthesize(dest: &Path, rate: u32, channels: u16, frames: u64) -> io::Result<()> {
    let mut out = BufWriter::new(File::options().write(true).create_new(true).open(dest)?);
    out.write_all(&header(rate, channels, frames))?;
    for frame in 0..frames {
        let value = (0.1 * (std::f64::consts::TAU * 220.0 * frame as f64 / f64::from(rate.max(1))).sin()) as f32;
        for _ in 0..channels {
            out.write_all(&value.to_le_bytes())?;
        }
    }
    out.flush()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Silence with clicks at the frames given, stereo, at 44.1 kHz.
    fn clicks(frames: usize, at: &[usize]) -> Vec<f32> {
        let mut samples = vec![0f32; frames * 2];
        for &frame in at {
            for k in 0..32 {
                let value = 0.4 * (1.0 - k as f32 / 32.0);
                samples[(frame + k) * 2] = value;
                samples[(frame + k) * 2 + 1] = value;
            }
        }
        samples
    }

    fn write_capture(path: &Path, samples: &[f32], channels: u16) {
        let mut bytes = header(44100, channels, (samples.len() / usize::from(channels)) as u64);
        for sample in samples {
            bytes.extend_from_slice(&sample.to_le_bytes());
        }
        std::fs::write(path, bytes).unwrap();
    }

    #[test]
    fn clicks_and_struck_notes_are_found_where_they_start() {
        let found = onsets(&clicks(44100, &[4410, 21000, 37538]), 2, 44100.0);
        assert_eq!(found, vec![4410.0, 21000.0, 37538.0]);
        // A note over a decaying one: found at its rise, not at the louder tail before it.
        let mut samples = vec![0f32; 44100 * 2];
        for k in 0..20000 {
            let value = 0.3 * (-(k as f32) / 4000.0).exp() * (k as f32 * 0.07).sin();
            samples[(2000 + k) * 2] = value;
        }
        for k in 0..8000 {
            samples[(12000 + k) * 2] += 0.6 * (k as f32 * 0.09).sin().signum() * (-(k as f32) / 3000.0).exp();
        }
        let found = onsets(&samples, 2, 44100.0);
        assert_eq!(found.len(), 2, "{found:?}");
        assert!((found[0] - 2000.0).abs() <= 25.0 && (found[1] - 12000.0).abs() <= 2.0, "{found:?}");
        // Too quiet to count.
        let quiet: Vec<f32> = clicks(4410, &[1000]).iter().map(|s| s * 0.001).collect();
        assert!(onsets(&quiet, 2, 44100.0).is_empty());
    }

    #[test]
    fn a_part_is_cut_into_a_file_of_its_own_and_measured() {
        let dir = tempfile::tempdir().unwrap();
        let source = dir.path().join("tap.wav");
        let samples = clicks(44100, &[1000, 30000]);
        write_capture(&source, &samples, 2);
        let (_, wav) = open(&source).unwrap();
        assert_eq!(wav, Wav { rate: 44100, channels: 2, data: 44, frames: 44100 });
        let dest = dir.path().join("part.wav");
        let cut = cut(&source, &dest, 20000, 20000).unwrap();
        assert_eq!((cut.frames, cut.rate, cut.channels), (20000, 44100, 2));
        assert!((cut.peak - 0.4).abs() < 1e-6, "{cut:?}");
        let (mut file, part) = open(&dest).unwrap();
        assert_eq!(part.frames, 20000);
        let read = read_frames(&mut file, &part, 0, 20000).unwrap();
        assert_eq!(read, samples[40000..80000].to_vec());
        let expected = (samples[40000..80000].iter().map(|s| f64::from(*s).powi(2)).sum::<f64>() / 40000.0).sqrt();
        assert!((cut.rms - expected).abs() < 1e-9);
        assert_eq!(onsets_in(&dest, 0, 20000).unwrap(), vec![10000.0], "the click at 30000 is the part's 10000th frame");
        // Never over a file, and a part past the end is what is there.
        assert!(super::cut(&source, &dest, 0, 10).is_err());
        let tail = super::cut(&source, &dir.path().join("tail.wav"), 40000, 10000).unwrap();
        assert_eq!(tail.frames, 4100);
        // A header that says more than the file holds is read at the file's length.
        let mut bytes = std::fs::read(&source).unwrap();
        bytes.truncate(44 + 8 * 100);
        std::fs::write(&source, bytes).unwrap();
        assert_eq!(open(&source).unwrap().1.frames, 100);
        std::fs::write(&source, b"RIFF....WAVEfmt nothing").unwrap();
        assert!(open(&source).is_err());
        let synthesized = dir.path().join("stand-in.wav");
        synthesize(&synthesized, 48000, 2, 4800).unwrap();
        assert_eq!(open(&synthesized).unwrap().1, Wav { rate: 48000, channels: 2, data: 44, frames: 4800 });
        assert_eq!(crate::disk::read_sound(&synthesized).map(|s| (s.frames, s.float)), Some((4800, true)));
    }
}
