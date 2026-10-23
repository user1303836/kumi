//! The scripts tool against a fake DAW (no DAW): a chunk that ran is kept only once the producer said yes (Kumi's copy and the
//! DAW's action), listed, run again by name and removed whole; what would stop it working next time keeps it from being kept.
use super::backend::{Installed, ScriptsBackend, Words};
use super::store::Saved;
use super::tool::Scripts;
use async_trait::async_trait;
use kumi_common::abort::Signal;
use kumi_types::contracts::KernelTool;
use serde_json::{json, Value};
use std::cell::{Cell, RefCell};
use std::rc::Rc;

static WORDS: Words = Words {
    app: "Fake",
    language: "Lua",
    action: "an entry in the Fake's menu",
    places: &[(r"track\([0-9]+\)", "takes a track by its place")],
};

#[derive(Default)]
struct Fake {
    ran: RefCell<Vec<String>>,
    installed: RefCell<Vec<(String, bool)>>,
    uninstalled: RefCell<Vec<String>>,
    runs: RefCell<Vec<String>>,
    refuse_install: Cell<bool>,
}

#[async_trait(?Send)]
impl ScriptsBackend for Rc<Fake> {
    fn words(&self) -> &'static Words {
        &WORDS
    }

    fn ran_cleanly(&self, code: &str) -> bool {
        self.ran.borrow().iter().any(|ran| ran == code)
    }

    async fn install(&self, saved: &Saved, replace: bool, _signal: &Signal) -> Result<Installed, String> {
        if self.refuse_install.get() {
            return Err("the Fake wouldn't take it as an action".into());
        }
        self.installed.borrow_mut().push((saved.slug.clone(), replace));
        Ok(Installed { action: format!("Menu > Kumi > {}", saved.name), file: format!("/fake/{}.lua", saved.slug) })
    }

    async fn uninstall(&self, slug: &str, _signal: &Signal) -> Result<(), String> {
        self.uninstalled.borrow_mut().push(slug.to_string());
        Ok(())
    }

    async fn run(&self, code: &str, _signal: &Signal) -> Result<Value, String> {
        self.runs.borrow_mut().push(code.to_string());
        Ok(json!({"changed": "Ran Lua", "change": "c1"}))
    }
}

async fn call(tool: &Rc<dyn KernelTool>, input: Value) -> Result<Value, String> {
    let done = tool.execute(input.as_object().cloned().unwrap(), Signal::new()).await.unwrap();
    if done.is_error {
        return Err(done.text);
    }
    Ok(serde_json::from_str(&done.text).unwrap())
}

const CODE: &str = "colour(track(2))";

#[tokio::test(flavor = "current_thread")]
async fn a_chunk_that_ran_is_kept_once_the_producer_said_yes_run_by_name_and_removed_whole() {
    let dir = tempfile::tempdir().unwrap();
    let fake = Rc::new(Fake::default());
    let tool = Scripts::new(fake.clone(), dir.path().join("fake")).tool();
    let save = json!({"action": "save", "name": "Colour the drums", "description": "Drums red", "code": CODE, "confirmed": true});
    let mut unasked = save.clone();
    unasked["confirmed"] = false.into();
    assert!(call(&tool, unasked).await.unwrap_err().contains("ask them first"));
    let never = call(&tool, save.clone()).await.unwrap_err();
    assert!(never.starts_with("not kept: it hasn't run in this session"), "{never}");
    assert!(fake.installed.borrow().is_empty(), "nothing installed");

    fake.ran.borrow_mut().push(CODE.into());
    let lint = call(&tool, json!({"action": "lint", "code": CODE})).await.unwrap();
    assert_eq!(lint["findings"], json!([{"blocks": false, "says": "takes a track by its place"}]));
    let kept = call(&tool, save.clone()).await.unwrap();
    assert_eq!(kept["action"], "Menu > Kumi > Colour the drums");
    assert_eq!(kept["notes"], json!(["takes a track by its place"]));
    assert_eq!(*fake.installed.borrow(), [("colour-the-drums".to_string(), false)]);
    let listed = call(&tool, json!({})).await.unwrap();
    assert_eq!(listed["count"], 1);
    assert_eq!(listed["scripts"][0]["name"], "Colour the drums");
    assert_eq!(listed["scripts"][0]["action"], "Menu > Kumi > Colour the drums");
    // Run again by its name, as one change.
    let ran = call(&tool, json!({"action": "run", "name": "colour the drums"})).await.unwrap();
    assert_eq!(ran["answer"]["change"], "c1");
    assert_eq!(*fake.runs.borrow(), [CODE]);
    assert!(call(&tool, json!({"action": "run", "name": "Tidy hats"})).await.unwrap_err().contains("no script named"));
    // Kept already: written over only when asked.
    assert!(call(&tool, save.clone()).await.unwrap_err().contains("kept already"));
    let mut again = save.clone();
    again["replace"] = true.into();
    call(&tool, again).await.unwrap();
    assert_eq!(fake.installed.borrow()[1], ("colour-the-drums".to_string(), true));
    // Removed only when the producer says, its action too.
    assert!(call(&tool, json!({"action": "remove", "name": "Colour the drums"})).await.unwrap_err().contains("confirmed"));
    call(&tool, json!({"action": "remove", "name": "Colour the drums", "confirmed": true})).await.unwrap();
    assert_eq!(*fake.uninstalled.borrow(), ["colour-the-drums"]);
    assert_eq!(call(&tool, json!({})).await.unwrap()["count"], 0);
}

#[tokio::test(flavor = "current_thread")]
async fn ids_from_this_project_block_unless_its_for_this_project_and_a_refused_install_keeps_nothing() {
    let dir = tempfile::tempdir().unwrap();
    let fake = Rc::new(Fake::default());
    let tool = Scripts::new(fake.clone(), dir.path().join("fake")).tool();
    let code = "local t = by_id('{0AF3C7B1-1E0A-4C59-9F0B-0D3F5C1B2A99}')";
    fake.ran.borrow_mut().push(code.into());
    let save = json!({"action": "save", "name": "Mute the click", "code": code, "confirmed": true});
    assert!(call(&tool, save.clone()).await.unwrap_err().contains("by its id in this project"));
    let mut here = save.clone();
    here["thisProject"] = true.into();
    fake.refuse_install.set(true);
    assert!(call(&tool, here.clone()).await.unwrap_err().contains("wouldn't take it"));
    assert_eq!(call(&tool, json!({})).await.unwrap()["count"], 0, "nothing kept when the DAW refused");
    fake.refuse_install.set(false);
    call(&tool, here).await.unwrap();
    let listed = call(&tool, json!({})).await.unwrap();
    assert_eq!(listed["scripts"][0]["description"], "(for this project only)");
    // Kept for Kumi alone: no action.
    fake.ran.borrow_mut().push("return 1".into());
    let alone = call(&tool, json!({"action": "save", "name": "One", "code": "return 1", "confirmed": true, "inDaw": false})).await.unwrap();
    assert!(alone.get("action").is_none());
    assert_eq!(fake.installed.borrow().len(), 1);
}
