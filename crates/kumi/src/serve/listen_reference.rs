//! CC-27: approved file excerpts and equal-weight reference profiles. No DAW or model calls here.

use super::{
    artifacts::{StagedArtifact, MAX_BYTES},
    change::one_line,
    ctx::{Call, Ctx, JsonObject, OpError, OpFuture},
    listen::{self, MeasuredAudio, Source},
};
use kumi_common::abort::{self, Signal};
use kumi_listening::{
    checklist::{Profile, Spread, REGIONS},
    reference::{self, ReferenceAvailability, ReferenceOptions, ReferenceWindow, RetainedWav, SampleWindow, UnavailableReason},
};
use kumi_media::audio::{self, DecodeOptions};
use serde_json::{json, Value};
use std::{
    path::{Path, PathBuf},
    time::Duration,
};

pub(super) const PREPARE_SECONDS: u64 = 120;

pub(super) fn reference_schema() -> Value {
    let mut schema = listen::source_schema();
    schema["properties"]["from"] =
        json!({"type":"number","minimum":0,"description":"Reference-file offset in seconds, not capture beats. Requires seconds."});
    schema["properties"]["seconds"] = json!({"type":"number","exclusiveMinimum":0,"maximum":60,"description":"Exact reference excerpt duration; requires from and must fit within EOF."});
    schema["dependentRequired"] = json!({"from":["seconds"],"seconds":["from"]});
    schema
}

pub(super) fn references_schema() -> Value {
    json!({"type":"array","minItems":1,"maxItems":5,"items":reference_schema(),
        "description":"Approved file/sound/artifact references, equally weighted into one measured profile. Exclusive with compare_to; duplicate source/window pairs are refused."})
}

pub(super) fn is_new_request(args: &JsonObject) -> bool {
    args.contains_key("references")
        || args.get("compare_to").is_some_and(|value| value.get("from").is_some() || value.get("seconds").is_some())
}

fn invalid(reason: &str) -> OpError {
    OpError::Invalid { reason: reason.into() }
}

pub(super) struct Reference<'a> {
    source: Source<'a>,
    window: Option<ReferenceWindow>,
}

impl<'a> Reference<'a> {
    fn parse(value: &'a Value) -> Result<Self, OpError> {
        let fields = value.as_object().ok_or_else(|| invalid("A reference must be a file, sound or artifact object."))?;
        let window = if fields.contains_key("from") || fields.contains_key("seconds") {
            let bad = || {
                invalid("A reference excerpt needs both finite from >= 0 and 0 < seconds <= 60, with a finite end; no implicit or padded window.")
            };
            let from = fields.get("from").and_then(Value::as_f64).filter(|value| value.is_finite() && *value >= 0.).ok_or_else(bad)?;
            let seconds = fields
                .get("seconds")
                .and_then(Value::as_f64)
                .filter(|value| value.is_finite() && *value > 0. && *value <= 60.)
                .ok_or_else(bad)?;
            if !(from + seconds).is_finite() || from + seconds <= from {
                return Err(bad());
            }
            Some(ReferenceWindow { from, seconds })
        } else {
            None
        };
        let source = Source::parse_with_extra(value, if window.is_some() { 2 } else { 0 })?;
        Ok(Self { source, window })
    }

    fn identity(&self, file: &StagedArtifact) -> Value {
        let identity = file.source_identity().value();
        match &self.source {
            Source::File(_) => json!({"kind":"file","file_identity":identity}),
            Source::Sound { id, revision } => json!({"kind":"sound","sound_id":id,"map_revision":revision,"file_identity":identity}),
            Source::Artifact(id) => json!({"kind":"artifact","artifact_id":id,"file_identity":identity}),
        }
    }
}

pub(super) struct ReferenceSet<'a> {
    entries: Vec<Reference<'a>>,
    pub profile: bool,
}

impl<'a> ReferenceSet<'a> {
    pub(super) fn parse(args: &'a JsonObject) -> Result<Self, OpError> {
        if args.contains_key("references") && args.contains_key("compare_to") {
            return Err(invalid("Choose compare_to or references, not both."));
        }
        let (entries, profile) = if let Some(values) = args.get("references") {
            let values = values
                .as_array()
                .filter(|values| (1..=5).contains(&values.len()))
                .ok_or_else(|| invalid("references requires 1 to 5 file, sound or artifact references."))?;
            (values.iter().map(Reference::parse).collect::<Result<Vec<_>, _>>()?, true)
        } else {
            (args.get("compare_to").map(Reference::parse).transpose()?.into_iter().collect(), false)
        };
        Ok(Self { entries, profile })
    }

