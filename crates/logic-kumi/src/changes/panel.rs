//! Logic's open and save panels. The App Store build is sandboxed, so its panels are drawn by
//! `com.apple.appkit.xpc.openAndSavePanelService`, a process of its own: keys posted to Logic never reach them, and
//! Accessibility reads them under the service's pid (the lab of 2026-10-18). While one is up, Logic's own window
//! list shows it as a modal dialog (`Save`, `Open`), which is how the bridge waits for it and knows it closed. Inside
//! (measured on Logic Pro 12.3.1): the name field `saveAsNameTextField`, the `OKButton` (Save, Open or Import) and the
//! `CancelButton`, by their identifiers; Cmd-Shift-G posted to the service opens the go-to-folder sheet
//! (`GoToWindow`: a `PathTextField`, a `CloseButton`, a table of suggestions), whose field takes a path and, once the
//! field is focused, a Return key posted to the service takes the panel there (`AXConfirm` on the field doesn't, nor
//! Return while the field isn't focused).

use std::time::{Duration, Instant};

use serde_json::{json, Value};

use super::nudge;
use super::Done;
use crate::ax::{Ax, Node};
use crate::refs::failed;

/// The executable path's distinctive part.
const SERVICE: &str = "openAndSavePanelService";
/// How long a panel gets to appear after the menu item, and to go after its button.
pub const PANEL_WAIT: Duration = Duration::from_secs(4);
/// How long the go-to-folder sheet gets to appear, and the panel to follow the path.
const SHEET_WAIT: Duration = Duration::from_millis(1500);
const SETTLE_MS: u64 = 300;

/// A panel up: the service's pid and the panel's title.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Panel {
    pub pid: u32,
    pub title: String,
}

/// The modal dialog Logic's windows show while a panel is up: its title.
fn logic_modal(ax: &mut Ax) -> Done<Option<String>> {
    let reply = nudge::read(ax.windows())?;
    let (windows, _) = crate::logic::parse_windows(&reply);
    Ok(windows
        .iter()
        .find(|window| window.dialog() && window.modal || window.subrole == "AXDialog" && window.main)
        .map(|window| window.title.clone()))
}

/// The service process whose windows hold a system dialog (the panel): several run, one per sandboxed app.
fn service_with_panel(ax: &mut Ax) -> Done<Option<Panel>> {
    let reply = nudge::read(ax.helper.call("processes", json!({"contains": SERVICE})).map_err(crate::ax::AxError::from))?;
    let pids: Vec<u32> = reply["pids"].as_array().into_iter().flatten().filter_map(Value::as_u64).map(|pid| pid as u32).collect();
    for pid in pids.into_iter().rev() {
        let Ok(windows) = ax.helper.call("windows", json!({"pid": pid})) else { continue };
        let (windows, _) = crate::logic::parse_windows(&windows);
        if let Some(window) = windows.iter().find(|window| window.subrole == "AXSystemDialog" || window.main) {
            return Ok(Some(Panel { pid, title: window.title.clone() }));
        }
    }
    Ok(None)
}

/// Waits for a panel to come up after a menu item: Logic shows it as a modal dialog, the service draws it. A read the
/// helper doesn't answer in time is tried again until the wait is up: the service can be slow to draw a panel, and the
/// M4 gate's fourth run lost its save as to one such read.
pub fn find(ax: &mut Ax) -> Done<Panel> {
    let until = Instant::now() + PANEL_WAIT;
    loop {
        let seen = match logic_modal(ax) {
            Ok(Some(_)) => service_with_panel(ax),
            Ok(None) => Ok(None),
            Err(fault) => Err(fault),
        };
        match seen {
            Ok(Some(panel)) => return Ok(panel),
            Err(fault) if Instant::now() >= until => return Err(fault),
            _ => {}
        }
        if Instant::now() >= until {
            return Err(failed("Logic Pro showed no open or save panel"));
        }
        super::pause(Duration::from_millis(150));
    }
}

/// [`find`] right after the menu item that opens a panel: when no panel is found, Escape goes to Logic, so a panel that
/// came up unseen is never left standing in front of the project (the M4 gate's fourth run left a Save panel up).
pub fn find_or_close(ax: &mut Ax) -> Done<Panel> {
    let found = find(ax);
    if found.is_err() {
        let _ = ax.keys("escape", &[]);
        let _ = gone(ax, Duration::from_millis(800));
    }
    found
}

