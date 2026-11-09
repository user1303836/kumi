//! `design`: cross, fuse or stack two to sixteen approved sounds into a new one, kept as a private artifact. Sources are
//! map sounds (by revision), artifacts, or files the host resolved inside an approved root (its own results, kept in a
//! media root); each is authorized, then staged as an immutable copy before it's decoded, and nothing changes in the DAW.

use super::artifacts::StagedArtifact;
use super::ctx::{Call, Ctx, Effect, OpError, OpFuture, Operation};
use super::listen::{check, Source};
use kumi_design::{fresh_seed, load_source, render, Answer, Mode, Op, Request, MAX_SEED, MAX_SOURCES, MIN_SOURCES};
use kumi_media::audio::DecodeOptions;
use serde_json::{json, Value};
use std::{io::Write, path::Path};

pub static DESIGN: Operation = Operation {
    name: "design",
    description: "Make a new sound from 2-16 approved sounds (map sounds, artifacts, or files in an approved folder): cross them (blend: one morphed body; attack: one's transient on the blend of the rest; resonate: half ring through the other half; modulate: they phase-, ring-, FM-, filter- and spectrum-modulate each other), fuse them (texture: a loopable bed that moves between them; loop: their hits sequenced on a tempo grid, 1-4 bars; smear: a 4-16x granular stretch with a diffuse tail), or stack 2-8 as one hit. A seed picks every choice: the same request and seed make the same sound, another seed a variation (reroll). Returns a private 48 kHz 24-bit WAV artifact, its peaks and a one-line recipe; changes nothing in the DAW.",
    schema,
    effect: |_| Effect::Read,
    run,
};

fn schema() -> Value {
    let name = json!({"type": "string", "minLength": 1, "maxLength": 120});
    let id = json!({"type": "string", "minLength": 1, "maxLength": 200});
    let ops: Vec<&str> = [Op::Cross, Op::Fuse, Op::Stack].iter().map(|op| op.name()).collect();
    let modes: Vec<&str> = [Mode::Blend, Mode::Attack, Mode::Resonate, Mode::Modulate, Mode::Texture, Mode::Loop, Mode::Smear]
        .iter()
        .map(|m| m.name())
        .collect();
    json!({
        "type": "object", "additionalProperties": false, "required": ["op", "sources"],
        "properties": {
            "op": {"type": "string", "enum": ops},
            "mode": {"type": "string", "enum": modes, "description": "cross: blend, attack, resonate, modulate; fuse: texture, loop, smear; stack: none."},
            "sources": {"type": "array", "minItems": MIN_SOURCES, "maxItems": MAX_SOURCES, "items": {
                "type": "object", "additionalProperties": false,
                "properties": {"sound_id": id, "map_revision": id, "artifact_id": id, "file": {"type": "string", "minLength": 1}, "name": name},
                "oneOf": [
                    {"required": ["sound_id", "map_revision"], "not": {"anyOf": [{"required": ["artifact_id"]}, {"required": ["file"]}]}},
                    {"required": ["artifact_id"], "not": {"anyOf": [{"required": ["sound_id"]}, {"required": ["map_revision"]}, {"required": ["file"]}]}},
                    {"required": ["file"], "not": {"anyOf": [{"required": ["sound_id"]}, {"required": ["map_revision"]}, {"required": ["artifact_id"]}]}}
                ]
            }},
            "seed": {"type": "integer", "minimum": 0, "maximum": MAX_SEED, "description": "The variation; omitted: a fresh one, answered."},
            "options": {"type": "object", "additionalProperties": false, "properties": {
                "seconds": {"type": "number", "minimum": 0.1, "maximum": 30},
                "bpm": {"type": "number", "minimum": 40, "maximum": 240},
                "bars": {"type": "integer", "minimum": 1, "maximum": 4},
                "attack_from": {"type": "integer", "minimum": 0, "maximum": MAX_SOURCES - 1},
                "split": {"type": "array", "minItems": 1, "maxItems": MAX_SOURCES - 1, "items": {"type": "integer", "minimum": 0, "maximum": MAX_SOURCES - 1}},
                "depth": {"type": "number", "minimum": 0, "maximum": 1},
                "stretch": {"type": "number", "minimum": 4, "maximum": 16}
            }}
        }
    })
}

fn invalid(reason: impl Into<String>) -> OpError {
    OpError::Invalid { reason: reason.into() }
}

