//! Logic's custom pop-ups: a Mixer strip's insert slot, insert bar and send slot (and an occupied slot's list button)
//! open a menu Logic draws itself. Accessibility shows no element for it (the window server shows it, a window at the
//! pop-up menu level), and a press holds the Accessibility call while the menu is up (M2's finding), until the
//! helper's messaging timeout gives up on it: the press is answered `cannotComplete` after 1.5 s (or `timeout`, on a
//! slower Mac) with the menu still up, and keys posted to Logic then go to the menu (measured on Logic Pro 12.3.1):
//! the plug-in menu's search takes the plug-in's name typed key by key, Down highlights its first result and Return
//! takes it; the send menu is walked by arrows; Escape closes either. So a choice is the press, whose held answer is
//! the menu open, the keys, and the slot read back by the caller. A key the helper can't take yet (its worker still
//! held by the press) is answered `busy`, and posted again.

use std::time::Duration;

use super::nudge::Control;
use super::{busy, Ctx, Done};
use crate::ax::AxError;
use crate::helper::HelperError;
use crate::refs::failed;

/// How long the menu gets to draw before keys go to it, and between keys.
const MENU_SETTLE: Duration = Duration::from_millis(300);
const KEY_GAP: Duration = Duration::from_millis(120);
/// How often a key the helper couldn't take yet (its worker still held by the press) is posted again, and the pause
/// between.
const KEY_TRIES: usize = 8;
const KEY_RETRY: Duration = Duration::from_millis(400);

/// A key to post to the open menu, by its name (`down`, `right`, `return`, `escape`, a letter).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Key {
    Named(&'static str),
}

/// Whether a helper error is the press held by an open menu: Accessibility's `cannotComplete` after its messaging
/// timeout, the helper's own `timeout`, or no reply in time.
pub fn held(error: &AxError) -> bool {
    matches!(error, AxError::Helper(HelperError::Timeout))
        || matches!(error, AxError::Helper(HelperError::Refused { code, .. }) if code == "timeout" || code == "cannotComplete")
}

/// The keys that type a name into a menu's search, one key event each (the search takes key codes, not text): a
/// letter lower-cased, a digit, a space, the U.S. layout's unshifted punctuation; the typing stops at a character
/// none of those makes, and the search narrows on what came before it.
pub fn typed(name: &str) -> Vec<Key> {
    const PLAIN: &str = "-=[]\\;',./`";
    let mut keys = Vec::new();
    for c in name.chars() {
        let c = c.to_ascii_lowercase();
        let key: Option<&'static str> = match c {
            ' ' => Some("space"),
            'a'..='z' | '0'..='9' => KEYS.iter().find(|k| k.starts_with(c) && k.len() == 1).copied(),
            c if PLAIN.contains(c) => KEYS.iter().find(|k| k.starts_with(c) && k.len() == 1).copied(),
            _ => None,
        };
        match key {
            Some(key) => keys.push(Key::Named(key)),
            None => break,
        }
    }
    keys
}

/// The one-character key names the helper takes, as `&'static str`s.
const KEYS: [&str; 47] = [
    "a", "b", "c", "d", "e", "f", "g", "h", "i", "j", "k", "l", "m", "n", "o", "p", "q", "r", "s", "t", "u", "v", "w", "x", "y", "z", "0",
    "1", "2", "3", "4", "5", "6", "7", "8", "9", "-", "=", "[", "]", "\\", ";", "'", ",", ".", "/", "`",
];

/// Whether a helper error says its worker is still held by the press.
fn worker_held(error: &AxError) -> bool {
    held(error) || matches!(error, AxError::Helper(HelperError::Refused { code, .. }) if code == "busy")
}

/// Presses a custom pop-up: Ok when the press was held (the menu is up), an error when it was answered at once (no
/// menu stayed up) or refused.
pub fn open(ctx: &mut Ctx, control: &Control) -> Done<()> {
    match ctx.ax.action(&control.root, &control.path, "AXPress", 50) {
        Err(error) if held(&error) => {
            super::pause(MENU_SETTLE);
            Ok(())
        }
        Ok(_) => Err(failed("Logic Pro's slot answered the press without keeping its menu up")),
        Err(error) => Err(super::ax_fault(&error, false)),
    }
}

