//! Bounded own-bridge rows and revision-bound positional references.
use crate::{changes::Journal, integer, native::Client, Error, Result};
use daw_bridge::{auth, wire::code};
use serde_json::{json, Value};
use std::collections::HashMap;

pub const PAGE_BYTES: usize = 48 * 1024;
pub const READ_OPS: &[&str] = &[
    "api.describe",
    "status",
    "changes",
    "device.state",
    "project.read",
    "session.inspect",
    "pattern.notes",
    "plugin.inspect",
    "effect.catalog",
    "instrument.catalog",
    "playlist.inspect",
];
pub const WRITE_OPS: &[&str] = &[
    "tempo.set",
    "mixer.set",
    "channel.set",
    "pattern.replace_notes",
    "plugin.parameters_set",
    "effect.load",
    "effect.remove",
    "playlist.edit",
    "channel.create",
    "instrument.replace",
    "project.edit",
];

pub struct Bridge {
    pub client: Client,
    pub(crate) journal: Journal,
    pub(crate) document: Option<crate::document::Document>,
    /// Only pages actually returned whole to the host count as complete note reads.
    notes_read: HashMap<(String, u64), Vec<(u64, u64)>>,
}
impl Bridge {
    pub fn new(client: Client) -> Self {
        Self { client, journal: Journal::default(), document: None, notes_read: HashMap::new() }
    }

    pub async fn session(&mut self) -> Result<Value> {
        let session = match self.client.call("session.inspect", json!({})).await {
            Ok(session) => session,
            Err(error) if error.message.starts_with("LYNX [limit]:") => {
                let mut status = self.status().await?;
                status["overviewUnavailable"] = true.into();
                for key in ["channels", "mixer", "patterns", "playlist"] {
                    status[key] = json!([]);
                }
                status
            }
            Err(error) => return Err(error),
        };
        identity(&session)?;
        let current = epoch(&session);
        self.notes_read.retain(|(of, _), _| *of == current);
        Ok(session)
    }

    pub async fn status(&self) -> Result<Value> {
        let status = self.client.call("status", json!({})).await?;
        identity(&status)?;
        Ok(status)
    }