    pub(super) fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// Resolve every selection before any DSP. Canonical paths also detect aliases of the same source/window.
    pub(super) async fn resolve(&self, ctx: &Ctx, signal: &Signal) -> Result<Vec<PathBuf>, OpError> {
        let mut paths = Vec::with_capacity(self.entries.len());
        for (index, entry) in self.entries.iter().enumerate() {
            let path = listen::resolve_source(ctx, &entry.source, signal).await?;
            if paths.iter().enumerate().any(|(prior, other)| *other == path && self.entries[prior].window == self.entries[index].window) {
                return Err(invalid("A reference repeats the same source and window; choose each source/window once."));
            }
            paths.push(path);
        }
        Ok(paths)
    }

    async fn measure_resolved(&self, ctx: &Ctx, paths: Vec<PathBuf>, signal: &Signal) -> Result<PreparedReferences, OpError> {
        let mut entries: Vec<PreparedReference> = Vec::with_capacity(self.entries.len());
        for (entry, path) in self.entries.iter().zip(paths) {
            let file = listen::stage_source(ctx, &entry.source, signal).await?;
            let identity = entry.identity(&file);
            let existing_origin = file.audio_origin().cloned();
            let name = existing_origin
                .as_ref()
                .and_then(|origin| origin.get("name"))
                .and_then(Value::as_str)
                .map(display_label)
                .unwrap_or_else(|| display_name(&path));
            let measured = measure_local(ctx, file, &name, entry.window, self.profile, signal).await?;
            if entries.iter().any(|previous| {
                previous.measured.audio.file.source_identity() == measured.audio.file.source_identity()
                    && previous.measured.facts.as_ref().zip(measured.facts.as_ref()).is_some_and(|(a, b)| {
                        a.window.start_frame == b.window.start_frame
                            && a.window.frame_count == b.window.frame_count
                            && a.window.sample_rate == b.window.sample_rate
                            && a.window.channels == b.window.channels
                    })
            }) {
                return Err(invalid("A reference repeats the same source and actual sample window; choose it once."));
            }
            let mut metadata = json!({"identity":identity,"name":name,"window":measured.facts.as_ref().map(|facts| &facts.window)});
            if let Some(origin) = existing_origin {
                metadata["origin"] = origin;
            }
            let mut measured = measured;
            // Only newly owned stages acquire provenance. Registered artifacts keep their receipt and identity.
            if entry.window.is_some() || !matches!(&entry.source, Source::Artifact(_)) {
                measured.audio.file.set_audio_origin(metadata_without_ancestor(&metadata));
            }
            entries.push(PreparedReference { measured, metadata });
        }
        Ok(PreparedReferences { entries, profile: self.profile })
    }

    /// One allowance for the complete set, never one allowance per member. No artifact is published here.
    pub(super) async fn prepare(&self, ctx: &Ctx, original: &Signal, seconds: u64) -> Result<PreparedReferences, OpError> {
        if self.is_empty() {
            return Ok(PreparedReferences { entries: Vec::new(), profile: self.profile });
        }
        if seconds == 0 {
            return Err(OpError::Refused {
                code: "prepare_timeout",
                reason: "The references have no preparation allowance; nothing was captured or rendered.".into(),
                owner: None,
            });
        }
        let stop = Signal::new();
        let signal = abort::any([original.clone(), stop.clone()]);
        let work = async {
            let paths = self.resolve(ctx, &signal).await?;
            self.measure_resolved(ctx, paths, &signal).await
        };
        tokio::select! {
            biased;
            _ = tokio::time::sleep(Duration::from_secs(seconds)) => {
                stop.cancel();
                Err(OpError::Refused { code:"prepare_timeout", reason:"The references exceeded their shared preparation deadline; nothing was captured or rendered.".into(), owner:None })
            }
            _ = original.cancelled() => {
                stop.cancel();
                Err(OpError::Refused { code:"cancelled", reason:"Reference preparation was cancelled; nothing was captured or rendered.".into(), owner:None })
            }
            result = work => result,
        }
    }
}

fn metadata_without_ancestor(metadata: &Value) -> Value {
    json!({"identity":metadata["identity"],"name":metadata["name"],"window":metadata["window"]})
}

fn display_name(path: &Path) -> String {
    display_label(&path.file_name().unwrap_or_default().to_string_lossy())
}

fn display_label(name: &str) -> String {
    one_line(name.rsplit(['\\', '/']).next().unwrap_or(name), 160)
}

