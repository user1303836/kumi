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

## Rounds: comparing candidate changes
- **Only where `kumi_listen` captures a track or the mix** (its description names those DAWs). Elsewhere, compare finished audio files: bounces into an approved folder.
- **Agree first:**
  - a measurable goal;
  - the source (a track or the mix) and one exact `from_beat`/`beats` window;
  - the reference;
  - how many candidates: four, unless the producer says otherwise. Never add one yourself; ask before another round.
- **Level or tone, said up front.**
  - A level round reports the loudness difference itself. Never match it away.
  - A tone round needs an agreed loudness match first. A gain correction is an ordinary approved candidate, counted like the rest.
  - Without matched levels, report that limitation, not a tonal winner.
- **The baseline:** observe, then capture it through the normal `kumi_listen` approval, and keep its `artifact_id`.
  - A refused, cancelled or unsure listen isn't a baseline, and neither is one without an artifact.
  - Every candidate uses the same source, the same window, and that `artifact_id` as `compare_to`.
  - For a fader or level change, measure the mix: a track's capture can't show its fader's effect.
- **One candidate at a time.** Apply it with `kumi_change`, approved like any change, and keep every `history_id` it returns: together they are this candidate. Never batch candidates, or ask for several at once.
- **Your own change moves the epoch.** A done change returns the new epoch: adopt that one only, then observe again and check that it's the same project and source. Every capture and every undo is approved as usual.
- **Measure each candidate the same way.**
  - Capture with the same window and `compare_to`, then check the window, length and state that come back.
  - Report each difference with its direction: the candidate minus the reference.
  - An interpretation is the model's reading of the numbers, not a listen. Never say a candidate sounds better.
- **Rejecting a candidate is undoing it exactly.**
  - `kumi_undo` each of its HISTORY ids, newest first, each one approved. Confirm they're undone before the next candidate.
  - Never use an inverse change, the DAW's own Undo, or anything forced.
  - A candidate that can't be undone stays unresolved: say so. An undo that comes back `kept` means the producer changed it in the DAW since: that's a stop too.
- **Stop** on any of these: `stale`, `changed_since`, an undo that comes back `kept`, `unsure`, a lost answer, a denial, a project or epoch change that isn't your own, or a tempo change.
  - Don't work around it, and don't take a new baseline yourself. Tell the producer what's left and ask.
  - A changed tempo, or someone else's change, needs a new baseline the producer approves, and a new round.
- **End** at the agreed count or when the goal is met, with one line: the chosen candidate's HISTORY ids (or none), any left unresolved, and what each candidate measured. Choosing a candidate isn't saving the project.

## Mastering against a reference
"Master my mix like this reference" is a round of the kind above, on the master track, with a hard cap: **three rounds at most**.
- **Agree first:**
  - the reference file (in an approved folder, a map sound, or a file Kumi made);
  - one window of the mix, its loudest part, 60 s at most;
  - the target: the reference's loudness and true peak, its tonal balance, its stereo width.
- **The extents differ.** `kumi_listen` measures a reference file's first 12 minutes, and the mix only over its window. So prefer a reference already cut to the matching section. Otherwise say what each measurement covers, and never take the window's loudness range for the whole song's.
- **Where `kumi_listen` can't capture the mix in this DAW,** use a bounce of the mix in an approved folder instead, and say each round needs a new bounce. A bounce is never a retry after a capture that was refused, cancelled or unsure.
- **The baseline:** capture the mix over that window with `compare_to` the reference. Report the gaps:
  - integrated LUFS;
  - true peak;
  - the spectral balance (band levels and centroid);
  - the stereo width and correlation;
  - the dynamics (loudness range, over what was measured).
- **A round is one plan on the master track, approved once.**
  - **The processors the producer chose come first:** iZotope Ozone when it's installed and wanted, set by its parameter names in its own units.
  - **Otherwise, native devices:**
    - EQ toward the reference's balance: Live `EQ Eight`, REAPER `ReaEQ`, Renoise `EQ 10`;
    - gentle glue compression: Live `Glue Compressor`, REAPER `ReaComp`, Renoise `Bus Compressor`;
    - a limiter with its gain toward the reference's LUFS: Live `Limiter`, REAPER `ReaLimit`, Renoise `Maximizer`.
  - **In one plan, name each device as you load it, then set it:** load the EQ (`as: "eq"`), set `@eq`, then the compressor, then the limiter. Later rounds read the devices again and change only their settings.
  - **REAPER:** the master track's own FX chain, enabled, never its monitoring FX (monitoring FX aren't rendered).
- **−1 dBTP is a measured target, not a knob value.** Set the limiter's ceiling with a margin under it, then let the next capture show the true peak. Never report a ceiling setting as the result.
- **After each round, capture the same window again,** with `compare_to` the reference's kept copy (`compare.artifact_id`), and report what moved.
  - **A round that made nothing better ends the rounds:** stop there, without another try.
  - **To take back a worse round,** `kumi_undo` each of its HISTORY ids, newest first, each approved. Stop at the first that isn't undone.
  - **Stop when the gaps are within what the producer agreed,** or after the third round. Any more rounds are the producer's new decision.
- **The rounds section's stops apply here too,** and so does a plan that came back `partial`, `refused` or `unsure`.
- **Say what this is:** the numbers match the reference's, measured, not heard. A limiter can't fix a mix: say so when the gaps are in the mix itself, such as a harsh band or a narrow image.
