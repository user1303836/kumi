//! The commands a front end offers, from one table ([`COMMANDS`]): each command's name, what it does, the words the `/`
//! menu also finds it by, and what it needs. A line is read into what it asks for
//! ([`run`]), and each front end carries that out in its own words.

use crate::core::{goal_mode::command_word, protocol::Capabilities};
use kumi_common::js::string::trim;
use kumi_types::daw::DawWords;

/// One command, as the menu and the help show it.
pub struct Command {
    pub name: &'static str,
    /// Other names it answers to, with no row of their own: the menu lists the command for them.
    pub aliases: &'static [&'static str],
    /// What it does, with `{app}` and `{place}` for the DAW's words ([`Command::about`] puts them in).
    pub about: &'static str,
    /// Words the menu also finds it by, that it doesn't show ("history" for /undo).
    pub finds: &'static [&'static str],
    pub needs: Needs,
}

/// What a command needs to be offered: something the session can do, or something the front end has itself.
#[derive(Clone, Copy)]
pub struct Needs {
    pub can: Option<fn(&Capabilities) -> bool>,
    pub front: Option<Front>,
}

const ALWAYS: Needs = Needs { can: None, front: None };
const fn can(can: fn(&Capabilities) -> bool) -> Needs {
    Needs { can: Some(can), front: None }
}
const fn front(front: Front) -> Needs {
    Needs { can: None, front: Some(front) }
}

/// What a front end has itself, that a command may need.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Front {
    /// Choosing a model and signing in.
    Models,
    /// Talking instead of typing.
    Voice,
    /// Getting a newer Kumi.
    Updates,
    /// Opening Kumi again with another DAW.
    SwitchDaw,
    /// Willington's bindings in Live.
    Willington,
}

/// Which of [`Front`] a front end has.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Fronts {
    pub models: bool,
    pub voice: bool,
    pub updates: bool,
    pub switch_daw: bool,
    pub willington: bool,
}

impl Fronts {
    pub fn has(&self, front: Front) -> bool {
        match front {
            Front::Models => self.models,
            Front::Voice => self.voice,
            Front::Updates => self.updates,
            Front::SwitchDaw => self.switch_daw,
            Front::Willington => self.willington,
        }
    }
}

impl Command {
    /// What the command does, in the DAW's words (`live`: Live, where /stop stops clips too).
    pub fn about(&self, words: &DawWords, live: bool) -> String {
        match self.name {
            "/stop" if !live => format!("Stop {}: the transport and recording", words.app),
            _ => self.about.replace("{app}", words.app).replace("{place}", words.place),
        }
    }
    /// Whether a front end offers it, in the menu and the help: what it needs, where this session and front end have it.
    pub fn offered(&self, can: &Capabilities, fronts: &Fronts) -> bool {
        self.needs.can.is_none_or(|needs| needs(can)) && self.needs.front.is_none_or(|front| fronts.has(front))
    }
}

