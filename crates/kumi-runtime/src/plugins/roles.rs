//! Knobs found by what they do. A mapped plug-in's role ("ceiling" on Ozone 12) is looked for among the names Live
//! lists for that plug-in (the map's names are patterns, not promises) and used only when Live has that parameter
//! configured. A job ("limiter gain", "ceiling", "eq gain") is a role any device can have: the mapped plug-ins' roles
//! that do it, and Live's own knobs that do. When a device can't turn a knob, another on the same track does the job,
//! in the agreed order: Ozone 12, then the other mapped plug-ins, then Live's own devices. A device that's off isn't
//! tried (it's said when it's the only one that would do it), and Live's own device to add goes where its job does: a
//! limiter's last on the chain, the rest before the track's last limiter.

use std::sync::LazyLock;

use crate::plugins::{
    adapter::{matches, pattern, Pattern, PluginAdapter, PluginParameterHint},
    registry::adapter_for,
};

/// One of Live's devices that does a job: its class, its name in words, the knob's pattern, the knob in words, and a
/// switch of the device's that has to be on for that knob to do it (an entry with one is tried first).
pub struct Stock {
    pub class: &'static str,
    pub device: &'static str,
    pub knob: Pattern,
    pub knob_said: &'static str,
    pub when: Option<&'static str>,
}

/// A job any device can do, by its knob on each.
pub struct Job {
    /// What tune takes for it in knobs.
    pub name: &'static str,
    /// Other words for it.
    pub words: &'static [&'static str],
    /// Mapped plug-ins' roles that do it: the adapter, its role, and a narrower pattern where the role covers more.
    pub plugins: Vec<(&'static str, &'static str, Option<Pattern>)>,
    /// Live's devices that do it, the one to suggest first.
    pub stock: Vec<Stock>,
}

fn stock(class: &'static str, device: &'static str, knob: &str, knob_said: &'static str) -> Stock {
    Stock { class, device, knob: pattern(knob), knob_said, when: None }
}

fn stock_when(switch: &'static str, class: &'static str, device: &'static str, knob: &str, knob_said: &'static str) -> Stock {
    Stock { when: Some(switch), ..stock(class, device, knob, knob_said) }
}

