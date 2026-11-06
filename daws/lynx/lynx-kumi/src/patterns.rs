//! Pattern tools translated into the published native document API.
use crate::{
    integer,
    project::{epoch, Bridge},
    Error, Result,
};
use daw_bridge::wire::code;
use serde_json::{json, Value};

pub(crate) fn is_kind(kind: &str) -> bool {
    matches!(kind, "pattern.create" | "pattern.clone" | "pattern.set")
}
pub(crate) struct Plan {
    pub input: Value,
    pub index: usize,
    pub unchanged: bool,
}
impl Bridge {
    pub(crate) async fn pattern_plan(&self, kind: &str, input: &Value, session: &Value) -> Result<Plan> {
        let doc = self.fetch_document(session).await?;
        let (patch, index, unchanged) = patch(kind, input, &doc.value)?;
        let input = json!({"snapshotId":doc.id,"expectedDocumentHash":doc.hash,"expectedRevision":session["revision"],"patch":patch});
        if input.to_string().len() > 1024 * 1024 - 4096 {
            return Err(Error::bad("This pattern edit exceeds the native atomic document bound; no change was sent"));
        }
        Ok(Plan { input, index, unchanged })
    }
    pub(crate) async fn pattern_metadata(&mut self, session: &mut Value) -> Result<()> {
        if self.document.as_ref().is_none_or(|doc| doc.epoch != epoch(session)) {
            self.document = Some(self.fetch_document(session).await?);
        }
        let doc = &self.document.as_ref().ok_or_else(|| Error::new(code::INTERNAL, "Document cache is absent"))?.value;
        let patterns = list(doc, "patterns")?;
        let beats = integer(doc, "beatsPerBar")?;
        if session["overviewUnavailable"] == true {
            session["patterns"] = patterns
                .iter()
                .enumerate()
                .map(|(index, pattern)| {
                    let uid = pattern["uid"].as_str().ok_or_else(|| Error::bad("Native document pattern identity is absent"))?;
                    Ok(json!({"index":index,"patternId":uid,"name":pattern["name"],"noteCount":list(pattern,"notes")?.len()}))
                })
                .collect::<Result<Vec<_>>>()?
                .into();
        }
        let rows = session["patterns"].as_array_mut().ok_or_else(|| Error::bad("Native pattern overview is malformed"))?;
        if rows.len() != patterns.len() {
            return Err(Error::new(code::CHANGED, "Native document and pattern overview differ; read again"));
        }
        for (row, pattern) in rows.iter_mut().zip(patterns) {
            row["lengthTicks"] = pattern.get("length").cloned().unwrap_or(json!(0));
            row["effectiveLengthTicks"] = effective_length(pattern, beats)?.into();
        }
        Ok(())
    }
}
fn list<'a>(value: &'a Value, name: &str) -> Result<&'a Vec<Value>> {
    value[name].as_array().ok_or_else(|| Error::bad(format!("Native document {name} is not an array")))
}
fn cell(value: &Value, index: usize) -> Result<u64> {
    value.get(index).and_then(Value::as_u64).ok_or_else(|| Error::bad("Native pattern/reference data is malformed"))
}
fn effective_length(pattern: &Value, beats: u64) -> Result<u64> {
    let length = pattern.get("length").map(|_| integer(pattern, "length")).transpose()?.unwrap_or(0);
    if length > 0 {
        return Ok(length);
    }
    let bar = beats.max(1).checked_mul(960).ok_or_else(|| Error::bad("Native meter overflows"))?;
    let mut end = 0;
    for note in list(pattern, "notes")? {
        end = end.max(cell(note, 0)?.checked_add(cell(note, 1)?.max(1)).ok_or_else(|| Error::bad("Native note end overflows"))?);
    }
    let bars = end.checked_add(bar - 1).ok_or_else(|| Error::bad("Native pattern length overflows"))? / bar;
    Ok(bar.max(bars.checked_mul(bar).ok_or_else(|| Error::bad("Native pattern length overflows"))?))
}
fn unique_name(patterns: &[Value], base: Option<&str>) -> String {
    for number in if base.is_some() { 2.. } else { 1.. } {
        let name = base.map_or_else(|| format!("Pattern {number}"), |name| format!("{name} #{number}"));
        if !patterns.iter().any(|p| p["name"] == name) {
            return name;
        }
    }
    unreachable!()
}
fn change_clips(value: &mut Value, at: usize, insertion: bool, lengths: Option<(u64, u64)>) -> Result<()> {
    let clips = value.as_array_mut().ok_or_else(|| Error::bad("Native clip list is malformed"))?;
    for clip in clips {
        if clip.get(5).and_then(Value::as_u64).unwrap_or(0) != 0 {
            continue;
        }
        let pattern = cell(clip, 0)?;
        if insertion && pattern >= at as u64 {
            clip[0] = (pattern + 1).into();
        }
        if !insertion && pattern == at as u64 {
            if let Some((before, after)) = lengths {
                if cell(clip, 4)? == 0 && cell(clip, 3)? == before {
                    clip[3] = after.into();
                }
            }
        }
    }
    Ok(())
}
fn shift_blocks(value: &mut Value, at: usize) -> Result<()> {
    for block in value.as_array_mut().ok_or_else(|| Error::bad("Native Pattern Blocks are malformed"))? {
        let pattern = cell(block, 0)?;
        if pattern >= at as u64 {
            block[0] = (pattern + 1).into();
        }
    }
    Ok(())
}
fn patch(kind: &str, input: &Value, document: &Value) -> Result<(Vec<Value>, usize, bool)> {
    let patterns = list(document, "patterns")?;
    let source = input.get("index").map(|_| integer(input, "index")).transpose()?.unwrap_or(patterns.len() as u64) as usize;
    let insertion = kind != "pattern.set";
    let index = if kind == "pattern.clone" { source + 1 } else { source };
    if source > patterns.len() || (kind != "pattern.create" && source >= patterns.len()) || (insertion && patterns.len() >= 100000) {
        return Err(Error::bad("Pattern index/count is outside the native collection"));
    }
    let mut operations = Vec::new();
    let mut lengths = None;
    if insertion {
        let mut value =
            if kind == "pattern.clone" { patterns[source].clone() } else { json!({"name":unique_name(patterns,None),"notes":[]}) };
        if kind == "pattern.clone" {
            value.as_object_mut().ok_or_else(|| Error::bad("Native pattern is malformed"))?.remove("uid");
            value["name"] = unique_name(patterns, value["name"].as_str()).into();
        }
        if let Some(name) = input.get("name") {
            value["name"] = name.clone();
        }
        if let Some(length) = input.get("lengthTicks").and_then(Value::as_u64).filter(|n| *n > 0) {
            value["length"] = length.into();
        }
        operations.push(json!({"op":"add","path":format!("/patterns/{index}"),"value":value}));
        if document["currentPattern"].as_u64() != Some(index as u64) {
            operations.push(json!({"op":"replace","path":"/currentPattern","value":index}));
        }
    } else {
        let before = &patterns[index];
        if let Some(name) = input.get("name").filter(|name| **name != before["name"]) {
            operations.push(json!({"op":"replace","path":format!("/patterns/{index}/name"),"value":name}));
        }
        if let Some(length) = input.get("lengthTicks") {
            let length = length.as_u64().ok_or_else(|| Error::bad("lengthTicks must be an integer"))?;
            if length > 0
                && list(before, "notes")?.iter().any(|note| {
                    let start = note.get(0).and_then(Value::as_u64);
                    let size = note.get(1).and_then(Value::as_u64);
                    match (start, size) {
                        (Some(start), Some(size)) => start > length || size > length.saturating_sub(start),
                        _ => true,
                    }
                })
            {
                return Err(Error::bad("lengthTicks would hide existing notes"));
            }
            if before.get("length").and_then(Value::as_u64).unwrap_or(0) != length {
                let mut after = before.clone();
                let object = after.as_object_mut().ok_or_else(|| Error::bad("Native pattern is malformed"))?;
                if length == 0 {
                    object.remove("length");
                } else {
                    object.insert("length".into(), length.into());
                }
                lengths = Some((
                    effective_length(before, integer(document, "beatsPerBar")?)?,
                    effective_length(&after, integer(document, "beatsPerBar")?)?,
                ));
                let mut op = json!({"op":if length == 0 {"remove"} else {"add"},"path":format!("/patterns/{index}/length")});
                if length > 0 {
                    op["value"] = length.into();
                }
                operations.push(op);
            }
        }
    }
    for key in ["playlist", "blocks", "arrangements"] {
        let Some(before) = document.get(key) else { continue };
        let mut after = before.clone();
        if key == "playlist" {
            change_clips(&mut after, index, insertion, lengths)?;
        }
        if key == "blocks" && insertion {
            shift_blocks(&mut after, index)?;
        }
        if key == "arrangements" {
            for arrangement in after.as_array_mut().ok_or_else(|| Error::bad("Native arrangements are malformed"))? {
                if let Some(clips) = arrangement.get_mut("clips") {
                    change_clips(clips, index, insertion, lengths)?;
                }
                if insertion {
                    if let Some(blocks) = arrangement.get_mut("blocks") {
                        shift_blocks(blocks, index)?;
                    }
                }
            }
        }
        if after != *before {
            operations.push(json!({"op":"replace","path":format!("/{key}"),"value":after}));
        }
    }
    if insertion {
        for (i, channel) in list(document, "channels")?.iter().enumerate() {
            if let Some(loop_starter) = channel.get("loopStarter") {
                let pattern = cell(loop_starter, 0)?;
                if pattern >= index as u64 {
                    operations.push(json!({"op":"replace","path":format!("/channels/{i}/loopStarter/0"),"value":pattern+1}));
                }
            }
        }
    }
    let unchanged = operations.is_empty();
    if unchanged {
        operations.push(json!({"op":"test","path":format!("/patterns/{index}/name"),"value":patterns[index]["name"]}));
    }
    if operations.len() > 128 {
        return Err(Error::bad("Pattern reference remapping exceeds the native atomic 128-operation bound"));
    }
    Ok((operations, index, unchanged))
}
