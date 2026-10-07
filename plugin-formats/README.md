# Plug-in formats

Kumi reads third-party plug-ins' presets and saved state itself, and keeps what it knows about each format here
as data. The Rust readers are in `crates/kumi-runtime/src/plugins/formats/`; they and their tests run off these
files, and Kumi carries them in its binary.

```
plugin-formats/
  serum-2/
    plugin.json          vendor, plug-in IDs (VST3, VST2, AU, CLAP), preset extensions, folders
    format-1/            a new one only when the bytes change
      structure.json     the wrapping, and every field: type, unit, range, option meanings, linked parameters
      verified.json      the plug-in versions read and written against this format
      fixtures/          files Kumi made for tests, from the plug-in's init patch; never vendor presets
  vital/
  ozone-12/              the main plug-in and its module plug-ins share one format
```

- **Guessed or verified.** Every ID, folder, field meaning and parameter link says which. Verified means checked
  against the plug-in itself: by changing one thing and reading what moved, or (Vital) from its published source.
- **Writes follow `verified.json`.** Kumi writes a file only for a plug-in version listed under `writes`;
  otherwise it reads only, and changes things through the plug-in's parameters.
- **Plug-ins are known by their IDs**, not by the name a host shows.
- **No vendor content.** Factory presets, wavetables and samples stay on the machine they were installed on.
  The survey below reads them where they lie and keeps only counts, types, ranges and option-like values.
- **Learn from files and behaviour only.** Never disassemble or decompile a plug-in. Vital's source (GPL-3.0)
  is read to learn its format; none of its code is copied.

## Surveying a format

`plugin_format_survey` decodes every preset under the folders it's given and every state of that plug-in in
Live Sets (`.als`) and device presets (`.adv`). It checks each file's round trip, writes a report, and with
`--structure` folds what it found into `structure.json`. Hand-written meanings, units, ranges and statuses stay;
new fields arrive as guessed.

```sh
cargo run -p kumi-runtime --example plugin_format_survey -- serum-2 report.json \
  --structure plugin-formats/serum-2/format-1/structure.json \
  "~/Documents/Xfer/Serum 2 Presets/Presets" "~/Music/Ableton/Projects"
```

Rerun it when a plug-in updates and look at what moved.
