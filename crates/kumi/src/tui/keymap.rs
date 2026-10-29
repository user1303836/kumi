//! Kumi's keys, as actions. Each thing a key does is an action with an id ("composer.undo", "pane.toggle") and the
//! scopes it works in, bound by default to the keys it has always had. `~/.kumi/keys.json` gives an action other keys,
//! or none:
//!
//! ```json
//! { "composer.undo": ["ctrl+z"], "app.background": ["ctrl+b", "f5"], "pane.toggle": [] }
//! ```
//!
//! Each scope reads its keys apart (enter sends in the box and chooses in a list), and one key can be several actions
//! there, tried in turn: an action that doesn't apply just now (tab while there's nothing to send after the answer)
//! lets the next one have the key. keys.json's keys come first, then the defaults, in the order of [`ACTIONS`], which
//! is the order the keys were always tried in. These are the full-screen app's keys: plain mode reads lines, and the
//! map and setup keep keys of their own.
use serde_json::Value;

use super::keys::{InputEvent, Modifiers};

/// A key as the terminal says it: its name ("enter", "a", "?", "f5", "\\") and the modifiers held with it.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct Chord {
    pub key: String,
    pub mods: Modifiers,
}

/// The keys whose names are longer than a character, as the terminal names them.
const NAMED: &[&str] = &[
    "enter",
    "tab",
    "escape",
    "backspace",
    "space",
    "up",
    "down",
    "left",
    "right",
    "home",
    "end",
    "insert",
    "delete",
    "pageup",
    "pagedown",
    "f1",
    "f2",
    "f3",
    "f4",
    "f5",
    "f6",
    "f7",
    "f8",
    "f9",
    "f10",
    "f11",
    "f12",
];

/// Other ways of writing those.
const ALSO_CALLED: &[(&str, &str)] = &[
    ("esc", "escape"),
    ("return", "enter"),
    ("del", "delete"),
    ("ins", "insert"),
    ("pgup", "pageup"),
    ("page-up", "pageup"),
    ("pgdn", "pagedown"),
    ("pgdown", "pagedown"),
    ("page-down", "pagedown"),
    ("↑", "up"),
    ("↓", "down"),
    ("←", "left"),
    ("→", "right"),
];

impl Chord {
    pub fn new(key: &str, mods: Modifiers) -> Chord {
        Chord { key: key.to_string(), mods }
    }

    /// Reads a key as keys.json writes it: "ctrl+shift+tab", "alt+up" or "alt+↑", "ctrl+-", "ctrl++", "f5", "esc". None
    /// when it isn't a key: a name or a modifier Kumi doesn't know, or modifiers alone.
    pub fn parse(words: &str) -> Option<Chord> {
        let (chord, also) = read(words, false)?;
        (also == Modifiers::NONE).then_some(chord)
    }

    /// The key an input event is: a key pressed, or one character typed, which is a key where letters are (the tabs, a
    /// side answer's `c`). Anything else (a paste, the mouse) is none.
    pub fn of(event: &InputEvent) -> Option<Chord> {
        match event {
            InputEvent::Key { name, mods, .. } => Some(Chord::new(name, *mods)),
            InputEvent::Text { text } => {
                let mut chars = text.chars();
                let (Some(typed), None) = (chars.next(), chars.next()) else { return None };
                Some(match typed {
                    ' ' => Chord::new("space", Modifiers::NONE),
                    // An uppercase letter is its letter with shift, as the terminal sends a key it can name.
                    typed if typed.is_uppercase() => Chord { key: typed.to_lowercase().collect(), mods: Modifiers::SHIFT },
                    typed => Chord::new(&typed.to_string(), Modifiers::NONE),
                })
            }
            _ => None,
        }
    }

    /// How the help and the box's hints write it: "ctrl+z", "alt+↑", "page up", "esc".
    pub fn label(&self) -> String {
        let key = match self.key.as_str() {
            "escape" => "esc",
            "up" => "↑",
            "down" => "↓",
            "left" => "←",
            "right" => "→",
            "pageup" => "page up",
            "pagedown" => "page down",
            key => key,
        };
        let held = [(self.mods.ctrl, "ctrl+"), (self.mods.alt, "alt+"), (self.mods.shift, "shift+")];
        held.iter().filter(|(on, _)| *on).map(|(_, word)| *word).collect::<String>() + key
    }

    /// A character typed rather than a key: one character (a space too), without ctrl or alt. Where what's typed is
    /// words, it's words.
    pub fn typed(&self) -> bool {
        !self.mods.ctrl && !self.mods.alt && (self.key == "space" || self.key.chars().count() == 1)
    }
}

/// How soon the second of keys pressed one after another (esc esc) comes after the first.
pub const AGAIN_MS: f64 = 500.;

/// Keys pressed one after another (esc esc), written with a space between.
pub fn label(keys: &[Chord]) -> String {
    keys.iter().map(Chord::label).collect::<Vec<_>>().join(" ")
}

/// A key and the modifiers that may be held with it besides (`shift?+left`, `any+escape`), which only Kumi's own
/// defaults write: those keys did the same with them before keys had names, and still do.
fn read(words: &str, loose: bool) -> Option<(Chord, Modifiers)> {
    let words = words.trim();
    // The key follows the last `+`, but a `+` at the end is the key itself ("ctrl++").
    let (held, key) = match words.strip_suffix('+') {
        Some(rest) if rest.is_empty() || rest.ends_with('+') => (rest.strip_suffix('+').unwrap_or(rest), "+"),
        _ => words.rsplit_once('+').unwrap_or(("", words)),
    };
    let (mut mods, mut also) = (Modifiers::NONE, Modifiers::NONE);
    for word in held.split('+').filter(|_| !held.is_empty()) {
        let word = word.trim().to_lowercase();
        if loose && word == "any" {
            also = Modifiers { ctrl: true, alt: true, shift: true };
            continue;
        }
        let (word, optional) = match word.strip_suffix('?') {
            Some(_) if !loose => return None,
            Some(word) => (word, true),
            None => (word.as_str(), false),
        };
        let set = if optional { &mut also } else { &mut mods };
        match word {
            "ctrl" | "control" => set.ctrl = true,
            "alt" | "option" | "opt" | "meta" => set.alt = true,
            "shift" => set.shift = true,
            _ => return None,
        }
    }
    let lower = key.trim().to_lowercase();
    let named = ALSO_CALLED.iter().find(|(name, _)| *name == lower).map(|(_, name)| *name);
    let key = match named.or_else(|| NAMED.iter().copied().find(|name| *name == lower)) {
        Some(name) => name.to_string(),
        None => {
            let mut chars = key.trim().chars();
            let (Some(key), None) = (chars.next(), chars.next()) else { return None };
            // "Z" is what shift+z types; with ctrl or alt a letter's case says nothing.
            if key.is_uppercase() && !mods.ctrl && !mods.alt {
                mods.shift = true;
            }
            key.to_lowercase().collect()
        }
    };
    Some((Chord { key, mods }, also))
}