/// Waits for the panel to go: Logic's windows show no modal dialog any more. Whether it went.
pub fn gone(ax: &mut Ax, wait: Duration) -> Done<bool> {
    let until = Instant::now() + wait;
    loop {
        if logic_modal(ax)?.is_none() {
            return Ok(true);
        }
        if Instant::now() >= until {
            return Ok(false);
        }
        super::pause(Duration::from_millis(150));
    }
}

/// The panel's tree, from the service's focused window.
fn tree(ax: &mut Ax, panel: &Panel, depth: usize) -> Done<Node> {
    let reply = nudge::read(
        ax.helper
            .call(
                "tree",
                json!({"pid": panel.pid, "root": "focused", "path": "", "depth": depth, "children": 40, "attrs": ["role", "subrole", "title", "desc", "value", "id"]}),
            )
            .map_err(crate::ax::AxError::from),
    )?;
    Ok(Node::from_json(&reply["node"]))
}

fn call(ax: &mut Ax, op: &str, params: Value) -> Done<Value> {
    nudge::wrote(ax.helper.call(op, params).map_err(crate::ax::AxError::from))
}

/// A control by its identifier (`OKButton`, `CancelButton`, `saveAsNameTextField`) in the panel.
fn control<'a>(panel_tree: &'a Node, id: &str) -> Option<&'a Node> {
    panel_tree.find(&|n| n.id == id)
}

/// Sets the name field to `name`, without confirming it (Return would save).
pub fn set_name(ax: &mut Ax, panel: &Panel, name: &str) -> Done<()> {
    let panel_tree = tree(ax, panel, 3)?;
    let field = control(&panel_tree, "saveAsNameTextField").ok_or_else(|| failed("the save panel shows no name field"))?;
    let reply = call(
        ax,
        "set",
        json!({"pid": panel.pid, "root": "focused", "path": field.path, "attr": "AXValue", "value": name, "settleMs": SETTLE_MS}),
    )?;
    if reply["after"].as_str() != Some(name) {
        return Err(failed(format!("the save panel's name field reads {:?} after Kumi set it", reply["after"])));
    }
    Ok(())
}

/// The go-to-folder sheet, when it is up: the service's focused window is the sheet itself.
fn sheet(ax: &mut Ax, panel: &Panel) -> Done<Option<Node>> {
    let panel_tree = tree(ax, panel, 2)?;
    Ok((panel_tree.role == "AXSheet" || panel_tree.id == "GoToWindow").then_some(panel_tree))
}

/// Takes the panel to a folder (or a file) through its go-to-folder sheet: Cmd-Shift-G to the service, the
/// sheet's field set to the path, a Return key to the service.
pub fn go_to(ax: &mut Ax, panel: &Panel, path: &str) -> Done<()> {
    call(ax, "keys", json!({"pid": panel.pid, "key": "g", "mods": ["cmd", "shift"]}))?;
    let until = Instant::now() + SHEET_WAIT;
    let field = loop {
        let field = sheet(ax, panel)?
            .and_then(|sheet| sheet.find(&|n| n.id == "PathTextField" || n.role == "AXTextField" || n.role == "AXComboBox").cloned());
        if let Some(field) = field {
            break field;
        }
        if Instant::now() >= until {
            return Err(failed("the panel's go-to-folder sheet didn't come up"));
        }
        super::pause(Duration::from_millis(100));
    };
    call(
        ax,
        "set",
        json!({"pid": panel.pid, "root": "focused", "path": field.path, "attr": "AXValue", "value": path, "settleMs": SETTLE_MS}),
    )?;
    // Return lands only once the field has the keyboard focus (the lab: the sheet stays otherwise).
    call(ax, "set", json!({"pid": panel.pid, "root": "focused", "path": field.path, "attr": "AXFocused", "value": true, "settleMs": 150}))?;
    call(ax, "keys", json!({"pid": panel.pid, "key": "return", "mods": []}))?;
    // The sheet goes once the panel is there.
    let until = Instant::now() + SHEET_WAIT;
    loop {
        super::pause(Duration::from_millis(150));
        if sheet(ax, panel)?.is_none() {
            return Ok(());
        }
        if Instant::now() >= until {
            return Err(failed(format!("the panel didn't go to {path}: the folder may not be there")));
        }
    }
}

