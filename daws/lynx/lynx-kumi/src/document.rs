//! A bounded model view of LYNX's complete native project document.
use crate::{
    integer,
    project::{check_native_result, cursor, cursor_offset, epoch, revision, Bridge, PAGE_BYTES},
    Error, Result,
};
use base64::{engine::general_purpose::STANDARD, Engine};
use daw_bridge::{auth, wire::code};
use serde_json::{json, Value};

const MAX_DOCUMENT: u64 = 64 * 1024 * 1024;
pub(crate) struct Document {
    pub epoch: String,
    pub id: String,
    pub hash: String,
    pub value: Value,
}

impl Bridge {
    pub(crate) async fn document_read(&mut self, params: &Value, session: &Value) -> Result<Value> {
        let args = params.get("args").cloned().unwrap_or(json!({}));
        if !args.is_object() {
            return Err(Error::bad("Document args must be an object"));
        }
        let path = args["path"].as_str().unwrap_or("");
        if path.len() > 4096 || (!path.is_empty() && !path.starts_with('/')) {
            return Err(Error::bad("path must be a bounded JSON Pointer, or empty for the root"));
        }
        let refresh = args["refresh"] == true;
        let same = self.document.as_ref().is_some_and(|doc| doc.epoch == epoch(session));
        if refresh || !same {
            if params.get("cursor").is_some() || args.get("snapshotId").is_some() {
                return Err(Error::new(code::CHANGED, "That document snapshot expired; refresh from the root"));
            }
            self.document = Some(self.fetch_document(session).await?);
        }
        let doc = self.document.as_ref().ok_or_else(|| Error::new(code::INTERNAL, "Document cache is absent"))?;
        if args.get("snapshotId").is_some_and(|id| id.as_str() != Some(doc.id.as_str())) {
            return Err(Error::new(code::CHANGED, "Document snapshot identity changed; read it again"));
        }
        let value = doc.value.pointer(path).ok_or_else(|| Error::bad("This JSON Pointer is absent from the project document"))?;
        let key = format!("document:{}:{path}", doc.id);
        let offset = cursor_offset(params, session, &key)? as usize;
        let limit = params.get("limit").map(|_| integer(params, "limit")).transpose()?.unwrap_or(50);
        if !(1..=200).contains(&limit) {
            return Err(Error::bad("limit must be from 1 to 200"));
        }
        let mut rows = Vec::new();
        let mut bytes = 2048usize;
        let total;
        match value {
            Value::Array(values) => {
                total = values.len();
                for (index, value) in values.iter().enumerate().skip(offset).take(limit as usize) {
                    let path = format!("{path}/{index}");
                    if !push(&mut rows, &mut bytes, json!({"path":path,"value":summary(value,1),"type":kind(value)})) {
                        break;
                    }
                }
            }
            Value::Object(values) => {
                total = values.len();
                for (name, value) in values.iter().skip(offset).take(limit as usize) {
                    let path = format!("{path}/{}", name.replace('~', "~0").replace('/', "~1"));
                    if !push(&mut rows, &mut bytes, json!({"path":path,"key":name,"value":summary(value,1),"type":kind(value)})) {
                        break;
                    }
                }
            }
            Value::String(text) => {
                // Strings include opaque vendor state. Page by Unicode characters without corrupting UTF-8.
                total = text.chars().count();
                let chunk = text.chars().skip(offset).take(4096).collect::<String>();
                rows.push(json!({"path":path,"type":"string","value":chunk}));
            }
            _ => {
                total = 1;
                if offset == 0 {
                    rows.push(json!({"path":path,"type":kind(value),"value":value}));
                }
            }
        }
        if offset > total {
            return Err(Error::bad("Document cursor is outside its value"));
        }
        let count = if value.is_string() {
            rows.first().and_then(|row| row["value"].as_str()).map_or(0, |s| s.chars().count())
        } else {
            rows.len()
        };
        if count == 0 && offset < total {
            return Err(Error::new(code::FAILED, "This document value exceeds the bounded model page; inspect its child paths"));
        }
        let mut answer = json!({"epoch":epoch(session),"layout":revision(session)?,"revision":revision(session)?,"snapshotId":doc.id,"documentHash":doc.hash,"path":path,"items":rows,"total":total,"indexFrom":0});
        if offset + count < total {
            answer["nextCursor"] = cursor(session, &key, (offset + count) as u64).into();
        }
        Ok(answer)
    }