/// A default's keys, one or two pressed one after another (`any+escape any+escape`), and the modifiers they may be held
/// with besides.
fn read_default(words: &str) -> Option<(Vec<Chord>, Modifiers)> {
    let mut also = Modifiers::NONE;
    let keys: Option<Vec<Chord>> = words
        .split_whitespace()
        .map(|word| {
            read(word, true).map(|(chord, more)| {
                also = more;
                chord
            })
        })
        .collect();
    keys.filter(|keys| (1..=2).contains(&keys.len())).map(|keys| (keys, also))
}

/// Where a key is pressed. Each scope reads its keys apart: enter sends in the box and chooses in a list.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Scope {
    /// The input box, and the conversation it scrolls.
    Box,
    /// The `/` menu, over the box while a command is being typed.
    Menu,
    /// The pane's tabs, after shift+tab.
    Tabs,
    /// FOCUS's tree, after tab.
    Tree,
    /// While Kumi listens (ctrl+t).
    Voice,
    /// While looking back through what was sent (ctrl+r).
    Search,
    /// While text is selected, or just after a copy.
    Selection,
    Help,
    Aside,
    Job,
    List,
    KeyBox,
    ChatGpt,
    /// The conversation in detail (ctrl+o).
    Details,
    /// While a search is typed in the detailed view.
    DetailsSearch,
}

impl Scope {
    /// Every scope.
    pub const ALL: &'static [Scope] = &[
        Scope::Box,
        Scope::Menu,
        Scope::Tabs,
        Scope::Tree,
        Scope::Voice,
        Scope::Search,
        Scope::Selection,
        Scope::Help,
        Scope::Aside,
        Scope::Job,
        Scope::List,
        Scope::KeyBox,
        Scope::ChatGpt,
        Scope::Details,
        Scope::DetailsSearch,
    ];

    /// Where what's typed is words (or goes on to the box): a character bound there would stop it being typed.
    pub fn types(self) -> bool {
        matches!(
            self,
            Scope::Box | Scope::Menu | Scope::Tree | Scope::Voice | Scope::Search | Scope::Selection | Scope::List | Scope::KeyBox
        )
    }

    /// Where it is, as Kumi says it to the producer.
    pub fn words(self) -> &'static str {
        match self {
            Scope::Box => "in the box",
            Scope::Menu => "in the / menu",
            Scope::Tabs => "in the tabs",
            Scope::Tree => "in FOCUS's tree",
            Scope::Voice => "while Kumi listens",
            Scope::Search => "while looking back through what was sent",
            Scope::Selection => "over selected text",
            Scope::Help => "in the help",
            Scope::Aside => "in a side answer",
            Scope::Job => "in a job's details",
            Scope::List => "in a list",
            Scope::KeyBox => "in a key's box",
            Scope::ChatGpt => "in ChatGPT's sign-in",
            Scope::Details => "in the detailed view",
            Scope::DetailsSearch => "while searching the detailed view",
        }
    }
}

/// One thing a key does: its id, the scopes it works in, its keys by default, and what it does.
pub struct Action {
    pub id: &'static str,
    pub scopes: &'static [Scope],
    /// Its keys by default. A modifier written with `?` ("shift?+left") may be held or not, and `any+` is all three:
    /// the key did the same with them before it had a name. keys.json's keys are exact.
    pub keys: &'static [&'static str],
    pub does: &'static str,
}

const fn act(id: &'static str, scopes: &'static [Scope], keys: &'static [&'static str], does: &'static str) -> Action {
    Action { id, scopes, keys, does }
}

use Scope::{Aside, Box as InBox, ChatGpt, Details, DetailsSearch, Help, Job, KeyBox, List, Menu, Search, Selection, Tabs, Tree, Voice};