fn run(ctx: &Ctx, call: Call) -> OpFuture<'_> {
    Box::pin(async move {
        check(&call.signal)?;
        let mut request: Request =
            serde_json::from_value(Value::Object(call.args.clone())).map_err(|e| invalid(format!("That isn't a design request: {e}")))?;
        request.check().map_err(|e| invalid(e.0))?;
        // Each source authorized before anything is copied or decoded: a file only inside an approved root.
        let raw = call.args.get("sources").and_then(Value::as_array).ok_or_else(|| invalid("design needs sources."))?;
        let parsed: Vec<Value> = raw
            .iter()
            .map(|value| {
                let mut source = value.clone();
                if let Some(fields) = source.as_object_mut() {
                    fields.remove("name");
                }
                source
            })
            .collect();
        let sources: Vec<Source> = parsed.iter().map(Source::parse).collect::<Result<_, _>>()?;
        let mut names = Vec::with_capacity(sources.len());
        for (source, given) in sources.iter().zip(&request.sources) {
            let path = super::listen::resolve_source(ctx, source, &call.signal).await?;
            let stem = path.file_stem().map(|s| s.to_string_lossy().into_owned());
            names.push(given.name.clone().or(match source {
                Source::Sound { .. } | Source::File(_) => stem,
                Source::Artifact(_) => None,
            }));
        }
        call.progress.report(0.1, Some(1.0), Some("reading the sounds"));
        let mut staged: Vec<StagedArtifact> = Vec::with_capacity(sources.len());
        for source in &sources {
            staged.push(super::listen::stage_source(ctx, source, &call.signal).await?);
        }
        let decode = DecodeOptions { installed_only: true, ffmpeg: ctx.media().tool("ffmpeg").map(Path::to_path_buf) };
        let mut decoded = Vec::with_capacity(staged.len());
        for (i, file) in staged.iter().enumerate() {
            check(&call.signal)?;
            file.verify()?;
            let name = names[i].clone().unwrap_or_else(|| format!("made sound {}", i + 1));
            let source = load_source(file.path(), &name, &decode, Some(call.signal.clone())).await.map_err(|e| OpError::Failed {
                reason: format!("{name}: {}", e.0.replace(&file.path().display().to_string(), "the sound")),
            })?;
            decoded.push(source);
        }
        let seed = *request.seed.get_or_insert_with(fresh_seed);
        call.progress.report(0.3, Some(1.0), Some("designing"));
        let job = request.clone();
        let rendered = tokio::task::spawn_blocking(move || render(&job, seed, &decoded, &mut |_| {}))
            .await
            .map_err(|e| OpError::Failed { reason: e.to_string() })?
            .map_err(|e| OpError::Failed { reason: e.0 })?;
        check(&call.signal)?;
        call.progress.report(0.9, Some(1.0), Some("keeping it"));
        let output = ctx.artifacts().audio_stage()?;
        let mut writer = output.writer()?;
        writer.write_all(&rendered.wav()).and_then(|()| writer.sync_all()).map_err(|e| OpError::Failed { reason: e.to_string() })?;
        let identity = staged[0].source_identity();
        let mut file = output.finish(identity)?;
        file.set_audio_origin(json!({"tool": "design", "op": request.op, "mode": request.mode, "seed": seed}));
        let saved = ctx.artifacts().publish(ctx, file, "audio", "audio/wav", "design", &call.signal).await?;
        let mut answer = serde_json::to_value(Answer::new(&request, seed, &rendered, Some(saved.id.clone())))
            .map_err(|e| OpError::Failed { reason: e.to_string() })?;
        answer["media_tag"] = json!(format!("MEDIA:{}", saved.path.display()));
        Ok(answer)
    })
}

#[cfg(test)]
mod tests {
    use super::super::ctx::{Host, Progress};
    use super::super::Args;
    use super::*;
    use kumi_common::abort::Signal;
    use kumi_types::{daw::DawId, system::Env};
    use std::{collections::BTreeMap, path::PathBuf, rc::Rc};

    /// Half a second of a decaying tone, as a 16-bit mono WAV: a hit to design with.
    fn hit(hz: f64) -> Vec<u8> {
        let rate = 48_000u32;
        let samples = rate / 2;
        let mut bytes = Vec::new();
        bytes.extend_from_slice(b"RIFF");
        bytes.extend_from_slice(&(36 + samples * 2).to_le_bytes());
        bytes.extend_from_slice(b"WAVEfmt ");
        for (value, width) in [(16u32, 4), (1, 2), (1, 2), (rate, 4), (rate * 2, 4), (2, 2), (16, 2)] {
            bytes.extend_from_slice(&value.to_le_bytes()[..width]);
        }
        bytes.extend_from_slice(b"data");
        bytes.extend_from_slice(&(samples * 2).to_le_bytes());
        for n in 0..samples {
            let t = n as f64 / rate as f64;
            let value = (0.8 * (std::f64::consts::TAU * hz * t).sin() * (-t * 9.0).exp() * i16::MAX as f64) as i16;
            bytes.extend_from_slice(&value.to_le_bytes());
        }
        bytes
    }