/// Every command, in the order the menu and the help list them.
pub const COMMANDS: &[Command] = &[
    command("/new", "Start a fresh conversation (the last one is kept); /new <name> names it", &[], ALWAYS),
    command("/btw", "Ask something on the side, without interrupting Kumi", &["aside", "question"], can(|c| c.aside)),
    command("/voice", "Talk instead of typing: ctrl+t, and how it listens", &["microphone", "speak"], front(Front::Voice)),
    command(
        "/resume",
        "Go back to a kept conversation, this {place}'s or any {place}'s: /resume <id or name>",
        &[],
        can(|c| c.conversations),
    ),
    command("/conversations", "The same as /resume", &[], can(|c| c.conversations)),
    command("/rename", "Name this conversation: /rename <name>", &["title"], can(|c| c.conversations)),
    command("/fork", "Carry on in a copy of this conversation; the original stays", &["branch"], can(|c| c.conversations)),
    command("/reconnect", "Connect to {app} again, keeping the conversation", &[], can(|c| c.reconnect)),
    command("/daw", "Which DAW Kumi works with; /daw <name> opens Kumi again with another", &[], front(Front::SwitchDaw)),
    command("/undo", "Undo Kumi's last change", &["history", "revert"], ALWAYS),
    command("/stop", "Stop {app}: clips, the transport and recording", &[], can(|c| c.stop_daw)),
    command("/refresh", "Read your {app} {place} again", &["reload"], ALWAYS),
    command("/copy", "Copy Kumi's last answer", &["clipboard"], ALWAYS),
    command("/export", "This conversation as a file: choose, or /export copy, md, html or json (for another Kumi)", &["save"], ALWAYS),
    command("/share", "Save this conversation as a web page that opens anywhere, to send", &["link"], ALWAYS),
    command("/model", "Choose the model Kumi talks to", &[], front(Front::Models)),
    command("/effort", "How hard the model thinks", &["reasoning"], front(Front::Models)),
    command("/fast", "The model's faster tier, when its provider offers one", &["speed"], front(Front::Models)),
    command("/willington", "Willington's bindings in {app}: macro mapping, zones", &[], front(Front::Willington)),
    command("/slots", "Which model does each listening job: /slots listening gemini, /slots back <job>", &[], can(|c| c.slots)),
    command(
        "/palette",
        "The sounds you gathered (Enter on /map): list, new, use, rename, forget; strict or loose",
        &["samples"],
        can(|c| c.palette),
    ),
    command("/music", "Your own music, which Kumi learns your taste from: folders, songs, pause, forget", &[], can(|c| c.music)),
    command("/map", "Your sounds and songs as a map: hear a point, find more like it; /map open in the browser", &[], can(|c| c.map)),
    command("/login", "Sign in to a provider", &["key"], front(Front::Models)),
    command("/loop", "Listen, judge and adjust in rounds until the goal is met: /loop <what>, then stop", &[], ALWAYS),
    command("/goal", "Keep at one goal until it's met: /goal <what>, then resume, edit, pause, clear", &["objective"], can(|c| c.goal)),
    command("/memory", "What Kumi remembers", &["notes"], can(|c| c.memory)),
    command("/note", "Change a note's words: /note <id> <new words>", &[], can(|c| c.change_note)),
    command("/recipes", "Your saved ways of working", &[], can(|c| c.recipes)),
    command("/recipe", "Run a recipe now: /recipe <name> blank=value …", &[], can(|c| c.run_recipe)),
    command("/jobs", "What's running in the background: see, stop or run one again", &[], can(|c| c.jobs)),
    command("/logout", "Sign out of a provider", &[], front(Front::Models)),
    command("/status", "What Kumi is connected to", &["connection"], ALWAYS),
    command("/update", "Get the newest Kumi", &["upgrade", "version"], front(Front::Updates)),
    command("/changelog", "What's new in Kumi", &["release", "news"], ALWAYS),
    command("/help", "Keys and commands", &[], ALWAYS),
    command("/quit", "Close Kumi", &["exit"], ALWAYS),
];

const fn command(name: &'static str, about: &'static str, finds: &'static [&'static str], needs: Needs) -> Command {
    Command { name, aliases: &[], about, finds, needs }
}

/// The command named `name`, or one that has it as another name.
pub fn named(name: &str) -> Option<&'static Command> {
    COMMANDS.iter().find(|command| command.name == name || command.aliases.contains(&name))
}

/// A command by the full-screen app's rule: `/`, letters, then the end or a space. A line that starts with a path (a
/// file dragged in) is a message.
pub fn is_command(text: &str) -> bool {
    let Some(rest) = text.strip_prefix('/') else { return false };
    let letters = rest.bytes().take_while(u8::is_ascii_alphabetic).count();
    letters > 0 && (letters == rest.len() || rest[letters..].chars().next().is_some_and(|c| trim(&c.to_string()).is_empty()))
}

/// The words after `name`, when `command` is it (alone, or with words after a space).
pub fn argument(command: &str, name: &str) -> Option<String> {
    let rest = command.strip_prefix(name)?;
    (rest.is_empty() || rest.starts_with(char::is_whitespace)).then(|| trim(rest).to_string())
}

/// `/goal`'s or `/loop`'s word, read as their control words are: in any case, with trailing punctuation.
pub fn subcommand(command: &str, name: &str) -> Option<String> {
    let rest = command.strip_prefix(name)?;
    (rest.is_empty() || rest.starts_with(char::is_whitespace)).then(|| command_word(rest))
}

