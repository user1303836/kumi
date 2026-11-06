//! `preset` (CC-26): a verified plug-in preset, an artifact `preset_build` made, placed in Live's User Library (Kumi
//! folder) through the session's integration ([`Integration::preset_preview`], [`Integration::preset_place`]).
//!
//! The source is only an artifact id: serve reads the bytes through `verified_preset`, which checks the receipt's
//! writer, class, version and digest on the handle it reads, so no path is ever opened again.
//! - `preview` (effect Read) says the plug-in, the version it was verified with, the settings it was made with (as the
//!   receipt has them: a catalog unit or "stored value", never converted, 26-s1) and the file it would write, and
//!   remembers that file for the digest.
//! - `place` (effect LibraryWrite) refuses `changed` unless the bytes still hash to the approved `digest` and this
//!   runtime's preview of that digest named the approved `file` (26-r1). Both actions read the plug-in's VST3 on disk
//!   first (26-s4): none is `not_installed`, another version or VSTs that disagree are `changed`, and a version that
//!   can't be read leaves the approval's "verified with" (`version_checked: false`). Live then refuses `not_installed`
//!   unless its Browser lists the plug-in by its exact name. The answer is the Browser item for `load_device`; a file
//!   Live hasn't listed yet is said, never loaded by path (26-r5).
//!
//! LibraryWrite takes no DAW write lease: the file goes beside the Set, never into it. Loading it is an ordinary
//! approved change (`change`'s `load_device`), and Kumi's undo of that never removes the file (26e).

use super::change::one_line;
use super::connection::names_only_text;
use super::ctx::{Call, Ctx, Effect, JsonObject, OpError, OpFuture, Operation};
use kumi_agent::core::contracts::Integration;
use kumi_library::plugins::{inventory, preset::review};
use serde_json::{json, Value};
use std::path::PathBuf;

pub static PRESET: Operation = Operation {
    name: "preset",
    description: "Places a verified plug-in preset (an artifact from preset_build) in Live's User Library, Kumi folder. \
                  `preview` says the plug-in, the version it was verified with, its settings and the file it would write, \
                  changing nothing; `place`, with the approved `digest` and `file`, writes it as a new file and answers its \
                  Browser item for `load_device`. The file stays after an undo.",
    schema,
    effect,
    run,
};

fn schema() -> Value {
    json!({"type": "object", "additionalProperties": false, "required": ["action", "artifact_id"], "properties": {
        "action": {"enum": ["preview", "place"]},
        "artifact_id": {"type": "string", "minLength": 1, "maxLength": 128, "description": "The preset artifact preset_build made"},
        "digest": {"type": "string", "pattern": "^[0-9a-f]{64}$", "description": "place: the approved preview's digest"},
        "file": {"type": "string", "minLength": 1, "maxLength": 512, "description": "place: the approved preview's file"}
    }})
}

fn effect(args: &JsonObject) -> Effect {
    match args.get("action").and_then(Value::as_str) {
        Some("place") => Effect::LibraryWrite,
        _ => Effect::Read,
    }
}

fn run(ctx: &Ctx, call: Call) -> OpFuture<'_> {
    Box::pin(preset(ctx, call, inventory::standard_roots()))
}

fn changed(reason: impl Into<String>) -> OpError {
    OpError::Refused { code: "changed", reason: reason.into(), owner: None }
}

fn failed(error: impl std::fmt::Display) -> OpError {
    OpError::Failed { reason: format!("Kumi couldn't place the preset: {error}") }
}

