//! A session's palettes: files gathered make one when there's none, /palette names, lists and chooses them, a sound that
//! moved is found again by its name and size, a loose one brings each sound's nearest (taste first), Kumi's turn is told
//! of it, and the palette tool works with it.
use async_trait::async_trait;
use kumi_agent::core::{
    palette::{Gathered, PaletteLibrary, PaletteView, Palettes},
    palette_tool::PaletteTool,
    store_client::StoreClient,
};
use kumi_common::abort::Signal;
use kumi_library::{
    learn::SoundEntry,
    search::SoundIndex,
    sources::{Source, SourceKind},
    store::{pack_vector, Entry},
    SoundClass,
};
use kumi_store::Store;
use serde_json::{json, Value};
use std::{cell::RefCell, collections::HashMap, rc::Rc};

macro_rules! case {
    ($name:ident, $body:expr) => {
        #[tokio::test(flavor = "current_thread")]
        async fn $name() {
            tokio::task::LocalSet::new().run_until($body).await;
        }
    };
}

/// A sound file of `bytes` bytes in `folder`, and what the map gathers of it.
fn sound(folder: &std::path::Path, name: &str, bytes: usize) -> Gathered {
    let path = folder.join(format!("{name}.wav"));
    std::fs::write(&path, vec![0u8; bytes]).unwrap();
    Gathered { path: path.to_string_lossy().into(), name: name.into(), song: false, class: Some("kick".into()) }
}

/// A library for palettes: where moved sounds went, its sounds, and how many projects use each.
#[derive(Default)]
struct Library {
    asked: RefCell<Vec<(String, i64)>>,
    moved: Option<(String, i64)>,
    index: Option<Rc<SoundIndex>>,
    used: HashMap<String, usize>,
}

#[async_trait(?Send)]
impl PaletteLibrary for Library {
    async fn refind(&self, wanted: Vec<(String, i64)>) -> Vec<Option<(String, i64)>> {
        self.asked.borrow_mut().extend(wanted.iter().cloned());
        wanted.iter().map(|(name, size)| self.moved.clone().filter(|(to, _)| to.ends_with(name.as_str()) && *size == 10)).collect()
    }
    async fn index(&self) -> Option<Rc<SoundIndex>> {
        self.index.clone()
    }
    async fn used(&self) -> HashMap<String, usize> {
        self.used.clone()
    }
}

/// Palettes on a fresh database, with what each change told the app.
fn palettes(dir: &std::path::Path, library: Option<Rc<Library>>) -> (Palettes, Rc<RefCell<Vec<Option<PaletteView>>>>) {
    let told: Rc<RefCell<Vec<Option<PaletteView>>>> = Rc::default();
    let client = StoreClient::new(Store::open(dir.join("kumi.db")).unwrap());
    let heard = told.clone();
    let library = library.map(|library| library as Rc<dyn PaletteLibrary>);
    (Palettes::new(client, library, Rc::new(move |view| heard.borrow_mut().push(view))), told)
}

/// A library of `kicks` kick files and `pads` pad files in `folder` (each written, 10 bytes), the kicks alike and the
/// pads alike, each a little apart from the rest of its kind.
fn library(folder: &std::path::Path, kicks: usize, pads: usize) -> (Rc<SoundIndex>, Vec<Gathered>) {
    let mut entries = vec![];
    let mut gathered = vec![];
    for (kind, count, class) in [("Kick", kicks, SoundClass::Kick), ("Pad", pads, SoundClass::Pad)] {
        for n in 0..count {
            let name = format!("{kind} {n}");
            let path = folder.join(format!("{name}.wav"));
            std::fs::write(&path, [0u8; 10]).unwrap();
            let numbers: Vec<f32> =
                (0..40).map(|d| if (d % 2 == 0) == (kind == "Kick") { 5. } else { 0. } + d as f32 * 0.001 * n as f32).collect();
            let path = path.to_string_lossy().to_string();
            entries.push(SoundEntry {
                file: Entry { path: path.clone(), size: 10, mtime: 1, gone: None },
                vector: Some(pack_vector(&numbers)),
                r#class: Some(class),
                ..Default::default()
            });
            gathered.push(Gathered { path, name, song: false, class: Some(class.as_str().into()) });
        }
    }
    let source = Source { path: folder.to_string_lossy().into(), label: "Samples".into(), kind: SourceKind::Folder };
    (Rc::new(SoundIndex::of_every_source(entries, &[source])), gathered)
}

