//! An audio clip's waveform for the change that imported it or changed it, read from the clip's sample once the change
//! has been told: the change isn't kept waiting, and a long sample takes a moment to read. Put on the change's record,
//! which is told again, as a device's curve is (`parameters::curve_later`).
//!
//! Only for a picture the producer didn't ask for, so it costs little: a WAV or AIFF file on this computer, read in place
//! (never converted, never from a share or a device), one at a time, and read once while it's unchanged.
use super::changes::{pictures, ChangeKind};
use super::history::History;
use indexmap::IndexMap;
use kumi_ai::timing;
use kumi_common::abort;
use kumi_common::path::{network_or_device, reserved_device};
use kumi_types::contracts::JsonObject;
use kumi_types::picture::MAX_PEAKS;
use serde_json::Value;
use std::path::Path;
use std::rc::Rc;
use std::sync::{LazyLock, Mutex};
use std::time::SystemTime;

/// The longest a sample's read may take: past it, the change keeps the picture it has.
const WAVE_WAIT_MS: u64 = 20_000;
/// How many samples' waveforms are kept, so a run of changes to one clip reads its sample once.
const KEPT: usize = 16;

/// A file as it was when its waveform was read: its path, size and last change.
type Read = (String, u64, Option<SystemTime>);
/// Waveforms read, by the file as it was: its length in seconds and its peaks.
static WAVES: LazyLock<Mutex<IndexMap<Read, (f64, Vec<f32>)>>> = LazyLock::new(Mutex::default);
/// One sample read at a time: a plan changing twenty clips reads their samples one after another.
static READING: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

/// Whether a sample's waveform is read: a WAV or AIFF file on this computer (others would be converted whole first).
fn readable(path: &str) -> bool {
    let extension = Path::new(path).extension().map(|e| e.to_string_lossy().to_ascii_lowercase()).unwrap_or_default();
    ["wav", "wave", "aif", "aiff", "aifc"].contains(&extension.as_str()) && !network_or_device(path) && !reserved_device(path)
}

/// Reads the waveform of the clip an import made (`input`'s file) or an audio clip change changed (the preview's
/// `sample`), and puts it on the change `id`'s record. Nothing for any other change, one made quietly, or a sample it
/// doesn't read (`readable`): the change keeps the values it has.
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
    let Some(path) = sample.get("filePath").and_then(Value::as_str).filter(|path| readable(path)).map(str::to_owned) else { return };
    // (an import plays its whole sample: no part of it to light)
    let (sample, proposed) =
        if kind.tool == "import_audio" { (JsonObject::new(), JsonObject::new()) } else { (sample.clone(), proposed.clone()) };
    let (history, id) = (history.clone(), id.to_owned());
    tokio::task::spawn_local(timing::background(async move {
        let signal = abort::any([history.connection.lifetime.clone(), abort::timeout(WAVE_WAIT_MS)]);
        let read = tokio::select! {
            biased;
            () = signal.cancelled() => return,
            read = waveform(&path, &signal) => read,
        };
        let Some((seconds, peaks)) = read else { return };
        let peaks = peaks.into_iter().map(f64::from).collect();
        if let Some(wave) = pictures::audio_wave(seconds, peaks, &sample, &proposed, values).bounded() {
            history.set_picture(&id, wave);
        }
    }));
}

/// A sample's length and peaks, read once while the file is unchanged, one sample at a time.
async fn waveform(path: &str, signal: &abort::Signal) -> Option<(f64, Vec<f32>)> {
    let _turn = READING.lock().await;
    let meta = tokio::fs::metadata(path).await.ok()?;
    let read: Read = (path.to_owned(), meta.len(), meta.modified().ok());
    if let Some(kept) = WAVES.lock().ok()?.get(&read).cloned() {
        return Some(kept);
    }
    let wave = kumi_media::audio::decode::peaks(path, MAX_PEAKS, signal).await.ok()?;
    let mut waves = WAVES.lock().ok()?;
    waves.insert(read, wave.clone());
    if waves.len() > KEPT {
        waves.shift_remove_index(0);
    }
    Some(wave)
}

#[cfg(test)]
mod tests {
    use super::*;
    use kumi_types::contracts::ChangePicture;
    use serde_json::json;

    fn object(value: Value) -> JsonObject {
        value.as_object().unwrap().clone()
    }

    /// What `wave_later` puts on a change's record, once the sample is read.
    fn drawn(seconds: f64, peaks: Vec<f64>, sample: &JsonObject, proposed: &JsonObject) -> Option<ChangePicture> {
        pictures::audio_wave(seconds, peaks, sample, proposed, Vec::new()).bounded()
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

    #[test]
    fn only_a_wav_or_aiff_on_this_computer_is_read() {
        assert!(readable("/Users/me/Samples/Kick.wav") && readable(r"C:\Samples\Pad.AIF"));
        // Converted whole first: not for a picture.
        assert!(!readable("/Users/me/Music/song.mp3") && !readable("/x/loop.flac"));
        // A share or a device is never opened.
        assert!(!readable(r"\\nas\samples\kick.wav") && !readable("//nas/samples/kick.wav"));
    }

    #[tokio::test(flavor = "current_thread")]
    async fn a_samples_waveform_is_read_once_while_it_is_unchanged() {
        let folder = std::env::temp_dir().join(format!("kumi-waves-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir(&folder).unwrap();
        let path = folder.join("hit.wav");
        kumi_media::audio::decode::write_test_wav(&path, &[0.5; 2000], 1000);
        let path = path.to_string_lossy().into_owned();
        let signal = abort::Signal::new();
        let (seconds, peaks) = waveform(&path, &signal).await.unwrap();
        assert_eq!((seconds, peaks.len()), (2., MAX_PEAKS));
        // Kept by the file as it was: read again unchanged, it isn't read from the file.
        let meta = std::fs::metadata(&path).unwrap();
        assert!(WAVES.lock().unwrap().contains_key(&(path.clone(), meta.len(), meta.modified().ok())));
        assert_eq!(waveform(&path, &signal).await.unwrap().0, 2.);
        std::fs::remove_dir_all(&folder).unwrap();
        assert_eq!(waveform(&path, &signal).await, None, "a file gone has none");
    }
}
