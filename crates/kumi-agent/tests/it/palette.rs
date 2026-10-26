//! A session's palettes: files gathered make one when there's none, /palette names, lists and chooses them, and a sound
//! that moved is found again by its name and size.
use futures::FutureExt;
use kumi_agent::core::{
    palette::{Gathered, PaletteView, Palettes, Refind},
    store_client::StoreClient,
};
use kumi_store::Store;
use std::{cell::RefCell, rc::Rc};

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

/// Palettes on a fresh database, with what each change told the app.
fn palettes(dir: &std::path::Path, refind: Option<Refind>) -> (Palettes, Rc<RefCell<Vec<Option<PaletteView>>>>) {
    let told: Rc<RefCell<Vec<Option<PaletteView>>>> = Rc::default();
    let client = StoreClient::new(Store::open(dir.join("kumi.db")).unwrap());
    let heard = told.clone();
    (Palettes::new(client, refind, Rc::new(move |view| heard.borrow_mut().push(view))), told)
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

case!(a_sound_that_moved_is_found_again_by_its_name_and_size, async {
    let dir = tempfile::tempdir().unwrap();
    let asked: Rc<RefCell<Vec<(String, i64)>>> = Rc::default();
    let (moved_to, heard) = (dir.path().join("Moved").join("Kick 1.wav"), asked.clone());
    let to = moved_to.to_string_lossy().to_string();
    let refind: Refind = Rc::new(move |wanted: Vec<(String, i64)>| {
        heard.borrow_mut().extend(wanted.iter().cloned());
        let to = to.clone();
        async move { wanted.iter().map(|(name, size)| (name == "Kick 1.wav" && *size == 10).then(|| (to.clone(), 42))).collect() }
            .boxed_local()
    });
    let (palettes, _) = palettes(dir.path(), Some(refind));
    let kick = sound(dir.path(), "Kick 1", 10);
    palettes.gather(vec![kick.clone(), sound(dir.path(), "Snare", 4)]).await.unwrap();
    assert!(asked.borrow().is_empty(), "nothing moved: the library isn't asked");
    // The folder renamed: the file is gone from its path, and there's one of its name and size elsewhere.
    std::fs::create_dir_all(moved_to.parent().unwrap()).unwrap();
    std::fs::rename(&kick.path, &moved_to).unwrap();
    let view = palettes.view().await.unwrap().unwrap();
    assert_eq!((view.files[0].path.as_str(), view.files[0].mtime), (moved_to.to_string_lossy().as_ref(), 42));
    assert_eq!(*asked.borrow(), [("Kick 1.wav".to_string(), 10)]);
    // Kept so: read again, it's where it went, and nothing is asked again.
    assert_eq!(palettes.view().await.unwrap().unwrap().files[0].path, moved_to.to_string_lossy());
    assert_eq!(asked.borrow().len(), 1);
});
