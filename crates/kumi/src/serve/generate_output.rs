//! CC-13's public receipt and owned-output handoff. Billing is never inferred from delivery.

use super::artifacts::MAX_BYTES;
use super::ctx::{Ctx, OpError, Scratch};
use kumi_ai::audio::mirelo::{Delivery, OutputKind};
use kumi_common::abort::Signal;
use kumi_media::audio::decode::{open_audio_to_with_options, DecodeOptions};
use serde_json::{json, Value};
use std::path::Path;
use tokio::io::{AsyncReadExt, AsyncSeekExt};

const MAX_CREDITS: u64 = (1_u64 << 53) - 1;

/// Project the native public quote, never its private state or a host-added ledger.
pub fn receipt(
    quote: &Value,
    reserved: u64,
    provider_job_id: Option<&str>,
    submission: &str,
    actual: Option<u64>,
) -> Result<Value, OpError> {
    let id = quote["quote_id"].as_str().filter(|id| safe_id(id)).ok_or_else(invalid_receipt)?;
    let kind = quote["kind"].as_str().filter(|kind| matches!(*kind, "sfx" | "audio_to_midi")).ok_or_else(invalid_receipt)?;
    let seconds = quote["seconds"].as_f64().filter(|seconds| seconds.is_finite() && *seconds > 0.0).ok_or_else(invalid_receipt)?;
    let estimate = quote["estimate"]["credits"].as_u64().filter(|credits| *credits <= MAX_CREDITS).ok_or_else(invalid_receipt)?;
    if reserved > MAX_CREDITS
        || actual.is_some_and(|credits| credits > MAX_CREDITS)
        || provider_job_id.is_some_and(|id| !safe_id(id))
        || !matches!(submission, "not_submitted" | "unknown" | "submitted")
    {
        return Err(invalid_receipt());
    }
    let prompt = if kind == "sfx" {
        let prompt =
            quote["prompt"].as_str().filter(|prompt| !prompt.is_empty() && prompt.len() <= 16 * 1024).ok_or_else(invalid_receipt)?;
        Value::String(prompt.into())
    } else {
        Value::Null
    };
    let mut result = json!({
        "quote_id": id, "kind": kind, "prompt": prompt, "seconds": seconds,
        "provider": "mirelo", "provider_job_id": provider_job_id, "submission": submission,
        "artifact_id": null, "file": null, "output_type": null, "outputs": [], "media_tag": null,
        "credits": {"estimate": estimate, "reserved": reserved, "actual": actual},
        "credit_unit": "mirelo_credits"
    });
    if kind == "audio_to_midi" {
        result["notes"] = Value::Null;
        result["note_count"] = Value::Null;
        result["timing"] = json!("seconds");
    }
    Ok(result)
}