async fn preset(ctx: &Ctx, call: Call, roots: Vec<PathBuf>) -> Result<Value, OpError> {
    let place = call.args.get("action").and_then(Value::as_str) == Some("place");
    let id = call.args.get("artifact_id").and_then(Value::as_str).unwrap_or_default();
    let connection = ctx.connection()?;
    let integration: &dyn Integration = connection.integration.as_ref();
    if !integration.has_preset_place() {
        return Err(OpError::Refused {
            code: "unsupported",
            reason: format!("Kumi places plug-in presets only in Live, not in {}.", ctx.daw().profile().words.app),
            owner: None,
        });
    }
    let verified = ctx.artifacts().verified_preset(ctx, id, &call.signal).await?;
    let record = &verified.metadata;
    let (plugin, class) = (record.plugin.clone(), record.class.clone());
    let installed = tokio::task::spawn_blocking(move || inventory::installed_vst3(&roots, &plugin, &class)).await.map_err(failed)?;
    let installed = match installed {
        // The writer needed this VST3 to build the preset: without it, it's gone since (and a same-named AU can't load it).
        Ok(None) => {
            return Err(OpError::Refused {
                code: "not_installed",
                reason: "The plug-in's VST3 isn't installed in a standard plug-in folder now. Install it, then ask again.".into(),
                owner: None,
            })
        }
        Ok(Some(entry)) => entry.version,
        Err(_) => {
            return Err(changed(
                "Kumi can't tell which version of the plug-in is installed (its VST3s disagree): build the preset again once one is.",
            ))
        }
    };
    if let Some(installed) = &installed {
        if installed != &record.version {
            return Err(changed(format!("The installed plug-in is {installed} now, not {}: build the preset again.", record.version)));
        }
    }
    let name = one_line(&names_only_text(&record.name), 64);
    let preview = integration.preset_preview(&verified.bytes, &name).await.map_err(failed)?.map_err(|findings| OpError::Findings {
        code: "refused",
        reason: "Kumi won't place this preset: see findings.".into(),
        findings,
    })?;
    // 26-s2: the class Live's own decoder reads from the bytes is the receipt's.
    if !preview.class_id.eq_ignore_ascii_case(&record.class) {
        return Err(changed("This preset's contents aren't the plug-in its receipt names: build the preset again."));
    }
    if !place {
        ctx.preset_previews().remember(&record.digest, &preview.file);
        return Ok(json!({
            "state": "preview", "plugin": preview.plugin, "class": record.class, "version": record.version,
            "version_checked": installed.is_some(), "format": record.format, "digest": record.digest,
            "settings": record.settings, "values_note": review::VALUE_NOTE, "file": preview.file, "name": preview.name,
        }));
    }
    let (Some(digest), Some(file)) = (call.args.get("digest").and_then(Value::as_str), call.args.get("file").and_then(Value::as_str))
    else {
        return Err(OpError::Invalid { reason: "place takes the approved preview's digest and file.".into() });
    };
    if digest != record.digest || ctx.preset_previews().file(digest).as_deref() != Some(file) || preview.file != file {
        return Err(changed("This isn't the preset or the file that was approved: preview it again."));
    }
    let placed = match integration.preset_place(&verified.bytes, &name, file, call.signal.clone()).await.map_err(failed)? {
        Ok(placed) => placed,
        Err(refusal) => return Err(OpError::Refused { code: refusal.code, reason: refusal.reason, owner: None }),
    };
    Ok(json!({
        "name": placed.name, "item": placed.item, "file": placed.file, "indexed": placed.indexed,
        "plugin": placed.plugin, "class": record.class, "version": record.version,
        "next": if placed.indexed {
            "Load it with change's load_device and this item; Kumi's undo removes the device, never this file."
        } else {
            "It's written; Live hasn't listed it yet. Ask again in a moment: load_device waits for Live's Browser."
        },
    }))
}

#[cfg(test)]
mod tests {
    use super::super::ctx::{Host, Progress};
    use super::super::Args;
    use super::*;
    use async_trait::async_trait;
    use kumi_agent::core::contracts::{Observation, ObserveHints, PlacedPreset, PresetPreview, PresetRefusal};
    use kumi_common::abort::Signal;
    use kumi_daw_ableton::{devices::tool::DeviceToolOptions, plugin_presets};
    use kumi_library::plugins::{
        formats::structure,
        preset::{Edit, WriteOptions},
    };
    use kumi_types::{daw::DawId, errors::RuntimeError, system::Env};
    use std::{collections::BTreeMap, path::Path, rc::Rc};

    /// Live's placing on a User Library of the test's own, its Browser listing the plug-ins `names` and every file.
    struct Live {
        library: PathBuf,
        names: Vec<String>,
    }

    impl Live {
        fn browser(&self) -> plugin_presets::Browser {
            let names = self.names.clone();
            plugin_presets::Browser {
                plugins: Rc::new(move |_, _| {
                    let names = names.clone();
                    futures::FutureExt::boxed_local(async move { Ok(names) })
                }),
                options: DeviceToolOptions {
                    user_library: self.library.to_string_lossy().into_owned(),
                    browser_sees: Rc::new(|_, _| futures::FutureExt::boxed_local(async { Ok(true) })),
                    wait_ms: Some(0),
                    made: Default::default(),
                },
            }
        }
    }

