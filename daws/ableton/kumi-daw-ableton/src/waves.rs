//! An audio clip's waveform for the change that imported it or changed it, read from the clip's sample once the change
//! has been told: the change isn't kept waiting, and a long sample takes a moment to read. Put on the change's record,
//! which is told again, as a device's curve is (`parameters::curve_later`).
use super::changes::{pictures, ChangeKind};
use super::history::History;
use kumi_ai::timing;
use kumi_common::abort;
use kumi_types::contracts::JsonObject;
use kumi_types::picture::MAX_PEAKS;
use serde_json::Value;
use std::rc::Rc;

/// The longest a sample's read may take: past it, the change keeps the picture it has.
const WAVE_WAIT_MS: u64 = 20_000;

/// Reads the waveform of the clip an import made (`input`'s file) or an audio clip change changed (the preview's
/// `sample`), and puts it on the change `id`'s record. Nothing for any other change, or one made quietly.
pub fn wave_later(history: &Rc<History>, kind: &ChangeKind, preview: &JsonObject, input: &JsonObject, id: &str) {
    if history.is_quiet() {
        return;
    }
    let empty = JsonObject::new();
    let (sample, proposed, values) = match kind.tool.as_str() {
        "import_audio" => (input, &empty, Vec::new()),
        "set_audio_clip" => (
            preview.get("sample").and_then(Value::as_object).unwrap_or(&empty),
            preview.get("proposed").and_then(Value::as_object).unwrap_or(input),
            pictures::audio_values(preview, input),
        ),
        _ => return,
    };
    let Some(path) = sample.get("filePath").and_then(Value::as_str).filter(|path| !path.is_empty()).map(str::to_owned) else { return };
    // (an import plays its whole sample: no part of it to light)
    let (sample, proposed) =
        if kind.tool == "import_audio" { (JsonObject::new(), JsonObject::new()) } else { (sample.clone(), proposed.clone()) };
    let (history, id) = (history.clone(), id.to_owned());
    tokio::task::spawn_local(timing::background(async move {
        let signal = abort::any([history.connection.lifetime.clone(), abort::timeout(WAVE_WAIT_MS)]);
        let Ok((seconds, peaks)) = kumi_media::audio::decode::peaks(&path, MAX_PEAKS, &signal).await else { return };
        let peaks = peaks.into_iter().map(f64::from).collect();
        if let Some(wave) = pictures::audio_wave(seconds, peaks, &sample, &proposed, values).bounded() {
            history.set_picture(&id, wave);
        }
    }));
}

#[cfg(test)]
mod tests {
    use super::*;
    use kumi_types::contracts::ChangePicture;
    use serde_json::json;

    /// What `wave_later` puts on a change's record, once the sample is read.
    fn drawn(seconds: f64, peaks: Vec<f64>, sample: &JsonObject, proposed: &JsonObject) -> Option<ChangePicture> {
        pictures::audio_wave(seconds, peaks, sample, proposed, Vec::new()).bounded()
    }

    fn object(value: Value) -> JsonObject {
        value.as_object().unwrap().clone()
    }

    #[test]
    fn a_warped_clips_loop_is_found_in_its_sample_by_its_warp_markers() {
        // 120 BPM over a 10 s sample: beat 4 is second 2.
        let sample = object(json!({"filePath":"/x.wav","warping":true,"loopStart":0,"loopEnd":8,
            "warpMarkers":[{"beatTime":0,"sampleTime":0},{"beatTime":20,"sampleTime":10}]}));
        let Some(ChangePicture::Wave { before, after, .. }) = drawn(10., vec![0.5; 8], &sample, &object(json!({"loopStart":4}))) else {
            panic!()
        };
        assert_eq!((before, after), (Some([0., 4.]), Some([2., 4.])));
        // Unwarped, a loop is in seconds already.
        let plain = object(json!({"filePath":"/x.wav","warping":false,"loopStart":1,"loopEnd":3}));
        let Some(ChangePicture::Wave { after, .. }) = drawn(10., vec![0.5; 8], &plain, &JsonObject::new()) else { panic!() };
        assert_eq!(after, Some([1., 3.]));
    }

    #[test]
    fn an_audio_clips_settings_are_said_before_and_after() {
        let preview =
            object(json!({"prior":{"gain":0.5,"pitchCoarse":0,"warpMode":0},"proposed":{"gain":0.4,"pitchCoarse":12,"warpMode":4}}));
        let values = pictures::audio_values(&preview, &JsonObject::new());
        let said: Vec<(&str, Option<&str>, Option<&str>)> =
            values.iter().map(|value| (value.name.as_str(), value.from_text.as_deref(), value.to_text.as_deref())).collect();
        assert_eq!(
            said,
            [("Gain", None, None), ("Transpose", Some("0 st"), Some("+12 st")), ("Warp mode", Some("Beats"), Some("Complex"))]
        );
        assert_eq!((values[0].from, values[0].to), (Some(0.5), Some(0.4)));
    }
}