    struct Fixture {
        _dir: tempfile::TempDir,
        root: PathBuf,
        ctx: Rc<Ctx>,
    }

    impl Fixture {
        fn new() -> Self {
            let dir = tempfile::tempdir().unwrap();
            let root = dir.path().join("approved");
            std::fs::create_dir(&root).unwrap();
            let root = root.canonicalize().unwrap();
            let args = Args {
                state_dir: dir.path().join("state"),
                kumi_home: dir.path().join("home"),
                hermes_home: None,
                hermes_root: None,
                daw: DawId::Reaper,
                library_roots: vec![root.clone()],
                media_roots: Vec::new(),
                tools: BTreeMap::new(),
            };
            Self { _dir: dir, root, ctx: Ctx::new(args, Env::new(), Default::default()).unwrap() }
        }

        /// A hit kept as an artifact: its id.
        async fn artifact(&self, name: &str, hz: f64) -> String {
            let path = self.root.join(name);
            std::fs::write(&path, hit(hz)).unwrap();
            self.ctx.artifacts().add(&self.ctx, &path, None, "audio", "audio/wav", "test", &Signal::new()).await.unwrap().id
        }

        async fn design(&self, args: Value) -> Result<Value, OpError> {
            let call =
                Call { args: args.as_object().unwrap().clone(), host: Host::default(), signal: Signal::new(), progress: Progress::none() };
            (DESIGN.run)(&self.ctx, call).await
        }

        fn bytes(&self, answer: &Value) -> Vec<u8> {
            std::fs::read(self.ctx.artifacts().resolve(&self.ctx, answer["artifact_id"].as_str().unwrap()).unwrap()).unwrap()
        }
    }

    #[tokio::test]
    async fn a_design_of_artifacts_is_a_playable_artifact_and_its_seed_makes_it_again() {
        let fixture = Fixture::new();
        let (low, high) = (fixture.artifact("low.wav", 110.0).await, fixture.artifact("high.wav", 1760.0).await);
        let args = json!({"op": "cross", "mode": "blend", "seed": 7,
            "sources": [{"artifact_id": low, "name": "Low Tom"}, {"artifact_id": high}]});
        let answer = fixture.design(args.clone()).await.unwrap();
        assert_eq!(answer["seed"], 7);
        assert_eq!(answer["peaks"].as_array().unwrap().len(), 256);
        assert!(answer["recipe"].as_str().unwrap().contains("Low Tom"), "{}", answer["recipe"]);
        assert_eq!(answer["sources"][0], json!({"artifact_id": low, "name": "Low Tom"}), "sources are echoed, never paths");
        assert!(answer["media_tag"].as_str().unwrap().starts_with("MEDIA:"));
        let wav = fixture.bytes(&answer);
        assert_eq!((&wav[..4], u16::from_le_bytes([wav[34], wav[35]])), (&b"RIFF"[..], 24));
        let again = fixture.design(args).await.unwrap();
        assert_ne!(again["artifact_id"], answer["artifact_id"]);
        assert!(fixture.bytes(&again) == wav, "the same request and seed make the same sound");
        // A seed left out is drawn, and answered so it can be made again.
        let fresh = fixture.design(json!({"op": "stack", "sources": [{"artifact_id": low}, {"artifact_id": high}]})).await.unwrap();
        assert!(fresh["seed"].as_u64().is_some());
    }

    #[tokio::test]
    async fn a_design_reads_only_approved_files_and_says_what_it_cant_make() {
        let fixture = Fixture::new();
        let low = fixture.artifact("low.wav", 110.0).await;
        let path = fixture.root.join("low.wav").to_string_lossy().into_owned();
        let approved = fixture.design(json!({"op": "stack", "sources": [{"artifact_id": low}, {"file": path}]})).await.unwrap();
        assert!(approved["recipe"].as_str().unwrap().contains("low"), "{}", approved["recipe"]);
        let elsewhere = tempfile::tempdir().unwrap();
        let outside = elsewhere.path().join("outside.wav");
        std::fs::write(&outside, hit(220.0)).unwrap();
        let file = fixture.design(json!({"op": "stack", "sources": [{"artifact_id": low}, {"file": outside}]})).await;
        assert!(file.is_err(), "a file outside the approved roots isn't read");
        let modeless = fixture.design(json!({"op": "cross", "sources": [{"artifact_id": low}, {"artifact_id": low}]})).await.unwrap_err();
        assert!(modeless.reason().contains("needs a mode"), "{}", modeless.reason());
        let unknown = fixture.design(json!({"op": "stack", "sources": [{"artifact_id": low}, {"artifact_id": "a-nothing-here"}]})).await;
        assert!(unknown.is_err());
    }
}