/// The jobs, with Live's own knobs by the names Live 12.4 gives them (read from Live).
pub static JOBS: LazyLock<Vec<Job>> = LazyLock::new(|| {
    // Ozone's Dynamics has a compressor and a limiter per band: a compressor's job is the compressor's.
    let compressor = |knob: &str| Some(pattern(&format!(r"(?i)^(?!.*\bLim(iter)?\b).*\b{knob}")));
    vec![
        Job {
            name: "limiter gain",
            words: &["limiter gain", "maximizer gain", "limiter input", "gain into the limiter"],
            plugins: vec![("ozone12", "threshold", None), ("prol2", "gain", None)],
            // Live 12.4 names it Input Gain; earlier Lives, Gain. Maximizing, Input Gain does nothing and Threshold drives
            // it (heard on Live 12.4: Threshold -0.3 to -3.1 dB raised the loudness 2.9 LU).
            stock: vec![
                stock_when("Maximize On", "Limiter", "Limiter", r"^Threshold$", "Threshold, as it's maximizing"),
                stock("Limiter", "Limiter", r"^(Input )?Gain$", "Input Gain"),
            ],
        },
        Job {
            name: "ceiling",
            words: &["ceiling", "limiter ceiling", "output ceiling", "true peak ceiling"],
            plugins: vec![("ozone12", "ceiling", None), ("prol2", "output level", None)],
            // Maximizing, Ceiling does nothing and Output sets where it peaks (heard on Live 12.4: Output -24 to 0 dB moved
            // the true peak -26.2 to -2.2 dBTP).
            stock: vec![
                stock_when("Maximize On", "Limiter", "Limiter", r"^Output$", "Output, as it's maximizing"),
                stock("Limiter", "Limiter", r"^Ceiling$", "Ceiling"),
            ],
        },
        Job {
            name: "eq gain",
            words: &["eq gain", "band gain", "eq band gain"],
            plugins: vec![("ozone12", "eq gain", None), ("proq4", "gain", None)],
            stock: vec![stock("Eq8", "EQ Eight", r"^[1-8] Gain A$", "band gain: 1 Gain A … 8 Gain A")],
        },
        Job {
            name: "eq frequency",
            words: &["eq frequency", "band frequency", "eq band frequency"],
            plugins: vec![("ozone12", "eq frequency", None), ("proq4", "frequency", None)],
            stock: vec![stock("Eq8", "EQ Eight", r"^[1-8] Frequency A$", "band frequency: 1 Frequency A … 8 Frequency A")],
        },
        Job {
            name: "compressor threshold",
            words: &["compressor threshold", "comp threshold"],
            plugins: vec![("ozone12", "dynamics", compressor("Threshold"))],
            stock: vec![
                stock("Compressor2", "Compressor", r"^Threshold$", "Threshold"),
                stock("GlueCompressor", "Glue Compressor", r"^Threshold$", "Threshold"),
            ],
        },
        Job {
            name: "compressor ratio",
            words: &["compressor ratio", "comp ratio"],
            plugins: vec![("ozone12", "dynamics", compressor("Ratio"))],
            stock: vec![
                stock("Compressor2", "Compressor", r"^Ratio$", "Ratio"),
                stock("GlueCompressor", "Glue Compressor", r"^Ratio$", "Ratio"),
            ],
        },
        Job {
            name: "compressor attack",
            words: &["compressor attack", "comp attack"],
            plugins: vec![("ozone12", "dynamics", compressor("Attack"))],
            stock: vec![
                stock("Compressor2", "Compressor", r"^Attack$", "Attack"),
                stock("GlueCompressor", "Glue Compressor", r"^Attack$", "Attack"),
            ],
        },
        Job {
            name: "compressor release",
            words: &["compressor release", "comp release"],
            plugins: vec![("ozone12", "dynamics", compressor("Release"))],
            stock: vec![
                stock("Compressor2", "Compressor", r"^Release$", "Release"),
                stock("GlueCompressor", "Glue Compressor", r"^Release$", "Release"),
            ],
        },
        Job {
            name: "drive",
            words: &["drive", "saturation", "saturation drive"],
            plugins: vec![
                ("saturn2", "drive", None),
                ("decapitator", "drive", None),
                ("ozone12", "exciter", Some(pattern(r"(?i)Amount|Drive"))),
            ],
            stock: vec![stock("Saturator", "Saturator", r"^Drive$", "Drive"), stock("Roar", "Roar", r"^Drive$", "Drive")],
        },
        Job {
            name: "width",
            words: &["width", "stereo width"],
            plugins: vec![("ozone12", "width", Some(pattern(r"(?i)Width"))), ("supermassive", "width", None)],
            stock: vec![],
        },
        // The lows' width is the lowest band's: Ozone 12's Imager band 1 (a reverb's width isn't the low end's).
        Job {
            name: "low width",
            words: &["low width", "low end width", "bass width", "low stereo width"],
            plugins: vec![("ozone12", "width", Some(pattern(r"(?i)\bBand 1\b.*Width")))],
            stock: vec![],
        },
    ]
});

/// The jobs that are a final limiter's: what does them goes last on the chain.
const LIMITER_JOBS: [&str; 2] = ["limiter gain", "ceiling"];

/// Live's classes for plug-in devices (VST, VST3 and Audio Units).
pub fn is_plugin(class: &str) -> bool {
    matches!(class, "PluginDevice" | "AuPluginDevice")
}

/// A word as tune compares it: any case, quotes, underscores and runs of spaces aside.
pub fn normal(word: &str) -> String {
    word.trim()
        .trim_matches(|c| matches!(c, '"' | '\'' | '“' | '”'))
        .replace('_', " ")
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .to_lowercase()
}

/// Whether two parameter names are the same name.
pub fn same(a: &str, b: &str) -> bool {
    normal(a) == normal(b)
}

/// The job a word names.
pub fn job_named(word: &str) -> Option<&'static Job> {
    let word = normal(word);
    JOBS.iter().find(|job| job.name == word || job.words.contains(&word.as_str()))
}

