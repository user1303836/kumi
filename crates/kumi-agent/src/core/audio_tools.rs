//! Session-owned audio model tools. File operations run away from the DAW; track insertion stays in its adapter.
use std::rc::Rc;

use async_trait::async_trait;
use kumi_ai::audio::{Selection, Service, Task};
use kumi_common::{
    abort::{Signal, SignalExt},
    js::json::stringify,
};
use kumi_types::{
    contracts::{AudioTranscriber, JsonObject, KernelTool, Reach, ToolResult, ToolRole},
    errors::RuntimeError,
};
use serde_json::{json, Value};

pub const TOOL: &str = "audio_models";
pub const GUIDANCE: &str = "Audio generation/transcription: use audio_models for text-to-sfx, video-to-sfx, extend, inpaint and file audio-to-midi. It works without a DAW. /audio governs these providers, not /model or /slots. Only run generation/edit/transcription for the producer's request, not speculative background work. Configuration is not permission to upload library/project audio for background analysis or groove listening. provider:auto follows each task's settings: configured automatic Mirelo first, otherwise configured local; an explicitly requested provider wins. MuScriptor is transcription only. quote:true is preflight only (MIDI is an estimate): it may upload the input, but never creates/bills a generation job. Keep returned files: use the current DAW's file-import tool schema to place audio, never invent paths or cross-DAW commands. For track transcription use audio_to_midi where offered (it renders and inserts provider notes). Otherwise render the source audio track with render {track,from_beat,beats}, then audio_models task:audio-to-midi with the returned file. For processed audio/instruments, bounce or resample first; render's raw source excludes effects. Write returned notes using the current DAW's note-writing tools; seconds are not beats: at constant tempo beats=seconds*BPM/60, plus the destination position only once. With tempo changes, map seconds through the project's tempo map; never treat average-tempo placement as exact. Preserve provider metadata and say Powered by Mirelo for Mirelo results. Do not silently substitute a built-in converter when a preferred provider fails; provider:builtin on the native conversion tool is only for an explicit request to use the built-in converter.";

pub fn tool(service: Rc<Service>) -> Rc<dyn KernelTool> {
    Rc::new(AudioTool { service })
}

struct AudioTool {
    service: Rc<Service>,
}

fn selection(input: &mut JsonObject) -> Result<Selection, RuntimeError> {
    serde_json::from_value(input.remove("provider").unwrap_or_else(|| json!("auto")))
        .map_err(|_| RuntimeError::plain("provider must be auto, mirelo or local"))
}

