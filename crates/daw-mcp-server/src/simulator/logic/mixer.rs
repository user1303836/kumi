//! (M4b) The simulated Logic's channel strips past their header's controls, as the bridge's second M4 slice makes them
//! over the real Logic (`crates/logic-kumi/src/changes`: `strips.rs`, `plugins.rs`, `params.rs`, `window.rs`,
//! `preset.rs`, `patch.rs`, `sends.rs`, `smart.rs`, `automation.rs`): the plug-ins in a strip's insert slots, each with
//! the rows its window's Controls View lists and the settings its next and previous buttons step through; the sends
//! (one to a bus nothing takes has Logic make an aux for it); a patch from the Library; the Smart Controls; and the
//! automation the Track Automation Event List lists. A strip is a track's or an output's: the Stereo Out is
//! `<epoch>:output:1`, its plug-ins `<epoch>:plugin:0.1.<slot>` and their parameters `<epoch>:param:0.1.<slot>.<n>`;
//! the auxes Logic makes for a bus have no ref. What the bridge's scenes recorded on Scratch is as they read it: Channel
//! EQ's Controls View (`change-fx-params`: its 46 rows, their ranges and texts) and its first setting's name
//! (`change-fx-preset`), the Gain's rows (`change-fx-output`), Keys' Piano and its Smart Controls (`scratch-m4b`), the
//! plug-ins the patch Classic Vocal loads (`change-patch-load`), Volume's automation number; the rest of the catalogue
//! (the settings' values, the other plug-ins' rows and settings, the Library's other patches and their Smart Controls,
//! the automation's other parameters) is the simulator's own.
//!
//! Logic keeps no step of its project's undo for a parameter, a setting, a Smart Control or a send's level (a
//! parameter and a setting are steps of the plug-in window's own undo); a plug-in inserted or removed ("Insert Plug-in
//! in Channel Strip", "Change Plug-in in Channel Strip", "Remove Plug-in in Channel Strip"), a send made or removed
//! ("Change Send in Channel Strip"), a patch loaded ("Load Patch") and automation events ("Create Event", "Value
//! Change", "Delete") are steps of it. Nothing here moves the epoch (M2's rule for plug-ins and sends).
use super::{
    alphabetical, bad, changed, db_text, failed, gone, page_of, quoted, uncertain, Answer, Fault, State, UndoStep, CLOSE_DB, MIN_DB,
    RECORDED_EPOCH,
};
use serde_json::{json, Value};

/// A strip's effect slots and send slots, as Logic's Mixer has them.
const MOST_EFFECTS: usize = 15;
const MOST_SENDS: usize = 8;
/// The busses Logic's send menu offers.
const MOST_BUS: u64 = 64;
/// The longest plug-in name Kumi types, the most parameters one change sets, and the most setting presses.
const NAME_LIMIT: usize = 120;
const MOST_PARAMS: usize = 64;
const MOST_PRESSES: usize = 64;
/// The most events one automation change takes.
const MOST_EVENTS: usize = 32;
/// A send's knob goes no higher.
const SEND_MAX_DB: f64 = 6.0;
/// How close a Smart Control comes to the value asked (a set moves it a hundredth).
const SMART_CLOSE: f64 = 0.0051;
/// What a plug-in window's setting pop-up names before any setting is loaded.
pub(super) const DEFAULT_PRESET: &str = "Default Preset";
/// Logic's names for its steps.
const INSERTED: &str = "Insert Plug-in in Channel Strip";
const CHANGED_SLOT: &str = "Change Plug-in in Channel Strip";
const REMOVED: &str = "Remove Plug-in in Channel Strip";
const SEND_STEP: &str = "Change Send in Channel Strip";
const PATCH_STEP: &str = "Load Patch";
/// What the bridge's answers say.
const DELETE_NOTE: &str = "Logic Pro's own undo puts it back with its settings while the deletion is its latest step; after that Kumi puts it back by name, with Logic Pro's default settings.";
const PARAM_NOTE: &str =
    "A parameter moved in the plug-in window is a step of that window's own undo, not of the project's: Kumi puts it back by value.";
const PATCH_NOTE: &str =
    "A patch replaces the track's channel strip; only Logic Pro's own undo brings the strip back, while the load is its latest step.";
const NEW_SEND_NOTE: &str = "Logic Pro makes a send at its own default level (−∞ on this Mac): give volumeDb to hear it.";

/// How a slider's text reads its raw value.
#[derive(Clone, Copy, Debug)]
enum Text {
    /// dB from `low`, `step` a raw unit, to a tenth: "-6.0 dB", "0.0 dB", "+3.0 dB".
    Db { low: f64, step: f64 },
    /// A number from `low`, `step` a raw unit, to `decimals` places, `unit` after it: "50%", "10 dB/s", "375 ms".
    Linear { low: f64, step: f64, decimals: usize, unit: &'static str },
    /// Channel EQ's frequency, along its curve: "75.0 Hz", "2500 Hz".
    Hz,
    /// Channel EQ's Q, along its curve: "0.71".
    Q,
}

/// The raw values Channel EQ's frequency and Q sliders read as, where the scene read them (between them, along the
/// logarithm; past the last, as the last).
const FREQUENCY: [(f64, f64); 8] =
    [(0.0, 20.0), (200.0, 75.0), (250.0, 100.0), (375.0, 250.0), (604.0, 1040.0), (725.0, 2500.0), (900.0, 7500.0), (1050.0, 20000.0)];
const Q_FACTOR: [(f64, f64); 8] =
    [(0.0, 0.1), (13.0, 0.2), (20.0, 0.3), (26.0, 0.41), (33.0, 0.6), (36.0, 0.71), (40.0, 1.0), (127.0, 100.0)];

/// A row of the Controls View: a slider (its raw range from 0, its text), a checkbox, radio buttons (the options), or a
/// pop-up menu Logic draws itself (its text alone).
#[derive(Clone, Copy, Debug)]
enum Control {
    Slider { max: f64, text: Text },
    Switch,
    Choice(&'static [&'static str]),
    Menu(&'static str),
}

/// A parameter as the Controls View lists it, and its raw value on a new plug-in.
#[derive(Clone, Copy, Debug)]
struct ParamSpec {
    name: &'static str,
    control: Control,
    default: f64,
}

const fn slider(name: &'static str, max: f64, text: Text, default: f64) -> ParamSpec {
    ParamSpec { name, control: Control::Slider { max, text }, default }
}

const fn switch(name: &'static str, on: bool) -> ParamSpec {
    ParamSpec { name, control: Control::Switch, default: if on { 1.0 } else { 0.0 } }
}

const fn choice(name: &'static str, options: &'static [&'static str], on: usize) -> ParamSpec {
    ParamSpec { name, control: Control::Choice(options), default: on as f64 }
}

const fn menu(name: &'static str, text: &'static str) -> ParamSpec {
    ParamSpec { name, control: Control::Menu(text), default: 0.0 }
}

const fn db(low: f64, step: f64) -> Text {
    Text::Db { low, step }
}

const fn linear(low: f64, step: f64, decimals: usize, unit: &'static str) -> Text {
    Text::Linear { low, step, decimals, unit }
}

const PERCENT: Text = linear(0.0, 1.0, 0, " %");
const MS: Text = linear(0.0, 1.0, 0, " ms");
const TENTH_MS: Text = linear(0.0, 0.1, 1, " ms");
const EQ_GAIN: Text = db(-24.0, 0.1);

/// Channel EQ's Controls View, as the bridge's `change-fx-params` scene read it on Logic Pro 12.3.1 (its "Master  Gain"
/// with two spaces, as Logic writes it).
static CHANNEL_EQ: [ParamSpec; 46] = [
    switch("Low Cut On/Off", false),
    slider("Low Cut Frequency", 1050.0, Text::Hz, 0.0),
    menu("Low Cut Slope", "12 dB/Oct"),
    slider("Low Cut Q-Factor", 127.0, Text::Q, 36.0),
    switch("Low Shelf On/Off", true),
    slider("Low Shelf Frequency", 1050.0, Text::Hz, 200.0),
    slider("Low Shelf Gain", 480.0, EQ_GAIN, 240.0),
    slider("Low Shelf Q-Factor", 52.0, Text::Q, 40.0),
    switch("Peak 1 On/Off", true),
    slider("Peak 1 Frequency", 1050.0, Text::Hz, 250.0),
    slider("Peak 1 Gain", 480.0, EQ_GAIN, 240.0),
    slider("Peak 1 Q-Factor", 127.0, Text::Q, 33.0),
    switch("Peak 2 On/Off", true),
    slider("Peak 2 Frequency", 1050.0, Text::Hz, 375.0),
    slider("Peak 2 Gain", 480.0, EQ_GAIN, 240.0),
    slider("Peak 2 Q-Factor", 127.0, Text::Q, 20.0),
    switch("Peak 3 On/Off", true),
    slider("Peak 3 Frequency", 1050.0, Text::Hz, 604.0),
    slider("Peak 3 Gain", 480.0, EQ_GAIN, 240.0),
    slider("Peak 3 Q-Factor", 127.0, Text::Q, 26.0),
    switch("Peak 4 On/Off", true),
    slider("Peak 4 Frequency", 1050.0, Text::Hz, 725.0),
    slider("Peak 4 Gain", 480.0, EQ_GAIN, 240.0),
    slider("Peak 4 Q-Factor", 127.0, Text::Q, 13.0),
    switch("High Shelf On/Off", true),
    slider("High Shelf Frequency", 1050.0, Text::Hz, 900.0),
    slider("High Shelf Gain", 480.0, EQ_GAIN, 240.0),
    slider("High Shelf Q-Factor", 52.0, Text::Q, 40.0),
    switch("High Cut On/Off", false),
    slider("High Cut Frequency", 1050.0, Text::Hz, 1050.0),
    menu("High Cut Slope", "24 dB/Oct"),
    slider("High Cut Q-Factor", 127.0, Text::Q, 36.0),
    slider("Master  Gain", 480.0, EQ_GAIN, 240.0),
    switch("Analyzer On/Off", true),
    menu("Analyzer Resolution", "Medium (4096 points)"),
    slider("Analyzer Top", 12.0, linear(-12.0, 3.0, 0, " dB"), 4.0),
    choice("Analyzer Position", &["Pre EQ", "Post EQ"], 1),
    choice("Analyzer Mode", &["Peak", "RMS"], 0),
    slider("Analyzer Decay", 20.0, linear(0.0, 1.0, 0, " dB/s"), 10.0),
    switch("Gain-Q Coupling", true),
    menu("Gain-Q Couple Mode", "Asymmetric Medium"),
    switch("Oversampling", false),
    switch("Visualize Master Gain", true),
    menu("EQ dB Scale Mode", "Linear 30 dB"),
    slider("EQ Scale Shift", 10.0, linear(-5.0, 1.0, 0, ""), 5.0),
    choice("Analyzer Range", &["60 dB", "120 dB"], 0),
];

/// A plug-in the simulated Logic's menus have, by the name its slot shows: whether it is an instrument (a software instrument's own slot, from a patch: never in an effect slot's menu), its Controls View, and
/// the settings its next and previous buttons step through, each with the raw values it gives.
struct PluginSpec {
    name: &'static str,
    instrument: bool,
    params: &'static [ParamSpec],
    settings: &'static [(&'static str, &'static [(&'static str, f64)])],
}

static PLUGINS: [PluginSpec; 15] = [
    PluginSpec {
        name: "Gain",
        instrument: false,
        params: &[
            slider("Balance", 400.0, linear(-100.0, 0.5, 1, " %"), 200.0),
            slider("Gain", 1200.0, db(-96.0, 0.1), 960.0),
            choice("Phase Left", &["Norm", "Inv"], 0),
            choice("Phase Right", &["Norm", "Inv"], 0),
            choice("Swap L/R", &["Norm", "Swap"], 0),
            choice("Make Mono", &["Stereo", "Mono"], 0),
        ],
        settings: &[],
    },
    PluginSpec {
        name: "Channel EQ",
        instrument: false,
        params: &CHANNEL_EQ,
        settings: &[
            ("Brighten Hi Hat", &[("Low Cut On/Off", 1.0), ("Low Cut Frequency", 375.0), ("High Shelf Gain", 280.0)]),
            ("Warm Bass", &[("Low Shelf Gain", 265.0), ("High Cut On/Off", 1.0), ("High Cut Frequency", 900.0)]),
        ],
    },
    PluginSpec {
        name: "Compressor",
        instrument: false,
        params: &[
            slider("Threshold", 600.0, db(-60.0, 0.1), 400.0),
            slider("Ratio", 290.0, linear(1.0, 0.1, 1, ":1"), 10.0),
            slider("Attack", 2000.0, TENTH_MS, 150.0),
            slider("Release", 5000.0, MS, 50.0),
            slider("Make Up", 600.0, db(-30.0, 0.1), 300.0),
            slider("Mix", 100.0, PERCENT, 100.0),
        ],
        settings: &[
            ("Vocal Compressor", &[("Threshold", 360.0), ("Ratio", 20.0), ("Attack", 100.0), ("Release", 80.0)]),
            ("Drum Bus", &[("Threshold", 420.0), ("Ratio", 30.0), ("Attack", 300.0), ("Release", 100.0)]),
        ],
    },
    PluginSpec {
        name: "Limiter",
        instrument: false,
        params: &[
            slider("Gain", 200.0, db(0.0, 0.1), 0.0),
            slider("Output Level", 200.0, db(-20.0, 0.1), 200.0),
            slider("Release", 1000.0, TENTH_MS, 30.0),
            slider("Lookahead", 200.0, TENTH_MS, 20.0),
        ],
        settings: &[],
    },
    PluginSpec {
        name: "Adaptive Limiter",
        instrument: false,
        params: &[
            slider("Input Scale", 400.0, db(-20.0, 0.1), 200.0),
            slider("Gain", 300.0, db(0.0, 0.1), 0.0),
            slider("Out Ceiling", 200.0, db(-20.0, 0.1), 199.0),
        ],
        settings: &[],
    },
    PluginSpec {
        name: "DeEsser 2",
        instrument: false,
        params: &[
            slider("Threshold", 600.0, db(-60.0, 0.1), 300.0),
            slider("Max Reduction", 300.0, db(0.0, 0.1), 120.0),
            slider("Frequency", 1920.0, linear(800.0, 10.0, 0, " Hz"), 520.0),
        ],
        settings: &[],
    },
    PluginSpec {
        name: "Multipressor",
        instrument: false,
        params: &[slider("Out Level", 600.0, db(-30.0, 0.1), 300.0), slider("Lookahead", 200.0, TENTH_MS, 0.0)],
        settings: &[],
    },
    PluginSpec {
        name: "ChromaVerb",
        instrument: false,
        params: &[
            slider("Decay", 999.0, linear(0.1, 0.1, 1, " s"), 19.0),
            slider("Pre-Delay", 500.0, MS, 20.0),
            slider("Size", 100.0, PERCENT, 50.0),
            slider("Dry", 100.0, PERCENT, 100.0),
            slider("Wet", 100.0, PERCENT, 30.0),
        ],
        settings: &[("Small Room", &[("Decay", 7.0), ("Size", 30.0)]), ("Large Hall", &[("Decay", 44.0), ("Size", 90.0)])],
    },
    PluginSpec {
        name: "Space Designer",
        instrument: false,
        params: &[slider("Length", 100.0, PERCENT, 100.0), slider("Dry", 100.0, PERCENT, 100.0), slider("Wet", 100.0, PERCENT, 30.0)],
        settings: &[],
    },
    PluginSpec {
        name: "Spring Box",
        instrument: false,
        params: &[slider("Time", 100.0, PERCENT, 50.0), slider("Tone", 100.0, PERCENT, 50.0), slider("Mix", 100.0, PERCENT, 30.0)],
        settings: &[],
    },
    PluginSpec {
        name: "Tape Delay",
        instrument: false,
        params: &[
            slider("Delay", 2000.0, MS, 375.0),
            slider("Feedback", 100.0, PERCENT, 30.0),
            slider("Dry", 100.0, PERCENT, 100.0),
            slider("Wet", 100.0, PERCENT, 20.0),
        ],
        settings: &[],
    },
    PluginSpec {
        name: "Stereo Delay",
        instrument: false,
        params: &[
            slider("Left Delay", 2000.0, MS, 375.0),
            slider("Right Delay", 2000.0, MS, 375.0),
            slider("Feedback", 100.0, PERCENT, 30.0),
            slider("Mix", 100.0, PERCENT, 30.0),
        ],
        settings: &[],
    },
    PluginSpec {
        name: "Piano",
        instrument: true,
        params: &[
            menu("Instrument", "Studio Grand (One Mic)"),
            slider("Main Volume", 66.0, linear(-60.0, 1.0, 0, " dB"), 60.0),
            slider("Pedal Noise", 100.0, PERCENT, 60.0),
            slider("Key Noise", 100.0, PERCENT, 35.0),
            slider("Release Samples", 100.0, PERCENT, 60.0),
            slider("Sympathetic Resonances", 100.0, PERCENT, 60.0),
        ],
        settings: &[],
    },
    // (M6) The Sampler Logic's Convert Regions to New Sampler Track loads (its Controls View is the simulator's).
    PluginSpec {
        name: "Sampler",
        instrument: true,
        params: &[
            slider("Volume", 1080.0, db(-96.0, 0.1), 960.0),
            slider("Cutoff", 100.0, PERCENT, 100.0),
            slider("Resonance", 100.0, PERCENT, 0.0),
            slider("Attack", 10000.0, MS, 0.0),
            slider("Release", 10000.0, MS, 50.0),
        ],
        settings: &[],
    },
    PluginSpec {
        name: "Alchemy",
        instrument: true,
        params: &[
            slider("Master Volume", 1080.0, db(-96.0, 0.1), 960.0),
            slider("Cutoff", 100.0, PERCENT, 100.0),
            slider("Resonance", 100.0, PERCENT, 0.0),
            slider("Attack", 10000.0, MS, 10.0),
            slider("Release", 10000.0, MS, 300.0),
        ],
        settings: &[],
    },
];

