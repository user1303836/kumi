//! `device` (CC-22 22d): a Max for Live audio effect built from a typed spec into Live's User Library, through the
//! session's integration ([`Integration::device_preview`], [`Integration::device_build`]).
//!
//! The spec comes as `spec_json`, the exact string the host canonicalized, hashed and showed the producer: serve hashes
//! the bytes it received and parses that same string, never re-encoding it, so its SHA-256 is the approval's key.
//! - `preview` reads only the folder for the free name (effect Read) and remembers the file it named for that hash.
//! - `build` (effect LibraryWrite) refuses `stale` unless the spec still hashes to the approved `sha256` and this
//!   runtime's preview of it named the file the build would write (W1-B); it then writes the device as a new file, keeps a
//!   copy as an artifact (CC-11) and answers its Browser item for `load_device`, with no absolute path.
//!
//! LibraryWrite takes no DAW write lease: the build writes a new file beside the Set, never the Set, and waits for Live's
//! Browser only by reading it. Placing the device is an ordinary approved change (`change`'s `load_device`).
//!
//! A refusal is `{"error": {"code": "refused" | "stale", "reason", "findings": [...]}}`, its findings the plain lines the
//! model fixes the spec by (none for `stale`).

use super::ctx::{Call, Ctx, Effect, JsonObject, OpError, OpFuture, Operation};
use kumi_agent::core::contracts::Integration;
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::cell::RefCell;
use std::collections::VecDeque;

pub static DEVICE: Operation = Operation {
    name: "device",
    description: "Builds a Max for Live audio effect from a typed GenExpr spec into Live's User Library (Kumi folder). \
                  `preview` checks the spec and says the file it would write, its controls and code, changing nothing; \
                  `build`, with the approved `sha256`, writes it as a new file and answers its Browser item for \
                  `load_device`. A spec Kumi can't build is refused with findings to fix.",
    schema,
    effect,
    run,
};

/// The most of a spec Kumi reads: 32 KB of code beside its name, about and 16 controls.
const MOST_SPEC_BYTES: usize = 64 * 1024;
/// The most findings an answer carries, and the longest each is.
const MOST_FINDINGS: usize = 20;
const MOST_FINDING_CHARS: usize = 200;
/// The newest previews whose files a build may write.
const REMEMBERED: usize = 64;

fn schema() -> Value {
    json!({"type": "object", "additionalProperties": false, "required": ["action", "spec_json"], "properties": {
        "action": {"enum": ["preview", "build"]},
        "spec_json": {"type": "string", "minLength": 2, "maxLength": MOST_SPEC_BYTES,
            "description": "The spec {name, about, controls, code} as canonical JSON, exactly as approved"},
        "sha256": {"type": "string", "pattern": "^[0-9a-f]{64}$", "description": "The approved SHA-256 of spec_json (build)"}
    }})
}

fn effect(args: &JsonObject) -> Effect {
    match args.get("action").and_then(Value::as_str) {
        Some("build") => Effect::LibraryWrite,
        _ => Effect::Read,
    }
}

fn run(ctx: &Ctx, call: Call) -> OpFuture<'_> {
    Box::pin(device(ctx, call))
}

/// The files this runtime's previews named, by the SHA-256 of what was previewed (a device's spec, a preset's bytes),
/// the newest [`REMEMBERED`]: what an approved write may write.
#[derive(Default)]
pub struct Previews(RefCell<VecDeque<(String, String)>>);

impl Previews {
    pub(super) fn remember(&self, sha256: &str, file: &str) {
        let mut previews = self.0.borrow_mut();
        previews.retain(|(known, _)| known != sha256);
        previews.push_back((sha256.to_string(), file.to_string()));
        while previews.len() > REMEMBERED {
            previews.pop_front();
        }
    }

    pub(super) fn file(&self, sha256: &str) -> Option<String> {
        self.0.borrow().iter().find(|(known, _)| known == sha256).map(|(_, file)| file.clone())
    }
}