#[async_trait(?Send)]
impl KernelTool for AudioTool {
    fn name(&self) -> &str {
        TOOL
    }
    fn description(&self) -> &str {
        "Generate sound effects from text/video, extend or inpaint audio, or transcribe an audio file to MIDI. Native Mirelo (Powered by Mirelo) or configured self-hosted models; no DAW required. Returns saved local output files, provider/model/job/credit metadata and a bounded preview of MIDI notes in seconds. Only for user-requested work. Auto routing follows independent task settings; explicit mirelo/local overrides them. quote:true may upload input but never creates/bills a generation job. Import output files with the DAW's existing import tools. For more notes, repeat task:audio-to-midi with notes_file:<returned notes_path>, offset and limit; this only reads the saved notes and never retranscribes."
    }
    fn input_schema(&self) -> JsonObject {
        json!({"type":"object","additionalProperties":false,"required":["task"],"properties":{
            "task":{"type":"string","enum":["text-to-sfx","video-to-sfx","extend","inpaint","audio-to-midi"]},
            "provider":{"type":"string","enum":["auto","mirelo","local"],"default":"auto"},
            "prompt":{"type":"string","description":"The requested sound or edit"},
            "audio":{"type":"string","description":"Absolute local audio path or public URL; required for extend/inpaint/audio-to-midi"},
            "video":{"type":"string","description":"Absolute local video path or public URL for video-to-sfx"},
            "duration_ms":{"type":"integer","exclusiveMinimum":0,"description":"Requested generation duration; audio-to-midi estimate (local files are measured when omitted)"},
            "append_duration_ms":{"type":"integer","exclusiveMinimum":0},
            "region_start_ms":{"type":"integer","minimum":0},
            "region_end_ms":{"type":"integer","exclusiveMinimum":0},
            "model":{"type":"string","description":"Explicit supported model release, not a chat model"},
            "instruments":{"type":"array","items":{"type":"string"}},
            "quote":{"type":"boolean","default":false},
            "notes_file":{"type":"string","description":"Read a page of a previously returned notes_path instead of transcribing; audio-to-midi only"},
            "offset":{"type":"integer","minimum":0,"default":0},
            "limit":{"type":"integer","minimum":1,"maximum":128,"default":64}
        }}).as_object().unwrap().clone()
    }
    async fn execute(&self, mut input: JsonObject, signal: Signal) -> Result<ToolResult, RuntimeError> {
        let result = async {
            let task = input
                .remove("task")
                .and_then(|v| v.as_str().and_then(Task::parse))
                .ok_or_else(|| RuntimeError::plain("task must be text-to-sfx, video-to-sfx, extend, inpaint or audio-to-midi"))?;
            if input.contains_key("notes_file") {
                if task != Task::AudioToMidi {
                    return Err(RuntimeError::plain("notes_file is only for audio-to-midi results"));
                }
                return read_notes(&input, &signal).await;
            }
            if input.contains_key("offset") || input.contains_key("limit") {
                return Err(RuntimeError::plain("offset and limit require notes_file"));
            }
            let provider = selection(&mut input)?;
            if task == Task::AudioToMidi && !input.contains_key("duration_ms") {
                measure_duration(&mut input, &signal).await?;
            }
            self.service.execute(task, Value::Object(input), provider, &signal).await.map(compact)
        }
        .await;
        Ok(match result {
            Ok(value) => ToolResult::text(stringify(&value)),
            Err(error) => ToolResult { text: error.to_string(), is_error: true, ..Default::default() },
        })
    }
    fn reach(&self) -> Reach {
        Reach::AwayFromDaw
    }
    fn deferred(&self) -> bool {
        true
    }
}

/// Keep successful output paths and accounting visible even for a long transcription.
pub(super) fn compact(mut result: Value) -> Value {
    if let Some(object) = result.as_object_mut() {
        if let Some(Value::Array(mut notes)) = object.remove("notes") {
            let count = notes.len();
            notes.truncate(64);
            object.insert("note_count".into(), json!(count));
            object.insert("notes".into(), Value::Array(notes));
            if count > 64 {
                object.insert("next_offset".into(), json!(64));
                object.insert("notes_help".into(), json!("Read remaining notes with audio_models task:audio-to-midi, notes_file:notes_path, offset:next_offset. Times are seconds."));
            }
        }
    }
    result
}

async fn read_notes(input: &JsonObject, signal: &Signal) -> Result<Value, RuntimeError> {
    signal.check()?;
    let path =
        input.get("notes_file").and_then(Value::as_str).ok_or_else(|| RuntimeError::plain("notes_file must be a local notes_path"))?;
    let path = std::path::Path::new(path);
    crate::core::plugin_tools::local_path(path).map_err(RuntimeError::plain)?;
    let integer = |name: &str, default: u64| -> Result<u64, RuntimeError> {
        input
            .get(name)
            .map_or(Ok(default), |v| v.as_u64().ok_or_else(|| RuntimeError::plain(format!("{name} must be a nonnegative integer"))))
    };
    let offset = integer("offset", 0)?;
    let limit = integer("limit", 64)?;
    if !(1..=128).contains(&limit) {
        return Err(RuntimeError::plain("limit must be 1..128"));
    }
    let file = tokio::fs::File::open(path).await.map_err(|e| RuntimeError::plain(format!("Saved MIDI notes: {e}")))?;
    use tokio::io::AsyncReadExt;
    let mut bytes = Vec::new();
    file.take(64 * 1024 * 1024 + 1).read_to_end(&mut bytes).await.map_err(|e| RuntimeError::plain(format!("Saved MIDI notes: {e}")))?;
    if bytes.len() > 64 * 1024 * 1024 {
        return Err(RuntimeError::plain("Saved MIDI notes exceed 64 MiB"));
    }
    signal.check()?;
    let notes: Vec<Value> =
        serde_json::from_slice(&bytes).map_err(|e| RuntimeError::plain(format!("Saved MIDI notes are not a note array: {e}")))?;
    let count = notes.len();
    let offset = usize::try_from(offset).map_err(|_| RuntimeError::plain("offset is too large"))?;
    let page: Vec<Value> = notes.into_iter().skip(offset).take(limit as usize).collect();
    let next = offset.saturating_add(page.len());
    Ok(
        json!({"notes_path":path,"note_count":count,"offset":offset,"notes":page,"next_offset":if next < count {Some(next)} else {None},"time_unit":"seconds"}),
    )
}

