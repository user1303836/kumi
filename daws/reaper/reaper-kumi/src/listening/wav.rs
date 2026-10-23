//! WAV files: the header of a render REAPER wrote (its chunks walked, as REAPER adds `bext` and `junk`), and the 32-bit
//! float files Kumi writes itself (a capture, a track's items), owner-only from their first byte.

use std::fs::File;
use std::io::{self, BufWriter, Read, Seek, SeekFrom, Write};
use std::path::Path;

/// What a WAV file holds.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Info {
    pub channels: u16,
    pub rate: u32,
    pub bits: u16,
    pub float: bool,
    pub frames: u64,
}

impl Info {
    pub fn seconds(&self) -> f64 {
        if self.rate == 0 {
            0.0
        } else {
            self.frames as f64 / self.rate as f64
        }
    }
}

const FORMAT_FLOAT: u16 = 3;
const FORMAT_EXTENSIBLE: u16 = 0xFFFE;

fn u16_at(bytes: &[u8], at: usize) -> u16 {
    u16::from_le_bytes([bytes[at], bytes[at + 1]])
}

fn u32_at(bytes: &[u8], at: usize) -> u32 {
    u32::from_le_bytes([bytes[at], bytes[at + 1], bytes[at + 2], bytes[at + 3]])
}

/// Reads a WAV file's header: its format and how many frames its data holds. Only the chunk headers are read, never the
/// audio. RF64 (`ds64`) sizes are taken where a file has them.
pub fn read_info(path: &Path) -> io::Result<Info> {
    let invalid = |why: &str| io::Error::new(io::ErrorKind::InvalidData, format!("{} isn't a WAV file Kumi reads: {why}", path.display()));
    let mut file = File::open(path)?;
    let length = file.metadata()?.len();
    let mut head = [0u8; 12];
    file.read_exact(&mut head)?;
    let rf64 = &head[0..4] == b"RF64";
    if !(&head[0..4] == b"RIFF" || rf64) || &head[8..12] != b"WAVE" {
        return Err(invalid("no RIFF/WAVE header"));
    }
    let mut format: Option<(u16, u16, u32, u16)> = None;
    let mut big_data: Option<u64> = None;
    let mut at = 12u64;
    while at + 8 <= length {
        file.seek(SeekFrom::Start(at))?;
        let mut chunk = [0u8; 8];
        file.read_exact(&mut chunk)?;
        let size = u32_at(&chunk, 4) as u64;
        match &chunk[0..4] {
            b"ds64" => {
                let mut ds64 = [0u8; 24];
                file.read_exact(&mut ds64)?;
                big_data = Some(u64::from_le_bytes(ds64[8..16].try_into().unwrap_or_default()));
            }
            b"fmt " => {
                let mut fmt = vec![0u8; size.min(64) as usize];
                file.read_exact(&mut fmt)?;
                if fmt.len() < 16 {
                    return Err(invalid("its format chunk is short"));
                }
                let mut tag = u16_at(&fmt, 0);
                if tag == FORMAT_EXTENSIBLE && fmt.len() >= 26 {
                    tag = u16_at(&fmt, 24);
                }
                format = Some((tag, u16_at(&fmt, 2), u32_at(&fmt, 4), u16_at(&fmt, 14)));
            }
            b"data" => {
                let (tag, channels, rate, bits) = format.ok_or_else(|| invalid("its data comes before its format"))?;
                let size = match (rf64, size, big_data) {
                    (true, 0xFFFF_FFFF, Some(big)) => big,
                    _ => size,
                };
                // A render cut short has fewer bytes than its header says.
                let size = size.min(length.saturating_sub(at + 8));
                let frame = channels as u64 * (bits as u64 / 8);
                return Ok(Info { channels, rate, bits, float: tag == FORMAT_FLOAT, frames: if frame == 0 { 0 } else { size / frame } });
            }
            _ => {}
        }
        at += 8 + size + (size & 1);
    }
    Err(invalid("it has no data"))
}

/// A 32-bit float WAV Kumi writes as the audio comes: the sizes in its header are written when it is finished.
pub struct Writer {
    file: BufWriter<File>,
    channels: u16,
    frames: u64,
}

