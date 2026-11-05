---
name: listening
description: What Kumi's audio measurements mean and how to report them honestly - loudness, true peak, spectral balance, comparisons at matched loudness, mono limits; measured, never heard.
---
# Measuring audio with Kumi

`kumi_listen` measures an audio file on this computer and, in the DAWs its description names, a track or the mix of the open project over a beat window (that plays the window in the DAW while it's stopped, so the producer approves it first). Nothing is heard, by Kumi or by you: report measurements as measurements.

## What it measures
- **Loudness:** integrated LUFS. Higher is louder, and streaming services normalise around −14 LUFS.
- **Loudness range (LRA, in LU):** how much the loudness moves over the file.
- **True peak (dBTP):** the highest peak between samples. Above −1 dBTP risks clipping after encoding.
- **Spectral centroid (Hz) and band levels:** the balance of low and high energy. A lower centroid, or less energy in the high bands, is what "darker" usually means. More upper-mid energy reads as "brighter" or "harsher".
- **Tempo and key, when they can be found.** Treat them as estimates.

## Comparing
- **Use `compare_to` for two files,** and report the measured differences.
- **Louder seems better.** Compare tone at matched loudness: say how far apart the files are in LUFS before you say which is brighter or fuller.
- **Stereo, only when measured.** When a result has stereo correlation and width, report them as measured. When it has none (a mono file or a mono sum), say nothing about width or placement.
- **Short one-shots give few frames.** Say when a measurement rests on very little audio.

## Reporting
- **Name what was measured, the numbers and what they suggest:** "The replacement measures 410 Hz lower in spectral centroid and 1.2 LU quieter, which fits a darker snare."
- **Never say a file "sounds" a way** unless the producer said it.
- **Measuring a file changes nothing in the project.** Changes go through `kumi_change` (skill `kumi:daw`).
- **An `interpretation` is a model's reading of the measurements,** written from the numbers and a coarse band-energy image (its `inputs` say which), never from the audio. It appears only when the producer turned on "Interpret measurements with a model". Call it that ("the listening model reads these numbers as…"), keep the measurements first, and never present it as a listen. An `interpretation_error` means there's no reading this time; report the measurements alone.
- **To share the measured file,** put the result's `MEDIA:` line in your reply, on a line of its own. In a messaging chat (Telegram, Discord…) only a `MEDIA:` line in your answer reaches the producer; one inside a tool result doesn't.

## Listening to a track or the mix
- **Only in the DAWs `kumi_listen`'s description names.** Kumi's runtime says which it can listen to, and a version may name none. Elsewhere, Kumi refuses a track or mix listen before anything plays: say so, and offer to measure a file instead (a bounce of the part, or the sample it plays).
- **It plays the project.** `kumi_listen` with `source: {"track": {"ref": "…"} or {"mix": true}, "from_beat", "beats"}`, plus the project key and epoch from `kumi_observe`, plays exactly that window (60 s at most) while the DAW is stopped, and measures it. The producer approves it first, in the runtime's own words.
- **In Live, a listen leaves traces Kumi can't remove.** Live may show the Set as modified, and Live's own Undo history holds the listen's temporary device steps, so pressing Undo in Live can bring Kumi Ears back. The approval says so; say it again if the producer asks what changed. A listen isn't a change in Kumi's HISTORY.
- **A listen that didn't measure** (`refused`, `cancelled` or `unsure`) has no measurements. Say what happened; after `unsure`, observe again (`kumi_observe`) before anything else, and never send it again.