/// A patch of the simulated Library, by its path in the Library's columns: the tracks it is shown for (a software
/// instrument's patches, an audio track's), the plug-ins it loads (the instrument first), and its Smart Controls.
struct PatchSpec {
    path: &'static str,
    track_kind: &'static str,
    plugins: &'static [&'static str],
    smart: &'static [(&'static str, f64)],
}

/// The recorded project's Keys plays "Studio Grand" (its strip's setting button reads so in the M2 scene, its Smart
/// Controls as `scratch-m4b` reads them); Classic Vocal loads what `change-patch-load` read; the others, and Classic
/// Vocal's Smart Controls, are the simulator's.
static PATCHES: [PatchSpec; 5] = [
    PatchSpec {
        path: "Piano/Studio Grand",
        track_kind: "instrument",
        plugins: &["Piano", "Channel EQ", "Compressor", "ChromaVerb"],
        smart: &[("Tone", 0.0), ("Low Cut", 0.0), ("Comp", 0.375), ("Ratio", 0.4)],
    },
    PatchSpec {
        path: "Piano/Steinway Grand Piano",
        track_kind: "instrument",
        plugins: &["Piano", "Channel EQ", "Space Designer"],
        smart: &[("Tone", 0.55), ("Room", 0.4), ("Reverb", 0.3)],
    },
    PatchSpec {
        path: "Synthesizer/Bass/Deep Sub Bass",
        track_kind: "instrument",
        plugins: &["Alchemy", "Channel EQ", "Compressor"],
        smart: &[("Cutoff", 0.6), ("Resonance", 0.1), ("Drive", 0.2), ("Attack", 0.0)],
    },
    PatchSpec {
        path: "Vocal/Classic Vocal",
        track_kind: "audio",
        plugins: &["Channel EQ", "Compressor", "Channel EQ", "Tape Delay", "Spring Box"],
        smart: &[("Compression", 0.4), ("Brightness", 0.5), ("De-Ess", 0.3)],
    },
    PatchSpec {
        path: "Vocal/Natural Vocal",
        track_kind: "audio",
        plugins: &["Channel EQ", "Compressor"],
        smart: &[("Compression", 0.3), ("Presence", 0.5)],
    },
];

impl PatchSpec {
    /// Its name, as the strip's setting button and the Library's search show it: the path's last column.
    fn name(&self) -> &'static str {
        self.path.rsplit('/').next().unwrap_or(self.path)
    }
}

/// The patch the recorded project's Keys plays, and the one Logic gives a software instrument track it makes for a
/// MIDI file.
pub(super) const KEYS_PATCH: &str = "Piano/Studio Grand";

/// The kinds the bridge reads (`read.rs`'s `KINDS`), with (M6) the projects Logic opened lately.
const READ_KINDS: [&str; 12] =
    ["project", "track", "region", "note", "plugin", "send", "marker", "tempo", "param", "smart", "automation", "recent"];

/// The parameters the simulated Track Automation Event List names (Volume is 7, as the bridge's scenes read it; Balance
/// 8 and Pan 10 are the simulator's).
const AUTOMATED: [(&str, u32); 3] = [("Volume", 7), ("Balance", 8), ("Pan", 10)];

fn spec_named(name: &str) -> Option<&'static PluginSpec> {
    PLUGINS.iter().find(|spec| spec.name.eq_ignore_ascii_case(name.trim()))
}

fn patch_path(path: &str) -> Option<&'static PatchSpec> {
    PATCHES.iter().find(|patch| patch.path == path)
}

/// Whether two plug-in names are the same as Logic shows them (its search is given any case).
fn same_name(a: &str, b: &str) -> bool {
    a.trim().eq_ignore_ascii_case(b.trim())
}

/// The first number in a text, as the bridge reads a slider's text ("+3.0 dB" is 3, "−6" is -6).
fn first_number(text: &str) -> Option<f64> {
    let mut number = String::new();
    let mut seen_digit = false;
    for c in text.chars() {
        if c.is_ascii_digit() {
            number.push(c);
            seen_digit = true;
        } else if (c == '.' || c == ',') && seen_digit && !number.contains('.') {
            number.push('.');
        } else if (c == '-' || c == '+' || c == '−') && !seen_digit && number.is_empty() {
            number.push(if c == '+' { '+' } else { '-' });
        } else if seen_digit {
            break;
        } else {
            number.clear();
        }
    }
    number.trim_end_matches('.').parse().ok()
}

/// Along a curve of recorded points: between two, along the logarithm; before the first or past the last, as it.
fn along(points: &[(f64, f64)], raw: f64) -> f64 {
    let (first, last) = (points[0], points[points.len() - 1]);
    if raw <= first.0 {
        return first.1;
    }
    if raw >= last.0 {
        return last.1;
    }
    let at = points.windows(2).find(|pair| raw <= pair[1].0).unwrap_or(&points[points.len() - 2..]);
    let ((r0, v0), (r1, v1)) = (at[0], at[1]);
    (v0.ln() + (raw - r0) / (r1 - r0) * (v1.ln() - v0.ln())).exp()
}

/// A slider's text for a raw value.
fn slider_text(text: Text, raw: f64) -> String {
    match text {
        Text::Db { low, step } => {
            let value = ((low + raw * step) * 10.0).round() / 10.0;
            if value.abs() < 0.05 {
                "0.0 dB".into()
            } else if value > 0.0 {
                format!("+{value:.1} dB")
            } else {
                format!("{value:.1} dB")
            }
        }
        Text::Linear { low, step, decimals, unit } => {
            let value = low + raw * step;
            let value = if value.abs() < 1e-9 { 0.0 } else { value };
            format!("{value:.decimals$}{unit}")
        }
        Text::Hz => {
            let hz = along(&FREQUENCY, raw);
            if hz < 99.95 {
                format!("{hz:.1} Hz")
            } else {
                format!("{hz:.0} Hz")
            }
        }
        Text::Q => format!("{:.2}", along(&Q_FACTOR, raw)),
    }
}

/// A parameter's row parts for a raw value: its value (none for a menu), its text, its range.
fn shown(spec: &ParamSpec, raw: f64) -> (Option<f64>, String, Option<f64>, Option<f64>) {
    match spec.control {
        Control::Slider { max, text } => (Some(raw), slider_text(text, raw), Some(0.0), Some(max)),
        Control::Switch => (Some(raw), if raw >= 0.5 { "on".into() } else { "off".into() }, Some(0.0), Some(1.0)),
        Control::Choice(options) => {
            let at = (raw.round().max(0.0) as usize).min(options.len() - 1);
            (Some(at as f64), options[at].into(), Some(0.0), Some((options.len() - 1) as f64))
        }
        Control::Menu(text) => (None, text.into(), None, None),
    }
}

/// A plug-in in a strip's slot: its name as the slot shows it, its bypass, the setting its window names, each row's raw
/// value in the Controls View's order, and its window's own undo (the setting and values before each step).
#[derive(Clone, Debug, PartialEq)]
pub(super) struct Plugin {
    pub(super) name: String,
    pub(super) bypassed: bool,
    pub(super) preset: String,
    values: Vec<f64>,
    window_undo: Vec<(String, Vec<f64>)>,
}

impl Plugin {
    fn new(spec: &PluginSpec) -> Plugin {
        Plugin {
            name: spec.name.into(),
            bypassed: false,
            preset: DEFAULT_PRESET.into(),
            values: spec.params.iter().map(|param| param.default).collect(),
            window_undo: vec![],
        }
    }

    /// A plug-in of the catalogue by its name, as Logic inserts it.
    pub(super) fn named(name: &str) -> Plugin {
        Plugin::new(spec_named(name).expect("a plug-in the simulated Logic knows"))
    }

    fn spec(&self) -> &'static PluginSpec {
        spec_named(&self.name).expect("a plug-in the simulated Logic knows")
    }

    fn is_instrument(&self) -> bool {
        self.spec().instrument
    }

    fn kind(&self) -> &'static str {
        if self.is_instrument() {
            "instrument"
        } else {
            "effect"
        }
    }

    fn param_count(&self) -> usize {
        self.values.len()
    }

    /// A row's state as a change keeps it (`at` from 0): `{index, name, value, text}`.
    fn state(&self, at: usize) -> Value {
        let spec = &self.spec().params[at];
        let (value, text, _, _) = shown(spec, self.values[at]);
        json!({"index": at + 1, "name": spec.name, "value": value, "text": text})
    }

    /// A row's text now.
    fn text(&self, at: usize) -> String {
        shown(&self.spec().params[at], self.values[at]).1
    }

    /// The window's own undo takes a step: the setting and values before it.
    fn remember(&mut self) {
        self.window_undo.push((self.preset.clone(), self.values.clone()));
    }

    /// A row set to a raw value in the Controls View (a step of the window's own undo).
    fn set(&mut self, at: usize, raw: f64) {
        self.remember();
        self.values[at] = raw;
    }

    /// The window's next (or previous) setting button pressed, a step of the window's own undo: the setting after (or
    /// before) the one it names, staying at either end of the list; from "Default Preset", which is on neither way, the
    /// first (measured on Logic Pro 12.3.1: next from it is the first preset, previous from there stays put). Nothing
    /// for a plug-in without settings. The setting after.
    fn press(&mut self, next: bool) -> String {
        self.remember();
        let settings = self.spec().settings;
        if settings.is_empty() {
            return self.preset.clone();
        }
        let n = settings.len();
        let to = match (settings.iter().position(|(name, _)| *name == self.preset), next) {
            (None, _) => 0,
            (Some(at), true) => (at + 1).min(n - 1),
            (Some(at), false) => at.saturating_sub(1),
        };
        let (name, values) = settings[to];
        self.values = self.spec().params.iter().map(|param| param.default).collect();
        for (param, raw) in values.iter() {
            if let Some(at) = self.spec().params.iter().position(|spec| spec.name == *param) {
                self.values[at] = *raw;
            }
        }
        self.preset = name.into();
        self.preset.clone()
    }

    /// The window's Undo pressed: its latest step taken back, if it has one.
    fn window_undo(&mut self) {
        if let Some((preset, values)) = self.window_undo.pop() {
            self.preset = preset;
            self.values = values;
        }
    }

    /// A row by its name (any case), as a script asks for it: its raw value and its text.
    pub(super) fn row_named(&self, name: &str) -> Option<(Option<f64>, String)> {
        let at = self.spec().params.iter().position(|spec| spec.name.eq_ignore_ascii_case(name))?;
        let (value, text, _, _) = shown(&self.spec().params[at], self.values[at]);
        Some((value, text))
    }

    /// (M5) What it does to the sound the simulated Logic plays, read from its Controls View as it shows it: a Gain
    /// turns it by its gain (−96 dB and below silences it), with its balance, phase, swap and mono; a Limiter and an
    /// Adaptive Limiter turn it by their gain (and input scale) and hold its peaks at their ceiling; one bypassed, or
    /// any other plug-in, leaves it as it is.
    pub(super) fn stage(&self) -> Stage {
        if self.bypassed {
            return Stage::Through;
        }
        let db = |name: &str| self.row_named(name).and_then(|(_, text)| first_number(&text.replace('\u{2212}', "-"))).unwrap_or(0.0);
        let option = |name: &str| self.row_named(name).is_some_and(|(value, _)| value.unwrap_or(0.0) >= 0.5);
        match self.name.as_str() {
            "Gain" => Stage::Gain {
                db: db("Gain"),
                balance: db("Balance") / 100.0,
                invert: (option("Phase Left"), option("Phase Right")),
                swap: option("Swap L/R"),
                mono: option("Make Mono"),
            },
            "Limiter" => Stage::Limit { db: db("Gain"), ceiling: db("Output Level") },
            "Adaptive Limiter" => Stage::Limit { db: db("Input Scale") + db("Gain"), ceiling: db("Out Ceiling") },
            _ => Stage::Through,
        }
    }
}

/// (M5) What a plug-in does to the simulated sound (`Plugin::stage`).
#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) enum Stage {
    /// A Gain: its gain in dB, its balance (-1 left to 1 right), each side's phase inverted, the sides swapped, mono.
    Gain { db: f64, balance: f64, invert: (bool, bool), swap: bool, mono: bool },
    /// A limiter: its gain in, then its ceiling (dBFS) over the peaks.
    Limit { db: f64, ceiling: f64 },
    /// Nothing the simulation hears.
    Through,
}

impl Stage {
    /// A stereo frame through it.
    pub(super) fn apply(self, (left, right): (f64, f64)) -> (f64, f64) {
        match self {
            Stage::Gain { db, balance, invert, swap, mono } => {
                if db <= MIN_DB {
                    return (0.0, 0.0);
                }
                let gain = 10f64.powf(db / 20.0);
                let (mut l, mut r) = (left * gain * (1.0 - balance).min(1.0), right * gain * (1.0 + balance).min(1.0));
                if invert.0 {
                    l = -l;
                }
                if invert.1 {
                    r = -r;
                }
                if swap {
                    (l, r) = (r, l);
                }
                if mono {
                    let both = (l + r) / 2.0;
                    (l, r) = (both, both);
                }
                (l, r)
            }
            Stage::Limit { db, ceiling } => {
                let (gain, most) = (10f64.powf(db / 20.0), 10f64.powf(ceiling.min(0.0) / 20.0));
                ((left * gain).clamp(-most, most), (right * gain).clamp(-most, most))
            }
            Stage::Through => (left, right),
        }
    }
}

/// A send in a strip's send slot: the bus it sends to and its level in dB (−∞ as -96).
#[derive(Clone, Debug, PartialEq)]
pub(super) struct Send {
    pub(super) bus: usize,
    pub(super) level: f64,
}

impl Send {
    fn destination(&self) -> String {
        format!("Bus {}", self.bus)
    }
}

/// A channel strip's plug-ins (its instrument first, then its effects from the top), its sends, and the name its
/// setting button shows (the patch it plays; empty for none).
#[derive(Clone, Debug, Default, PartialEq)]
pub(super) struct Channel {
    pub(super) plugins: Vec<Plugin>,
    pub(super) sends: Vec<Send>,
    pub(super) patch: String,
}

impl Channel {
    /// A patch's strip: its plug-ins as new ones, and its name on the setting button.
    fn of_patch(patch: &PatchSpec) -> Channel {
        Channel { plugins: patch.plugins.iter().map(|name| Plugin::named(name)).collect(), sends: vec![], patch: patch.name().into() }
    }

    /// Where its effects start among its plug-ins: after its instrument, when it has one.
    fn first_effect(&self) -> usize {
        usize::from(self.plugins.first().is_some_and(Plugin::is_instrument))
    }

    fn instrument(&self) -> Option<&Plugin> {
        self.plugins.first().filter(|plugin| plugin.is_instrument())
    }

    fn effect_names(&self) -> Vec<String> {
        self.plugins[self.first_effect()..].iter().map(|plugin| plugin.name.clone()).collect()
    }

    /// Every plug-in's name, the instrument first: what a patch's fence holds.
    fn names(&self) -> Vec<String> {
        self.plugins.iter().map(|plugin| plugin.name.clone()).collect()
    }

    /// What a plug-in change fences: the effects from the top, and the instrument.
    fn fence(&self) -> Value {
        json!({"plugins": self.effect_names(), "instrument": self.instrument().map(|plugin| plugin.name.clone())})
    }

    fn busses(&self) -> Vec<String> {
        self.sends.iter().map(Send::destination).collect()
    }

    /// The plug-in in a slot (from 1, the instrument first), and whether it is the instrument.
    fn at_slot(&self, slot: usize) -> Option<(&Plugin, bool)> {
        let plugin = self.plugins.get(slot.checked_sub(1)?)?;
        Some((plugin, slot == 1 && plugin.is_instrument()))
    }

    /// The effect a restore means (its index among the plug-ins): in its slot when it still has its name there, else
    /// the one effect of that name.
    fn effect_named(&self, slot: usize, name: &str) -> Option<usize> {
        if let Some((plugin, false)) = self.at_slot(slot) {
            if same_name(&plugin.name, name) {
                return Some(slot - 1);
            }
        }
        let named: Vec<usize> = (self.first_effect()..self.plugins.len()).filter(|at| same_name(&self.plugins[*at].name, name)).collect();
        (named.len() == 1).then(|| named[0])
    }
}

/// A Smart Control of a track's patch: its name and its slider's share of its range (0 to 1).
#[derive(Clone, Debug, PartialEq)]
pub(super) struct Smart {
    pub(super) name: String,
    pub(super) value: f64,
}

impl Smart {
    /// Its text, as the pane shows it beside the slider: `38%`.
    fn text(&self) -> String {
        format!("{}%", (self.value * 100.0).round() as i64)
    }

    fn state(&self) -> Value {
        json!({"name": self.name, "value": round4(self.value), "text": self.text()})
    }
}

/// The Smart Controls a patch shows.
fn smart_of(patch: &PatchSpec) -> Vec<Smart> {
    patch.smart.iter().map(|(name, value)| Smart { name: (*name).into(), value: *value }).collect()
}

/// An automation event of a track, as the Track Automation Event List lists its "Fader" rows: its position (beats from
/// the project's start), its parameter's number (Num) and name (the Length/Info cell), and its value (Val, 0 to 127).
#[derive(Clone, Debug, PartialEq)]
pub(super) struct AutoEvent {
    pub(super) beats: f64,
    pub(super) number: u32,
    pub(super) parameter: String,
    pub(super) value: u32,
}

/// The name the list's Length/Info cell writes for a parameter number.
fn automated_name(number: u32) -> String {
    AUTOMATED.iter().find(|(_, n)| *n == number).map_or_else(|| format!("Parameter {number}"), |(name, _)| (*name).to_string())
}

