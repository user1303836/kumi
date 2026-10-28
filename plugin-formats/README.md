# Plug-in formats

Kumi reads, creates and edits third-party plug-in presets locally. The Rust codecs are in
`crates/kumi-library/src/plugins/formats/`; `plugins/preset.rs` combines them into complete VST3 presets.
The `vst3` tool discovers installed products, searches editable fields, inspects presets and writes new files.
These workflows support **VST3 only**. Kumi carries the catalog and initialized states in its binary;
creating a supported preset needs no downloaded converter or running plug-in. The installed version must
match a verified writer; an unknown or newer version stays read-only until verified.

Kumi is independent: it is not affiliated with, associated with, sponsored by or endorsed by any plug-in
manufacturer, and has no relationship with them. Product names identify compatible software. All trademarks
belong to their owners.

## Capabilities and verification

| Products | Native format | VST3 creation, editing and reload evidence |
| --- | --- | --- |
| Serum 2 | Xfer JSON header, Zstandard CBOR; separate processor/controller documents | 2.1.5: oscillators, scalar fields, names and initialized Distortion insertion; translated state reloaded and read back |
| Ozone 12 suite and all 20 standalone modules | XML native presets; typed JSON in zlib component state | 12.1.0.1440: each exact class initialized, edited by Rust, loaded and read back; suite module ordering supported |
| All 10 captured Valhalla DSP products | XML in product-specific VST3 wrappers | Each product's recorded version; native XML conversion preserves its component wrapper and private data. ValhallaDelay BBD mode checked |
| All 16 captured Minimal Audio products | JUCE binary XML, physical `mapped_value` controls | Each product's recorded version; normalized host values and stored physical values remain distinct |
| All 13 captured Pulsar Modular products | JUCE binary XML, physical parameter values | Each product's recorded version; stale/unknown parameter IDs rejected |
| Zynaptiq PUNCH 1 and UNMIX DRUMS | Native XML and length-prefixed XML component state | 1.1.1 and 1.3: initialized states, Rust edits, native reload and parameter readback |
| Superior Drummer 3 | Native brace text and VST3 compatibility wrapper | 3.4.1: new initialized state and existing-state edits; installed sample libraries remain external |
| Other catalogued Zynaptiq products | Native XML; known fields and local preset schemas | Native reading and edits from a supplied template where the schema is known. VST3 state creation remains unavailable where the installed binary could not initialize or class/state identity was not verified |
| Weiss Compressor Limiter, DS1-MK3, MM-1, Deess; Tape and Tape Multi Track | Softube JSON bundles/presets with normalized parameter tuples | Native reading and edits from a supplied template. VST3 component writing remains unavailable: the local standalone host and Renoise rejected these installed binaries |

`plugin.json` lists every individual product, VST3 class, filename aliases and processing roles.
`verified.json` gives exact versions and the checks performed. Runtime readback proves the named controls
and serialization, not every audible DSP behavior. Unit and integration tests use committed initialized or
synthetic fixtures; they never require an installed plug-in, a running DAW or a factory library.

## Using the built-in tools

- `vst3 installed` scans standard VST3 directories, plus optional custom folders, without executing binaries.
  It reports roles, class identity when available and tested write versions. Presence does not mean the DAW
  can load or authorize the plug-in; filename identification is explicitly marked for confirmation.
- `vst3 catalog` searches parameter names and stored paths. `inspect` reads concrete fields from a file.
  `create` starts from initialized state; `edit` applies a batch to an existing native or VST3 preset.
  Output is a plain filename saved under Kumi's `presets/` folder; existing files are never overwritten.
  The actual installed version must be verified; a model-supplied version cannot override it. Source version
  markers are checked wherever the format stores them. Some component formats
  contain only a schema version: their source binary version cannot be inferred. Wrong classes and incompatible
  schemas are rejected. Unrelated chunks and opaque state are retained.
- Softube and the remaining documented Zynaptiq native formats can be edited or used as the source of a new
  native preset. Those outputs explicitly say that plugin reload has not been verified; they are not passed
  to a VST3 loader as if they were component state. No factory templates are bundled.
- In Live, `plugin prepare-preset` places the file in the User Library, then `load_device` loads the browser
  item with its existing HISTORY and undo. Live's API does not replace arbitrary state in an existing
  instance: use exposed parameters or load a new instance for comparison.
  The saved User Library preset is retained when undo removes its loaded device.
- In REAPER, `fx_preset` takes a `.vstpreset` path. In Renoise, `device_preset_data` loads an effect and
  `plugin_preset_data` loads an instrument. Both preserve before-state for undo and reject a different class.
