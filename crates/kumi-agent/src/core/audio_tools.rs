//! Session-owned audio model tools. File operations run away from the DAW; track insertion stays in its adapter.
use std::rc::Rc;

use async_trait::async_trait;
use kumi_ai::audio::{LocalProtocol, Selection, Service, Task};
use kumi_common::{
    abort::{Signal, SignalExt},
    js::json::stringify,
};
use kumi_types::{
    contracts::{AudioTranscriber, JsonObject, KernelTool, Reach, ToolResult},
    errors::RuntimeError,
};
use serde_json::{json, Value};

pub const TOOL: &str = "audio_models";
pub const GUIDANCE: &str = "Audio generation/transcription: use audio_models for text-to-sfx, video-to-sfx, extend, inpaint and file audio-to-midi. It works without a DAW. /audio governs these providers, not /model or /slots. Only run generation/edit/transcription for the producer's request, not speculative background work. Configuration is not permission to upload library/project audio for background analysis or groove listening; listen, including transcribe:true note analysis, stays local. provider:auto follows each task's settings: automatic Mirelo only after onboarding and with that task on, otherwise configured local; explicit mirelo cannot bypass a task being off. An environment key alone is not consent. MuScriptor is transcription only. Each Mirelo create spends the producer's credits: quote first and confirm the cost with the producer for anything beyond a single small request. Show each job's returned credits, or say when accounting is unavailable; never invent a cost. quote:true is preflight only (MIDI is an estimate): it may upload the input, but never creates/bills a generation job. job_id with the original task/provider only resumes polling/downloads, never creates or uploads; reuse it to recover an existing job instead of resubmitting. This is a file-output tool, not a DAW render or insertion tool. Return the saved output paths and provider metadata to the producer. MIDI note timings are seconds, not beats. Say Powered by Mirelo for Mirelo results. Do not silently substitute a built-in converter when a preferred provider fails; a built-in converter is only for an explicit request to use it.";

pub fn tool(service: Rc<Service>, notice: Rc<dyn Fn(String)>) -> Rc<dyn KernelTool> {
    Rc::new(AudioTool { service, notice })
}

/// A service exists even before onboarding; only configured providers need prompt guidance.
pub(super) async fn guidance(service: Option<&Service>) -> &'static str {
    let Some(service) = service else { return "" };
    match service.status().await {
        Ok(status)
            if status.config.tasks.values().any(|task| task.local.is_some())
                || (status.mirelo_configured
                    && status.config.acknowledged
                    && Task::ALL.into_iter().any(|task| status.config.task(task).auto_mirelo)) =>
        {
            GUIDANCE
        }
        _ => "",
    }
}