    pub async fn read(&mut self, params: &Value) -> Result<Value> {
        let mut session = self.session().await?;
        check_epoch(params, &session)?;
        let mut params = params.clone();
        let direct = params["ref"].as_str().map(|r| parse_ref(r, &session)).transpose()?;
        if let Some((kind, parts)) = &direct {
            if params.get("kind").is_none() {
                params["kind"] = kind.clone().into();
            }
            if params.get("parent").is_none() {
                if kind == "note" && parts.len() == 2 {
                    params["parent"] = reference("pattern", &parts[..1], &session).into();
                }
                if kind == "parameter" && parts.len() >= 3 {
                    params["parent"] = reference("device", &parts[..parts.len() - 1], &session).into();
                }
            }
        }
        let params = &params;
        let kind = params["kind"].as_str().unwrap_or("");
        if kind == "call" {
            return self.call_read(params, &session).await;
        }
        let kind = alias(kind);
        if kind == "pattern" && session["playing"] != true && session["recording"] != true {
            self.pattern_metadata(&mut session).await?;
        }
        if kind == "document" {
            return self.document_read(params, &session).await;
        }
        let limit = params.get("limit").map(|_| integer(params, "limit")).transpose()?.unwrap_or(50);
        if !(1..=200).contains(&limit) {
            return Err(Error::bad("limit must be from 1 to 200"));
        }
        if session["overviewUnavailable"] == true && matches!(kind, "track" | "mixer" | "pattern") {
            return Err(Error::new(
                code::FAILED,
                "This project exceeds the bounded overview. Read kind document with args.path to inspect every native project field",
            ));
        }
        let key = format!("{kind}:{}", params["parent"].as_str().unwrap_or(""));
        let offset = match &direct {
            Some((of, parts)) if matches!(of.as_str(), "note" | "parameter" | "instrument" | "effect" | "clip") => {
                *parts.last().ok_or_else(|| Error::bad("Malformed direct object ref"))?
            }
            _ => cursor_offset(params, &session, &key)?,
        };
        let mut rows: Vec<Value>;
        let mut total = None;
        let mut note_page = None;
        match kind {
            "project" => rows = vec![project(&session)],
            "track" | "mixer" | "pattern" => {
                let field = match kind {
                    "track" => "channels",
                    "mixer" => "mixer",
                    _ => "patterns",
                };
                rows = array(&session, field).iter().enumerate().map(|(i, row)| row_at(row, kind, &[i as u64], &session)).collect();
            }
            "note" => {
                let parent = parent(params, "pattern", &session)?;
                let pattern = *parent.first().ok_or_else(|| Error::bad("A pattern parent ref is required"))?;
                let page = self.client.call("pattern.notes", json!({"index":pattern,"offset":offset,"limit":limit.min(100)})).await?;
                check_native_result(&page, &session)?;
                let count = integer(&page, "totalNotes")?;
                rows = array(&page, "notes")
                    .iter()
                    .enumerate()
                    .map(|(i, n)| row_at(n, "note", &[pattern, offset + i as u64], &session))
                    .collect();
                total = Some(count);
                note_page = Some((pattern, offset));
            }
            "device" => {
                let devices = self.client.call("plugin.inspect", json!({})).await?;
                check_native_result(&devices, &session)?;
                rows = array(&devices, "devices")
                    .iter()
                    .map(|device| row_at(device, "device", &target_parts(&device["target"]), &session))
                    .collect();
            }
            "parameter" => {
                let parts = parent(params, "device", &session)?;
                let target = target_from_parts(&parts)?;
                let page = self.client.call("plugin.inspect", json!({"target":target,"offset":offset,"limit":limit.min(128)})).await?;
                check_native_result(&page, &session)?;
                total = Some(integer(&page, "parameterCount")?);
                rows = array(&page, "parameters")
                    .iter()
                    .map(|p| {
                        let mut parts = parts.clone();
                        parts.push(p["index"].as_u64().unwrap_or(0));
                        row_at(p, "parameter", &parts, &session)
                    })
                    .collect();
            }
            "instrument" | "effect" => {
                let op = if kind == "instrument" { "instrument.catalog" } else { "effect.catalog" };
                let page = self.client.call(op, json!({"offset":offset,"limit":limit})).await?;
                check_native_result(&page, &session)?;
                total = Some(integer(&page, "count")?);
                rows = array(&page, if kind == "instrument" { "instruments" } else { "effects" })
                    .iter()
                    .enumerate()
                    .map(|(i, row)| row_at(row, kind, &[offset + i as u64], &session))
                    .collect();
            }
            "clip" | "playlist" => {
                let page = self
                    .client
                    .call("playlist.inspect", json!({"offset":if kind=="clip" {offset} else {0},"limit":limit.min(100)}))
                    .await?;
                check_native_result(&page, &session)?;
                if kind == "clip" {
                    total = Some(integer(&page, "totalClips")?);
                    rows = array(&page, "clips")
                        .iter()
                        .enumerate()
                        .map(|(i, row)| row_at(row, "clip", &[offset + i as u64], &session))
                        .collect();
                } else {
                    rows = vec![json!({"ref":reference("playlist",&[page["currentArrangement"].as_u64().unwrap_or(0)],&session),
                        "currentArrangement":page["currentArrangement"],"totalClips":page["totalClips"],
                        "patternsTotal":page["patternsTotal"],"audioChannelsTotal":page["audioChannelsTotal"],
                        "automationsTotal":page["automationsTotal"],"ppq":960})];
                }
            }
            _ => {
                return Err(Error::bad(
                    "Read project, track/channel, mixer, pattern, note, device, parameter, instrument, effect, playlist, clip or call",
                ))
            }
        }
        let paged_native = total.is_some();
        let mut total = total.unwrap_or(rows.len() as u64);
        if let Some(given) = params["ref"].as_str() {
            let (of, parts) = parse_ref(given, &session)?;
            if alias(&of) != kind {
                return Err(Error::bad("ref and kind name different objects"));
            }
            let wanted = reference(kind, &parts, &session);
            rows.retain(|row| row["ref"] == wanted);
            if rows.is_empty() {
                return Err(Error::new("gone", "That object is not present in this read; use its parent and a paged listing"));
            }
            total = 1;
        }
        let from = if paged_native { 0 } else { offset as usize };
        if from > rows.len() {
            return Err(Error::bad("Cursor is outside this listing"));
        }
        let selected = bounded_rows(rows.into_iter().skip(from), limit as usize)?;
        let count = selected.len() as u64;
        let mut result = json!({"epoch":epoch(&session),"layout":revision(&session)?,"revision":revision(&session)?,
            "items":selected,"total":total,"indexFrom":0});
        if direct.is_none() && offset + count < total {
            result["nextCursor"] = cursor(&session, &key, offset + count).into();
        }
        if let Some((pattern, offset)) = note_page {
            self.note_page(&session, pattern, offset, count);
        }
        Ok(result)
    }