- For mastering and reference mastering, `vst3 guide` gives a measure/change/listen workflow. Kumi checks
  Ozone 12 first, then other suitable installed VST3 processors, then native devices when suitable plug-ins
  cannot be used. Specific user choices take precedence. Loudness, true peak, tone, dynamics and width are
  checked before and after; generating a preset alone is not a completed master.

```
plugin-formats/
  serum-2/
    plugin.json          vendor, VST3 IDs, filename aliases, roles, preset extensions, folders
    format-1/            a new one only when the bytes change
      structure.json     the wrapping, and every field: type, unit, range, option meanings, linked parameters
      verified.json      the plug-in versions read and written against this format
      fixtures/          files Kumi made for tests, from the plug-in's init patch; never vendor presets
  vital/
  ozone-12/              the main plug-in and its module plug-ins share one format
```

- **Guessed or verified.** Every ID, folder, field meaning and parameter link says which. Verified means checked
  against the plug-in itself: by changing one thing and reading what moved, or (Vital) from its published source.
- **VST3 writes follow `verified.json`.** Kumi writes VST3 state only for a requested plug-in version listed
  under `writes`. Native-template edits report their identity checks and unverified reload status separately;
  host parameter changes remain available when a VST3 state writer is unavailable.
- **Plug-ins are known by their IDs**, not by the name a host shows.
- **No vendor content.** Factory presets, wavetables and samples stay on the machine they were installed on.
  The survey below reads them where they lie and keeps only counts, types, ranges and option-like values.
- **Evidence stays explicit.** Learn from files, behaviour and normal host APIs by default. Disassembling or
  decompiling a plug-in requires explicit repository-owner authorization. No manufacturer code or authorization
  workarounds are included. Vital's published source (GPL-3.0)
  is read to learn its format; none of its code is copied.

## Fixtures

Each plug-in's init patch, saved by the plug-in itself, and the state Live 12.4 saved for the same unedited
patch when the device was dragged into its browser (Live writes a VST3 device as a `.vstpreset`). Made on
2026-10-07 with Serum 2.1.5, Vital 1.0.7 and Ozone 12.1.0.

| Fixture | What it is |
| --- | --- |
| `serum-2/…/init.SerumPreset` | Serum 2's Init Preset, saved from its window |
| `serum-2/…/init.vstpreset` | Live's state of that patch: the processor and controller containers |
| `serum-2/…/init-distortion.vstpreset` | the same with a Distortion unit added to the main FX rack |
| `vital/…/init.vital` | Vital's initialized preset, exported from its window (its sample and wavetable are generated by Vital's code) |
| `vital/…/init.vstpreset` | Live's state of that patch from Vital's VST3 build, which wraps its VST2 chunk |
| `ozone-12/…/init.xml`, `init-maximizer.xml` | Ozone 12's and the Maximizer's default settings, saved as presets |
| `ozone-12/…/init.vstpreset`, `init-maximizer.vstpreset`, `init-exciter.vstpreset` | Live's states of those, and of Ozone 12 with an Exciter module added |

Ozone fixture UUIDs are zeroed, and preset-browser extra state that contained local user paths is cleared.
New presets receive a fresh random UUID; edits retain the source identity.

The additional initialized VST3 captures were made on 2026-10-27. Ozone's `fixtures/defaults/` holds one
state per class; each other verified product has its own initialized state. Blank SD3 state contains no
sample library. Runtime evidence and editable-field checks are recorded beside each product's structure.

The Serum 2 controller states name the preset's full path (`selectedPresetPath`), which held the machine's
user folder. That one string was replaced by one of the same length and the controller's body recompressed,
with its hash recomputed; the processor halves are as Live wrote them. The fixtures are kept byte for byte
(`.gitattributes`).

## Surveying a format

`plugin_format_survey` decodes every preset under the folders it's given and every state of that plug-in in
Live Sets (`.als`) and device presets (`.adv`). It checks each file's round trip, writes a report, and with
`--structure` folds what it found into `structure.json`. Hand-written meanings, units, ranges and statuses stay;
new fields arrive as guessed.

```sh
cargo run -p kumi-daw-ableton --example plugin_format_survey -- serum-2 report.json \
  --structure plugin-formats/serum-2/format-1/structure.json \
  "$HOME/Documents/Xfer/Serum 2 Presets/Presets" "$HOME/Music/Ableton/Projects" plugin-formats/serum-2/format-1/fixtures
```

Rerun it when a plug-in updates and look at what moved.