pub(super) struct ProfileFacts {
    profile: Option<Profile>,
    window: SampleWindow,
    availability: ReferenceAvailability,
}

pub(super) struct MeasuredCurrent {
    pub audio: MeasuredAudio,
    facts: Option<ProfileFacts>,
}

struct PreparedReference {
    measured: MeasuredCurrent,
    metadata: Value,
}

pub(super) struct PreparedReferences {
    entries: Vec<PreparedReference>,
    pub profile: bool,
}

impl PreparedReferences {
    pub(super) fn preview(&self) -> Value {
        Value::Array(self.entries.iter().map(|entry| entry.metadata.clone()).collect())
    }
}

/// An ordinary current source stays on the old meter unless a profile was requested.
pub(super) async fn measure_current(
    ctx: &Ctx,
    file: StagedArtifact,
    name: &Path,
    profile: bool,
    signal: &Signal,
) -> Result<MeasuredCurrent, OpError> {
    if !profile {
        return Ok(MeasuredCurrent { audio: listen::measure(ctx, file, name, signal).await?, facts: None });
    }
    measure_local(ctx, file, &display_name(name), None, true, signal).await
}

async fn measure_local(
    ctx: &Ctx,
    file: StagedArtifact,
    name: &str,
    window: Option<ReferenceWindow>,
    profile: bool,
    signal: &Signal,
) -> Result<MeasuredCurrent, OpError> {
    listen::check(signal)?;
    file.verify()?;
    let path = file.path().to_str().ok_or_else(|| invalid("The staged audio path is not valid UTF-8."))?;
    let output = window.map(|_| ctx.artifacts().audio_stage()).transpose()?;
    let retained = output.as_ref().map(|output| output.writer().map(|file| RetainedWav { file, max_bytes: MAX_BYTES })).transpose()?;
    let measured = reference::measure_reference(
        path,
        name,
        ReferenceOptions {
            window,
            profile,
            signal: Some(signal.clone()),
            decode: DecodeOptions { installed_only: true, ffmpeg: ctx.media().tool("ffmpeg").map(Path::to_path_buf) },
        },
        retained,
    )
    .await;
    listen::check(signal)?;
    file.verify()?;
    let measured = measured.map_err(|error| OpError::Failed { reason: error.to_string().replace(path, "the retained audio") })?;
    let mime = if output.is_some() { "audio/wav" } else { listen::audio_mime(&measured.analysis.format)? };
    let file = match output {
        Some(output) => output.finish(file.source_identity())?,
        None => file,
    };
    Ok(MeasuredCurrent {
        audio: MeasuredAudio { file, metrics: measured.analysis, mime },
        facts: Some(ProfileFacts { profile: measured.profile, window: measured.window, availability: measured.availability }),
    })
}

pub(super) fn run_file(ctx: &Ctx, call: Call) -> OpFuture<'_> {
    Box::pin(async move {
        listen::check(&call.signal)?;
        if call.args.keys().any(|key| !matches!(key.as_str(), "source" | "compare_to" | "references")) {
            return Err(invalid("File listening accepts source and either compare_to or references."));
        }
        let source = Source::parse(call.args.get("source").ok_or_else(|| invalid("listen requires an audio source object."))?)?;
        let references = ReferenceSet::parse(&call.args)?;
        let path = listen::resolve_source(ctx, &source, &call.signal).await?;
        let reference_paths = references.resolve(ctx, &call.signal).await?;
        let file = listen::stage_source(ctx, &source, &call.signal).await?;
        let current = measure_current(ctx, file, &path, references.profile, &call.signal).await?;
        let references = references.measure_resolved(ctx, reference_paths, &call.signal).await?;
        retain(ctx, current, references, &call.signal, None).await
    })
}