/// A device as tune sees it.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Seen {
    /// Its ref, as the model names it.
    pub reference: String,
    /// Live's name for it (a plug-in's own name, unless it was renamed).
    pub name: String,
    /// Live's class for it ("PluginDevice", "Limiter").
    pub class: String,
    /// The parameters Live lets Kumi turn: a plug-in's configured ones.
    pub turnable: Vec<String>,
    /// Every name Live lists for a plug-in (its get_parameter_names); None for Live's own devices, or not read.
    pub listed: Option<Vec<String>>,
    /// Switched off in Live (kept for an A/B, say): it isn't tried for a job.
    pub off: bool,
    /// Its switches that are on (Live's Limiter's Maximize On): some change which knob does a job.
    pub switched_on: Vec<String>,
}

impl Seen {
    /// The plug-in map's entry for it, when it's a mapped plug-in.
    pub fn adapter(&self) -> Option<&'static PluginAdapter> {
        if is_plugin(&self.class) {
            adapter_for(&self.name, None)
        } else {
            None
        }
    }

    /// Its name in a message.
    pub fn called(&self) -> String {
        match self.adapter() {
            Some(adapter) => adapter.name.to_string(),
            None if self.name.is_empty() => "the device".into(),
            None => self.name.clone(),
        }
    }

    fn is_on(&self, switch: &str) -> bool {
        self.switched_on.iter().any(|name| same(name, switch))
    }

    fn can_turn(&self, name: &str) -> bool {
        self.turnable.iter().any(|turnable| same(turnable, name))
    }
}

/// A word as a knob on one device.
#[derive(Debug, Clone, PartialEq)]
pub enum Found {
    /// The knob to turn, by its name in Live, and how the word was read: "Ozone 12's ceiling is MAX: Output Level".
    Knob { name: String, read: String },
    /// A role or job this device can't turn: why, said plainly, with what to do. Another device may do the job.
    Missing(String),
    /// A role that's several knobs this device can turn: which, so one is named.
    Several(String),
}

fn own_role(adapter: &'static PluginAdapter, word: &str) -> Option<&'static PluginParameterHint> {
    adapter.hints().find(|hint| hint.role == word)
}

/// The role a mapped plug-in does a job with, and the narrower pattern for it.
pub fn job_role(adapter: &'static PluginAdapter, job: &'static Job) -> Option<(&'static PluginParameterHint, Option<&'static Pattern>)> {
    let (_, role, narrow) = job.plugins.iter().find(|(id, _, _)| *id == adapter.id)?;
    own_role(adapter, role).map(|hint| (hint, narrow.as_ref()))
}

/// A few names in a sentence.
fn list(names: &[&String]) -> String {
    let shown: Vec<&str> = names.iter().take(6).map(|name| name.as_str()).collect();
    let more = names.len().saturating_sub(shown.len());
    format!("{}{}", shown.join(", "), if more > 0 { format!(" and {more} more") } else { String::new() })
}

/// What Live lists for a plug-in (else what it can turn): every name, and those a role matches, narrowed.
fn named<'a>(device: &'a Seen, hint: &PluginParameterHint, narrow: Option<&Pattern>) -> (&'a [String], Vec<&'a String>) {
    let listed = device.listed.as_ref().unwrap_or(&device.turnable);
    (listed, listed.iter().filter(|name| hint.matches(name) && narrow.is_none_or(|narrow| matches(narrow, name))).collect())
}

/// A mapped plug-in's role, checked against what Live lists for it and has configured.
fn on_plugin(device: &Seen, adapter: &'static PluginAdapter, hint: &PluginParameterHint, narrow: Option<&Pattern>) -> Found {
    let (listed, names) = named(device, hint, narrow);
    let role = format!("{}'s {}", adapter.name, hint.role);
    if names.is_empty() {
        return Found::Missing(format!(
            "None of the {} parameters Live lists for {} is {role} as Kumi's map has it (the map's names are guesses for some versions): name the knob as {} shows it (plugin with action guide lists them).",
            listed.len(),
            adapter.name,
            adapter.name
        ));
    }
    let turnable: Vec<&String> = names.iter().copied().filter(|name| device.can_turn(name)).collect();
    match turnable.as_slice() {
        [one] => Found::Knob { name: (*one).clone(), read: format!("{role} is {one}") },
        [] => Found::Missing(format!(
            "{role} is {} in Live, and it isn't configured in the device, so Live won't let Kumi turn it. To configure it: click Configure in the plug-in's title bar in Live and move {} once in its window (Kumi can open the window: set_device_details with isEditorOpen); Live keeps it with the Set.",
            list(&names),
            names[0]
        )),
        several => Found::Several(format!("{role} is {} knobs Kumi can turn: {}. Name one in knobs.", several.len(), list(several))),
    }
}