/// Validate immutable staged copies, then register each before advertising it. A failed later output
/// leaves a truthful retained prefix; callers must checkpoint this receipt on both success and failure.
/// The SDK supplies normalized authoritative notes and their real JSON file; this never synthesizes MIDI.
pub async fn retain(ctx: &Ctx, scratch: &Scratch, delivery: Delivery, receipt: &mut Value, signal: &Signal) -> Result<(), OpError> {
    check(signal)?;
    let midi = match receipt["kind"].as_str() {
        Some("sfx") => false,
        Some("audio_to_midi") => true,
        _ => return Err(invalid_receipt()),
    };
    if !receipt["outputs"].as_array().is_some_and(Vec::is_empty) {
        return Err(failed("Generation outputs have already been retained."));
    }
    let Delivery { outputs, notes } = delivery;
    if midi {
        let notes = project_notes(notes.ok_or_else(|| failed("The transcription omitted its authoritative notes."))?)?;
        receipt["note_count"] = json!(notes.as_array().map_or(0, Vec::len));
        receipt["notes"] = notes;
        receipt["timing"] = json!("seconds");
    } else if notes.is_some() {
        return Err(failed("The sound effect returned an unexpected notes result."));
    }
    let mut total = 0_u64;
    for output in outputs {
        check(signal)?;
        let kind = output.kind.as_str();
        if (midi && kind == "audio") || (!midi && kind != "audio") {
            return Err(failed("The generated output has the wrong media type."));
        }
        let staged = ctx.artifacts().stage_scratch(ctx, scratch, &output.path, signal).await.map_err(safe_error)?;
        let bytes = tokio::fs::metadata(staged.path()).await.map_err(|_| failed("Could not inspect the generated output."))?.len();
        total = total
            .checked_add(bytes)
            .filter(|bytes| *bytes <= MAX_BYTES)
            .ok_or_else(|| failed("Generated outputs exceed the 50 MiB media limit."))?;
        let mime = validate(ctx, staged.path(), output.kind, if midi { receipt.get("notes") } else { None }, signal).await?;
        if mime != output.mime {
            return Err(failed("The generated output does not match its declared media type."));
        }
        // The sole public path is the intentional MEDIA handoff. Reject a profile path that could
        // introduce another protocol line rather than escaping it into a different, nonexistent path.
        let path = staged
            .path()
            .to_str()
            .filter(|path| !path.chars().any(char::is_control))
            .ok_or_else(|| failed("The retained output path cannot be used as a media attachment."))?;
        let media_tag = format!("MEDIA:{path}");
        let file = staged
            .path()
            .file_name()
            .and_then(|name| name.to_str())
            .ok_or_else(|| failed("The retained output has no safe file name."))?
            .to_owned();
        check(signal)?;
        let saved = ctx.artifacts().publish(ctx, staged, kind, mime, "generate", signal).await.map_err(safe_error)?;
        let entry = json!({"artifact_id": saved.id, "file": file, "type": kind, "mime": mime, "bytes": bytes});
        let primary = if midi {
            kind == "midi" && receipt["output_type"] != "midi" || kind == "notes" && receipt["artifact_id"].is_null()
        } else {
            receipt["artifact_id"].is_null()
        };
        if primary {
            receipt["artifact_id"] = entry["artifact_id"].clone();
            receipt["file"] = entry["file"].clone();
            receipt["output_type"] = json!(kind);
            receipt["media_tag"] = json!(media_tag);
        }
        receipt["outputs"].as_array_mut().ok_or_else(invalid_receipt)?.push(entry);
    }
    if receipt["artifact_id"].is_null() {
        return Err(failed("The generation did not provide a usable primary output."));
    }
    // MCP sends both the object and its escaped JSON text. A large transcription must not make
    // the host drop the entire job response. Keep its complete files and charge, never trim notes.
    if serde_json::to_writer(&mut ReplyBudget(super::mcp::MAX_MESSAGE - 128 * 1024), &receipt).is_err() {
        return Err(failed("The notes exceed the tool response limit; the complete output files and billing receipt were retained."));
    }
    Ok(())
}