    pub(crate) async fn fetch_document(&self, session: &Value) -> Result<Document> {
        let mut offset = 0u64;
        let mut bytes = Vec::new();
        let mut id = String::new();
        let mut hash = String::new();
        loop {
            let mut args = json!({"offset":offset,"limit":32768});
            if offset > 0 {
                args["snapshotId"] = id.clone().into();
                args["expectedDocumentHash"] = hash.clone().into();
            }
            let page = self.client.call("project.read", args).await?;
            check_native_result(&page, session)?;
            let page_id = page["snapshotId"].as_str().ok_or_else(|| Error::bad("Native document snapshot identity is missing"))?;
            let page_hash = page["documentHash"].as_str().ok_or_else(|| Error::bad("Native document hash is missing"))?;
            if offset == 0 {
                id = page_id.into();
                hash = page_hash.into();
            }
            if page_id != id || page_hash != hash || page["offset"].as_u64() != Some(offset) || page["encoding"] != "base64" {
                return Err(Error::new(code::CHANGED, "Native document pages changed identity"));
            }
            let total = integer(&page, "totalBytes")?;
            if total > MAX_DOCUMENT {
                return Err(Error::new(code::FAILED, "Native document exceeds the 64 MiB inspection bound"));
            }
            let data = page["data"].as_str().ok_or_else(|| Error::bad("Native document page is missing data"))?;
            let chunk = STANDARD.decode(data).map_err(|_| Error::bad("Native document page has invalid base64"))?;
            if chunk.len() > 32768 || offset + chunk.len() as u64 > total {
                return Err(Error::bad("Native document page exceeds its bound"));
            }
            if chunk.is_empty() && offset < total {
                return Err(Error::bad("Native document page made no progress"));
            }
            offset += chunk.len() as u64;
            bytes.extend(chunk);
            match page["nextOffset"].as_u64() {
                Some(next) if next == offset && next < total && !bytes.is_empty() => {}
                None if offset == total => break,
                _ => return Err(Error::bad("Native document paging made no bounded progress")),
            }
        }
        if auth::sha256_hex(&bytes) != hash {
            return Err(Error::bad("Native document hash does not match its complete bytes"));
        }
        let value = serde_json::from_slice(&bytes).map_err(|_| Error::bad("Native document is invalid JSON"))?;
        Ok(Document { epoch: epoch(session), id, hash, value })
    }
}

fn kind(value: &Value) -> &'static str {
    match value {
        Value::Object(_) => "object",
        Value::Array(_) => "array",
        Value::String(_) => "string",
        Value::Number(_) => "number",
        Value::Bool(_) => "boolean",
        Value::Null => "null",
    }
}
fn summary(value: &Value, depth: usize) -> Value {
    match value {
        Value::Object(values) if depth > 0 && values.len() <= 128 => {
            values.iter().map(|(key, value)| (key.clone(), summary(value, depth - 1))).collect()
        }
        Value::Array(values) if depth > 0 && values.len() <= 32 => values.iter().map(|value| summary(value, depth - 1)).collect(),
        Value::Object(values) => json!({"type":"object","count":values.len(),"expand":true}),
        Value::Array(values) => json!({"type":"array","count":values.len(),"expand":true}),
        Value::String(text) if text.len() > 1024 => json!({"type":"string","characters":text.chars().count(),"expand":true}),
        value => value.clone(),
    }
}
fn push(rows: &mut Vec<Value>, bytes: &mut usize, row: Value) -> bool {
    let size = row.to_string().len();
    if *bytes + size > PAGE_BYTES {
        return false;
    }
    *bytes += size;
    rows.push(row);
    true
}