case!(gathering_makes_a_palette_when_the_session_has_none_and_keeps_each_file_once, async {
    let dir = tempfile::tempdir().unwrap();
    let (palettes, told) = palettes(dir.path(), None);
    assert_eq!(palettes.view().await.unwrap(), None);
    let (kick, snare) = (sound(dir.path(), "Kick 1", 10), sound(dir.path(), "Snare 1", 20));
    let gathering = palettes.gather(vec![kick.clone(), snare.clone()]).await.unwrap();
    assert_eq!(gathering.words(), "Added 2 sounds to Palette 1.");
    assert_eq!(palettes.gather(vec![kick.clone()]).await.unwrap().words(), "Palette 1 has that sound already.");
    assert_eq!(
        palettes.gather(vec![kick, sound(dir.path(), "Hat", 5)]).await.unwrap().words(),
        "Added 1 sound to Palette 1 (1 it had already)."
    );
    let view = palettes.view().await.unwrap().unwrap();
    assert_eq!((view.name.as_str(), view.reach), ("Palette 1", 0));
    assert_eq!(
        view.files.iter().map(|file| (file.name.as_str(), file.size)).collect::<Vec<_>>(),
        [("Kick 1", 10), ("Snare 1", 20), ("Hat", 5)]
    );
    assert!(view.files[0].mtime > 0, "its file's modification time");
    assert_eq!(told.borrow().last().cloned().flatten(), Some(view.clone()), "the app hears each change");
    // Taken out by its path, size and time.
    let hat = view.files[2].clone();
    assert!(palettes.remove(&hat.path, hat.size, hat.mtime).await.unwrap());
    assert!(!palettes.remove(&hat.path, hat.size, hat.mtime).await.unwrap());
    assert_eq!(palettes.view().await.unwrap().unwrap().files.len(), 2);
});

case!(palette_words_list_make_choose_rename_forget_and_set_how_far_it_reaches, async {
    let dir = tempfile::tempdir().unwrap();
    let (palettes, _) = palettes(dir.path(), None);
    assert!(palettes.command("").await.unwrap().starts_with("No palettes yet"));
    assert!(palettes.command("strict").await.unwrap().starts_with("This session has no palette yet"));
    assert_eq!(
        palettes.command("new Drums").await.unwrap(),
        "Drums is this session's palette now: pick sounds on /map and press Enter to add them."
    );
    assert!(palettes.command("new drums").await.unwrap().starts_with("There's a palette called drums already"));
    palettes.gather(vec![sound(dir.path(), "Kick", 3)]).await.unwrap();
    palettes.command("new Pads").await.unwrap();
    assert_eq!(palettes.view().await.unwrap().unwrap().name, "Pads");
    assert_eq!(
        palettes.command("list").await.unwrap(),
        "Palettes: Pads (0 sounds, this session's) · Drums (1 sound). /palette use and a name works with another."
    );
    assert_eq!(palettes.command("use DRUMS").await.unwrap(), "Drums (1 sound) is this session's palette now.");
    assert_eq!(
        palettes.command("loose").await.unwrap(),
        "Loose: each of the palette's sounds brings its 5 nearest, and Kumi says when it uses one of those."
    );
    assert_eq!(palettes.view().await.unwrap().unwrap().reach, 5);
    assert!(palettes.command("loose 99").await.unwrap().starts_with("/palette loose takes how many"));
    assert_eq!(palettes.command("loose 8").await.unwrap().split(':').next(), Some("Loose"));
    assert_eq!(palettes.view().await.unwrap().unwrap().mode(), "loose, 8 nearest");
    assert_eq!(palettes.command("strict").await.unwrap(), "Strict: Kumi keeps to the palette's own sounds.");
    assert_eq!(palettes.command("rename Kicks").await.unwrap(), "This session's palette is called Kicks now.");
    assert!(palettes.command("rename pads").await.unwrap().starts_with("There's a palette called pads already"));
    assert!(palettes.command("use Nope").await.unwrap().starts_with("There's no palette called Nope"));
    assert_eq!(palettes.command("forget kicks").await.unwrap(), "Forgot the palette Kicks and its 1 sound.");
    assert_eq!(palettes.view().await.unwrap(), None, "the session's own forgotten: none now");
    assert!(palettes.command("what").await.unwrap().starts_with("/palette lists your palettes"));
});

