//! Logic in front, and its Edit menu's titles fresh. A menu item or a modified key command acts only while Logic is
//! the app in front, so a change that needs one asks macOS to bring Logic forward and checks that it is, and refuses
//! `busy` rather than fight an app that keeps the front. The Edit menu's Undo and Redo titles are set by Logic only
//! when it validates that menu: a key command of its own (the producer's Cmd-Z among them) validates nothing, so the
//! titles are refreshed on purpose with a key equivalent Logic doesn't own, which opens nothing — and only while
//! Logic is in front, where the key lands.
//!
//! The bridge never presses a menu bar item. It used to, to validate the Edit menu while Logic was behind (a press
//! and a cancel of the item, invisible since the menu bar wasn't on screen); at the M3 gate one such press left the
//! item `selected` with no menu to cancel, and from then on Logic took no click, key, selection or activation at
//! all, and answered no Apple Event, until a real click: the next change failed, and so would every one after it.
//! A menu bar item left open is a wedge, so while Logic is behind the titles are read as they stand (a read, which
//! can't wedge anything) and the focus tick puts no input into Logic on its own; the probes name the producer's
//! undo of a header, a track or the tempo at once, and a marker or note step is named when Logic next validates
//! its Edit menu (Kumi's next write while Logic is in front, or the producer opening the menu). What is kept of the
//! lesson is the cure: before a write gives up on the front as `busy`, a menu bar item found left open is closed
//! with a cancel on its menu, as that cleared the wedge at the gate.

use crate::ax::strings::Strings;
use crate::ax::{Ax, AxError, Root};
use crate::changes::{busy, nudge, Done};
use crate::log;

/// The key equivalent posted to refresh the menus while Logic is in front: one Logic doesn't own.
const REFRESH_KEY: &str = "f18";
const REFRESH_MODS: [&str; 4] = ["cmd", "opt", "ctrl", "shift"];
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

/// Posts a chord to Logic, which must be in front for a modified key to land (the caller brings it there): `busy`
/// when Logic lost the front meanwhile.
pub fn post(ax: &mut Ax, chord: &crate::keys::Chord) -> Done<()> {
    let active = nudge::wrote(ax.chord(&chord.helper_key(), &chord.mods()))?;
    if !active {
        return Err(busy("Logic Pro wasn't in front when the key was posted, so it didn't land; nothing changed"));
    }
    Ok(())
}

/// The menu bar item of the Edit menu: its path below the menu bar, found by its title in Logic's language.
pub fn edit_menu(ax: &mut Ax, strings: &Strings) -> Result<String, AxError> {
    let bar = ax.tree(&Root::MenuBar, "", 1, 20, Some(&["title"]))?;
    let found = bar.children.iter().find(|item| item.role == "AXMenuBarItem" && strings.is(&item.title, "Edit"));
    Ok(found.map(|item| item.path.clone()).unwrap_or_else(|| "3".to_string()))
}

/// Makes Logic validate its menus, so the Edit menu's Undo and Redo titles say what its history holds now: a key
/// equivalent it doesn't own, which opens nothing. It lands only while Logic is in front; the caller checks that.
pub fn refresh(ax: &mut Ax) -> Result<(), AxError> {
    ax.keys(REFRESH_KEY, &REFRESH_MODS)?;
    Ok(())
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
    fn the_edit_menu_is_found_by_its_title_and_refreshed_by_a_key_that_opens_nothing() {
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
        hands.reply("keys", Ok(json!({"posted": true, "active": true})));
        refresh(&mut ax).unwrap();
        let calls = hands.calls();
        assert_eq!((calls[1].0.as_str(), calls[1].1["key"].as_str()), ("keys", Some("f18")));
        assert_eq!(calls[1].1["mods"], json!(["cmd", "opt", "ctrl", "shift"]));
        assert_eq!(hands.calls_of("action"), 0, "the bridge presses no menu bar item");
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
