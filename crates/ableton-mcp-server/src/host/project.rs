//! Read-only semantic/offline project tools and manifest-bound file backups.
use super::*;
use crate::{als::*, project as project_files, project_semantic::*, project_semantic_diff::*};
use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine};
use kumi_common::{abort::Signal, js::json as js_json, time::now_ms};
use rand::RngCore;
use std::path::PathBuf;
pub(super) struct SemanticExport {
    artifact: Value,
    at: f64,
    epoch: i64,
}
fn project<T>(value: Result<T, project_files::ProjectError>) -> Result<T, LiveError> {
    value.map_err(|e| LiveError::error(e.0))
}
fn outcome(id: &Value, result: Result<Value, LiveError>, remediation: &str) -> Value {
    match result {
        Ok(value) => value,
        Err(error) => adapter_tool_error(id, &error, remediation),
    }
}
fn optional_limit(params: &Value) -> bool {
    params.get("limit").is_none_or(|v| is_integer_in_range(v, 1., 200.))
}
fn optional_cursor(params: &Value) -> bool {
    params.get("cursor").is_none_or(|v| is_non_empty_string(v, 4096))
}
fn page_options(params: &Value) -> SemanticPageOptions {
    SemanticPageOptions { limit: params["limit"].as_f64(), cursor: params["cursor"].as_str().map(str::to_owned) }
}
fn file_path(snapshot: &LiveSnapshot) -> Result<Option<String>, LiveError> {
    let set = snapshot.set.as_ref().ok_or_else(|| LiveError::type_error("Cannot read properties of undefined (reading 'filePath')"))?;
    Ok(serde_json::to_value(set).unwrap()["filePath"].as_str().filter(|s| !s.is_empty()).map(str::to_owned))
}
fn profile(value: Option<&Value>, fallback: &str) -> Result<String, LiveError> {
    match value {
        None => Ok(fallback.into()),
        Some(Value::String(s)) if ["strict", "collaboration", "local"].contains(&s.as_str()) => Ok(s.clone()),
        _ => Err(LiveError::error("profile must be strict, collaboration, or local")),
    }
}
fn file_authority(path: &Value, root: &Value) -> Result<(PathBuf, PathBuf), LiveError> {
    if !is_non_empty_string(path, 4096) || !is_non_empty_string(root, 1024) {
        return Err(LiveError::error("path and allowedRoot are required"));
    }
    let path = path.as_str().unwrap();
    let root = root.as_str().unwrap();
    let bytes = path.as_bytes();
    if !path.starts_with('/') && !(bytes.len() >= 2 && bytes[0].is_ascii_alphabetic() && bytes[1] == b':') {
        return Err(LiveError::error("path must be an absolute path"));
    }
    fn realpath(value: &str) -> Result<PathBuf, LiveError> {
        let error_path = crate::command::resolve(value)?;
        let path = std::fs::canonicalize(value).map_err(|e| crate::delivery::io_error(&e, "lstat", &[&error_path]))?;
        let text = path.to_string_lossy();
        Ok(PathBuf::from(
            text.strip_prefix(r"\\?\UNC\")
                .map(|s| format!(r"\\{s}"))
                .or_else(|| text.strip_prefix(r"\\?\").map(str::to_owned))
                .unwrap_or_else(|| text.into_owned()),
        ))
    }
    let root = realpath(root)?;
    let path = realpath(path)?;
    if !path.starts_with(&root) {
        return Err(LiveError::error("path escapes the allowed root"));
    }
    if !path.extension().is_some_and(|s| s.to_string_lossy().eq_ignore_ascii_case("als")) {
        return Err(LiveError::error("path must be an .als file"));
    }
    let stats = std::fs::metadata(&path).map_err(|e| crate::delivery::io_error(&e, "stat", &[&path]))?;
    if !stats.is_file() {
        return Err(LiveError::error("path is not a regular file"));
    }
    Ok((path, root))
}
impl McpHost {
    pub(super) fn has_semantic_export(&self, id: &str) -> bool {
        self.semantic_exports.borrow().iter().any(|row| row.artifact["artifact"]["id"] == id)
    }
    pub async fn dispatch_project_tool(&self, call: &ToolCall, signal: Option<&Signal>) -> Option<Result<Value, LiveError>> {
        if !call.asynchronous {
            return None;
        }
        let id = &call.id;
        let params = call.arguments.as_ref().unwrap_or(&Value::Null);
        Some(Ok(match call.name.as_str() {
            "live_project_snapshot_export" => self.live_project_snapshot_export_async(id, params).await,
            "live_project_snapshot_diff" => self.live_project_snapshot_diff(id, params),
            "als_read" => self.als_read(id, params),
            "als_lint" => self.als_lint(id, params),
            "als_diff" => self.als_diff(id, params),
            "live_project_info" => self.live_project_info_async(id, params).await,
            "live_project_backup_preview" => self.live_project_backup_preview_async(id, params).await,
            "live_project_backup_apply" => self.live_project_backup_apply_async(id, params, signal).await,
            _ => return None,
        }))
    }
    pub async fn live_project_snapshot_export_async(&self, id: &Value, params: &Value) -> Value {
        if !has_only(params, &["profile", "limit", "cursor"])
            || params.get("profile").is_some_and(|v| !v.as_str().is_some_and(|s| ["strict", "collaboration", "local"].contains(&s)))
            || !optional_limit(params)
            || !optional_cursor(params)
        {
            return error(id, -32602, "profile, limit, and cursor are invalid", None);
        }
        outcome(
            id,
            async {
                let status = self.require_connected(Some("session.read"))?;
                if !status.has_operation("snapshot") {
                    return Err(LiveError::error("snapshot operation is unavailable"));
                }
                let wanted = params["profile"].as_str().unwrap_or("collaboration");
                if let Some(cursor) = params["cursor"].as_str() {
                    let artifact_id = semantic_cursor_artifact_id(cursor).unwrap_or_default();
                    let kept = self
                        .semantic_exports
                        .borrow()
                        .iter()
                        .find(|k| {
                            k.artifact["artifact"]["id"] == artifact_id
                                && Some(k.epoch) == status.epoch
                                && k.artifact["policy"]["profile"] == wanted
                                && now_ms() as f64 - k.at < 300_000.
                        })
                        .map(|k| k.artifact.clone());
                    if let Some(artifact) = kept {
                        return Ok(success_text(id, &project(page_semantic_project_snapshot(&artifact, &page_options(params)))?));
                    }
                }
                let snapshot = self.views.whole_set(None, None).await?;
                let path = file_path(&snapshot)?;
                let mut live = json!({"protocol":status.protocol,"adapter":status.adapter,"provenance":status.provenance});
                if let Some(hash) = status.registry_hash {
                    live["registryHash"] = json!(hash);
                }
                let artifact = project(create_semantic_project_snapshot(
                    &serde_json::to_value(snapshot).unwrap(),
                    &CreateSemanticProjectOptions {
                        profile: Some(wanted.into()),
                        exporter_version: self.server_version().into(),
                        live,
                        project_path: path,
                        source_evidence: None,
                        max_records: None,
                        source_kind: None,
                        extra_unavailable: vec![],
                    },
                ))?;
                {
                    let mut exports = self.semantic_exports.borrow_mut();
                    let kept = SemanticExport { artifact: artifact.clone(), at: now_ms() as f64, epoch: status.epoch.unwrap() };
                    if let Some(existing) = exports.iter_mut().find(|k| k.artifact["artifact"]["id"] == artifact["artifact"]["id"]) {
                        *existing = kept;
                    } else {
                        exports.push_back(kept);
                    }
                    while exports.len() > 2 {
                        exports.pop_front();
                    }
                }
                Ok(success_text(id, &project(page_semantic_project_snapshot(&artifact, &page_options(params)))?))
            }
            .await,
            "Semantic snapshot export is read-only; retry only after a fresh readable snapshot or restart paging from the first page.",
        )
    }
    pub fn live_project_snapshot_diff(&self, id: &Value, params: &Value) -> Value {
        let bounded = |key| params[key].as_array().is_some_and(|a| !a.is_empty() && a.len() <= SEMANTIC_PROJECT_MAX_PAGES);
        if !has_only(params, &["beforePages", "afterPages", "limit", "cursor"])
            || !bounded("beforePages")
            || !bounded("afterPages")
            || !optional_limit(params)
            || !optional_cursor(params)
        {
            return error(id, -32602, "complete bounded beforePages and afterPages plus optional limit/cursor are required", None);
        }
        outcome(
            id,
            (|| {
                if js_json::byte_length(&json!({"beforePages":params["beforePages"],"afterPages":params["afterPages"]}))
                    > SEMANTIC_PROJECT_MAX_DIFF_INPUT_BYTES
                {
                    return Err(LiveError::error("combined semantic snapshot bundles exceed the bounded diff input size"));
                }
                let before = project(assemble_semantic_project_pages(params["beforePages"].as_array().unwrap()))?;
                let after = project(assemble_semantic_project_pages(params["afterPages"].as_array().unwrap()))?;
                let diff = project(diff_semantic_project_snapshots(&before, &after))?;
                Ok(success_text(id, &project(page_semantic_project_diff(&diff, &page_options(params)))?))
            })(),
            "Semantic diff requires complete untampered page bundles with the same schema and privacy profile; no merge was attempted.",
        )
    }
    pub fn als_read(&self, id: &Value, params: &Value) -> Value {
        if !has_only(params, &["path", "allowedRoot", "profile", "limit", "cursor", "includeNotes", "maxRecords"])
            || !optional_limit(params)
            || !optional_cursor(params)
            || params.get("maxRecords").is_some_and(|v| !is_integer_in_range(v, 1., SEMANTIC_PROJECT_MAX_RECORDS as f64))
            || params.get("includeNotes").is_some_and(|v| !v.is_boolean())
        {
            return error(id, -32602, "path, allowedRoot, and optional profile/limit/cursor/includeNotes/maxRecords are required", None);
        }
        outcome(
            id,
            (|| {
                let (path, _) = file_authority(&params["path"], &params["allowedRoot"])?;
let (source, model) = project(read_als_model(&path.to_string_lossy()))?;
let artifact = project(create_offline_als_artifact(
                    &source,
                    &model,
                    &OfflineAlsArtifactOptions {
                        profile: Some(profile(params.get("profile"), "collaboration")?),
                        exporter_version: self.server_version().into(),
                        max_records: params["maxRecords"].as_f64(),
                    },
                ))?;
let page = project(page_semantic_project_snapshot(&artifact, &page_options(params)))?;
let mut result = json!({"page":page,"provenance":"offline-file"});
if params["includeNotes"] == true {
                    let rows = extract_als_midi(&model, artifact["policy"]["profile"].as_str());
let rows = rows.as_array().unwrap();
let mut budget = 4096;
let mut truncated = rows.len() > 256;
let mut bounded = vec![];
for row in rows.iter().take(256) {
                        let mut row = row.clone();
let notes = row["notes"].as_array().unwrap();
if budget == 0 {
                            truncated = true;
row["notes"] = json!([]);
} else {
                            let kept = notes.len().min(budget);
budget -= kept;
if kept < notes.len() {
                                truncated = true;
}
                            row["notes"] = json!(&notes[..kept]);
}
                        bounded.push(row);
}
                    let clips: Vec<_> = bounded
                        .into_iter()
                        .enumerate()
                        .filter(|(i, row)| *i == 0 || !row["notes"].as_array().unwrap().is_empty() || !truncated)
                        .map(|(_, r)| r)
                        .collect();
result["midi"] = json!({"clips":clips,"truncated":truncated,"noteBudget":4096,"clipBudget":256});
}
                Ok(success_text(id, &result))
            })(),
            "Offline .als reading requires one owner-authorized regular file under the allowed root; sections that cannot be reconstructed offline are marked unavailable.",
        )
    }
    pub fn als_lint(&self, id: &Value, params: &Value) -> Value {
        if !has_only(params, &["path", "allowedRoot"]) {
            return error(id, -32602, "path and allowedRoot are required", None);
        }
        outcome(
            id,
            (|| {
                let (path, root) = file_authority(&params["path"], &params["allowedRoot"])?;
                let (_, model) = project(read_als_model(&path.to_string_lossy()))?;
                let lint = project(lint_als_model(
                    &model,
                    &AlsLintOptions {
                        allowed_root: Some(root.to_string_lossy().into_owned()),
                        set_directory: path.parent().map(|p| p.to_string_lossy().into_owned()),
                        max_findings: None,
                    },
                ))?;
                let mut severity = json!({});
                let mut check = json!({});
                let findings = lint["findings"].as_array().unwrap();
                for finding in findings {
                    let a = finding["severity"].as_str().unwrap();
                    let b = finding["check"].as_str().unwrap();
                    severity[a] = json!(severity[a].as_u64().unwrap_or(0) + 1);
                    check[b] = json!(check[b].as_u64().unwrap_or(0) + 1);
                }
                Ok(success_text(
                    id,
                    &json!({"findings":findings,"truncated":lint["truncated"],"summary":{"total":findings.len(),"bySeverity":severity,"byCheck":check},"parseNotes":model.parse_notes}),
                ))
            })(),
            "Offline .als lint requires one owner-authorized regular file under the allowed root; findings are never fixes.",
        )
    }
    pub fn als_diff(&self, id: &Value, params: &Value) -> Value {
        if !has_only(params, &["before", "after", "limit", "cursor"])
            || !params["before"].is_object()
            || !params["after"].is_object()
            || !optional_limit(params)
            || !optional_cursor(params)
        {
            return error(id, -32602, "before and after sides plus optional limit/cursor are required", None);
        }
        let side = |value: &Value, fallback: &str| -> Result<Value, LiveError> {
            if !has_only(value, &["als", "pages"]) {
                return Err(LiveError::error("each diff side must be an object with exactly one of als or pages"));
            }
            if let Some(args) = value.get("als") {
                if !has_only(args, &["path", "allowedRoot", "profile"]) {
                    return Err(LiveError::error("an als side requires path and allowedRoot with optional profile"));
                }
                let (path, _) = file_authority(&args["path"], &args["allowedRoot"])?;
                let (source, model) = project(read_als_model(&path.to_string_lossy()))?;
                return project(create_offline_als_artifact(
                    &source,
                    &model,
                    &OfflineAlsArtifactOptions {
                        profile: Some(profile(args.get("profile"), fallback)?),
                        exporter_version: self.server_version().into(),
                        max_records: None,
                    },
                ));
            }
            let pages = value["pages"]
                .as_array()
                .filter(|p| !p.is_empty() && p.len() <= SEMANTIC_PROJECT_MAX_PAGES)
                .ok_or_else(|| LiveError::error(format!("a pages side requires 1-{SEMANTIC_PROJECT_MAX_PAGES} complete pages")))?;
            project(assemble_semantic_project_pages(pages))
        };
        outcome(
            id,
            (|| {
                let before = side(&params["before"], "collaboration")?;
let after = side(&params["after"], before["policy"]["profile"].as_str().unwrap())?;
if before["policy"]["profile"] != after["policy"]["profile"] {
                    return Err(LiveError::error("semantic diff sides must share one privacy profile"));
}
                let diff = project(diff_semantic_project_snapshots(&before, &after))?;
Ok(success_text(id, &project(page_semantic_project_diff(&diff, &page_options(params)))?))
            })(),
            "Offline .als diff requires owner-authorized files or complete untampered page bundles with one shared privacy profile; no merge was attempted.",
        )
    }
    pub async fn live_project_info_async(&self, id: &Value, params: &Value) -> Value {
        if !has_only(params, &[]) {
            return error(id, -32602, "no arguments accepted", None);
        }
        outcome(
            id,
            async {
                self.require_connected(Some("session.read"))?;
                let snapshot = self.views.view(None, LiveViewScope::Indices(vec![]), Some(&[LiveSnapshotPart::Set])).await?;
                let Some(path) = file_path(&snapshot)? else {
                    return Ok(success_text(id, &json!({"exists":false,"note":"the current set has never been saved to disk"})));
                };
                Ok(success_text(id, &serde_json::to_value(project(project_files::project_info(&path))?).unwrap()))
            }
            .await,
            "Project info requires the current set path and host file access.",
        )
    }
    pub async fn live_project_backup_preview_async(&self, id: &Value, params: &Value) -> Value {
        if !has_only(params, &["confirmation", "allowedRoot"])
            || params["confirmation"] != "backup"
            || !is_non_empty_string(&params["allowedRoot"], 4096)
        {
            return error(id, -32602, "confirmation=backup and an explicit absolute allowedRoot are required", None);
        }
        let result = async {
            self.require_connected(Some("session.read"))?;
            let snapshot = self.views.view(None, LiveViewScope::Indices(vec![]), Some(&[LiveSnapshotPart::Set])).await?;
            let Some(path) = file_path(&snapshot)? else {
                return Ok(transaction_error(id, "the current set has never been saved to disk; save it through Live's UI first (save is a negotiated API limitation)"));
            };
            let manifest = project(project_files::project_info(&path))?;
            let fence = js_json::stringify(&json!({"path":path,"size":manifest.size,"mtimeMs":manifest.mtime_ms,"sha256":manifest.sha256}));
            let mut random = [0u8; 18];
            rand::rng().fill_bytes(&mut random);
            let transaction_id = format!("backup_{}", URL_SAFE_NO_PAD.encode(random));
            let expires = now_ms() as f64 + TRANSACTION_TTL_MS;
            let transaction = json!({"id":transaction_id,"epoch":self.safe_adapter_status().epoch.unwrap_or(0),"kind":"backup","fence":fence,"payload":{"path":path,"allowedRoot":params["allowedRoot"],"size":manifest.size,"mtimeMs":manifest.mtime_ms,"sha256":manifest.sha256},"expiresAt":expires,"state":"previewed"});
            self.clip_lifecycle_transactions.insert(&transaction_id, transaction).map_err(|e| {
                if e.message().contains("capacity is exhausted") { LiveError::error("project backup transaction capacity is exhausted by in-flight work") } else { e }
            })?;
            Ok(success_text(id, &json!({"transactionId":transaction_id,"path":path,"manifest":{"size":manifest.size,"mtimeMs":manifest.mtime_ms,"sha256":manifest.sha256},"allowedRoot":params["allowedRoot"],"impact":"creates-verified-backup","confirmation":"apply","expiresAt":expires})))
        }.await;
        outcome(id, result, "Project backup preview requires the current set path.")
    }
    pub async fn live_project_backup_apply_async(&self, id: &Value, params: &Value, signal: Option<&Signal>) -> Value {
        if !valid_transaction_params(params, "apply") {
            return error(id, -32602, "transactionId, confirmation=apply, and idempotencyKey are required", None);
        }
        let transaction = self.clip_lifecycle_transactions.get(params["transactionId"].as_str().unwrap());
        let Some(transaction) = transaction.filter(|t| {
            let t = t.borrow();
            t["kind"] == "backup" && !(t["state"] == "previewed" && t["expiresAt"].as_f64().is_some_and(|t| t <= now_ms() as f64))
        }) else {
            return transaction_error(id, "Unknown or expired backup transaction");
        };
        {
            let t = transaction.borrow();
            if t["state"] == "applied" && t["applyKey"] == params["idempotencyKey"] {
                return success_text(id, &json!({"transactionId":t["id"],"state":"applied","backup":t["created"],"idempotent":true}));
            }
            if t["state"] != "previewed" {
                return transaction_error(id, "Transaction is no longer applicable");
            }
        }
        if signal.is_some_and(Signal::is_cancelled) {
            return Value::Null;
        }
        let result: Result<Value, LiveError> = async {
            let snapshot = self.views.view(None, LiveViewScope::Indices(vec![]), Some(&[LiveSnapshotPart::Set])).await?;
            let t = transaction.borrow().clone();
            if file_path(&snapshot)?.as_deref() != t["payload"]["path"].as_str() { return Ok(transaction_error(id, "the current set path changed since preview; preview again")); }
            let path = t["payload"]["path"].as_str().unwrap();
            let current = project(project_files::project_info(path))?;
            let fence = js_json::stringify(&json!({"path":current.path,"size":current.size,"mtimeMs":current.mtime_ms,"sha256":current.sha256}));
            if t["fence"] != fence { return Ok(transaction_error(id, "the current Set content changed since preview; preview again")); }
            let result = project(project_files::project_backup(path, &project_files::ProjectBackupOptions {
                allowed_root: t["payload"]["allowedRoot"].as_str().map(str::to_owned),
                expected_sha256: t["payload"]["sha256"].as_str().map(str::to_owned),
                expected_size: t["payload"]["size"].as_u64(),
                expected_mtime_ms: t["payload"]["mtimeMs"].as_f64(),
            }))?;
            if !result.verified { return Err(LiveError::error("backup verification failed")); }
            {
                let mut t = transaction.borrow_mut();
                t["created"] = serde_json::to_value(&result).unwrap();
                t["applyKey"] = params["idempotencyKey"].clone();
                t["state"] = json!("applied");
            }
            Ok(success_text(id, &json!({"transactionId":t["id"],"state":"applied","backup":result.backup,"manifest":result.manifest,"verified":result.verified,"idempotent":false})))
        }.await;
        match result {
            Ok(value) => value,
            Err(cause) => {
                transaction.borrow_mut()["state"] = json!("uncertain");
                adapter_tool_error(id, &cause, "Project backup is uncertain; verify the backup file before relying on it.")
            }
        }
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[tokio::test]
    async fn expired_and_wrong_kind_backup_transactions_match_source_traces() {
        let fixture: Value =
            serde_json::from_slice(&std::fs::read(concat!(env!("CARGO_MANIFEST_DIR"), "/tests/support/host_project_oracle.json")).unwrap())
                .unwrap();
        for mode in ["expired", "wrong-kind"] {
            let host = McpHost::default();
            host.clip_lifecycle_transactions.insert("backup_ID",json!({"id":"backup_ID","kind":if mode=="expired"{"backup"}else{"other"},"state":"previewed","expiresAt":if mode=="expired"{0.}else{now_ms() as f64+10000.}})).unwrap();
            let source = fixture["backupTraces"].as_array().unwrap().iter().find(|row| row["mode"] == mode).unwrap();
            for (index, key) in ["backup-key", "backup-key", "other-key"].into_iter().enumerate() {
                let mut result = host
                    .live_project_backup_apply_async(
                        &json!(1),
                        &json!({"transactionId":"backup_ID","confirmation":"apply","idempotencyKey":key}),
                        None,
                    )
                    .await;
                let body: Value = serde_json::from_str(result["result"]["content"][0]["text"].as_str().unwrap()).unwrap();
                result["result"]["content"][0]["text"] = body;
                assert_eq!(result, source["results"][index + 1]);
            }
        }
    }
}
