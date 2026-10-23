//! `read {kind: "recent"}`: the projects Logic opened lately, as its File > Open Recent submenu lists them, most recent
//! first. Accessibility reads the submenu's items without opening the menu (each is `_openRecentDocument:` by its
//! identifier, titled with the project's file name), and no item says where its project is: an item has its title, its
//! identifier and its frame and nothing else (every attribute read on Logic Pro 12.3.1), Logic keeps the list in
//! macOS's shared file list, which macOS keeps from other processes, and its preferences hold no project path. So the
//! open project can't be matched by its whole path: it is the first item of its file name (the list starts with the
//! latest, and opening a project puts it there), with the path of its window's document, and another item of that
//! name, from another folder, isn't it. A row's `path` is otherwise known when Spotlight finds exactly one project of
//! that file name where Kumi may look besides the open one (never under `~/Music`, which macOS keeps from a process
//! without the Files and Folders permission); else it is `null`.

use std::path::Path;

use serde_json::{json, Value};

use crate::ax::{Ax, AxError, Root};
use crate::project::Project;

/// The identifiers AppKit gives the recent documents' items and the item that clears them.
const OPEN_RECENT_ITEM: &str = "_openRecentDocument:";
const CLEAR_ITEM: &str = "clearRecentDocuments:";

/// A project's name as a producer says it: its file name without the package's extension.
pub fn name_of(file: &str) -> String {
    for extension in [".logicx", ".logic"] {
        if let Some(stem) = file.strip_suffix(extension) {
            if !stem.is_empty() {
                return stem.to_string();
            }
        }
    }
    file.to_string()
}

/// The recent projects' file names, in the submenu's order.
pub fn titles(project: &Project, ax: &mut Ax) -> Result<Vec<String>, AxError> {
    let bar = ax.tree(&Root::MenuBar, "", 1, 20, Some(&["role", "title"]))?;
    let file = bar
        .children
        .iter()
        .find(|item| item.role == "AXMenuBarItem" && project.strings.is(&item.title, "File"))
        .ok_or_else(|| AxError::Missing("its File menu".into()))?;
    let menu = ax.tree(&Root::MenuBar, &format!("{}.0", file.path), 1, 60, Some(&["role", "title"]))?;
    let item = menu
        .children
        .iter()
        .find(|item| item.role == "AXMenuItem" && project.strings.is(&item.title, "Open Recent"))
        .ok_or_else(|| AxError::Missing("its File > Open Recent menu".into()))?;
    let submenu = ax.tree(&Root::MenuBar, &format!("{}.0", item.path), 1, 60, Some(&["role", "title", "id"]))?;
    if !submenu.children.iter().any(|item| item.id == OPEN_RECENT_ITEM || item.id == CLEAR_ITEM) {
        return Err(AxError::Missing("the recent projects in its File > Open Recent menu".into()));
    }
    Ok(submenu
        .children
        .iter()
        .filter(|item| item.role == "AXMenuItem" && item.id == OPEN_RECENT_ITEM && !item.title.is_empty())
        .map(|item| item.title.clone())
        .collect())
}