/// An output strip of the Mixer, which no track has: the Stereo Out, with its plug-ins.
#[derive(Clone, Debug)]
pub(super) struct Output {
    pub(super) name: String,
    pub(super) channel: Channel,
}

impl Output {
    /// The recorded project's Stereo Out: no plug-in in its slots (the M2 scene's Mixer).
    pub(super) fn stereo_out() -> Output {
        Output { name: "Stereo Out".into(), channel: Channel::default() }
    }
}

/// An aux Logic made for a send to a bus nothing took: its name ("Aux 1") and its bus. It has no ref.
#[derive(Clone, Debug)]
pub(super) struct Aux {
    pub(super) name: String,
    pub(super) bus: usize,
}

/// The track's own channel strip as the recorded project holds it: Vocal (track 1) with the plug-in its row reads, Keys
/// (track 2) playing Studio Grand, as the M2 scene's Mixer shows them.
pub(super) fn recorded_channel(plugin_rows: &[Value], name: &str) -> Channel {
    if name == "Keys" {
        return Channel::of_patch(patch_path(KEYS_PATCH).expect("Keys' patch"));
    }
    let plugins = plugin_rows
        .iter()
        .filter_map(|row| {
            let mut plugin = Plugin::new(spec_named(row["name"].as_str()?)?);
            plugin.bypassed = row["bypassed"] == true;
            Some(plugin)
        })
        .collect();
    Channel { plugins, sends: vec![], patch: String::new() }
}

/// The Smart Controls a recorded track shows: its patch's (none for a track that plays no patch).
pub(super) fn recorded_smart(channel: &Channel) -> Vec<Smart> {
    PATCHES.iter().find(|patch| patch.name() == channel.patch).map(smart_of).unwrap_or_default()
}

/// (M6) The strip of the software instrument track Logic makes for a sample: the Sampler alone, its setting button
/// naming the instrument.
pub(super) fn sampler_channel(name: &str) -> Channel {
    Channel { plugins: vec![Plugin::named("Sampler")], sends: vec![], patch: name.into() }
}

/// A software instrument track Logic makes for a MIDI file it imports: its default patch's strip and Smart Controls.
pub(super) fn imported_channel() -> (Channel, Vec<Smart>) {
    let patch = patch_path(KEYS_PATCH).expect("Logic's default patch");
    (Channel::of_patch(patch), smart_of(patch))
}

fn round4(value: f64) -> f64 {
    (value * 1e4).round() / 1e4
}

/// A share of a range, to a millionth, as the bridge gives a parameter's `normalized`.
fn normalized(value: Option<f64>, min: Option<f64>, max: Option<f64>) -> Option<f64> {
    let (value, min, max) = (value?, min?, max?);
    (max > min).then(|| ((value - min) / (max - min) * 1e6).round() / 1e6)
}

/// A level as a send's knob reads it: to a tenth of a dB, the floor as itself.
fn knob_db(db: f64) -> f64 {
    if db <= MIN_DB {
        MIN_DB
    } else {
        (db * 10.0).round() / 10.0
    }
}

fn same_db(a: Option<f64>, b: Option<f64>) -> bool {
    match (a, b) {
        (Some(a), Some(b)) => (a - b).abs() <= CLOSE_DB || (a <= MIN_DB && b <= MIN_DB),
        (None, None) => true,
        _ => false,
    }
}

// ----- what a change asks for, in the bridge's words -----

/// The plug-in a request names: the name Logic's slot will show (the last part of a category path such as
/// "Dynamics/Compressor"), which is also what the menu's search is given.
fn plugin_name(input: &Value) -> Result<String, Fault> {
    let name =
        input.get("name").and_then(Value::as_str).map(str::trim).filter(|name| !name.is_empty()).ok_or_else(|| {
            bad("name must be the plug-in's name as Logic Pro's plug-in menu shows it, such as \"Channel EQ\" or \"Gain\"")
        })?;
    if name.chars().count() > NAME_LIMIT || name.chars().any(char::is_control) {
        return Err(bad(format!("name must be one line of at most {NAME_LIMIT} characters")));
    }
    Ok(name.rsplit('/').map(str::trim).find(|part| !part.is_empty()).unwrap_or(name).to_string())
}

/// Where a new plug-in goes among the strip's effects (from 0; the end when not given).
fn index_of(input: &Value, count: usize) -> Result<usize, Fault> {
    match input.get("index") {
        None | Some(Value::Null) => Ok(count),
        Some(value) => value
            .as_u64()
            .map(|index| index as usize)
            .filter(|index| *index <= count)
            .ok_or_else(|| bad(format!("index must be a place among the strip's {count} effect plug-in(s), from 0 to {count}"))),
    }
}

/// What Logic's plug-in menu's search takes first for a name: the effect of that name, else the first whose name starts
/// with it, else the first holding it (an instrument is never in an effect slot's menu).
fn searched(name: &str) -> Option<&'static PluginSpec> {
    let wanted = name.trim().to_lowercase();
    let effects = || PLUGINS.iter().filter(|spec| !spec.instrument);
    effects()
        .find(|spec| spec.name.to_lowercase() == wanted)
        .or_else(|| effects().find(|spec| spec.name.to_lowercase().starts_with(&wanted)))
        .or_else(|| effects().find(|spec| spec.name.to_lowercase().contains(&wanted)))
}

/// What a value asked for is: a share of the range, a number in the parameter's own units (with how close it must
/// read), or an option's name.
#[derive(Clone, Debug, PartialEq)]
enum Wanted {
    Normalized(f64),
    Reading(f64, f64),
    Option(String),
}

/// Half a unit of the last decimal given: "-6" may read -6.4 at worst, "-6.0" -6.05.
fn tolerance(text: &str) -> f64 {
    let decimals = text.split_once('.').map_or(0, |(_, rest)| rest.chars().take_while(char::is_ascii_digit).count());
    0.5 * 10f64.powi(-(decimals as i32)) + 1e-9
}

fn wanted_of(item: &Value) -> Result<Wanted, Fault> {
    if let Some(n) = item.get("normalized").filter(|v| !v.is_null()) {
        let n = n.as_f64().filter(|n| (0.0..=1.0).contains(n)).ok_or_else(|| bad("normalized must be a number from 0 to 1"))?;
        return Ok(Wanted::Normalized(n));
    }
    match item.get("value") {
        Some(Value::Number(n)) => {
            let n = n.as_f64().filter(|n| n.is_finite()).ok_or_else(|| bad("value must be a finite number"))?;
            Ok(Wanted::Reading(n, tolerance(&n.to_string())))
        }
        Some(Value::String(text)) => {
            let reads = first_number(&text.replace('\u{2212}', "-"))
                .filter(|_| text.trim().starts_with(|c: char| c.is_ascii_digit() || "-+.−".contains(c)));
            match reads {
                Some(n) => Ok(Wanted::Reading(n, tolerance(text))),
                None if !text.trim().is_empty() => Ok(Wanted::Option(text.trim().to_string())),
                None => Err(bad("value must say what the parameter should show")),
            }
        }
        _ => Err(bad("give normalized (0 to 1) or value (what the Controls View should show, such as -6 or \"-6.0 dB\")")),
    }
}

/// The asks of a change: `fx-param`'s one (its `param`, or the place its param ref names), `fx-params`' list.
fn asks(input: &Value, from_ref: Option<usize>, many: bool) -> Result<Vec<(Value, Wanted)>, Fault> {
    if !many {
        let which = match (from_ref, input.get("param").filter(|v| !v.is_null())) {
            (Some(index), _) => json!(index),
            (None, Some(param)) => param.clone(),
            (None, None) => return Err(bad("pass the parameter's ref as ref, or the plug-in's ref as ref and param (its name or place)")),
        };
        return Ok(vec![(which, wanted_of(input)?)]);
    }
    let list = input
        .get("values")
        .and_then(Value::as_array)
        .filter(|list| !list.is_empty())
        .ok_or_else(|| bad("values: a list of {param, normalized | value}"))?;
    if list.len() > MOST_PARAMS {
        return Err(bad(format!("at most {MOST_PARAMS} values at once ({} given)", list.len())));
    }
    list.iter()
        .map(|item| {
            let which =
                item.get("param").filter(|v| !v.is_null()).cloned().ok_or_else(|| bad("each value needs its param (name or place)"))?;
            Ok((which, wanted_of(item)?))
        })
        .collect()
}

/// A parameter of a plug-in by its name as the Controls View shows it (any case) or its place (from 1): its index.
fn find_param(plugin: &Plugin, which: &Value) -> Result<usize, Fault> {
    let params = plugin.spec().params;
    match which {
        Value::Number(n) => n
            .as_u64()
            .map(|n| n as usize)
            .filter(|n| *n >= 1 && *n <= params.len())
            .map(|n| n - 1)
            .ok_or_else(|| bad(format!("param must be a parameter's place from 1 to {}, or its name", params.len()))),
        Value::String(name) => {
            let found: Vec<usize> = (0..params.len()).filter(|at| params[*at].name.eq_ignore_ascii_case(name.trim())).collect();
            match found.as_slice() {
                [one] => Ok(*one),
                [] => Err(bad(format!(
                    "{} has no parameter named {name:?}: read its parameters (read kind param) for their names",
                    quoted(&plugin.name)
                ))),
                _ => Err(bad(format!("several parameters of {} are named {name:?}: give its place or its ref", quoted(&plugin.name)))),
            }
        }
        _ => Err(bad("param must be the parameter's name or its place (from 1)")),
    }
}

/// The parameters the asks name, each once.
fn named(plugin: &Plugin, asks: &[(Value, Wanted)]) -> Result<Vec<usize>, Fault> {
    let mut out: Vec<usize> = Vec::new();
    for (which, _) in asks {
        let at = find_param(plugin, which)?;
        if out.contains(&at) {
            return Err(bad(format!("{} is given twice", plugin.spec().params[at].name)));
        }
        out.push(at);
    }
    Ok(out)
}

/// The raw value a wanted value is for a row: computed for a share, found by its text for a number, the option's place
/// for a name.
fn raw_for(plugin: &Plugin, at: usize, wanted: &Wanted) -> Result<f64, Fault> {
    let spec = &plugin.spec().params[at];
    let now = plugin.text(at);
    let refused = || bad(format!("{}'s {} doesn't take that value (it shows {now})", quoted(&plugin.name), spec.name));
    match (wanted, spec.control) {
        (Wanted::Normalized(n), Control::Slider { max, .. }) => Ok((n * max).round()),
        (Wanted::Normalized(n), Control::Choice(options)) => Ok((n * (options.len() - 1) as f64).round()),
        (Wanted::Normalized(n), Control::Switch) => Ok(if *n >= 0.5 { 1.0 } else { 0.0 }),
        (Wanted::Reading(n, tolerance), Control::Slider { max, text }) => {
            let reading = |raw: f64| first_number(&slider_text(text, raw).replace('\u{2212}', "-"));
            let found = match (reading(0.0), reading(max)) {
                (Some(low), Some(high)) if *n >= low.min(high) - tolerance && *n <= low.max(high) + tolerance => (0..=max as u64)
                    .filter_map(|raw| reading(raw as f64).map(|at| (raw as f64, at)))
                    .min_by(|a, b| (a.1 - n).abs().total_cmp(&(b.1 - n).abs()))
                    .filter(|(_, at)| (at - n).abs() <= *tolerance),
                _ => None,
            };
            found.map(|(raw, _)| raw).ok_or_else(|| {
                failed(format!(
                    "no value of {}'s {} reads {n} (it reads {now} now; read its parameters for the range)",
                    quoted(&plugin.name),
                    spec.name
                ))
            })
        }
        (Wanted::Option(name), Control::Choice(options)) => options
            .iter()
            .position(|title| title.eq_ignore_ascii_case(name))
            .map(|at| at as f64)
            .ok_or_else(|| bad(format!("{} is one of {}", spec.name, options.join(", ")))),
        (Wanted::Option(name), Control::Switch) if ["on", "off"].contains(&name.to_lowercase().as_str()) => {
            Ok(if name.eq_ignore_ascii_case("on") { 1.0 } else { 0.0 })
        }
        _ => Err(refused()),
    }
}

/// Whether two parameter states read the same: by value when both have one, else by text.
fn same_value(a: &Value, b: &Value) -> bool {
    match (a["value"].as_f64(), b["value"].as_f64()) {
        (Some(a), Some(b)) => (a - b).abs() < 1e-6,
        _ => a["text"] == b["text"],
    }
}

/// A parameter change's title: one parameter's new text, or how many.
fn params_title(plugin: &str, strip: &str, params: &[Value]) -> String {
    match params {
        [one] => format!(
            "{}'s {} → {} on {}",
            quoted(plugin),
            one["name"].as_str().unwrap_or_default(),
            one["text"].as_str().unwrap_or_default(),
            quoted(strip)
        ),
        many => format!("{} {} parameters on {}", quoted(plugin), many.len(), quoted(strip)),
    }
}

/// A preset asked for: presses of next (above 0) or previous, or a setting by its name.
#[derive(Clone, Debug, PartialEq)]
enum Ask {
    Step(i64),
    Named(String),
}

fn ask_of(input: &Value) -> Result<Ask, Fault> {
    match (input.get("preset").filter(|v| !v.is_null()), input.get("step").filter(|v| !v.is_null())) {
        (Some(Value::String(name)), None) if !name.trim().is_empty() => Ok(Ask::Named(name.trim().to_string())),
        (None, Some(step)) => {
            step.as_i64().filter(|n| *n != 0 && n.unsigned_abs() as usize <= MOST_PRESSES).map(Ask::Step).ok_or_else(|| {
                bad(format!(
                    "step must be a whole number from -{MOST_PRESSES} to {MOST_PRESSES}, not 0: the next (above 0) or previous settings"
                ))
            })
        }
        _ => {
            Err(bad("give preset (a setting's name as the plug-in window's setting menu shows it) or step (+1 the next, -1 the previous)"))
        }
    }
}

/// The bus a send asks for: `bus` or `toRef`, a number or "Bus n".
fn bus_of(input: &Value) -> Result<u64, Fault> {
    let value = input.get("bus").filter(|v| !v.is_null()).or_else(|| input.get("toRef").filter(|v| !v.is_null()));
    let number = match value {
        Some(Value::Number(n)) => n.as_u64(),
        Some(Value::String(text)) => text.trim().strip_prefix("Bus").map(str::trim).unwrap_or(text.trim()).parse().ok(),
        _ => None,
    };
    number.filter(|n| (1..=MOST_BUS).contains(n)).ok_or_else(|| {
        bad(format!(
            "bus (or toRef) must name a bus from 1 to {MOST_BUS}, such as 2 or \"Bus 2\": Logic Pro sends to busses, and makes an aux for one nothing uses yet"
        ))
    })
}

/// A send's level asked for, in dB (none when not given); -96 and below are −∞.
fn level_of(input: &Value) -> Result<Option<f64>, Fault> {
    match input.get("volumeDb") {
        None | Some(Value::Null) => Ok(None),
        Some(value) => {
            let db = value.as_f64().filter(|db| db.is_finite()).ok_or_else(|| bad("volumeDb must be a number"))?;
            if db > SEND_MAX_DB {
                return Err(bad(format!("volumeDb must be at most {SEND_MAX_DB} (a send's knob goes no higher)")));
            }
            Ok(Some(db.max(MIN_DB)))
        }
    }
}

/// The patch a request names: its name, the last column of its path in the Library ("Vocal/Classic Vocal").
fn patch_of(input: &Value) -> Result<String, Fault> {
    let text = input
        .get("patch")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|patch| !patch.is_empty())
        .ok_or_else(|| bad("patch: the patch's name as Logic Pro's Library shows it, or its path there (\"Vocal/Classic Vocal\")"))?;
    if text.chars().any(char::is_control) || text.chars().count() > 300 {
        return Err(bad("patch must be one line of at most 300 characters"));
    }
    text.split('/')
        .map(str::trim)
        .filter(|part| !part.is_empty())
        .next_back()
        .map(str::to_string)
        .ok_or_else(|| bad("patch must name a patch"))
}

/// A ref taken apart: its epoch, kind and numbers (`2:plugin:0.1.3` is 2, `plugin`, [0, 1, 3]).
fn parts(text: &str) -> Option<(u32, &str, Vec<usize>)> {
    let mut parts = text.splitn(3, ':');
    let epoch = parts.next()?.parse().ok()?;
    let kind = parts.next()?;
    let numbers: Option<Vec<usize>> = parts.next()?.split('.').map(|n| n.parse().ok()).collect();
    Some((epoch, kind, numbers?))
}

fn not_in_project(reference: &str) -> Fault {
    failed(format!("{reference} isn't in the project (deleted, or another project)"))
}

fn stale(from: u32, now: u32) -> Fault {
    ("stale".into(), format!("that ref is from an earlier epoch ({from}; now {now}): read again"))
}

/// Whose strip: a track's or an output's (each from 0).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Owner {
    Track(usize),
    Output(usize),
}

impl Owner {
    fn kind(self) -> &'static str {
        match self {
            Owner::Track(_) => "track",
            Owner::Output(_) => "output",
        }
    }

    /// The numbers a ref under the strip starts with: `2` for track 2's, `0.1` for the Stereo Out's.
    fn numbers(self) -> String {
        match self {
            Owner::Track(at) => (at + 1).to_string(),
            Owner::Output(at) => format!("0.{}", at + 1),
        }
    }

    fn text(self, epoch: u32) -> String {
        format!(
            "{epoch}:{}:{}",
            self.kind(),
            match self {
                Owner::Track(at) | Owner::Output(at) => at + 1,
            }
        )
    }

    fn plugin(self, epoch: u32, slot: usize) -> String {
        format!("{epoch}:plugin:{}.{slot}", self.numbers())
    }

    fn param(self, epoch: u32, slot: usize, index: usize) -> String {
        format!("{epoch}:param:{}.{slot}.{index}", self.numbers())
    }
}

