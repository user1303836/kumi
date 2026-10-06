//! Meter-aware change descriptions and the additional Live change catalog.
use kumi_common::js::number::{parse, to_fixed, to_string};
use std::cell::Cell;
thread_local! {static BEATS_PER_BAR:Cell<f64>=const{Cell::new(4.0)};}
thread_local! {static METER:Cell<(u32, u32)>=const{Cell::new((4, 4))};}
pub fn set_meter(numerator: f64, denominator: f64) {
    if numerator > 0.0 && denominator > 0.0 {
        BEATS_PER_BAR.set(numerator * 4.0 / denominator);
        METER.set((numerator as u32, denominator as u32));
    }
}
/// The Set's time signature, as the last look at it read it.
pub fn meter() -> (u32, u32) {
    METER.get()
}
thread_local! {static SCALE: std::cell::RefCell<Option<String>> = const { std::cell::RefCell::new(None) };}
/// The Set's scale ("D Dorian"), as the last look at it read it, when it has one worth naming.
pub fn set_scale(scale: Option<String>) {
    SCALE.with_borrow_mut(|kept| *kept = scale);
}
pub fn scale() -> Option<String> {
    SCALE.with_borrow(Clone::clone)
}
fn trim(value: f64) -> String {
    to_string(parse(&to_fixed(value, 2)).unwrap_or(f64::NAN))
}
fn plural(count: f64, one: &str) -> String {
    format!("{} {one}{}", to_string(count), if count == 1.0 { "" } else { "s" })
}
pub fn bars(beats: f64) -> String {
    let per_bar = BEATS_PER_BAR.get();
    let bar = (beats / per_bar).floor() + 1.0;
    let beat = beats - (bar - 1.0) * per_bar;
    if beat < 0.001 {
        format!("bar {}", to_string(bar))
    } else {
        format!("bar {} beat {}", to_string(bar), trim(beat + 1.0))
    }
}
pub fn span(beats: f64) -> String {
    let count = beats / BEATS_PER_BAR.get();
    if count.is_finite() && count.fract() == 0.0 {
        plural(count, "bar")
    } else {
        plural(parse(&trim(beats)).unwrap_or(f64::NAN), "beat")
    }
}
pub const MORE_REFERENCE_FIELDS: &[&str] =
    &["targetRef", "targetTrackRef", "targetChainRef", "slotRef", "sceneRef", "takeLaneRef", "destinationTrackRef", "locatorRef"];
pub static MORE_CHANGES: std::sync::LazyLock<Vec<&'static super::changes::ChangeKind>> = std::sync::LazyLock::new(|| {
    let names: Vec<String> = serde_json::from_str(include_str!("more-change-tools.json")).unwrap();
    names
        .iter()
        .map(|name| super::changes::CHANGES.iter().find(|kind| &kind.tool == name).expect("additional change is in full catalog"))
        .collect()
});