fn refused(findings: Vec<String>) -> OpError {
    let findings = findings
        .into_iter()
        .take(MOST_FINDINGS)
        .map(|finding| match finding.char_indices().nth(MOST_FINDING_CHARS - 1) {
            Some((at, _)) => format!("{}…", &finding[..at]),
            None => finding,
        })
        .collect();
    OpError::Findings {
        code: "refused",
        reason: "Kumi won't build this device: fix what's under findings, then preview it again.".into(),
        findings,
    }
}

fn stale(reason: &str) -> OpError {
    OpError::Findings { code: "stale", reason: reason.into(), findings: Vec::new() }
}

fn failed(error: impl std::fmt::Display) -> OpError {
    OpError::Failed { reason: format!("Kumi couldn't build the device: {error}") }
}

async fn device(ctx: &Ctx, call: Call) -> Result<Value, OpError> {
    let spec_json = call.args.get("spec_json").and_then(Value::as_str).unwrap_or_default();
    let sha256 = format!("{:x}", Sha256::digest(spec_json.as_bytes()));
    let spec: Value = serde_json::from_str(spec_json).map_err(|error| refused(vec![format!("spec_json: it isn't JSON ({error}).")]))?;
    let connection = ctx.connection()?;
    let integration: &dyn Integration = connection.integration.as_ref();
    if !integration.has_device_build() {
        return Err(OpError::Refused {
            code: "unsupported",
            reason: format!("Kumi builds devices only in Live, not in {}.", ctx.daw().profile().words.app),
            owner: None,
        });
    }
    let preview = integration.device_preview(&spec).await.map_err(failed)?.map_err(refused);
    if call.args.get("action").and_then(Value::as_str) != Some("build") {
        let preview = preview?;
        ctx.device_previews().remember(&sha256, &preview.file);
        return Ok(json!({
            "state": "preview", "name": preview.name, "file": preview.file, "controls": preview.controls,
            "code": preview.code, "lines": preview.lines, "sha256": sha256,
        }));
    }
    if call.args.get("sha256").and_then(Value::as_str) != Some(sha256.as_str()) {
        return Err(stale("This spec isn't the one approved (its SHA-256 differs): preview it again."));
    }
    let preview = preview?;
    let Some(file) = ctx.device_previews().file(&sha256) else {
        return Err(stale("This runtime hasn't previewed this spec, so it doesn't know the file that was approved: preview it again."));
    };
    if preview.file != file {
        return Err(stale(
            "The file the preview named is taken now (a file appeared since), so the build would write another: preview it again.",
        ));
    }
    // The spec passed its preview just now, so what the build refuses is the file it was approved for, taken since.
    let built = match integration.device_build(&spec, &sha256, &file, call.signal.clone()).await.map_err(failed)? {
        Ok(built) => built,
        Err(_) => {
            return Err(stale("The file the preview named was taken while it was being built, so nothing was written: preview it again."))
        }
    };
    // Its kept copy (CC-11), made from this call's own scratch.
    let scratch = ctx.scratch("device")?;
    let copy = scratch.path().join("device.amxd");
    tokio::fs::copy(&built.path, &copy).await.map_err(failed)?;
    let staged = ctx.artifacts().stage_scratch(ctx, &scratch, &copy, &call.signal).await?;
    let saved = ctx.artifacts().publish(ctx, staged, "device", "application/octet-stream", DEVICE.name, &call.signal).await?;
    Ok(json!({
        "name": built.name, "item": built.item, "file": built.file, "artifact_id": saved.id, "sha256": sha256,
        "listed": built.listed,
    }))
}

#[cfg(test)]
mod tests {
    use super::super::ctx::{Host, Progress};
    use super::super::Args;
    use super::*;
    use async_trait::async_trait;
    use kumi_agent::core::contracts::{BuiltDevice, DevicePreview, Observation, ObserveHints};
    use kumi_common::abort::Signal;
    use kumi_daw_ableton::devices::tool::{DeviceToolOptions, Made};
    use kumi_types::{daw::DawId, errors::RuntimeError, system::Env};
    use std::{collections::BTreeMap, path::PathBuf, rc::Rc};

    /// Live's builder on a User Library of the test's own, its Browser never listing anything.
    struct Library(PathBuf);