case!(a_files_time_is_named_as_the_library_names_it_so_a_sound_found_again_isnt_kept_twice, async {
    let dir = tempfile::tempdir().unwrap();
    let moved_to = dir.path().join("Moved").join("Kick 1.wav");
    // The library's name for the file's time: rounded to the millisecond (truncated would be ...000).
    let at = std::time::UNIX_EPOCH + std::time::Duration::new(1_700_000_000, 600_000);
    let rounded = 1_700_000_000_001;
    let library = Rc::new(Library { moved: Some((moved_to.to_string_lossy().to_string(), rounded)), ..Default::default() });
    let (palettes, _) = palettes(dir.path(), Some(library));
    let kick = sound(dir.path(), "Kick 1", 10);
    std::fs::File::options().write(true).open(&kick.path).unwrap().set_modified(at).unwrap();
    palettes.gather(vec![kick.clone()]).await.unwrap();
    assert_eq!(palettes.view().await.unwrap().unwrap().files[0].mtime, rounded, "as kumi-library's learn::mtime names it");
    // Moved, and found again by the library (its time as the library names it); picked again where it is now: once.
    std::fs::create_dir_all(moved_to.parent().unwrap()).unwrap();
    std::fs::rename(&kick.path, &moved_to).unwrap();
    assert_eq!(palettes.view().await.unwrap().unwrap().files[0].path, moved_to.to_string_lossy());
    let again = Gathered { path: moved_to.to_string_lossy().into(), ..kick };
    assert_eq!(palettes.gather(vec![again]).await.unwrap().words(), "Palette 1 has that sound already.");
    assert_eq!(palettes.view().await.unwrap().unwrap().files.len(), 1);
});

case!(a_sound_that_moved_is_found_again_by_its_name_and_size, async {
    let dir = tempfile::tempdir().unwrap();
    let moved_to = dir.path().join("Moved").join("Kick 1.wav");
    let library = Rc::new(Library { moved: Some((moved_to.to_string_lossy().to_string(), 42)), ..Default::default() });
    let (palettes, _) = palettes(dir.path(), Some(library.clone()));
    let asked = || library.asked.borrow().clone();
    let kick = sound(dir.path(), "Kick 1", 10);
    palettes.gather(vec![kick.clone(), sound(dir.path(), "Snare", 4)]).await.unwrap();
    assert!(asked().is_empty(), "nothing moved: the library isn't asked");
    // The folder renamed: the file is gone from its path, and there's one of its name and size elsewhere.
    std::fs::create_dir_all(moved_to.parent().unwrap()).unwrap();
    std::fs::rename(&kick.path, &moved_to).unwrap();
    let view = palettes.view().await.unwrap().unwrap();
    assert_eq!((view.files[0].path.as_str(), view.files[0].mtime), (moved_to.to_string_lossy().as_ref(), 42));
    assert_eq!(asked(), [("Kick 1.wav".to_string(), 10)]);
    // Kept so: read again, it's where it went, and nothing is asked again.
    assert_eq!(palettes.view().await.unwrap().unwrap().files[0].path, moved_to.to_string_lossy());
    assert_eq!(asked().len(), 1);
});

case!(a_loose_palette_brings_each_sounds_nearest_taste_first_and_none_it_has_or_excludes, async {
    let dir = tempfile::tempdir().unwrap();
    let (index, sounds) = library(dir.path(), 8, 6);
    // Kick 4 is the farthest of the three nearest Kick 0 and Kick 1 have, and two of the producer's projects use it.
    let used = HashMap::from([(sounds[4].path.clone(), 2)]);
    let library = Rc::new(Library { index: Some(index), used, ..Default::default() });
    let (palettes, told) = palettes(dir.path(), Some(library));
    palettes.gather(vec![sounds[0].clone(), sounds[1].clone()]).await.unwrap();
    assert!(palettes.reached().await.unwrap().is_empty(), "strict: nothing brought in");
    palettes.command("loose 3").await.unwrap();
    let reached = palettes.reached().await.unwrap();
    let names: Vec<&str> = reached.iter().map(|sound| sound.name.as_str()).collect();
    assert_eq!(
        names,
        ["Kick 4", "Kick 2", "Kick 3"],
        "the one the producer's projects use first, then by how many call it near and how near"
    );
    assert_eq!(reached[0].used, 2);
    assert_eq!(reached[1].near, ["Kick 0", "Kick 1"]);
    // The app hears what it brings in, to mark on the map.
    let view = told.borrow().last().cloned().flatten().unwrap();
    assert_eq!(view.reached.len(), reached.len());
    // Excluded: never brought in.
    let excluded = reached[1].path.clone();
    assert_eq!(palettes.exclude_paths(&[excluded.clone()]).await.unwrap(), 1);
    assert!(palettes.reached().await.unwrap().iter().all(|sound| sound.path != excluded));
    assert_eq!(palettes.view().await.unwrap().unwrap().excluded.len(), 1);
});