struct AudioTool {
    service: Rc<Service>,
    notice: Rc<dyn Fn(String)>,
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
        "Generate sound effects from text/video, extend or inpaint audio, or transcribe an audio file to MIDI. Native Mirelo (Powered by Mirelo) or configured self-hosted models; no DAW required. Returns saved local output files, provider/model/job/credit metadata and a bounded preview of MIDI notes in seconds. This tool does not render from or insert into a DAW. Only for user-requested work. Automatic Mirelo requires onboarding and the task on; explicit mirelo cannot bypass Off. Each Mirelo create spends the producer's credits: quote first and confirm the cost for anything beyond a single small request. Show each job's returned credits. quote:true may upload input but never creates/bills a generation job. job_id with the original task/provider only resumes GET polling/downloads without creating or uploading. Return saved output paths to the producer. For more notes, repeat task:audio-to-midi with notes_file:<returned notes_path>, offset and limit; this only reads saved notes inside the audio output folder and never retranscribes."
    }
    fn input_schema(&self) -> JsonObject {
        json!({"type":"object","additionalProperties":false,"required":["task"],"properties":{
            "task":{"type":"string","enum":["text-to-sfx","video-to-sfx","extend","inpaint","audio-to-midi"]},
            "provider":{"type":"string","enum":["auto","mirelo","local"],"default":"auto"},
            "prompt":{"type":"string","description":"The requested sound or edit"},
            "audio":{"type":"string","description":"Absolute local audio path or public URL; required for extend/inpaint/audio-to-midi"},
            "video":{"type":"string","description":"Absolute local video path or public URL for video-to-sfx"},
            "duration_ms":{"type":"integer","exclusiveMinimum":0,"description":"Requested generation duration for SFX; for audio-to-midi only a public URL's duration estimate. Local MIDI input is always measured for Mirelo/compatible providers; any supplied estimate is replaced. Native MuScriptor needs no duration."},
            "append_duration_ms":{"type":"integer","exclusiveMinimum":0},
            "region_start_ms":{"type":"integer","minimum":0},
            "region_end_ms":{"type":"integer","exclusiveMinimum":0},
            "model":{"type":"string","description":"Explicit supported model release, not a chat model"},
            "instruments":{"type":"array","items":{"type":"string"}},
            "quote":{"type":"boolean","default":false},
            "job_id":{"type":"string","description":"Resume an existing job with GET-only polling/downloads; use the original task and provider. Do not include generation inputs, quote or notes_file. Never creates or uploads. Native MuScriptor does not support resume."},
            "notes_file":{"type":"string","description":"Read a page of a previously returned notes_path inside Kumi's audio output folder; audio-to-midi only. Paths or symlinks escaping that folder are refused."},
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
            if input.contains_key("job_id") && input.keys().any(|key| !matches!(key.as_str(), "job_id" | "provider")) {
                return Err(RuntimeError::plain("job_id resumes only; do not mix it with generation inputs, quote or notes_file"));
            }
            if input.contains_key("notes_file") {
                if task != Task::AudioToMidi {
                    return Err(RuntimeError::plain("notes_file is only for audio-to-midi results"));
                }
                return read_notes(&input, self.service.output_dir(), &signal).await;
            }
            if input.contains_key("offset") || input.contains_key("limit") {
                return Err(RuntimeError::plain("offset and limit require notes_file"));
            }
            let provider = selection(&mut input)?;
            if task == Task::AudioToMidi {
                prepare_midi_input(&self.service, provider, &mut input, &signal).await?;
            }
            self.service.execute(task, Value::Object(input), provider, &signal, self.notice.as_ref()).await.map(compact)
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

async fn read_notes(input: &JsonObject, output_dir: &std::path::Path, signal: &Signal) -> Result<Value, RuntimeError> {
    signal.check()?;
    let path =
        input.get("notes_file").and_then(Value::as_str).ok_or_else(|| RuntimeError::plain("notes_file must be a local notes_path"))?;
    let path = std::path::Path::new(path);
    crate::core::plugin_tools::local_path(path).map_err(RuntimeError::plain)?;
    let root = tokio::fs::canonicalize(output_dir).await.map_err(|e| RuntimeError::plain(format!("Saved MIDI output folder: {e}")))?;
    let path = tokio::fs::canonicalize(path).await.map_err(|e| RuntimeError::plain(format!("Saved MIDI notes: {e}")))?;
    if path == root || !path.starts_with(&root) {
        return Err(RuntimeError::plain("notes_file must be inside the audio output folder; symlink escapes are not allowed"));
    }
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
    let file = tokio::fs::File::open(&path).await.map_err(|e| RuntimeError::plain(format!("Saved MIDI notes: {e}")))?;
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

async fn prepare_midi_input(service: &Service, provider: Selection, input: &mut JsonObject, signal: &Signal) -> Result<(), RuntimeError> {
    if input.contains_key("job_id") {
        return Ok(());
    }
    let selected = if provider == Selection::Auto { service.preferred(Task::AudioToMidi).await? } else { Some(provider) };
    let native = selected == Some(Selection::Local)
        && service.config().task(Task::AudioToMidi).local.is_some_and(|endpoint| endpoint.protocol == LocalProtocol::Muscriptor);
    if selected.is_some() && !native {
        measure_duration(input, signal).await?;
    }
    Ok(())
}

async fn measure_duration(input: &mut JsonObject, signal: &Signal) -> Result<(), RuntimeError> {
    let Some(audio) = input.get("audio").and_then(Value::as_str) else {
        return Ok(());
    };
    if audio.starts_with("https://") || audio.starts_with("http://") {
        return Ok(());
    }
    crate::core::plugin_tools::local_path(std::path::Path::new(audio)).map_err(RuntimeError::plain)?;
    // Use the same native file metadata reader as local listening before trying an installed probe.
    // This is measured from the file, never the model's duration_ms.
    let path = std::path::PathBuf::from(audio);
    signal.check()?;
    let seconds = tokio::task::spawn_blocking(move || kumi_listening::music::tags::read(&path, path.parent().unwrap_or(&path)).seconds)
        .await
        .map_err(|e| RuntimeError::plain(format!("Could not read audio metadata: {e}")))?;
    signal.check()?;
    let seconds = match seconds.filter(|seconds| seconds.is_finite() && *seconds > 0.) {
        Some(seconds) => seconds,
        None => {
            let ffmpeg = kumi_media::programs::find_ffmpeg(kumi_media::programs::FfmpegOptions {
                signal: Some(signal.clone()),
                installed_only: true,
                ..Default::default()
            })
            .await
            .map_err(|e| RuntimeError::plain(e.to_string()))?
            .ok_or_else(|| RuntimeError::plain("Could not measure local audio: no installed metadata probe. Install ffmpeg/ffprobe; a supplied duration cannot replace measurement."))?;
            let seconds = kumi_media::frames::duration_of(&ffmpeg, audio, Some(signal.clone())).await;
            signal.check()?;
            seconds
                .filter(|n| n.is_finite() && *n > 0.)
                .ok_or_else(|| RuntimeError::plain("Could not measure the local input duration; no quote or submission was made"))?
        }
    };
    input.insert("duration_ms".into(), json!((seconds * 1000.).ceil() as u64));
    Ok(())
}

struct Transcriber {
    service: Rc<Service>,
    notice: Rc<dyn Fn(String)>,
}

#[async_trait(?Send)]
impl AudioTranscriber for Transcriber {
    async fn preferred(&self) -> Result<bool, RuntimeError> {
        Ok(self.service.preferred(Task::AudioToMidi).await?.is_some())
    }
    async fn transcribe(&self, mut input: JsonObject, signal: Signal) -> Result<Option<Value>, RuntimeError> {
        if input.get("provider").and_then(Value::as_str) == Some("builtin") {
            return Ok(None);
        }
        let provider = selection(&mut input)?;
        if matches!(provider, Selection::Auto) && self.service.preferred(Task::AudioToMidi).await?.is_none() {
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
        self.service.execute(Task::AudioToMidi, Value::Object(input), provider, &signal, self.notice.as_ref()).await.map(Some)
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

/// Bind native converters only. Listening, including note analysis, stays local and unchanged.
pub fn route(inner: Rc<dyn KernelTool>, service: Rc<Service>, notice: Rc<dyn Fn(String)>) -> Rc<dyn KernelTool> {
    inner.with_audio_transcriber(Rc::new(Transcriber { service, notice })).unwrap_or(inner)
}

#[cfg(test)]
mod tests {
    use super::*;
    use kumi_ai::audio::{Config, Endpoint, ServiceOptions, TaskConfig};
    use std::cell::{Cell, RefCell};

    fn service(key: bool) -> (tempfile::TempDir, Rc<Service>, Rc<RefCell<Config>>) {
        let dir = tempfile::tempdir().unwrap();
        let config = Rc::new(RefCell::new(Config { acknowledged: key, ..Default::default() }));
        let read = config.clone();
        let service = Service::new(ServiceOptions {
            store: Rc::new(kumi_ai::auth::store::open_credential_store(dir.path().join("auth.json"))),
            env: if key { [("MIRELO_API_KEY".into(), "test-key".into())].into() } else { Default::default() },
            read_config: Rc::new(move || read.borrow().clone()),
            write_config: Rc::new(|_| Ok(())),
            output_dir: dir.path().join("output"),
            mirelo_base: None,
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
    async fn listening_and_note_analysis_stay_local_with_a_cloud_provider_configured() {
        let (_dir, service, config) = service(true);
        let inner: Rc<dyn KernelTool> = Rc::new(Listen(Cell::new(0)));
        let tool = route(inner.clone(), service, Rc::new(|_| panic!("Local analysis must not produce a billing notice")));
        assert!(Rc::ptr_eq(&inner, &tool), "analysis must keep its original tool, schema and streaming behavior");
        let run = |input: Value| tool.execute(input.as_object().unwrap().clone(), Signal::new());
        assert!(!run(json!({"file":"/not-opened.wav"})).await.unwrap().is_error);
        assert!(!run(json!({"transcribe":true})).await.unwrap().is_error);
        config.borrow_mut().tasks.insert(Task::AudioToMidi, TaskConfig { auto_mirelo: false, ..Default::default() });
        assert!(!run(json!({"transcribe":true})).await.unwrap().is_error);
    }

    #[tokio::test(flavor = "current_thread")]
    async fn guidance_requires_a_configured_provider_not_just_a_service() {
        assert!(guidance(None).await.is_empty());
        let (_dir, service, config) = service(false);
        assert!(guidance(Some(&service)).await.is_empty());
        config.borrow_mut().tasks.insert(
            Task::AudioToMidi,
            TaskConfig {
                auto_mirelo: false,
                local: Some(Endpoint { url: "http://127.0.0.1:1".into(), protocol: LocalProtocol::Muscriptor, model: None }),
                fallback: false,
            },
        );
        assert_eq!(guidance(Some(&service)).await, GUIDANCE);
        config.borrow_mut().tasks.clear();
        assert!(guidance(Some(&service)).await.is_empty());
        let (_cloud_dir, cloud, cloud_config) = self::service(true);
        assert_eq!(guidance(Some(&cloud)).await, GUIDANCE);
        cloud_config.borrow_mut().acknowledged = false;
        assert!(guidance(Some(&cloud)).await.is_empty(), "an environment key is not consent");
        cloud_config.borrow_mut().acknowledged = true;
        for task in Task::ALL {
            cloud_config.borrow_mut().tasks.insert(task, TaskConfig { auto_mirelo: false, ..Default::default() });
        }
        assert!(guidance(Some(&cloud)).await.is_empty(), "all tasks Off is not an operational cloud provider");
    }

    #[tokio::test(flavor = "current_thread")]
    async fn local_midi_duration_is_measured_even_when_the_model_supplies_a_short_quote() {
        let (dir, service, _) = service(true);
        let path = dir.path().join("two-seconds.wav");
        kumi_media::audio::decode::write_test_wav(&path, &vec![0.; 2000], 1000);
        for provider in [Selection::Auto, Selection::Mirelo] {
            for duration in [None, Some(1), Some(10_000)] {
                let mut input = json!({"audio":path,"quote":true}).as_object().unwrap().clone();
                if let Some(duration) = duration {
                    input.insert("duration_ms".into(), json!(duration));
                }
                prepare_midi_input(&service, provider, &mut input, &Signal::new()).await.unwrap();
                assert_eq!(input["duration_ms"], 2000);
            }
        }
        let mut remote = json!({"audio":"https://example.com/input.wav","duration_ms":1234}).as_object().unwrap().clone();
        prepare_midi_input(&service, Selection::Mirelo, &mut remote, &Signal::new()).await.unwrap();
        assert_eq!(remote["duration_ms"], 1234, "remote URL estimates cannot be measured locally");
        let mut unsafe_path = json!({"audio":"relative.wav","duration_ms":1}).as_object().unwrap().clone();
        assert!(prepare_midi_input(&service, Selection::Mirelo, &mut unsafe_path, &Signal::new()).await.is_err());
    }

    #[tokio::test(flavor = "current_thread")]
    async fn native_muscriptor_and_resume_do_not_require_a_generation_probe() {
        let (_dir, service, config) = service(false);
        config.borrow_mut().tasks.insert(
            Task::AudioToMidi,
            TaskConfig {
                auto_mirelo: false,
                local: Some(Endpoint { url: "http://127.0.0.1:1".into(), protocol: LocalProtocol::Muscriptor, model: None }),
                fallback: false,
            },
        );
        for provider in [Selection::Auto, Selection::Local] {
            let mut input = json!({"audio":"not-opened-by-a-probe.wav"}).as_object().unwrap().clone();
            prepare_midi_input(&service, provider, &mut input, &Signal::new()).await.unwrap();
            assert!(!input.contains_key("duration_ms"));
        }
        // The backend validates resume: preparation must never perform generation probing.
        let mut resume = json!({"job_id":"existing-job"}).as_object().unwrap().clone();
        prepare_midi_input(&service, Selection::Mirelo, &mut resume, &Signal::new()).await.unwrap();
        assert_eq!(resume, json!({"job_id":"existing-job"}).as_object().unwrap().clone());
        let tool = tool(service, Rc::new(|_| panic!("Rejected resume inputs must not bill")));
        assert!(tool.input_schema()["properties"]["job_id"].is_object());
        for extra in [
            json!({"audio":"not-opened.wav"}),
            json!({"duration_ms":1}),
            json!({"quote":true}),
            json!({"notes_file":"not-opened.json"}),
            json!({"prompt":"new sound"}),
        ] {
            let mut input = json!({"task":"audio-to-midi","job_id":"existing-job"}).as_object().unwrap().clone();
            input.extend(extra.as_object().unwrap().clone());
            let done = tool.execute(input, Signal::new()).await.unwrap();
            assert!(done.is_error);
            assert!(done.text.contains("job_id resumes only"), "{}", done.text);
        }
    }

    #[tokio::test(flavor = "current_thread")]
    async fn unset_transcriber_falls_back_without_opening_or_uploading_input() {
        let (_dir, service, _) = service(false);
        let transcriber = Transcriber { service, notice: Rc::new(|_| panic!("Local fallback must not bill")) };
        assert!(!transcriber.preferred().await.unwrap());
        assert!(transcriber
            .transcribe(json!({"audio":"/never-open-this"}).as_object().unwrap().clone(), Signal::new())
            .await
            .unwrap()
            .is_none());
    }

    #[tokio::test(flavor = "current_thread")]
    async fn unacknowledged_key_and_task_off_preserve_local_conversion_without_reading_input() {
        let (_dir, service, config) = service(true);
        let transcriber = Transcriber { service, notice: Rc::new(|_| panic!("Disabled cloud routing must not bill")) };
        assert!(transcriber.preferred().await.unwrap());
        for acknowledged in [false, true] {
            config.borrow_mut().acknowledged = acknowledged;
            config.borrow_mut().tasks.insert(Task::AudioToMidi, TaskConfig { auto_mirelo: !acknowledged, ..Default::default() });
            assert!(!transcriber.preferred().await.unwrap());
            let input = json!({"audio":"not-opened.wav","provider":"auto"}).as_object().unwrap().clone();
            assert!(transcriber.transcribe(input, Signal::new()).await.unwrap().is_none());
        }
    }

    #[tokio::test(flavor = "current_thread")]
    async fn provider_tool_rejects_unknown_task_or_selection_before_dispatch() {
        let (_dir, service, _) = service(false);
        let tool = tool(service, Rc::new(|_| panic!("Invalid inputs must not bill")));
        for args in [json!({"task":"other"}), json!({"task":"text-to-sfx","provider":"builtin"})] {
            assert!(tool.execute(args.as_object().unwrap().clone(), Signal::new()).await.unwrap().is_error);
        }
    }

    #[tokio::test(flavor = "current_thread")]
    async fn notes_are_bounded_and_paged_without_another_generation() {
        let (_dir, service, _) = service(false);
        let notes: Vec<Value> = (0..150).map(|i| json!({"pitch":60,"start":i,"end":i+1,"velocity":80})).collect();
        std::fs::create_dir_all(service.output_dir()).unwrap();
        let path = service.output_dir().join("notes.json");
        std::fs::write(&path, serde_json::to_vec(&notes).unwrap()).unwrap();
        let shown = compact(json!({"provider":"mirelo","notes":notes,"notes_path":path,"midi_path":"/saved.mid","credits":3}));
        assert_eq!(shown["notes"].as_array().unwrap().len(), 64);
        assert_eq!(shown["note_count"], 150);
        assert_eq!(shown["next_offset"], 64);
        assert_eq!(shown["midi_path"], "/saved.mid");
        assert_eq!(shown["credits"], 3);
        let tool = tool(service.clone(), Rc::new(|_| panic!("Reading saved notes must not bill")));
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
            assert!(read_notes(input.as_object().unwrap(), service.output_dir(), &Signal::new()).await.is_err());
        }
    }

    #[tokio::test(flavor = "current_thread")]
    async fn notes_file_rejects_outside_files_and_parent_traversal() {
        let (dir, service, _) = service(false);
        std::fs::create_dir_all(service.output_dir()).unwrap();
        let outside = dir.path().join("private.json");
        std::fs::write(&outside, br#"[{"private":"do not disclose"}]"#).unwrap();
        for path in [outside, service.output_dir().join("../private.json")] {
            let input = json!({"notes_file":path});
            let error = read_notes(input.as_object().unwrap(), service.output_dir(), &Signal::new()).await.unwrap_err();
            assert!(error.to_string().contains("inside the audio output folder"));
            assert!(!error.to_string().contains("do not disclose"));
        }
    }

    #[cfg(unix)]
    #[tokio::test(flavor = "current_thread")]
    async fn notes_file_rejects_file_and_directory_symlink_escapes() {
        let (dir, service, _) = service(false);
        std::fs::create_dir_all(service.output_dir()).unwrap();
        let private = dir.path().join("private");
        std::fs::create_dir(&private).unwrap();
        std::fs::write(private.join("notes.json"), b"[]").unwrap();
        let file_link = service.output_dir().join("file.json");
        let directory_link = service.output_dir().join("folder");
        std::os::unix::fs::symlink(private.join("notes.json"), &file_link).unwrap();
        std::os::unix::fs::symlink(&private, &directory_link).unwrap();
        for path in [file_link, directory_link.join("notes.json")] {
            let input = json!({"notes_file":path});
            let error = read_notes(input.as_object().unwrap(), service.output_dir(), &Signal::new()).await.unwrap_err();
            assert!(error.to_string().contains("symlink escapes"));
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