pub(super) async fn retain(
    ctx: &Ctx,
    current: MeasuredCurrent,
    mut references: PreparedReferences,
    signal: &Signal,
    deadline: Option<tokio::time::Instant>,
) -> Result<Value, OpError> {
    listen::check(signal)?;
    let mut verdict = audio::tools::summary(&current.audio.metrics);
    let profile = if references.profile { Some(profile_report(current.facts.as_ref(), &mut references.entries)) } else { None };
    let comparison = if !references.profile {
        references.entries.first().map(|entry| audio::compare(&current.audio.metrics, &entry.measured.audio.metrics))
    } else {
        None
    };
    if let Some(comparison) = &comparison {
        for headline in &comparison.headlines {
            verdict.push('\n');
            verdict.push_str(headline);
        }
    }
    let mut files = Vec::with_capacity(1 + references.entries.len());
    files.push((current.audio.file, current.audio.mime));
    let mut replies = Vec::with_capacity(references.entries.len());
    for entry in references.entries {
        let mut reply = entry.metadata;
        reply["metrics"] = json!(entry.measured.audio.metrics);
        if references.profile {
            reply["availability"] = json!(entry.measured.facts.as_ref().map(|facts| &facts.availability));
        }
        replies.push(reply);
        files.push((entry.measured.audio.file, entry.measured.audio.mime));
    }
    // Publication owns the no-await signal/deadline gate and, once enqueued, its non-abortable commit outcome.
    let mut saved = ctx.artifacts().publish_audio_group(ctx, files, signal, deadline).await?.into_iter();
    let source = saved.next().ok_or_else(|| invalid("Listening returned no current artifact."))?;
    let mut result = json!({"metrics":current.audio.metrics,"artifact_id":source.id,"verdict_text":verdict,"media_tag":format!("MEDIA:{}",source.path.display())});
    for (reply, saved) in replies.iter_mut().zip(saved) {
        reply["artifact_id"] = json!(saved.id);
    }
    if let Some(profile) = profile {
        result["references"] = json!(replies);
        result["reference_profile"] = profile;
    } else if let (Some(mut reply), Some(comparison)) = (replies.into_iter().next(), comparison) {
        reply["differences"] = json!(comparison);
        result["compare"] = reply;
    }
    Ok(result)
}

fn missing_reason(facts: Option<&ProfileFacts>, key: &str) -> Option<UnavailableReason> {
    let facts = facts?;
    facts.availability.profile_reason.or_else(|| {
        facts
            .availability
            .unavailable
            .iter()
            .find(|missing| missing.quantity == key || (key.starts_with("region/") && missing.quantity == "regions"))
            .map(|missing| missing.reason)
    })
}

fn nyquist_for(reason: Option<UnavailableReason>, facts: Option<&ProfileFacts>) -> Option<f64> {
    matches!(reason, Some(UnavailableReason::AboveNyquist | UnavailableReason::PartialFrequencyCoverage))
        .then(|| facts.map(|facts| facts.window.sample_rate / 2.))
        .flatten()
}

fn reason_text(reason: UnavailableReason, nyquist: Option<f64>) -> String {
    match reason {
        UnavailableReason::AboveNyquist => {
            format!("Above this audio's Nyquist, {:.3} kHz.", nyquist.expect("bandwidth reason has a rate") / 1000.)
        }
        UnavailableReason::PartialFrequencyCoverage => format!(
            "Only part of this quantity is below this audio's Nyquist, {:.3} kHz.",
            nyquist.expect("bandwidth reason has a rate") / 1000.
        ),
        UnavailableReason::ZeroEnergy => "No measured audio energy.".into(),
        UnavailableReason::TooShortForProfileFrames => "Too short for the profile's real main and low-band frames.".into(),
        UnavailableReason::NoUsableProfileEnergy => "No usable energy in the profile's real spectral frames.".into(),
        UnavailableReason::TooShortForLoudness => "Too short for this loudness measurement.".into(),
        UnavailableReason::BelowLoudnessGate => "Below this measurement's loudness gate.".into(),
        UnavailableReason::TooShortForRange => "Too short for loudness range.".into(),
        UnavailableReason::Mono => "Mono audio has no measured stereo width.".into(),
        UnavailableReason::NoLowBandEnergy => "No usable low-band energy.".into(),
        UnavailableReason::NotRequested => "A profile was not requested.".into(),
        UnavailableReason::NotMeasured => "This quantity was not measured.".into(),
    }
}