impl State {
    // ----- the strips, and the refs that name them -----

    pub(super) fn channel(&self, owner: Owner) -> &Channel {
        match owner {
            Owner::Track(at) => &self.tracks[at].channel,
            Owner::Output(at) => &self.outputs[at].channel,
        }
    }

    fn channel_mut(&mut self, owner: Owner) -> &mut Channel {
        match owner {
            Owner::Track(at) => &mut self.tracks[at].channel,
            Owner::Output(at) => &mut self.outputs[at].channel,
        }
    }

    /// What a strip is called in a title, as its name field (and its plug-in windows' titles) show it: the track's
    /// name (or "Track 3"), the output's.
    fn label(&self, owner: Owner) -> String {
        match owner {
            Owner::Track(at) if self.tracks[at].name.is_empty() => format!("Track {}", at + 1),
            Owner::Track(at) => self.tracks[at].name.clone(),
            Owner::Output(at) => self.outputs[at].name.clone(),
        }
    }

    /// A plug-in as a change's target: its ref and its strip's.
    fn plugin_target(&self, owner: Owner, slot: usize) -> Value {
        json!({"ref": owner.plugin(self.epoch, slot), "kind": "plugin", "track": owner.text(self.epoch)})
    }

    fn send_target(&self, track: usize, slot: usize) -> Value {
        json!({"ref": format!("{}:send:{}.{slot}", self.epoch, track + 1), "kind": "send", "track": format!("{}:track:{}", self.epoch, track + 1)})
    }

    /// The track (from 0) at a place of an epoch: the epoch now, or (a restore's, `carry`) an earlier one, carried to
    /// where that track is now.
    fn track_in(&self, epoch: u32, place: usize, carry: bool, text: &str) -> Result<usize, Fault> {
        if epoch == self.epoch {
            return (place >= 1 && place <= self.tracks.len()).then(|| place - 1).ok_or_else(|| not_in_project(text));
        }
        if !carry || epoch > self.epoch {
            return Err(stale(epoch, self.epoch));
        }
        self.places
            .get(&epoch)
            .and_then(|ids| ids.get(place.wrapping_sub(1)))
            .and_then(|id| self.tracks.iter().position(|track| track.id == *id))
            .ok_or_else(|| gone(format!("the track of {text} isn't in the project any more (or that epoch is too far back)")))
    }

    /// The track a `track` ref names (`carry`: a restore's, from the change's epoch).
    fn track_for(&self, reference: Option<&str>, key: &str, carry: bool) -> Result<usize, Fault> {
        let text = reference.ok_or_else(|| bad(format!("{key} must be a track ref")))?;
        let e = self.epoch;
        let (epoch, kind, numbers) = parts(text).ok_or_else(|| bad(format!("{key} must be a track ref, such as {e}:track:1")))?;
        if !carry && epoch != self.epoch {
            return Err(stale(epoch, self.epoch));
        }
        if kind != "track" || numbers.len() != 1 || numbers[0] == 0 {
            return Err(bad(format!("{key} must be a track ref ({e}:track:1), not {text}")));
        }
        self.track_in(epoch, numbers[0], carry, text)
    }

    /// The strip a `track` or `output` ref names.
    fn owner_for(&self, reference: Option<&str>, key: &str, carry: bool) -> Result<Owner, Fault> {
        let text = reference.ok_or_else(|| bad(format!("{key} must be a track or output ref")))?;
        let e = self.epoch;
        let Some((epoch, kind, numbers)) = parts(text).filter(|(_, kind, _)| matches!(*kind, "track" | "output")) else {
            return Err(bad(format!(
                "{key} must be a track or output ref, such as {e}:track:1, or {e}:output:1 for the Stereo Out, not {text}"
            )));
        };
        if !carry && epoch != self.epoch {
            return Err(stale(epoch, self.epoch));
        }
        if numbers.len() != 1 || numbers[0] == 0 {
            return Err(bad(format!("{key} must be a track or output ref, not {text}")));
        }
        match kind {
            "output" if numbers[0] <= self.outputs.len() => Ok(Owner::Output(numbers[0] - 1)),
            "output" => Err(not_in_project(text)),
            _ => Ok(Owner::Track(self.track_in(epoch, numbers[0], carry, text)?)),
        }
    }

    /// A ref under a strip (`plugin`, `param`, `send`): the strip its numbers start with (`[track, …]`, or `[0, output,
    /// …]`) and the `rest` numbers after it.
    fn under(&self, reference: Option<&str>, key: &str, kind: &str, rest: usize, carry: bool) -> Result<(Owner, Vec<usize>), Fault> {
        let e = self.epoch;
        let text = reference.ok_or_else(|| bad(format!("{key} must be a {kind} ref")))?;
        let Some((epoch, _, numbers)) = parts(text).filter(|(_, named, _)| *named == kind) else {
            let example = match kind {
                "plugin" => format!("{e}:plugin:1.1, or {e}:plugin:0.1.1 on the Stereo Out"),
                "param" => format!("{e}:param:1.1.1"),
                _ => format!("{e}:send:1.1"),
            };
            return Err(bad(format!("{key} must be a {kind} ref, such as {example}, not {text}")));
        };
        if !carry && epoch != self.epoch {
            return Err(stale(epoch, self.epoch));
        }
        let malformed = || bad(format!("{text} isn't a ref Logic Pro's bridge gave"));
        let (owner, tail) = match numbers.first() {
            Some(0) if numbers.len() == rest + 2 && numbers[1] > 0 => {
                if numbers[1] > self.outputs.len() {
                    return Err(not_in_project(&format!("{e}:output:{}", numbers[1])));
                }
                (Owner::Output(numbers[1] - 1), &numbers[2..])
            }
            Some(place) if *place > 0 && numbers.len() == rest + 1 => {
                (Owner::Track(self.track_in(epoch, *place, carry, text)?), &numbers[1..])
            }
            _ => return Err(malformed()),
        };
        if tail.contains(&0) {
            return Err(malformed());
        }
        Ok((owner, tail.to_vec()))
    }

    /// The strip and slot (from 1) a `plugin` ref names.
    fn plugin_for(&self, reference: Option<&str>, key: &str, carry: bool) -> Result<(Owner, usize), Fault> {
        let (owner, tail) = self.under(reference, key, "plugin", 1, carry)?;
        Ok((owner, tail[0]))
    }

    /// The strip, plug-in slot and parameter (each from 1) a `param` ref names.
    fn param_for(&self, reference: Option<&str>, key: &str, carry: bool) -> Result<(Owner, usize, usize), Fault> {
        let (owner, tail) = self.under(reference, key, "param", 2, carry)?;
        Ok((owner, tail[0], tail[1]))
    }

    /// The track (from 0) and send slot (from 1) a `send` ref names: a track's, outputs have no sends.
    fn send_for(&self, reference: Option<&str>, key: &str, carry: bool) -> Result<(usize, usize), Fault> {
        match self.under(reference, key, "send", 1, carry)? {
            (Owner::Track(at), tail) => Ok((at, tail[0])),
            (Owner::Output(_), _) => Err(bad("an output strip has no sends")),
        }
    }

    /// The plug-in a ref names, as its window shows it: the plug-in, cloned.
    fn plugin_in(&self, owner: Owner, slot: usize) -> Result<Plugin, Fault> {
        self.channel(owner).at_slot(slot).map(|(plugin, _)| plugin.clone()).ok_or_else(|| not_in_project(&owner.plugin(self.epoch, slot)))
    }

    /// A preview's or an apply's refusal of a ref: one gone since the preview is `changed`.
    fn went(method: &str, fault: Fault) -> Fault {
        if method == "change_apply" && fault.0 == "failed" {
            changed(format!("{}: it went since the preview", fault.1))
        } else {
            fault
        }
    }

    /// A restore's refusal of a ref: what isn't there any more is `gone`.
    fn at_restore(fault: Fault) -> Fault {
        if fault.0 == "failed" {
            gone(fault.1)
        } else {
            fault
        }
    }

    // ----- the reads -----

    fn plugin_row(&self, owner: Owner, slot: usize) -> Value {
        let plugin = &self.channel(owner).plugins[slot - 1];
        alphabetical(json!({
            "bypassed": plugin.bypassed, "kind": plugin.kind(), "name": plugin.name, "ref": owner.plugin(RECORDED_EPOCH, slot),
            "slot": slot, "trackRef": owner.text(RECORDED_EPOCH),
        }))
    }

    fn param_row(&self, owner: Owner, slot: usize, at: usize) -> Value {
        let plugin = &self.channel(owner).plugins[slot - 1];
        let spec = &plugin.spec().params[at];
        let (value, text, min, max) = shown(spec, plugin.values[at]);
        alphabetical(json!({
            "index": at + 1, "max": max, "min": min, "name": spec.name, "normalized": normalized(value, min, max),
            "pluginRef": owner.plugin(RECORDED_EPOCH, slot), "ref": owner.param(RECORDED_EPOCH, slot, at + 1), "text": text, "value": value,
        }))
    }

    fn send_row(&self, track: usize, slot: usize) -> Value {
        let send = &self.tracks[track].channel.sends[slot - 1];
        alphabetical(json!({
            "destination": send.destination(), "level": send.level, "ref": format!("{RECORDED_EPOCH}:send:{}.{slot}", track + 1),
            "slot": slot, "trackRef": format!("{RECORDED_EPOCH}:track:{}", track + 1),
        }))
    }

    fn smart_row(&self, track: usize, at: usize) -> Value {
        let smart = &self.tracks[track].smart[at];
        alphabetical(json!({
            "index": at + 1, "name": smart.name, "ref": format!("{RECORDED_EPOCH}:smart:{}.{}", track + 1, at + 1), "text": smart.text(),
            "trackRef": format!("{RECORDED_EPOCH}:track:{}", track + 1), "value": round4(smart.value),
        }))
    }

    /// A track's automation as the read gives it: a row per parameter, in the order the list first names each, with its
    /// events.
    fn automation_rows(&self, track: usize) -> Vec<Value> {
        let mut groups: Vec<(String, u32, Vec<Value>)> = Vec::new();
        for event in &self.tracks[track].automation {
            let point = json!({"positionBeats": event.beats, "value": event.value});
            match groups.iter_mut().find(|(name, number, _)| *name == event.parameter && *number == event.number) {
                Some((_, _, points)) => points.push(point),
                None => groups.push((event.parameter.clone(), event.number, vec![point])),
            }
        }
        groups
            .into_iter()
            .enumerate()
            .map(|(at, (parameter, number, events))| {
                alphabetical(json!({
                    "events": events, "index": at + 1, "number": number, "parameter": parameter,
                    "ref": format!("{RECORDED_EPOCH}:automation:{}.{}", track + 1, at + 1), "trackRef": format!("{RECORDED_EPOCH}:track:{}", track + 1),
                }))
            })
            .collect()
    }

    /// Every plug-in row: the tracks' strips in order, then the outputs'.
    pub(super) fn plugin_rows(&self) -> Vec<Value> {
        let owners = (0..self.tracks.len()).map(Owner::Track).chain((0..self.outputs.len()).map(Owner::Output));
        owners
            .flat_map(|owner| (1..=self.channel(owner).plugins.len()).map(move |slot| (owner, slot)))
            .map(|(o, s)| self.plugin_row(o, s))
            .collect()
    }

    pub(super) fn send_rows(&self) -> Vec<Value> {
        (0..self.tracks.len())
            .flat_map(|track| (1..=self.tracks[track].channel.sends.len()).map(move |slot| (track, slot)))
            .map(|(track, slot)| self.send_row(track, slot))
            .collect()
    }

    /// A strip's inserts and sends as FOCUS's tree body shows them (the recorded shape).
    fn tree_body(&self, track: usize) -> Value {
        let owner = Owner::Track(track);
        let row = self.tracks[track].row(track + 1);
        let inserts: Vec<Value> = (1..=self.channel(owner).plugins.len())
            .map(|slot| {
                let mut insert = self.plugin_row(owner, slot);
                if let Some(map) = insert.as_object_mut() {
                    let reference = map.remove("ref").unwrap_or(Value::Null);
                    map.remove("trackRef");
                    map.insert("pluginRef".into(), reference);
                }
                alphabetical(insert)
            })
            .collect();
        alphabetical(json!({
            "epoch": RECORDED_EPOCH.to_string(), "kind": "tree", "trackRef": format!("{RECORDED_EPOCH}:track:{}", track + 1),
            "strip": {
                "armed": row["armed"], "inputs": row["inputs"], "inserts": inserts, "kind": row["kind"], "mute": row["mute"],
                "name": row["name"], "output": row["output"], "pan": row["pan"], "sends": self.body_sends(track), "solo": row["solo"],
                "volumeDb": row["volumeDb"],
            },
        }))
    }

    /// A strip's sends as the bodies show them: each with its `sendRef`.
    fn body_sends(&self, track: usize) -> Vec<Value> {
        (1..=self.tracks[track].channel.sends.len())
            .map(|slot| {
                let mut send = self.send_row(track, slot);
                if let Some(map) = send.as_object_mut() {
                    let reference = map.remove("ref").unwrap_or(Value::Null);
                    map.remove("trackRef");
                    map.insert("sendRef".into(), reference);
                }
                alphabetical(send)
            })
            .collect()
    }

    fn routing_body(&self, track: usize) -> Value {
        let row = self.tracks[track].row(track + 1);
        alphabetical(json!({
            "epoch": RECORDED_EPOCH.to_string(), "inputs": row["inputs"], "kind": "routing", "output": row["output"],
            "sends": self.body_sends(track), "trackRef": format!("{RECORDED_EPOCH}:track:{}", track + 1),
        }))
    }

    /// The reads of this slice, over the project as changed (`asked` in the recorded epoch), as the bridge answers them
    /// (`read.rs` and `changes::read_wide`): the strips' plug-ins and sends (under a parent, by its `trackRef`, or all of
    /// them), a plug-in's parameters, a track's Smart Controls and automation (each under its parent alone), one row by
    /// its ref, and the tree and routing bodies with the strip's inserts and sends. None for any other read.
    pub(super) fn mixer_rows(&self, method: &str, asked: &Value, params: &Value) -> Option<Answer> {
        let sent = |key: &str| params[key].as_str().unwrap_or_default().to_string();
        let one = |row: Value| Ok(json!({"epoch": RECORDED_EPOCH.to_string(), "items": [row]}));
        // (a ref in the recorded epoch, as `asked` has it; one from another epoch is left to the stale check)
        let recorded = |key: &str| -> Option<(u32, String, Vec<usize>)> {
            let (epoch, kind, numbers) = parts(asked.get(key)?.as_str()?)?;
            Some((epoch, kind.to_string(), numbers))
        };
        let kind = asked.get("kind").and_then(Value::as_str);
        Some(match method {
            // (the kinds the bridge's `read.rs` takes; a kind it hasn't, such as M4b's strip before the bridge's own
            // shapes, is refused as it refuses one)
            "read" if asked.get("ref").is_none() && kind.is_some_and(|kind| !READ_KINDS.contains(&kind)) => {
                Err(bad(format!("Logic Pro has no kind {:?} to read", kind.unwrap_or_default())))
            }
            "read" if asked.get("ref").is_some() => {
                let (epoch, ref_kind, numbers) = recorded("ref")?;
                if !matches!(ref_kind.as_str(), "plugin" | "param" | "send" | "smart" | "automation") {
                    return None;
                }
                if epoch != RECORDED_EPOCH {
                    return None;
                }
                // (the strip is found by the ref as it was sent, in the epoch of now: past the recorded one too)
                let sent_ref = sent("ref");
                let reference = sent_ref.as_str();
                let missing = || Err(not_in_project(&sent("ref")));
                match ref_kind.as_str() {
                    "plugin" => match self.under(Some(reference), "ref", "plugin", 1, false) {
                        Ok((owner, tail)) if tail[0] <= self.channel(owner).plugins.len() => one(self.plugin_row(owner, tail[0])),
                        _ => missing(),
                    },
                    "param" => match self.under(Some(reference), "ref", "param", 2, false) {
                        Ok((owner, tail))
                            if tail[0] <= self.channel(owner).plugins.len()
                                && tail[1] <= self.channel(owner).plugins[tail[0] - 1].param_count() =>
                        {
                            one(self.param_row(owner, tail[0], tail[1] - 1))
                        }
                        _ => missing(),
                    },
                    "send" => match self.under(Some(reference), "ref", "send", 1, false) {
                        Ok((Owner::Track(track), tail)) if tail[0] <= self.tracks[track].channel.sends.len() => {
                            one(self.send_row(track, tail[0]))
                        }
                        _ => missing(),
                    },
                    "smart" => match numbers.as_slice() {
                        [track, at]
                            if *track >= 1 && *track <= self.tracks.len() && *at >= 1 && *at <= self.tracks[track - 1].smart.len() =>
                        {
                            one(self.smart_row(track - 1, at - 1))
                        }
                        _ => missing(),
                    },
                    _ => match numbers.as_slice() {
                        [track, at] if *track >= 1 && *track <= self.tracks.len() => {
                            match self.automation_rows(track - 1).into_iter().nth(at.wrapping_sub(1)) {
                                Some(row) => one(row),
                                None => missing(),
                            }
                        }
                        _ => missing(),
                    },
                }
            }
            "read" if matches!(kind, Some("plugin" | "send")) => {
                if recorded("parent").is_some_and(|(epoch, _, _)| epoch != RECORDED_EPOCH) {
                    return None;
                }
                let rows = if kind == Some("plugin") { self.plugin_rows() } else { self.send_rows() };
                let rows = match asked.get("parent").and_then(Value::as_str) {
                    Some(parent) => rows.into_iter().filter(|row| row["trackRef"] == parent).collect(),
                    None => rows,
                };
                page_of(rows, asked)
            }
            "read" if matches!(kind, Some("param" | "smart" | "automation")) => {
                let kind = kind.unwrap_or_default();
                if asked.get("parent").and_then(Value::as_str).is_none() {
                    let what = if kind == "param" { "plug-in" } else { "track" };
                    return Some(Err(bad(format!("{kind}: pass the parent {what}"))));
                }
                if recorded("parent").is_some_and(|(epoch, _, _)| epoch != RECORDED_EPOCH) {
                    return None;
                }
                // (the parent is found as it was sent, in the epoch of now: past the recorded one too)
                let parent = sent("parent");
                if kind == "param" {
                    match self.plugin_for(Some(&parent), "parent", false).and_then(|(owner, slot)| {
                        self.channel(owner).at_slot(slot).map(|_| (owner, slot)).ok_or_else(|| not_in_project(&sent("parent")))
                    }) {
                        Ok((owner, slot)) => {
                            let count = self.channel(owner).plugins[slot - 1].param_count();
                            page_of((0..count).map(|at| self.param_row(owner, slot, at)).collect(), asked)
                        }
                        Err(fault) => Err(fault),
                    }
                } else {
                    match self.track_for(Some(&parent), "parent", false) {
                        Ok(track) if kind == "smart" => {
                            if self.tracks[track].smart.is_empty() {
                                Err(failed("Logic Pro's Smart Controls pane shows no controls for this track"))
                            } else {
                                page_of((0..self.tracks[track].smart.len()).map(|at| self.smart_row(track, at)).collect(), asked)
                            }
                        }
                        Ok(track) => page_of(self.automation_rows(track), asked),
                        Err(fault) => Err(fault),
                    }
                }
            }
            "focus_body" if matches!(kind, Some("tree" | "routing")) => {
                let track = match asked.get("ref").and_then(Value::as_str) {
                    // (the selected track, as recorded, when none is named)
                    None => self.tracks.iter().position(|track| track.row["selected"] == true).unwrap_or(0),
                    Some(reference) => match parts(reference) {
                        Some((epoch, _, _)) if epoch != RECORDED_EPOCH => return None,
                        Some((_, "track", numbers)) if numbers.len() == 1 && numbers[0] >= 1 && numbers[0] <= self.tracks.len() => {
                            numbers[0] - 1
                        }
                        Some((_, "track", _)) => return Some(Err(not_in_project(&sent("ref")))),
                        _ => return Some(Err(bad(format!("a {} body is a track's", kind.unwrap_or_default())))),
                    },
                };
                if track >= self.tracks.len() {
                    return Some(Err(("failed".into(), "no track is selected to show".into())));
                }
                Ok(if kind == Some("tree") { self.tree_body(track) } else { self.routing_body(track) })
            }
            _ => return None,
        })
    }

