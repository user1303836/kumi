//! Initialized verified presets remain private artifacts until a separate LibraryWrite and DAW change.

use super::ctx::{Call, Ctx, Effect, OpError, OpFuture, Operation};
use kumi_common::abort::Signal;
use kumi_library::plugins::{
    formats::structure,
    inventory,
    preset::{
        review::{self, ReviewError, ReviewedPreset},
        Edit, WriteOptions,
    },
};
use kumi_types::daw::DawId;
use serde::Deserialize;
use serde_json::{json, Value};
use std::path::PathBuf;

pub(super) const LIVE_ONLY: &str = "Plug-in presets are Live-only for now: Kumi builds and places them only in Ableton Live.";

pub static PRESET_BUILD: Operation = Operation {
    name: "preset_build",
    description: "Build a private preset artifact for Ableton Live from a verified initialized Serum 2, Pro-Q 4 or Ozone 12 template and typed edits. Reads installed VST3 version metadata in standard plug-in folders without executing a plug-in. No input file, bytes, download or requested-version override. This does not place a file in the User Library or change the DAW; those are separate approved steps. Values are stored preset values, not necessarily the plug-in's displayed units. Generated presets are Live-only for now.",
    schema,
    effect: |_| Effect::Read,
    run,
};

fn schema() -> Value {
    json!({"type":"object","additionalProperties":false,"required":["plugin","name","edits"],"properties":{
        "plugin":{"type":"string","enum":review::PLUGINS},
        "class":{"type":"string","pattern":"^[0-9A-Fa-f]{32}$","description":"Catalog VST3 processor class; omitted uses the product's initialized default."},
        "name":{"type":"string","minLength":1,"maxLength":80},
        "edits":{"type":"array","maxItems":review::MAX_SETTINGS,"items":{
            "type":"object","additionalProperties":false,"required":["path","value"],"properties":{
                "path":{"type":"string","minLength":1,"maxLength":256,"description":"A writable catalog control path. Serum $add_distortion takes rack 0, 1 or 2; Ozone $chain takes module names."},
                "value":{"oneOf":[{"type":"number"},{"type":"boolean"},{"type":"string","maxLength":80},
                    {"type":"array","maxItems":16,"items":{"type":"string","maxLength":40}}]}
            }
        }}
    }})
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Request {
    plugin: String,
    name: String,
    class: Option<String>,
    edits: Vec<Edit>,
}

fn run(ctx: &Ctx, call: Call) -> OpFuture<'_> {
    Box::pin(build(ctx, call, inventory::standard_roots()))
}

async fn build(ctx: &Ctx, call: Call, roots: Vec<PathBuf>) -> Result<Value, OpError> {
    if ctx.daw() != DawId::Ableton {
        return Err(refused("unsupported", LIVE_ONLY));
    }
    let request = serde_json::from_value(Value::Object(call.args)).map_err(|_| OpError::Invalid {
        reason: "Use plugin, name and typed edits from the plug-in catalog; no file, bytes or version override.".into(),
    })?;
    let signal = call.signal.clone();
    let preset = tokio::task::spawn_blocking(move || initialized(request, &roots, &signal))
        .await
        .map_err(|_| refused("failed", "The local preset writer stopped before publishing a file."))??;
    let (saved, metadata) = ctx.artifacts().publish_preset(ctx, preset, &call.signal).await?;
    Ok(json!({
        "state":"built", "artifact_id":saved.id, "plugin":metadata.plugin, "class":metadata.class,
        "version":metadata.version, "format":metadata.format, "digest":metadata.digest,
        "name":metadata.name, "settings":metadata.settings, "values_note":review::VALUE_NOTE,
    }))
}

fn initialized(request: Request, roots: &[PathBuf], signal: &Signal) -> Result<ReviewedPreset, OpError> {
    check(signal)?;
    if !review::PLUGINS.contains(&request.plugin.as_str()) {
        return Err(refused("unsupported", "Use a verified Serum 2, Pro-Q 4 or Ozone 12 writer; Vital isn't supported."));
    }
    let format = structure::builtin(&request.plugin).ok_or_else(|| refused("unsupported", "The built-in plug-in format is missing."))?;
    let class = request
        .class
        .as_deref()
        .or_else(|| format.info.ids.vst3.first().map(|class| class.id.as_str()))
        .ok_or_else(|| refused("unsupported", "This plug-in has no known VST3 processor class."))?;
    let installed = inventory::installed_vst3(roots, &request.plugin, class)
        .map_err(|error| OpError::Refused { code: "unverified_install", reason: error.to_string(), owner: None })?
        .ok_or_else(|| {
            refused(
                "not_installed",
                "This VST3 isn't installed in a standard plug-in folder. Install or enable it before building its preset.",
            )
        })?;
    if installed.installed_version_verified != Some(true) || !installed.create_preset {
        return Err(refused("unverified_version", "The installed VST3 version isn't covered by Kumi's verified writer, or its version couldn't be read. Use its exposed DAW parameters instead."));
    }
    let version = installed.version.ok_or_else(|| refused("unverified_version", "Kumi couldn't read the installed VST3 version."))?;
    check(signal)?;
    let options = WriteOptions { version, class_id: Some(class.to_uppercase()), name: Some(request.name) };
    let result = review::build(&request.plugin, &options, &request.edits).map_err(|error| {
        let code = if matches!(error, ReviewError::TooLong) { "too_long" } else { "invalid_preset" };
        OpError::Refused { code, reason: error.to_string(), owner: None }
    })?;
    check(signal)?;
    Ok(result)
}

