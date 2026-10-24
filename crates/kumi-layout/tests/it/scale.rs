//! The map at a big library's size, in Extended only: 300,000 songs' style vectors laid out within the brief's budget.

use super::layout::{grouped, kept_together};
use kumi_layout::{layout, Layout};

#[test]
fn three_hundred_thousand_songs_are_laid_out_within_fifteen_minutes() {
    let (count, groups) = (300_000, 30);
    let read = |at: usize, buffer: &mut [f32]| grouped(at, 512, groups, 0.3, buffer);
    let started = std::time::Instant::now();
    let places = layout(count, 512, &read, &Layout::default());
    let took = started.elapsed();
    eprintln!("{count} points of 512 numbers on {} threads: {took:?}", Layout::default().threads);
    assert!(took < std::time::Duration::from_secs(15 * 60), "{took:?}");
    assert_eq!(places.len(), count);
    assert!(places.iter().all(|place| place.iter().all(|value| (0. ..=1.).contains(value))));
    let kept = kept_together(&places, groups);
    assert!(kept >= count * 95 / 100, "{kept} of {count} by their own group");
}