    // ----- plug-ins: put on a strip, taken off it, bypassed -----

    /// `fx-add {ref, name, index?}`: the Mixer's empty slot under the strip's last effect (or the insert bar along the
    /// top of the effect at `index`, from 0), its menu's search typed and its first result taken, read back from the
    /// slot. A step of Logic's undo ("Insert Plug-in in Channel Strip", or "Change Plug-in in Channel Strip" through an
    /// insert bar); the restore takes it off again, by value.
    pub(super) fn fx_add(&mut self, method: &str, params: &Value) -> Answer {
        if method == "change_restore" {
            return self.fx_add_restore(params);
        }
        let input = &params["input"];
        let owner = self.owner_for(input["ref"].as_str(), "ref", false).map_err(|fault| Self::went(method, fault))?;
        let name = plugin_name(input)?;
        let (label, channel) = (self.label(owner), self.channel(owner).clone());
        let count = channel.effect_names().len();
        let index = index_of(input, count)?;
        let fence = channel.fence();
        let target = json!({"ref": owner.text(self.epoch), "kind": owner.kind()});
        if method == "change_preview" {
            let display = json!({"title": format!("Added {} to {}", quoted(&name), quoted(&label)), "index": index});
            return Ok(json!({"epoch": self.epoch_text(), "target": target, "prior": fence, "fence": fence, "display": display}));
        }
        if params["fence"] != fence {
            return Err(changed(format!("the plug-ins on {} changed since the preview", quoted(&label))));
        }
        if !name.chars().any(|c| c.is_ascii_alphanumeric()) {
            return Err(bad(format!("{} doesn't start with anything Kumi can type into Logic Pro's plug-in search", quoted(&name))));
        }
        if count >= MOST_EFFECTS {
            return Err(failed("Logic Pro's strip shows no free slot there (its slots are all taken)"));
        }
        let desc = Self::desc(params)?;
        let step = if index == count { INSERTED } else { CHANGED_SLOT };
        let at = channel.first_effect() + index;
        let Some(spec) = searched(&name) else {
            // The search found nothing: Down and Return took nothing, and the strip is as it was.
            return Err(uncertain(format!(
                "Logic Pro's strip {} shows {:?} after Kumi asked for {} at place {}",
                quoted(&label),
                channel.effect_names(),
                quoted(&name),
                index + 1
            )));
        };
        if !same_name(spec.name, &name) {
            // Logic's search took another plug-in: off it comes again, and nothing is left changed (but the two steps).
            for taken in [step, REMOVED] {
                self.undo.push(UndoStep { title: format!("Undo {taken}"), desc: None, key: None });
            }
            return Err(failed(format!(
                "Logic Pro's plug-in search for {} took {}, so Kumi took it off again: give the name as Logic Pro's plug-in menu shows it",
                quoted(&name),
                quoted(spec.name)
            )));
        }
        self.channel_mut(owner).plugins.insert(at, Plugin::new(spec));
        let undo = self.applied(Some(step), &desc)?;
        let slot = at + 1;
        Ok(json!({
            "epoch": self.epoch_text(),
            "applied": {"index": index, "name": spec.name, "slot": slot, "plugins": self.channel(owner).effect_names()},
            "created": {"ref": owner.plugin(self.epoch, slot), "kind": "plugin"},
            "display": {"title": format!("Added {} to {}", quoted(spec.name), quoted(&label)), "index": index},
            "undo": undo,
        }))
    }

    fn fx_add_restore(&mut self, params: &Value) -> Answer {
        let desc = Self::desc(params)?;
        let created =
            params.get("created").filter(|created| !created.is_null()).ok_or_else(|| bad("created: the plug-in that was added"))?;
        let (owner, slot) = self.plugin_for(created["ref"].as_str(), "ref", true).map_err(Self::at_restore)?;
        let name = params["applied"]["name"].as_str().unwrap_or_default().to_string();
        let label = self.label(owner);
        let channel = self.channel(owner);
        if !channel.effect_names().iter().any(|there| same_name(there, &name)) {
            let undo = self.restored(None, &desc)?;
            return Ok(json!({
                "epoch": self.epoch_text(),
                "restored": {"deleted": created["ref"]},
                "display": {"title": format!("{} is off {} already", quoted(&name), quoted(&label))},
                "undo": undo,
            }));
        }
        let at = channel.effect_named(slot, &name).ok_or_else(|| {
            changed(format!("the plug-ins on {} have been rearranged in Logic Pro since Kumi added {}", quoted(&label), quoted(&name)))
        })?;
        self.channel_mut(owner).plugins.remove(at);
        let undo = self.restored(Some(REMOVED), &desc)?;
        Ok(json!({
            "epoch": self.epoch_text(),
            "restored": {"deleted": created["ref"], "plugins": self.channel(owner).effect_names()},
            "display": {"title": format!("{} taken off {}", quoted(&name), quoted(&label))},
            "undo": undo,
        }))
    }

    /// `fx-delete {ref}`: the plug-in's list button pressed and No Plug-in taken; an instrument isn't taken off. A step
    /// of Logic's undo ("Remove Plug-in in Channel Strip"); the restore is Logic's own undo while that step is on top
    /// (the plug-in back with its settings), else the plug-in inserted again by its name at its place (with Logic's
    /// defaults).
    pub(super) fn fx_delete(&mut self, method: &str, params: &Value) -> Answer {
        if method == "change_restore" {
            return self.fx_delete_restore(params);
        }
        let input = &params["input"];
        let (owner, slot) = self.plugin_for(input["ref"].as_str(), "ref", false).map_err(|fault| Self::went(method, fault))?;
        let label = self.label(owner);
        let channel = self.channel(owner).clone();
        let (plugin, instrument) =
            channel.at_slot(slot).ok_or_else(|| Self::went(method, not_in_project(&owner.plugin(self.epoch, slot))))?;
        if instrument {
            return Err(bad(format!(
                "{} is the instrument of {}: Kumi takes effects off a strip, not its instrument (load another patch instead)",
                quoted(&plugin.name),
                quoted(&label)
            )));
        }
        let name = plugin.name.clone();
        let fence = channel.fence();
        let title = format!("Deleted {} from {}", quoted(&name), quoted(&label));
        if method == "change_preview" {
            return Ok(json!({
                "epoch": self.epoch_text(),
                "target": self.plugin_target(owner, slot),
                "prior": {"name": name, "slot": slot},
                "fence": fence,
                "display": {"title": title, "note": DELETE_NOTE},
            }));
        }
        if params["fence"] != fence {
            return Err(changed(format!("the plug-ins on {} changed since the preview", quoted(&label))));
        }
        let desc = Self::desc(params)?;
        let index = slot - 1 - channel.first_effect();
        let removed = self.channel_mut(owner).plugins.remove(slot - 1);
        self.removed_plugins.insert(desc.clone(), removed);
        let undo = self.applied(Some(REMOVED), &desc)?;
        Ok(json!({
            "epoch": self.epoch_text(),
            "applied": {"deleted": true, "plugins": self.channel(owner).effect_names()},
            "material": {"name": name, "index": index, "strip": owner.text(self.epoch), "step": desc},
            "display": {"title": title},
            "undo": undo,
        }))
    }

    fn fx_delete_restore(&mut self, params: &Value) -> Answer {
        let desc = Self::desc(params)?;
        let material =
            params.get("material").filter(|material| !material.is_null()).ok_or_else(|| bad("material: what the delete took off"))?;
        let name = material["name"].as_str().unwrap_or_default().to_string();
        let index = material["index"].as_u64().unwrap_or(0) as usize;
        let (owner, _) = self.plugin_for(params["target"]["ref"].as_str(), "ref", true).map_err(Self::at_restore)?;
        let label = self.label(owner);
        let (first, effects) = (self.channel(owner).first_effect(), self.channel(owner).effect_names());
        if effects.get(index).is_some_and(|there| same_name(there, &name)) {
            let undo = self.restored(None, &desc)?;
            return Ok(json!({
                "epoch": self.epoch_text(),
                "restored": {"ref": owner.plugin(self.epoch, first + index + 1)},
                "display": {"title": format!("{} is back on {} already", quoted(&name), quoted(&label))},
                "undo": undo,
            }));
        }
        // Logic's own undo first, while the deletion is its latest step: it brings the plug-in back with its settings.
        let key = Self::key_of(params);
        let step = material["step"].as_str().unwrap_or_default().to_string();
        let ours = |top: &UndoStep| top.key.as_deref() == Some(key.as_str()) && top.desc.as_deref() == Some(step.as_str());
        if let Ok(UndoStep { title: pressed, .. }) = self.undo_in_logic(REMOVED, ours) {
            if self.restore_unanswered() {
                return Err(uncertain(format!(
                    "Logic Pro's {pressed:?} was pressed, but {} isn't back on {}",
                    quoted(&name),
                    quoted(&label)
                )));
            }
            let plugin = self.removed_plugins.remove(&step).unwrap_or_else(|| Plugin::named(&name));
            let at = first + index.min(effects.len());
            self.channel_mut(owner).plugins.insert(at, plugin);
            return Ok(json!({
                "epoch": self.epoch_text(),
                "restored": {"ref": owner.plugin(self.epoch, at + 1), "pressed": pressed},
                "display": {"title": format!("{} put back on {}", quoted(&name), quoted(&label)), "note": "Put back with Logic Pro's own undo, with its settings."},
                "undo": {"desc": desc, "entry": false, "how": "undo"},
            }));
        }
        let spec = searched(&name)
            .filter(|spec| same_name(spec.name, &name))
            .ok_or_else(|| failed(format!("Logic Pro's plug-in search finds no {} to put back", quoted(&name))))?;
        let place = index.min(effects.len());
        self.channel_mut(owner).plugins.insert(first + place, Plugin::new(spec));
        let undo = self.restored(Some(if place == effects.len() { INSERTED } else { CHANGED_SLOT }), &desc)?;
        Ok(json!({
            "epoch": self.epoch_text(),
            "restored": {"ref": owner.plugin(self.epoch, first + place + 1), "plugins": self.channel(owner).effect_names()},
            "display": {
                "title": format!("{} put back on {}", quoted(&name), quoted(&label)),
                "note": "Put back by name, with Logic Pro's default settings: Logic Pro's own undo had other steps on top.",
            },
            "undo": undo,
        }))
    }

    /// `fx-bypass {ref, bypass}`: the Mixer strip slot's bypass, as the bridge makes it (`changes/fx.rs`, a track's
    /// plug-in): its target names the plug-in and its track, its state is `{bypassed}`, and Logic keeps no step of it.
    /// Its restore presses it back, by value.
    pub(super) fn fx_bypass(&mut self, method: &str, params: &Value) -> Answer {
        let title_of = |plugin: &str, track: &str, bypassed: bool| {
            format!("{} {} on {}", if bypassed { "Bypassed" } else { "Enabled" }, quoted(plugin), quoted(track))
        };
        let restore = method == "change_restore";
        let reference = if restore { params["target"]["ref"].as_str() } else { params["input"]["ref"].as_str() };
        // (a track's plug-in alone: the bridge's bypass reads two numbers)
        let e = self.epoch;
        let text = reference.ok_or_else(|| bad("ref must be a plugin ref"))?;
        let (epoch, kind, numbers) = parts(text).ok_or_else(|| bad(format!("ref must be a plugin ref, such as {e}:plugin:1.1")))?;
        if !restore && epoch != self.epoch {
            return Err(stale(epoch, self.epoch));
        }
        if kind != "plugin" || numbers.len() != 2 || numbers.contains(&0) {
            return Err(bad(format!("ref must be a plugin ref ({e}:plugin:1.1), not {text}")));
        }
        let track =
            self.track_in(epoch, numbers[0], restore, text).map_err(|fault| if restore { Self::at_restore(fault) } else { fault })?;
        let (owner, slot) = (Owner::Track(track), numbers[1]);
        let plugin = self.plugin_in(owner, slot).map_err(|fault| if restore { Self::at_restore(fault) } else { fault })?;
        let label = self.label(owner);
        if restore {
            let desc = Self::desc(params)?;
            let (prior, applied) = (params["prior"]["bypassed"] == true, params["applied"]["bypassed"] == true);
            if plugin.bypassed != prior {
                if params["discard"] != true && plugin.bypassed != applied {
                    return Err(changed(format!("{} has been switched in Logic Pro since Kumi did", quoted(&plugin.name))));
                }
                self.channel_mut(owner).plugins[slot - 1].bypassed = prior;
            }
            let undo = self.restored(None, &desc)?;
            let title = format!("{} (put back)", title_of(&plugin.name, &label, prior));
            return Ok(json!({"epoch": self.epoch_text(), "restored": {"bypassed": prior}, "display": {"title": title}, "undo": undo}));
        }
        let input = &params["input"];
        let bypass = input.get("bypass").and_then(Value::as_bool).ok_or_else(|| bad("bypass must be true or false"))?;
        let now = json!({"bypassed": plugin.bypassed});
        if method == "change_preview" {
            let mut display = json!({"title": title_of(&plugin.name, &label, bypass), "before": now, "after": {"bypassed": bypass}});
            if plugin.bypassed == bypass {
                display["unchanged"] = true.into();
            }
            return Ok(
                json!({"epoch": self.epoch_text(), "target": self.plugin_target(owner, slot), "prior": now, "fence": now, "display": display}),
            );
        }
        if params["fence"] != now {
            return Err(changed(format!("{} was switched in Logic Pro since the preview", quoted(&plugin.name))));
        }
        let desc = Self::desc(params)?;
        self.channel_mut(owner).plugins[slot - 1].bypassed = bypass;
        let undo = self.applied(None, &desc)?;
        Ok(json!({
            "epoch": self.epoch_text(),
            "applied": {"bypassed": bypass},
            "display": {"title": title_of(&plugin.name, &label, bypass), "before": now, "after": {"bypassed": bypass}},
            "undo": undo,
        }))
    }

    // ----- a plug-in's parameters and settings, in its window -----

    /// `fx-param {ref, param?, normalized | value}` (a param ref, or a plug-in's with `param`: its name or place) and
    /// `fx-params {ref, values}`: the plug-in's window opened from its slot, its Controls View, each row set (a slider's
    /// raw value, a choice's option, a switch) and read back, the window closed. No step of Logic's project undo (the
    /// window's own undo has them): the restore sets each back, by value.
    pub(super) fn fx_param(&mut self, method: &str, params: &Value) -> Answer {
        self.fx_params_of(method, params, false)
    }

    pub(super) fn fx_params(&mut self, method: &str, params: &Value) -> Answer {
        self.fx_params_of(method, params, true)
    }