    #[async_trait(?Send)]
    impl Integration for Live {
        async fn start(&self, _: Signal) -> Result<(), RuntimeError> {
            Ok(())
        }
        async fn observe(&self, _: Signal, _: Option<ObserveHints>) -> Result<Observation, RuntimeError> {
            unreachable!("the preset op doesn't look at the Set")
        }
        async fn close(&self) -> Result<(), RuntimeError> {
            Ok(())
        }
        fn has_preset_place(&self) -> bool {
            true
        }
        async fn preset_preview(&self, bytes: &[u8], name: &str) -> Result<Result<PresetPreview, Vec<String>>, RuntimeError> {
            Ok(plugin_presets::preview(&self.library, bytes, name))
        }
        async fn preset_place(
            &self,
            bytes: &[u8],
            name: &str,
            file: &str,
            signal: Signal,
        ) -> Result<Result<PlacedPreset, PresetRefusal>, RuntimeError> {
            plugin_presets::place_approved(&self.browser(), bytes, name, file, &signal).await
        }
    }

    struct Fixture {
        ctx: Rc<Ctx>,
        library: PathBuf,
        vst3: PathBuf,
        _dir: tempfile::TempDir,
    }

    impl Fixture {
        /// A session on Live whose Browser lists `names`, a preset artifact of Serum 2 named "Night Bass", and no VST3 yet.
        async fn new(names: &[&str]) -> (Fixture, String) {
            let dir = tempfile::tempdir().unwrap();
            let (library, vst3) = (dir.path().join("User Library"), dir.path().join("VST3"));
            std::fs::create_dir_all(&library).unwrap();
            std::fs::create_dir_all(&vst3).unwrap();
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
            ctx.link().connected_to(Rc::new(Live { library: library.clone(), names: names.iter().map(|name| name.to_string()).collect() }));
            let built = review::build(
                "serum-2",
                &WriteOptions { version: verified_version(), class_id: None, name: Some("Night Bass".into()) },
                &[Edit { path: "Oscillator1/plainParams/kParamVolume".into(), value: json!(0.45) }],
            )
            .unwrap();
            let (saved, _) = ctx.artifacts().publish_preset(&ctx, built, &Signal::new()).await.unwrap();
            (Fixture { ctx, library, vst3, _dir: dir }, saved.id)
        }

        /// Serum 2's VST3 in the test's plug-in folder, its moduleinfo saying `version` (None: no version in it).
        fn installed(&self, version: Option<&str>) {
            let folder = self.vst3.join("Serum 2.vst3/Contents/Resources");
            std::fs::create_dir_all(&folder).unwrap();
            let mut class = json!({"Name": "Serum 2", "CID": structure::builtin("serum-2").unwrap().info.ids.vst3[0].id, "Category": "Audio Module Class"});
            if let Some(version) = version {
                class["Version"] = json!(version);
            }
            std::fs::write(folder.join("moduleinfo.json"), serde_json::to_vec(&json!({"Classes": [class]})).unwrap()).unwrap();
        }

        async fn call(&self, args: Value) -> Result<Value, OpError> {
            let call =
                Call { args: args.as_object().unwrap().clone(), host: Host::default(), signal: Signal::new(), progress: Progress::none() };
            preset(&self.ctx, call, vec![self.vst3.clone()]).await
        }

