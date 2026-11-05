---
name: sounds
description: Find, audition and place sounds from the producer's own sample folders with Kumi's sound map - approved folders only, revision-scoped sound ids, placing as an approved change.
---
# The producer's sounds, with Kumi

Kumi builds a sound map from sample folders the producer approved. Similar sounds sit near each other, and every sound has an id.

## Folders
- **Kumi reads only the folders the producer added.**
  - `kumi_library` with `action: add_folder` and an absolute path adds one; the producer approves it.
  - Ask which folder to add. Never suggest scanning a whole disk or home folder.
- **`action: scan` builds or refreshes the map.**
  - The producer approves it, and it runs as a job: follow it with `kumi_job`.
  - `action: status` says what's there.

## Finding sounds
- **`kumi_sounds`** searches by words (`query`), by `kind` (kick, snare, pad, loop…), or `near` another sound's id.
- **Sound ids belong to one map revision** (`map_revision`). After a rescan, search again rather than reuse old ids: an old id is refused as stale.
- **Describe sounds from what Kumi returns:** name, kind, folder, length and measurements. Don't invent qualities you didn't measure.

## Auditioning
- **`kumi_listen`** with `source: {"sound_id", "map_revision"}` measures a sound (skill `kumi:listening`), and with `compare_to` compares two.
- **In Hermes Desktop the producer can click sounds on the map** to hear them locally. That plays nothing in the DAW.
- **To let the producer hear a sound in a messaging chat** (Telegram, Discord…), put the `MEDIA:` line `kumi_listen` returned for it in your reply, on a line of its own. A `MEDIA:` line inside a tool result isn't delivered there.

## Placing a sound
- **Placing is a change:** `kumi_change` with an `import_audio` step.
  - Its `source` is the sound (`{"sound_id", "map_revision"}`) or an earlier Kumi artifact (`{"artifact_id"}`).
  - Its target is a `track_ref` from your latest observe, or a `new_track`.
  - To swap the sample in an existing sampler (Live's Simpler on a drum pad), use that DAW's replace step: `kumi_read` with `action: schema` says which steps exist.
- **The producer approves the exact placement,** and HISTORY lets them undo it.
- **When a producer picks a sound on the map,** Desktop puts a request in the chat ("Use sound s-… on the selected track"). Treat it like any other request: observe, then make an approved change.