    async fn call_read(&mut self, params: &Value, session: &Value) -> Result<Value> {
        let op = params["fn"]
            .as_str()
            .or_else(|| params["command"].as_str())
            .ok_or_else(|| Error::bad("fn must name an exposed native read"))?;
        if !READ_OPS.contains(&op) {
            return Err(Error::bad("call permits only native inspection operations; changes use change_preview/apply"));
        }
        let mut args = params.get("args").or_else(|| params.get("params")).cloned().unwrap_or(json!({}));
        if !args.is_object() {
            return Err(Error::bad("args must be an object"));
        }
        if op == "pattern.notes" {
            if args.get("offset").is_none() {
                args["offset"] = 0.into();
            }
            if args.get("limit").is_none() {
                args["limit"] = 100.into();
            }
        }
        let result = self.client.call(op, args.clone()).await?;
        check_native_result(&result, session)?;
        let answer = json!({"epoch":epoch(session),"layout":revision(session)?,"revision":revision(session)?,"result":result});
        if answer.to_string().len() > PAGE_BYTES {
            return Err(Error::new("limit","This native read is too large to return whole. Use paged rows, or lower the native offset/limit; no complete read was recorded"));
        }
        if op == "pattern.notes" {
            self.note_page(session, integer(&args, "index")?, integer(&args, "offset")?, array(&answer["result"], "notes").len() as u64);
        }
        Ok(answer)
    }

    pub async fn snapshot(&mut self) -> Result<Value> {
        let session = self.session().await?;
        let rows = array(&session, "channels").iter().enumerate().map(|(i, row)| row_at(row, "track", &[i as u64], &session));
        let tracks = bounded_rows(rows, 50)?;
        let total = array(&session, "channels").len();
        let mut snapshot = json!({"epoch":epoch(&session),"layout":revision(&session)?,"revision":revision(&session)?,
            "project":project(&session),"transport":transport(&session),"tracks":tracks,"selection":selection(&session),
            "markers":[],"setup":{"capabilities":session["capabilities"],"nativeProtocol":"lynx-control/1"}});
        if snapshot["tracks"].as_array().unwrap().len() < total {
            snapshot["truncated"] = json!({"tracks":{"total":total,"nextCursor":cursor(&session,"track:",snapshot["tracks"].as_array().unwrap().len() as u64)}});
        }
        Ok(snapshot)
    }

    pub async fn focus_body(&mut self, params: &Value) -> Result<Value> {
        let mut request = params.clone();
        let kind = match params["kind"].as_str().unwrap_or("") {
            "pattern" => "note",
            "playlist" => "clip",
            "mixer" => "mixer",
            "fx" => "device",
            _ => return Err(Error::bad("Read a pattern, playlist, mixer or fx body")),
        };
        request["kind"] = kind.into();
        if kind == "note" && request.get("parent").is_none() {
            if let Some(reference) = request.get("ref").cloned() {
                request["parent"] = reference;
                request.as_object_mut().unwrap().remove("ref");
            } else {
                let s = self.session().await?;
                request["parent"] = reference("pattern", &[s["currentPattern"].as_u64().unwrap_or(0)], &s).into();
            }
        }
        self.read(&request).await
    }

    pub async fn digest(&mut self, params: &Value) -> Result<Value> {
        let session = self.session().await?;
        let mut records = Vec::new();
        for (kind, field, idfield) in [("track", "channels", "channelId"), ("pattern", "patterns", "patternId"), ("mixer", "mixer", "")] {
            for (i, row) in array(&session, field).iter().enumerate() {
                records.push(json!({"k":kind,"id":row[idfield].as_str().filter(|id|!id.is_empty()).map(str::to_owned).unwrap_or_else(||format!("{kind}:{i}")),
                    "n":row["name"],"o":i,"f":row}));
            }
        }
        for clip in array(&session, "playlist") {
            records.push(json!({"k":"clip","id":format!("clip:{}",clip["index"]),"n":"Playlist clip","o":clip["index"],"f":clip}));
        }
        let from = cursor_offset(params, &session, "digest")?;
        let total = records.len();
        let page = bounded_rows(records.into_iter().skip(from as usize), 100)?;
        let mut result = json!({"epoch":epoch(&session),"changed":revision(&session)?,"records":page});
        if from as usize + result["records"].as_array().unwrap().len() < total {
            result["nextCursor"] = cursor(&session, "digest", from + result["records"].as_array().unwrap().len() as u64).into();
        }
        if from == 0 {
            result["project"] = project(&session);
            result["facts"] = json!({"tempo":session["bpm"],"timeSignature":[session["beatsPerBar"],4]});
            result["summaries"] = json!({"track":"channels","pattern":"patterns","mixer":"mixer","clip":"Playlist clips"});
            result["bounded"] = json!({"coverage":"Channel/mixer values, pattern counts and Playlist placement; no deep plugin parameter or note-content digest yet. Clip IDs in this summary are positional."});
        }
        Ok(result)
    }