/// One of Live's devices doing a job.
fn on_stock(device: &Seen, job: &Job, stock: &Stock) -> Found {
    let names: Vec<&String> = device.turnable.iter().filter(|name| matches(&stock.knob, name)).collect();
    let called = device.called();
    match names.as_slice() {
        [one] => Found::Knob { name: (*one).clone(), read: format!("{called}'s {} is {one}", job.name) },
        [] => Found::Missing(format!("{called} shows no {} knob ({}).", job.name, stock.knob_said)),
        several => {
            Found::Several(format!("The {} on {called} is {} knobs: {}. Name one in knobs.", job.name, several.len(), list(several)))
        }
    }
}

/// An EQ's band for a job named by role, when the target's frequency is known: of `bands` (each band's number, its
/// frequency in Hz, whether it's on, and whether its gain does anything: a bell or a shelf, not a cut), the one
/// nearest `near` in octaves. Bands that are on and shape come first, then any on, then any.
pub fn nearest_band(bands: &[(u8, f64, bool, bool)], near: f64) -> Option<u8> {
    let nearest = |keep: &dyn Fn(&(u8, f64, bool, bool)) -> bool| {
        bands
            .iter()
            .filter(|band| keep(band) && band.1 > 0. && near > 0.)
            .min_by(|a, b| (a.1 / near).log2().abs().total_cmp(&(b.1 / near).log2().abs()))
            .map(|band| band.0)
    };
    nearest(&|band| band.2 && band.3).or_else(|| nearest(&|band| band.2)).or_else(|| nearest(&|_| true))
}

/// A word as a role on a device: its knob, or why it can't be turned there; None when the word is no role of this
/// device's and no job (a knob's own name, maybe).
pub fn find(device: &Seen, word: &str) -> Option<Found> {
    let word = normal(word);
    let job = job_named(&word);
    match device.adapter() {
        Some(adapter) => {
            // A job it does first, narrowed as the job is (its role may cover more), then the plug-in's own role.
            let wanted = job.and_then(|job| job_role(adapter, job)).or_else(|| own_role(adapter, &word).map(|hint| (hint, None)));
            match wanted {
                Some((hint, narrow)) => Some(on_plugin(device, adapter, hint, narrow)),
                None => job.map(|job| Found::Missing(format!("{} has no {} in Kumi's map of it.", adapter.name, job.name))),
            }
        }
        None if is_plugin(&device.class) => job.map(|job| {
            Found::Missing(format!(
                "Kumi has no map of {}, so it can't tell which of its knobs is the {}: name the knob as the plug-in shows it.",
                device.called(),
                job.name
            ))
        }),
        None => {
            let job = job?;
            let fits = |stock: &&Stock| stock.class == device.class && stock.when.is_none_or(|switch| device.is_on(switch));
            Some(match job.stock.iter().find(fits) {
                Some(stock) => on_stock(device, job, stock),
                None => Found::Missing(format!("{} has no {}.", device.called(), job.name)),
            })
        }
    }
}

/// The job a word does on a device: the job it names, or the one its plug-in role does.
fn job_of(device: &Seen, word: &str) -> Option<&'static Job> {
    if let Some(job) = job_named(word) {
        return Some(job);
    }
    let adapter = device.adapter()?;
    let word = normal(word);
    JOBS.iter().find(|job| job.plugins.iter().any(|(id, role, _)| *id == adapter.id && *role == word))
}

/// Whether a device does a job: a mapped plug-in with a role for it, or one of Live's devices the job names.
pub fn does(device: &Seen, job: &'static Job) -> bool {
    match device.adapter() {
        Some(adapter) => job_role(adapter, job).is_some(),
        None => !is_plugin(&device.class) && job.stock.iter().any(|stock| stock.class == device.class),
    }
}

