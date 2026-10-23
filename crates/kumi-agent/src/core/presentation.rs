//! How the app shows a tool at work: the step's words in the transcript ("looked for sounds"), what NOW says while it
//! runs ("looking for sounds"), the kind of task it is (the glyph that turns beside it), and how NOW treats it. Kumi's
//! own tools are shown alike in every DAW, in its words; a DAW's integration shows its own (`ToolPresentation`).
use super::daw::DawProfile;

/// The kind of task a tool's step is.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ToolActivity {
    Search,
    Read,
    Look,
    Build,
    Change,
    Listen,
    Watch,
    Play,
    Record,
    Code,
}

/// A DAW's own tools, as the app shows them.
#[derive(Clone, Copy)]
pub struct ToolPresentation {
    /// A step's words in the transcript; None for a tool it doesn't know.
    pub label: fn(&str) -> Option<&'static str>,
    /// What NOW says while it runs.
    pub doing: fn(&str) -> Option<&'static str>,
    pub activity: fn(&str) -> Option<ToolActivity>,
    /// A plan: its steps arrive as it's written, and NOW shows its changes as they land.
    pub plan: fn(&str) -> bool,
    /// What NOW shows the DAW doing while it runs (a plan's actions, playing, launching, showing).
    pub acts: fn(&str) -> bool,
    /// What the DAW's bridge reads start with ("live_"): an unknown one looks at the project, and its words drop it.
    pub read_prefix: Option<&'static str>,
}

impl ToolPresentation {
    /// No DAW's tools: Kumi's own are shown, the rest by their names.
    pub const NONE: ToolPresentation =
        ToolPresentation { label: |_| None, doing: |_| None, activity: |_| None, plan: |_| false, acts: |_| false, read_prefix: None };
}

/// Kumi's own tools: their words in the transcript and while they run, and their kind.
fn own(profile: &DawProfile, tool: &str) -> Option<(String, Option<String>, Option<ToolActivity>)> {
    use ToolActivity::*;
    let places = profile.words.places;
    let said = |done: &str, doing: Option<&str>, kind: Option<ToolActivity>| Some((done.to_string(), doing.map(str::to_string), kind));
    match tool {
        "listen" => said("listened", Some("listening"), Some(Listen)),
        "reference" => said("measured a reference", Some("measuring a reference"), Some(Listen)),
        "run_recipe" => said("ran a recipe", Some("running a recipe"), None),
        "save_recipe" => said("saved a recipe", None, None),
        "find_sounds" => said("looked for sounds", Some("looking for sounds"), Some(Search)),
        "find_presets" => said("looked for presets", Some("looking for presets"), Some(Search)),
        "my_sets" => Some((format!("looked through your {places}"), Some(format!("looking through your {places}")), Some(Search))),
        "watch_video" => said("watched a video", Some("watching the video"), Some(Watch)),
        "search_web" => said("searched the web", Some("searching the web"), Some(Search)),
        "read_web" => said("read a page", Some("reading a page"), Some(Read)),
        "search_conversations" => said("looked through earlier conversations", Some("looking through earlier conversations"), Some(Search)),
        _ => None,
    }
}

/// A step's words in the transcript: Kumi's own tool's, the DAW's, or its name in plain words.
pub fn step_label(shown: &ToolPresentation, profile: &DawProfile, tool: &str) -> String {
    if let Some((done, _, _)) = own(profile, tool) {
        return done;
    }
    if let Some(label) = (shown.label)(tool) {
        return label.into();
    }
    shown.read_prefix.and_then(|prefix| tool.strip_prefix(prefix)).unwrap_or(tool).replace('_', " ")
}

/// What NOW says while a tool runs; `fallback` when neither Kumi nor the DAW says, nothing for no tool.
pub fn doing_label(shown: &ToolPresentation, profile: &DawProfile, tool: Option<&str>, fallback: &str) -> String {
    if tool == Some("") {
        return String::new();
    }
    let tool = tool.unwrap_or("");
    if let Some((_, doing, _)) = own(profile, tool) {
        return doing.unwrap_or_else(|| fallback.into());
    }
    (shown.doing)(tool).map(str::to_string).unwrap_or_else(|| fallback.into())
}

/// The kind of task a tool is: Kumi's own, the DAW's, a DAW read, or a change.
pub fn activity(shown: &ToolPresentation, profile: &DawProfile, tool: &str) -> ToolActivity {
    let known = match own(profile, tool) {
        Some((_, _, kind)) => kind,
        None => (shown.activity)(tool),
    };
    known.unwrap_or(if shown.read_prefix.is_some_and(|prefix| tool.starts_with(prefix)) {
        ToolActivity::Look
    } else {
        ToolActivity::Change
    })
}
