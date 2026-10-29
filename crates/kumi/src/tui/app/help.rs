//! The help: every key and command, in sections, opened by /help or by `?` on an empty box. Each row is a key with what
//! it does beside it, wrapped to the panel's width so all of it shows at any size; ↑↓, the pages, home and end scroll it.
use super::super::{
    width::text_width,
    wrap::{wrap, Span},
};
use super::drawing::{sp, st};
use super::input::COMMANDS;
use super::*;

/// The widest the keys column gets: a key wider than this has what it does on the rows under it.
const KEYS_MOST: i32 = 18;
/// What a key does goes beside it while it has this much room; on a narrower panel, under it.
const BESIDE_LEAST: i32 = 24;

/// A section of the help: its heading, then its keys (or commands), each with what it does.
pub(super) struct HelpSection {
    pub title: &'static str,
    pub rows: Vec<(String, String)>,
}

/// What this session has that the help's keys depend on.
#[derive(Clone, Copy, Default)]
pub(super) struct HelpHas {
    pub voice: bool,
    pub attachments: bool,
    pub jobs: bool,
    pub palette: bool,
    pub slots: bool,
}

/// The help's sections: the keys that work in this session (`has`), in the DAW's words (`app`), the pane's `tabs`, and
/// the commands the `/` menu offers, each with what it says of itself.
pub(super) fn help_sections(has: HelpHas, app: &str, tabs: &[String], commands: Vec<(String, String)>) -> Vec<HelpSection> {
    let row = |keys: &str, does: &str| (keys.to_string(), does.to_string());
    let mut send = vec![
        row("enter", "Send. While Kumi works, it reads your message after the step under way"),
        row("tab", "While Kumi works: send it after the answer instead; it waits above the box"),
        row("alt+↑", "Take the last waiting message back into the box"),
        row("ctrl+j  alt+enter", "A new line (shift+enter too, where the terminal tells it apart)"),
        row("↑ ↓", "The box's lines, then what you sent before"),
    ];
    if has.voice {
        send.push(row(
            "ctrl+t",
            "Talk instead of typing: press it again to stop, or hold it while you talk. What you said lands in the box; \
             enter stops and sends at once, and esc drops it",
        ));
    }
    if has.attachments {
        send.push(row("ctrl+v", "Add the picture on the clipboard to your next message; files dragged in go with it too"));
        send.push(row("backspace", "In an empty box, take back the last file added"));
    }
    if has.jobs {
        send.push((
            "ctrl+b".into(),
            format!("Move a slow step that doesn't use {app} (a video, pages) to the background, and carry on; in tmux, press it twice"),
        ));
    }
    let around = vec![
        row("page up  page down", "Scroll the conversation; the mouse wheel too (over the tabs, it scrolls them)"),
        row("ctrl+home", "The start of the conversation"),
        row("ctrl+end", "Back to the latest"),
        row("drag", "Select text: ctrl+c or a right-click copies it, and esc lets it go"),
        row("shift+drag", "Your terminal's own selection (option+drag in iTerm2)"),
        row("ctrl+l", "Draw the screen again"),
    ];
    let mut pane = vec![
        (
            "shift+tab".to_string(),
            format!("Into the tabs ({}), then on to the next: ↑↓ and page up/down move, esc or tab comes back", tabs.join(", ")),
        ),
        row(
            "enter",
            if has.jobs {
                "In the tabs: open a change's picture or a job; on what Kumi kept, forget it"
            } else {
                "In the tabs: open a change's picture; on what Kumi kept, forget it"
            },
        ),
        row("→  space", "On a change: open its picture; ← closes it"),
        row("u", "On a change with its picture open: undo it"),
    ];
    if has.jobs {
        pane.push(row("x", "On a running job: stop it"));
    }
    if has.palette {
        pane.push(row("space  x", "On a sound in PALETTE: play it, or take it out"));
    }
    pane.extend([
        (
            "tab".to_string(),
            format!("Into the tree in FOCUS, at {app}'s selection: ↑↓ move, enter pins a row so what you ask is about it, esc or tab comes back"),
        ),
        row("esc", "Unpin what's pinned (× beside it does too)"),
        (
            "click".to_string(),
            format!(
                "undo or forget at a row's end; a change's words open its picture{}, a row in FOCUS's tree pins it, and a tab's \
                 name shows that tab",
                if has.jobs { ", a job's open the job" } else { "" }
            ),
        ),
    ]);
    let edit = vec![
        row("← →", "Along the line; with alt or ctrl, a word at a time (alt+b, alt+f too)"),
        row("home  end", "The start or end of the line (ctrl+a, ctrl+e too)"),
        row("backspace", "Delete the character before the cursor"),
        row("delete", "Delete the character after it (ctrl+d too)"),
        row("ctrl+w", "Cut the word before the cursor (alt+backspace and ctrl+backspace too)"),
        row("ctrl+k", "Cut to the end of the line"),
        row("ctrl+u", "Cut to the start of the line"),
        row("ctrl+y", "Put back what you cut last (cuts in a row come back together); alt+y right after swaps it for the cut before"),
        row(
            "ctrl+-  ctrl+_",
            "Undo in the box: a run of typing or deleting, a paste, a cut or what you dictated at a time; ctrl+_ where ctrl+- \
             zooms the terminal",
        ),
    ];
    let mut stop = vec![row("esc", "Stop Kumi's answer: the steps it finished stay, and waiting messages go back into the box")];
    if has.slots {
        stop.push(row("esc", "A /slots check under way stops first"));
    }
    stop.extend([
        row("ctrl+c", "Stop Kumi; when it isn't working, clear the box (↑ or ctrl+- brings the words back), then quit"),
        row("ctrl+d", "Quit, from an empty box while Kumi isn't working"),
        row("/undo", "Undo Kumi's last change; undo at the end of a HISTORY row takes back that one"),
    ]);
    let mut listed = vec![
        row("/", "The commands as you type: ↑↓ choose, tab completes, enter runs, esc closes"),
        row("?", "This help, from an empty box"),
    ];
    listed.extend(commands);
    vec![
        HelpSection { title: "Send and steer", rows: send },
        HelpSection { title: "Move around", rows: around },
        HelpSection { title: "The pane", rows: pane },
        HelpSection { title: "Edit", rows: edit },
        HelpSection { title: "Stop and take back", rows: stop },
        HelpSection { title: "Commands", rows: listed },
    ]
}

