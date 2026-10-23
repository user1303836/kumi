use kumi_runtime::core::disk::{free_bytes, low_disk_with, MB};

#[tokio::test]
async fn free_space_is_read_for_a_paths_disk_from_its_nearest_folder_that_exists() {
    let free = free_bytes(&std::env::temp_dir().join("kumi-not-there").join("deeper").join("file.wav")).await;
    assert!(matches!(free, Some(bytes) if bytes > 0.0));
}

#[tokio::test]
async fn a_nearly_full_disk_gets_a_plain_refusal_saying_whats_free_and_what_to_do_enough_room_or_no_answer_says_nothing() {
    assert_eq!(
        low_disk_with("/x", 500.0 * MB, "Live records to", |_| async { Some(182.0 * MB) }).await.as_deref(),
        Some("Only 182 MB is free on the disk Live records to, so it would likely fail partway. Free some space (empty the Trash or Recycle Bin, or move old bounces and videos off that disk), then try again."),
    );
    assert!(low_disk_with("/x", 2_000.0 * MB, "Kumi keeps its programs on", |_| async { Some(1_500.0 * MB) })
        .await
        .unwrap()
        .starts_with("Only 1.5 GB is free"));
    assert_eq!(low_disk_with("/x", 500.0 * MB, "Live records to", |_| async { Some(800.0 * MB) }).await, None);
    assert_eq!(low_disk_with("/x", 500.0 * MB, "Live records to", |_| async { None }).await, None);
}