    impl Library {
        fn options(&self) -> DeviceToolOptions {
            DeviceToolOptions {
                user_library: self.0.to_string_lossy().into_owned(),
                browser_sees: Rc::new(|_, _| futures::FutureExt::boxed_local(async { Ok(false) })),
                wait_ms: Some(0),
                made: Rc::new(RefCell::new(Made::default())),
            }
        }
    }

    #[async_trait(?Send)]
    impl Integration for Library {
        async fn start(&self, _: Signal) -> Result<(), RuntimeError> {
            Ok(())
        }
        async fn observe(&self, _: Signal, _: Option<ObserveHints>) -> Result<Observation, RuntimeError> {
            unreachable!("the device op doesn't look at the Set")
        }
        async fn close(&self) -> Result<(), RuntimeError> {
            Ok(())
        }
        fn has_device_build(&self) -> bool {
            true
        }
        async fn device_preview(&self, spec: &Value) -> Result<Result<DevicePreview, Vec<String>>, RuntimeError> {
            Ok(kumi_daw_ableton::devices::build::preview(&self.0, spec))
        }
        async fn device_build(
            &self,
            spec: &Value,
            _: &str,
            file: &str,
            signal: Signal,
        ) -> Result<Result<BuiltDevice, Vec<String>>, RuntimeError> {
            kumi_daw_ableton::devices::build::build(&self.options(), spec, file, &signal).await
        }
    }

    struct Fixture {
        ctx: Rc<Ctx>,
        library: PathBuf,
        _dir: tempfile::TempDir,
    }

    impl Fixture {
        fn new() -> Fixture {
            let dir = tempfile::tempdir().unwrap();
            let library = dir.path().join("User Library");
            let args = Args {
                state_dir: dir.path().join("state"),
                kumi_home: dir.path().join("home"),
                hermes_home: None,
                hermes_root: None,
                daw: DawId::Ableton,
                library_roots: Vec::new(),
                media_roots: Vec::new(),
                tools: BTreeMap::new(),
            };
            let ctx = Ctx::new(args, Env::new(), Default::default()).unwrap();
            ctx.link().connected_to(Rc::new(Library(library.clone())));
            Fixture { ctx, library, _dir: dir }
        }

        async fn call(&self, args: Value) -> Result<Value, OpError> {
            let call =
                Call { args: args.as_object().unwrap().clone(), host: Host::default(), signal: Signal::new(), progress: Progress::none() };
            (DEVICE.run)(&self.ctx, call).await
        }

        /// Every file in the User Library.
        fn written(&self) -> Vec<PathBuf> {
            fn walk(folder: &std::path::Path, into: &mut Vec<PathBuf>) {
                for entry in std::fs::read_dir(folder).into_iter().flatten().flatten() {
                    if entry.path().is_dir() {
                        walk(&entry.path(), into);
                    } else {
                        into.push(entry.path());
                    }
                }
            }
            let mut found = Vec::new();
            walk(&self.library, &mut found);
            found
        }
    }

    impl Drop for Fixture {
        fn drop(&mut self) {
            self.ctx.snapshots().flush();
        }
    }

    fn spec_json() -> String {
        let spec = json!({"about": "A soft clipper.", "code": "out1 = tanh(in1 * drive);\nout2 = tanh(in2 * drive);\n",
            "controls": [{"default": 1, "max": 4, "min": 1, "name": "Drive", "type": "number"}], "name": "Soft Clip"});
        serde_json::to_string(&spec).unwrap()
    }

    fn sha(text: &str) -> String {
        format!("{:x}", Sha256::digest(text.as_bytes()))
    }

    #[tokio::test]
    async fn a_preview_writes_nothing_and_hashes_the_bytes_it_was_given() {
        let fixture = Fixture::new();
        let spec = spec_json();
        let preview = fixture.call(json!({"action": "preview", "spec_json": spec})).await.unwrap();
        assert_eq!(preview["state"], "preview");
        assert_eq!(preview["file"], "User Library/Kumi/Audio Effects/Soft Clip.amxd");
        assert_eq!(preview["sha256"], sha(&spec));
        assert_eq!((preview["lines"].clone(), preview["controls"][0]["name"].clone()), (json!(2), json!("Drive")));
        assert!(fixture.written().is_empty(), "{:?}", fixture.written());
        assert_eq!(effect(&json!({"action": "preview"}).as_object().unwrap().clone()), Effect::Read);
        assert_eq!(effect(&json!({"action": "build"}).as_object().unwrap().clone()), Effect::LibraryWrite);
        assert!(!Effect::LibraryWrite.writes(), "a build into the library takes no DAW write lease");
    }