    fn fx_params_of(&mut self, method: &str, params: &Value, many: bool) -> Answer {
        if method == "change_restore" {
            return self.fx_params_restore(params);
        }
        let input = &params["input"];
        let reference = input["ref"].as_str();
        let (owner, slot, from_ref) = match reference.and_then(parts) {
            Some((_, "param", _)) => {
                let (owner, slot, index) = self.param_for(reference, "ref", false).map_err(|fault| Self::went(method, fault))?;
                (owner, slot, Some(index))
            }
            _ => {
                let (owner, slot) = self.plugin_for(reference, "ref", false).map_err(|fault| Self::went(method, fault))?;
                (owner, slot, None)
            }
        };
        let asks = asks(input, from_ref, many)?;
        let plugin = self.plugin_in(owner, slot).map_err(|fault| Self::went(method, fault))?;
        let label = self.label(owner);
        let chosen = named(&plugin, &asks)?;
        let prior: Vec<Value> = chosen.iter().map(|at| plugin.state(*at)).collect();
        let fence = json!({"plugin": plugin.name, "params": prior});
        if method == "change_preview" {
            for ((_, wanted), at) in asks.iter().zip(&chosen) {
                let spec = &plugin.spec().params[*at];
                if let (Wanted::Option(_), Control::Slider { .. }) = (wanted, spec.control) {
                    return Err(bad(format!("{} is a slider: give normalized or a number", spec.name)));
                }
            }
            let what = match chosen.as_slice() {
                [one] => plugin.spec().params[*one].name.to_string(),
                many => format!("{} parameters", many.len()),
            };
            return Ok(json!({
                "epoch": self.epoch_text(),
                "target": self.plugin_target(owner, slot),
                "prior": {"plugin": plugin.name, "params": prior},
                "fence": fence,
                "display": {
                    "title": format!("{} {what} on {}", quoted(&plugin.name), quoted(&label)),
                    "params": chosen.iter().map(|at| plugin.spec().params[*at].name).collect::<Vec<_>>(),
                    "note": PARAM_NOTE,
                },
            }));
        }
        if params["fence"] != fence {
            return Err(changed(format!("{} changed in Logic Pro since the preview", quoted(&plugin.name))));
        }
        let desc = Self::desc(params)?;
        // Every value is found before any is set: nothing half-done stays.
        let raws: Vec<f64> = asks.iter().zip(&chosen).map(|((_, wanted), at)| raw_for(&plugin, *at, wanted)).collect::<Result<_, _>>()?;
        let mut after = Vec::new();
        for (at, raw) in chosen.iter().zip(raws) {
            let plugin = &mut self.channel_mut(owner).plugins[slot - 1];
            plugin.set(*at, raw);
            after.push(plugin.state(*at));
        }
        let undo = self.applied(None, &desc)?;
        let lines: Vec<String> = after
            .iter()
            .map(|p| format!("{} → {}", p["name"].as_str().unwrap_or_default(), p["text"].as_str().unwrap_or_default()))
            .collect();
        Ok(json!({
            "epoch": self.epoch_text(),
            "applied": {"plugin": plugin.name, "params": after},
            "display": {"title": params_title(&plugin.name, &label, &after), "lines": lines},
            "undo": undo,
        }))
    }

    fn fx_params_restore(&mut self, params: &Value) -> Answer {
        let desc = Self::desc(params)?;
        let (owner, slot) = self.plugin_for(params["target"]["ref"].as_str(), "ref", true).map_err(Self::at_restore)?;
        let prior: Vec<Value> = params["prior"]["params"].as_array().cloned().ok_or_else(|| bad("prior.params"))?;
        let applied: Vec<Value> = params["applied"]["params"].as_array().cloned().unwrap_or_default();
        let name = params["prior"]["plugin"].as_str().unwrap_or_default().to_string();
        let label = self.label(owner);
        let plugin = self.plugin_in(owner, slot)?;
        if plugin.name != name {
            return Err(changed(format!("the slot holds {} now, not {}", quoted(&plugin.name), quoted(&name))));
        }
        let mut back = Vec::new();
        for was in &prior {
            let index = was["index"].as_u64().unwrap_or(0) as usize;
            let plugin = &self.channel(owner).plugins[slot - 1];
            if index == 0 || index > plugin.param_count() {
                return Err(failed(format!("{} has no parameter {index} in its Controls View", quoted(&name))));
            }
            let now = plugin.state(index - 1);
            if now["name"] != was["name"] {
                return Err(changed(format!("{}'s parameters aren't as Kumi left them", quoted(&name))));
            }
            if same_value(&now, was) {
                back.push(now);
                continue;
            }
            let kumi = applied.iter().find(|a| a["index"] == was["index"]);
            if params["discard"] != true && !kumi.is_some_and(|a| same_value(&now, a)) {
                return Err(changed(format!(
                    "{}'s {} has been changed in Logic Pro since Kumi set it",
                    quoted(&name),
                    now["name"].as_str().unwrap_or_default()
                )));
            }
            let raw = was["value"]
                .as_f64()
                .ok_or_else(|| failed(format!("{} was a menu Kumi can't set back", now["name"].as_str().unwrap_or_default())))?;
            let plugin = &mut self.channel_mut(owner).plugins[slot - 1];
            plugin.set(index - 1, raw);
            back.push(plugin.state(index - 1));
        }
        let undo = self.restored(None, &desc)?;
        Ok(json!({
            "epoch": self.epoch_text(),
            "restored": {"plugin": name, "params": back},
            "display": {"title": format!("{} (put back)", params_title(&name, &label, &back))},
            "undo": undo,
        }))
    }

    /// `fx-preset {ref, preset | step}`: the plug-in window's next or previous setting button, `step` times, or next
    /// until the setting pop-up names `preset` (its menu is Logic's own, which Accessibility can't read). Steps of the
    /// window's own undo, never of the project's: the restore presses the window's Undo as many times.
    pub(super) fn fx_preset(&mut self, method: &str, params: &Value) -> Answer {
        if method == "change_restore" {
            return self.fx_preset_restore(params);
        }
        let input = &params["input"];
        let (owner, slot) = self.plugin_for(input["ref"].as_str(), "ref", false).map_err(|fault| Self::went(method, fault))?;
        let ask = ask_of(input)?;
        let plugin = self.plugin_in(owner, slot).map_err(|fault| Self::went(method, fault))?;
        let label = self.label(owner);
        let before = plugin.preset.clone();
        let state = json!({"plugin": plugin.name, "preset": before});
        if method == "change_preview" {
            let what = match &ask {
                Ask::Named(name) => quoted(name),
                Ask::Step(1) => "the next setting".into(),
                Ask::Step(-1) => "the previous setting".into(),
                Ask::Step(n) if *n > 0 => format!("the setting {n} on"),
                Ask::Step(n) => format!("the setting {} back", -n),
            };
            let mut display = json!({"title": format!("{} on {}: {what}", quoted(&plugin.name), quoted(&label)), "before": before});
            if matches!(&ask, Ask::Named(name) if *name == before) {
                display["unchanged"] = true.into();
            }
            return Ok(
                json!({"epoch": self.epoch_text(), "target": self.plugin_target(owner, slot), "prior": state, "fence": state, "display": display}),
            );
        }
        if params["fence"] != state {
            return Err(changed(format!("{}'s setting changed in Logic Pro since the preview", quoted(&plugin.name))));
        }
        let desc = Self::desc(params)?;
        let mut now = before.clone();
        let mut steps = 0usize;
        let presses_back = |state: &mut State, steps: usize| {
            for _ in 0..steps {
                state.channel_mut(owner).plugins[slot - 1].window_undo();
            }
        };
        match &ask {
            Ask::Named(name) if *name == before => {}
            Ask::Named(name) => {
                while now != *name {
                    if steps >= MOST_PRESSES {
                        presses_back(self, steps);
                        return Err(failed(format!(
                            "{} isn't among the next {MOST_PRESSES} settings of {} (its setting menu, which Kumi can't read, lists them all)",
                            quoted(name),
                            quoted(&plugin.name)
                        )));
                    }
                    let was = now.clone();
                    now = self.channel_mut(owner).plugins[slot - 1].press(true);
                    steps += 1;
                    if now == was {
                        presses_back(self, steps);
                        return Err(uncertain(format!(
                            "{}'s setting stayed {} after Kumi pressed next",
                            quoted(&plugin.name),
                            quoted(&was)
                        )));
                    }
                }
            }
            Ask::Step(n) => {
                for _ in 0..n.unsigned_abs() {
                    now = self.channel_mut(owner).plugins[slot - 1].press(*n > 0);
                    steps += 1;
                }
                if now == before {
                    presses_back(self, steps);
                    return Err(uncertain(format!(
                        "{}'s setting is still {} after Kumi stepped it",
                        quoted(&plugin.name),
                        quoted(&before)
                    )));
                }
            }
        }
        let undo = self.applied(None, &desc)?;
        let mut display = json!({"title": format!("{} on {} set to {}", quoted(&plugin.name), quoted(&label), quoted(&now)), "before": before, "after": now});
        if steps == 0 {
            display["unchanged"] = true.into();
        }
        Ok(json!({
            "epoch": self.epoch_text(),
            "applied": {"plugin": plugin.name, "preset": now, "steps": steps},
            "material": {"steps": steps},
            "display": display,
            "undo": undo,
        }))
    }

    fn fx_preset_restore(&mut self, params: &Value) -> Answer {
        let desc = Self::desc(params)?;
        let (owner, slot) = self.plugin_for(params["target"]["ref"].as_str(), "ref", true).map_err(Self::at_restore)?;
        let prior = params["prior"]["preset"].as_str().unwrap_or_default().to_string();
        let applied = params["applied"]["preset"].as_str().unwrap_or_default().to_string();
        let steps = params["material"]["steps"].as_u64().unwrap_or(0) as usize;
        let plugin = self.plugin_in(owner, slot)?;
        if plugin.preset == prior {
            let undo = self.restored(None, &desc)?;
            return Ok(json!({
                "epoch": self.epoch_text(),
                "restored": {"preset": plugin.preset},
                "display": {"title": format!("{} is on {} already", quoted(&plugin.name), quoted(&plugin.preset))},
                "undo": undo,
            }));
        }
        if params["discard"] != true && plugin.preset != applied {
            return Err(changed(format!("{}'s setting has been changed in Logic Pro since Kumi set it", quoted(&plugin.name))));
        }
        for _ in 0..steps {
            self.channel_mut(owner).plugins[slot - 1].window_undo();
        }
        let after = self.channel(owner).plugins[slot - 1].preset.clone();
        if after != prior {
            return Err(uncertain(format!(
                "{}'s window's own undo shows {} after Kumi pressed it {steps} time(s), not {}",
                quoted(&plugin.name),
                quoted(&after),
                quoted(&prior)
            )));
        }
        let undo = self.restored(None, &desc)?;
        Ok(json!({
            "epoch": self.epoch_text(),
            "restored": {"preset": after},
            "display": {"title": format!("{} back on {}", quoted(&plugin.name), quoted(&after)), "note": "Put back with the plug-in window's own undo."},
            "undo": undo,
        }))
    }

    // ----- a patch from the Library -----

    /// The strip a patch fences and answers: the name its setting button shows, and its plug-ins (the instrument first).
    fn patch_state(&self, track: usize) -> Value {
        let channel = &self.tracks[track].channel;
        json!({"patch": channel.patch, "plugins": channel.names()})
    }

    /// `patch-load {ref, patch}`: the track selected, the Library (Y): its search set to the patch's name, the result's
    /// row selected (the Library shows a track the patches of its kind), the search cleared and the Library closed; read
    /// back from the Inspector's strip. A step of Logic's undo ("Load Patch"); the restore is Logic's own undo while it
    /// is on top, and nothing by value (a patch replaces the whole strip: its plug-ins, sends and Smart Controls).
    pub(super) fn patch_load(&mut self, method: &str, params: &Value) -> Answer {
        if method == "change_restore" {
            return self.patch_restore(params);
        }
        let input = &params["input"];
        let track = self.track_for(input["ref"].as_str(), "ref", false).map_err(|fault| Self::went(method, fault))?;
        let name = patch_of(input)?;
        let label = self.label(Owner::Track(track));
        let now = self.patch_state(track);
        let title = format!("Loaded the patch {} on {}", quoted(&name), quoted(&label));
        if method == "change_preview" {
            return Ok(json!({
                "epoch": self.epoch_text(),
                "target": {"ref": format!("{}:track:{}", self.epoch, track + 1), "kind": "track"},
                "prior": now,
                "fence": now,
                "display": {"title": title, "before": now, "note": PATCH_NOTE},
            }));
        }
        if params["fence"] != now {
            return Err(changed(format!("{}'s channel strip changed since the preview", quoted(&label))));
        }
        let desc = Self::desc(params)?;
        let kind = self.tracks[track].kind.clone();
        let found: Vec<&PatchSpec> =
            PATCHES.iter().filter(|patch| patch.track_kind == kind && patch.name().eq_ignore_ascii_case(&name)).collect();
        let patch = match found.as_slice() {
            [] => return Err(failed(format!("Logic Pro's Library finds no patch named {} for this track", quoted(&name)))),
            [one] => *one,
            many => {
                return Err(bad(format!(
                    "Logic Pro's Library has {} patches named {}: Kumi can't tell them apart",
                    many.len(),
                    quoted(&name)
                )))
            }
        };
        let before = (self.tracks[track].channel.clone(), self.tracks[track].smart.clone());
        self.loaded_patches.insert(desc.clone(), before);
        self.tracks[track].channel = Channel::of_patch(patch);
        self.tracks[track].smart = smart_of(patch);
        let undo = self.applied(Some(PATCH_STEP), &desc)?;
        let after = self.patch_state(track);
        let mut material = now.clone();
        material["step"] = desc.into();
        Ok(json!({
            "epoch": self.epoch_text(),
            "applied": after,
            "material": material,
            "display": {"title": title, "before": now, "after": after},
            "undo": undo,
        }))
    }

    fn patch_restore(&mut self, params: &Value) -> Answer {
        let desc = Self::desc(params)?;
        let track = self.track_for(params["target"]["ref"].as_str(), "ref", true).map_err(Self::at_restore)?;
        let label = self.label(Owner::Track(track));
        let now = self.patch_state(track);
        let prior = &params["prior"];
        if now["patch"] == prior["patch"] && now["plugins"] == prior["plugins"] {
            let undo = self.restored(None, &desc)?;
            return Ok(json!({
                "epoch": self.epoch_text(),
                "restored": now,
                "display": {"title": format!("{}'s channel strip is as it was", quoted(&label))},
                "undo": undo,
            }));
        }
        if params["discard"] != true && now != params["applied"] {
            return Err(changed(format!("{}'s channel strip has been changed in Logic Pro since Kumi loaded the patch", quoted(&label))));
        }
        let key = Self::key_of(params);
        let step = params["material"]["step"].as_str().unwrap_or_default().to_string();
        let ours = |top: &UndoStep| top.key.as_deref() == Some(key.as_str()) && top.desc.as_deref() == Some(step.as_str());
        let Ok(UndoStep { title: pressed, .. }) = self.undo_in_logic(PATCH_STEP, ours) else {
            return Err(failed(
                "Logic Pro's undo has other steps on top of the patch load, so Kumi can't put the strip back (a patch replaces the whole strip): Cmd-Z in Logic Pro can, step by step",
            ));
        };
        if self.restore_unanswered() {
            return Err(uncertain(format!("Logic Pro's {pressed:?} was pressed, but {}'s strip isn't as it was", quoted(&label))));
        }
        let (channel, smart) = self.loaded_patches.remove(&step).unwrap_or_else(|| {
            let plugins = prior["plugins"].as_array().into_iter().flatten().filter_map(Value::as_str).map(Plugin::named).collect();
            (Channel { plugins, sends: vec![], patch: prior["patch"].as_str().unwrap_or_default().into() }, vec![])
        });
        self.tracks[track].channel = channel;
        self.tracks[track].smart = smart;
        let after = self.patch_state(track);
        Ok(json!({
            "epoch": self.epoch_text(),
            "restored": {"patch": after["patch"], "plugins": after["plugins"], "pressed": pressed},
            "display": {"title": format!("{}'s channel strip put back", quoted(&label)), "note": "Put back with Logic Pro's own undo."},
            "undo": {"desc": desc, "entry": false, "how": "undo"},
        }))
    }

    // ----- sends -----

    /// Whether an aux (or a summing Track Stack's own track) takes a bus.
    fn bus_taken(&self, bus: usize) -> bool {
        let input = json!(format!("Bus {bus}"));
        self.auxes.iter().any(|aux| aux.bus == bus)
            || self
                .tracks
                .iter()
                .any(|track| track.is_stack() && track.row["inputs"].as_array().is_some_and(|inputs| inputs.contains(&input)))
    }

    /// The name Logic gives the next aux it makes: its aux strips counted (a summing stack's among them).
    fn next_aux_name(&self) -> String {
        let stacks = self.tracks.iter().filter(|track| track.is_stack() && track.kind == "aux").count();
        format!("Aux {}", self.auxes.len() + stacks + 1)
    }

    /// The send a ref names, as the strip shows it: its track (from 0), slot (from 1), the track's label and the send.
    fn found_send(&self, reference: Option<&str>, carry: bool) -> Result<(usize, usize, String, Send), Fault> {
        let (track, slot) = self.send_for(reference, "ref", carry)?;
        let send = self.tracks[track].channel.sends.get(slot - 1).cloned().ok_or_else(|| not_in_project(reference.unwrap_or_default()))?;
        Ok((track, slot, self.label(Owner::Track(track)), send))
    }

    /// `send-add {ref, bus | toRef, volumeDb?}`: the track's strip's empty send slot pressed and Logic's send menu walked
    /// by keys to the bus (Logic making an aux for a bus nothing takes yet), the level then stepped as `send-volume`;
    /// Logic makes a send at −∞. Its mode can't be chosen from outside. A step of Logic's undo ("Change Send in Channel
    /// Strip"); the restore is Logic's own undo while on top (the aux goes with it), else the send taken off by value
    /// (the aux stays in the Mixer).
    pub(super) fn send_add(&mut self, method: &str, params: &Value) -> Answer {
        if method == "change_restore" {
            return self.send_add_restore(params);
        }
        let desc = if method == "change_apply" { Some(Self::desc(params)?) } else { None };
        let mut answer = self.send_add_of(method, &params["input"], &params["fence"])?;
        if let Some(desc) = desc {
            answer["undo"] = self.applied(Some(SEND_STEP), &desc)?;
        }
        Ok(answer)
    }

