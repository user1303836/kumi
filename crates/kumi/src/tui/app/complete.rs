//! The `/` menu: the commands a name finds as it's typed, and, once a command and a space are typed, the words it takes
//! for the word being typed. Those are its fixed words, a slot's choices, the producer's palettes and the kept
//! conversations; the last two are read through the session once, as the word starts, and kept until the line's
//! command changes, never a request a frame.

use super::*;
use kumi_agent::core::commands::{self, By, Completes, Completing, Found};
use kumi_common::time::since;

/// How many letters of a word an about's words are found by, as the commands' are.
const WORD_LEAST: usize = 3;

/// A word the menu offers: what tab puts in, what the menu shows for it, and what it says of it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct Candidate {
    pub word: String,
    pub label: String,
    pub about: String,
}

impl Candidate {
    fn word(word: &str) -> Candidate {
        Candidate { word: word.into(), label: word.into(), about: String::new() }
    }
}

/// A candidate the menu shows, and how what's typed found it.
#[derive(Debug, Clone)]
pub(super) struct Offered {
    pub candidate: Candidate,
    pub by: By,
}

/// What the `/` menu shows for the line.
pub(super) enum Menu {
    /// The commands the name being typed finds.
    Commands(Vec<Found>),
    /// The words the command takes for the word being typed.
    Words { completing: Completing, offered: Vec<Offered> },
    /// What `@` names, as the session ranked it for what's typed after it: `count` characters before the cursor are the
    /// `@` and those.
    References { count: usize, rows: Vec<kumi_agent::core::references::Candidate> },
}

/// What the menu puts in the box for an item.
pub(super) enum Insert {
    /// The line, as it now reads.
    Text(String),
    /// A chip naming `reference`, drawn and sent as `label`, in place of the `count` characters before the cursor.
    Chip { count: usize, label: String, reference: Reference },
}

impl Menu {
    pub(super) fn is_empty(&self) -> bool {
        self.len() == 0
    }

    pub(super) fn len(&self) -> usize {
        match self {
            Menu::Commands(found) => found.len(),
            Menu::Words { offered, .. } => offered.len(),
            Menu::References { rows, .. } => rows.len(),
        }
    }

    /// The chip for what `@` names at `index`.
    fn chip(count: usize, rows: &[kumi_agent::core::references::Candidate], index: usize) -> Insert {
        let row = &rows[index];
        Insert::Chip { count, label: format!("@{}", row.label), reference: row.reference.clone() }
    }

    /// What tab puts in the box for the item at `index`: a command's name, or the line with the word chosen, and a space
    /// where the menu offers the word after it.
    pub(super) fn fill(&self, index: usize) -> Insert {
        match self {
            Menu::Commands(found) => Insert::Text(found[index].command.name.into()),
            Menu::Words { completing, offered } => {
                let line = format!("{}{}", completing.before, offered[index].candidate.word);
                // (a name runs to the line's end: nothing follows it)
                let next = format!("{line} ");
                Insert::Text(if !completing.arg.to_end && commands::completing(&next, |_| true).is_some() { next } else { line })
            }
            Menu::References { count, rows } => Menu::chip(*count, rows, index),
        }
    }

    /// What Enter does with the item at `index` (`chosen` with ↑↓): the line it leaves in the box, and whether that line
    /// runs. A command found only by the words of what it does, or a word only partly typed, is none the producer named:
    /// Enter puts it in, as tab does, unless they chose it. A word not begun yet leaves the line to run as it stands.
    pub(super) fn enter(&self, index: usize, line: &str, chosen: bool) -> (Insert, bool) {
        match self {
            Menu::Commands(found) => (Insert::Text(found[index].command.name.into()), found[index].named() || chosen),
            Menu::Words { completing, .. } if completing.typed.is_empty() && !chosen => (Insert::Text(line.into()), true),
            Menu::Words { completing, offered } => {
                let offered = &offered[index];
                (Insert::Text(format!("{}{}", completing.before, offered.candidate.word)), chosen || offered.by == By::Typed)
            }
            // What's named goes in; the message is sent with the next Enter.
            Menu::References { count, rows } => (Menu::chip(*count, rows, index), false),
        }
    }
}