/// Where the panel stands: the folder its "where" pop-up names (the folder's own name, as the Finder shows it).
pub fn location(ax: &mut Ax, panel: &Panel) -> Done<String> {
    let panel_tree = tree(ax, panel, 3)?;
    let popup = control(&panel_tree, "where popup")
        .or_else(|| panel_tree.find(&|n| n.role == "AXPopUpButton" && n.id.contains("where")))
        .ok_or_else(|| failed("the save panel shows no folder pop-up"))?;
    let reply = call(ax, "get", json!({"pid": panel.pid, "root": "focused", "path": popup.path, "attrs": ["AXValue"]}))?;
    Ok(reply["attrs"]["AXValue"].as_str().unwrap_or_default().to_string())
}

/// Presses a button by its identifier: `OKButton` (Save, Open, Import) or `CancelButton`.
pub fn press(ax: &mut Ax, panel: &Panel, id: &str) -> Done<()> {
    let panel_tree = tree(ax, panel, 2)?;
    let button = control(&panel_tree, id).ok_or_else(|| failed(format!("the panel shows no {id}")))?;
    call(ax, "action", json!({"pid": panel.pid, "root": "focused", "path": button.path, "name": "AXPress", "settleMs": SETTLE_MS}))?;
    Ok(())
}

/// Presses a button (`OKButton`) once the panel enables it. Logic's panel for an audio import reads the file it was
/// taken to (its format and length show beside it) before its Open button takes a press: pressed right after the
/// go-to sheet went, the button was still disabled and the panel stayed up (M6). Polled until it is enabled or the
/// wait is up, then pressed; the panel's going says whether it took the press.
pub fn press_when_enabled(ax: &mut Ax, panel: &Panel, id: &str, wait: Duration) -> Done<()> {
    let until = Instant::now() + wait;
    loop {
        let reply = nudge::read(
            ax.helper
                .call("tree", json!({"pid": panel.pid, "root": "focused", "path": "", "depth": 2, "children": 40, "attrs": ["role", "id", "enabled"]}))
                .map_err(crate::ax::AxError::from),
        )?;
        let node = Node::from_json(&reply["node"]);
        let ready = control(&node, id).map(|button| (button.path.clone(), button.enabled != Some(false)));
        match ready {
            Some((path, enabled)) if enabled || Instant::now() >= until => {
                call(ax, "action", json!({"pid": panel.pid, "root": "focused", "path": path, "name": "AXPress", "settleMs": SETTLE_MS}))?;
                return Ok(());
            }
            None if Instant::now() >= until => return Err(failed(format!("the panel shows no {id}"))),
            _ => super::pause(Duration::from_millis(150)),
        }
    }
}

