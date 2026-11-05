---
name: generate
description: Paid sound effects and audio-to-MIDI with Mirelo through Kumi - only when the producer switched it on; quote first, then a create the producer approves from the quote; credits are an estimate and a reservation, never a price.
---
# Paid generation with Mirelo

Kumi can ask Mirelo for a sound effect from a prompt, or for MIDI notes from an audio file. It costs Mirelo API credits, so it runs only when the producer has switched it on in Kumi's settings in Hermes (Mirelo acknowledged, the kind enabled, a key, and a credit cap above 0). Otherwise `kumi_generate` says `not_enabled` or `not_configured`: tell the producer, and don't look for another way.

## Two steps, always
- **Quote first.** `kumi_generate` with `action: quote`:
  - a sound effect: `kind: sfx`, a `prompt` (what it should sound like) and, if you like, `seconds` (5 when left out);
  - MIDI notes: `kind: audio_to_midi` and a `source` (`{"artifact_id"}`, `{"sound_id", "map_revision"}`, or `{"file", "provenance": {"host_tool", "tool_call_id"}}`), never a path. Sending that file to Mirelo is approved by the producer before the quote.
- **Then create.** `action: create` with the `quote_id` the quote returned, and nothing else. The producer approves it from the quote itself: its prompt or file, its seconds, and how many credits it reserves. A quote is used once, and expires after a few minutes: quote again rather than reuse one.

## Credits, said plainly
- **Mirelo API credits, never money.** A quote's `estimate` is what Kumi reserves, not a price. Say "about N credits reserved", never a cost in currency.
- **A charge can stay unknown** (`credits.actual: null`) until Mirelo reports it. Never call it 0, and never say anything was refunded.
- **`ledger`** in a result is the producer's credit cap, what's spent, what's reserved and what's left, as of that result.
- **`frozen: true`** means new paid generations are paused, because a charge came in above its estimate. Only the producer can clear it, in a terminal (`hermes kumi credits unfreeze`). Tell them; don't try again.

## After a create
- **The generation is a job.** Follow it with `kumi_job` and its `job_id`. A finished one has its file as `artifact_id` (and a `MEDIA:` line for audio).
- **An `unsure` create is never created again:** whether Mirelo got it is unknown, and its credits stay reserved. Check the job with `kumi_job`, and tell the producer they can check Mirelo.
- **Placing a sound effect** is a change, as in skill `kumi:sounds`: the DAW's own file step, with `{"artifact_id"}`. MIDI notes are never placed through an audio file step; plan them as a separate, approved change after reading the project's tempo.