async fn measure_duration(input: &mut JsonObject, signal: &Signal) -> Result<(), RuntimeError> {
    let Some(audio) = input.get("audio").and_then(Value::as_str) else {
        return Ok(());
    };
    if audio.starts_with("https://") || audio.starts_with("http://") {
        return Ok(());
    }
    crate::core::plugin_tools::local_path(std::path::Path::new(audio)).map_err(RuntimeError::plain)?;
    let ffmpeg = kumi_media::programs::find_ffmpeg(kumi_media::programs::FfmpegOptions {
        signal: Some(signal.clone()),
        installed_only: true,
        ..Default::default()
    })
    .await
    .map_err(|e| RuntimeError::plain(e.to_string()))?
    .ok_or_else(|| RuntimeError::plain("No installed audio metadata probe; supply duration_ms (an estimate for the quote)"))?;
    let seconds = kumi_media::frames::duration_of(&ffmpeg, audio, Some(signal.clone())).await;
    signal.check()?;
    let seconds = seconds
        .filter(|n| n.is_finite() && *n > 0.)
        .ok_or_else(|| RuntimeError::plain("Could not measure the input duration; supply duration_ms"))?;
    input.insert("duration_ms".into(), json!((seconds * 1000.).ceil() as u64));
    Ok(())
}

struct Transcriber(Rc<Service>);

#[async_trait(?Send)]
impl AudioTranscriber for Transcriber {
    async fn preferred(&self) -> Result<bool, RuntimeError> {
        Ok(self.0.preferred(Task::AudioToMidi).await?.is_some())
    }
    async fn transcribe(&self, mut input: JsonObject, signal: Signal) -> Result<Option<Value>, RuntimeError> {
        if input.get("provider").and_then(Value::as_str) == Some("builtin") {
            return Ok(None);
        }
        let provider = selection(&mut input)?;
        if matches!(provider, Selection::Auto) && self.0.preferred(Task::AudioToMidi).await?.is_none() {
            return Ok(None);
        }
        let start = input.remove("audio_start_seconds").and_then(|v| v.as_f64()).unwrap_or(0.);
        let seconds = input
            .remove("audio_duration_seconds")
            .and_then(|v| v.as_f64())
            .ok_or_else(|| RuntimeError::plain("Rendered transcription needs its measured duration"))?;
        let audio =
            input.get("audio").and_then(Value::as_str).ok_or_else(|| RuntimeError::plain("Rendered transcription needs its audio file"))?;
        let (folder, duration_ms) = rendered_audio(audio, start, seconds, &signal).await?;
        input.insert("audio".into(), json!(folder.path().join("transcription.wav")));
        input.insert("duration_ms".into(), json!(duration_ms));
        self.0.execute(Task::AudioToMidi, Value::Object(input), provider, &signal).await.map(Some)
    }
}

