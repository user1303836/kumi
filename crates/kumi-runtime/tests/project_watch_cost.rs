//! Telling what changed in a Set reads the two snapshots it's given without copying them.
use kumi_runtime::{
    core::contracts::JsonObject,
    integrations::ableton::project::{describe_diff, describe_watch},
};
use serde_json::{json, Value};
use std::{
    alloc::{GlobalAlloc, Layout, System},
    cell::Cell,
};

thread_local! {
    static COUNTING: Cell<bool> = const { Cell::new(false) };
    static ALLOCATED: Cell<usize> = const { Cell::new(0) };
}
/// The system's allocator, adding up what a thread allocates while it's counting.
struct Counted;
fn counted(bytes: usize) {
    if COUNTING.try_with(Cell::get).unwrap_or(false) {
        let _ = ALLOCATED.try_with(|n| n.set(n.get() + bytes));
    }
}
// SAFETY: each call is passed to System unchanged; the counting uses const thread-locals and allocates nothing.
unsafe impl GlobalAlloc for Counted {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        counted(layout.size());
        unsafe { System.alloc(layout) }
    }
    unsafe fn alloc_zeroed(&self, layout: Layout) -> *mut u8 {
        counted(layout.size());
        unsafe { System.alloc_zeroed(layout) }
    }
    unsafe fn dealloc(&self, pointer: *mut u8, layout: Layout) {
        unsafe { System.dealloc(pointer, layout) }
    }
    unsafe fn realloc(&self, pointer: *mut u8, layout: Layout, size: usize) -> *mut u8 {
        counted(size.saturating_sub(layout.size()));
        unsafe { System.realloc(pointer, layout, size) }
    }
}
#[global_allocator]
static ALLOCATOR: Counted = Counted;

/// The bytes `work` allocates on this thread.
fn allocated(work: impl FnOnce()) -> usize {
    ALLOCATED.with(|n| n.set(0));
    COUNTING.with(|on| on.set(true));
    work();
    COUNTING.with(|on| on.set(false));
    ALLOCATED.with(Cell::get)
}
/// A snapshot of 200 tracks, each with 50 KB of routing (10 MB in all), the eighth named `eighth`.
fn snapshot(eighth: &str) -> Vec<JsonObject> {
    let routing = "x".repeat(50_000);
    let records: Vec<_> = (0..200)
        .map(|i| {
            let name = if i == 7 { eighth.to_owned() } else { format!("Track {i}") };
            json!({"kind":"track","order":i,"name":name,"snapshotId":format!("track-{i}"),"data":{"kind":"midi","structureHash":format!("h{i}"),"routing":routing}})
        })
        .collect();
    vec![json!({"records":records}).as_object().unwrap().clone()]
}
#[test]
fn a_rename_is_told_without_copying_the_snapshots() {
    let before = snapshot("Track 7");
    let after = snapshot("Lead");
    let diff = json!({"items":[{"type":"change","kind":"track","beforeSnapshotId":"track-7","afterSnapshotId":"track-7","facets":["renamed"],"details":[]}]});
    let diff = diff.as_object().unwrap();
    let mut described = None;
    let bytes = allocated(|| described = Some(describe_diff(diff, &before, &after, None)));
    assert_eq!(described.unwrap().lines, ["Renamed track “Track 7” → “Lead”"]);
    assert!(bytes < 1_000_000, "describing a rename allocated {bytes} bytes: a 10 MB snapshot was copied");
    let mut watched = None;
    let bytes = allocated(|| watched = Some(describe_watch(diff, &before, &after, None)));
    let watched = watched.unwrap();
    assert_eq!((watched.changes.len(), watched.changes[0].get("renamedFrom")), (1, Some(&Value::from("Track 7"))));
    assert!(bytes < 1_000_000, "watching a rename allocated {bytes} bytes: a 10 MB snapshot was copied");
}