/// How many letters a word of what a command does needs typed before the menu finds the command by it, so `/s` lists
/// the commands that start with it and not every one that says "see".
const WORD_LEAST: usize = 3;

/// The commands the menu shows for what's typed (`/` and the start of a name). Those whose name begins with it come
/// first, then those another name of theirs begins with, then those a word of what they do (or are found by) begins
/// with. Within each, the ones used lately come first (`recent`, latest first), then the table's order; one typed in
/// full comes before them all.
pub fn matching(typed: &str, recent: &[&str], offered: impl Fn(&Command) -> bool) -> Vec<&'static Command> {
    let Some(word) = typed.strip_prefix('/') else { return vec![] };
    let word = word.to_lowercase();
    let says = |command: &Command| {
        word.len() >= WORD_LEAST
            && command
                .about
                .replace("{app}", "")
                .replace("{place}", "")
                .split(|c: char| !c.is_alphanumeric())
                .chain(command.finds.iter().copied())
                .any(|said| said.to_lowercase().starts_with(&word))
    };
    let tier = |command: &Command| -> Option<u8> {
        if command.name == typed {
            Some(0)
        } else if command.name.starts_with(typed) {
            Some(1)
        } else if command.aliases.iter().any(|alias| alias.starts_with(typed)) {
            Some(2)
        } else if says(command) {
            Some(3)
        } else {
            None
        }
    };
    let lately = |command: &Command| recent.iter().position(|used| *used == command.name).unwrap_or(usize::MAX);
    let mut found: Vec<(u8, usize, usize, &'static Command)> = COMMANDS
        .iter()
        .enumerate()
        .filter(|(_, command)| offered(command))
        .filter_map(|(at, command)| tier(command).map(|tier| (tier, lately(command), at, command)))
        .collect();
    found.sort_by_key(|(tier, lately, at, _)| (*tier, *lately, *at));
    found.into_iter().map(|(_, _, _, command)| command).collect()
}

/// What the front end knows as a line is read.
pub struct Now {
    /// Kumi is working: an answer, a goal, a loop.
    pub busy: bool,
    /// A /goal objective or a sound-match search is under way (/goal alone shows its tab).
    pub goal_running: bool,
    pub can: Capabilities,
    pub fronts: Fronts,
}

/// Why a command can't run now. Each front end says it in its own words.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Why {
    /// A chat that keeps no conversations was asked about them.
    NotKept,
    /// Kumi is working: `verb` runs once it's done.
    Busy { verb: String },
}

/// What a line asks for, which the front end carries out. A command's words, where the front end reads them itself,
/// are still in the line.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Effect {
    Quit,
    Help,
    Changelog,
    Jobs,
    Voice,
    Fast,
    Model,
    Effort,
    Login,
    Logout,
    Recipes,
    Memory,
    /// A kept conversation: the list (no word), or the one the word gives.
    Resume {
        word: String,
    },
    /// A name for this conversation, or (`<id> <name>`) a kept one; no words, the name it has now to change.
    Rename {
        words: String,
    },
    /// Carry on in a copy, named or not.
    Fork {
        name: String,
    },
    Recipe,
    Note,
    Stop,
    Daw,
    Update,
    Slots,
    Music,
    Map,
    Palette,
    Status,
    Btw,
    /// `/goal pause` or `/goal hold`: stop what works, or say there's no goal to pause.
    PauseGoal,
    /// `/loop pause`.
    PauseLoop,
    /// `/goal stop` (end, clear, cancel, done).
    StopGoal,
    /// `/loop stop` (end, cancel).
    StopLoop,
    /// `/goal` alone while a goal works: its tab.
    ShowGoal,
    Goal,
    /// `/loop <what>`: the line goes as a request.
    Loop,
    Undo,
    Copy,
    /// This conversation as a file: the choices (no words), or the format named.
    Export {
        words: String,
    },
    Share,
    Willington,
    Refresh,
    /// A fresh conversation, named or not.
    New {
        name: String,
    },
    Reconnect,
    Refused(Why),
    /// A command there's none of here: `name` as typed.
    Unknown {
        name: String,
    },
    /// Not a command: words for the model.
    Message,
}