async fn rendered_audio(audio: &str, start: f64, seconds: f64, signal: &Signal) -> Result<(tempfile::TempDir, u64), RuntimeError> {
    if !start.is_finite() || start < 0. || !seconds.is_finite() || seconds <= 0. || seconds > 1800. {
        return Err(RuntimeError::plain("Transcription needs a finite render offset and 0..1800 seconds of audio"));
    }
    // Render.start is a file offset, not the destination timeline. Never upload the surrounding project audio.
    let folder = tempfile::tempdir().map_err(|e| RuntimeError::plain(format!("Audio transcription scratch folder: {e}")))?;
    let mut source = kumi_media::audio::decode::open_audio_to(audio, Some(signal.clone()), Some(start + seconds))
        .await
        .map_err(|e| RuntimeError::plain(e.0))?;
    let copied: Result<u64, RuntimeError> = async {
        use tokio::io::{AsyncSeekExt, AsyncWriteExt};
        let rate = source.sample_rate;
        let channels = source.channels.min(2);
        if !rate.is_finite() || rate <= 0. || channels == 0 {
            return Err(RuntimeError::plain("The rendered transcription has invalid audio metadata"));
        }
        let wanted = (seconds * rate) as usize;
        let bytes_per_frame = channels * 4;
        if wanted > (u32::MAX as usize - 36) / bytes_per_frame {
            return Err(RuntimeError::plain("The rendered transcription exceeds WAV's file size limit"));
        }
        let empty = [Vec::new(), Vec::new()];
        let mut header = kumi_media::audio::decode::float_wav(&empty[..channels], rate as u32);
        let mut output =
            tokio::fs::File::create(folder.path().join("transcription.wav")).await.map_err(|e| RuntimeError::plain(e.to_string()))?;
        output.write_all(&header).await.map_err(|e| RuntimeError::plain(e.to_string()))?;
        source.seek(start * rate);
        let mut frames = 0;
        let mut bytes = Vec::with_capacity(8192 * bytes_per_frame);
        while frames < wanted {
            signal.check()?;
            let Some(block) = source.read((wanted - frames).min(8192)).await.map_err(|e| RuntimeError::plain(e.0))? else {
                break;
            };
            let count = block.first().map_or(0, Vec::len);
            if count == 0 {
                break;
            }
            bytes.clear();
            for frame in 0..count {
                for channel in &block[..channels] {
                    bytes.extend_from_slice(&channel[frame].to_le_bytes());
                }
            }
            output.write_all(&bytes).await.map_err(|e| RuntimeError::plain(e.to_string()))?;
            frames += count;
        }
        if frames == 0 {
            return Err(RuntimeError::plain("The rendered transcription stretch is empty"));
        }
        let size = (frames * bytes_per_frame) as u32;
        header[4..8].copy_from_slice(&(36 + size).to_le_bytes());
        header[40..44].copy_from_slice(&size.to_le_bytes());
        output.seek(std::io::SeekFrom::Start(0)).await.map_err(|e| RuntimeError::plain(e.to_string()))?;
        output.write_all(&header).await.map_err(|e| RuntimeError::plain(e.to_string()))?;
        output.flush().await.map_err(|e| RuntimeError::plain(e.to_string()))?;
        Ok((frames as f64 / rate * 1000.).ceil() as u64)
    }
    .await;
    let closed = source.close().await.map_err(|e| RuntimeError::plain(e.0));
    let duration_ms = copied?;
    closed?;
    Ok((folder, duration_ms))
}

/// Bind only native converters; unrelated tools retain identity, streaming and metadata unchanged.
pub fn route(inner: Rc<dyn KernelTool>, service: Rc<Service>) -> Rc<dyn KernelTool> {
    if let Some(tool) = inner.with_audio_transcriber(Rc::new(Transcriber(service.clone()))) {
        return tool;
    }
    if inner.name() == "listen" {
        let description = format!("{} Preferred audio-to-MIDI provider routing is enforced. For an explicitly requested built-in conversion only, set provider:builtin; otherwise use audio_models with rendered audio when redirected.", inner.description());
        Rc::new(ConversionGuard { inner, service, description })
    } else {
        inner
    }
}

struct ConversionGuard {
    inner: Rc<dyn KernelTool>,
    service: Rc<Service>,
    description: String,
}