/// The help's rows at `width` cells: each section's heading, then each key with what it does beside it, in one column
/// the whole help through and wrapped under itself, with a blank row between sections. Where there's too little room
/// beside the keys, what a key does goes under it. No row runs past `width` (5 cells or more: a panel is never
/// narrower), and nothing is cut.
pub(super) fn help_rows(sections: &[HelpSection], width: i32) -> Vec<Vec<Span>> {
    let width = width.max(1);
    let text = |line: &[Span]| line.iter().map(|span| span.text.as_str()).collect::<String>();
    let pad = |cells: i32| " ".repeat(cells.max(0) as usize);
    let keys = sections.iter().flat_map(|section| &section.rows).map(|(keys, _)| text_width(keys)).max().unwrap_or(0).min(KEYS_MOST);
    let beside = width - keys - 4 >= BESIDE_LEAST;
    // Where what a key does starts, and how much room it has.
    let column = if beside { keys + 4 } else { 4 };
    let mut rows: Vec<Vec<Span>> = vec![];
    for (at, section) in sections.iter().enumerate() {
        if at > 0 {
            rows.push(vec![]);
        }
        rows.extend(wrap(&[sp(section.title, st::LABEL)], width));
        for (key, does) in &section.rows {
            let words: Vec<String> = wrap(&[sp(does, st::TEXT)], (width - column).max(1)).iter().map(|line| text(line)).collect();
            if beside && text_width(key) <= keys {
                for (index, line) in words.iter().enumerate() {
                    rows.push(if index == 0 {
                        vec![sp(format!("  {key}{}", pad(keys - text_width(key))), st::BRIGHT), sp(format!("  {line}"), st::TEXT)]
                    } else {
                        vec![sp(format!("{}{line}", pad(column)), st::TEXT)]
                    });
                }
            } else {
                for line in wrap(&[sp(key, st::BRIGHT)], (width - 2).max(1)) {
                    rows.push(vec![sp(format!("  {}", text(&line)), st::BRIGHT)]);
                }
                rows.extend(words.iter().map(|line| vec![sp(format!("{}{line}", pad(column)), st::TEXT)]));
            }
        }
    }
    rows
}