/// The candidates for what's typed of a word: the one it is in full first, then those whose word or label it begins
/// (in any case), then those a word of whose label or about it begins (three letters or more), each in the order given.
pub(super) fn offering(candidates: &[Candidate], typed: &str) -> Vec<Offered> {
    let typed = typed.to_lowercase();
    let says = |text: &str| {
        typed.chars().count() >= WORD_LEAST
            && text.split(|c: char| !c.is_alphanumeric()).any(|said| said.to_lowercase().starts_with(&typed))
    };
    let by = |candidate: &Candidate| {
        let (word, label) = (candidate.word.to_lowercase(), candidate.label.to_lowercase());
        if word == typed {
            Some(By::Typed)
        } else if word.starts_with(&typed) || label.starts_with(&typed) {
            Some(By::Name)
        } else if says(&candidate.label) || says(&candidate.about) {
            Some(By::Words)
        } else {
            None
        }
    };
    let mut found: Vec<(By, usize, &Candidate)> =
        candidates.iter().enumerate().filter_map(|(at, candidate)| by(candidate).map(|by| (by, at, candidate))).collect();
    found.sort_by_key(|(by, at, _)| (*by, *at));
    found.into_iter().map(|(by, _, candidate)| Offered { candidate: candidate.clone(), by }).collect()
}

/// The words a command takes for the word being typed, as the session read them: kept until the line's command
/// changes, so the menu asks once.
#[derive(Default)]
pub(super) struct Offers {
    /// What they're for: the command, and what it completes.
    key: Option<(&'static str, Completes)>,
    /// None while they're read.
    words: Option<Rc<Vec<Candidate>>>,
}

impl TuiApp {
    /// What the `/` menu shows for the line in the box: nothing once it's closed, while a search runs, or for a message.
    pub(super) fn menu(&self) -> Menu {
        if let Some(menu) = self.references_menu() {
            return menu;
        }
        let text = {
            let mut state = self.0.state.borrow_mut();
            let text = state.editor.text().to_string();
            // What was read for a command goes once the line names another, or none (by its name: an alias is it).
            let command = commands::named(text.split(' ').next().unwrap_or_default()).map(|command| command.name);
            if state.offers.key.is_some_and(|(name, _)| Some(name) != command) {
                state.offers = Offers::default();
            }
            if state.menu_dismissed || state.search.is_some() || !text.starts_with('/') {
                return Menu::Commands(vec![]);
            }
            text
        };
        let has = self.has();
        let spaced = text.chars().any(|c| string::trim(&c.to_string()).is_empty());
        let menu = if spaced {
            self.words_menu(&text, &has)
        } else {
            let recent = self.recent_commands();
            let recent: Vec<&str> = recent.iter().map(String::as_str).collect();
            Menu::Commands(commands::matching(&text, &recent, |command| has.offers(command)))
        };
        let mut state = self.0.state.borrow_mut();
        // Another list (a command's words after its name, the next word's after one) starts at its first row, nothing
        // chosen in it: what was chosen in the last is none of its.
        let list = match &menu {
            Menu::Commands(_) => None,
            Menu::Words { completing, .. } => Some(completing.before.clone()),
            // (`@`'s list isn't made here: references_menu makes it, and keys it)
            Menu::References { .. } => None,
        };
        if state.menu_list != list {
            state.menu_list = list;
            state.menu_index = 0;
            state.menu_chosen = None;
        }
        if state.menu_index >= menu.len() {
            state.menu_index = 0;
        }
        // A choice made with ↑↓ holds for the words it was made for.
        if state.menu_chosen.as_deref().is_some_and(|chosen| chosen != text) {
            state.menu_chosen = None;
        }
        menu
    }

    /// What `@` names, where the cursor is in one and the session answered for what's typed after it.
    fn references_menu(&self) -> Option<Menu> {
        let mut state = self.0.state.borrow_mut();
        if state.menu_dismissed || state.search.is_some() {
            return None;
        }
        let (count, query) = state.editor.at_query()?;
        let rows = state.references.as_ref().filter(|named| named.query == query && !named.rows.is_empty())?.rows.clone();
        // Each `@` is a list of its own, by where it is in the box: it starts at its first row, as the menu's others do.
        let list = Some(format!("@{}", state.editor.cursor() - count));
        if state.menu_list != list {
            state.menu_list = list;
            state.menu_index = 0;
            state.menu_chosen = None;
        }
        if state.menu_index >= rows.len() {
            state.menu_index = 0;
        }
        Some(Menu::References { count, rows })
    }

    /// The words the line's command takes for the word being typed, where it offers any; what was read for another
    /// command goes.
    fn words_menu(&self, text: &str, has: &crate::has::Has) -> Menu {
        let Some(completing) = commands::completing(text, |command| has.offers(command)) else {
            self.0.state.borrow_mut().offers = Offers::default();
            return Menu::Commands(vec![]);
        };
        let Some(words) = self.offers_for(&completing) else { return Menu::Commands(vec![]) };
        let offered = offering(&words, &completing.typed);
        Menu::Words { completing, offered }
    }

