---
name: listening
description: What Kumi's audio measurements mean and how to report them honestly - loudness, true peak, spectral balance, comparisons at matched loudness, mono limits; measured, never heard.
---
# Measuring audio with Kumi

`kumi_listen` measures an audio file on this computer. Nothing is heard, by Kumi or by you: report measurements as measurements.

## What it measures
- **Loudness:** integrated LUFS. Higher is louder, and streaming services normalise around −14 LUFS.
- **Loudness range (LRA, in LU):** how much the loudness moves over the file.
- **True peak (dBTP):** the highest peak between samples. Above −1 dBTP risks clipping after encoding.
- **Spectral centroid (Hz) and band levels:** the balance of low and high energy. A lower centroid, or less energy in the high bands, is what "darker" usually means. More upper-mid energy reads as "brighter" or "harsher".
- **Tempo and key, when they can be found.** Treat them as estimates.

## Comparing
- **Use `compare_to` for two files,** and report the measured differences.
- **Louder seems better.** Compare tone at matched loudness: say how far apart the files are in LUFS before you say which is brighter or fuller.
- **The measurements may be of a mono sum.** They can't tell you about stereo width or placement.
- **Short one-shots give few frames.** Say when a measurement rests on very little audio.

## Reporting
- **Name what was measured, the numbers and what they suggest:** "The replacement measures 410 Hz lower in spectral centroid and 1.2 LU quieter, which fits a darker snare."
- **Never say a file "sounds" a way** unless the producer said it.
- **Measuring a file changes nothing in the project.** Changes go through `kumi_change` (skill `kumi:daw`).
- **To share the measured file,** put the result's `MEDIA:` line in your reply, on a line of its own. In a messaging chat (Telegram, Discord…) only a `MEDIA:` line in your answer reaches the producer; one inside a tool result doesn't.