/// Every action, in the order their keys are tried where they share one: an action listed first has the key first, and
/// lets it go on when what it does doesn't apply.
pub const ACTIONS: &[Action] = &[
    // Wherever the keyboard is, a panel too, unless the panel is in the middle of something. Over selected text it lets
    // the selection go first.
    act(
        "voice.talk",
        &[InBox, Menu, Tabs, Tree, Voice, Search, Help, Aside, Job, List, KeyBox, ChatGpt, Details, DetailsSearch],
        &["ctrl+shift?+t"],
        "Talk instead of typing: press it again to stop, or hold it while you talk",
    ),
    // Over selected text, or just after a copy: otherwise the key goes on.
    act("selection.copy", &[Selection], &["ctrl+shift?+c"], "Copy the selected text; just after a copy, copy it again"),
    act("selection.clear", &[Selection], &["any+escape"], "Let go of the selected text; just after a copy, take its word away"),
    // While Kumi listens. Esc and enter go on while the / menu is open, and enter for a command.
    act("voice.drop", &[Voice], &["any+escape", "ctrl+alt?+shift?+c"], "Stop listening, and drop what was heard"),
    act("voice.send", &[Voice], &["ctrl?+enter"], "Stop listening, and send what was heard at once"),
    // While looking back through what was sent. Any other key stops looking, with the message found, and goes on.
    act("search.back", &[Search], &["ctrl+shift?+r"], "Look further back"),
    act("search.erase", &[Search], &["shift?+backspace"], "Take back the last character looked for"),
    act("search.take", &[Search], &["any+enter", "any+tab"], "Put the message found in the box"),
    act("search.cancel", &[Search], &["any+escape", "ctrl+alt?+shift?+c"], "Stop looking, and keep the draft"),
    // From the box and the tree; the tabs it leaves.
    act(
        "conversation.details",
        &[InBox, Tree],
        &["ctrl+shift?+o"],
        "The conversation in detail: search it, go from request to request, print it or open it in your editor",
    ),
    // In the box. From the tabs or the tree they come back to the box first.
    act("composer.history-search", &[InBox], &["ctrl+shift?+r"], "Look back through what you sent for the words you type"),
    act("composer.editor", &[InBox], &["ctrl+shift?+g"], "Open the draft in your editor"),
    act("composer.stash", &[InBox], &["ctrl+shift?+s"], "Put the draft aside; in an empty box, bring it back"),
    // The pane, from the box, the tabs and the tree alike.
    act(
        "pane.toggle",
        &[InBox, Tabs, Tree],
        &["ctrl+shift?+\\"],
        "Below 100 columns, open or close the drawer; at full width, put the side pane away or bring it back",
    ),
    act("pane.tabs", &[InBox, Tree], &["shift+tab"], "Into the pane's tabs"),
    // In the tabs. A row's own keys (space, →, ←, u, x, delete) are the row's.
    act("tabs.next", &[Tabs], &["shift+tab"], "On to the next tab"),
    act("tabs.leave", &[Tabs], &["any+escape", "any+tab"], "Back to the box"),
    act("tabs.up", &[Tabs], &["shift?+up"], "The row above"),
    act("tabs.down", &[Tabs], &["shift?+down"], "The row below"),
    act("tabs.page-up", &[Tabs], &["shift?+pageup"], "A page up"),
    act("tabs.page-down", &[Tabs], &["shift?+pagedown"], "A page down"),
    act("tabs.open", &[Tabs], &["shift?+enter"], "Open the row: a change's picture or a job; on what Kumi kept, forget it"),
    // In FOCUS's tree.
    act("tree.up", &[Tree], &["shift?+up"], "The row above"),
    act("tree.down", &[Tree], &["shift?+down"], "The row below"),
    act("tree.pin", &[Tree], &["shift?+enter"], "Pin the row, so what you ask is about it"),
    act("tree.leave", &[Tree], &["any+escape", "any+tab"], "Back to the box"),
    // The / menu.
    act("menu.close", &[Menu], &["any+escape"], "Close the menu"),
    act("menu.up", &[Menu], &["any+up"], "The command above"),
    act("menu.down", &[Menu], &["any+down"], "The command below"),
    act("menu.complete", &[Menu], &["any+tab"], "Complete the command"),
    act("menu.run", &[Menu], &["ctrl?+enter"], "Run the command"),
    // The box. Send-after has tab only while Kumi works with words in the box; the tree, only when FOCUS shows it.
    act("composer.send-after", &[InBox], &["shift?+tab"], "While Kumi works: send the words after the answer instead"),
    act("pane.tree", &[InBox], &["shift?+tab"], "Into FOCUS's tree, at the DAW's selection"),
    act("app.interrupt", &[InBox], &["ctrl+alt?+shift?+c"], "Stop Kumi; when it isn't working, clear the box, then quit"),
    act("app.background", &[InBox], &["ctrl+alt?+shift?+b"], "Move a slow step that doesn't use the DAW to the background"),
    act("app.paste-picture", &[InBox], &["ctrl+alt?+shift?+v"], "Add the picture on the clipboard to your next message"),
    // From an empty box while Kumi isn't working; otherwise ctrl+d deletes.
    act("app.quit", &[InBox], &["ctrl+alt?+shift?+d"], "Quit, from an empty box while Kumi isn't working"),
    act("app.stop", &[InBox], &["any+escape"], "Stop a /slots check, then Kumi's answer; when it isn't working, unpin"),
    // Two escs that did nothing else, in an empty box.
    act("app.take-back", &[InBox], &["any+escape any+escape"], "List your requests, to take one back"),
    act("composer.send", &[InBox], &["ctrl?+enter"], "Send; while Kumi works, it reads it after the step under way"),
    act("composer.new-line", &[InBox], &["ctrl+alt?+shift?+j", "alt+ctrl?+shift?+enter", "shift+ctrl?+enter"], "A new line"),
    // In an empty box with files added, any backspace takes the last one back first.
    act("composer.take-back-file", &[InBox], &["any+backspace"], "In an empty box, take back the last file added"),
    act(
        "composer.cut-word",
        &[InBox],
        &["alt+ctrl?+shift?+backspace", "ctrl+shift?+backspace", "ctrl+alt?+shift?+w"],
        "Cut the word before the cursor",
    ),
    act("composer.backspace", &[InBox], &["shift?+backspace"], "Delete the character before the cursor"),
    act("composer.delete", &[InBox], &["any+delete", "ctrl+alt?+shift?+d"], "Delete the character after the cursor"),
    act("composer.word-left", &[InBox], &["alt+ctrl?+shift?+left", "ctrl+shift?+left", "alt+ctrl?+shift?+b"], "A word left"),
    act("composer.left", &[InBox], &["shift?+left"], "Left along the line"),
    act("composer.word-right", &[InBox], &["alt+ctrl?+shift?+right", "ctrl+shift?+right", "alt+ctrl?+shift?+f"], "A word right"),
    act("composer.right", &[InBox], &["shift?+right"], "Right along the line"),
    // With nothing waiting, alt+↑ is ↑.
    act("composer.take-back-waiting", &[InBox], &["alt+ctrl?+shift?+up"], "Take the last waiting message back into the box"),
    act("composer.up", &[InBox], &["any+up"], "The line above, then what you sent before"),
    act("composer.down", &[InBox], &["any+down"], "The line below, then what you sent after it"),
    act("conversation.start", &[InBox], &["ctrl+alt?+shift?+home"], "The start of the conversation"),
    act("conversation.latest", &[InBox], &["ctrl+alt?+shift?+end"], "Back to the latest"),
    act("composer.line-start", &[InBox], &["any+home", "ctrl+alt?+shift?+a"], "The start of the line"),
    act("composer.line-end", &[InBox], &["any+end", "ctrl+alt?+shift?+e"], "The end of the line"),
    act("composer.undo", &[InBox], &["ctrl+alt?+shift?+-", "ctrl+alt?+shift?+_"], "Undo in the box"),
    act("composer.yank", &[InBox], &["ctrl+alt?+shift?+y"], "Put back what you cut last"),
    act("composer.yank-earlier", &[InBox], &["alt+ctrl?+shift?+y"], "Swap what was put back for the cut before it"),
    act("composer.cut-to-end", &[InBox], &["ctrl+alt?+shift?+k"], "Cut to the end of the line"),
    act("composer.cut-to-start", &[InBox], &["ctrl+alt?+shift?+u"], "Cut to the start of the line"),
    act("conversation.page-up", &[InBox], &["any+pageup"], "Scroll the conversation up a page"),
    act("conversation.page-down", &[InBox], &["any+pagedown"], "Scroll the conversation down a page"),
    act("app.redraw", &[InBox], &["ctrl+alt?+shift?+l"], "Draw the screen again"),
    // `?` on an empty box opens the help as a character typed, which no key takes: keys.json can add keys (f1).
    act("app.help", &[InBox], &[], "Open the help"),
    // The help.
    act("help.close", &[Help], &["any+escape", "any+enter", "ctrl+alt?+shift?+c", "?"], "Close the help"),
    act("help.up", &[Help], &["any+up"], "Scroll up a row"),
    act("help.down", &[Help], &["any+down"], "Scroll down a row"),
    act("help.page-up", &[Help], &["any+pageup"], "Scroll up a page"),
    act("help.page-down", &[Help], &["any+pagedown"], "Scroll down a page"),
    act("help.top", &[Help], &["any+home"], "The top of the help"),
    act("help.end", &[Help], &["any+end"], "The end of the help"),
    // A side answer (/btw).
    act("aside.close", &[Aside], &["any+escape", "any+enter", "ctrl+alt?+shift?+c", "space"], "Close the side answer"),
    act("aside.up", &[Aside], &["any+up"], "Scroll up a row"),
    act("aside.page-up", &[Aside], &["any+pageup"], "Scroll up five rows"),
    act("aside.down", &[Aside], &["any+down"], "Scroll down a row"),
    act("aside.page-down", &[Aside], &["any+pagedown"], "Scroll down five rows"),
    act("aside.earlier", &[Aside], &["any+left"], "The side answer before"),
    act("aside.later", &[Aside], &["any+right"], "The side answer after"),
    act("aside.copy", &[Aside], &["shift?+c"], "Copy the side answer"),
    // A job's details.
    act("job.close", &[Job], &["any+escape", "any+enter", "ctrl+alt?+shift?+c", "space"], "Close the job's details"),
    act("job.up", &[Job], &["any+up"], "Scroll up a row"),
    act("job.page-up", &[Job], &["any+pageup"], "Scroll up five rows"),
    act("job.down", &[Job], &["any+down"], "Scroll down a row"),
    act("job.page-down", &[Job], &["any+pagedown"], "Scroll down five rows"),
    act("job.stop", &[Job], &["shift?+x"], "Stop the job"),
    act("job.again", &[Job], &["shift?+r"], "Run the job again, once it's done"),
    act("job.copy", &[Job], &["shift?+c"], "Copy the job's report"),
    // A list to choose from. Tab goes to the next of its tabs where it has them; otherwise it's ↓.
    act("list.close", &[List], &["any+escape", "ctrl+alt?+shift?+c"], "Close the list"),
    act("list.up", &[List], &["any+up"], "The item above"),
    act("list.next-tab", &[List], &["any+tab"], "The list's next tab"),
    act("list.down", &[List], &["any+down", "any+tab"], "The item below"),
    act("list.erase", &[List], &["any+backspace"], "Take back the last character typed to find an item"),
    act("list.choose", &[List], &["any+enter"], "Choose the item"),
    act("list.choose-other", &[List], &["ctrl+alt?+shift?+f"], "Choose it the other way, where the list has one (/model: the fallback)"),
    // A key's box.
    act("key.cancel", &[KeyBox], &["any+escape", "ctrl+alt?+shift?+c"], "Close the key's box"),
    act("key.erase", &[KeyBox], &["any+backspace"], "Take back the last character of the key"),
    act("key.clear", &[KeyBox], &["ctrl+alt?+shift?+u"], "Clear the key"),
    act("key.check", &[KeyBox], &["any+enter"], "Check the key with the provider, and save it"),
    // ChatGPT's sign-in.
    act("chatgpt.cancel", &[ChatGpt], &["any+escape", "ctrl+alt?+shift?+c"], "Stop signing in"),
    act("chatgpt.copy", &[ChatGpt], &["shift?+c"], "Copy the link, or the code"),
    act("chatgpt.by-code", &[ChatGpt], &["shift?+d"], "Sign in with a code instead of the browser"),
    act("chatgpt.by-browser", &[ChatGpt], &["shift?+b"], "Sign in with the browser again"),
    // The conversation in detail. While a search is typed, backspace, enter and esc are the search's, and esc doesn't
    // close the view.
    act("details.search-erase", &[DetailsSearch], &["any+backspace"], "Take back the last character searched for"),
    act("details.search-keep", &[DetailsSearch], &["any+enter"], "Keep the search, and go back to the view's keys"),
    act("details.search-drop", &[DetailsSearch], &["any+escape"], "Drop the search"),
    act(
        "details.close",
        &[Details, DetailsSearch],
        &["any+escape", "ctrl+alt?+shift?+c", "ctrl+alt?+shift?+o", "q"],
        "Close the detailed view",
    ),
    act("details.up", &[Details, DetailsSearch], &["any+up"], "Scroll up a row"),
    act("details.down", &[Details, DetailsSearch], &["any+down"], "Scroll down a row"),
    act("details.page-up", &[Details, DetailsSearch], &["any+pageup"], "Scroll up a page"),
    act("details.page-down", &[Details, DetailsSearch], &["any+pagedown"], "Scroll down a page"),
    act("details.top", &[Details, DetailsSearch], &["any+home"], "The start of the conversation"),
    act("details.end", &[Details, DetailsSearch], &["any+end"], "The end of the conversation"),
    act("details.search", &[Details], &["/"], "Search the conversation: type the words, enter keeps them"),
    act("details.next-match", &[Details], &["n"], "The next place the words are"),
    act("details.previous-match", &[Details], &["shift+n"], "The place before"),
    act("details.next-request", &[Details], &["}"], "The next request"),
    act("details.previous-request", &[Details], &["{"], "The request before"),
    act("details.print", &[Details], &["["], "Print it into the terminal's scrollback"),
    act("details.edit", &[Details], &["v"], "Open it in your editor"),
];

