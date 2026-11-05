//! Native undo points tracked by exact project/process/revision, never a blind shared undo.
use crate::{
    integer,
    project::{array, check_epoch, epoch, identity, revision, Bridge, WRITE_OPS},
    Error, Result,
};
use daw_bridge::{auth, wire::code};
use serde_json::{json, Value};
use std::collections::VecDeque;

const MAX_RECORDS: usize = 256;
const MAX_BYTES: usize = 16 * 1024 * 1024;

#[derive(Clone, Copy, PartialEq, Eq)]
enum Phase {
    Previewed,
    Uncertain,
    Applied,
    Undone,
}
struct Entry {
    id: String,
    kind: String,
    input: Value,
    fence: Value,
    display: Value,
    target: Value,
    prior: Value,
    project: String,
    control: String,
    revision: u64,
    after_revision: Option<u64>,
    desc: Option<String>,
    phase: Phase,
    answer: Option<Value>,
    parameters: Option<String>,
}
#[derive(Default)]
pub(crate) struct Journal {
    entries: VecDeque<Entry>,
    owned: Vec<String>,
}
impl Journal {
    fn find(&self, id: &str) -> Result<usize> {
        self.entries
            .iter()
            .position(|e| e.id == id)
            .ok_or_else(|| Error::new(code::CHANGED, "This bridge no longer holds that preview or undo record; inspect and preview again"))
    }
    fn trim(&mut self) -> Result<()> {
        let bytes =
            |e: &Entry| e.input.to_string().len() + e.fence.to_string().len() + e.answer.as_ref().map_or(0, |a| a.to_string().len()) + 2048;
        while self.entries.len() >= MAX_RECORDS || self.entries.iter().map(bytes).sum::<usize>() > MAX_BYTES {
            let Some(index) = self.entries.iter().position(|e| matches!(e.phase, Phase::Previewed | Phase::Undone)) else {
                return Err(Error::new(
                    code::BUSY,
                    "The bridge's uncertain-change journal is full; reconnect after inspecting the project",
                ));
            };
            let old = self.entries.remove(index).unwrap();
            self.owned.retain(|id| *id != old.id);
        }
        Ok(())
    }
}

impl Bridge {
    pub async fn preview(&mut self, params: &Value) -> Result<Value> {
        let kind = params["kind"]
            .as_str()
            .filter(|kind| WRITE_OPS.contains(kind))
            .ok_or_else(|| Error::bad("This change kind is not supported by the LYNX native adapter"))?
            .to_owned();
        let mut input = params.get("input").filter(|v| v.is_object()).cloned().ok_or_else(|| Error::bad("input must be an object"))?;
        if input.to_string().len() > 1024 * 1024 - 4096 {
            return Err(Error::bad("Change input exceeds the native message bound"));
        }
        let expected = integer(&input, "expectedRevision")?;
        input["expectedRevision"] = expected.into();
        for field in ["expectedProjectId", "expectedEpoch", "token"] {
            if input.get(field).is_some() {
                return Err(Error::bad("Project identity and native credentials are owned by the bridge"));
            }
        }
        let session = self.session().await?;
        check_epoch(params, &session)?;
        if params["epoch"].as_str().is_none() {
            return Err(Error::bad("A persistent change must name the epoch of its read"));
        }
        if revision(&session)? != expected {
            return Err(Error::new(code::CHANGED, "The project changed after this change was planned; read it again"));
        }
        if session["playing"] == true || session["recording"] == true {
            return Err(Error::new(code::BUSY, "Stop playback and recording before editing through the native adapter"));
        }
        validate_target(&kind, &input, &session)?;
        if kind == "pattern.replace_notes" {
            let pattern = integer(&input, "index")?;
            if !self.complete_notes(&session, pattern) {
                return Err(Error::bad("Read every page of this pattern's notes at the current revision before full replacement; partial reads cannot safely replace a pattern"));
            }
        }
        if kind == "channel.create" || kind == "instrument.replace" {
            let wanted = input["instrumentId"].as_str().ok_or_else(|| Error::bad("instrumentId must come from the catalog"))?;
            let mut found = false;
            let mut offset = 0;
            loop {
                let page = self.client.call("instrument.catalog", json!({"offset":offset,"limit":512})).await?;
                crate::project::check_native_result(&page, &session)?;
                if let Some(entry) = array(&page, "instruments").iter().find(|e| e["instrumentId"] == wanted) {
                    if entry["available"] != true {
                        return Err(Error::bad("This catalog instrument is unavailable in the current LYNX installation"));
                    }
                    found = true;
                    break;
                }
                let Some(next) = page["nextOffset"].as_u64() else { break };
                if next <= offset || next > 100000 {
                    return Err(Error::bad("Invalid instrument catalog cursor"));
                }
                offset = next;
            }
            if !found {
                return Err(Error::bad("instrumentId is not in LYNX's already-known instrument catalog; paths are not accepted"));
            }
        }
        let (id, display, target, prior) =
            (auth::random_hex16(), display(&kind, &input, &session), target(&kind, &input, &session), prior(&kind, &input, &session));
        let (project, control, rev) = identity(&session)?;
        let fence = json!({"id":id,"projectId":project,"controlEpoch":control,"revision":rev});
        let answer = json!({"epoch":epoch(&session),"layout":rev,"target":target,"prior":prior,"fence":fence,"display":display});
        self.journal.trim()?;
        self.journal.entries.push_back(Entry {
            id,
            kind,
            input,
            fence,
            display,
            target,
            prior,
            project: project.into(),
            control: control.into(),
            revision: rev,
            after_revision: None,
            desc: None,
            phase: Phase::Previewed,
            answer: None,
            parameters: None,
        });
        Ok(answer)
    }