/// Posts one key to Logic (the open menu takes it), again while the helper's worker is still held by the press.
fn post(ctx: &mut Ctx, key: &Key) -> Done<()> {
    let mut last = None;
    for _ in 0..KEY_TRIES {
        let Key::Named(name) = key;
        let result = ctx.ax.keys(name, &[]);
        match result {
            Ok(_) => return Ok(()),
            Err(error) if worker_held(&error) => {
                last = Some(error);
                super::pause(KEY_RETRY);
            }
            Err(error) => return Err(super::ax_fault(&error, true)),
        }
    }
    Err(busy(format!(
        "Logic Pro's menu held the helper too long to take a key ({}); press Escape in Logic Pro if a menu is still open",
        last.map(|e| e.to_string()).unwrap_or_default()
    )))
}

/// Posts keys to the open menu, one after another.
pub fn keys(ctx: &mut Ctx, keys: &[Key]) -> Done<()> {
    for key in keys {
        post(ctx, key)?;
        super::pause(KEY_GAP);
    }
    Ok(())
}

/// Closes a menu left up (Escape), as far as it can: after a choice that went wrong, so Logic takes input again.
pub fn close(ctx: &mut Ctx) {
    let _ = post(ctx, &Key::Named("escape"));
    super::pause(KEY_GAP);
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn a_press_held_by_its_menu_is_the_menu_open_and_keys_wait_for_the_helper() {
        assert!(held(&AxError::Helper(HelperError::Timeout)));
        assert!(held(&AxError::Helper(HelperError::Refused { code: "timeout".into(), reply: json!({}) })));
        assert!(!held(&AxError::Helper(HelperError::Refused { code: "busy".into(), reply: json!({}) })));
        assert!(worker_held(&AxError::Helper(HelperError::Refused { code: "busy".into(), reply: json!({}) })));
        assert!(!worker_held(&AxError::Missing("x".into())));
        assert!(held(&AxError::Helper(HelperError::Refused { code: "cannotComplete".into(), reply: json!({}) })));
        let names: Vec<Key> = typed("Channel EQ");
        assert_eq!(names.len(), 10);
        assert_eq!((names[0].clone(), names[7].clone()), (Key::Named("c"), Key::Named("space")));
        assert_eq!(typed("Pitch (Mono)").len(), 6, "the typing stops at the bracket");
        crate::changes::set_settling(false);
        let hands = crate::helper::MockHands::new();
        hands.reply("action", Err(HelperError::Refused { code: "timeout".into(), reply: json!({"ok": false, "error": "timeout"}) }));
        hands.reply("keys", Err(HelperError::Refused { code: "busy".into(), reply: json!({"ok": false, "error": "busy"}) }));
        hands.reply("keys", Ok(json!({"posted": true, "active": true})));
        hands.reply("keys", Ok(json!({"posted": true, "active": true})));
        let mut launcher = hands.clone();
        let mut helper = crate::helper::Launch::launch(&mut launcher).unwrap();
        let mut ax = crate::ax::Ax::new(helper.as_mut(), 7);
        let mut project = crate::project::Project::new(crate::ax::strings::Strings::english());
        let mut ctx = Ctx { project: &mut project, ax: &mut ax };
        let slot = Control::new(&crate::ax::Root::Main, "8.1.0.15");
        open(&mut ctx, &slot).unwrap();
        keys(&mut ctx, &[Key::Named("g"), Key::Named("return")]).unwrap();
        let ops: Vec<String> = hands.calls().into_iter().map(|(op, _)| op).collect();
        assert_eq!(ops, ["action", "keys", "keys", "keys"], "the first key posted again once the helper was free");
        hands.reply("action", Ok(json!({"result": "success"})));
        assert_eq!(open(&mut ctx, &slot).unwrap_err().0, "failed", "a press answered at once kept no menu up");
    }
}