/// The track's devices other than `asked`, in the agreed order: Ozone 12, then the other mapped plug-ins, then Live's
/// own devices; within each, later in the chain first (nearer the output). Plug-ins Kumi has no map of aren't tried,
/// nor devices that are off.
pub fn agreed_order(devices: &[Seen], asked: Option<usize>) -> Vec<usize> {
    in_order(devices, asked, false)
}

/// The track's devices that are on (or off), other than `asked`, in the agreed order.
fn in_order(devices: &[Seen], asked: Option<usize>, off: bool) -> Vec<usize> {
    let rank = |device: &Seen| match device.adapter() {
        Some(adapter) if adapter.id == "ozone12" => Some(0),
        Some(_) => Some(1),
        None if is_plugin(&device.class) => None,
        None => Some(2),
    };
    let mut order: Vec<(usize, usize)> = devices
        .iter()
        .enumerate()
        .filter(|(at, device)| Some(*at) != asked && device.off == off)
        .filter_map(|(at, device)| rank(device).map(|rank| (rank, at)))
        .collect();
    order.sort_by(|(rank_a, at_a), (rank_b, at_b)| rank_a.cmp(rank_b).then(at_b.cmp(at_a)));
    order.into_iter().map(|(_, at)| at).collect()
}

/// What tune turns for its knobs' words.
#[derive(Debug, Clone, PartialEq)]
pub enum Resolved {
    /// Every word is a knob's name on the device, or no role: as asked.
    AsAsked,
    /// The knobs on `devices[device]`, in the words' order, how each role was read, and why, when it isn't the device
    /// asked for.
    On { device: usize, knobs: Vec<String>, read: Vec<String>, instead: Option<String> },
    /// Nothing on the track can: why, and what to do.
    Refused(String),
}