/// Makes a new file, owner-only (0600) from its first byte; an existing one is never written over.
pub fn create_owner_only(path: &Path) -> io::Result<File> {
    let mut options = std::fs::OpenOptions::new();
    options.write(true).read(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    options.open(path)
}

impl Writer {
    pub fn create(path: &Path, channels: u16, rate: u32) -> io::Result<Writer> {
        let file = create_owner_only(path)?;
        let mut file = BufWriter::with_capacity(1 << 16, file);
        let block = channels * 4;
        file.write_all(b"RIFF")?;
        file.write_all(&0u32.to_le_bytes())?;
        file.write_all(b"WAVEfmt ")?;
        file.write_all(&16u32.to_le_bytes())?;
        file.write_all(&FORMAT_FLOAT.to_le_bytes())?;
        file.write_all(&channels.to_le_bytes())?;
        file.write_all(&rate.to_le_bytes())?;
        file.write_all(&(rate * block as u32).to_le_bytes())?;
        file.write_all(&block.to_le_bytes())?;
        file.write_all(&32u16.to_le_bytes())?;
        file.write_all(b"data")?;
        file.write_all(&0u32.to_le_bytes())?;
        Ok(Writer { file, channels, frames: 0 })
    }

    /// Interleaved samples, whole frames.
    pub fn write(&mut self, samples: &[f32]) -> io::Result<()> {
        let mut bytes = Vec::with_capacity(samples.len() * 4);
        for sample in samples {
            bytes.extend_from_slice(&sample.to_le_bytes());
        }
        self.file.write_all(&bytes)?;
        self.frames += (samples.len() / self.channels.max(1) as usize) as u64;
        Ok(())
    }

    pub fn frames(&self) -> u64 {
        self.frames
    }

    /// Writes the sizes into the header and closes the file.
    pub fn finish(mut self) -> io::Result<u64> {
        let data = self.frames * self.channels as u64 * 4;
        let data32 = u32::try_from(data).unwrap_or(u32::MAX);
        self.file.flush()?;
        let file = self.file.get_mut();
        file.seek(SeekFrom::Start(4))?;
        file.write_all(&(36u32.saturating_add(data32)).to_le_bytes())?;
        file.seek(SeekFrom::Start(40))?;
        file.write_all(&data32.to_le_bytes())?;
        file.flush()?;
        Ok(self.frames)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_written_file_reads_back_with_its_frames_and_format() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("a.wav");
        let mut writer = Writer::create(&path, 2, 48000).unwrap();
        writer.write(&[0.5, -0.5, 1.5, -1.5]).unwrap();
        writer.write(&[0.25, 0.25]).unwrap();
        assert_eq!(writer.finish().unwrap(), 3);
        let info = read_info(&path).unwrap();
        assert_eq!(info, Info { channels: 2, rate: 48000, bits: 32, float: true, frames: 3 });
        assert_eq!(std::fs::metadata(&path).unwrap().len(), 44 + 24);
        // Over full scale stays as it is: float.
        let bytes = std::fs::read(&path).unwrap();
        assert_eq!(f32::from_le_bytes(bytes[52..56].try_into().unwrap()), 1.5);
        // Never over an existing file.
        assert!(Writer::create(&path, 2, 48000).is_err());
        #[cfg(unix)]
        assert!(daw_bridge::owner::is_owner_only(&path));
    }

    /// REAPER's render: `bext` and `junk` before the data (measured in 7.82).
    #[test]
    fn chunks_before_the_data_are_walked_past() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("r.wav");
        let mut bytes = Vec::new();
        bytes.extend_from_slice(b"RIFF\0\0\0\0WAVE");
        bytes.extend_from_slice(b"bext");
        bytes.extend_from_slice(&602u32.to_le_bytes());
        bytes.extend_from_slice(&[0u8; 602]);
        bytes.extend_from_slice(b"junk");
        bytes.extend_from_slice(&27u32.to_le_bytes());
        bytes.extend_from_slice(&[0u8; 28]);
        bytes.extend_from_slice(b"fmt ");
        bytes.extend_from_slice(&16u32.to_le_bytes());
        for value in [3u16, 2] {
            bytes.extend_from_slice(&value.to_le_bytes());
        }
        bytes.extend_from_slice(&44100u32.to_le_bytes());
        bytes.extend_from_slice(&(44100u32 * 8).to_le_bytes());
        for value in [8u16, 32] {
            bytes.extend_from_slice(&value.to_le_bytes());
        }
        bytes.extend_from_slice(b"data");
        bytes.extend_from_slice(&80u32.to_le_bytes());
        bytes.extend_from_slice(&[0u8; 80]);
        std::fs::write(&path, &bytes).unwrap();
        let info = read_info(&path).unwrap();
        assert_eq!((info.channels, info.rate, info.float, info.frames), (2, 44100, true, 10));
        // A file cut short counts only what it has.
        std::fs::write(&path, &bytes[..bytes.len() - 40]).unwrap();
        assert_eq!(read_info(&path).unwrap().frames, 5);
        std::fs::write(&path, b"not a wav file at all").unwrap();
        assert_eq!(read_info(&path).unwrap_err().kind(), io::ErrorKind::InvalidData);
    }
}
