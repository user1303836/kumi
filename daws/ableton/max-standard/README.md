# Max standard

What Kumi holds Max for Live patchers to, as data. The checker is in `crates/kumi-runtime/src/devices/patch/`; it and
its tests run off these files, and Kumi carries them in its binary.

```
max-standard/
  standard.json   each rule's level (error, warn, advice, off) and what measuring well-made devices found for it
  objects.json    what the rules know of Max's objects: aliases, hot inlets, objects that defer, names copies share
```

- **The model never reads these.** It hears a broken rule in one line, with the rule's id and the fix, every broken
  rule at once.
- **A level changes here, a rule's logic in the checker.** A rule not listed is off.
- **Measured, not copied.** The survey reads devices where they lie, frozen ones with their own files, and keeps only
  counts: how often each rule was broken in how many things it looked at, how patchers are laid out, which of Max's
  objects they use. No device's names, text or code leave the machine.
- **Exceptions only.** `objects.json` lists what differs from Max's conventions (a class not listed has only its left
  inlet hot).

## Survey

```
cargo run -p kumi-runtime --example max_patch_survey -- report.json --into max-standard/standard.json <device or folder>...
```

- `--into` replaces the measured counts, it doesn't add to them: survey every device measured so far at once.
- `--findings` also prints each device's broken rules, to read them; they never go into the report.
