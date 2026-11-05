//! Bringing Logic forward for an explicit edit and reading Edit-menu titles without posting any input to refresh
//! them. Generic titles cannot prove native Undo ownership. A menu bar item left open can wedge input, so explicit
//! edits may cancel an already open menu before giving up on activation; history polling never does so.

use crate::ax::strings::Strings;
use crate::ax::{Ax, AxError, Root};
use crate::changes::{busy, nudge, Done};
use crate::log;

/// How long a cancel on a menu left open gets to close it.
const CANCEL_SETTLE_MS: u64 = 40;
/// How many cancels a menu left open gets before it is given up on.
const CANCELS: usize = 2;

/// Brings Logic in front for a menu item, a key or a click; `busy` when another app keeps the front. A menu bar item
/// left open takes every click and key Logic is sent and its activation too, so one found open is closed first, and
/// Logic asked forward once more.
pub fn ensure(ax: &mut Ax) -> Done<()> {
    if nudge::read(ax.in_front())? {
        return Ok(());
    }
    if nudge::read(ax.activate())? {
        return Ok(());
    }
    match close_open_menu(ax) {
        Ok(Some(closed)) => {
            log::line(&format!("the {closed} menu bar item was left open (which takes every click and key): closed it"));
            if nudge::read(ax.activate())? {
                return Ok(());
            }
        }
        Ok(None) => {}
        Err(error) => log::line(&format!("couldn't look for a menu bar item left open: {error}")),
    }
    Err(busy("Logic Pro couldn't be brought in front (another app keeps it); nothing changed"))
}

/// The menu bar item of the Edit menu: its path below the menu bar, found by its title in Logic's language.
pub fn edit_menu(ax: &mut Ax, strings: &Strings) -> Result<String, AxError> {
    let bar = ax.tree(&Root::MenuBar, "", 1, 20, Some(&["title"]))?;
    let found = bar.children.iter().find(|item| item.role == "AXMenuBarItem" && strings.is(&item.title, "Edit"));
    Ok(found.map(|item| item.path.clone()).unwrap_or_else(|| "3".to_string()))
}

/// The Edit menu's first two items: the Undo and Redo titles as they stand.
pub fn titles(ax: &mut Ax, edit: &str) -> Result<(String, String), AxError> {
    let menu = ax.tree(&Root::MenuBar, &format!("{edit}.0"), 1, 2, Some(&["title", "enabled"]))?;
    let title = |index: usize| menu.children.get(index).map(|item| item.title.clone()).unwrap_or_default();
    Ok((title(0), title(1)))
}