    pub async fn apply(&mut self, params: &Value) -> Result<Value> {
        let id = params.pointer("/fence/id").and_then(Value::as_str).ok_or_else(|| Error::bad("Apply requires the exact preview fence"))?;
        let index = self.journal.find(id)?;
        let entry = &self.journal.entries[index];
        if params["fence"] != entry.fence || params["kind"] != entry.kind {
            return Err(Error::bad("Apply kind or fence differs from its preview"));
        }
        let mut given = params["input"].clone();
        let n = integer(&given, "expectedRevision")?;
        given["expectedRevision"] = n.into();
        if given != entry.input {
            return Err(Error::bad("Apply input differs from its preview"));
        }
        match entry.phase {
            Phase::Applied => {
                return entry
                    .answer
                    .clone()
                    .ok_or_else(|| Error::new(code::UNCERTAIN, "Applied change has no answer; inspect before any retry"))
            }
            Phase::Undone => return Err(Error::new(code::CHANGED, "This change was already undone; preview a new change")),
            Phase::Uncertain => {
                return Err(Error::new(code::UNCERTAIN, "This apply may already have run; inspect the project. It will not be sent again"))
            }
            Phase::Previewed => {}
        }
        let (kind, mut input, project, control, expected) =
            (entry.kind.clone(), entry.input.clone(), entry.project.clone(), entry.control.clone(), entry.revision);
        let session = self.session().await?;
        let (current_project, current_control, current_revision) = identity(&session)?;
        if current_project != project || current_control != control || current_revision != expected {
            return Err(Error::new(code::CHANGED, "The project changed between preview and apply; nothing was sent"));
        }
        if session["playing"] == true || session["recording"] == true {
            return Err(Error::new(code::BUSY, "Stop playback and recording before applying changes"));
        }
        let desc = params["desc"]
            .as_str()
            .filter(|s| !s.is_empty() && s.len() <= 512 && !s.chars().any(char::is_control))
            .ok_or_else(|| Error::bad("Apply needs its journal description"))?
            .to_owned();
        if self.journal.entries.iter().any(|e| e.id != id && e.desc.as_deref() == Some(&desc)) {
            return Err(Error::bad("A journal description was reused for a different change"));
        }
        input["expectedProjectId"] = project.clone().into();
        input["expectedEpoch"] = control.clone().into();
        self.journal.entries[index].phase = Phase::Uncertain;
        let native = match self.client.call(&kind, input).await {
            Ok(result) => result,
            Err(error) => {
                let error = self.classify_write_failure(error, &project, &control, expected).await;
                if error.code != code::UNCERTAIN {
                    self.journal.entries[index].phase = Phase::Previewed;
                }
                return Err(error);
            }
        };
        let after = native["revision"].as_u64().filter(|rev| *rev >= expected);
        if native["projectId"] != project || native["controlEpoch"] != control || after.is_none() {
            return Err(Error::new(
                code::UNCERTAIN,
                "Native write response has mismatched ownership metadata; inspect the project before any history action",
            ));
        }
        let after = after.unwrap();
        let changed = after > expected;
        let epoch_after = format!("{control}:{project}:{after}");
        // The native response proves the operation landed. A missing ownership fence
        // disables targeted undo; it must not turn a known successful apply into an uncertain write.
        let fingerprint = if changed {
            match self.session().await {
                Ok(inspected) if epoch(&inspected) == epoch_after => self.parameter_fingerprint(&inspected).await.ok(),
                _ => None,
            }
        } else {
            None
        };
        let entry = &mut self.journal.entries[index];
        let mut answer = json!({"epoch":epoch_after,"layout":after,"applied":native,"display":entry.display,"target":entry.target,
            "undo":{"desc":desc,"entry":changed,"covered":true}});
        if changed && fingerprint.is_none() {
            answer["display"]["note"] = "Applied as a native undo step. Kumi could not establish the device-state fence, so targeted undo is withheld; inspect the project before continuing.".into();
        }
        entry.phase = Phase::Applied;
        entry.after_revision = Some(after);
        entry.desc = Some(desc);
        entry.answer = Some(answer.clone());
        entry.parameters = fingerprint;
        if changed {
            self.journal.owned.push(id.into());
        }
        Ok(answer)
    }

