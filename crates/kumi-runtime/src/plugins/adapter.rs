use serde::{Deserialize, Serialize};

/// A forgiving pattern for the names a host shows (the TypeScript's `RegExp`; lookarounds need fancy_regex).
pub type Pattern = fancy_regex::Regex;

/// A pattern from its source, `(?i)` included where the TypeScript's had the `i` flag.
pub fn pattern(source: &str) -> Pattern {
    Pattern::new(source).unwrap_or_else(|error| panic!("plug-in pattern {source}: {error}"))
}

/// `pattern.test(text)`.
pub fn matches(pattern: &Pattern, text: &str) -> bool {
    pattern.is_match(text).unwrap_or(false)
}

#[derive(Debug, Clone)]
pub struct PluginParameterHint {
    pub role: &'static str,
    pub names: Pattern,
    pub about: &'static str,
}

impl PluginParameterHint {
    /// Whether a host's parameter name is one of this hint's.
    pub fn matches(&self, name: &str) -> bool {
        matches(&self.names, name)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum PluginKind {
    Instrument,
    Effect,
}

#[derive(Debug, Clone)]
pub struct PluginSection {
    pub name: &'static str,
    pub about: &'static str,
    pub parameters: Vec<PluginParameterHint>,
}

#[derive(Debug, Clone, Serialize)]
pub struct PluginRecipe {
    pub name: &'static str,
    pub how: &'static str,
}

/// Paths per OS, "~" for the home folder.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct OsPaths {
    pub mac: Option<&'static str>,
    pub windows: Option<&'static str>,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct PluginFolders {
    pub presets: Option<OsPaths>,
    pub wavetables: Option<OsPaths>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum WavetableFormat {
    Clm,
    Plain,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PluginWavetable {
    pub frame: u32,
    pub max_frames: u32,
    pub format: WavetableFormat,
}

#[derive(Debug, Clone)]
pub struct PluginAdapter {
    pub id: &'static str,
    pub name: &'static str,
    pub vendor: &'static str,
    pub kind: PluginKind,
    /// Live's name for the device (the plug-in's own name, maybe with a format or version suffix): what picks this adapter.
    pub matcher: Pattern, // TS: `match` (a keyword here)
    /// A few lines: what it is, its architecture and signal flow.
    pub overview: &'static str,
    /// Its parameters as hosts see them, by section: what each does, the names to expect (a forgiving pattern), and its unit in words.
    pub sections: Vec<PluginSection>,
    /// Sound-design moves that work in it (a reese, a pluck, a neuro growl; for effects, mixing and mastering moves).
    pub recipes: Vec<PluginRecipe>,
    /// What isn't a host parameter (wavetable editing, mod matrix routing, FX order, adding modules, the assistant) and how to get there: the plug-in's own window (Kumi can open it), its menus, files Kumi can write, or the producer.
    pub beyond: &'static str,
    /// Its user folders, per OS (~ for the home folder), where presets and wavetables go. Only folders you're confident of.
    pub folders: Option<PluginFolders>,
    /// It reads single-cycle wavetables from WAV files: samples per frame, most frames, and whether it reads Serum's "clm " chunk or plain frames.
    pub wavetable: Option<PluginWavetable>,
}

impl PluginAdapter {
    /// `adapter.match.test(name)`: whether Live's name for a device is this plug-in.
    pub fn matches(&self, device_name: &str) -> bool {
        matches(&self.matcher, device_name)
    }

    /// Every hint, section by section.
    pub fn hints(&self) -> impl Iterator<Item = &PluginParameterHint> {
        self.sections.iter().flat_map(|section| section.parameters.iter())
    }
}