/// The action called `id`.
pub fn action(id: &str) -> Option<&'static Action> {
    ACTIONS.iter().find(|action| action.id == id)
}

/// A key (or keys, one after another) that does an action in a scope.
struct Binding {
    scope: Scope,
    id: &'static str,
    keys: Vec<Chord>,
    /// The modifiers its key may be held with besides: a default's, never keys.json's.
    also: Modifiers,
    /// keys.json's.
    asked: bool,
}

impl Binding {
    /// Whether `chord` is this binding's one key.
    fn takes(&self, chord: &Chord) -> bool {
        match self.keys.as_slice() {
            [key] => held(chord, key, self.also),
            _ => false,
        }
    }
}

/// Whether `chord` is `key`, held with only the modifiers `key` needs and those it may have.
fn held(chord: &Chord, key: &Chord, also: Modifiers) -> bool {
    let each = |pressed: bool, needed: bool, may: bool| pressed == needed || (pressed && may);
    chord.key == key.key
        && each(chord.mods.ctrl, key.mods.ctrl, also.ctrl)
        && each(chord.mods.alt, key.mods.alt, also.alt)
        && each(chord.mods.shift, key.mods.shift, also.shift)
}

/// The keys in force: the defaults, with keys.json's over them.
pub struct Keymap {
    /// In the order they're tried: keys.json's first, then the defaults in [`ACTIONS`] order.
    bindings: Vec<Binding>,
    /// The actions keys.json gives keys of its own (or none).
    rebound: Vec<&'static str>,
}