impl TuiApp {
    /// What the help says in this session: the keys that work here, and the commands the `/` menu offers, in its words.
    pub(super) fn help_sections(&self) -> Vec<HelpSection> {
        let can = self.can();
        let has = HelpHas {
            voice: self.0.voice.is_some(),
            attachments: can.attachments,
            jobs: can.jobs,
            palette: can.palette,
            slots: self.0.options.slots.is_some(),
        };
        let commands =
            COMMANDS.iter().filter(|command| self.offered(&can, command.name)).map(|command| (command.name.into(), command.about()));
        help_sections(has, crate::daw::words().app, &self.0.tabs.titles(), commands.collect())
    }
    /// Opens the help: /help, or `?` on an empty box.
    pub(super) fn open_help(&self) {
        self.0.tabs.leave();
        let mut state = self.0.state.borrow_mut();
        state.tree_cursor = None;
        state.panel = Some(Rc::new(RefCell::new(Panel::Help { scroll: 0, shown: 1, most: 0 })));
        drop(state);
        self.0.scheduler.request();
    }
    /// Keys in the help: ↑↓, the pages, home and end scroll it; esc, enter, `?` and ctrl+c close it. Anything else typed
    /// or pasted closes it and goes into the box, so none of what the producer goes on to type is lost.
    pub(super) fn help_input(&self, panel: &PanelRef, event: InputEvent) {
        match event {
            InputEvent::Key { ref name, mods, .. } => {
                if matches!(name.as_str(), "escape" | "enter") || (mods.ctrl && name == "c") {
                    self.close_panel();
                    return;
                }
                if let Panel::Help { scroll, shown, most } = &mut *panel.borrow_mut() {
                    let page = (*shown - 1).max(1);
                    *scroll = match name.as_str() {
                        "up" => *scroll - 1,
                        "down" => *scroll + 1,
                        "pageup" => *scroll - page,
                        "pagedown" => *scroll + page,
                        "home" => 0,
                        "end" => *most,
                        _ => *scroll,
                    }
                    .clamp(0, *most);
                }
            }
            InputEvent::Text { ref text } if text == "?" => self.close_panel(),
            InputEvent::Text { .. } | InputEvent::Paste { .. } => {
                self.close_panel();
                self.on_input(event);
            }
            _ => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn every_section() -> Vec<HelpSection> {
        let has = HelpHas { voice: true, attachments: true, jobs: true, palette: true, slots: true };
        let tabs = ["HISTORY", "GOAL", "JOBS", "PALETTE"].map(String::from);
        help_sections(has, "Live", &tabs, COMMANDS.iter().map(|command| (command.name.to_string(), command.about.to_string())).collect())
    }

    fn lines(rows: &[Vec<Span>]) -> Vec<String> {
        rows.iter().map(|row| row.iter().map(|span| span.text.as_str()).collect()).collect()
    }

    #[test]
    fn all_of_the_help_shows_at_any_width_and_no_row_runs_past_it() {
        let sections = every_section();
        for width in 5..=120 {
            let lines = lines(&help_rows(&sections, width));
            for line in &lines {
                assert!(text_width(line) <= width, "{line:?} is wider than {width}");
            }
            // Every character of every heading, key and description, in order: words wider than a row are split.
            let shown: String = lines.concat().chars().filter(|c| !c.is_whitespace()).collect();
            let mut from = 0;
            for section in &sections {
                let wanted = std::iter::once(section.title.to_string())
                    .chain(section.rows.iter().flat_map(|(keys, does)| [keys.clone(), does.clone()]));
                for wanted in wanted {
                    let wanted: String = wanted.chars().filter(|c| !c.is_whitespace()).collect();
                    let at = shown[from..].find(&wanted).unwrap_or_else(|| panic!("{wanted:?} is missing at width {width}"));
                    from += at + wanted.len();
                }
            }
        }
    }

    #[test]
    fn words_stay_whole_and_what_a_key_does_lines_up_beside_it() {
        let sections = every_section();
        for width in [74, 96] {
            let lines = lines(&help_rows(&sections, width));
            let joined = lines.join(" ").split_whitespace().collect::<Vec<_>>().join(" ");
            for (keys, does) in sections.iter().flat_map(|section| &section.rows) {
                let does = does.split_whitespace().collect::<Vec<_>>().join(" ");
                assert!(joined.contains(&format!("{} {does}", keys.split_whitespace().collect::<Vec<_>>().join(" "))), "{keys}: {does}");
            }
            // What the keys do starts in one column, the whole help through.
            let starts =
                |key: &str, does: &str| lines.iter().find(|line| line.starts_with(&format!("  {key} "))).and_then(|line| line.find(does));
            assert_eq!(starts("enter", "Send."), starts("ctrl+home", "The start"));
            assert_eq!(starts("enter", "Send."), starts("/quit", "Close Kumi"));
        }
    }

    #[test]
    fn a_narrow_panel_puts_what_a_key_does_under_it() {
        let lines = lines(&help_rows(&every_section(), 30));
        let at = lines.iter().position(|line| line == "  enter").expect("enter on a row of its own");
        assert!(lines[at + 1].starts_with("    Send."), "{:?}", &lines[at..at + 2]);
    }
}