    /// A send made (or, for a preview, what it would be): the answer but its `undo`.
    fn send_add_of(&mut self, method: &str, input: &Value, fence_sent: &Value) -> Answer {
        let owner = self.owner_for(input["ref"].as_str(), "ref", false).map_err(|fault| Self::went(method, fault))?;
        let Owner::Track(track) = owner else { return Err(bad("a send goes from a track's strip: an output has none")) };
        let bus = bus_of(input)? as usize;
        let level = level_of(input)?;
        if method == "change_preview" && input.get("mode").is_some_and(|mode| !mode.is_null()) {
            return Err(bad("Logic Pro shows a send's mode only as a mark in its menu, which Kumi can't read back: leave mode out"));
        }
        let label = self.label(owner);
        let fence = json!({"sends": self.tracks[track].channel.busses()});
        let wanted = format!("Bus {bus}");
        if method == "change_preview" {
            let mut display = json!({"title": format!("Added send from {} to {wanted}", quoted(&label))});
            match level {
                Some(db) => display["level"] = db_text(db).into(),
                None => display["note"] = NEW_SEND_NOTE.into(),
            }
            return Ok(
                json!({"epoch": self.epoch_text(), "target": {"ref": owner.text(self.epoch), "kind": "track"}, "prior": fence, "fence": fence, "display": display}),
            );
        }
        if *fence_sent != fence {
            return Err(changed(format!("the sends of {} changed since the preview", quoted(&label))));
        }
        if self.tracks[track].channel.sends.len() >= MOST_SENDS {
            return Err(failed(format!("Logic Pro's strip {} shows no free send slot", quoted(&label))));
        }
        let aux = (!self.bus_taken(bus)).then(|| self.next_aux_name());
        if let Some(name) = &aux {
            self.auxes.push(Aux { name: name.clone(), bus });
        }
        let reached = level.map_or(MIN_DB, knob_db);
        self.tracks[track].channel.sends.push(Send { bus, level: reached });
        let slot = self.tracks[track].channel.sends.len();
        let mut created = json!({"ref": format!("{}:send:{}.{slot}", self.epoch, track + 1), "kind": "send", "bus": bus});
        let mut display = json!({"title": format!("Added send from {} to {wanted}", quoted(&label))});
        if let Some(name) = &aux {
            created["aux"] = name.clone().into();
            display["note"] = format!("Logic Pro made the aux {} for {wanted}.", quoted(name)).into();
        }
        let applied = json!({"destination": wanted, "slot": slot, "level": reached});
        Ok(json!({"epoch": self.epoch_text(), "applied": applied, "created": created, "display": display}))
    }

    fn send_add_restore(&mut self, params: &Value) -> Answer {
        let desc = Self::desc(params)?;
        let created = params.get("created").filter(|created| !created.is_null()).ok_or_else(|| bad("created: the send that was added"))?;
        let (track, slot) = self.send_for(created["ref"].as_str(), "ref", true).map_err(Self::at_restore)?;
        let bus = params["applied"]["destination"].as_str().unwrap_or_default().to_string();
        let label = self.label(Owner::Track(track));
        let channel = &self.tracks[track].channel;
        let Some(send) = channel.sends.get(slot - 1).filter(|send| send.destination() == bus).cloned() else {
            if !channel.busses().contains(&bus) {
                let undo = self.restored(None, &desc)?;
                return Ok(json!({
                    "epoch": self.epoch_text(),
                    "restored": {"removed": created["ref"]},
                    "display": {"title": format!("The send to {bus} is off {} already", quoted(&label))},
                    "undo": undo,
                }));
            }
            return Err(changed(format!("the sends of {} have been rearranged in Logic Pro since Kumi added one", quoted(&label))));
        };
        if params["discard"] != true
            && params["applied"]["level"].is_number()
            && !same_db(Some(send.level), params["applied"]["level"].as_f64())
        {
            return Err(changed(format!("the send to {bus} has been changed in Logic Pro since Kumi added it")));
        }
        let title = format!("The send from {} to {bus} taken away again", quoted(&label));
        // Logic's own undo while the send is its latest step: it takes the aux it made away too.
        let key = Self::key_of(params);
        if let Ok(UndoStep { title: pressed, .. }) = self.undo_in_logic(SEND_STEP, |top| top.key.as_deref() == Some(key.as_str())) {
            if self.restore_unanswered() {
                return Err(uncertain(format!(
                    "Logic Pro's {pressed:?} was pressed, but the send to {bus} is still on {}",
                    quoted(&label)
                )));
            }
            self.tracks[track].channel.sends.remove(slot - 1);
            if let Some(aux) = created["aux"].as_str() {
                self.auxes.retain(|made| made.name != aux);
            }
            return Ok(json!({
                "epoch": self.epoch_text(),
                "restored": {"removed": created["ref"], "pressed": pressed},
                "display": {"title": title, "note": "Taken back with Logic Pro's own undo, with the aux it made."},
                "undo": {"desc": desc, "entry": false, "how": "undo"},
            }));
        }
        self.tracks[track].channel.sends.remove(slot - 1);
        let undo = self.restored(Some(SEND_STEP), &desc)?;
        let mut display = json!({"title": title});
        if let Some(aux) = created["aux"].as_str() {
            display["note"] = format!("The aux {} Logic Pro made for it stays in the Mixer.", quoted(aux)).into();
        }
        Ok(json!({"epoch": self.epoch_text(), "restored": {"removed": created["ref"]}, "display": display, "undo": undo}))
    }

    /// `send-remove {ref}`: the send's list button pressed and No Send taken. A step of Logic's undo ("Change Send in
    /// Channel Strip"); the restore is Logic's own undo while on top, else a send to the same bus at its level, made again.
    pub(super) fn send_remove(&mut self, method: &str, params: &Value) -> Answer {
        if method == "change_restore" {
            return self.send_remove_restore(params);
        }
        let input = &params["input"];
        let (track, slot, label, send) = self.found_send(input["ref"].as_str(), false).map_err(|fault| Self::went(method, fault))?;
        let state = json!({"destination": send.destination(), "level": send.level, "slot": slot});
        let fence = json!({"send": state, "sends": self.tracks[track].channel.busses()});
        let title = format!("Removed the send from {} to {}", quoted(&label), send.destination());
        if method == "change_preview" {
            return Ok(
                json!({"epoch": self.epoch_text(), "target": self.send_target(track, slot), "prior": state, "fence": fence, "display": {"title": title}}),
            );
        }
        if params["fence"] != fence {
            return Err(changed(format!("the sends of {} changed since the preview", quoted(&label))));
        }
        let desc = Self::desc(params)?;
        self.tracks[track].channel.sends.remove(slot - 1);
        let undo = self.applied(Some(SEND_STEP), &desc)?;
        let mut material = state;
        material["step"] = desc.into();
        Ok(json!({
            "epoch": self.epoch_text(),
            "applied": {"removed": true, "sends": self.tracks[track].channel.busses()},
            "material": material,
            "display": {"title": title},
            "undo": undo,
        }))
    }

    fn send_remove_restore(&mut self, params: &Value) -> Answer {
        let desc = Self::desc(params)?;
        let material =
            params.get("material").filter(|material| !material.is_null()).ok_or_else(|| bad("material: the send that was removed"))?;
        let bus = material["destination"].as_str().unwrap_or_default().to_string();
        let (track, _) = self.send_for(params["target"]["ref"].as_str(), "ref", true).map_err(Self::at_restore)?;
        let label = self.label(Owner::Track(track));
        if self.tracks[track].channel.busses().contains(&bus) {
            let undo = self.restored(None, &desc)?;
            return Ok(json!({
                "epoch": self.epoch_text(),
                "restored": {"destination": bus},
                "display": {"title": format!("The send to {bus} is on {} already", quoted(&label))},
                "undo": undo,
            }));
        }
        let number = bus_of(&json!({"bus": bus}))? as usize;
        let level = material["level"].as_f64().unwrap_or(MIN_DB);
        let title = format!("The send from {} to {bus} put back", quoted(&label));
        let key = Self::key_of(params);
        let step = material["step"].as_str().unwrap_or_default().to_string();
        let ours = |top: &UndoStep| top.key.as_deref() == Some(key.as_str()) && top.desc.as_deref() == Some(step.as_str());
        if let Ok(UndoStep { title: pressed, .. }) = self.undo_in_logic(SEND_STEP, ours) {
            if self.restore_unanswered() {
                return Err(uncertain(format!(
                    "Logic Pro's {pressed:?} was pressed, but the send to {bus} isn't back on {}",
                    quoted(&label)
                )));
            }
            let slot = (material["slot"].as_u64().unwrap_or(1) as usize).clamp(1, self.tracks[track].channel.sends.len() + 1);
            self.tracks[track].channel.sends.insert(slot - 1, Send { bus: number, level });
            return Ok(json!({
                "epoch": self.epoch_text(),
                "restored": {"destination": bus, "pressed": pressed},
                "display": {"title": title, "note": "Put back with Logic Pro's own undo."},
                "undo": {"desc": desc, "entry": false, "how": "undo"},
            }));
        }
        // By value: a send to the same bus at the level it had, as send-add makes it.
        let input = json!({"ref": format!("{}:track:{}", self.epoch, track + 1), "bus": number, "volumeDb": level});
        let fence = json!({"sends": self.tracks[track].channel.busses()});
        let made = self.send_add_of("change_apply", &input, &fence)?;
        let undo = self.restored(Some(SEND_STEP), &desc)?;
        Ok(json!({
            "epoch": self.epoch_text(),
            "restored": {"destination": bus, "ref": made["created"]["ref"]},
            "display": {"title": title, "note": "Put back by value: Logic Pro's own undo had other steps on top."},
            "undo": undo,
        }))
    }

    /// `send-volume {ref, volumeDb}`: the send's knob stepped, read back as its text. No step of Logic's undo: the
    /// restore sets it back, by value.
    pub(super) fn send_volume(&mut self, method: &str, params: &Value) -> Answer {
        if method == "change_restore" {
            let desc = Self::desc(params)?;
            let (track, slot, label, send) = self.found_send(params["target"]["ref"].as_str(), true).map_err(Self::at_restore)?;
            let prior = params["prior"]["level"].as_f64();
            if same_db(Some(send.level), prior) {
                let undo = self.restored(None, &desc)?;
                return Ok(json!({
                    "epoch": self.epoch_text(),
                    "restored": {"level": send.level},
                    "display": {"title": format!("The send to {} is where it was", send.destination())},
                    "undo": undo,
                }));
            }
            if params["discard"] != true && !same_db(Some(send.level), params["applied"]["level"].as_f64()) {
                return Err(changed(format!("the send to {} has been changed in Logic Pro since Kumi set it", send.destination())));
            }
            let reached = knob_db(prior.unwrap_or(MIN_DB));
            self.tracks[track].channel.sends[slot - 1].level = reached;
            let undo = self.restored(None, &desc)?;
            return Ok(json!({
                "epoch": self.epoch_text(),
                "restored": {"level": reached},
                "display": {"title": format!("Send from {} to {} at {} dB (put back)", quoted(&label), send.destination(), db_text(reached))},
                "undo": undo,
            }));
        }
        let input = &params["input"];
        let (track, slot, label, send) = self.found_send(input["ref"].as_str(), false).map_err(|fault| Self::went(method, fault))?;
        let db = level_of(input)?.ok_or_else(|| bad("volumeDb: the send's level in dB"))?;
        let now = json!({"level": send.level});
        if method == "change_preview" {
            let mut display = json!({
                "title": format!("Send from {} to {} at {} dB", quoted(&label), send.destination(), db_text(db)),
                "before": now,
                "after": {"level": db},
            });
            if same_db(Some(send.level), Some(db)) {
                display["unchanged"] = true.into();
            }
            return Ok(json!({
                "epoch": self.epoch_text(),
                "target": self.send_target(track, slot),
                "prior": now,
                "fence": {"send": send.destination(), "level": send.level},
                "display": display,
            }));
        }
        if params["fence"]["send"] != json!(send.destination()) || !same_db(Some(send.level), params["fence"]["level"].as_f64()) {
            return Err(changed(format!("the send from {} to {} changed since the preview", quoted(&label), send.destination())));
        }
        let desc = Self::desc(params)?;
        let reached = if same_db(Some(send.level), Some(db)) { send.level } else { knob_db(db) };
        self.tracks[track].channel.sends[slot - 1].level = reached;
        let undo = self.applied(None, &desc)?;
        Ok(json!({
            "epoch": self.epoch_text(),
            "applied": {"level": reached},
            "display": {
                "title": format!("Send from {} to {} at {} dB", quoted(&label), send.destination(), db_text(reached)),
                "before": {"level": send.level},
                "after": {"level": reached},
            },
            "undo": undo,
        }))
    }

    // ----- Smart Controls -----

    /// The Smart Control a request names on a track: by its name (any case) or its place (from 1); its index.
    fn smart_named(&self, track: usize, which: &Value) -> Result<usize, Fault> {
        let all = &self.tracks[track].smart;
        if all.is_empty() {
            return Err(failed("Logic Pro's Smart Controls pane shows no controls for this track"));
        }
        match which {
            Value::Number(n) => n
                .as_u64()
                .map(|n| n as usize)
                .filter(|n| *n >= 1 && *n <= all.len())
                .map(|n| n - 1)
                .ok_or_else(|| bad(format!("control must be a Smart Control's place from 1 to {}, or its name", all.len()))),
            Value::String(name) => all.iter().position(|smart| smart.name.eq_ignore_ascii_case(name.trim())).ok_or_else(|| {
                bad(format!(
                    "this track has no Smart Control named {name:?}: it has {}",
                    all.iter().map(|smart| quoted(&smart.name)).collect::<Vec<_>>().join(", ")
                ))
            }),
            _ => Err(bad("control must be the Smart Control's name or its place (from 1)")),
        }
    }

    /// `smart-control-set {ref, control, value}`: the track selected, the Smart Controls pane (B): the control's
    /// slider stepped to the value (a hundredth a set), read back. No step of Logic's undo: the restore steps it back.
    pub(super) fn smart_control_set(&mut self, method: &str, params: &Value) -> Answer {
        if method == "change_restore" {
            let desc = Self::desc(params)?;
            let track = self.track_for(params["target"]["track"].as_str(), "ref", true).map_err(Self::at_restore)?;
            let label = self.label(Owner::Track(track));
            let name = params["prior"]["name"].as_str().unwrap_or_default().to_string();
            let prior = params["prior"]["value"].as_f64().ok_or_else(|| bad("prior.value"))?;
            let at = self.smart_named(track, &json!(name))?;
            let now = self.tracks[track].smart[at].value;
            if (now - prior).abs() > SMART_CLOSE {
                let applied = params["applied"]["value"].as_f64();
                if params["discard"] != true && !applied.is_some_and(|applied| (now - applied).abs() <= SMART_CLOSE) {
                    return Err(changed(format!(
                        "{}'s Smart Control {} has been changed in Logic Pro since Kumi set it",
                        quoted(&label),
                        quoted(&name)
                    )));
                }
                self.tracks[track].smart[at].value = (prior * 100.0).round() / 100.0;
            }
            let smart = &self.tracks[track].smart[at];
            let (restored, title) =
                (smart.state(), format!("{}'s Smart Control {} at {} (put back)", quoted(&label), quoted(&smart.name), smart.text()));
            let undo = self.restored(None, &desc)?;
            return Ok(json!({"epoch": self.epoch_text(), "restored": restored, "display": {"title": title}, "undo": undo}));
        }
        let input = &params["input"];
        let track = self.track_for(input["ref"].as_str(), "ref", false).map_err(|fault| Self::went(method, fault))?;
        let which =
            input.get("control").filter(|v| !v.is_null()).cloned().ok_or_else(|| bad("control: the Smart Control's name or place"))?;
        let value = input
            .get("value")
            .and_then(Value::as_f64)
            .filter(|v| (0.0..=1.0).contains(v))
            .ok_or_else(|| bad("value must be a number from 0 to 1: the control's share of its range"))?;
        let label = self.label(Owner::Track(track));
        let at = self.smart_named(track, &which)?;
        let smart = self.tracks[track].smart[at].clone();
        let now = smart.state();
        if method == "change_preview" {
            let mut display = json!({
                "title": format!("{}'s Smart Control {} at {}%", quoted(&label), quoted(&smart.name), (value * 100.0).round()),
                "before": now,
            });
            if (smart.value - value).abs() <= SMART_CLOSE {
                display["unchanged"] = true.into();
            }
            return Ok(json!({
                "epoch": self.epoch_text(),
                "target": {"ref": format!("{}:smart:{}.{}", self.epoch, track + 1, at + 1), "kind": "smart", "track": format!("{}:track:{}", self.epoch, track + 1)},
                "prior": now,
                "fence": now,
                "display": display,
            }));
        }
        if params["fence"] != now {
            return Err(changed(format!("{}'s Smart Control {} changed since the preview", quoted(&label), quoted(&smart.name))));
        }
        let desc = Self::desc(params)?;
        self.tracks[track].smart[at].value = (value * 100.0).round() / 100.0;
        let after = self.tracks[track].smart[at].clone();
        let undo = self.applied(None, &desc)?;
        Ok(json!({
            "epoch": self.epoch_text(),
            "applied": after.state(),
            "display": {
                "title": format!("{}'s Smart Control {} at {}", quoted(&label), quoted(&after.name), after.text()),
                "before": {"value": round4(smart.value), "text": smart.text()},
                "after": after.state(),
            },
            "undo": undo,
        }))
    }

    // ----- automation, through the Track Automation Event List -----

    /// A track's automation as the list lists it: by position, then by the parameter's number.
    fn sort_events(events: &mut [AutoEvent]) {
        events.sort_by(|a, b| a.beats.total_cmp(&b.beats).then(a.number.cmp(&b.number)));
    }

