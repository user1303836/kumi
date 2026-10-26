use kumi_store::{
    palettes::{self, Item, ItemKind},
    Store,
};

fn item(path: &str, size: i64, mtime: i64) -> Item {
    Item {
        path: path.into(),
        size,
        mtime,
        name: path.rsplit('/').next().unwrap().trim_end_matches(".wav").into(),
        kind: ItemKind::Sound,
        class: Some("kick".into()),
        excluded: false,
        added_at: 0,
    }
}

fn open() -> (tempfile::TempDir, Store) {
    let folder = tempfile::tempdir().unwrap();
    let store = Store::open(folder.path().join("kumi.db")).unwrap();
    (folder, store)
}

#[test]
fn palettes_have_names_of_their_own_whatever_their_case() {
    let (_folder, store) = open();
    let drums = store.write_wait(|c| palettes::create(c, "Drums", 1)).unwrap().expect("made");
    assert_eq!((drums.name.as_str(), drums.reach, drums.items), ("Drums", 0, 0));
    assert!(store.write_wait(|c| palettes::create(c, " drums ", 2)).unwrap().is_none(), "the same name, another case");
    assert_eq!(store.read(|c| palettes::named(c, "DRUMS")).unwrap().map(|p| p.id), Some(drums.id.clone()));
    assert_eq!(store.read(palettes::next_name).unwrap(), "Palette 1");
    store.write_wait(|c| palettes::create(c, "Palette 1", 3)).unwrap();
    assert_eq!(store.read(palettes::next_name).unwrap(), "Palette 2");
    // Renamed, not to another's name.
    let id = drums.id.clone();
    assert!(!store.write_wait(move |c| palettes::rename(c, &id, "palette 1", 4)).unwrap());
    let id = drums.id.clone();
    assert!(store.write_wait(move |c| palettes::rename(c, &id, "Kicks", 5)).unwrap());
    let names: Vec<String> = store.read(palettes::all).unwrap().into_iter().map(|p| p.name).collect();
    assert_eq!(names, ["Kicks", "Palette 1"], "the one changed last first");
}

#[test]
fn a_palette_keeps_each_file_once_in_the_order_it_came_and_its_excluded_sounds_apart() {
    let (_folder, store) = open();
    let id = store.write_wait(|c| palettes::create(c, "Drums", 1)).unwrap().unwrap().id;
    let (first, second) = (item("/s/Kick 1.wav", 100, 7), item("/s/Kick 2.wav", 200, 8));
    let (id2, both) = (id.clone(), vec![first.clone(), second.clone()]);
    assert_eq!(store.write_wait(move |c| palettes::add(c, &id2, &both, 10)).unwrap(), 2);
    // The same file again: kept once. The same path with another size or time is another file.
    let (id2, again) = (id.clone(), vec![first.clone(), item("/s/Kick 1.wav", 101, 7)]);
    assert_eq!(store.write_wait(move |c| palettes::add(c, &id2, &again, 20)).unwrap(), 1);
    let id2 = id.clone();
    let items = store.read(move |c| palettes::items(c, &id2)).unwrap();
    assert_eq!(
        items.iter().map(|i| (i.path.as_str(), i.size)).collect::<Vec<_>>(),
        [("/s/Kick 1.wav", 100), ("/s/Kick 2.wav", 200), ("/s/Kick 1.wav", 101)]
    );
    assert_eq!((items[0].name.as_str(), items[0].class.as_deref(), items[0].kind), ("Kick 1", Some("kick"), ItemKind::Sound));
    // Excluded: one of its items no more, and kept apart; added again, an item again.
    let (id2, gone) = (id.clone(), second.clone());
    store.write_wait(move |c| palettes::exclude(c, &id2, &gone, true, 30)).unwrap();
    let id2 = id.clone();
    let palette = store.read(move |c| palettes::by_id(c, &id2)).unwrap().unwrap();
    assert_eq!(palette.items, 2, "its excluded sound isn't an item");
    let id2 = id.clone();
    assert!(store.read(move |c| palettes::items(c, &id2)).unwrap().iter().any(|i| i.path == "/s/Kick 2.wav" && i.excluded));
    let (id2, back) = (id.clone(), vec![second.clone()]);
    assert_eq!(store.write_wait(move |c| palettes::add(c, &id2, &back, 40)).unwrap(), 1);
    // Removed, and moved (found again by name and size).
    let id2 = id.clone();
    assert!(store.write_wait(move |c| palettes::remove(c, &id2, ("/s/Kick 1.wav", 101, 7), 50)).unwrap());
    let id2 = id.clone();
    assert!(store.write_wait(move |c| palettes::moved(c, &id2, ("/s/Kick 1.wav", 100, 7), ("/new/Kick 1.wav", 9))).unwrap());
    let id2 = id.clone();
    let paths: Vec<(String, i64)> = store.read(move |c| palettes::items(c, &id2)).unwrap().into_iter().map(|i| (i.path, i.mtime)).collect();
    assert_eq!(paths, [("/new/Kick 1.wav".to_string(), 9), ("/s/Kick 2.wav".to_string(), 8)]);
    // Its reach, and forgetting it takes its files.
    let id2 = id.clone();
    store.write_wait(move |c| palettes::set_reach(c, &id2, 5, 60)).unwrap();
    let id2 = id.clone();
    assert_eq!(store.read(move |c| palettes::by_id(c, &id2)).unwrap().unwrap().reach, 5);
    let id2 = id.clone();
    assert!(store.write_wait(move |c| palettes::forget(c, &id2)).unwrap());
    let id2 = id.clone();
    assert!(store.read(move |c| palettes::items(c, &id2)).unwrap().is_empty());
    assert!(store.read(palettes::all).unwrap().is_empty());
}