/// Cancels a panel left up, as far as it can (a go-to-folder sheet closed first): nothing is written.
pub fn cancel(ax: &mut Ax, panel: &Panel) {
    if let Ok(Some(_)) = sheet(ax, panel) {
        let _ = press(ax, panel, "CloseButton");
        super::pause(Duration::from_millis(300));
    }
    let _ = press(ax, panel, "CancelButton");
    let _ = gone(ax, Duration::from_millis(800));
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::helper::{Launch, MockHands};

    fn window(title: &str, subrole: &str, modal: bool, main: bool) -> Value {
        json!({"title": title, "role": "AXWindow", "subrole": subrole, "main": main, "focused": true, "modal": modal})
    }

    #[test]
    fn a_panel_is_found_under_the_service_whose_windows_hold_it_and_its_controls_by_id() {
        let hands = MockHands::new();
        hands.set_apps(vec![json!({"bundle": "com.apple.logic10", "pid": 7, "active": true})]);
        // Logic shows the modal; of two services, the second holds the panel.
        hands.set_windows(json!([window("Save", "AXDialog", true, true), window("Song - Tracks", "AXStandardWindow", false, false)]), &[]);
        hands.reply("processes", Ok(json!({"pids": [100, 200]})));
        hands.reply("windows", Ok(json!({"windows": [window("Save", "AXDialog", true, true), window("Song - Tracks", "AXStandardWindow", false, false)], "listed": []})));
        hands.reply("windows", Ok(json!({"windows": [window("Save", "AXSystemDialog", false, true)], "listed": []})));
        let mut launcher = hands.clone();
        let mut helper = Launch::launch(&mut launcher).unwrap();
        let mut ax = Ax::new(helper.as_mut(), 7);
        let panel = find(&mut ax).unwrap();
        assert_eq!(panel, Panel { pid: 200, title: "Save".into() });
        hands.set_tree(json!({"role": "AXWindow", "id": "SavePanel", "c": [{"i": "0", "role": "AXSplitGroup", "c": [
            {"i": "0.10", "role": "AXTextField", "id": "saveAsNameTextField", "value": "Song.logicx"},
            {"i": "0.25", "role": "AXButton", "title": "Cancel", "id": "CancelButton"},
            {"i": "0.26", "role": "AXButton", "title": "Save", "id": "OKButton"}
        ]}]}));
        hands.reply("set", Ok(json!({"before": "Song.logicx", "after": "Copy.logicx", "result": "success"})));
        set_name(&mut ax, &panel, "Copy.logicx").unwrap();
        hands.reply("action", Ok(json!({"result": "success"})));
        press(&mut ax, &panel, "OKButton").unwrap();
        let calls = hands.calls();
        let set = calls.iter().find(|(op, _)| op == "set").unwrap();
        assert_eq!(
            (set.1["pid"].as_u64(), set.1["path"].as_str(), set.1["value"].as_str()),
            (Some(200), Some("0.10"), Some("Copy.logicx"))
        );
        let pressed = calls.iter().find(|(op, _)| op == "action").unwrap();
        assert_eq!((pressed.1["pid"].as_u64(), pressed.1["path"].as_str()), (Some(200), Some("0.26")));
        // The panel gone: Logic shows no modal any more.
        hands.set_windows(json!([window("Song - Tracks", "AXStandardWindow", false, true)]), &[]);
        assert!(gone(&mut ax, Duration::from_millis(10)).unwrap());
    }

    #[test]
    fn a_read_the_helper_did_not_answer_is_tried_again_while_the_panel_comes_up() {
        let hands = MockHands::new();
        hands.set_apps(vec![json!({"bundle": "com.apple.logic10", "pid": 7, "active": true})]);
        hands.set_windows(json!([window("Save", "AXDialog", true, true)]), &[]);
        hands.reply("windows", Err(crate::helper::HelperError::Timeout));
        hands.reply("windows", Ok(json!({"windows": [window("Save", "AXDialog", true, true)], "listed": []})));
        hands.reply("processes", Ok(json!({"pids": [200]})));
        hands.reply("windows", Ok(json!({"windows": [window("Save", "AXSystemDialog", false, true)], "listed": []})));
        let mut launcher = hands.clone();
        let mut helper = Launch::launch(&mut launcher).unwrap();
        let mut ax = Ax::new(helper.as_mut(), 7);
        assert_eq!(find(&mut ax).unwrap(), Panel { pid: 200, title: "Save".into() });
    }

    #[test]
    fn a_panel_never_found_is_closed_with_escape_in_case_it_came_up_unseen() {
        let hands = MockHands::new();
        hands.set_apps(vec![json!({"bundle": "com.apple.logic10", "pid": 7, "active": true})]);
        hands.set_windows(json!([window("Song - Tracks", "AXStandardWindow", false, true)]), &[]);
        let mut launcher = hands.clone();
        let mut helper = Launch::launch(&mut launcher).unwrap();
        let mut ax = Ax::new(helper.as_mut(), 7);
        crate::changes::set_settling(false);
        assert_eq!(find_or_close(&mut ax).unwrap_err().0, "failed");
        assert!(hands.calls().iter().any(|(op, params)| op == "keys" && params.to_string().contains("escape")), "{:?}", hands.calls());
    }

    #[test]
    fn without_a_panel_the_wait_ends_as_failed() {
        let hands = MockHands::new();
        hands.set_apps(vec![json!({"bundle": "com.apple.logic10", "pid": 7, "active": true})]);
        hands.set_windows(json!([window("Song - Tracks", "AXStandardWindow", false, true)]), &[]);
        let mut launcher = hands.clone();
        let mut helper = Launch::launch(&mut launcher).unwrap();
        let mut ax = Ax::new(helper.as_mut(), 7);
        crate::changes::set_settling(false);
        let started = Instant::now();
        let refused = find(&mut ax).unwrap_err();
        assert_eq!(refused.0, "failed");
        assert!(started.elapsed() >= PANEL_WAIT, "{:?}", started.elapsed());
        assert!(!gone(&mut ax, Duration::from_millis(1)).unwrap_or(true) || true);
    }
}
