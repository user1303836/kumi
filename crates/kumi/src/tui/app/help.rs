//! The help: every key and command, in sections, opened by /help or by `?` on an empty box. Each row is a key with what
//! it does beside it, wrapped to the panel's width so all of it shows at any size; ↑↓, the pages, home and end scroll it.
use super::super::{
    render::Cursor,
    width::text_width,
    wrap::{wrap, Span},
};
use super::draw_panel::Line;
use super::drawing::{sp, st};
use super::input::about;
use super::*;
use crate::has::Has;
use kumi_agent::core::commands::COMMANDS;

/// The widest the keys column gets: a key wider than this has what it does on the rows under it.
const KEYS_MOST: i32 = 18;
/// What a key does goes beside it while it has this much room; on a narrower panel, under it.
const BESIDE_LEAST: i32 = 24;

/// A section of the help: its heading, then its keys (or commands), each with what it does.
pub(super) struct HelpSection {
    pub title: &'static str,
    pub rows: Vec<(String, String)>,
}

/// The help's sections: the keys that work in this session (`has`), in the DAW's words (`app`), the pane's `tabs`, and
/// the commands the `/` menu offers, each with what it says of itself.
pub(super) fn help_sections(has: Has, app: &str, tabs: &[String], commands: Vec<(String, String)>) -> Vec<HelpSection> {
    let row = |keys: &str, does: &str| (keys.to_string(), does.to_string());
    let mut send = vec![
        row("enter", "Send. While Kumi works, it reads your message after the step under way"),
        row("tab", "While Kumi works: send it after the answer instead; it waits above the box"),
        row("alt+↑", "Take the last waiting message back into the box"),
        row("ctrl+j  alt+enter", "A new line (shift+enter too, where the terminal tells it apart)"),
        row("↑ ↓", "The box's lines, then what you sent before"),
        row(
            "ctrl+r",
            "Look back through what you sent for the words you type: ctrl+r again finds an earlier one, enter puts it in the \
             box, esc goes back to your draft",
        ),
    ];
    if has.fronts.voice {
        send.push(row(
            "ctrl+t",
            "Talk instead of typing: press it again to stop, or hold it while you talk. What you said lands in the box; \
             enter stops and sends at once, and esc drops it",
        ));
    }
    if has.can.attachments {
        send.push(row("ctrl+v", "Add the picture on the clipboard to your next message; files dragged in go with it too"));
        send.push(row("backspace", "In an empty box, take back the last file added"));
    }
    if has.can.jobs {
        send.push((
            "ctrl+b".into(),
            format!("Move a slow step that doesn't use {app} (a video, pages) to the background, and carry on; in tmux, press it twice"),
        ));
    }
    let mut around = vec![
        row(
            "page up  page down",
            if has.mouse {
                "Scroll the conversation; the mouse wheel too (over the tabs, it scrolls them)"
            } else {
                "Scroll the conversation"
            },
        ),
        row("ctrl+home", "The start of the conversation"),
        row("ctrl+end", "Back to the latest"),
        row(
            "ctrl+o",
            "The conversation in detail, with what each step was given and gave back: / searches it, { } go from request \
             to request, [ prints it into the terminal's scrollback, v opens it in your editor",
        ),
    ];
    if has.mouse {
        around.push(row(
            "drag",
            if has.copy_on_select {
                "Copy text as you let go; double-click for a word, triple-click for a line"
            } else {
                "Select text (double-click for a word, triple-click for a line); ctrl+c or a right-click copies it, and esc \
                 clears it"
            },
        ));
        around.push(row("shift+drag", "Your terminal's own selection (option+drag in iTerm2)"));
    } else {
        around.push(row("drag", "The mouse is your terminal's (\"mouse\": \"off\"), so its own selection copies"));
    }
    around.push(row("ctrl+l", "Draw the screen again"));
    let mut pane = vec![
        row(
            "ctrl+\\",
            "Below 100 columns, open or close the drawer; at full width, put the side pane away or bring it back; below 24 \
             rows, the same for the two-line strip",
        ),
        (
            "shift+tab".to_string(),
            format!("Into the tabs ({}), then on to the next: ↑↓ and page up/down move, esc or tab comes back", tabs.join(", ")),
        ),
        row(
            "enter",
            if has.can.jobs {
                "In the tabs: open a change's picture or a job; on what Kumi kept, forget it"
            } else {
                "In the tabs: open a change's picture; on what Kumi kept, forget it"
            },
        ),
        row("→  space", "On a change: open its picture; ← closes it"),
        row("u", "On a change with its picture open: undo it"),
    ];
    if has.can.jobs {
        pane.push(row("x", "On a running job: stop it"));
    }
    if has.can.palette {
        pane.push(row("space  x", "On a sound in PALETTE: play it, or take it out"));
    }
    pane.extend([
        (
            "tab".to_string(),
            format!("Into the tree in FOCUS, at {app}'s selection: ↑↓ move, enter pins a row so what you ask is about it, esc or tab comes back"),
        ),
        row("esc", if has.mouse { "Unpin what's pinned (× beside it does too)" } else { "Unpin what's pinned" }),
    ]);
    if has.mouse {
        pane.push((
            "click".to_string(),
            format!(
                "undo or forget at a row's end; a change's words open its picture{}, a row in FOCUS's tree pins it, and a tab's \
                 name shows that tab",
                if has.can.jobs { ", a job's open the job" } else { "" }
            ),
        ));
    }
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
        row(
            "ctrl+g",
            "Open the draft in your own editor (VISUAL or EDITOR); what you save comes back into the box, and ctrl+- takes it \
             back",
        ),
        row("ctrl+s", "Put the draft aside, with its files; in an empty box, bring it back"),
    ];
    let mut stop = vec![row("esc", "Stop Kumi's answer: the steps it finished stay, and waiting messages go back into the box")];
    if has.can.slots {
        stop.push(row("esc", "A /slots check under way stops first"));
    }
    stop.extend([
        row("ctrl+c", "Stop Kumi; when it isn't working, clear the box (↑ or ctrl+- brings the words back), then quit"),
        row("ctrl+d", "Quit, from an empty box while Kumi isn't working"),
        row(
            "/undo",
            if has.mouse {
                "Undo Kumi's last change; undo at the end of a HISTORY row takes back that one"
            } else {
                "Undo Kumi's last change"
            },
        ),
    ]);
    if has.can.take_back {
        stop.push(row(
            "/takeback",
            "Take back your last request (/takeback 3, your last three): Kumi undoes its changes, newest first, and its words \
             come back to the box to change and send again",
        ));
    }
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
        let has = self.has();
        let commands = COMMANDS.iter().filter(|command| has.offers(command)).map(|command| (command.name.into(), about(command)));
        // The tabs shift+tab goes through: those on the strip.
        let tabs: Vec<String> = self.0.tabs.titles().into_iter().map(|(_, title, _)| title).collect();
        help_sections(has, crate::daw::words().app, &tabs, commands.collect())
    }
    /// Opens the help: /help, or `?` on an empty box.
    pub(super) fn open_help(&self) {
        self.0.tabs.leave();
        self.0.state.borrow_mut().tree_cursor = None;
        self.open_overlay(HelpPanel { scroll: 0, shown: 1, most: 0 });
    }
}