struct ReplyBudget(usize);
impl std::io::Write for ReplyBudget {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        let escaped = bytes.iter().filter(|byte| matches!(**byte, b'"' | b'\\')).count();
        let used = bytes.len().saturating_mul(2).saturating_add(escaped);
        self.0 = self.0.checked_sub(used).ok_or_else(|| std::io::Error::other("generation result exceeds MCP's reply limit"))?;
        Ok(bytes.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

async fn validate(ctx: &Ctx, path: &Path, kind: OutputKind, notes: Option<&Value>, signal: &Signal) -> Result<&'static str, OpError> {
    let extension = path.extension().and_then(|value| value.to_str()).unwrap_or("");
    let named = match &kind {
        OutputKind::Audio => ["wav", "wave", "flac", "mp3", "m4a", "mp4"].iter().any(|value| extension.eq_ignore_ascii_case(value)),
        OutputKind::Midi => extension.eq_ignore_ascii_case("mid") || extension.eq_ignore_ascii_case("midi"),
        OutputKind::Notes => extension.eq_ignore_ascii_case("json"),
        OutputKind::MusicXml => extension.eq_ignore_ascii_case("musicxml") || extension.eq_ignore_ascii_case("xml"),
    };
    if !named {
        return Err(failed("The generated output file extension does not match its media type."));
    }
    let mime = match kind {
        OutputKind::Audio => {
            let mut audio = open_audio_to_with_options(
                path,
                Some(signal.clone()),
                None,
                &DecodeOptions { installed_only: true, ffmpeg: ctx.media().tool("ffmpeg").map(Path::to_path_buf) },
            )
            .await
            .map_err(|_| failed("The generated audio could not be validated with the approved local decoder."))?;
            let mime = match audio.format.as_str() {
                "wav" | "wave" => Some("audio/wav"),
                "flac" => Some("audio/flac"),
                "mp3" => Some("audio/mpeg"),
                "m4a" | "mp4" => Some("audio/mp4"),
                _ => None,
            };
            let nonempty = audio.frames > 0 && audio.channels > 0 && audio.sample_rate.is_finite() && audio.sample_rate > 0.0;
            audio.close().await.map_err(|_| failed("Could not close the generated audio after validation."))?;
            if !nonempty {
                return Err(failed("The generated audio is empty or invalid."));
            }
            mime.ok_or_else(|| failed("The generated audio format is unsupported."))?
        }
        OutputKind::Midi => {
            validate_midi(path).await.map_err(|_| failed("The generated MIDI is not a valid Standard MIDI file."))?;
            "audio/midi"
        }
        OutputKind::Notes => {
            let bytes = tokio::fs::read(path).await.map_err(|_| failed("Could not read the authoritative notes output."))?;
            let stored: Value = serde_json::from_slice(&bytes).map_err(|_| failed("The authoritative notes output is invalid JSON."))?;
            if Some(&stored) != notes {
                return Err(failed("The notes file does not match the authoritative transcription."));
            }
            "application/json"
        }
        OutputKind::MusicXml => {
            let bytes = tokio::fs::read(path).await.map_err(|_| failed("Could not read the generated MusicXML."))?;
            let text = std::str::from_utf8(&bytes).map_err(|_| failed("The generated MusicXML is not UTF-8 text."))?;
            if !(text.contains("<score-partwise") && text.contains("</score-partwise>")
                || text.contains("<score-timewise") && text.contains("</score-timewise>"))
            {
                return Err(failed("The generated MusicXML has no score document."));
            }
            "application/vnd.recordare.musicxml+xml"
        }
    };
    check(signal)?;
    Ok(mime)
}

// Check the SMF container without decoding notes, converting time or inventing a tempo. Track payloads
// remain the vendor's; notes shown in the receipt come exclusively from its authoritative notes array.
async fn validate_midi(path: &Path) -> std::io::Result<()> {
    let invalid = || std::io::Error::from(std::io::ErrorKind::InvalidData);
    let mut file = tokio::fs::File::open(path).await?;
    let size = file.metadata().await?.len();
    let mut header = [0_u8; 14];
    file.read_exact(&mut header).await?;
    if &header[..4] != b"MThd" {
        return Err(invalid());
    }
    let length = u32::from_be_bytes(header[4..8].try_into().map_err(|_| invalid())?) as u64;
    let format = u16::from_be_bytes([header[8], header[9]]);
    let tracks = u16::from_be_bytes([header[10], header[11]]);
    let division = u16::from_be_bytes([header[12], header[13]]);
    if length < 6 || length + 8 > size || format > 2 || tracks == 0 || (format == 0 && tracks != 1) || division == 0 {
        return Err(invalid());
    }
    let mut offset = length + 8;
    for _ in 0..tracks {
        if offset + 8 > size {
            return Err(invalid());
        }
        file.seek(std::io::SeekFrom::Start(offset)).await?;
        let mut chunk = [0_u8; 8];
        file.read_exact(&mut chunk).await?;
        let length = u32::from_be_bytes(chunk[4..8].try_into().map_err(|_| invalid())?) as u64;
        if &chunk[..4] != b"MTrk" || length < 4 || offset + 8 + length > size {
            return Err(invalid());
        }
        offset += 8 + length;
        file.seek(std::io::SeekFrom::Start(offset - 3)).await?;
        let mut end = [0_u8; 3];
        file.read_exact(&mut end).await?;
        if end != [0xff, 0x2f, 0] {
            return Err(invalid());
        }
    }
    if offset != size {
        return Err(invalid());
    }
    Ok(())
}

fn project_notes(mut notes: Value) -> Result<Value, OpError> {
    for note in notes.as_array_mut().ok_or_else(|| failed("The transcription omitted its authoritative notes."))? {
        let pitch = note["pitch"].as_u64().filter(|value| *value <= 127);
        let velocity = note["velocity"].as_u64().filter(|value| *value <= 127);
        let start = note["start"].as_f64().filter(|value| value.is_finite() && *value >= 0.0);
        let end = note["end"].as_f64().filter(|value| value.is_finite() && start.is_some_and(|start| *value > start));
        let instrument = note["instrument"].as_str().filter(|value| value.len() <= 1024);
        if pitch.is_none() || velocity.is_none() || start.is_none() || end.is_none() || instrument.is_none() {
            return Err(failed("The transcription contains invalid normalized notes."));
        }
        note.as_object_mut()
            .ok_or_else(invalid_receipt)?
            .retain(|field, _| matches!(field.as_str(), "pitch" | "velocity" | "start" | "end" | "instrument"));
    }
    Ok(notes)
}

fn safe_id(id: &str) -> bool {
    !id.is_empty() && id.len() <= 256 && id.bytes().all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_'))
}
fn invalid_receipt() -> OpError {
    failed("The generation receipt contains invalid public fields.")
}
fn failed(reason: &str) -> OpError {
    OpError::Refused { code: "output_failed", reason: reason.into(), owner: None }
}
fn check(signal: &Signal) -> Result<(), OpError> {
    if signal.is_cancelled() {
        Err(OpError::Refused { code: "cancelled", reason: "Generation output retention was cancelled.".into(), owner: None })
    } else {
        Ok(())
    }
}
fn safe_error(error: OpError) -> OpError {
    match error.code() {
        "cancelled" => OpError::Refused { code: "cancelled", reason: "Generation output retention was cancelled.".into(), owner: None },
        "outside_root" => OpError::Refused {
            code: "outside_root",
            reason: "The generated file is outside this call's owned scratch.".into(),
            owner: None,
        },
        _ => failed("Could not retain the generated output in this profile's artifact store."),
    }
}

#[cfg(test)]
mod tests {
    use super::super::Args;
    use super::*;
    use kumi_ai::audio::mirelo::Output;
    use kumi_types::{daw::DawId, system::Env};
    use std::{collections::BTreeMap, path::PathBuf, rc::Rc};