/// `words` on `devices[asked]`: knob names as they are, roles found. When a role can't be turned there, every word on
/// the first other device of the track that turns them all, in the agreed order; or why nothing can, and how to fix it.
pub fn resolve(devices: &[Seen], asked: usize, words: &[String]) -> Resolved {
    let device = &devices[asked];
    let (mut knobs, mut read, mut missing) = (vec![], vec![], vec![]);
    for word in words {
        if let Some(name) = device.turnable.iter().find(|name| same(name, word)) {
            knobs.push(name.clone());
            continue;
        }
        match find(device, word) {
            // Not a role: tune's own lookup by name decides.
            None => knobs.push(word.clone()),
            Some(Found::Knob { name, read: how }) => {
                knobs.push(name);
                read.push(how);
            }
            Some(Found::Several(why)) => return Resolved::Refused(why),
            Some(Found::Missing(why)) => missing.push(why),
        }
    }
    if missing.is_empty() {
        return if read.is_empty() { Resolved::AsAsked } else { Resolved::On { device: asked, knobs, read, instead: None } };
    }
    let why = missing.join(" ");
    // Another device does every word's job.
    let Some(jobs) = words.iter().map(|word| job_of(device, word)).collect::<Option<Vec<_>>>() else {
        return Resolved::Refused(why);
    };
    // The first that turns every word's knob; else the first that does every job with a knob to name.
    let mut naming: Option<(usize, Vec<String>)> = None;
    for at in agreed_order(devices, Some(asked)) {
        let other = &devices[at];
        let found: Vec<Option<Found>> = jobs.iter().map(|job| find(other, job.name)).collect();
        if found.iter().all(|found| matches!(found, Some(Found::Knob { .. }))) {
            let (knobs, read) = found
                .into_iter()
                .filter_map(|found| match found {
                    Some(Found::Knob { name, read }) => Some((name, read)),
                    _ => None,
                })
                .unzip();
            return Resolved::On {
                device: at,
                knobs,
                read,
                instead: Some(format!("{why} Kumi tunes {} on the same track instead.", other.called())),
            };
        }
        if naming.is_none() && found.iter().all(|found| matches!(found, Some(Found::Knob { .. } | Found::Several(_)))) {
            let several = found
                .into_iter()
                .filter_map(|found| match found {
                    Some(Found::Several(why)) => Some(why),
                    _ => None,
                })
                .collect();
            naming = Some((at, several));
        }
    }
    if let Some((at, several)) = naming {
        let other = &devices[at];
        return Resolved::Refused(format!(
            "{why} {} on the same track does it (device \"{}\"): {}",
            other.called(),
            other.reference,
            several.join(" ")
        ));
    }
    // One that does it is off: said, as it may be kept off on purpose.
    if let Some(off) =
        in_order(devices, Some(asked), true).into_iter().map(|at| &devices[at]).find(|other| jobs.iter().all(|job| does(other, job)))
    {
        return Resolved::Refused(format!(
            "{why} {} on the same track does it (device \"{}\"), but it's off: switch it on (make_changes' switch_device) to tune it there.",
            off.called(),
            off.reference
        ));
    }
    // Nothing on the track does it: Live's own devices that would, each with its knobs (not the kind asked, which
    // just said it can't), and where each goes: a limiter's job last on the chain, the rest before the track's last
    // limiter.
    let last_limiter =
        devices.iter().rposition(|device| LIMITER_JOBS.iter().filter_map(|name| job_named(name)).any(|job| does(device, job)));
    let place = |job: &Job| match last_limiter {
        Some(at) if !LIMITER_JOBS.contains(&job.name) => {
            format!("before {} (device \"{}\")", devices[at].called(), devices[at].reference)
        }
        _ => "last on the chain".to_string(),
    };
    let mut by_device: Vec<(&str, Vec<&str>, String)> = vec![];
    // A device added fresh has its switches as they come: no maximizing.
    let fresh =
        |job: &&'static Job| job.stock.iter().find(|stock| stock.class != device.class && stock.when.is_none()).map(|stock| (*job, stock));
    for (job, stock) in jobs.iter().filter_map(fresh) {
        match by_device.iter_mut().find(|(device, _, _)| *device == stock.device) {
            Some((_, knobs, _)) => knobs.push(stock.knob_said),
            None => by_device.push((stock.device, vec![stock.knob_said], place(job))),
        }
    }
    let suggested: Vec<String> =
        by_device.iter().map(|(device, knobs, place)| format!("Live's {device} ({}) {place}", knobs.join(", "))).collect();
    if suggested.is_empty() {
        return Resolved::Refused(format!("{why} Nothing else on this track does it."));
    }
    Resolved::Refused(format!(
        "{why} Nothing else on this track does it: put {}, and tune it by the same role (make_changes loads it).",
        suggested.join(" and ")
    ))
}

/// The job a checklist item's fix is, by the item's id.
pub fn job_for_item(id: &str) -> Option<&'static Job> {
    match id {
        "loudness" => job_named("limiter gain"),
        "true peak" => job_named("ceiling"),
        "low width" => job_named("low width"),
        _ if id.starts_with("balance ") => job_named("eq gain"),
        _ => None,
    }
}

/// A fix naming a mapped plug-in's role, when one on the track that's on does the job (the first in the agreed order),
/// with the fix it would have had after it. None when no mapped plug-in on the track does it, or Live lists no name
/// for its role there (a map that doesn't fit that version leads nowhere).
pub fn plugin_fix(devices: &[Seen], job: &'static Job, otherwise: Option<&str>) -> Option<String> {
    let said = agreed_order(devices, None).into_iter().find_map(|at| {
        let device = &devices[at];
        let adapter = device.adapter()?;
        let (hint, narrow) = job_role(adapter, job)?;
        if named(device, hint, narrow).1.is_empty() {
            return None;
        }
        let role = format!("{}'s {}", adapter.name, hint.role);
        let reference = &device.reference;
        Some(match on_plugin(device, adapter, hint, narrow) {
            Found::Knob { name, .. } => {
                format!("{role} ({name}), homed in (tune with how: home, device \"{reference}\", knobs [\"{name}\"])")
            }
            Found::Several(why) => format!("{why} Then tune with how: home, device \"{reference}\", knobs [that one]"),
            Found::Missing(why) => format!("{role} would do it: {why}"),
        })
    })?;
    Some(match otherwise {
        Some(otherwise) => format!("{}; or {otherwise}", said.trim_end_matches('.')),
        None => said,
    })
}
