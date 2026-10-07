#[path = "support/library_fixture_paths.rs"]
mod library_fixture_paths;
use async_trait::async_trait;
use kumi_common::abort::Signal;
use kumi_runtime::{
    core::errors::RuntimeError,
    library::{
        features::MeasureOptions,
        learn::{PresetEntry, SetEntry, SoundEntry},
        search::SoundIndex,
        sources::Source,
        tools::{library_tools, LearningState, LibraryAccess, LibraryToolsOptions},
    },
};
use serde_json::{json, Value};
use std::{cell::RefCell, rc::Rc};
struct Access {
    index: Rc<SoundIndex>,
    presets: Vec<PresetEntry>,
    sets: Vec<SetEntry>,
    state: RefCell<LearningState>,
    root: String,
    remembered: RefCell<Vec<String>>,
}
#[async_trait(?Send)]
impl LibraryAccess for Access {
    async fn sounds(&self) -> Result<Rc<SoundIndex>, RuntimeError> {
        Ok(self.index.clone())
    }
    async fn presets(&self) -> Result<Vec<PresetEntry>, RuntimeError> {
        Ok(self.presets.clone())
    }
    async fn sets(&self) -> Result<Vec<SetEntry>, RuntimeError> {
        Ok(self.sets.clone())
    }
    fn learning(&self) -> LearningState {
        self.state.borrow().clone()
    }
    fn remember(&self, folders: Vec<String>) {
        self.remembered.borrow_mut().extend(folders);
    }
    async fn folders(&self) -> Vec<String> {
        vec![self.root.clone()]
    }
    async fn measure(&self, _: String, _: MeasureOptions) -> Result<SoundEntry, RuntimeError> {
        Err(RuntimeError::plain("unreadable."))
    }
}
#[tokio::test]
async fn tool_results_and_events_match_typescript() {
    let root = tempfile::tempdir().unwrap();
    std::fs::create_dir(root.path().join("Nested")).unwrap();
    let root_text = root.path().to_str().unwrap();
    let fixture = library_fixture_paths::load(include_str!("support/library-search-oracle.json"), root.path());
    let entries: Vec<SoundEntry> = serde_json::from_value(fixture["entries"].clone()).unwrap();
    let sources: Vec<Source> = serde_json::from_value(fixture["sources"].clone()).unwrap();
    let access = Rc::new(Access {
        index: Rc::new(SoundIndex::new(entries, &sources)),
        presets: serde_json::from_value(fixture["presets"].clone()).unwrap(),
        sets: serde_json::from_value(fixture["sets"].clone()).unwrap(),
        state: RefCell::new(LearningState::default()),
        root: root_text.into(),
        remembered: RefCell::new(vec![]),
    });
    let events = Rc::new(RefCell::new(vec![]));
    let received = events.clone();
    let tools = library_tools(
        access.clone(),
        LibraryToolsOptions {
            on_event: Some(Rc::new(move |e| received.borrow_mut().push(serde_json::to_value(e).unwrap()))),
            ..Default::default()
        },
    );
    let cases = library_fixture_paths::load(include_str!("support/library-tools-oracle.json"), root.path());
    for case in cases.as_array().unwrap() {
        let state = &case["state"];
        *access.state.borrow_mut() = LearningState {
            learning: state["learning"].as_bool().unwrap(),
            first: state["first"].as_bool().unwrap(),
            sounds: state["sounds"].as_u64().unwrap() as usize,
            todo: state["todo"].as_u64().map(|n| n as usize),
            done: state["done"].as_u64().map(|n| n as usize),
        };
        events.borrow_mut().clear();
        let tool = tools.iter().find(|t| t.name() == case["name"]).unwrap();
        let result = tool.execute(case["input"].as_object().unwrap().clone(), Signal::new()).await.unwrap();
        let actual = serde_json::from_str::<Value>(&result.text).unwrap_or(json!(result.text));
        assert_eq!(actual, case["expected"], "{} {}", case["name"], case["input"]);
        assert_eq!(result.is_error, case["isError"].as_bool().unwrap());
        assert_eq!(*events.borrow(), *case["events"].as_array().unwrap());
    }
    // An unlearned folder is scanned by names and remembered for the next run.
    let fresh = root.path().join("Fresh");
    std::fs::create_dir(&fresh).unwrap();
    std::fs::write(fresh.join("New Kick.wav"), vec![0; 70]).unwrap();
    let result = tools[0].execute(json!({"folders":[fresh],"words":["kick"]}).as_object().unwrap().clone(), Signal::new()).await.unwrap();
    let row: Value = serde_json::from_str(&result.text).unwrap();
    assert_eq!(row["sounds"][0]["name"], "New Kick");
    assert!(row["note"].as_str().unwrap().contains("hasn't learned that folder yet"));
    assert_eq!(*access.remembered.borrow(), vec![fresh.to_string_lossy()]);
}