#[async_trait(?Send)]
impl KernelTool for ConversionGuard {
    fn name(&self) -> &str {
        self.inner.name()
    }
    fn description(&self) -> &str {
        &self.description
    }
    fn input_schema(&self) -> JsonObject {
        let mut schema = self.inner.input_schema();
        if let Some(properties) = schema.get_mut("properties").and_then(Value::as_object_mut) {
            properties.insert("provider".into(), json!({"type":"string","enum":["auto","mirelo","local","builtin"],"description":"Only builtin explicitly bypasses configured audio-to-MIDI routing"}));
        }
        schema
    }
    async fn execute(&self, mut input: JsonObject, signal: Signal) -> Result<ToolResult, RuntimeError> {
        let is_conversion = input.get("transcribe") == Some(&Value::Bool(true));
        let provider = input.remove("provider").unwrap_or_else(|| json!("auto"));
        if !matches!(provider.as_str(), Some("auto" | "mirelo" | "local" | "builtin")) {
            return Ok(ToolResult { text: "provider must be auto, mirelo, local or builtin".into(), is_error: true, ..Default::default() });
        }
        if is_conversion && provider != "builtin" {
            let preferred = self.service.preferred(Task::AudioToMidi).await?;
            if provider != "auto" || preferred.is_some() {
                return Ok(ToolResult {
                    text: stringify(&json!({
                        "error":"Use the selected audio-to-MIDI provider, not the built-in converter. No conversion was performed.",
                        "provider":if provider == "auto" { serde_json::to_value(preferred).unwrap_or(Value::Null) } else { provider },
                        "next":"Use render {track,from_beat,beats} for the source audio (bounce/resample instruments or effects first), then audio_models {task:'audio-to-midi',audio:<returned file>,provider:<selected provider>}. For a file, pass its path directly. Insert returned seconds-based notes with the DAW's discovered note-writing schema, converting seconds with the project tempo map. Do not invent an import-MIDI API."
                    })),
                    is_error: true,
                    ..Default::default()
                });
            }
        }
        self.inner.execute(input, signal).await
    }
    fn reach(&self) -> Reach {
        self.inner.reach()
    }
    fn deferred(&self) -> bool {
        self.inner.deferred()
    }
    fn listed_by(&self) -> Option<&str> {
        self.inner.listed_by()
    }
    fn role(&self) -> Option<ToolRole> {
        self.inner.role()
    }
    fn concurrent(&self) -> bool {
        self.inner.concurrent()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use kumi_ai::audio::{Config, ServiceOptions, TaskConfig};
    use std::cell::{Cell, RefCell};

    fn service(key: bool) -> (tempfile::TempDir, Rc<Service>, Rc<RefCell<Config>>) {
        let dir = tempfile::tempdir().unwrap();
        let config = Rc::new(RefCell::new(Config::default()));
        let read = config.clone();
        let service = Service::new(ServiceOptions {
            store: Rc::new(kumi_ai::auth::store::open_credential_store(dir.path().join("auth.json"))),
            env: if key { [("MIRELO_API_KEY".into(), "test-key".into())].into() } else { Default::default() },
            read_config: Rc::new(move || read.borrow().clone()),
            write_config: Rc::new(|_| Ok(())),
            output_dir: dir.path().join("output"),
        });
        (dir, service, config)
    }

    struct Listen(Cell<usize>);
    #[async_trait(?Send)]
    impl KernelTool for Listen {
        fn name(&self) -> &str {
            "listen"
        }
        fn description(&self) -> &str {
            "Listen to a file"
        }
        fn input_schema(&self) -> JsonObject {
            json!({"type":"object","properties":{}}).as_object().unwrap().clone()
        }
        async fn execute(&self, input: JsonObject, _: Signal) -> Result<ToolResult, RuntimeError> {
            assert!(!input.contains_key("provider"));
            self.0.set(self.0.get() + 1);
            Ok(ToolResult::text("heard"))
        }
        fn reach(&self) -> Reach {
            Reach::AwayFromDaw
        }
    }

    #[tokio::test(flavor = "current_thread")]
    async fn listening_routes_only_transcription_and_observes_live_task_settings() {
        let (_dir, service, config) = service(true);
        let inner = Rc::new(Listen(Cell::new(0)));
        let tool = route(inner.clone(), service);
        let run = |input: Value| tool.execute(input.as_object().unwrap().clone(), Signal::new());
        assert!(!run(json!({"file":"/not-opened.wav"})).await.unwrap().is_error);
        assert!(run(json!({"transcribe":true})).await.unwrap().is_error);
        assert_eq!(inner.0.get(), 1, "cloud default must not run builtin");
        assert!(!run(json!({"transcribe":true,"provider":"builtin"})).await.unwrap().is_error);
        config.borrow_mut().tasks.insert(Task::AudioToMidi, TaskConfig { auto_mirelo: false, ..Default::default() });
        assert!(!run(json!({"transcribe":true})).await.unwrap().is_error);
        assert!(run(json!({"transcribe":true,"provider":"local"})).await.unwrap().is_error);
        assert_eq!(inner.0.get(), 3, "explicit provider must not silently run builtin");
    }

    #[tokio::test(flavor = "current_thread")]
    async fn unset_transcriber_falls_back_without_opening_or_uploading_input() {
        let (_dir, service, _) = service(false);
        let transcriber = Transcriber(service);
        assert!(!transcriber.preferred().await.unwrap());
        assert!(transcriber
            .transcribe(json!({"audio":"/never-open-this"}).as_object().unwrap().clone(), Signal::new())
            .await
            .unwrap()
            .is_none());
    }

    #[tokio::test(flavor = "current_thread")]
    async fn provider_tool_rejects_unknown_task_or_selection_before_dispatch() {
        let (_dir, service, _) = service(false);
        let tool = tool(service);
        for args in [json!({"task":"other"}), json!({"task":"text-to-sfx","provider":"builtin"})] {
            assert!(tool.execute(args.as_object().unwrap().clone(), Signal::new()).await.unwrap().is_error);
        }
    }

    #[tokio::test(flavor = "current_thread")]
    async fn notes_are_bounded_and_paged_without_another_generation() {
        let (dir, service, _) = service(false);
        let notes: Vec<Value> = (0..150).map(|i| json!({"pitch":60,"start":i,"end":i+1,"velocity":80})).collect();
        let path = dir.path().join("notes.json");
        std::fs::write(&path, serde_json::to_vec(&notes).unwrap()).unwrap();
        let shown = compact(json!({"provider":"mirelo","notes":notes,"notes_path":path,"midi_path":"/saved.mid","credits":3}));
        assert_eq!(shown["notes"].as_array().unwrap().len(), 64);
        assert_eq!(shown["note_count"], 150);
        assert_eq!(shown["next_offset"], 64);
        assert_eq!(shown["midi_path"], "/saved.mid");
        assert_eq!(shown["credits"], 3);
        let tool = tool(service);
        let done = tool
            .execute(json!({"task":"audio-to-midi","notes_file":path,"offset":128,"limit":128}).as_object().unwrap().clone(), Signal::new())
            .await
            .unwrap();
        assert!(!done.is_error, "{}", done.text);
        let page: Value = serde_json::from_str(&done.text).unwrap();
        assert_eq!(page["notes"].as_array().unwrap().len(), 22);
        assert_eq!(page["notes"][0]["start"], 128);
        assert!(page["next_offset"].is_null());
        for input in [json!({"notes_file":"relative.json"}), json!({"notes_file":path,"limit":129}), json!({"notes_file":path,"offset":-1})]
        {
            assert!(read_notes(input.as_object().unwrap(), &Signal::new()).await.is_err());
        }
    }

    #[tokio::test(flavor = "current_thread")]
    async fn uploaded_render_excludes_audio_before_and_after_the_selected_stretch() {
        let dir = tempfile::tempdir().unwrap();
        let source = dir.path().join("source.wav");
        let samples: Vec<f32> = (0..3000).map(|i| if (1000..2000).contains(&i) { 0.25 } else { -0.5 }).collect();
        kumi_media::audio::decode::write_test_wav(&source, &samples, 1000);
        let signal = Signal::new();
        let (cropped, duration) = rendered_audio(source.to_str().unwrap(), 1., 1., &signal).await.unwrap();
        assert_eq!(duration, 1000);
        let path = cropped.path().join("transcription.wav");
        let (channels, rate) = kumi_media::audio::decode::read_stereo(&path, &signal, 0., 10.).await.unwrap();
        assert_eq!(rate, 1000.);
        assert_eq!(channels[0].len(), 1000);
        assert!(channels[0].iter().all(|v| (*v - 0.25).abs() < 0.001));
        assert!(rendered_audio(source.to_str().unwrap(), -1., 1., &signal).await.is_err());
    }
}
