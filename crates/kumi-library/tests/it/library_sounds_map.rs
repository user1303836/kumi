//! The sounds' map made after learning: every measured sound in the library's folders placed, in order by path, when
//! the map is due.
use kumi_layout::Layout;
use kumi_library::{
    learn::{SoundEntry, LOG_VERSION},
    map::read_sounds_map,
    sounds_map::{due, lay_out_sounds},
    sources::{Source, SourceKind},
    store::{pack_vector, Entry, Log},
};
use std::path::Path;

/// Sound `n` in `folder`, in one of three groups by how it sounds (each group's numbers high in its own places), a
/// little apart from the rest of its group.
fn sound(folder: &Path, n: usize) -> SoundEntry {
    let numbers: Vec<f32> = (0..40).map(|d| if d % 3 == n % 3 { 5. } else { 0. } + ((n * 31 + d * 17) % 10) as f32 * 0.02).collect();
    let path = folder.join(format!("{:03} {}.wav", n, ["kick", "pad", "hat"][n % 3])).to_string_lossy().into_owned();
    SoundEntry { file: Entry { path, size: 1, mtime: 1, gone: None }, vector: Some(pack_vector(&numbers)), ..Default::default() }
}

fn how() -> Layout {
    Layout { threads: 1, ..Layout::default() }
}

#[test]
fn the_map_is_due_when_there_is_none_or_the_sounds_change_by_a_tenth() {
    assert!(!due(None, 0) && due(None, 5));
    assert!(!due(Some(100), 109) && due(Some(100), 110) && due(Some(100), 90) && !due(Some(100), 100));
    assert!(due(Some(5), 6), "a small library: one sound more");
}

#[tokio::test]
async fn measured_sounds_in_the_librarys_folders_are_placed_by_path() {
    let dir = tempfile::tempdir().unwrap();
    let samples = dir.path().join("Samples");
    std::fs::create_dir_all(&samples).unwrap();
    let source = Source { path: samples.to_string_lossy().into_owned(), label: "Samples".into(), kind: SourceKind::Folder };
    let log = Log::<SoundEntry>::new(dir.path().join("sounds.jsonl"), "sounds", LOG_VERSION);
    // Thirty measured, in no order; one not measured; one outside the library's folders.
    let mut entries: Vec<SoundEntry> = (0..30).rev().map(|n| sound(&samples, n)).collect();
    entries.push(SoundEntry { vector: None, ..sound(&samples, 99) });
    entries.push(sound(&dir.path().join("Elsewhere"), 98));
    log.append(&entries).await.unwrap();

    assert_eq!(lay_out_sounds(dir.path(), &[source.clone()], &how()).await.unwrap(), Some(30));
    let map = read_sounds_map(dir.path()).unwrap();
    let paths: Vec<&str> = map.points.iter().map(|(path, ..)| path.as_str()).collect();
    let mut sorted = paths.clone();
    sorted.sort();
    assert_eq!((paths.len(), &paths), (30, &sorted), "by path");
    assert!(paths.iter().all(|path| !path.contains("099") && !path.contains("Elsewhere")));
    assert!(map.points.iter().all(|(_, x, y)| (0. ..=1.).contains(x) && (0. ..=1.).contains(y)));
    // The kicks sit nearer each other than the pads.
    let middle = |kind: &str| {
        let mine: Vec<&(String, f64, f64)> = map.points.iter().filter(|(path, ..)| path.contains(kind)).collect();
        (
            mine.iter().map(|point| point.1).sum::<f64>() / mine.len() as f64,
            mine.iter().map(|point| point.2).sum::<f64>() / mine.len() as f64,
        )
    };
    let apart = |a: (f64, f64), b: (f64, f64)| ((a.0 - b.0).powi(2) + (a.1 - b.1).powi(2)).sqrt();
    let kicks = middle("kick");
    let farthest_kick =
        map.points.iter().filter(|(path, ..)| path.contains("kick")).map(|point| apart(kicks, (point.1, point.2))).fold(0., f64::max);
    assert!(farthest_kick < apart(kicks, middle("pad")), "{farthest_kick} against {}", apart(kicks, middle("pad")));

    // Not due again until the sounds change by a tenth: two more aren't (of 30), and three more are.
    assert_eq!(lay_out_sounds(dir.path(), &[source.clone()], &how()).await.unwrap(), None);
    log.append(&[sound(&samples, 30), sound(&samples, 31)]).await.unwrap();
    assert_eq!(lay_out_sounds(dir.path(), &[source.clone()], &how()).await.unwrap(), None);
    log.append(&[sound(&samples, 32)]).await.unwrap();
    assert_eq!(lay_out_sounds(dir.path(), &[source], &how()).await.unwrap(), Some(33));
}
