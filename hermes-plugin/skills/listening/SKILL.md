---
name: listening
description: What Kumi's audio measurements mean and how to report them honestly - loudness, true peak, spectral balance, comparisons at matched loudness, mono limits; measured, never heard.
---
# Measuring audio with Kumi

`kumi_listen` measures an audio file on this computer and, in the DAWs its description names, a track or the mix of the open project over a beat window (that plays the window in the DAW while it's stopped: say what will play in one line first; it's asked when `kumi_status` says `ask_before_changes: true`). Nothing is heard, by Kumi or by you: report measurements as measurements.

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
- **Several candidates against one reference:** the first `kumi_listen` with `compare_to` returns `compare.artifact_id`, Kumi's kept copy of the reference. Pass `{"artifact_id": …}` as `compare_to` for the next candidates: every candidate is measured against the same reference audio, even if the original moves or the map changes.
- **Which files Kumi can measure:** a file inside one of the producer's approved sample folders or this profile's media, a map sound, or a file Kumi made. A mixdown exported anywhere else is refused: ask the producer to export it into an approved folder, or to add its folder (`kumi_library` `add_folder`).
- **How far from a loudness target** (−14 LUFS for streaming, or the producer's figure): measure the file, then report its integrated LUFS against the target, and its true peak against −1 dBTP.
  - This recipe only measures: say what's off, and by how much. To bring the mix to a reference, see "Mastering against a reference" below.
  - Where a level change is the fix, offer one, as a `kumi_change`.
  - Measuring the same file again can't show that change: it measures as before. To check it, the producer exports or bounces again into an approved folder, and you measure the new file. Otherwise say Kumi can't check the result itself.


## Reference excerpts and several references
- **Connect first; only use fields the connected runtime advertises.** `compare_to` can select one reference. When offered, `references` selects 1–5 approved file, sound or artifact objects instead, never together with `compare_to`. A reference is never a track, mix, URL or output path. Do not repeat the same source/window to weight it twice.
- **Choose like-for-like sections with the producer.** Put paired `from` and `seconds` only on each reference object: for example `compare_to: {"file": …, "from": 42, "seconds": 20}`. Both must be finite, `from` ≥ 0 and 0 < `seconds` ≤ 60, wholly within the file. A past-EOF window is refused, never padded or shortened. The source is unchanged; a source capture uses separate `from_beat`/`beats`, not these file seconds. A file source must already be the intended bounce/section.
- **Names and windows matter.** Use the approved file's identity and returned `name`, `identity` and exact `window` (sample rate, channels, `start_frame`, `frame_count`), not an artifact's generated filename or `metrics.seconds`. That duration may still describe the original file; rounded `analyzed` clock labels are not exact sample bounds. Approval lists the reference file excerpts separately from source capture beats.
- **One preparation allowance, not one per reference.** Every reference is authorized and measured before DAW preflight; a bad last reference means no capture. The set shares the existing preparation cap and one unchanged native capture deadline. A failure or cancellation returns no partial new reference artifacts; do not retry a capture with an unsure outcome.
- **A profile is a measured target, not an averaged recording.** `references` returns each reference's canonical `metrics` and separate per-member `profile_quantities`, plus the combined `reference_profile`. Member Profile values and combined source/target/deviations use real Heard/Profile, with explicit `basis` and `unit`. Canonical Analysis and Profile are different meters even for similarly named LUFS, crest, centroid or width. Never subtract canonical `metrics` from a Profile target or relabel low-band side/mid dB as full-band width.
- **Equal weighting and honest spreads.** Each original usable reference has equal weight, not duration weight; combine the original profiles once, never recursively combine targets. Report `target.mid`, `low`/`high`, source-minus-mid `deviation` and `inside_spread` per quantity, not a scalar "better". With fewer than five references the range covers member centers plus minimum tolerances; short-frame region tolerances are fixed values. Neither is a confidence interval or purely observed variation.
- **Null is not zero.** Report `source_reason`, `target_reason`, per-reference `availability`, `usable_references` and each quantity's contributor/reference counts. Short audio can lack LUFS/LRA; mono lacks stereo evidence; silence/floor-only energy is no tonal target. Never silently average fewer references or invent missing values.
- **Sample rate limits coverage per quantity.** Above-Nyquist or only partly covered frequency regions have null member `profile_quantities` with `reason_code` and numeric `nyquist_hz`, not meaningful floor/zero values. They do not contribute to that region's target or count. The source likewise carries `source_reason`/`source_nyquist_hz` when unavailable. Preserve the other usable quantities from a low-rate reference; do not refuse it globally, invent missing high bands, or substitute its canonical Analysis bands for Profile values.
- **No Profile true-peak target.** Keep each reference's canonical peak and the producer's explicit ceiling as separate measurements/constraints. A reference that clips never excuses clipping the candidate.
- **Retain and replay the measured excerpt at zero.** Reuse each returned reference `artifact_id` without the original `from`/`seconds`: the kept float32 excerpt starts at sample zero and contains the samples measured. `origin` records the original approved source/window separately; it is not a new path permission and never a second playback offset. A new source, excerpt or reference set is a new comparison identity, not the previous round.
- **Hermes schema availability has a frozen-session limit.** With Tool Search on (the default), tool descriptions follow the live profile/process runtime. With Tool Search off, a session keeps its starting schema until a new session or compaction. A stale schema is not permission: both the hook and handler refuse unadvertised fields before any native call. No runtime still means connect first.

## Reporting
- **Name what was measured, the numbers and what they suggest:** "The replacement measures 410 Hz lower in spectral centroid and 1.2 LU quieter, which fits a darker snare."
- **Never say a file "sounds" a way** unless the producer said it.
- **Measuring a file changes nothing in the project.** Changes go through `kumi_change` (skill `kumi:daw`).
- **An `interpretation` is a model's reading of the measurements,** written from the numbers and a coarse band-energy image (its `inputs` say which), never from the audio. It appears only when the producer turned on "Interpret measurements with a model". Call it that ("the listening model reads these numbers as…"), keep the measurements first, and never present it as a listen. An `interpretation_error` means there's no reading this time; report the measurements alone.
- **To share the measured file,** put the result's `MEDIA:` line in your reply, on a line of its own. In a messaging chat (Telegram, Discord…) only a `MEDIA:` line in your answer reaches the producer; one inside a tool result doesn't.

## Listening to a track or the mix
- **Only in the DAWs `kumi_listen`'s description names.** Kumi's runtime says which it can listen to, and a version may name none. Elsewhere, Kumi refuses a track or mix listen before anything plays: say so, and offer to measure a file instead (a bounce of the part, or the sample it plays).
- **It plays the project.** `kumi_listen` with `source: {"track": {"ref": "…"} or {"mix": true}, "from_beat", "beats"}`, plus the project key and epoch from `kumi_observe`, plays exactly that window (60 s at most) while the DAW is stopped, and measures it. By default it runs without a prompt: say in one line what will play first. With `ask_before_changes: true` the producer approves it first, in the runtime's own words.
- **In Live, a listen leaves traces Kumi can't remove.** Live may show the Set as modified, and Live's own Undo history holds the listen's temporary device steps, so pressing Undo in Live can bring Kumi Ears back. Kumi's answer says so; say it again if the producer asks what changed. A listen isn't a change in Kumi's HISTORY.
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
  - A tone round needs an agreed loudness match first. A gain correction is an ordinary candidate, counted like the rest.
  - Without matched levels, report that limitation, not a tonal winner.
- **The baseline:** observe, then capture it with `kumi_listen` as usual, and keep its `artifact_id`.
  - A refused, cancelled or unsure listen isn't a baseline, and neither is one without an artifact.
  - Every candidate uses the same source, the same window, and that `artifact_id` as `compare_to`.
  - For a fader or level change, measure the mix: some DAWs capture a track before its fader, where the capture can't show the change.
  - In Live, where `kumi_listen` captures the mix, it hears the end of Main's chain: after Main's devices (its Limiter too), before Main's fader. A change to Main's fader isn't in that capture: never report one as measured there.
- **One candidate at a time.** Apply it with `kumi_change` like any change, and keep every `history_id` it returns: together they are this candidate. Never batch candidates, or ask for several at once.
- **Your own change moves the epoch.** A done change returns the new epoch: adopt that one only, then observe again and check that it's the same project and source. Every capture and every undo is asked as usual when `ask_before_changes` is true.
- **Measure each candidate the same way.**
  - Capture with the same window and `compare_to`, then check the window, length and state that come back.
  - Report each difference with its direction: the candidate minus the reference.
  - An interpretation is the model's reading of the numbers, not a listen. Never say a candidate sounds better.
- **Rejecting a candidate is undoing it exactly.**
  - `kumi_undo` each of its HISTORY ids, newest first. Confirm they're undone before the next candidate.
  - Never use an inverse change, the DAW's own Undo, or anything forced.
  - A candidate that can't be undone stays unresolved: say so. An undo that comes back `kept` means the producer changed it in the DAW since: that's a stop too.
- **Stop** on any of these: `stale`, `changed_since`, an undo that comes back `kept`, `unsure`, a lost answer, a denial, a project or epoch change that isn't your own, or a tempo change.
  - Don't work around it, and don't take a new baseline yourself. Tell the producer what's left and ask.
  - A changed tempo, or someone else's change, needs a new baseline, and a new round.
- **End** at the agreed count or when the goal is met, with one line: the chosen candidate's HISTORY ids (or none), any left unresolved, and what each candidate measured. Choosing a candidate isn't saving the project.

## Mastering against a reference
"Master my mix like this reference" (or "these references") is a round of the kind above, on the master track, with a hard cap: **three rounds at most**.
- **Agree first:**
  - the approved reference file and matching section, or a set of 1–5 approved references when `references` is offered;
  - one window of the mix, its loudest part, 60 s at most;
  - the target: the single reference's measurements, or the set's per-quantity Profile target/spread; keep each canonical true peak and the producer's ceiling separate, never invent a combined peak target.
- **The extents differ.** Without an explicit reference excerpt, `kumi_listen` measures up to the reference file's first 12 minutes, versus only the captured/bounced mix section. When excerpt fields are offered, choose the matching reference section with `from`/`seconds`; otherwise use an already cut reference. Report exact returned `window` provenance where supplied, else the rounded `analyzed` span; never take a window's loudness range for the whole song's.
- **Measuring the mix:** by capture where `kumi_listen` offers the mix in this DAW. Otherwise, and that's every DAW until its description says so, by bounce:
  - the producer bounces the same range, after each round, into an approved folder (`kumi_library` `add_folder` if needed);
  - Kumi can't tell a file from before a round from one after it, so the producer confirms which round each file is;
  - measure it as `{"file": …}`, reusing the chosen reference artifact or reference set as below;
  - window, epoch and length checks are for captures; they don't apply to a file.
  - A bounce is never a retry after a capture that was refused, cancelled or unsure.
- **The baseline:** measure the mix (captured over the window, or the first bounce) with `compare_to` for one reference or `references` for the agreed set. For one reference, report the canonical gaps:
  - integrated LUFS;
  - true peak;
  - the spectral balance (band levels and centroid);
  - the stereo width and correlation;
  - the dynamics: the crest factor, and the loudness range over what was measured.
  - For a set, report the returned Profile source/target/deviations and contributor counts instead; keep each reference's canonical measurements separate. Never calculate a gap between canonical Analysis and a Profile target.
- **A round is one plan on the master track, one change.**
  - **The master:** Live's main track as `kumi_observe` gives it; REAPER's `r:track:master`; Renoise's master track, as read.
  - **The processors the producer chose come first:** iZotope Ozone when the producer asks for it or it's already on the master as read, set by the parameter names Kumi reads on it, in its own units.
  - **Otherwise, native devices:**
    - EQ toward the reference's balance: Live `EQ Eight` (`audio_effects/EQ Eight`), REAPER `ReaEQ (Cockos)`, Renoise `EQ 10`;
    - gentle glue compression: Live `Glue Compressor` (`audio_effects/Glue Compressor`; where the Browser has none, `Compressor`), REAPER `ReaComp (Cockos)`, Renoise `Bus Compressor`;
    - a limiter: Live `Limiter` (`audio_effects/Limiter`), REAPER `ReaLimit (Cockos)`, Renoise `Maximizer`.
  - **Each limiter's gain and ceiling.** Use the parameter names the device shows when Kumi reads it, for native devices as for Ozone: names change across versions and modes.
    - Live `Limiter`: its gain control (`Input Gain` in Live 12.4; its Maximize mode has its own threshold and output controls), raised toward the reference's LUFS, with the ceiling under the true-peak target.
    - REAPER `ReaLimit`: lower `Threshold` for gain, with `Ceiling` a few tenths of a dB under the target.
    - Renoise `Maximizer`: raise `Boost` for gain, with `Ceiling` under the target. Renoise's master fader tops out at +3 dB.
  - **In one plan, name each device as you load it, then set it:** load the EQ (`as: "eq"`), set `@eq`, then the compressor, then the limiter. When you can't be sure of a device's parameter names in this version, load the devices in one change, read them, and set them in the next. Later rounds read the devices again and change only their settings.
  - **REAPER:** the master track's own FX chain, enabled, never its monitoring FX (monitoring FX aren't rendered).
  - **Live:** where `kumi_listen` captures the mix, a round works on Main's devices and never moves Main's fader to reach a target: the capture is taken before that fader, so it wouldn't show the move.