        fn written(&self) -> Vec<PathBuf> {
            fn walk(folder: &Path, into: &mut Vec<PathBuf>) {
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

    fn verified_version() -> String {
        structure::builtin("serum-2").unwrap().verified.writes[0].version.clone()
    }

    #[tokio::test]
    async fn a_preview_says_what_would_be_placed_and_writes_nothing() {
        let (fixture, id) = Fixture::new(&["Serum 2"]).await;
        fixture.installed(Some(&verified_version()));
        let preview = fixture.call(json!({"action": "preview", "artifact_id": id})).await.unwrap();
        assert_eq!((preview["state"].as_str(), preview["plugin"].as_str()), (Some("preview"), Some("Serum 2")));
        assert_eq!((preview["version"].as_str(), preview["version_checked"].as_bool()), (Some(verified_version().as_str()), Some(true)));
        assert_eq!(preview["settings"][0]["after"], 0.45);
        assert_eq!(
            (preview["settings"][0]["unit"].as_str(), preview["values_note"].as_str()),
            (Some("stored value"), Some(review::VALUE_NOTE))
        );
        let name = preview["name"].as_str().unwrap();
        assert!(name.starts_with("Night Bass "), "{name}");
        assert_eq!(preview["file"], format!("User Library/Kumi/Instruments/{name}.vstpreset"));
        assert!(fixture.written().is_empty(), "{:?}", fixture.written());
        assert_eq!(effect(&json!({"action": "preview"}).as_object().unwrap().clone()), Effect::Read);
        assert_eq!(effect(&json!({"action": "place"}).as_object().unwrap().clone()), Effect::LibraryWrite);

        // A VST3 whose version can't be read: the preview goes on, and its approval says "verified with".
        fixture.installed(None);
        let unread = fixture.call(json!({"action": "preview", "artifact_id": id})).await.unwrap();
        assert_eq!(unread["version_checked"], false);
    }

    #[tokio::test]
    async fn a_place_is_held_to_the_previewed_digest_and_file() {
        let (fixture, id) = Fixture::new(&["Serum 2"]).await;
        fixture.installed(Some(&verified_version()));
        let verified = fixture.ctx.artifacts().verified_preset(&fixture.ctx, &id, &Signal::new()).await.unwrap();
        let digest = verified.metadata.digest.clone();
        let place = |digest: &str, file: &str| json!({"action": "place", "artifact_id": id, "digest": digest, "file": file});

        // Never previewed in this runtime: there's no approved file to write.
        let unpreviewed = fixture.call(place(&digest, "User Library/Kumi/Instruments/x.vstpreset")).await.unwrap_err();
        assert_eq!(unpreviewed.code(), "changed");
        let preview = fixture.call(json!({"action": "preview", "artifact_id": id})).await.unwrap();
        let file = preview["file"].as_str().unwrap();
        assert_eq!(fixture.call(place(&"0".repeat(64), file)).await.unwrap_err().code(), "changed");
        assert_eq!(fixture.call(place(&digest, "User Library/Kumi/Instruments/Other.vstpreset")).await.unwrap_err().code(), "changed");
        assert_eq!(fixture.call(json!({"action": "place", "artifact_id": id})).await.unwrap_err().code(), "invalid");
        assert!(fixture.written().is_empty());

        let placed = fixture.call(place(&digest, file)).await.unwrap();
        assert_eq!((placed["file"].as_str(), placed["indexed"].as_bool()), (Some(file), Some(true)));
        assert_eq!(placed["item"], format!("user_library/Kumi/Instruments/{}.vstpreset", preview["name"].as_str().unwrap()));
        assert_eq!(fixture.written().len(), 1);
        assert_eq!(std::fs::read(&fixture.written()[0]).unwrap(), verified.bytes, "the artifact's verified bytes, unchanged");
    }

    #[tokio::test]
    async fn the_plug_in_installed_now_and_live_s_browser_decide() {
        let (fixture, id) = Fixture::new(&["Serum 2"]).await;
        let preview = json!({"action": "preview", "artifact_id": id});
        // No VST3 on disk: a same-named AU in Live's Browser can't load a .vstpreset.
        assert_eq!(fixture.call(preview.clone()).await.unwrap_err().code(), "not_installed");
        fixture.installed(Some("999.0"));
        let other = fixture.call(preview.clone()).await.unwrap_err();
        assert_eq!(other.code(), "changed");
        assert!(other.reason().contains("999.0"), "{}", other.reason());

        // Installed as verified, but Live's Browser lists only a near name: the place writes nothing.
        let (near, id) = Fixture::new(&["Serum 2 FX"]).await;
        near.installed(Some(&verified_version()));
        let previewed = near.call(json!({"action": "preview", "artifact_id": id})).await.unwrap();
        let place = json!({"action": "place", "artifact_id": id, "digest": previewed["digest"], "file": previewed["file"]});
        assert_eq!(near.call(place).await.unwrap_err().code(), "not_installed");
        assert!(near.written().is_empty());
    }
}