    pub async fn restore(&mut self, params: &Value) -> Result<Value> {
        let desc = params["of"].as_str().ok_or_else(|| Error::bad("Undo must name its applied journal description"))?;
        let index = self
            .journal
            .entries
            .iter()
            .position(|e| e.desc.as_deref() == Some(desc))
            .ok_or_else(|| Error::new(code::CHANGED, "This bridge no longer owns that undo record"))?;
        let entry = &self.journal.entries[index];
        if params["kind"] != entry.kind {
            return Err(Error::bad("Undo kind does not match the applied change"));
        }
        if params.get("prior") != Some(&entry.prior) || params.get("target") != Some(&entry.target) {
            return Err(Error::bad("Undo prior state or target differs from its recorded preview"));
        }
        if entry.phase == Phase::Undone {
            return Ok(json!({"restored":true,"already":true}));
        }
        if entry.phase != Phase::Applied || self.journal.owned.last() != Some(&entry.id) {
            return Err(Error::new(
                code::CHANGED,
                "Only the most recent still-owned native undo step can be undone; later changes must be undone first",
            ));
        }
        if entry.answer.as_ref().and_then(|a| a.get("applied")) != params.get("applied") {
            return Err(Error::bad("Undo does not carry the recorded applied state"));
        }
        let (id, project, control, expected, before_revision, fingerprint) = (
            entry.id.clone(),
            entry.project.clone(),
            entry.control.clone(),
            entry.after_revision.unwrap_or(entry.revision),
            entry.revision,
            entry.parameters.clone(),
        );
        let session = self.session().await?;
        let (p, c, rev) = identity(&session)?;
        if p != project || c != control || rev != expected {
            return Err(Error::new(
                code::CHANGED,
                "A producer edit, restart or project switch followed this change; native undo was intentionally withheld",
            ));
        }
        if session["playing"] == true || session["recording"] == true {
            return Err(Error::new(code::BUSY, "Stop playback and recording before undo"));
        }
        let current = self
            .parameter_fingerprint(&session)
            .await
            .map_err(|_| Error::new(code::CHANGED, "Device state could not be verified; native undo was intentionally withheld"))?;
        if fingerprint.as_deref() != Some(current.as_str()) {
            return Err(Error::new(code::CHANGED,"Device state changed after this apply, even though the song revision may be unchanged; native undo was intentionally withheld"));
        }
        let args =
            json!({"expectedProjectId":project,"expectedEpoch":control,"expectedRevision":expected,"expectedDeviceStateHash":current});
        self.journal.entries[index].phase = Phase::Uncertain;
        let native = match self.client.call("history.undo", args).await {
            Ok(result) => result,
            Err(error) => {
                let error = self.classify_write_failure(error, &project, &control, expected).await;
                if error.code != code::UNCERTAIN {
                    self.journal.entries[index].phase = Phase::Applied;
                }
                return Err(error);
            }
        };
        if native["changed"] == false && native["revision"].as_u64() == Some(expected) {
            self.journal.entries[index].phase = Phase::Applied;
            return Err(Error::new(code::CHANGED, "The native undo step is no longer present; nothing was undone"));
        }
        let after = native["revision"].as_u64().filter(|n| *n > expected);
        if native["projectId"] != project || native["controlEpoch"] != control || native["changed"] != true || after.is_none() {
            return Err(Error::new(code::UNCERTAIN, "Native undo ownership could not be verified; it will not be repeated"));
        }
        let after = after.unwrap();
        self.journal.entries[index].phase = Phase::Undone;
        if self.journal.owned.pop().as_deref() != Some(&id) {
            return Err(Error::new(code::INTERNAL, "Undo ownership stack changed unexpectedly"));
        }
        if let Some(previous) = self.journal.owned.last() {
            if let Ok(previous) = self.journal.find(previous) {
                if self.journal.entries[previous].after_revision == Some(before_revision) {
                    self.journal.entries[previous].after_revision = Some(after);
                }
            }
        }
        Ok(json!({"restored":true,"epoch":format!("{control}:{project}:{after}"),"layout":after,"native":native}))
    }