/// What `line` (trimmed) asks for. The order is the full-screen app's: a command a front end doesn't offer reads as
/// unknown, or, while Kumi works, as one to run once it's done.
pub fn run(line: &str, now: &Now) -> Effect {
    if !is_command(line) {
        return Effect::Message;
    }
    let (can, fronts) = (&now.can, &now.fronts);
    let with_words = |name: &str| line == name || line.starts_with(&format!("{name} "));
    match line {
        "/quit" => return Effect::Quit,
        "/help" => return Effect::Help,
        "/changelog" => return Effect::Changelog,
        "/jobs" if can.jobs => return Effect::Jobs,
        "/voice" if fronts.voice => return Effect::Voice,
        "/fast" if fronts.models => return Effect::Fast,
        "/model" if fronts.models => return Effect::Model,
        "/effort" if fronts.models => return Effect::Effort,
        "/login" if fronts.models => return Effect::Login,
        "/logout" if fronts.models => return Effect::Logout,
        "/recipes" if can.recipes => return Effect::Recipes,
        "/memory" if can.memory => return Effect::Memory,
        _ => {}
    }
    if !can.conversations && ["/resume", "/conversations", "/rename", "/fork"].into_iter().any(|verb| argument(line, verb).is_some()) {
        return Effect::Refused(Why::NotKept);
    }
    if can.conversations {
        if let Some(word) = argument(line, "/resume").or_else(|| argument(line, "/conversations")) {
            return Effect::Resume { word };
        }
        if let Some(words) = argument(line, "/rename") {
            return Effect::Rename { words };
        }
        if let Some(name) = argument(line, "/fork") {
            return Effect::Fork { name };
        }
    }
    if with_words("/recipe") && can.run_recipe {
        return Effect::Recipe;
    }
    if with_words("/note") && can.change_note {
        return Effect::Note;
    }
    if line == "/stop" && can.stop_daw {
        return Effect::Stop;
    }
    if with_words("/daw") && fronts.switch_daw {
        return Effect::Daw;
    }
    if line == "/update" && fronts.updates {
        return Effect::Update;
    }
    if with_words("/slots") && can.slots {
        return Effect::Slots;
    }
    if with_words("/music") && can.music {
        return Effect::Music;
    }
    if with_words("/map") && can.map {
        return Effect::Map;
    }
    if with_words("/palette") && can.palette {
        return Effect::Palette;
    }
    if line == "/status" {
        return Effect::Status;
    }
    if with_words("/btw") && can.aside {
        return Effect::Btw;
    }
    // /goal's and /loop's control words, in any case and with trailing punctuation: never a goal or a request.
    let goal_word = subcommand(line, "/goal");
    let loop_word = subcommand(line, "/loop");
    if loop_word.as_deref() == Some("pause") {
        return Effect::PauseLoop;
    }
    if matches!(goal_word.as_deref(), Some("pause" | "hold")) {
        return Effect::PauseGoal;
    }
    if matches!(goal_word.as_deref(), Some("stop" | "end" | "clear" | "cancel" | "done")) && can.stop_goal {
        return Effect::StopGoal;
    }
    if matches!(loop_word.as_deref(), Some("stop" | "end" | "cancel")) {
        return Effect::StopLoop;
    }
    if line == "/goal" && now.busy && now.goal_running {
        return Effect::ShowGoal;
    }
    if now.busy {
        return Effect::Refused(Why::Busy { verb: line.split_whitespace().next().unwrap_or_default().to_string() });
    }
    if with_words("/goal") && can.goal {
        return Effect::Goal;
    }
    if with_words("/loop") {
        return Effect::Loop;
    }
    match line {
        "/undo" => return Effect::Undo,
        "/copy" => return Effect::Copy,
        "/share" => return Effect::Share,
        "/willington" if fronts.willington => return Effect::Willington,
        "/refresh" => return Effect::Refresh,
        "/reconnect" if can.reconnect => return Effect::Reconnect,
        _ => {}
    }
    if let Some(words) = argument(line, "/export") {
        return Effect::Export { words };
    }
    if let Some(name) = argument(line, "/new") {
        return Effect::New { name };
    }
    Effect::Unknown { name: line.split_whitespace().next().unwrap_or_default().to_string() }
}