impl Keymap {
    /// The keys, with keys.json's (`file`) over the defaults, and what Kumi couldn't use of it or what it changes, said
    /// once each: an action it doesn't know, a key it can't read or that would stop a character being typed, and a key
    /// two actions now share.
    pub fn new(file: Option<&Value>) -> (Keymap, Vec<String>) {
        let mut said: Vec<String> = vec![];
        let mut say = |words: String| {
            if !said.contains(&words) {
                said.push(words);
            }
        };
        let mut asked: Vec<Binding> = vec![];
        let mut rebound: Vec<&'static str> = vec![];
        match file {
            None => {}
            Some(Value::Object(entries)) => {
                for (id, keys) in entries {
                    let Some(action) = action(id) else {
                        say(match nearest(id) {
                            Some(near) => format!("keys.json names “{id}”, which isn't an action: did you mean {near}?"),
                            None => format!("keys.json names “{id}”, which isn't an action"),
                        });
                        continue;
                    };
                    let written: Vec<&str> = match keys {
                        Value::String(key) => vec![key.as_str()],
                        Value::Array(keys) if keys.iter().all(Value::is_string) => keys.iter().filter_map(Value::as_str).collect(),
                        _ => {
                            say(format!(
                                "keys.json gives {id} something other than a list of keys (like [\"ctrl+z\"]), so it keeps its \
                                 own"
                            ));
                            continue;
                        }
                    };
                    let mut taken: Vec<Vec<Chord>> = vec![];
                    for words in &written {
                        let keys: Option<Vec<Chord>> = words.split_whitespace().map(Chord::parse).collect();
                        let keys = match keys {
                            Some(keys) if (1..=2).contains(&keys.len()) => keys,
                            _ => {
                                say(format!("keys.json gives {id} “{words}”, which Kumi can't read as a key"));
                                continue;
                            }
                        };
                        // Keys one after another are read only in the box.
                        if keys.len() == 2 && !action.scopes.contains(&Scope::Box) {
                            say(format!("keys.json gives {id} “{words}”, but keys one after another work only in the box"));
                            continue;
                        }
                        if let Some(scope) = action.scopes.iter().find(|scope| scope.types() && keys.iter().any(Chord::typed)) {
                            say(format!("keys.json gives {id} “{words}”, but that types itself {}", scope.words()));
                            continue;
                        }
                        if !taken.contains(&keys) {
                            taken.push(keys);
                        }
                    }
                    // An empty list takes the action's keys away; a list none of which Kumi can use leaves it its own.
                    if !written.is_empty() && taken.is_empty() {
                        say(format!("keys.json gives {id} no key Kumi can use, so it keeps its own"));
                        continue;
                    }
                    rebound.push(action.id);
                    for keys in taken {
                        for &scope in action.scopes {
                            asked.push(Binding { scope, id: action.id, keys: keys.clone(), also: Modifiers::NONE, asked: true });
                        }
                    }
                }
            }
            Some(_) => say("keys.json should be an object of action ids and their keys (like {\"composer.undo\": [\"ctrl+z\"]}), so Kumi \
                 keeps its own keys"
                .into()),
        }
        let mut bindings = asked;
        for action in ACTIONS.iter().filter(|action| !rebound.contains(&action.id)) {
            for (keys, also) in action.keys.iter().filter_map(|key| read_default(key)) {
                for &scope in action.scopes {
                    bindings.push(Binding { scope, id: action.id, keys: keys.clone(), also, asked: false });
                }
            }
        }
        // What a key of keys.json's comes in front of, where it's bound.
        for (at, binding) in bindings.iter().enumerate().filter(|(_, binding)| binding.asked) {
            let first =
                bindings[..at].iter().find(|other| other.scope == binding.scope && other.id != binding.id && other.keys == binding.keys);
            if let Some(first) = first {
                say(format!(
                    "keys.json gives {} to both {} and {} {}: {} has it first",
                    label(&binding.keys),
                    first.id,
                    binding.id,
                    binding.scope.words(),
                    first.id
                ));
                continue;
            }
            let mut behind: Vec<&str> = vec![];
            for other in bindings[at + 1..].iter().filter(|other| !other.asked && other.scope == binding.scope && other.id != binding.id) {
                let shares = match binding.keys.as_slice() {
                    [key] => other.takes(key),
                    keys => other.keys == keys,
                };
                if shares && !behind.contains(&other.id) {
                    behind.push(other.id);
                }
            }
            if !behind.is_empty() {
                say(format!(
                    "keys.json gives {} to {} {}, ahead of {}",
                    label(&binding.keys),
                    binding.id,
                    binding.scope.words(),
                    and(&behind)
                ));
            }
            // A pair whose first key is another action's starts only when that action doesn't apply.
            if let [first, _] = binding.keys.as_slice() {
                let mut takers: Vec<&str> = vec![];
                for other in bindings.iter().filter(|other| other.scope == binding.scope && other.id != binding.id && other.takes(first)) {
                    if !takers.contains(&other.id) {
                        takers.push(other.id);
                    }
                }
                if !takers.is_empty() {
                    say(format!(
                        "keys.json gives {} to {} {}: {} starts it only when {} {} take it",
                        label(&binding.keys),
                        binding.id,
                        binding.scope.words(),
                        first.label(),
                        and(&takers),
                        if takers.len() == 1 { "doesn't" } else { "don't" }
                    ));
                }
            }
        }
        (Keymap { bindings, rebound }, said)
    }