/// A menu bar item left `selected` (open, with or without its menu showing) is closed with a cancel on its menu,
/// read back, and cancelled once more if it stayed; the item's title when one was found, whether or not it closed.
pub fn close_open_menu(ax: &mut Ax) -> Result<Option<String>, AxError> {
    let open = |ax: &mut Ax| -> Result<Option<(String, String)>, AxError> {
        let bar = ax.tree(&Root::MenuBar, "", 1, 20, Some(&["title", "selected"]))?;
        let found = bar.children.iter().find(|item| item.role == "AXMenuBarItem" && item.is_selected());
        Ok(found.map(|item| (item.title.clone(), item.path.clone())))
    };
    let Some((title, path)) = open(ax)? else { return Ok(None) };
    for _ in 0..CANCELS {
        ax.action(&Root::MenuBar, &format!("{path}.0"), "AXCancel", CANCEL_SETTLE_MS)?;
        if open(ax)?.is_none() {
            return Ok(Some(title));
        }
    }
    log::line(&format!("the {title} menu bar item stays open after {CANCELS} cancels: Logic Pro may take no input until a real click"));
    Ok(Some(title))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::helper::{Launch, MockHands};
    use serde_json::{json, Value};

    fn bar(edit_selected: bool) -> Value {
        json!({"role": "AXMenuBar", "c": [
            {"i": "0", "role": "AXMenuBarItem", "title": "Apple"}, {"i": "1", "role": "AXMenuBarItem", "title": "Logic Pro"},
            {"i": "2", "role": "AXMenuBarItem", "title": "File"},
            {"i": "3", "role": "AXMenuBarItem", "title": "Edit", "selected": edit_selected}
        ]})
    }

    #[test]
    fn logic_is_brought_in_front_once_and_an_app_that_keeps_the_front_is_busy() {
        let hands = MockHands::new();
        hands.set_apps(vec![json!({"bundle": "com.apple.logic10", "pid": 7, "active": true})]);
        hands.set_tree(bar(false));
        let mut launcher = hands.clone();
        let mut helper = Launch::launch(&mut launcher).unwrap();
        let mut ax = Ax::new(helper.as_mut(), 7);
        assert!(ensure(&mut ax).is_ok());
        assert_eq!(hands.calls_of("activate"), 0, "in front already");
        hands.set_apps(vec![json!({"bundle": "com.apple.logic10", "pid": 7, "active": false})]);
        hands.reply("activate", Ok(json!({"active": true})));
        assert!(ensure(&mut ax).is_ok());
        hands.reply("activate", Ok(json!({"active": false})));
        let refused = ensure(&mut ax).unwrap_err();
        assert_eq!(refused.0, "busy");
        assert!(refused.1.ends_with("nothing changed"), "{}", refused.1);
        assert_eq!(hands.calls_of("action"), 0, "no menu bar item was open: nothing to cancel");
    }

    #[test]
    fn the_edit_menu_is_found_by_its_title_and_observed_without_posting_a_key() {
        let hands = MockHands::new();
        hands.set_tree(json!({"role": "AXMenuBar", "c": [
            {"i": "0", "role": "AXMenuBarItem", "title": "Apple"}, {"i": "1", "role": "AXMenuBarItem", "title": "Logic Pro"},
            {"i": "2", "role": "AXMenuBarItem", "title": "File"}, {"i": "3", "role": "AXMenuBarItem", "title": "Bearbeiten"}
        ]}));
        let strings = Strings::with("de", &[("Edit", "Bearbeiten")]);
        let mut launcher = hands.clone();
        let mut helper = Launch::launch(&mut launcher).unwrap();
        let mut ax = Ax::new(helper.as_mut(), 7);
        assert_eq!(edit_menu(&mut ax, &strings).unwrap(), "3");
        assert_eq!(hands.calls_of("action") + hands.calls_of("keys"), 0, "title bookkeeping sends no input");
        hands.set_tree(json!({"role": "AXMenu", "c": [
            {"i": "3.0.0", "role": "AXMenuItem", "title": "Undo Renaming"}, {"i": "3.0.1", "role": "AXMenuItem", "title": "Can’t Redo"}
        ]}));
        assert_eq!(titles(&mut ax, "3").unwrap(), ("Undo Renaming".to_string(), "Can’t Redo".to_string()));
    }

    #[test]
    fn a_menu_bar_item_left_open_is_closed_with_a_cancel_on_its_menu_before_the_front_is_given_up_on() {
        // As at the M3 gate: the Edit item stayed selected and Logic took no activation, until a cancel on its menu.
        let hands = MockHands::new();
        hands.set_apps(vec![json!({"bundle": "com.apple.logic10", "pid": 7, "active": false})]);
        hands.reply("activate", Ok(json!({"active": false})));
        hands.reply("tree", Ok(json!({"node": bar(true)})));
        hands.reply("action", Ok(json!({"result": "success"})));
        hands.reply("tree", Ok(json!({"node": bar(false)})));
        hands.reply("activate", Ok(json!({"active": true})));
        let mut launcher = hands.clone();
        let mut helper = Launch::launch(&mut launcher).unwrap();
        let mut ax = Ax::new(helper.as_mut(), 7);
        assert!(ensure(&mut ax).is_ok());
        let calls = hands.calls();
        let ops: Vec<&str> = calls.iter().map(|(op, _)| op.as_str()).collect();
        assert_eq!(ops, ["apps", "activate", "tree", "action", "tree", "activate"]);
        assert_eq!(
            (calls[3].1["name"].as_str(), calls[3].1["path"].as_str()),
            (Some("AXCancel"), Some("3.0")),
            "the Edit menu, not its bar item"
        );
    }

    #[test]
    fn a_menu_that_stays_open_after_two_cancels_is_given_up_on_and_the_change_is_busy() {
        let hands = MockHands::new();
        hands.set_apps(vec![json!({"bundle": "com.apple.logic10", "pid": 7, "active": false})]);
        hands.set_tree(bar(true));
        hands.reply("activate", Ok(json!({"active": false})));
        hands.reply("action", Ok(json!({"result": "success"})));
        hands.reply("action", Ok(json!({"result": "success"})));
        hands.reply("activate", Ok(json!({"active": false})));
        let mut launcher = hands.clone();
        let mut helper = Launch::launch(&mut launcher).unwrap();
        let mut ax = Ax::new(helper.as_mut(), 7);
        let refused = ensure(&mut ax).unwrap_err();
        assert_eq!(refused.0, "busy");
        assert_eq!(hands.calls_of("action"), CANCELS, "two cancels, then no more");
        assert_eq!(hands.calls_of("activate"), 2, "asked forward once more after the cancels, then given up on");
        assert!(hands.calls().iter().all(|(op, p)| op != "action" || p["name"] == "AXCancel"), "never a press");
    }
}