    fn note_page(&mut self, session: &Value, pattern: u64, offset: u64, count: u64) {
        let current = epoch(session);
        self.notes_read.retain(|(of, _), _| *of == current);
        if self.notes_read.len() > 512 {
            self.notes_read.clear();
        }
        let ranges = self.notes_read.entry((current, pattern)).or_default();
        ranges.push((offset, offset + count));
        ranges.sort_unstable();
        let mut merged: Vec<(u64, u64)> = Vec::new();
        for &(start, end) in ranges.iter() {
            if let Some(last) = merged.last_mut().filter(|last| last.1 >= start) {
                last.1 = last.1.max(end);
            } else {
                merged.push((start, end));
            }
        }
        if merged.len() > 1024 {
            merged.clear();
        }
        *ranges = merged;
    }
    pub(crate) fn complete_notes(&self, session: &Value, pattern: u64) -> bool {
        let Some(count) = array(session, "patterns").get(pattern as usize).and_then(|p| p["noteCount"].as_u64()) else { return false };
        if count == 0 {
            return true;
        }
        let Some(read) = self.notes_read.get(&(epoch(session), pattern)) else { return false };
        let mut ranges = read.clone();
        ranges.sort_unstable();
        let mut end = 0;
        for (start, to) in ranges {
            if start > end {
                return false;
            }
            end = end.max(to);
        }
        end >= count
    }
}