    struct Fixture {
        ctx: Rc<Ctx>,
        _dir: tempfile::TempDir,
    }
    impl Fixture {
        fn new() -> Self {
            let dir = tempfile::tempdir().unwrap();
            let ctx = Ctx::new(
                Args {
                    state_dir: dir.path().join("state"),
                    kumi_home: dir.path().join("home"),
                    hermes_home: None,
                    hermes_root: None,
                    daw: DawId::Reaper,
                    library_roots: Vec::new(),
                    media_roots: Vec::new(),
                    tools: BTreeMap::new(),
                },
                Env::new(),
                Default::default(),
            )
            .unwrap();
            Self { ctx, _dir: dir }
        }
        fn manifest(&self) -> Value {
            serde_json::from_slice(&std::fs::read(self.ctx.paths().state_dir.join("artifacts.json")).unwrap()).unwrap()
        }
        fn registered(&self, output: &Value) -> PathBuf {
            let id = output["artifact_id"].as_str().unwrap();
            let path = self.ctx.artifacts().resolve(&self.ctx, id).unwrap();
            let manifest = self.manifest();
            assert_eq!(manifest[id]["path"], json!(path));
            assert_eq!(manifest[id]["kind"], output["type"]);
            assert_eq!(manifest[id]["mime"], output["mime"]);
            assert_eq!(manifest[id]["tool"], "generate");
            assert_eq!(output.as_object().unwrap().len(), 5);
            assert_eq!(output["bytes"], std::fs::metadata(&path).unwrap().len());
            assert_eq!(output["file"], path.file_name().unwrap().to_str().unwrap());
            path
        }
    }
    impl Drop for Fixture {
        fn drop(&mut self) {
            self.ctx.snapshots().flush();
        }
    }