    /// The actions `chord` is in `scope`, in the order they're tried.
    pub fn actions(&self, scope: Scope, chord: &Chord) -> Vec<&'static str> {
        let mut ids: Vec<&'static str> = vec![];
        for binding in self.bindings.iter().filter(|binding| binding.scope == scope && binding.takes(chord)) {
            if !ids.contains(&binding.id) {
                ids.push(binding.id);
            }
        }
        ids
    }

    /// Whether `chord` starts keys pressed one after another (esc esc) in `scope`: it waits for the next when no
    /// action took it.
    pub fn starts(&self, scope: Scope, chord: &Chord) -> bool {
        self.bindings.iter().any(|binding| binding.scope == scope && binding.keys.len() == 2 && held(chord, &binding.keys[0], binding.also))
    }

    /// The action `first` then `second` are in `scope`.
    pub fn then(&self, scope: Scope, first: &Chord, second: &Chord) -> Option<&'static str> {
        let binding = self.bindings.iter().find(|binding| {
            binding.scope == scope
                && matches!(binding.keys.as_slice(), [one, two] if held(first, one, binding.also) && held(second, two, binding.also))
        });
        binding.map(|binding| binding.id)
    }

    /// Whether keys.json gives action `id` keys of its own, or none.
    pub fn rebound(&self, id: &str) -> bool {
        self.rebound.contains(&id)
    }

    /// The keys an action is bound to, each as it's pressed (one key, or two one after another), in the order they were
    /// given: for the help and the box's hints.
    pub fn keys(&self, id: &str) -> Vec<&[Chord]> {
        let mut keys: Vec<&[Chord]> = vec![];
        for binding in self.bindings.iter().filter(|binding| binding.id == id) {
            if !keys.contains(&binding.keys.as_slice()) {
                keys.push(&binding.keys);
            }
        }
        keys
    }
}

impl Default for Keymap {
    /// Kumi's own keys.
    fn default() -> Self {
        Keymap::new(None).0
    }
}

/// "a", "a and b", "a, b and c".
fn and(words: &[&str]) -> String {
    match words {
        [] => String::new(),
        [one] => one.to_string(),
        [rest @ .., last] => format!("{} and {last}", rest.join(", ")),
    }
}

/// The action whose id is nearest `id`, when one is near enough to be what was meant.
fn nearest(id: &str) -> Option<&'static str> {
    let (distance, near) = ACTIONS.iter().map(|action| (edits(id, action.id), action.id)).min()?;
    (distance <= 3).then_some(near)
}