/// The help, open over the conversation: scrolled `scroll` rows down; `shown` rows fit when it was last drawn, and it
/// scrolls `most` rows at most.
pub(super) struct HelpPanel {
    scroll: i32,
    shown: i32,
    most: i32,
}
impl Overlay<TuiApp> for HelpPanel {
    /// ↑↓, the pages, home and end scroll it; esc, enter, `?` and ctrl+c close it. Anything else typed or pasted closes
    /// it and goes into the box, so none of what the producer goes on to type is lost.
    fn handle(&mut self, _: &TuiApp, event: InputEvent) -> Handled {
        match event {
            InputEvent::Key { ref name, mods, .. } => {
                if matches!(name.as_str(), "escape" | "enter") || (mods.ctrl && name == "c") {
                    return Handled::Close;
                }
                let page = (self.shown - 1).max(1);
                self.scroll = match name.as_str() {
                    "up" => self.scroll - 1,
                    "down" => self.scroll + 1,
                    "pageup" => self.scroll - page,
                    "pagedown" => self.scroll + page,
                    "home" => 0,
                    "end" => self.most,
                    _ => self.scroll,
                }
                .clamp(0, self.most);
                Handled::Kept
            }
            InputEvent::Text { ref text } if text == "?" => Handled::Close,
            InputEvent::Text { .. } | InputEvent::Paste { .. } => Handled::Pass(event),
            _ => Handled::Kept,
        }
    }
    /// Its title, then the rows that fit from under the header down to the box. How far it has scrolled is said beside
    /// the title, so no row loses its end to it. It's read rather than chosen from, so it takes more of a wide window
    /// than other panels, and less of it wraps.
    fn draw(&mut self, app: &TuiApp, screen: &mut Screen, area: Area) -> Option<Cursor> {
        let width = 100.min(area.left - 2);
        let rows = help_rows(&app.help_sections(), width - 4);
        let room = (area.box_top - 5 - 2).max(3);
        (self.shown, self.most) = (room.min(rows.len() as i32), (rows.len() as i32 - room).max(0));
        self.scroll = self.scroll.clamp(0, self.most);
        let (above, below) = (self.scroll, self.most - self.scroll);
        let mut title = Line::new("Keys and commands", st::TITLE);
        title.right = match (above > 0, below > 0) {
            (true, true) => Some(format!("↑ {above} · ↓ {below} more")),
            (true, false) => Some(format!("↑ {above} more")),
            (false, true) => Some(format!("↓ {below} more")),
            (false, false) => None,
        }
        .map(|more| sp(more, st::FAINT));
        let mut lines = vec![title];
        for row in rows.into_iter().skip(self.scroll as usize).take(room as usize) {
            let mut line = Line::new("", st::TEXT);
            line.spans = Some(row);
            lines.push(line);
        }
        app.draw_lines(screen, area.box_top, width, &lines, None)
    }
    fn hint(&self, _: &TuiApp) -> String {
        "↑↓ to scroll · esc to close".into()
    }
    fn scroll(&mut self, _: &TuiApp, rows: i32) -> bool {
        self.scroll = (self.scroll + rows).clamp(0, self.most);
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use kumi_agent::core::{commands::Fronts, protocol::Capabilities};

    /// The whole help, every key there, with text copying as the mouse lets go or not, and the mouse Kumi's or the
    /// terminal's.
    fn every_section_with(copy_on_select: bool, mouse: bool) -> Vec<HelpSection> {
        let can = Capabilities { attachments: true, jobs: true, palette: true, slots: true, take_back: true, ..Default::default() };
        let has = Has { can, fronts: Fronts { voice: true, ..Default::default() }, copy_on_select, mouse };
        let tabs = ["HISTORY", "GOAL", "JOBS", "PALETTE"].map(String::from);
        help_sections(
            has,
            "Live",
            &tabs,
            COMMANDS
                .iter()
                .map(|command| (command.name.to_string(), command.about(&kumi_types::daw::DawProfile::ABLETON.words, true)))
                .collect(),
        )
    }

    fn every_section() -> Vec<HelpSection> {
        every_section_with(true, true)
    }

    /// Every way the help can be: copying as the mouse lets go or not, and the mouse Kumi's or the terminal's.
    fn every_help() -> Vec<Vec<HelpSection>> {
        [(true, true), (false, true), (true, false), (false, false)].map(|(copying, mouse)| every_section_with(copying, mouse)).into()
    }

    /// The help's rows as text, a key and what it does on each line.
    fn said(sections: &[HelpSection]) -> String {
        sections.iter().flat_map(|section| &section.rows).map(|(keys, does)| format!("{keys}  {does}\n")).collect()
    }

    fn lines(rows: &[Vec<Span>]) -> Vec<String> {
        rows.iter().map(|row| row.iter().map(|span| span.text.as_str()).collect()).collect()
    }

    #[test]
    fn all_of_the_help_shows_at_any_width_and_no_row_runs_past_it() {
        for (sections, width) in every_help().iter().flat_map(|sections| (5..=120).map(move |width| (sections, width))) {
            let lines = lines(&help_rows(&sections, width));
            for line in &lines {
                assert!(text_width(line) <= width, "{line:?} is wider than {width}");
            }
            // Every character of every heading, key and description, in order: words wider than a row are split.
            let shown: String = lines.concat().chars().filter(|c| !c.is_whitespace()).collect();
            let mut from = 0;
            for section in sections {
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
    fn the_wheel_and_kumis_clicks_are_left_out_while_the_mouse_is_the_terminals() {
        for copying in [true, false] {
            let off = said(&every_section_with(copying, false));
            assert!(off.contains("drag  The mouse is your terminal's (\"mouse\": \"off\"), so its own selection copies\n"), "{off}");
            assert!(off.contains("page up  page down  Scroll the conversation\n") && off.contains("esc  Unpin what's pinned\n"), "{off}");
            assert!(off.contains("/undo  Undo Kumi's last change\n"), "{off}");
            for gone in ["wheel", "click", "shift+drag", "×", "HISTORY row", "as you let go", "right-click"] {
                assert!(!off.contains(gone), "{gone}: {off}");
            }
        }
        let on = said(&every_section());
        for there in ["mouse wheel", "\nclick  undo or forget", "\nshift+drag  ", "(× beside it does too)", "HISTORY row", "as you let go"]
        {
            assert!(on.contains(there), "{there}: {on}");
        }
    }

    #[test]
    fn words_stay_whole_and_what_a_key_does_lines_up_beside_it() {
        for (sections, width) in every_help().iter().flat_map(|sections| [74, 96].map(move |width| (sections, width))) {
            let lines = lines(&help_rows(sections, width));
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