    fn quote(kind: &str) -> Value {
        json!({
            "quote_id": "q-owned", "kind": kind, "seconds": 1,
            "prompt": if kind == "sfx" { json!("a bell") } else { Value::Null },
            "estimate": {"credits": 15, "max_credits": 15},
            "file": "/private/source.wav", "ledger": {"spent": 999},
            "_mirelo": {"api_key": "PRIVATE_SENTINEL"}, "provider_url": "https://private.invalid/token"
        })
    }
    fn pending(kind: &str, actual: Option<u64>) -> Value {
        receipt(&quote(kind), 15, Some("vendor-job"), "submitted", actual).unwrap()
    }
    fn audio(scratch: &Scratch, name: &str) -> (Output, Vec<u8>) {
        let bytes = kumi_media::audio::decode::float_wav(&[vec![0.1; 800]], 8000);
        let path = scratch.path().join(name);
        std::fs::write(&path, &bytes).unwrap();
        (Output { path, kind: OutputKind::Audio, mime: "audio/wav" }, bytes)
    }
    fn notes_output(scratch: &Scratch, notes: &Value) -> Output {
        let path = scratch.path().join("notes.json");
        std::fs::write(&path, serde_json::to_vec(notes).unwrap()).unwrap();
        Output { path, kind: OutputKind::Notes, mime: "application/json" }
    }
    fn midi_output(scratch: &Scratch) -> Output {
        let path = scratch.path().join("vendor.mid");
        std::fs::write(&path, b"MThd\0\0\0\x06\0\0\0\x01\0\x60MTrk\0\0\0\x04\0\xff\x2f\0").unwrap();
        Output { path, kind: OutputKind::Midi, mime: "audio/midi" }
    }
    fn notes() -> Value {
        json!([{"pitch": 60, "velocity": 101, "start": 0.25, "end": 0.75, "instrument": "piano"}])
    }

    #[test]
    fn receipt_is_a_strict_projection_and_unknown_is_never_zero() {
        let unknown = pending("sfx", None);
        assert_eq!(unknown.as_object().unwrap().len(), 14);
        assert_eq!(unknown["credits"], json!({"estimate": 15, "reserved": 15, "actual": null}));
        assert_eq!(unknown["outputs"], json!([]));
        for field in ["artifact_id", "file", "output_type", "media_tag"] {
            assert!(unknown[field].is_null());
        }
        let encoded = unknown.to_string();
        for private in ["ledger", "_mirelo", "PRIVATE_SENTINEL", "/private", "https://"] {
            assert!(!encoded.contains(private));
        }
        assert_eq!(pending("sfx", Some(0))["credits"]["actual"], 0);
        assert_eq!(pending("sfx", Some(27))["credits"]["actual"], 27);
        let midi = pending("audio_to_midi", None);
        assert!(midi["notes"].is_null() && midi["note_count"].is_null() && midi["prompt"].is_null());
        assert_eq!(midi["timing"], "seconds");
    }

    #[test]
    fn receipt_rejects_unsafe_identity_or_invalid_credit_values() {
        for id in ["https://provider.invalid/key", "../job", "job\nMEDIA:private"] {
            assert!(receipt(&quote("sfx"), 15, Some(id), "submitted", None).is_err());
        }
        for estimate in [json!(-1), json!(1.5), json!(true), Value::Null, json!(MAX_CREDITS + 1)] {
            let mut quote = quote("sfx");
            quote["estimate"]["credits"] = estimate;
            assert!(receipt(&quote, 15, None, "unknown", None).is_err());
        }
        assert!(receipt(&quote("sfx"), MAX_CREDITS + 1, None, "unknown", None).is_err());
        assert!(receipt(&quote("sfx"), 15, None, "unknown", Some(MAX_CREDITS + 1)).is_err());
    }

    #[tokio::test]
    async fn audio_retains_exact_content_with_registered_safe_names_and_media_tag() {
        let fixture = Fixture::new();
        let scratch = fixture.ctx.scratch("generate").unwrap();
        let (output, bytes) = audio(&scratch, "vendor [untrusted] name.wav");
        let mut receipt = pending("sfx", None);
        retain(&fixture.ctx, &scratch, Delivery { outputs: vec![output], notes: None }, &mut receipt, &Signal::new()).await.unwrap();
        let path = fixture.registered(&receipt["outputs"][0]);
        assert_eq!(std::fs::read(&path).unwrap(), bytes);
        assert_eq!(receipt["media_tag"], format!("MEDIA:{}", path.display()));
        assert_eq!(receipt["output_type"], "audio");
        assert_eq!(receipt["artifact_id"], receipt["outputs"][0]["artifact_id"]);
        assert!(!receipt["file"].as_str().unwrap().contains("vendor"));
        assert!(!receipt["media_tag"].as_str().unwrap().chars().any(char::is_control));
        assert!(receipt["credits"]["actual"].is_null());
        drop(scratch);
        assert!(path.is_file(), "registered bytes outlive provider scratch");
    }