/// The rows: `{index, name, path, open}` for each recent project. The open project is the first item of its file name,
/// with its path from its window; any other item has the path Spotlight finds when it finds one project of the name
/// besides the open one and no other item but the open one has that name.
pub fn rows(project: &Project, ax: &mut Ax) -> Result<Vec<Value>, AxError> {
    let titles = titles(project, ax)?;
    let open_path = project.path.as_deref().map(|path| path.trim_end_matches('/'));
    let open_file = open_path.and_then(|path| Path::new(path).file_name()).map(|n| n.to_string_lossy().into_owned());
    let open_at = titles.iter().position(|title| open_file.as_deref() == Some(title.as_str()));
    // Spotlight is asked about each name of the other items once: the open project's too when another item has it.
    let mut asked: Vec<String> = Vec::new();
    for (at, title) in titles.iter().enumerate() {
        if Some(at) != open_at && !asked.contains(title) {
            asked.push(title.clone());
        }
    }
    let found = crate::disk::projects_named(&asked);
    Ok(titles
        .iter()
        .enumerate()
        .map(|(at, title)| {
            let open = Some(at) == open_at;
            // Two items of a name that aren't the open project can't be told apart: neither has a path.
            let alone = titles.iter().enumerate().filter(|(other, name)| *name == title && Some(*other) != open_at).count() == 1;
            let path = if open {
                project.path.clone()
            } else if !alone {
                None
            } else {
                let elsewhere: Vec<&String> = found
                    .iter()
                    .filter(|(name, _)| name == title)
                    .flat_map(|(_, paths)| paths)
                    .filter(|path| Some(path.trim_end_matches('/')) != open_path)
                    .collect();
                (elsewhere.len() == 1).then(|| elsewhere[0].clone())
            };
            json!({"index": at + 1, "name": name_of(title), "path": path, "open": open})
        })
        .collect())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ax::strings::Strings;
    use crate::helper::{Launch, MockHands};

    /// The rows read from an Open Recent submenu of these titles, with the open project at `open` and Spotlight asked
    /// `asked` (the names, one a line) and answering `found`; and the requests the read made.
    fn read_rows(titles: &[&str], open: &str, asked: &str, found: Value) -> (Vec<Value>, Vec<(String, Value)>) {
        let hands = MockHands::new();
        hands.reply(
            "tree",
            Ok(json!({"node": {"role": "AXMenuBar", "c": [
                {"i": "0", "role": "AXMenuBarItem", "title": "Apple"}, {"i": "1", "role": "AXMenuBarItem", "title": "Logic Pro"},
                {"i": "2", "role": "AXMenuBarItem", "title": "File"}
            ]}})),
        );
        hands.reply(
            "tree",
            Ok(json!({"node": {"role": "AXMenu", "i": "2.0", "c": [
                {"i": "2.0.0", "role": "AXMenuItem", "title": "New"}, {"i": "2.0.3", "role": "AXMenuItem", "title": "Open Recent"}
            ]}})),
        );
        let mut items: Vec<Value> = titles
            .iter()
            .enumerate()
            .map(|(at, title)| json!({"i": format!("2.0.3.0.{at}"), "role": "AXMenuItem", "title": title, "id": OPEN_RECENT_ITEM}))
            .collect();
        items.push(json!({"i": format!("2.0.3.0.{}", titles.len()), "role": "AXMenuItem"}));
        items.push(json!({"i": format!("2.0.3.0.{}", titles.len() + 1), "role": "AXMenuItem", "title": "Clear Menu", "id": CLEAR_ITEM}));
        hands.reply("tree", Ok(json!({"node": {"role": "AXMenu", "i": "2.0.3.0", "c": items}})));
        let mut launcher = hands.clone();
        let mut helper = Launch::launch(&mut launcher).unwrap();
        let mut ax = Ax::new(helper.as_mut(), 7);
        let mut project = Project::new(Strings::english());
        project.set_identity(Some("id"), Some(open), &name_of(Path::new(open).file_name().unwrap().to_str().unwrap()));
        crate::disk::set_hook(Some(crate::disk::replaying(&[json!({"kind": "spotlight", "path": asked, "answer": found})])));
        let rows = rows(&project, &mut ax).unwrap();
        crate::disk::set_hook(None);
        (rows, hands.calls())
    }

    #[test]
    fn the_recent_projects_are_the_open_recent_items_with_a_path_where_one_is_known() {
        assert_eq!(name_of("Scratch.logicx"), "Scratch");
        assert_eq!(name_of("Old Song.logic"), "Old Song");
        assert_eq!(name_of("Song"), "Song");
        let (rows, ops) = read_rows(
            &["Scratch.logicx", "Demo.logicx", "Twice.logicx"],
            "/Users/me/Music/Logic/Scratch.logicx",
            "Demo.logicx\nTwice.logicx",
            json!({"Demo.logicx": ["/Users/me/Projects/Demo.logicx"], "Twice.logicx": ["/a/Twice.logicx", "/b/Twice.logicx"]}),
        );
        assert_eq!(
            rows,
            vec![
                json!({"index": 1, "name": "Scratch", "path": "/Users/me/Music/Logic/Scratch.logicx", "open": true}),
                json!({"index": 2, "name": "Demo", "path": "/Users/me/Projects/Demo.logicx", "open": false}),
                json!({"index": 3, "name": "Twice", "path": null, "open": false}),
            ]
        );
        assert!(ops.iter().all(|(op, params)| op == "tree" && params["root"] == "menubar"), "reads of the menu bar only: {ops:?}");
    }

    #[test]
    fn of_two_items_with_the_open_projects_file_name_only_the_first_is_the_open_one() {
        // No item says where its project is, so the first of the open project's file name is it (the list starts with the
        // latest); the other, a project of that name in another folder, has the path Spotlight finds for it.
        let (rows, _) = read_rows(
            &["Scratch.logicx", "Demo.logicx", "Scratch.logicx"],
            "/Users/me/Music/Logic/Scratch.logicx",
            "Demo.logicx\nScratch.logicx",
            json!({"Demo.logicx": ["/Users/me/Projects/Demo.logicx"], "Scratch.logicx": ["/Users/me/Projects/Old/Scratch.logicx"]}),
        );
        assert_eq!(
            rows,
            vec![
                json!({"index": 1, "name": "Scratch", "path": "/Users/me/Music/Logic/Scratch.logicx", "open": true}),
                json!({"index": 2, "name": "Demo", "path": "/Users/me/Projects/Demo.logicx", "open": false}),
                json!({"index": 3, "name": "Scratch", "path": "/Users/me/Projects/Old/Scratch.logicx", "open": false}),
            ]
        );
        // Spotlight finds the open project too when it is where Kumi may look: the other item's path is the other one.
        let (rows, _) = read_rows(
            &["Song.logicx", "Song.logicx"],
            "/Users/me/Projects/Song.logicx",
            "Song.logicx",
            json!({"Song.logicx": ["/Users/me/Projects/Song.logicx", "/Users/me/Projects/2025/Song.logicx"]}),
        );
        let shown: Vec<(&Value, &Value)> = rows.iter().map(|row| (&row["open"], &row["path"])).collect();
        assert_eq!(
            shown,
            [(&json!(true), &json!("/Users/me/Projects/Song.logicx")), (&json!(false), &json!("/Users/me/Projects/2025/Song.logicx"))]
        );
        // Three of the name: the two not open can't be told apart, so neither has a path.
        let (rows, _) = read_rows(
            &["Song.logicx", "Song.logicx", "Song.logicx"],
            "/Users/me/Projects/Song.logicx",
            "Song.logicx",
            json!({"Song.logicx": ["/Users/me/Projects/Song.logicx", "/a/Song.logicx", "/b/Song.logicx"]}),
        );
        assert_eq!(rows.iter().filter(|row| row["open"] == true).count(), 1);
        assert!(rows[1..].iter().all(|row| row["path"].is_null()), "{rows:?}");
        // Nor do two of another name when Spotlight finds one project of it: the other may be where it can't look.
        let (rows, _) = read_rows(
            &["Scratch.logicx", "Demo.logicx", "Demo.logicx"],
            "/Users/me/Music/Logic/Scratch.logicx",
            "Demo.logicx",
            json!({"Demo.logicx": ["/Users/me/Projects/Demo.logicx"]}),
        );
        assert!(rows[1..].iter().all(|row| row["path"].is_null() && row["open"] == false), "{rows:?}");
    }
}