pub fn identity(session: &Value) -> Result<(&str, &str, u64)> {
    let project = session
        .pointer("/project/id")
        .and_then(Value::as_str)
        .filter(|s| !s.is_empty())
        .ok_or_else(|| Error::new(code::FAILED, "LYNX did not report a project identity"))?;
    let control = session["controlEpoch"]
        .as_str()
        .filter(|s| !s.is_empty())
        .ok_or_else(|| Error::new(code::FAILED, "LYNX did not report a control epoch"))?;
    Ok((project, control, revision(session)?))
}
pub fn revision(session: &Value) -> Result<u64> {
    integer(session, "revision")
}
pub fn epoch(session: &Value) -> String {
    format!(
        "{}:{}:{}",
        session["controlEpoch"].as_str().unwrap_or(""),
        session.pointer("/project/id").and_then(Value::as_str).unwrap_or(""),
        session["revision"]
    )
}
pub fn check_epoch(params: &Value, session: &Value) -> Result<()> {
    if params.get("epoch").and_then(Value::as_str).is_some_and(|e| e != epoch(session)) {
        return Err(Error::new(code::STALE, "The project, process or revision changed; read current refs again"));
    }
    Ok(())
}
pub fn check_native_result(result: &Value, session: &Value) -> Result<()> {
    let (project, control, rev) = identity(session)?;
    if result.get("projectId").and_then(Value::as_str) != Some(project)
        || result["controlEpoch"] != control
        || result["revision"].as_u64() != Some(rev)
    {
        return Err(Error::new(code::CHANGED, "The native project changed during this read; read again"));
    }
    Ok(())
}
pub fn array<'a>(value: &'a Value, key: &str) -> &'a [Value] {
    value.get(key).and_then(Value::as_array).map(Vec::as_slice).unwrap_or(&[])
}
pub fn project(session: &Value) -> Value {
    let mut result = session["project"].clone();
    result["identity"] = format!("{}:{}", session["controlEpoch"].as_str().unwrap_or(""), result["id"].as_str().unwrap_or("")).into();
    result["tempo"] = session["bpm"].clone();
    result["revision"] = session["revision"].clone();
    result["counts"] = json!({"channels":session.get("channelCount").cloned().unwrap_or_else(||array(session,"channels").len().into()),"mixer":array(session,"mixer").len(),"patterns":array(session,"patterns").len(),"clips":array(session,"playlist").len()});
    result["ref"] = reference("project", &[0], session).into();
    result
}
pub fn transport(session: &Value) -> Value {
    json!({"playing":session["playing"],"recording":session.get("recording").cloned().unwrap_or(false.into()),"tempo":session["bpm"],"positionBeats":session["positionTicks"].as_f64().unwrap_or(0.)/960.,"timeSignature":[session["beatsPerBar"],4]})
}
pub fn selection(session: &Value) -> Value {
    json!({"patternRef":reference("pattern",&[session["currentPattern"].as_u64().unwrap_or(0)],session)})
}
pub fn reference(kind: &str, parts: &[u64], session: &Value) -> String {
    format!(
        "l:{}:{}:{}:{}",
        alias(kind),
        parts.iter().map(u64::to_string).collect::<Vec<_>>().join(":"),
        ref_epoch(session),
        session["revision"]
    )
}
fn ref_epoch(session: &Value) -> String {
    auth::sha256_hex(format!("{}:{}", session["controlEpoch"], session["project"]["id"]).as_bytes())[..16].into()
}
fn row_at(row: &Value, kind: &str, parts: &[u64], session: &Value) -> Value {
    let mut row = row.clone();
    row["ref"] = reference(kind, parts, session).into();
    row
}
pub fn parse_ref(reference: &str, session: &Value) -> Result<(String, Vec<u64>)> {
    let parts = reference.split(':').collect::<Vec<_>>();
    if parts.len() < 5 || parts[0] != "l" {
        return Err(Error::bad("Use an exact LYNX ref returned by a current read"));
    }
    let rev = parts.last().and_then(|s| s.parse::<u64>().ok()).ok_or_else(|| Error::bad("Malformed LYNX ref revision"))?;
    if rev != revision(session)? {
        return Err(Error::new(code::CHANGED, "That positional ref predates a project edit; read again"));
    }
    if parts[parts.len() - 2] != ref_epoch(session) {
        return Err(Error::new(code::STALE, "That ref belongs to another process or project; read again"));
    }
    let indices = parts[2..parts.len() - 2]
        .iter()
        .map(|n| n.parse::<u64>().map_err(|_| Error::bad("Malformed LYNX ref index")))
        .collect::<Result<Vec<_>>>()?;
    Ok((alias(parts[1]).into(), indices))
}
fn parent(params: &Value, kind: &str, session: &Value) -> Result<Vec<u64>> {
    let parent = params["parent"].as_str().ok_or_else(|| Error::bad(format!("A current {kind} parent ref is required")))?;
    let (of, parts) = parse_ref(parent, session)?;
    if of != kind {
        return Err(Error::bad(format!("parent must be a {kind} ref")));
    }
    Ok(parts)
}
fn alias(kind: &str) -> &str {
    match kind {
        "session" => "project",
        "channel" => "track",
        "param" => "parameter",
        k => k,
    }
}
fn target_parts(target: &Value) -> Vec<u64> {
    if target["kind"] == "instrument" {
        vec![0, target["channelIndex"].as_u64().unwrap_or(0)]
    } else {
        vec![1, target["mixerIndex"].as_u64().unwrap_or(0), target["effectIndex"].as_u64().unwrap_or(0)]
    }
}
fn target_from_parts(parts: &[u64]) -> Result<Value> {
    match parts {
        [0, c] => Ok(json!({"kind":"instrument","channelIndex":c})),
        [1, m, e] => Ok(json!({"kind":"effect","mixerIndex":m,"effectIndex":e})),
        _ => Err(Error::bad("Malformed device parent ref")),
    }
}
fn bounded_rows(rows: impl Iterator<Item = Value>, limit: usize) -> Result<Vec<Value>> {
    let (mut result, mut size) = (Vec::new(), 1024usize);
    for row in rows {
        let bytes = row.to_string().len() + 1;
        if result.len() == limit || size + bytes > PAGE_BYTES {
            if result.is_empty() {
                return Err(Error::new("limit", "One row exceeds the read budget; use a narrower native call"));
            }
            break;
        }
        size += bytes;
        result.push(row);
    }
    Ok(result)
}
pub(crate) fn cursor(session: &Value, key: &str, offset: u64) -> String {
    format!("c:{offset}:{}", &auth::sha256_hex(format!("{}:{key}", epoch(session)).as_bytes())[..24])
}
pub(crate) fn cursor_offset(params: &Value, session: &Value, key: &str) -> Result<u64> {
    let Some(given) = params["cursor"].as_str() else { return Ok(0) };
    let parts = given.split(':').collect::<Vec<_>>();
    let offset =
        parts.get(1).and_then(|s| s.parse::<u64>().ok()).filter(|n| *n <= 1_000_000).ok_or_else(|| Error::bad("Malformed cursor"))?;
    if given != cursor(session, key, offset) {
        return Err(Error::new(code::CHANGED, "The listing changed since this cursor; read from the start"));
    }
    Ok(offset)
}