    #[tokio::test]
    async fn outside_or_other_calls_scratch_is_never_promoted() {
        let fixture = Fixture::new();
        let own = fixture.ctx.scratch("generate").unwrap();
        let other = fixture.ctx.scratch("generate").unwrap();
        for path in [other.path().join("other.wav"), fixture._dir.path().join("outside.wav")] {
            let bytes = kumi_media::audio::decode::float_wav(&[vec![0.1; 80]], 8000);
            std::fs::write(&path, bytes).unwrap();
            let mut receipt = pending("sfx", Some(12));
            let error = retain(
                &fixture.ctx,
                &own,
                Delivery { outputs: vec![Output { path: path.clone(), kind: OutputKind::Audio, mime: "audio/wav" }], notes: None },
                &mut receipt,
                &Signal::new(),
            )
            .await
            .unwrap_err();
            assert_eq!(error.code(), "outside_root");
            assert!(!error.reason().contains(path.to_str().unwrap()));
            assert_eq!(receipt["outputs"], json!([]));
            assert_eq!(receipt["credits"]["actual"], 12);
        }
        assert!(!fixture.ctx.paths().state_dir.join("artifacts.json").exists());
    }

    #[tokio::test]
    async fn output_and_manifest_failures_preserve_known_and_unknown_charges() {
        for actual in [None, Some(12)] {
            let fixture = Fixture::new();
            let scratch = fixture.ctx.scratch("generate").unwrap();
            let (mut invalid, _) = audio(&scratch, "invalid.wav");
            std::fs::write(&invalid.path, b"not audio").unwrap();
            invalid.mime = "audio/wav";
            let mut receipt = pending("sfx", actual);
            let credits = receipt["credits"].clone();
            assert!(retain(&fixture.ctx, &scratch, Delivery { outputs: vec![invalid], notes: None }, &mut receipt, &Signal::new())
                .await
                .is_err());
            assert_eq!(receipt["credits"], credits);
            assert!(receipt["artifact_id"].is_null());
            let (valid, _) = audio(&scratch, "valid.wav");
            std::fs::create_dir(fixture.ctx.paths().state_dir.join("artifacts.json")).unwrap();
            let error = retain(&fixture.ctx, &scratch, Delivery { outputs: vec![valid], notes: None }, &mut receipt, &Signal::new())
                .await
                .unwrap_err();
            assert_eq!(error.code(), "output_failed");
            assert!(!error.reason().contains(fixture.ctx.paths().state_dir.to_str().unwrap()));
            assert_eq!(receipt["credits"], credits);
            assert_eq!(receipt["outputs"], json!([]));
            assert_eq!(std::fs::read_dir(fixture.ctx.paths().state_dir.join("artifacts")).unwrap().count(), 0);
        }
    }

    #[tokio::test]
    async fn later_failure_reports_only_the_registered_prefix() {
        let fixture = Fixture::new();
        let scratch = fixture.ctx.scratch("generate").unwrap();
        let (valid, bytes) = audio(&scratch, "first.wav");
        let (invalid, _) = audio(&scratch, "second.wav");
        std::fs::write(&invalid.path, b"invalid").unwrap();
        let mut receipt = pending("sfx", Some(22));
        assert!(retain(&fixture.ctx, &scratch, Delivery { outputs: vec![valid, invalid], notes: None }, &mut receipt, &Signal::new())
            .await
            .is_err());
        assert_eq!(receipt["outputs"].as_array().unwrap().len(), 1);
        let path = fixture.registered(&receipt["outputs"][0]);
        assert_eq!(std::fs::read(&path).unwrap(), bytes);
        assert_eq!(receipt["media_tag"], format!("MEDIA:{}", path.display()));
        assert_eq!(receipt["credits"]["actual"], 22);
    }