    #[tokio::test]
    async fn a_build_with_another_sha256_or_no_preview_is_stale_and_writes_nothing() {
        let fixture = Fixture::new();
        let spec = spec_json();
        let unpreviewed = fixture.call(json!({"action": "build", "spec_json": spec, "sha256": sha(&spec)})).await.unwrap_err();
        assert_eq!(unpreviewed.code(), "stale");
        fixture.call(json!({"action": "preview", "spec_json": spec})).await.unwrap();
        let other = fixture.call(json!({"action": "build", "spec_json": spec, "sha256": sha("another")})).await.unwrap_err();
        assert_eq!(other.to_value()["error"], json!({"code": "stale", "reason": other.reason(), "findings": []}));
        let missing = fixture.call(json!({"action": "build", "spec_json": spec})).await.unwrap_err();
        assert_eq!(missing.code(), "stale");
        assert!(fixture.written().is_empty());
    }

    #[tokio::test]
    async fn a_build_answers_no_path_and_keeps_the_device_as_an_artifact() {
        let fixture = Fixture::new();
        let spec = spec_json();
        fixture.call(json!({"action": "preview", "spec_json": spec})).await.unwrap();
        let built = fixture.call(json!({"action": "build", "spec_json": spec, "sha256": sha(&spec)})).await.unwrap();
        assert_eq!(built["item"], "user_library/Kumi/Audio Effects/Soft Clip");
        assert_eq!(built["file"], "User Library/Kumi/Audio Effects/Soft Clip.amxd");
        assert_eq!(
            (built["name"].clone(), built["sha256"].clone(), built["listed"].clone()),
            (json!("Soft Clip"), json!(sha(&spec)), json!(false))
        );
        let said = built.to_string();
        let root = fixture._dir.path().to_string_lossy().into_owned();
        assert!(!said.contains(&root) && !said.contains("/Users/") && !said.contains("\\\\"), "{said}");
        let device = fixture.library.join("Kumi/Audio Effects/Soft Clip.amxd");
        let kept = fixture.ctx.artifacts().resolve(&fixture.ctx, built["artifact_id"].as_str().unwrap()).unwrap();
        assert_eq!(std::fs::read(kept).unwrap(), std::fs::read(device).unwrap());
        // The same spec again: its approved file is taken now.
        let again = fixture.call(json!({"action": "build", "spec_json": spec, "sha256": sha(&spec)})).await.unwrap_err();
        assert_eq!(again.code(), "stale");
        assert_eq!(fixture.written().len(), 1);
    }

    #[tokio::test]
    async fn a_spec_kumi_cant_build_is_refused_with_its_findings() {
        let fixture = Fixture::new();
        let spec = r#"{"about":"Loops.","code":"x = 0;\nwhile (x < 4) { x += 1; }\nout1 = in1; out2 = in2;\n","name":"Loop","group":"A"}"#;
        let error = fixture.call(json!({"action": "preview", "spec_json": spec})).await.unwrap_err();
        let value = error.to_value();
        assert_eq!(value["error"]["code"], "refused");
        let findings: Vec<&str> = value["error"]["findings"].as_array().unwrap().iter().map(|f| f.as_str().unwrap()).collect();
        assert!(findings.iter().any(|f| f.starts_with("group:")) && findings.iter().any(|f| f.contains("while loop")), "{findings:?}");
        assert!(findings.iter().all(|f| f.chars().count() <= MOST_FINDING_CHARS));
        let built = fixture.call(json!({"action": "build", "spec_json": spec, "sha256": sha(spec)})).await.unwrap_err();
        assert_eq!(built.code(), "refused");
        assert!(fixture.written().is_empty());
    }
}