    pub async fn transport_action(&mut self, play: bool) -> Result<Value> {
        if !play {
            return self.client.call("transport.stop", json!({})).await;
        }
        let session = self.session().await?;
        let (project, control, revision) = identity(&session)?;
        self.client.call("transport.play", json!({"expectedProjectId":project,"expectedEpoch":control,"expectedRevision":revision})).await
    }

    async fn classify_write_failure(&self, error: Error, project: &str, control: &str, expected: u64) -> Error {
        if error.code == code::UNCERTAIN {
            return error;
        }
        match self.status().await {
            Ok(status) if identity(&status).is_ok_and(|(p, c, r)| p == project && c == control && r == expected) => error,
            _ => Error::new(
                code::UNCERTAIN,
                "LYNX refused or lost a write response, but the unchanged revision could not be proved; inspect before retrying",
            ),
        }
    }

    /// Native hash includes opaque hosted and nested device state, not just exposed parameters.
    async fn parameter_fingerprint(&self, session: &Value) -> Result<String> {
        let value = self.client.call("device.state", json!({})).await?;
        crate::project::check_native_result(&value, session)?;
        value["deviceStateHash"]
            .as_str()
            .filter(|hash| hash.len() == 64 && hash.bytes().all(|byte| byte.is_ascii_hexdigit() && !byte.is_ascii_uppercase()))
            .map(str::to_owned)
            .ok_or_else(|| Error::new(code::FAILED, "Native device-state fence is missing or malformed"))
    }
}