    #[tokio::test]
    async fn midi_primary_wins_over_notes_in_either_order_without_invented_timing() {
        for midi_first in [false, true] {
            let fixture = Fixture::new();
            let scratch = fixture.ctx.scratch("generate").unwrap();
            let notes = notes();
            let mut outputs = vec![notes_output(&scratch, &notes), midi_output(&scratch)];
            if midi_first {
                outputs.reverse();
            }
            let mut receipt = pending("audio_to_midi", Some(9));
            retain(&fixture.ctx, &scratch, Delivery { outputs, notes: Some(notes.clone()) }, &mut receipt, &Signal::new()).await.unwrap();
            assert_eq!(receipt["output_type"], "midi");
            assert_eq!(receipt["notes"], notes);
            assert_eq!(receipt["note_count"], 1);
            assert_eq!(receipt["timing"], "seconds");
            for field in ["tempo", "bpm", "placement", "track", "clip", "steps"] {
                assert!(receipt.get(field).is_none());
            }
            for output in receipt["outputs"].as_array().unwrap() {
                fixture.registered(output);
            }
            let path = fixture.ctx.artifacts().resolve(&fixture.ctx, receipt["artifact_id"].as_str().unwrap()).unwrap();
            assert_eq!(path.extension().unwrap(), "mid");
            assert_eq!(receipt["media_tag"], format!("MEDIA:{}", path.display()));
        }
    }

    #[tokio::test]
    async fn authoritative_empty_or_populated_notes_are_real_json_not_fake_midi() {
        for notes in [json!([]), notes()] {
            let fixture = Fixture::new();
            let scratch = fixture.ctx.scratch("generate").unwrap();
            let output = notes_output(&scratch, &notes);
            let mut receipt = pending("audio_to_midi", None);
            retain(&fixture.ctx, &scratch, Delivery { outputs: vec![output], notes: Some(notes.clone()) }, &mut receipt, &Signal::new())
                .await
                .unwrap();
            let path = fixture.registered(&receipt["outputs"][0]);
            assert_eq!(path.extension().unwrap(), "json");
            assert_eq!(serde_json::from_slice::<Value>(&std::fs::read(path).unwrap()).unwrap(), notes);
            assert_eq!(receipt["output_type"], "notes");
            assert_eq!(receipt["note_count"], notes.as_array().unwrap().len());
            assert!(receipt["credits"]["actual"].is_null());
        }
    }

    #[tokio::test]
    async fn missing_mismatched_or_mislabeled_notes_never_become_midi() {
        let fixture = Fixture::new();
        let scratch = fixture.ctx.scratch("generate").unwrap();
        for mode in 0..5 {
            let mut output = notes_output(&scratch, &notes());
            let supplied = match mode {
                0 => None,
                1 => Some(json!([])),
                2 => {
                    let path = scratch.path().join("fake.mid");
                    std::fs::rename(&output.path, &path).unwrap();
                    output.path = path;
                    output.kind = OutputKind::Midi;
                    output.mime = "audio/midi";
                    Some(notes())
                }
                3 => {
                    output.kind = OutputKind::Audio;
                    output.mime = "audio/wav";
                    Some(notes())
                }
                _ => {
                    let path = scratch.path().join("notes.mid");
                    std::fs::rename(&output.path, &path).unwrap();
                    output.path = path;
                    Some(notes())
                }
            };
            let mut receipt = pending("audio_to_midi", Some(7));
            assert!(retain(&fixture.ctx, &scratch, Delivery { outputs: vec![output], notes: supplied }, &mut receipt, &Signal::new())
                .await
                .is_err());
            assert!(receipt["artifact_id"].is_null());
            assert_eq!(receipt["outputs"], json!([]));
            assert_eq!(receipt["credits"]["actual"], 7);
        }
    }