    /// The candidates for a word: fixed ones at once; the session's once read, which starts the first time they're asked
    /// for and redraws the menu as they come.
    fn offers_for(&self, completing: &Completing) -> Option<Rc<Vec<Candidate>>> {
        let completes = completing.arg.completes;
        match completes {
            Completes::Words(words) => return Some(Rc::new(words.iter().map(|word| Candidate::word(word)).collect())),
            Completes::SlotModels => {
                let choices = kumi_listening::slots::choices_for(crate::daw::profile(), completing.after.as_deref()?)?;
                let choices = choices.into_iter().map(|(word, about)| Candidate { word: word.into(), label: word.into(), about });
                return Some(Rc::new(choices.collect()));
            }
            Completes::Palettes | Completes::Conversations | Completes::Models => {}
        }
        let key = (completing.command.name, completes);
        {
            let mut state = self.0.state.borrow_mut();
            if state.offers.key == Some(key) {
                return state.offers.words.clone();
            }
            state.offers = Offers { key: Some(key), words: None };
        }
        self.task(move |app| async move {
            // (one that can't be read offers nothing, said nowhere: the line still runs as typed)
            let words = app.read_offers(completes).await.unwrap_or_default();
            let mut state = app.0.state.borrow_mut();
            if state.offers.key == Some(key) {
                state.offers.words = Some(Rc::new(words));
            }
            drop(state);
            app.0.scheduler.request();
            Ok(())
        });
        None
    }

    /// The session's candidates for a word: its palettes, or the kept conversations (this Set's first, the one going on
    /// now left out, as /resume says that's this one).
    async fn read_offers(&self, completes: Completes) -> Result<Vec<Candidate>, RuntimeError> {
        match completes {
            Completes::Palettes => {
                let palettes = self.session().op(op::PaletteNames {}).await?;
                Ok(palettes
                    .into_iter()
                    .map(|palette| {
                        let sounds = format!("{} {}", palette.items, if palette.items == 1 { "sound" } else { "sounds" });
                        let about = if palette.current { format!("{sounds} · this session's") } else { sounds };
                        Candidate { word: palette.name.clone(), label: palette.name, about }
                    })
                    .collect())
            }
            Completes::Conversations => {
                let here = self.session().op(op::Conversations {}).await?;
                let all = self.session().op(op::AllConversations {}).await?;
                let rest: Vec<ConversationSummary> = all.into_iter().filter(|row| !here.iter().any(|kept| kept.id == row.id)).collect();
                let rows: Vec<ConversationSummary> = here.into_iter().chain(rest).filter(|row| !row.current).collect();
                let now = now_ms_f64();
                Ok(rows
                    .iter()
                    .map(|row| {
                        // By its name, where it has one only it has; else by its id.
                        let named =
                            row.name.as_deref().filter(|name| rows.iter().filter(|other| other.name.as_deref() == Some(name)).count() == 1);
                        let title = self.clean(&row.title(), 60);
                        let set = row.set.as_deref().map(|set| format!(" · {}", self.clean(set, 30))).unwrap_or_default();
                        Candidate {
                            word: named.map_or_else(|| row.id.clone(), str::to_string),
                            label: if title.is_empty() { "(nothing asked yet)".into() } else { title },
                            about: format!("{}{set}", since(row.saved_at as f64, now)),
                        }
                    })
                    .collect())
            }
            Completes::Models => {
                let Some(models) = self.models() else { return Ok(vec![]) };
                let current = models.current().model;
                // The providers signed in to and the model servers running, as /model's list has them, read together.
                let statuses = models.providers().await?;
                let servers = models.local().await;
                let sources: Vec<String> = statuses
                    .iter()
                    .filter(|status| status.signed_in)
                    .map(|status| status.id.as_str().to_string())
                    .chain(servers.into_iter().filter(|server| server.running).map(|server| server.id))
                    .collect();
                let lists = futures::future::join_all(sources.iter().map(|source| models.models(source, false))).await;
                Ok(lists
                    .into_iter()
                    .flatten()
                    .flatten()
                    .map(|info| {
                        let about =
                            if current.as_deref() == Some(info.id.as_str()) { format!("{} · chosen", info.name) } else { info.name };
                        Candidate { word: info.id.clone(), label: info.id, about }
                    })
                    .collect())
            }
            Completes::Words(_) | Completes::SlotModels => Ok(vec![]),
        }
    }
}