fn check(signal: &Signal) -> Result<(), OpError> {
    if signal.is_cancelled() {
        Err(refused("cancelled", "Preset building was cancelled before publication."))
    } else {
        Ok(())
    }
}
fn refused(code: &'static str, reason: &str) -> OpError {
    OpError::Refused { code, reason: reason.into(), owner: None }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::serve::{
        ctx::{Host, Progress},
        Args,
    };
    use kumi_types::system::Env;
    use std::{collections::BTreeMap, rc::Rc};

    struct Fixture {
        ctx: Rc<Ctx>,
        root: PathBuf,
        _dir: tempfile::TempDir,
    }
    impl Fixture {
        fn new(daw: DawId) -> Self {
            let dir = tempfile::tempdir().unwrap();
            let root = dir.path().join("VST3");
            std::fs::create_dir(&root).unwrap();
            let ctx = Ctx::new(
                Args {
                    state_dir: dir.path().join("state"),
                    kumi_home: dir.path().join("home"),
                    hermes_home: None,
                    hermes_root: None,
                    daw,
                    library_roots: vec![],
                    media_roots: vec![],
                    tools: BTreeMap::new(),
                },
                Env::new(),
                Default::default(),
            )
            .unwrap();
            Self { ctx, root, _dir: dir }
        }
        fn installed(&self, version: &str) {
            let directory = self.root.join("Serum 2.vst3/Contents/Resources");
            std::fs::create_dir_all(&directory).unwrap();
            let format = structure::builtin("serum-2").unwrap();
            std::fs::write(
                directory.join("moduleinfo.json"),
                serde_json::to_vec(&json!({"Classes":[{
                    "Name":"Serum 2", "CID":format.info.ids.vst3[0].id, "Version":version, "Category":"Audio Module Class"
                }]}))
                .unwrap(),
            )
            .unwrap();
        }
        async fn call(&self, args: Value, signal: Signal) -> Result<Value, OpError> {
            build(
                &self.ctx,
                Call { args: args.as_object().unwrap().clone(), host: Host::default(), signal, progress: Progress::none() },
                vec![self.root.clone()],
            )
            .await
        }
    }
    impl Drop for Fixture {
        fn drop(&mut self) {
            self.ctx.snapshots().flush();
        }
    }
    fn request() -> Value {
        json!({"plugin":"serum-2","name":"Measured value","edits":[{"path":"Oscillator1/plainParams/kParamVolume","value":0.45}]})
    }

    #[tokio::test]
    async fn an_installed_verified_version_builds_a_private_receipt_without_a_daw() {
        let fixture = Fixture::new(DawId::Ableton);
        let version = &structure::builtin("serum-2").unwrap().verified.writes[0].version;
        fixture.installed(version);
        let result = fixture.call(request(), Signal::new()).await.unwrap();
        assert_eq!(result["state"], "built");
        assert_eq!(result["version"], *version);
        assert_eq!(result["settings"][0]["after"], 0.45);
        assert_eq!(result["settings"][0]["unit"], "stored value");
        assert!(result.get("path").is_none());
        let checked =
            fixture.ctx.artifacts().verified_preset(&fixture.ctx, result["artifact_id"].as_str().unwrap(), &Signal::new()).await.unwrap();
        assert_eq!(checked.metadata.digest, result["digest"]);
        assert_eq!(checked.metadata.settings[0].after, result["settings"][0]["after"]);
        assert!(!fixture.ctx.paths().kumi_home.join("presets").exists());
    }

    #[tokio::test]
    async fn missing_unknown_and_model_asserted_versions_never_publish_a_preset() {
        let fixture = Fixture::new(DawId::Ableton);
        assert_eq!(fixture.call(request(), Signal::new()).await.unwrap_err().code(), "not_installed");
        fixture.installed("999.0");
        assert_eq!(fixture.call(request(), Signal::new()).await.unwrap_err().code(), "unverified_version");
        let mut overridden = request();
        overridden["version"] = json!(structure::builtin("serum-2").unwrap().verified.writes[0].version);
        assert_eq!(fixture.call(overridden, Signal::new()).await.unwrap_err().code(), "invalid");
        assert_eq!(std::fs::read_dir(fixture.ctx.paths().state_dir.join("artifacts")).unwrap().count(), 0);
    }

    #[tokio::test]
    async fn cancellation_and_unverified_controls_leave_no_artifact() {
        let fixture = Fixture::new(DawId::Ableton);
        fixture.installed(&structure::builtin("serum-2").unwrap().verified.writes[0].version);
        let signal = Signal::new();
        signal.cancel();
        assert_eq!(fixture.call(request(), signal).await.unwrap_err().code(), "cancelled");
        let mut invalid = request();
        invalid["edits"][0]["path"] = json!("Oscillator0/SampleOsc0/samplePath");
        invalid["edits"][0]["value"] = json!("file:///outside/sample.wav");
        assert_eq!(fixture.call(invalid, Signal::new()).await.unwrap_err().code(), "invalid_preset");
        assert_eq!(std::fs::read_dir(fixture.ctx.paths().state_dir.join("artifacts")).unwrap().count(), 0);
    }

    #[tokio::test]
    async fn other_daws_refuse_before_inventory_lookup_or_artifact_publication() {
        for daw in [DawId::Reaper, DawId::Renoise] {
            let fixture = Fixture::new(daw);
            // No installed version exists: this must refuse the DAW, not start discovery and report not_installed.
            assert_eq!(fixture.call(request(), Signal::new()).await.unwrap_err().code(), "unsupported");
            assert_eq!(std::fs::read_dir(fixture.ctx.paths().state_dir.join("artifacts")).unwrap().count(), 0);
        }
    }
}