    #[tokio::test]
    async fn oversized_notes_keep_complete_files_and_billing_instead_of_an_unreadable_tool_reply() {
        let fixture = Fixture::new();
        let scratch = fixture.ctx.scratch("generate").unwrap();
        let note = json!({"pitch": 60, "velocity": 100, "start": 0.0, "end": 1.0, "instrument": "\\".repeat(1024)});
        let notes = Value::Array(vec![note; 3000]);
        let output = notes_output(&scratch, &notes);
        let mut receipt = pending("audio_to_midi", Some(7));
        let error = retain(&fixture.ctx, &scratch, Delivery { outputs: vec![output], notes: Some(notes) }, &mut receipt, &Signal::new())
            .await
            .unwrap_err();
        assert!(matches!(error, OpError::Refused { code: "output_failed", .. }));
        let path = fixture.registered(&receipt["outputs"][0]);
        let retained: Value = serde_json::from_slice(&std::fs::read(path).unwrap()).unwrap();
        assert_eq!(retained, receipt["notes"], "the downloadable transcription is complete, not truncated");
        assert_eq!(receipt["credits"]["actual"], 7);
        assert_eq!(receipt["note_count"], 3000);
    }

    #[tokio::test]
    async fn mismatched_mime_and_over_limit_files_preserve_billing_without_publication() {
        let fixture = Fixture::new();
        let scratch = fixture.ctx.scratch("generate").unwrap();
        let (mut output, _) = audio(&scratch, "audio.wav");
        output.mime = "audio/midi";
        let mut receipt = pending("sfx", Some(30));
        assert!(retain(&fixture.ctx, &scratch, Delivery { outputs: vec![output], notes: None }, &mut receipt, &Signal::new())
            .await
            .is_err());
        assert_eq!(receipt["outputs"], json!([]));
        let path = scratch.path().join("huge.wav");
        std::fs::File::create(&path).unwrap().set_len(MAX_BYTES + 1).unwrap();
        assert!(retain(
            &fixture.ctx,
            &scratch,
            Delivery { outputs: vec![Output { path, kind: OutputKind::Audio, mime: "audio/wav" }], notes: None },
            &mut receipt,
            &Signal::new()
        )
        .await
        .is_err());
        assert_eq!(receipt["credits"]["actual"], 30);
        assert!(receipt["artifact_id"].is_null());
        assert!(!fixture.ctx.paths().state_dir.join("artifacts.json").exists());
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn scratch_symlink_cannot_promote_an_outside_file() {
        let fixture = Fixture::new();
        let scratch = fixture.ctx.scratch("generate").unwrap();
        let outside = fixture._dir.path().join("private.wav");
        std::fs::write(&outside, b"private bytes").unwrap();
        let path = scratch.path().join("looks-owned.wav");
        std::os::unix::fs::symlink(&outside, &path).unwrap();
        let mut receipt = pending("sfx", None);
        let error = retain(
            &fixture.ctx,
            &scratch,
            Delivery { outputs: vec![Output { path, kind: OutputKind::Audio, mime: "audio/wav" }], notes: None },
            &mut receipt,
            &Signal::new(),
        )
        .await
        .unwrap_err();
        assert_eq!(error.code(), "outside_root");
        assert_eq!(receipt["outputs"], json!([]));
        assert!(receipt["credits"]["actual"].is_null());
    }

    #[tokio::test]
    async fn notes_receipt_projects_only_authoritative_fields_and_xml_is_not_primary() {
        let fixture = Fixture::new();
        let scratch = fixture.ctx.scratch("generate").unwrap();
        let normalized = notes();
        let mut vendor = normalized.clone();
        vendor[0]["provider_url"] = json!("https://private.invalid/secret");
        let xml_path = scratch.path().join("score.musicxml");
        let xml = b"<?xml version=\"1.0\"?><score-partwise version=\"4.0\"><part-list/></score-partwise>";
        std::fs::write(&xml_path, xml).unwrap();
        let outputs = vec![
            Output { path: xml_path, kind: OutputKind::MusicXml, mime: "application/vnd.recordare.musicxml+xml" },
            notes_output(&scratch, &normalized),
        ];
        let mut receipt = pending("audio_to_midi", None);
        retain(&fixture.ctx, &scratch, Delivery { outputs, notes: Some(vendor) }, &mut receipt, &Signal::new()).await.unwrap();
        assert_eq!(receipt["output_type"], "notes");
        assert_eq!(receipt["notes"], normalized);
        assert!(!receipt.to_string().contains("private.invalid"));
        assert_eq!(std::fs::read(fixture.registered(&receipt["outputs"][0])).unwrap(), xml);
        fixture.registered(&receipt["outputs"][1]);
    }
}