case!(kumis_turn_is_told_of_the_palette_strict_or_loose, async {
    let dir = tempfile::tempdir().unwrap();
    let (palettes, _) = palettes(dir.path(), None);
    assert_eq!(palettes.brief().await, "", "no palette: nothing");
    palettes.command("new Drums").await.unwrap();
    assert_eq!(palettes.brief().await, "", "an empty palette: nothing");
    let (kick, hat) = (sound(dir.path(), "Kick", 3), sound(dir.path(), "Hat", 4));
    palettes.gather(vec![kick, Gathered { class: Some("hat".into()), ..hat }, sound(dir.path(), "Kick 2", 5)]).await.unwrap();
    let brief = palettes.brief().await;
    assert!(brief.starts_with("\n\n[Kumi] The producer's palette, Drums: 3 sounds (2 kick, 1 hat). It's strict:"), "{brief}");
    palettes.command("loose").await.unwrap();
    let brief = palettes.brief().await;
    assert!(brief.contains("It's loose: these, and each one's 5 nearest") && brief.contains("offer to add it"), "{brief}");
});

case!(the_palette_tool_lists_searches_brings_in_adds_takes_out_and_excludes, async {
    let dir = tempfile::tempdir().unwrap();
    let (index, sounds) = library(dir.path(), 6, 4);
    let library = Rc::new(Library { index: Some(index), ..Default::default() });
    let (palettes, _) = palettes(dir.path(), Some(library));
    let palettes = Rc::new(palettes);
    let tool = PaletteTool::new(palettes.clone());
    let call = |input: Value| {
        let tool = tool.clone();
        async move {
            let result = tool.execute(input.as_object().unwrap().clone(), Signal::new()).await.unwrap();
            (serde_json::from_str::<Value>(&result.text).unwrap_or(Value::String(result.text.clone())), result.is_error)
        }
    };
    let (none, _) = call(json!({"action": "list"})).await;
    assert_eq!(none["palette"], Value::Null, "{none}");
    // Added by path: named and classed as the library has them.
    let (added, _) = call(json!({"action": "add", "paths": [sounds[0].path, sounds[1].path, sounds[6].path]})).await;
    assert_eq!(added["said"], json!("Added 3 sounds to Palette 1."));
    let (listed, _) = call(json!({"action": "list"})).await;
    assert_eq!(
        listed["sounds"].as_array().unwrap().iter().map(|s| s["name"].as_str().unwrap()).collect::<Vec<_>>(),
        ["Kick 0", "Kick 1", "Pad 0"]
    );
    assert_eq!((listed["mode"].as_str(), listed["sounds"][2]["class"].as_str()), (Some("strict"), Some("pad")));
    let (found, _) = call(json!({"action": "search", "words": "PAD"})).await;
    assert_eq!(found["sounds"][0]["name"], json!("Pad 0"));
    assert_eq!(found["sounds"].as_array().unwrap().len(), 1);
    // Strict: neighbours says so; loose, it brings sounds in.
    let (strict, _) = call(json!({"action": "neighbours"})).await;
    assert!(strict["say"].as_str().unwrap().contains("strict"));
    palettes.command("loose 2").await.unwrap();
    let (near, _) = call(json!({"action": "neighbours"})).await;
    let near: Vec<&str> = near["sounds"].as_array().unwrap().iter().map(|s| s["name"].as_str().unwrap()).collect();
    assert!(near.contains(&"Kick 2") && near.contains(&"Pad 1") && !near.contains(&"Kick 0"), "{near:?}");
    let (of_one, _) = call(json!({"action": "neighbours", "path": sounds[6].path})).await;
    assert!(of_one["sounds"].as_array().unwrap().iter().all(|s| s["near"] == json!(["Pad 0"])), "{of_one}");
    let (_, error) = call(json!({"action": "neighbours", "path": "/nowhere.wav"})).await;
    assert!(error, "a path that isn't the palette's: an error");
    // Taken out and excluded by path.
    let (removed, _) = call(json!({"action": "remove", "paths": [sounds[6].path]})).await;
    assert_eq!(removed["removed"], json!(1));
    let (excluded, _) = call(json!({"action": "exclude", "paths": [sounds[2].path]})).await;
    assert_eq!(excluded["excluded"], json!(1));
    let (listed, _) = call(json!({"action": "list"})).await;
    assert_eq!(listed["excluded"][0]["path"], json!(sounds[2].path));
    let (wrong, error) = call(json!({"action": "add"})).await;
    assert!(error && wrong.as_str().unwrap().contains("paths"), "{wrong}");
});