fn profile_report(source: Option<&ProfileFacts>, references: &mut [PreparedReference]) -> Value {
    let mut profiles = Vec::with_capacity(references.len());
    let mut profile_indices = [None; 5];
    for (index, entry) in references.iter_mut().enumerate() {
        if let Some(profile) = entry.measured.facts.as_mut().and_then(|facts| facts.profile.take()) {
            profile_indices[index] = Some(profiles.len());
            profiles.push(profile);
        }
        entry.metadata["profile_quantities"] = Value::Array(Vec::with_capacity(REGIONS.len() + 11));
    }
    let combined = Profile::combine("References", &profiles);
    let source_profile = source.and_then(|facts| facts.profile.as_ref());
    let mut quantities = Vec::with_capacity(REGIONS.len() + 11);
    let mut add = |key: &str, unit: &str, basis: &str, get: &dyn Fn(&Profile) -> Option<Spread>| {
        let current = source_profile.filter(|_| missing_reason(source, key).is_none()).and_then(get).map(|spread| spread.mid);
        let source_reason = current.is_none().then(|| missing_reason(source, key).unwrap_or(UnavailableReason::NotMeasured));
        let source_nyquist = nyquist_for(source_reason, source);
        let mut spreads = [None; 5];
        let mut contributors = 0;
        for (index, entry) in references.iter_mut().enumerate() {
            let facts = entry.measured.facts.as_ref();
            let member =
                profile_indices[index].and_then(|index| profiles.get(index)).filter(|_| missing_reason(facts, key).is_none()).and_then(get);
            if let Some(spread) = member {
                spreads[contributors] = Some(spread);
                contributors += 1;
            }
            let reason = member.is_none().then(|| missing_reason(facts, key).unwrap_or(UnavailableReason::NotMeasured));
            let nyquist = nyquist_for(reason, facts);
            entry.metadata["profile_quantities"].as_array_mut().expect("member quantity array").push(json!({
                "quantity":key,"unit":unit,"basis":basis,"value":member.map(|spread|spread.mid),
                "reason_code":reason,"reason":reason.map(|reason|reason_text(reason,nyquist)),"nyquist_hz":nyquist
            }));
        }
        // The ordinary aggregate remains unchanged. A dimension with unavailable members uses
        // the exact same spread primitive over original, available member spreads, never floors.
        let target = if contributors == profiles.len() {
            combined.as_ref().and_then(get)
        } else {
            Spread::combine(spreads[..contributors].iter().flatten().copied().collect())
        };
        quantities.push(json!({"quantity":key,"unit":unit,"basis":basis,"source":current,
            "source_reason":source_reason,"source_reason_detail":source_reason.map(|reason|reason_text(reason,source_nyquist)),
            "source_nyquist_hz":source_nyquist,
            "target":target,"target_reason":if target.is_none() {Some("no_reference_measurement")} else {None},
            "contributors":contributors,"references":references.len(),
            "deviation":current.zip(target).map(|(current,target)|current-target.mid),
            "inside_spread":current.zip(target).map(|(current,target)|current>=target.low && current<=target.high)}));
    };
    for (index, (name, _, _)) in REGIONS.iter().enumerate() {
        add(&format!("region/{name}"), "dB", "Heard 31-third-octave relative band power / Profile::regions", &|profile| {
            profile.regions.get(index).copied()
        });
    }
    add("integrated", "LUFS", "Heard BS.1770-4 integrated / Profile::integrated; distinct from canonical Analysis", &|p| p.integrated);
    add("plr", "dB", "Heard true peak minus Heard integrated / Profile::plr", &|p| p.plr);
    add("crest", "dB", "Heard median 400 ms crest of the louder half / Profile::crest", &|p| p.crest);
    add("low_width", "dB", "Heard side/mid power below 120 Hz / Profile::low_width; not canonical full-band width", &|p| p.low_width);
    add("tilt", "dB/octave", "Heard 100 Hz–10 kHz third-octave slope / Profile::tilt", &|p| Some(p.tilt));
    add("range", "LU", "Heard EBU 3342 loudness range / Profile::range", &|p| p.range);
    add("centroid", "Hz", "Heard median loud-frame centroid / Profile::centroid; not canonical energy-weighted centroid", &|p| p.centroid);
    add("noise", "dB", "Heard loud-frame spectral flatness / Profile::noise", &|p| p.noise);
    add("attack", "ms", "Heard median per-hit 10–90 percent attack / Profile::attack", &|p| p.attack);
    add("decay", "ms", "Heard median per-hit peak to -20 dB decay / Profile::decay", &|p| p.decay);
    add("sustain", "dB", "Heard per-hit level a quarter second after peak / Profile::sustain", &|p| p.sustain);
    json!({"basis":"Real Heard frames → Profile::of per original reference → one equal-per-reference Profile::combine; never canonical Analysis minus a Profile target",
        "references":references.len(),"usable_references":profiles.len(),"source_window":source.map(|facts|&facts.window),"quantities":quantities,
        "availability_note":"Each target omits members unable to carry that quantity; contributors is n of the requested N. Partial frequency coverage is unavailable, not zero.",
        "range_note":"Spreads include Profile's minimum tolerances; with fewer than five references they cover the member centers. They are not confidence intervals or purely observed variation.",
        "peak_note":"No Profile true-peak target: use each reference's canonical peak and the producer's explicit ceiling separately."})
}