/// How many characters to add, take away or change to make `a` into `b`.
fn edits(a: &str, b: &str) -> usize {
    let b: Vec<char> = b.chars().collect();
    let mut row: Vec<usize> = (0..=b.len()).collect();
    for (i, x) in a.chars().enumerate() {
        let mut last = row[0];
        row[0] = i + 1;
        for (j, y) in b.iter().enumerate() {
            let next = (last + usize::from(x != *y)).min(row[j] + 1).min(row[j + 1] + 1);
            last = row[j + 1];
            row[j + 1] = next;
        }
    }
    row[b.len()]
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn chord(words: &str) -> Chord {
        Chord::parse(words).unwrap_or_else(|| panic!("{words} reads as a key"))
    }

    fn with(file: Value) -> (Keymap, Vec<String>) {
        Keymap::new(Some(&file))
    }

    #[test]
    fn a_key_reads_as_it_is_written() {
        let held = |ctrl, alt, shift| Modifiers { ctrl, alt, shift };
        for (words, key, mods) in [
            ("ctrl+z", "z", held(true, false, false)),
            ("Ctrl+Shift+Tab", "tab", held(true, false, true)),
            ("shift+ctrl+tab", "tab", held(true, false, true)),
            ("alt+up", "up", held(false, true, false)),
            ("alt+↑", "up", held(false, true, false)),
            ("option+left", "left", held(false, true, false)),
            ("ctrl+-", "-", held(true, false, false)),
            ("ctrl++", "+", held(true, false, false)),
            ("+", "+", Modifiers::NONE),
            ("ctrl+\\", "\\", held(true, false, false)),
            ("f5", "f5", Modifiers::NONE),
            ("esc", "escape", Modifiers::NONE),
            ("pgdn", "pagedown", Modifiers::NONE),
            ("space", "space", Modifiers::NONE),
            ("?", "?", Modifiers::NONE),
            // An uppercase letter alone is what shift types; with ctrl, the case says nothing.
            ("Z", "z", held(false, false, true)),
            ("ctrl+Z", "z", held(true, false, false)),
        ] {
            assert_eq!(Chord::parse(words), Some(Chord::new(key, mods)), "{words}");
        }
        for words in ["", "ctrl+", "ctrl+shift", "cmd+z", "hyper+a", "f13", "ctrl+zz", "shift?+left", "any+escape", "ctrl+?+x"] {
            assert_eq!(Chord::parse(words), None, "{words}");
        }
    }

    #[test]
    fn a_key_pressed_or_a_character_typed_is_a_chord() {
        assert_eq!(Chord::of(&InputEvent::key("tab", Modifiers::SHIFT)), Some(chord("shift+tab")));
        assert_eq!(Chord::of(&InputEvent::text("c")), Some(chord("c")));
        assert_eq!(Chord::of(&InputEvent::text("C")), Some(chord("shift+c")));
        assert_eq!(Chord::of(&InputEvent::text(" ")), Some(chord("space")));
        assert_eq!(Chord::of(&InputEvent::text("?")), Some(chord("?")));
        assert_eq!(Chord::of(&InputEvent::text("ab")), None);
        assert_eq!(Chord::of(&InputEvent::Paste { text: "c".into() }), None);
    }

    #[test]
    fn a_label_writes_the_key_as_the_help_does_and_reads_back() {
        for (words, label) in [
            ("ctrl+z", "ctrl+z"),
            ("alt+up", "alt+↑"),
            ("pageup", "page up"),
            ("escape", "esc"),
            ("shift+ctrl+tab", "ctrl+shift+tab"),
            ("ctrl+\\", "ctrl+\\"),
        ] {
            assert_eq!(chord(words).label(), label);
        }
        // Every default key's label is a key again, the same one (a page key's label has a space, so it's written whole).
        for action in ACTIONS {
            for key in action.keys.iter().filter_map(|key| read_default(key)).flat_map(|(keys, _)| keys) {
                let written = key.label().replace("page up", "pageup").replace("page down", "pagedown");
                assert_eq!(Chord::parse(&written), Some(key.clone()), "{}: {written}", action.id);
            }
        }
    }

    #[test]
    fn every_action_has_an_id_of_its_own_and_keys_that_read() {
        let mut ids: Vec<&str> = ACTIONS.iter().map(|action| action.id).collect();
        ids.sort();
        ids.dedup();
        assert_eq!(ids.len(), ACTIONS.len(), "an id is given twice");
        for action in ACTIONS {
            assert!(!action.scopes.is_empty() && !action.does.is_empty(), "{}", action.id);
            let (area, name) = action.id.split_once('.').unwrap_or_else(|| panic!("{} is area.name", action.id));
            assert!(!area.is_empty() && name.chars().all(|c| c.is_ascii_lowercase() || c == '-'), "{}", action.id);
            for key in action.keys {
                assert!(read_default(key).is_some(), "{}: {key} doesn't read", action.id);
            }
        }
        let (keymap, said) = Keymap::new(None);
        assert!(said.is_empty(), "{said:?}");
        assert_eq!(keymap.actions(Scope::Box, &chord("enter")), ["composer.send"]);
    }

    /// Where one key is several actions, they're tried in the order the keys always were.
    #[test]
    fn a_key_shared_by_actions_is_tried_in_their_old_order() {
        let keymap = Keymap::default();
        for (scope, words, ids) in [
            (Scope::Box, "tab", &["composer.send-after", "pane.tree"][..]),
            (Scope::Box, "shift+tab", &["pane.tabs", "composer.send-after", "pane.tree"]),
            (Scope::Tabs, "shift+tab", &["tabs.next", "tabs.leave"]),
            (Scope::Tree, "shift+tab", &["pane.tabs", "tree.leave"]),
            (Scope::Box, "ctrl+d", &["app.quit", "composer.delete"]),
            (Scope::Box, "ctrl+home", &["conversation.start", "composer.line-start"]),
            (Scope::Box, "alt+up", &["composer.take-back-waiting", "composer.up"]),
            (Scope::Box, "ctrl+alt+b", &["app.background", "composer.word-left"]),
            (Scope::Box, "backspace", &["composer.take-back-file", "composer.backspace"]),
            (Scope::Box, "alt+backspace", &["composer.take-back-file", "composer.cut-word"]),
            (Scope::Box, "ctrl+alt+y", &["composer.yank", "composer.yank-earlier"]),
            (Scope::List, "tab", &["list.next-tab", "list.down"]),
        ] {
            assert_eq!(keymap.actions(scope, &chord(words)), ids, "{words} {}", scope.words());
        }
    }

    #[test]
    fn a_default_takes_the_modifiers_its_key_always_ignored_and_no_others() {
        let keymap = Keymap::default();
        for (scope, words, ids) in [
            (Scope::Box, "ctrl+enter", &["composer.send"][..]),
            (Scope::Box, "alt+enter", &["composer.new-line"]),
            (Scope::Box, "shift+enter", &["composer.new-line"]),
            (Scope::Box, "ctrl+shift+alt+enter", &["composer.new-line"]),
            (Scope::Box, "shift+left", &["composer.left"]),
            (Scope::Box, "ctrl+left", &["composer.word-left"]),
            (Scope::Box, "ctrl+shift+c", &["app.interrupt"]),
            (Scope::Box, "ctrl+alt+\\", &[]),
            (Scope::Box, "ctrl+tab", &[]),
            (Scope::Box, "ctrl+t", &["voice.talk"]),
            (Scope::Box, "ctrl+alt+t", &[]),
            (Scope::Tabs, "ctrl+escape", &["tabs.leave"]),
            (Scope::Tabs, "ctrl+up", &[]),
            (Scope::Tabs, "ctrl+\\", &["pane.toggle"]),
            (Scope::Tree, "ctrl+\\", &["pane.toggle"]),
            (Scope::Menu, "ctrl+up", &["menu.up"]),
            (Scope::Menu, "alt+enter", &[]),
            (Scope::Help, "?", &["help.close"]),
            (Scope::Aside, "shift+c", &["aside.copy"]),
            (Scope::Aside, "ctrl+space", &[]),
            (Scope::Aside, "space", &["aside.close"]),
            (Scope::Selection, "ctrl+alt+c", &[]),
            (Scope::List, "ctrl+f", &["list.choose-other"]),
        ] {
            assert_eq!(keymap.actions(scope, &chord(words)), ids, "{words} {}", scope.words());
        }
    }

    #[test]
    fn keys_json_rebinds_an_action_or_unbinds_it() {
        let (keymap, said) = with(json!({ "composer.undo": ["ctrl+z"], "app.background": ["ctrl+b", "f5"], "pane.toggle": [] }));
        assert!(said.is_empty(), "{said:?}");
        assert_eq!(keymap.actions(Scope::Box, &chord("ctrl+z")), ["composer.undo"]);
        assert!(keymap.actions(Scope::Box, &chord("ctrl+-")).is_empty(), "undo's own keys go");
        assert_eq!(keymap.actions(Scope::Box, &chord("f5")), ["app.background"]);
        // keys.json's keys are exact.
        assert!(keymap.actions(Scope::Box, &chord("ctrl+shift+z")).is_empty());
        assert_eq!(keymap.keys("composer.undo"), [&[chord("ctrl+z")][..]]);
        assert_eq!(keymap.keys("app.background"), [&[chord("ctrl+b")][..], &[chord("f5")][..]]);
        // Unbound in each scope it was.
        for scope in [Scope::Box, Scope::Tabs, Scope::Tree] {
            assert!(keymap.actions(scope, &chord("ctrl+\\")).is_empty());
        }
        assert!(keymap.keys("pane.toggle").is_empty());
        // The others keep theirs, and one key given as a word on its own is a list of one.
        let (keymap, said) = with(json!({ "app.help": "f1" }));
        assert!(said.is_empty(), "{said:?}");
        assert_eq!(keymap.actions(Scope::Box, &chord("f1")), ["app.help"]);
        assert_eq!(keymap.keys("composer.undo"), [&[chord("ctrl+-")][..], &[chord("ctrl+_")][..]]);
    }

    #[test]
    fn what_keys_json_cant_use_is_said_once() {
        let (keymap, said) = with(json!({
            "composer.undoo": ["ctrl+z"],
            "nothing.like.it": ["ctrl+q"],
            "app.redraw": ["ctrl+cmd+l", "ctrl+cmd+l", "f9"],
            "composer.send": ["z", "ctrl+enter"],
            "help.close": ["q"],
            "app.quit": 5,
        }));
        assert_eq!(
            said,
            [
                "keys.json names “composer.undoo”, which isn't an action: did you mean composer.undo?",
                "keys.json names “nothing.like.it”, which isn't an action",
                "keys.json gives app.redraw “ctrl+cmd+l”, which Kumi can't read as a key",
                "keys.json gives composer.send “z”, but that types itself in the box",
                "keys.json gives app.quit something other than a list of keys (like [\"ctrl+z\"]), so it keeps its own",
            ]
        );
        // What it could read stands: f9 redraws, ctrl+enter sends, q closes the help (where letters are keys), and quit
        // keeps ctrl+d.
        assert_eq!(keymap.actions(Scope::Box, &chord("f9")), ["app.redraw"]);
        assert_eq!(keymap.actions(Scope::Box, &chord("ctrl+enter")), ["composer.send"]);
        assert!(keymap.actions(Scope::Box, &chord("enter")).is_empty(), "send's own keys go");
        assert_eq!(keymap.actions(Scope::Help, &chord("q")), ["help.close"]);
        assert_eq!(keymap.actions(Scope::Box, &chord("ctrl+d")), ["app.quit", "composer.delete"]);
        let (_, said) = Keymap::new(Some(&json!(["ctrl+z"])));
        assert_eq!(said.len(), 1);
        assert!(said[0].starts_with("keys.json should be an object"), "{said:?}");
    }

    #[test]
    fn an_entry_with_no_key_kumi_can_use_leaves_the_action_its_own() {
        let (keymap, said) = with(json!({ "composer.send": ["z"], "details.close": ["x"] }));
        assert_eq!(
            said,
            [
                "keys.json gives composer.send “z”, but that types itself in the box",
                "keys.json gives composer.send no key Kumi can use, so it keeps its own",
            ]
        );
        assert_eq!(keymap.actions(Scope::Box, &chord("enter")), ["composer.send"]);
        // In the detailed view a letter is a key (its search takes what's typed first): x closes it, and esc no longer does.
        assert_eq!(keymap.actions(Scope::Details, &chord("x")), ["details.close"]);
        assert!(keymap.actions(Scope::Details, &chord("esc")).is_empty());
        // An empty list still takes an action's keys away.
        let (keymap, said) = with(json!({ "composer.send": [] }));
        assert!(said.is_empty() && keymap.actions(Scope::Box, &chord("enter")).is_empty(), "{said:?}");
    }

    #[test]
    fn a_key_of_keys_json_goes_first_and_says_what_it_comes_ahead_of() {
        let (keymap, said) = with(json!({ "app.background": ["ctrl+d"] }));
        assert_eq!(keymap.actions(Scope::Box, &chord("ctrl+d")), ["app.background", "app.quit", "composer.delete"]);
        assert_eq!(said, ["keys.json gives ctrl+d to app.background in the box, ahead of app.quit and composer.delete"]);
        // A default that took the key with a modifier it ignores is behind it too. Keys are written as the help writes them.
        let (_, said) = with(json!({ "app.redraw": ["shift+left"] }));
        assert_eq!(said, ["keys.json gives shift+← to app.redraw in the box, ahead of composer.left"]);
        // Each scope a key works in is looked at: ctrl+\ given to tabs.leave comes ahead of pane.toggle in the tabs only.
        let (keymap, said) = with(json!({ "tabs.leave": ["ctrl+\\"] }));
        assert_eq!(said, ["keys.json gives ctrl+\\ to tabs.leave in the tabs, ahead of pane.toggle"]);
        assert_eq!(keymap.actions(Scope::Box, &chord("ctrl+\\")), ["pane.toggle"]);
        // A key moved off its old action leaves nothing behind it.
        let (_, said) = with(json!({ "app.background": ["ctrl+d"], "app.quit": ["ctrl+q"], "composer.delete": ["delete"] }));
        assert!(said.is_empty(), "{said:?}");
    }

    #[test]
    fn two_actions_keys_json_gives_one_key_are_said_with_which_has_it_first() {
        let (keymap, said) = with(json!({ "composer.undo": ["ctrl+z"], "app.redraw": ["ctrl+z"] }));
        assert_eq!(said, ["keys.json gives ctrl+z to both composer.undo and app.redraw in the box: composer.undo has it first"]);
        assert_eq!(keymap.actions(Scope::Box, &chord("ctrl+z")), ["composer.undo", "app.redraw"]);
        // In scopes of their own, they don't meet.
        let (_, said) = with(json!({ "help.close": ["q"], "aside.close": ["q"] }));
        assert!(said.is_empty(), "{said:?}");
    }

    #[test]
    fn keys_pressed_one_after_another_wait_for_the_second() {
        // Kumi's own: esc esc takes a request back.
        let esc = chord("esc");
        assert_eq!(Keymap::default().then(Scope::Box, &esc, &chord("shift+esc")), Some("app.take-back"));
        let (keymap, said) = with(json!({ "app.redraw": ["esc esc"] }));
        assert_eq!(
            said,
            [
                "keys.json gives esc esc to app.redraw in the box, ahead of app.take-back",
                "keys.json gives esc esc to app.redraw in the box: esc starts it only when app.stop doesn't take it",
            ]
        );
        // A pair whose first key another action has always (ctrl+g, the editor's) never starts, and that's said.
        let (_, said) = with(json!({ "app.help": ["ctrl+g ctrl+h"] }));
        assert_eq!(
            said,
            ["keys.json gives ctrl+g ctrl+h to app.help in the box: ctrl+g starts it only when composer.editor doesn't take it"]
        );
        assert!(keymap.starts(Scope::Box, &esc) && !keymap.starts(Scope::Help, &esc));
        assert_eq!(keymap.then(Scope::Box, &esc, &esc), Some("app.redraw"));
        assert_eq!(keymap.then(Scope::Box, &esc, &chord("enter")), None);
        // The first esc is still app.stop's when it has something to stop.
        assert_eq!(keymap.actions(Scope::Box, &esc), ["app.stop"]);
        assert_eq!(keymap.keys("app.redraw").iter().map(|keys| label(keys)).collect::<Vec<_>>(), ["esc esc"]);
        // Three keys aren't a key; with no other, the action keeps its own.
        let (keymap, said) = with(json!({ "app.redraw": ["esc esc esc"] }));
        assert_eq!(
            said,
            [
                "keys.json gives app.redraw “esc esc esc”, which Kumi can't read as a key",
                "keys.json gives app.redraw no key Kumi can use, so it keeps its own",
            ]
        );
        assert_eq!(keymap.actions(Scope::Box, &chord("ctrl+l")), ["app.redraw"]);
    }
}