fn validate_target(kind: &str, input: &Value, session: &Value) -> Result<()> {
    let index = |key: &str, count: usize, append: bool| -> Result<()> {
        let n = integer(input, key)?;
        if n as u128 >= count as u128 + u128::from(append) {
            return Err(Error::bad(format!("{key} is outside its current collection")));
        }
        Ok(())
    };
    match kind {
        "project.edit" => {
            for (key, size) in [("snapshotId", 32), ("expectedDocumentHash", 64)] {
                if !input[key]
                    .as_str()
                    .is_some_and(|text| text.len() == size && text.bytes().all(|b| b.is_ascii_hexdigit() && !b.is_ascii_uppercase()))
                {
                    return Err(Error::bad(format!("{key} must come from the complete document read")));
                }
            }
            let patch = input["patch"]
                .as_array()
                .filter(|patch| !patch.is_empty() && patch.len() <= 128)
                .ok_or_else(|| Error::bad("patch must contain 1–128 JSON Patch operations"))?;
            for operation in patch {
                let action = operation["op"].as_str().ok_or_else(|| Error::bad("Each patch operation requires op"))?;
                let path = operation["path"]
                    .as_str()
                    .filter(|path| path.len() <= 4096 && path.starts_with('/'))
                    .ok_or_else(|| Error::bad("Each patch path must be a bounded non-root JSON Pointer"))?;
                let _ = path;
                if !matches!(action, "test" | "add" | "replace" | "remove") || (action != "remove" && operation.get("value").is_none()) {
                    return Err(Error::bad("Patch op must be test/add/replace/remove with value except for remove"));
                }
                if operation.as_object().is_none_or(|fields| fields.keys().any(|key| !matches!(key.as_str(), "op" | "path" | "value"))) {
                    return Err(Error::bad("JSON Patch contains an unknown operation field"));
                }
            }
        }
        "mixer.set" => index("index", array(session, "mixer").len(), false)?,
        "channel.set" => index("index", array(session, "channels").len(), false)?,
        "pattern.replace_notes" => {
            index("index", array(session, "patterns").len(), false)?;
            if !input["notes"].is_array() {
                return Err(Error::bad("notes must be a complete array"));
            }
        }
        "instrument.replace" => index("channelIndex", array(session, "channels").len(), false)?,
        "channel.create" => {
            if input.get("channelIndex").is_some() {
                index("channelIndex", array(session, "channels").len(), true)?;
            }
            if input.get("mixerIndex").is_some() {
                index("mixerIndex", array(session, "mixer").len(), false)?;
            }
        }
        "effect.load" | "effect.remove" => index("mixerIndex", array(session, "mixer").len(), false)?,
        _ => {}
    }
    Ok(())
}
fn target(kind: &str, input: &Value, session: &Value) -> Value {
    let field = match kind {
        "mixer.set" => Some("mixer"),
        "channel.set" | "instrument.replace" => Some("channels"),
        "pattern.replace_notes" => Some("patterns"),
        _ => None,
    };
    if let Some(field) = field {
        let index = input.get("index").or_else(|| input.get("channelIndex")).and_then(Value::as_u64).unwrap_or(0) as usize;
        return array(session, field).get(index).cloned().unwrap_or(Value::Null);
    }
    input.get("target").cloned().unwrap_or_else(|| json!({"operation":kind}))
}
fn prior(kind: &str, input: &Value, session: &Value) -> Value {
    if kind == "tempo.set" {
        json!({"bpm":session["bpm"]})
    } else {
        target(kind, input, session)
    }
}
fn display(kind: &str, input: &Value, session: &Value) -> Value {
    let title = match kind {
        "tempo.set" => format!("Tempo {} → {} BPM", session["bpm"], input["bpm"]),
        "channel.create" => {
            format!("Add instrument {}", input["name"].as_str().or_else(|| input["instrumentId"].as_str()).unwrap_or("channel"))
        }
        "instrument.replace" => format!("Replace instrument on channel {}", input["channelIndex"]),
        "channel.set" => format!("Change channel {}: {}", input["index"], settings(input)),
        "mixer.set" => format!("Change mixer insert {}: {}", input["index"], settings(input)),
        "pattern.replace_notes" => {
            format!("Pattern {}: {} → {} notes", input["index"], target(kind, input, session)["noteCount"], array(input, "notes").len())
        }
        "plugin.parameters_set" => format!("Change {} plugin parameters", array(input, "parameters").len()),
        "effect.load" => format!("Load {} on mixer insert {}", input["effectId"].as_str().unwrap_or("effect"), input["mixerIndex"]),
        "effect.remove" => format!("Remove effect {} from mixer insert {}", input["effectIndex"], input["mixerIndex"]),
        "playlist.edit" => format!("Edit {} Playlist clips", array(input, "edits").len()),
        "project.edit" => format!("Edit project document in {} atomic operations", array(input, "patch").len()),
        _ => kind.into(),
    };
    let unchanged = kind == "tempo.set" && input["bpm"].as_f64() == session["bpm"].as_f64();
    json!({"title":title.chars().filter(|c|!c.is_control()).take(512).collect::<String>(),"unchanged":unchanged})
}
fn settings(input: &Value) -> String {
    ["volume", "pan", "muted", "solo"].iter().filter_map(|key| input.get(*key).map(|v| format!("{key} {v}"))).collect::<Vec<_>>().join(", ")
}