- **−1 dBTP is a measured target, not a knob value.** Set the limiter's ceiling with a margin under it, then let the next measurement show the true peak. Never report a ceiling setting as the result.
- **After each round, measure the mix again the same way** (the same window, or the producer's next bounce), reusing `compare.artifact_id` as `compare_to`, or each `references[].artifact_id` as the same reference set. Never reapply their original file offsets. Report what moved on the same measurement basis.
  - **Rounds add up.** A round that made things better stays applied, and the next one builds on it. Keep the HISTORY ids of every round still applied.
  - **A round that made nothing better ends the rounds:** stop there, without another try.
  - **To take back a worse round,** `kumi_undo` the HISTORY ids its plan returned (one for each kind of change in it), newest first. That returns the mix to the last round that stays. Stop at the first undo that doesn't come back undone.
  - **Stop when the gaps are within what the producer agreed,** or after the third round. Any more rounds are the producer's new decision.
- **The rounds section's stops apply here too,** and so does a plan that came back `partial`, `refused` or `unsure`.
- **End with the measurements, not a match:**
  - the gaps that remain;
  - the HISTORY ids of every round still applied;
  - why the rounds stopped.

  Say that the gaps are measured, not heard. Never say the mix matches the reference when the rounds stopped at the cap or stopped improving. A limiter can't fix a mix: say so when the gaps are in the mix itself, such as a harsh band or a narrow image.
