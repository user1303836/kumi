//! The listen tool against a reference: heard with the file at once when told how, and from memory when compared again.
use kumi_common::abort::Signal;
use kumi_media::audio::tools::{listening_tools, ListeningOptions};
use kumi_types::contracts::KernelTool;
use serde_json::{json, Value};
use std::{path::Path, rc::Rc, time::Duration};

fn tone(seconds: f64, hz: f64, amplitude: f64) -> Vec<f32> {
    (0..(seconds * 48000.) as usize).map(|i| (amplitude * (2. * std::f64::consts::PI * hz * i as f64 / 48000.).sin()) as f32).collect()
}
/// 16-bit stereo at 48 kHz, the channel in both.
fn wav(path: &Path, channel: &[f32]) {
    let size = (channel.len() * 4) as u32;
    let mut out = Vec::new();
    out.extend(b"RIFF");
    out.extend((size + 36).to_le_bytes());
    out.extend(b"WAVEfmt ");
    out.extend(16u32.to_le_bytes());
    out.extend(1u16.to_le_bytes());
    out.extend(2u16.to_le_bytes());
    out.extend(48000u32.to_le_bytes());
    out.extend((48000u32 * 4).to_le_bytes());
    out.extend(4u16.to_le_bytes());
    out.extend(16u16.to_le_bytes());
    out.extend(b"data");
    out.extend(size.to_le_bytes());
    for value in channel {
        let sample = ((*value as f64).clamp(-1., 1.) * 32767.).round() as i16;
        out.extend(sample.to_le_bytes());
        out.extend(sample.to_le_bytes());
    }
    std::fs::write(path, out).unwrap();
}
async fn heard(tool: &Rc<dyn KernelTool>, input: Value) -> String {
    let result = tool.execute(input.as_object().unwrap().clone(), Signal::new()).await.unwrap();
    assert!(!result.is_error, "{}", result.text);
    result.text
}

#[tokio::test]
async fn a_reference_is_heard_with_the_file_when_told_how_and_again_only_once_its_file_changes() {
    let root = tempfile::tempdir().unwrap();
    let (mine, reference) = (root.path().join("mine.wav"), root.path().join("reference.wav"));
    wav(&mine, &tone(3., 220., 0.4));
    wav(&reference, &tone(3., 440., 0.2));
    let told = json!({"file":mine,"compare_to":reference,"focus":"sound"});
    // Told how to hear them, the two are heard at once; not told, the file's length chooses (3 s: a sound) and the file
    // is heard first. The same answer either way.
    let tool = listening_tools(ListeningOptions::default()).remove(0);
    let first = heard(&tool, told.clone()).await;
    let chosen = heard(&listening_tools(ListeningOptions::default()).remove(0), json!({"file":mine,"compare_to":reference})).await;
    assert_eq!(chosen, first);
    assert_eq!(heard(&tool, told.clone()).await, first);
    // While its file is as it was (its size and time), the reference isn't heard again: other sound written into it at
    // that size and time isn't heard.
    let modified = std::fs::metadata(&reference).unwrap().modified().unwrap();
    wav(&reference, &tone(3., 880., 0.5));
    let file = std::fs::File::options().write(true).open(&reference).unwrap();
    file.set_modified(modified).unwrap();
    drop(file);
    assert_eq!(heard(&tool, told.clone()).await, first);
    // Saved again, it is.
    std::fs::File::options().write(true).open(&reference).unwrap().set_modified(modified + Duration::from_secs(2)).unwrap();
    let changed = heard(&tool, told.clone()).await;
    assert_ne!(changed, first);
    assert_eq!(changed, heard(&listening_tools(ListeningOptions::default()).remove(0), told).await);
}
