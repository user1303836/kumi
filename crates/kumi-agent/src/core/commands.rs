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
    /// The words it takes that the menu offers, as each is typed (tab puts the one chosen in).
    pub args: &'static [Arg],
    pub needs: Needs,
}

/// A word a command takes, and what the menu offers for it.
pub struct Arg {
    /// What the word is ("palette").
    pub name: &'static str,
    /// Which word it is after the command's name: 1 for the first.
    pub at: usize,
    /// The words before it that it follows (`use` for a palette's name); empty, any.
    pub after: &'static [&'static str],
    pub completes: Completes,
    /// It runs to the end of the line, spaces and all: a palette's or a conversation's name.
    pub to_end: bool,
}

/// What the menu offers for a word. Fixed words are the command's own; the rest the front end reads through the
/// session, once as the word starts.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Completes {
    Words(&'static [&'static str]),
    /// The producer's palettes, by name.
    Palettes,
    /// Kept conversations, by name or the start of their id.
    Conversations,
    /// The models a provider lists, by id.
    Models,
    /// The models the slot named by the word before can take.
    SlotModels,
}

const fn arg(name: &'static str, at: usize, after: &'static [&'static str], completes: Completes) -> Arg {
    Arg { name, at, after, completes, to_end: false }
}

/// A word that runs to the end of the line, spaces and all: a name.
const fn to_end(name: &'static str, at: usize, after: &'static [&'static str], completes: Completes) -> Arg {
    Arg { to_end: true, ..arg(name, at, after, completes) }
}

/// The slots `/slots` names, and the jobs `/slots back` takes back.
pub const JOBS: &[&str] = &["listening", "stems", "transcription", "embeddings"];

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
    pub voice: bool,
    pub updates: bool,
    pub switch_daw: bool,
    pub willington: bool,
}

impl Fronts {
    pub fn has(&self, front: Front) -> bool {
        match front {
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
    Command {
        args: &[to_end("conversation", 1, &[], Completes::Conversations)],
        ..command(
            "/resume",
            "Go back to a kept conversation, this {place}'s or any {place}'s: /resume <id or name>",
            &[],
            can(|c| c.conversations),
        )
    },
    Command {
        args: &[to_end("conversation", 1, &[], Completes::Conversations)],
        ..command("/conversations", "The same as /resume", &[], can(|c| c.conversations))
    },
    command("/rename", "Name this conversation: /rename <name>", &["title"], can(|c| c.conversations)),
    command("/fork", "Carry on in a copy of this conversation; the original stays", &["branch"], can(|c| c.conversations)),
    command("/reconnect", "Connect to {app} again, keeping the conversation", &[], can(|c| c.reconnect)),
    command("/daw", "Which DAW Kumi works with; /daw <name> opens Kumi again with another", &[], front(Front::SwitchDaw)),
    command("/undo", "Undo Kumi's last change", &["history", "revert"], ALWAYS),
    command(
        "/takeback",
        "Take back your last request: its changes, and its words to edit; /takeback 3, your last three",
        &["rewind", "back"],
        can(|c| c.take_back),
    ),
    command("/stop", "Stop {app}: clips, the transport and recording", &[], can(|c| c.stop_daw)),
    command("/refresh", "Read your {app} {place} again", &["reload"], ALWAYS),
    command("/copy", "Copy Kumi's last answer", &["clipboard"], ALWAYS),
    Command {
        args: &[arg("format", 1, &[], Completes::Words(&["copy", "md", "html", "json"]))],
        ..command("/export", "This conversation as a file: choose, or /export copy, md, html or json (for another Kumi)", &["save"], ALWAYS)
    },
    command("/share", "Save this conversation as a web page that opens anywhere, to send", &["link"], ALWAYS),
    Command {
        args: &[arg("model", 1, &[], Completes::Models)],
        ..command("/model", "Choose the model Kumi talks to", &[], can(|c| c.models))
    },
    command("/effort", "How hard the model thinks", &["reasoning"], can(|c| c.models)),
    command("/fast", "The model's faster tier, when its provider offers one", &["speed"], can(|c| c.models)),
    command("/willington", "Willington's bindings in {app}: macro mapping, zones", &[], front(Front::Willington)),
    Command {
        args: &[
            arg("slot", 1, &[], Completes::Words(&["listening", "stems", "transcription", "embeddings", "back"])),
            arg("slot", 2, &["back"], Completes::Words(JOBS)),
            // (after any word: a slot answers to other names too, "ears" for listening)
            arg("model", 2, &[], Completes::SlotModels),
        ],
        ..command("/slots", "Which model does each listening job: /slots listening gemini, /slots back <job>", &[], can(|c| c.slots))
    },
    Command {
        args: &[
            arg("what", 1, &[], Completes::Words(&["list", "new", "use", "rename", "forget", "strict", "loose"])),
            // (not after rename, which takes a new name)
            to_end("palette", 2, &["use", "forget"], Completes::Palettes),
        ],
        ..command(
            "/palette",
            "The sounds you gathered (Enter on /map): list, new, use, rename, forget; strict or loose",
            &["samples"],
            can(|c| c.palette),
        )
    },
    Command {
        args: &[arg("what", 1, &[], Completes::Words(&["add", "remove", "pause", "resume", "forget"]))],
        ..command("/music", "Your own music, which Kumi learns your taste from: folders, songs, pause, forget", &[], can(|c| c.music))
    },
    command("/map", "Your sounds and songs as a map: hear a point, find more like it; /map open in the browser", &[], can(|c| c.map)),
    command("/login", "Sign in to a provider", &["key"], can(|c| c.models)),
    command("/loop", "Listen, judge and adjust in rounds until the goal is met: /loop <what>, then stop", &[], ALWAYS),
    command("/goal", "Keep at one goal until it's met: /goal <what>, then resume, edit, pause, clear", &["objective"], can(|c| c.goal)),
    command("/memory", "What Kumi remembers", &["notes"], can(|c| c.memory)),
    command("/note", "Change a note's words: /note <id> <new words>", &[], can(|c| c.change_note)),
    command("/recipes", "Your saved ways of working", &[], can(|c| c.recipes)),
    command("/recipe", "Run a recipe now: /recipe <name> blank=value …", &[], can(|c| c.run_recipe)),
    command("/jobs", "What's running in the background: see, stop or run one again", &[], can(|c| c.jobs)),
    command("/logout", "Sign out of a provider", &[], can(|c| c.models)),
    command("/status", "What Kumi is connected to", &["connection"], ALWAYS),
    command("/update", "Get the newest Kumi", &["upgrade", "version"], front(Front::Updates)),
    command("/changelog", "What's new in Kumi", &["release", "news"], ALWAYS),
    command("/help", "Keys and commands", &[], ALWAYS),
    command("/quit", "Close Kumi", &["exit"], ALWAYS),
];

const fn command(name: &'static str, about: &'static str, finds: &'static [&'static str], needs: Needs) -> Command {
    Command { name, aliases: &[], about, finds, args: &[], needs }
}

/// The word a line is typing, where its command offers words for it: `/palette use Ki` is typing the palette (`Ki` so
/// far) after `/palette use `.
#[derive(Clone)]
pub struct Completing {
    pub command: &'static Command,
    pub arg: &'static Arg,
    /// The line before the word, its space included: what a candidate goes after.
    pub before: String,
    /// The word as typed so far.
    pub typed: String,
    /// The word before it, in lower case (the slot whose models are offered); none for a command's first.
    pub after: Option<String>,
}

/// What `line` is typing, if it's a word its command (one `offered`) offers words for: after the command's name and
/// a space, the words typed before it (any for a word that follows any, else one it follows), and the start of it. A
/// word that runs to the end of the line takes all that follows the words before it, spaces and all.
pub fn completing(line: &str, offered: impl Fn(&Command) -> bool) -> Option<Completing> {
    let (name, rest) = line.split_once(' ')?;
    let command = named(name).filter(|command| offered(command))?;
    // Each word after the command's name, with where it ends in the line.
    let mut at = name.len() + 1;
    let mut words: Vec<(usize, &str)> = vec![];
    for word in rest.split(' ') {
        at += word.len();
        if !word.is_empty() {
            words.push((at, word));
        }
        at += 1;
    }
    command.args.iter().find_map(|arg| {
        let (from, before): (usize, Vec<&str>) = if arg.to_end {
            match arg.at - 1 {
                0 => (name.len() + 1, vec![]),
                // Typed once a space follows the words before it.
                before => {
                    let &(end, _) = words.get(before - 1).filter(|(end, _)| *end < line.len())?;
                    (end + 1, words[..before].iter().map(|(_, word)| *word).collect())
                }
            }
        } else {
            // The word after the last space, with every word before it.
            let from = line.rfind(' ')? + 1;
            let before: Vec<&str> = words.iter().filter(|(end, _)| *end < from).map(|(_, word)| *word).collect();
            if before.len() + 1 != arg.at {
                return None;
            }
            (from, before)
        };
        let after = before.last().map(|word| word.to_lowercase());
        if !arg.after.is_empty() && !after.as_deref().is_some_and(|after| arg.after.contains(&after)) {
            return None;
        }
        Some(Completing { command, arg, before: line[..from].to_string(), typed: line[from..].to_string(), after })
    })
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

/// Words too common in what the commands do for the menu to find one by: most of the table says them.
const COMMON: &[&str] = &[
    "about",
    "again",
    "and",
    "another",
    "any",
    "are",
    "does",
    "each",
    "for",
    "from",
    "how",
    "instead",
    "into",
    "its",
    "keeping",
    "kumi",
    "like",
    "more",
    "now",
    "once",
    "one",
    "out",
    "own",
    "same",
    "something",
    "that",
    "the",
    "then",
    "this",
    "until",
    "what",
    "when",
    "which",
    "with",
    "without",
    "works",
    "you",
    "your",
];

/// How what's typed found a command in the menu.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum By {
    /// Typed in full.
    Typed,
    /// Its name begins with what's typed.
    Name,
    /// Another name of it does.
    Alias,
    /// A word of what it does (or of `finds`) does. The producer didn't name it, so Enter doesn't run it unchosen.
    Words,
}

/// A command the menu shows, and how what's typed found it.
#[derive(Clone, Copy)]
pub struct Found {
    pub command: &'static Command,
    pub by: By,
}

impl Found {
    /// Whether what's typed names it (in full, or the start of a name), so Enter may run it as it stands.
    pub fn named(&self) -> bool {
        self.by != By::Words
    }
}

/// The commands the menu shows for what's typed (`/` and the start of a name), each with how it was found. Those whose
/// name begins with it come first, then those another name of theirs begins with, then those a word of what they do
/// (or are found by) begins with, common words aside. Within each, the ones used lately come first (`recent`, latest
/// first), then the table's order; one typed in full comes before them all.
pub fn matching(typed: &str, recent: &[&str], offered: impl Fn(&Command) -> bool) -> Vec<Found> {
    let Some(word) = typed.strip_prefix('/') else { return vec![] };
    let word = word.to_lowercase();
    let says = |command: &Command| {
        word.len() >= WORD_LEAST
            && command
                .about
                .replace("{app}", "")
                .replace("{place}", "")
                .split(|c: char| !c.is_alphanumeric())
                .map(str::to_lowercase)
                .filter(|said| !COMMON.contains(&said.as_str()))
                .chain(command.finds.iter().map(|find| find.to_string()))
                .any(|said| said.starts_with(&word))
    };
    let by = |command: &Command| -> Option<By> {
        if command.name == typed {
            Some(By::Typed)
        } else if command.name.starts_with(typed) {
            Some(By::Name)
        } else if command.aliases.iter().any(|alias| alias.starts_with(typed)) {
            Some(By::Alias)
        } else if says(command) {
            Some(By::Words)
        } else {
            None
        }
    };
    let lately = |command: &Command| recent.iter().position(|used| *used == command.name).unwrap_or(usize::MAX);
    let mut found: Vec<(By, usize, usize, &'static Command)> = COMMANDS
        .iter()
        .enumerate()
        .filter(|(_, command)| offered(command))
        .filter_map(|(at, command)| by(command).map(|by| (by, lately(command), at, command)))
        .collect();
    found.sort_by_key(|(by, lately, at, _)| (*by, *lately, *at));
    found.into_iter().map(|(by, _, _, command)| Found { command, by }).collect()
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
    Effort,
    /// The model picker (no words), or the model the words name, by its id.
    Model {
        words: String,
    },
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
    /// `/takeback [n]`: the last request taken back, or the last n.
    TakeBack {
        words: String,
    },
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
    // A name no command has is unknown whatever Kumi is doing: while it works, there's nothing to run once it's done.
    let verb = line.split_whitespace().next().unwrap_or_default();
    if named(verb).is_none() {
        return Effect::Unknown { name: verb.to_string() };
    }
    let (can, fronts) = (&now.can, &now.fronts);
    let with_words = |name: &str| line == name || line.starts_with(&format!("{name} "));
    match line {
        "/quit" => return Effect::Quit,
        "/help" => return Effect::Help,
        "/changelog" => return Effect::Changelog,
        "/jobs" if can.jobs => return Effect::Jobs,
        "/voice" if fronts.voice => return Effect::Voice,
        "/fast" if can.models => return Effect::Fast,
        "/effort" if can.models => return Effect::Effort,
        "/login" if can.models => return Effect::Login,
        "/logout" if can.models => return Effect::Logout,
        "/recipes" if can.recipes => return Effect::Recipes,
        "/memory" if can.memory => return Effect::Memory,
        _ => {}
    }
    if let Some(words) = argument(line, "/model").filter(|_| can.models) {
        return Effect::Model { words };
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
        return Effect::Refused(Why::Busy { verb: verb.to_string() });
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
    if let Some(words) = argument(line, "/takeback") {
        return Effect::TakeBack { words };
    }
    if let Some(words) = argument(line, "/export") {
        return Effect::Export { words };
    }
    if let Some(name) = argument(line, "/new") {
        return Effect::New { name };
    }
    Effect::Unknown { name: verb.to_string() }
}
