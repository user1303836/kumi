# Audio intelligence

English · [简体中文](../zh-CN/AUDIO_INTELLIGENCE.md) · [日本語](../ja/AUDIO_INTELLIGENCE.md)

The bridge's audio tools for MCP clients. They measure audio you send, compare
it with a reference, relate the measurements to a track in Live, and record a
short capture from Live to measure and then delete.

**Kumi doesn't use these tools.** Kumi has its own listening, on your computer:
it reads audio files and the Set's audio clips, records or renders what it
built (its `audition` tool records each candidate onto a scratch track with
Main silenced; `render` renders an audio track's clips through Kumi's Live
extension), and measures the result. See [listening](KUMI_GUIDE.md#listening)
in the Kumi guide and [how Kumi changes your Set](KUMI_CHANGES.md).

## The tools

| Tool | Takes | Gives |
| --- | --- | --- |
| `audio_analyze` | Interleaved little-endian float32 PCM in base64, normalized to −1…1: 8–384 kHz, 1–32 channels, up to 10,000,000 samples and 600 seconds | `pcm-analysis/v3`: loudness, peaks, spectrum, dynamics, transients, clipping |
| `audio_compare_reference` | Two such sources: 32–96 kHz, mono or stereo, up to 30 seconds each and 4,000,000 samples together | `reference-analysis/v2`: both analyses, alignment, and the differences |
| `audio_diagnose_live_context` | Mono or stereo PCM, as for `audio_analyze`, plus one `trackRef` | The analysis, with findings linked to the track, its devices and routing |
| `live_audio_capture_preview/apply`, `live_audio_capture_status`, `live_audio_capture_emergency_stop` | A Session clip, an empty audio slot and 1–9 seconds | An analysis of Live's Resampling input; the recording is deleted afterwards |

None of them accepts a file path or a URL, and none returns audio. PCM you send
is never treated as Live's output: a link to Live is always marked as yours to
vouch for, except for a capture the bridge made itself.

## Measurements

`standardsAudio` in `pcm-analysis/v3` follows the published standards:

- ITU-R BS.1770-5 programme loudness, with EBU R128 operating practice;
- EBU Tech 3341 momentary (400 ms) and short-term (3 s) loudness;
- EBU Tech 3342 loudness range: −70 LUFS absolute gate, −20 LU relative gate;
- integrated loudness from 400 ms blocks every 100 ms, gated at −70 LUFS and
  −10 LU;
- channel weights: mono and stereo are inferred, larger layouts need the labels
  `M`, `L`, `R`, `C`, `Ls`, `Rs` and `LFE`; LFE is left out and surrounds weigh
  1.41;
- sample peak and true peak, reported separately.

True peak uses BS.1770-5 Annex 2's four-phase, order-48 filter at 48 kHz, and a
64-tap Blackman-windowed sinc conversion to 48 kHz first at 44.1 kHz. At other
rates true peak is reported unavailable. Loudness works at any integer rate from
8 to 384 kHz, with the published filter coefficients at 48 kHz.

Silence, audio that's too short, an unknown multichannel layout and input past
the true-peak work bound give explicit unavailable values, never NaN, infinity
or a made-up figure. The momentary and short-term series hold at most 128
points; gating still uses every window.

`clipping` counts source samples at full scale. `reconstructedOvers` counts
values above 0 dBFS that appear only after reconstruction; they don't prove the
source clips. The `loudness` field is an older RMS estimate kept for
compatibility: use `standardsAudio` for delivery and mastering decisions.

## Reference comparison

`audio_compare_reference`:

1. converts each source to 48 kHz with a 32-tap Blackman-windowed sinc kernel;
2. aligns them with a coarse search at 100 Hz, then a fine search at 1 kHz
   within ±10 ms (`alignment.mode` `auto`, the default; or `manual` with an
   offset, or `disabled`). `maxLagSeconds` defaults to 5 and goes up to 10;
3. refuses a weak, silent or ambiguous automatic match: each source is still
   analyzed, but the overlap is zero and every difference is withheld;
4. otherwise analyzes only the overlap, and reports the difference in
   integrated loudness, a level match suggestion within ±24 dB, and the
   differences in true and sample peak, RMS, crest, dynamic range, spectrum and
   transient density.

`resampling.*.sourceClipping` counts full-scale samples in each source before
conversion. Resampling doesn't stretch time or match tempo, and a suggested
level match changes no audio. No aligned audio is returned.

## Diagnosis

`audio_diagnose_live_context` analyzes your PCM, reads one fresh snapshot of a
track (the Set, the track, its mixer and routing, its devices in order and
their parameter values) and links the measurements to it. The link is marked as declared by you and
unverified; after a capture, it's marked `verified-by-capture-lifecycle`.

Findings keep measurements apart from hypotheses. A device on the track is
never called the cause (`causality.claimed` is always false), and what can't be
known is named: latency, sidechains, hidden parameters, gain reduction, and
where inside a device the signal was. A suggested mixer change is a reversible
experiment to try and capture again, not a promised dB correction.

## Workers

Analysis never runs on the host's own event loop. Each job runs in a
throwaway worker process, `ableton-mcp-analysis-worker`:

| What | Limit |
| --- | --- |
| Jobs | 2 at once, 4 waiting |
| Time | 30 seconds |
| Request | 64 MiB |
| Output | 2 MiB result, 16 KiB of error text |

The worker inherits no secrets and gets only the results back as JSON.
Cancelling the MCP request or running out of time kills it at once.

## Live capture

Live's scripting gives no access to its audio, so the bridge records instead:
it plays one Session clip into Live's Resampling input, records it into an
empty audio slot, measures the file and deletes it. It runs only on real Live,
when the Remote Script offers `audio.capture.resampling` and all six
`audio.capture.*` operations. In the deployment policy, the preview, apply and
emergency stop are in the `capture` class (only the `full` profile includes
it); `live_audio_capture_status` is a read.

### Preview

`live_audio_capture_preview` needs:

- the exact name of the open Set, which must be saved;
- one Session clip to play and a different, empty audio slot to record into,
  on another track;
- the destination's input routing available now, so it can be put back. If
  Live shows a stale `Ext. In`, choose `No Input` with `live_routing_*` first;
- the transport stopped, nothing recording or playing, every track unarmed and
  no track monitoring its input;
- `durationSeconds` from 1 to 9;
- `consent: "ephemeral-analysis-and-delete"`;
- `outputSafety` is optional.

The preview expires after 60 seconds.

### Apply

`live_audio_capture_apply` takes the preview's unpredictable confirmation and
an idempotency key. On Live's main thread the Remote Script checks the source
and destination again, briefly renames the destination track with a private
tag, sets its input to Resampling, its monitoring off and its arm on, sets
launch quantization to none, and fires both slots. It owns a new clip only if
Live marks it recording and the name carries the tag; then it puts the track's
name and the launch quantization back. It never retries a start.

A watchdog in Live stops the capture after at most 10 seconds (the requested
length plus 3 seconds). Stopping it, cancelling, the watchdog, emergency stop
and shutdown all stop both slots, the transport and recording, and put the
playhead, track name, routing, arm and monitoring back. A change someone else
made in between is reported, not overwritten.

### The recording

The host accepts only a fresh, regular, single-link WAV inside the saved Set's
project folder or the User Library's `Samples/Recorded`: at most 32 MiB, 12
seconds and two channels, as 16-, 24- or 32-bit PCM or 32-bit float. It fences
the file's identity, size, time and SHA-256 while reading it.

After the analysis, it opens the WAV and its `.asd` without following links,
checks their identity again, moves them into a private quarantine folder on the
same disk, empties and deletes them, and only then deletes the clip in Live.
It then checks that no media and no quarantine file is left. "Deleted" means
unlinked; it doesn't promise the data is gone from an SSD or a copy-on-write
disk. A successful result shows a stopped, non-recording Live, the destination
restored, an empty slot, capture state `cleaned` and no files left.

The result has formats, rates, durations and the analysis, but no path, digest,
audio, token or confirmation.

If Live or the Remote Script shuts down mid-capture, the Remote Script stops
and restores what it can but can't delete files itself: it leaves the clip and
its file for the host, or you, to clean up.

### Recovery

`live_audio_capture_status` shows the capture's state, its source and
destination, whether playback stopped, and the file's availability, without
the path or the recovery token. It works from a new host process.

If the state isn't `cleaned`, call `live_audio_capture_emergency_stop` with
`confirmation: "emergency-stop-and-clean"` and the identities the status
showed. It stops the capture, checks and deletes the file as above, and only
then deletes the clip. If the path, identity, format or cleanup can't be
confirmed, it reports what's left and deletes nothing it can't vouch for.
Don't start another capture while anything is left.

## Checking the analysis

The standards analysis is checked against FFmpeg's independent `ebur128`
filter with generated audio (no third-party recordings): the bridge's tests
(`crates/ableton-mcp-server/tests/audio_standards.rs`) hold it to FFmpeg's
results for those signals, from the tracked report
[phase-8-audio-oracle.json](../evidence/phase-8-audio-oracle.json)
(2026-07-27). The tolerance is 0.1 LU or dB at 48 kHz and 0.15 dBTP for
44.1 kHz true peak. The published standards stay the definition; FFmpeg is a
cross-check.

On real Live, Kumi's opt-in acceptance run (`accept_live`; see
[testing](TESTING.md)) plays, bounces and listens. The last tracked run of the
earlier capture verifier,
[phase-8-audio-live.json](../evidence/phase-8-audio-live.json), was on Live
12.4.5b8 on macOS (2026-07-27, bridge 0.1.0) and predates the current bridge.
No Windows run is tracked.

## Limits

- True peak only at 44.1 and 48 kHz.
- Conventional channel labels only; no immersive or object-based layouts.
- Capture needs a saved Set, a WAV recording and a routing that can be put
  back.
- No Max for Live tap, no plug-in meter, no file paths or URLs, no time
  stretching, and no mastering grade or compliance verdict.
- Deleting a file isn't secure erasure. A program running as your user can read
  the bridge's secret and is outside what the bridge protects against; the
  file checks guard against mistakes and swapped paths within that.
