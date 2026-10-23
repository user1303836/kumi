//! Kumi's media layer: audio (decoding, analysis, matching a sound, DSP, the listen tool), voice input, and the
//! programs that decode media (`programs`: ffmpeg, yt-dlp and whisper.cpp, fetched where they're needed) with what a video's
//! frames, captions and speech give (`frames`, `captions`, `speech`).

pub mod audio;
pub mod captions;
pub mod frames;
pub mod programs;
pub mod speech;
pub mod voice;