    /// The event a request's `{positionBeats, value}` makes: on a whole beat (the LCD places the playhead on one), its
    /// value from 0 to 127.
    fn point_of(spec: &Value) -> Result<(f64, u32), Fault> {
        let beats = spec
            .get("positionBeats")
            .and_then(Value::as_f64)
            .filter(|n| n.is_finite() && (0.0..=1.0e6).contains(n))
            .ok_or_else(|| bad("every event needs positionBeats (beats from the project's start)"))?;
        if beats.fract().abs() > 1e-9 {
            return Err(bad("Logic Pro's playhead display places an automation event on a beat: give whole beats"));
        }
        let value = spec
            .get("value")
            .and_then(Value::as_u64)
            .filter(|v| *v <= 127)
            .ok_or_else(|| bad("every event needs a value from 0 to 127"))?;
        Ok((beats, value as u32))
    }

    /// How many events a request names, checked (an add's each as `point_of` takes it).
    fn asked_count(input: &Value, kind: &str) -> Result<usize, Fault> {
        let key = match kind {
            "automation-add" => "events",
            "automation-change" => "changes",
            _ => "indexes",
        };
        let list = input.get(key).and_then(Value::as_array).filter(|list| !list.is_empty()).ok_or_else(|| bad(format!("{key}: a list")))?;
        if list.len() > MOST_EVENTS {
            return Err(bad(format!("at most {MOST_EVENTS} events at once")));
        }
        if kind == "automation-add" {
            for spec in list {
                Self::point_of(spec)?;
            }
        }
        Ok(list.len())
    }

    /// The indexes a change or a delete names, checked against the parameter's events (`count`).
    fn check_indexes(input: &Value, kind: &str, count: usize) -> Result<Vec<usize>, Fault> {
        let indexes: Vec<Option<u64>> = match kind {
            "automation-delete" => input["indexes"].as_array().into_iter().flatten().map(Value::as_u64).collect(),
            _ => input["changes"].as_array().into_iter().flatten().map(|change| change.get("index").and_then(Value::as_u64)).collect(),
        };
        let mut seen = Vec::new();
        for index in indexes {
            let index = index.map(|i| i as usize).filter(|i| *i < count).ok_or_else(|| {
                bad(format!(
                    "every index must be the place of an event of that parameter, from 0 (it has {count}): read the automation again"
                ))
            })?;
            if seen.contains(&index) {
                return Err(bad("an event is named twice"));
            }
            seen.push(index);
        }
        Ok(seen)
    }

    fn events_json(events: &[AutoEvent]) -> Value {
        Value::Array(
            events
                .iter()
                .map(|e| json!({"positionBeats": e.beats, "value": e.value, "number": e.number, "parameter": e.parameter}))
                .collect(),
        )
    }

    fn events_of_json(events: &Value) -> Vec<AutoEvent> {
        events
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(|e| {
                Some(AutoEvent {
                    beats: e["positionBeats"].as_f64()?,
                    number: e["number"].as_u64()? as u32,
                    parameter: e["parameter"].as_str().unwrap_or_default().into(),
                    value: e["value"].as_u64()? as u32,
                })
            })
            .collect()
    }

    /// An event made in the list at a position: Create new Event, its Num stepped to the parameter (by its number, or up
    /// from 0 until the Length/Info cell names it), its Val to the value.
    fn made_event(parameter: &Value, beats: f64, value: u32) -> Result<AutoEvent, Fault> {
        let number = match parameter {
            Value::Number(n) => {
                n.as_u64().filter(|n| *n <= 16383).map(|n| n as u32).ok_or_else(|| bad("parameter: a number from 0 to 16383"))?
            }
            Value::String(name) => {
                AUTOMATED.iter().find(|(known, _)| known.eq_ignore_ascii_case(name.trim())).map(|(_, n)| *n).ok_or_else(|| {
                    bad(format!(
                        "no automation parameter of this track is named {} (Kumi looked through numbers 0 to 127)",
                        quoted(name.trim())
                    ))
                })?
            }
            _ => {
                return Err(bad(
                    "parameter: the parameter's name as Logic Pro's Track Automation Event List names it (\"Volume\"), or its number",
                ))
            }
        };
        Ok(AutoEvent { beats, number, parameter: automated_name(number), value })
    }

    /// `automation-add {ref, parameter, events}`, `automation-change {ref, parameter, changes}` and `automation-delete
    /// {ref, parameter, indexes}`, as the bridge makes them (`changes/automation.rs`): the track selected, its Track
    /// Automation Event List (Ctrl-Cmd-E), the event type Fader; an event made at the playhead (on a whole beat), Create
    /// new Event, its Num and Val stepped; a change the event taken out and made again; a row's Delete. The parameter is
    /// its name as the list's Length/Info cell writes it ("Volume") or its number; an event is named by its place among
    /// that parameter's events (from 0, as the read lists them). Each event is steps of Logic's undo ("Create Event",
    /// "Value Change", "Delete"); the change fences the track's whole automation, and a restore puts it back by value
    /// (the events Kumi made deleted, those it deleted made again).
    pub(super) fn automation(&mut self, kind: &str, method: &str, params: &Value) -> Answer {
        if method == "change_restore" {
            let desc = Self::desc(params)?;
            let track = self.track_for(params["target"]["ref"].as_str(), "ref", true).map_err(Self::at_restore)?;
            let label = self.label(Owner::Track(track));
            let made = Self::events_of_json(&params["applied"]["made"]);
            let removed = Self::events_of_json(&params["applied"]["removed"]);
            let now = self.tracks[track].automation.clone();
            if params["discard"] != true && Self::events_json(&now) != params["applied"]["events"] {
                return Err(changed(format!("{}'s automation has been changed in Logic Pro since Kumi wrote it", quoted(&label))));
            }
            let mut after: Vec<AutoEvent> = now.into_iter().filter(|event| !made.contains(event)).collect();
            after.extend(removed.iter().cloned());
            Self::sort_events(&mut after);
            let mut steps = vec!["Delete"; made.len()];
            for _ in &removed {
                steps.extend(["Create Event", "Value Change"]);
            }
            self.tracks[track].automation = after.clone();
            let undo = self.restored_steps(&steps, &desc)?;
            if Self::events_json(&after) != params["prior"]["events"] {
                return Err(uncertain(format!("{}'s automation isn't as it was after Kumi put it back", quoted(&label))));
            }
            return Ok(json!({
                "epoch": self.epoch_text(),
                "restored": {"events": Self::events_json(&after)},
                "display": {"title": format!("{}'s automation put back", quoted(&label))},
                "undo": undo,
            }));
        }
        let input = &params["input"];
        let track = self.track_for(input["ref"].as_str(), "ref", false).map_err(|fault| Self::went(method, fault))?;
        let parameter = match input.get("parameter") {
            Some(Value::String(name)) if !name.trim().is_empty() => Value::String(name.trim().to_string()),
            Some(Value::Number(n)) if n.as_u64().is_some_and(|n| n <= 16383) => Value::Number(n.clone()),
            Some(Value::Number(_)) => return Err(bad("parameter: a number from 0 to 16383")),
            _ => {
                return Err(bad(
                    "parameter: the parameter's name as Logic Pro's Track Automation Event List names it (\"Volume\"), or its number",
                ))
            }
        };
        let matches = |event: &AutoEvent| match &parameter {
            Value::String(name) => event.parameter.eq_ignore_ascii_case(name),
            _ => Some(event.number) == parameter.as_u64().map(|n| n as u32),
        };
        let shown = match &parameter {
            Value::String(name) => name.clone(),
            other => format!("parameter {other}"),
        };
        let count = Self::asked_count(input, kind)?;
        let label = self.label(Owner::Track(track));
        let all = self.tracks[track].automation.clone();
        let mine: Vec<AutoEvent> = all.iter().filter(|event| matches(event)).cloned().collect();
        if kind != "automation-add" {
            Self::check_indexes(input, kind, mine.len())?;
        }
        let title = |count: usize| {
            let verb = match kind {
                "automation-add" => "Wrote",
                "automation-change" => "Changed",
                _ => "Deleted",
            };
            format!("{verb} {count} {shown} automation event{} on {}", if count == 1 { "" } else { "s" }, quoted(&label))
        };
        let fence = json!({"events": Self::events_json(&all)});
        if method == "change_preview" {
            return Ok(json!({
                "epoch": self.epoch_text(),
                "target": {"ref": format!("{}:track:{}", self.epoch, track + 1), "kind": "track"},
                "prior": fence,
                "fence": fence,
                "display": {"title": title(count)},
            }));
        }
        if params["fence"] != fence {
            return Err(changed(format!("{}'s automation changed since the preview", quoted(&label))));
        }
        let desc = Self::desc(params)?;
        let (mut made, mut removed, mut steps) = (Vec::new(), Vec::new(), Vec::new());
        let mut after = all.clone();
        match kind {
            "automation-add" => {
                for spec in input["events"].as_array().into_iter().flatten() {
                    let (beats, value) = Self::point_of(spec)?;
                    made.push(Self::made_event(&parameter, beats, value)?);
                    steps.extend(["Create Event", "Value Change"]);
                }
            }
            "automation-delete" => {
                for index in Self::check_indexes(input, kind, mine.len())? {
                    removed.push(mine[index].clone());
                    steps.push("Delete");
                }
            }
            _ => {
                for change in input["changes"].as_array().into_iter().flatten() {
                    let was = mine[change["index"].as_u64().unwrap_or(0) as usize].clone();
                    let beats = match change.get("positionBeats").filter(|v| !v.is_null()) {
                        Some(_) => Self::point_of(&json!({"positionBeats": change["positionBeats"], "value": 0}))?.0,
                        None => was.beats,
                    };
                    let value = match change.get("value").filter(|v| !v.is_null()) {
                        Some(v) => v.as_u64().filter(|v| *v <= 127).ok_or_else(|| bad("value must be from 0 to 127"))? as u32,
                        None => was.value,
                    };
                    // (the event taken out and made again where and as asked)
                    removed.push(was.clone());
                    made.push(AutoEvent { beats, value, ..was });
                    steps.extend(["Delete", "Create Event", "Value Change"]);
                }
            }
        }
        after.retain(|event| !removed.contains(event));
        after.extend(made.iter().cloned());
        Self::sort_events(&mut after);
        self.tracks[track].automation = after.clone();
        let undo = self.applied_steps(&steps, &desc)?;
        Ok(json!({
            "epoch": self.epoch_text(),
            "applied": {"made": Self::events_json(&made), "removed": Self::events_json(&removed), "events": Self::events_json(&after)},
            "display": {"title": title(made.len().max(removed.len()))},
            "undo": undo,
        }))
    }

    // ----- what a script asks of the strips -----

    /// The strip a script names, `track:2` or `output:1`, if the project has it.
    pub(super) fn strip_named(&self, strip: &str) -> Option<Owner> {
        let (kind, n) = strip.split_once(':')?;
        let n: usize = n.parse().ok()?;
        match kind {
            "track" if n >= 1 && n <= self.tracks.len() => Some(Owner::Track(n - 1)),
            "output" if n >= 1 && n <= self.outputs.len() => Some(Owner::Output(n - 1)),
            _ => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn channel_eqs_rows_read_as_the_scene_read_them() {
        let eq = Plugin::named("Channel EQ");
        let low_shelf = eq.state(6);
        assert_eq!(low_shelf, json!({"index": 7, "name": "Low Shelf Gain", "value": 240.0, "text": "0.0 dB"}));
        assert_eq!(eq.state(8), json!({"index": 9, "name": "Peak 1 On/Off", "value": 1.0, "text": "on"}));
        let texts: Vec<String> = [1, 3, 5, 7, 9, 11, 13, 15, 17, 19, 21, 23, 25, 29, 35, 38, 44].iter().map(|at| eq.text(*at)).collect();
        assert_eq!(
            texts,
            [
                "20.0 Hz", "0.71", "75.0 Hz", "1.00", "100 Hz", "0.60", "250 Hz", "0.30", "1040 Hz", "0.41", "2500 Hz", "0.20", "7500 Hz",
                "20000 Hz", "0 dB", "10 dB/s", "0"
            ]
        );
        assert_eq!(
            (eq.text(2), eq.text(36), eq.text(37), eq.text(45)),
            ("12 dB/Oct".into(), "Post EQ".into(), "Peak".into(), "60 dB".into())
        );
        assert_eq!(eq.state(2)["value"], Value::Null, "a menu has its text alone");
        // +3 dB is found by reading the slider's text, as the bridge finds it; "off" sets a switch.
        assert_eq!(raw_for(&eq, 6, &wanted_of(&json!({"value": 3})).unwrap()).unwrap(), 270.0);
        assert_eq!(slider_text(EQ_GAIN, 270.0), "+3.0 dB");
        assert_eq!(raw_for(&eq, 8, &wanted_of(&json!({"value": "off"})).unwrap()).unwrap(), 0.0);
        assert!(raw_for(&eq, 6, &wanted_of(&json!({"value": 40})).unwrap())
            .unwrap_err()
            .1
            .contains("no value of “Channel EQ”'s Low Shelf Gain reads 40"));
        assert!(raw_for(&eq, 2, &wanted_of(&json!({"value": "6 dB/Oct"})).unwrap()).unwrap_err().1.contains("doesn't take that value"));
        assert_eq!(raw_for(&eq, 37, &wanted_of(&json!({"value": "rms"})).unwrap()).unwrap(), 1.0);
        assert_eq!(normalized(Some(270.0), Some(0.0), Some(480.0)), Some(0.5625));
    }

    #[test]
    fn the_gains_gain_is_its_second_row_from_minus_96_to_plus_24_db() {
        let gain = Plugin::named("Gain");
        assert_eq!(gain.state(1), json!({"index": 2, "name": "Gain", "value": 960.0, "text": "0.0 dB"}));
        assert_eq!(
            (gain.text(0), gain.text(2), gain.text(5)),
            ("0.0 %".into(), "Norm".into(), "Stereo".into()),
            "as change-fx-output read it"
        );
        assert_eq!(raw_for(&gain, 1, &wanted_of(&json!({"value": -6})).unwrap()).unwrap(), 900.0);
        assert_eq!(raw_for(&gain, 1, &wanted_of(&json!({"value": "-6.0 dB"})).unwrap()).unwrap(), 900.0);
        assert_eq!(raw_for(&gain, 1, &wanted_of(&json!({"normalized": 0.5})).unwrap()).unwrap(), 600.0);
        assert_eq!(slider_text(db(-96.0, 0.1), 1200.0), "+24.0 dB");
        assert_eq!(wanted_of(&json!({"normalized": 2})).unwrap_err().1, "normalized must be a number from 0 to 1");
        assert_eq!(wanted_of(&json!({"value": "Inv"})).unwrap(), Wanted::Option("Inv".into()));
    }

    #[test]
    fn a_setting_is_stepped_along_the_list_and_only_the_windows_undo_brings_default_preset_back() {
        let mut compressor = Plugin::named("Compressor");
        assert_eq!(compressor.press(true), "Vocal Compressor");
        assert_eq!(compressor.press(true), "Drum Bus");
        assert_eq!(compressor.row_named("Threshold"), Some((Some(420.0), "-18.0 dB".into())));
        assert_eq!(compressor.press(true), "Drum Bus", "the last stays");
        assert_eq!(
            (compressor.press(false), compressor.press(false), compressor.press(false)),
            ("Vocal Compressor".into(), "Vocal Compressor".into(), "Vocal Compressor".into()),
            "so does the first"
        );
        for _ in 0..6 {
            compressor.window_undo();
        }
        assert_eq!(
            (compressor.preset.as_str(), compressor.row_named("Threshold")),
            (DEFAULT_PRESET, Some((Some(400.0), "-20.0 dB".into())))
        );
        assert_eq!(Plugin::named("Channel EQ").press(false), "Brighten Hi Hat", "from Default Preset, the first either way");
        let mut gain = Plugin::named("Gain");
        assert_eq!(gain.press(true), DEFAULT_PRESET, "a plug-in without settings stays");
        assert_eq!(ask_of(&json!({"step": 65})).unwrap_err().0, "bad-params");
        assert_eq!(ask_of(&json!({"preset": "Drum Bus"})).unwrap(), Ask::Named("Drum Bus".into()));
    }

    #[test]
    fn a_plug_in_a_bus_and_a_patch_are_named_as_the_bridge_takes_them() {
        assert_eq!(plugin_name(&json!({"name": "Dynamics/Compressor"})).unwrap(), "Compressor");
        assert!(plugin_name(&json!({"name": ""})).is_err());
        assert_eq!(index_of(&json!({}), 3).unwrap(), 3);
        assert!(index_of(&json!({"index": 4}), 3).unwrap_err().1.contains("from 0 to 3"));
        assert_eq!(searched("Gain").map(|spec| spec.name), Some("Gain"));
        assert_eq!(searched("comp").map(|spec| spec.name), Some("Compressor"));
        assert_eq!(searched("Piano").map(|spec| spec.name), None, "an instrument isn't in an effect slot's menu");
        assert_eq!((bus_of(&json!({"bus": 2})).unwrap(), bus_of(&json!({"toRef": "Bus 3"})).unwrap()), (2, 3));
        assert!(bus_of(&json!({"bus": 65})).is_err());
        assert!(level_of(&json!({"volumeDb": 7})).is_err());
        assert_eq!(level_of(&json!({"volumeDb": -120})).unwrap(), Some(MIN_DB));
        assert_eq!(patch_of(&json!({"patch": "Vocal/Classic Vocal"})).unwrap(), "Classic Vocal");
        assert_eq!(Owner::Output(0).plugin(3, 2), "3:plugin:0.1.2");
        assert_eq!(Owner::Track(1).param(3, 1, 4), "3:param:2.1.4");
        assert_eq!(Owner::Output(0).text(3), "3:output:1");
    }
}
