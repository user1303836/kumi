//! The runtime's HISTORY (C2, C3's `history.json`): one row for each change Kumi made through it, newest first, at most
//! [`KEPT`]. A row's id is opaque and bound to this runtime (`h-<generation>-<change>`), so an old card never reaches
//! another runtime's change that happens to share Kumi's own id (`c14`). Rows from an earlier runtime are kept for the
//! record only: their undo is gone with that runtime (`undo_available: false`).

use super::say;
use super::snapshots::Snapshots;
use kumi_types::contracts::{ChangeRecord, ChangeState};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::path::Path;

/// The most rows kept.
pub const KEPT: usize = 200;

/// [`Row::reason`] of a change only the DAW's own Undo takes back (CC-20).
pub const DAW_UNDO: &str = "daw_undo";

/// One HISTORY row, as `history` and `history.json` give it.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Row {
    /// The public id `undo` takes: this runtime's, never another's.
    pub id: String,
    /// Kumi's own id for the change in its runtime ("c14"): a label, never a key.
    pub label: String,
    pub project_key: String,
    pub runtime_generation: String,
    pub title: String,
    pub family: String,
    pub at: i64,
    /// `applied`, `undone`, `kept`, `unsure`, `expired`; a script's (CC-19) also `failed`.
    pub state: String,
    pub undo_available: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub track: Option<String>,
    /// Why an undo didn't happen, or what's unsure, in plain words.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
    /// Why it can't be undone from here when that's what it is: [`DAW_UNDO`], a change only the DAW's own Undo takes
    /// back, which it stays through any state since.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
    /// The host call that made it (CC-5: its profile, session and id) and what it asked, digested: a retry after a
    /// restart is known by them.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub profile_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub session_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tool_call_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub digest: Option<String>,
    /// What the call that made it came to (`applied`, `partial`, `unsure`), as it answered.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub call_outcome: Option<String>,
    /// The `change` call that recorded it (CC-3 A2), the same on every row that one call recorded, its audit row
    /// included; opaque. None for a row anything else recorded, which is a change of its own.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub change_id: Option<String>,
    /// The files the call placed (CC-11): each source as it named the file, the file's name and the identity of the
    /// file the DAW got.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sources: Option<Vec<Value>>,
    /// A script's row (CC-19): which code ran, by its SHA-256 (the runtime keeps the code beside HISTORY), and its
    /// language. Never Kumi's to undo.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub script: Option<ScriptRun>,
}

/// A script's family (CC-19).
pub const SCRIPT_FAMILY: &str = "script";

/// The code a script's row ran (CC-19).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ScriptRun {
    /// The SHA-256 of the code's UTF-8 bytes, in lowercase hex.
    pub sha256: String,
    /// `python` or `lua`.
    pub language: String,
}

pub struct History {
    generation: String,
    rows: RefCell<Vec<Row>>,
    /// The change ids a running `change` call has made so far (its HISTORY rows), while one runs.
    collecting: RefCell<Option<Vec<String>>>,
    /// What the running `change` call's rows say when it holds a step only the DAW's own Undo takes back.
    collecting_daw_undo: RefCell<Option<String>>,
    /// While a `script` call runs: its row's title and code, which the row the DAW's executor makes for it takes.
    scripting: RefCell<Option<(String, ScriptRun)>>,
    /// Audit rows made so far: each one's id is this runtime's generation and its number.
    audits: Cell<u64>,
    /// `change` calls that recorded rows so far: each one's [`Row::change_id`] is this runtime's generation and its
    /// number.
    changes: Cell<u64>,
    /// Kumi's writes so far (a change that recorded rows, an undo), numbered, and each project's last: a change only the
    /// DAW's own Undo takes back is buried in that undo once Kumi writes after it in its project.
    writes: Cell<u64>,
    last_write: RefCell<HashMap<String, u64>>,
    /// This runtime's rows of changes only the DAW's own Undo takes back, each with the write that made it.
    daw_undo_writes: RefCell<HashMap<String, u64>>,
    /// Script rows made by the runtime itself (no executor row for them) so far: numbered as audit rows are.
    scripts: Cell<u64>,
}

/// The host call a row came from (CC-5): its profile, session and `tool_call_id` (a host's call ids are only unique
/// within a session: `call_0` comes again in another chat), and the digest of what it asked (the operation's name and
/// its arguments, `_host` left out), so a retry after a restart is known by them.
#[derive(Debug, Clone, Copy)]
pub struct CallIdentity<'a> {
    pub profile_id: Option<&'a str>,
    pub session_id: Option<&'a str>,
    pub tool_call_id: Option<&'a str>,
    pub digest: &'a str,
}

impl<'a> CallIdentity<'a> {
    /// The call `host` names, asking what `digest` digests.
    pub fn of(host: &'a super::ctx::Host, digest: &'a str) -> CallIdentity<'a> {
        CallIdentity {
            profile_id: host.profile_id.as_deref(),
            session_id: host.session_id.as_deref(),
            tool_call_id: host.tool_call_id.as_deref(),
            digest,
        }
    }
}

impl History {
    /// This runtime's HISTORY (`generation`: its own, unique among runtimes), with an earlier runtime's rows from
    /// `history.json` kept for the record.
    pub fn new(state_dir: &Path, generation: String) -> History {
        let earlier: Vec<Row> = std::fs::read(state_dir.join("history.json"))
            .ok()
            .and_then(|bytes| serde_json::from_slice::<Value>(&bytes).ok())
            .and_then(|kept| serde_json::from_value(kept["items"].clone()).ok())
            .unwrap_or_default();
        let rows = earlier.into_iter().map(|row| Row { undo_available: false, ..row }).take(KEPT).collect();
        History {
            generation,
            rows: RefCell::new(rows),
            collecting: RefCell::new(None),
            collecting_daw_undo: RefCell::new(None),
            audits: Cell::new(0),
            changes: Cell::new(0),
            writes: Cell::new(0),
            last_write: RefCell::new(HashMap::new()),
            daw_undo_writes: RefCell::new(HashMap::new()),
            scripting: RefCell::new(None),
            scripts: Cell::new(0),
        }
    }

    pub fn generation(&self) -> &str {
        &self.generation
    }

    /// The newest `limit` rows.
    pub fn rows(&self, limit: usize) -> Vec<Row> {
        self.rows.borrow().iter().take(limit).cloned().collect()
    }

    /// The row `id` names, when it's one of these.
    pub fn row(&self, id: &str) -> Option<Row> {
        self.rows.borrow().iter().find(|row| row.id == id).cloned()
    }

    /// Kumi's record of a change (new, or its state since), on `project_key`: the row it is, written to `history.json`.
    pub fn record(&self, record: &ChangeRecord, project_key: &str, snapshots: &Snapshots) -> Row {
        let id = format!("h-{}-{}", self.generation, record.id);
        let state = match record.state {
            ChangeState::Applied => "applied",
            ChangeState::Undone => "undone",
            ChangeState::Kept => "kept",
            ChangeState::Unsure => "unsure",
            ChangeState::Expired => "expired",
            ChangeState::Heard => "heard",
        };
        let family = serde_json::to_value(record.family).ok().and_then(|family| family.as_str().map(str::to_string)).unwrap_or_default();
        let fresh = Row {
            id: id.clone(),
            label: record.id.clone(),
            project_key: project_key.to_string(),
            runtime_generation: self.generation.clone(),
            title: record.title.clone(),
            family,
            at: record.at,
            state: state.into(),
            // An unsure change can't be undone from here (CC-2's W1-b): only what Kumi saw applied can.
            undo_available: record.state == ChangeState::Applied,
            track: record.track.as_ref().map(|track| track.name.clone()),
            note: record.note.clone(),
            reason: None,
            profile_id: None,
            session_id: None,
            tool_call_id: None,
            digest: None,
            call_outcome: None,
            change_id: None,
            sources: None,
            script: None,
        };
        let row = {
            let mut rows = self.rows.borrow_mut();
            match rows.iter_mut().find(|row| row.id == id) {
                Some(row) => {
                    // Its state since: the project, the call, what the call came to, the change it's part of and when it
                    // was made stay as first recorded.
                    let (project_key, profile_id, session_id, tool_call_id, digest, call_outcome, change_id, sources, at) = (
                        row.project_key.clone(),
                        row.profile_id.clone(),
                        row.session_id.clone(),
                        row.tool_call_id.clone(),
                        row.digest.clone(),
                        row.call_outcome.clone(),
                        row.change_id.clone(),
                        row.sources.clone(),
                        row.at,
                    );
                    let daw_undo = (row.reason.as_deref() == Some(DAW_UNDO)).then(|| (row.reason.clone(), row.note.clone()));
                    // A script's row stays the script's (CC-19): its title, family and code, never Kumi's to undo, and
                    // its note when the executor's word on it brings none.
                    let script = row.script.clone().map(|script| (row.title.clone(), row.family.clone(), row.note.clone(), script));
                    *row = Row { project_key, profile_id, session_id, tool_call_id, digest, call_outcome, change_id, sources, at, ..fresh };
                    if let Some((title, family, note, script)) = script {
                        (row.title, row.family, row.script, row.undo_available) = (title, family, Some(script), false);
                        row.note = row.note.take().or(note);
                    }
                    // A change only the DAW's own Undo takes back stays one, with its words, whatever its state since.
                    if let Some((reason, note)) = daw_undo {
                        (row.reason, row.note, row.undo_available) = (reason, note, false);
                    }
                    row.clone()
                }
                None => {
                    let mut fresh = fresh;
                    // The row the DAW's executor makes for a running script (REAPER's and Renoise's run_lua) is the
                    // script's own: Kumi can't undo it, so it's never offered.
                    if let Some((title, script)) = self.scripting.borrow().as_ref() {
                        fresh.title = title.clone();
                        fresh.family = SCRIPT_FAMILY.into();
                        fresh.undo_available = false;
                        fresh.script = Some(script.clone());
                    }
                    rows.insert(0, fresh.clone());
                    rows.truncate(KEPT);
                    // A row first made while a `change` call runs is that call's; a row only updated isn't.
                    if let Some(collecting) = self.collecting.borrow_mut().as_mut() {
                        collecting.push(fresh.id.clone());
                    }
                    fresh
                }
            }
        };
        self.publish(snapshots);
        row
    }

    /// A `change` call starts: the rows made from here are its own, until [`History::end_call`].
    pub fn begin_call(&self) {
        *self.collecting.borrow_mut() = Some(Vec::new());
        *self.collecting_daw_undo.borrow_mut() = None;
    }

    /// The running `change` call holds a step only the DAW's own Undo takes back (CC-20): every row it makes is one Kumi
    /// doesn't undo and says `note`, however the call ends.
    pub fn daw_undo_call(&self, note: String) {
        *self.collecting_daw_undo.borrow_mut() = Some(note);
    }

    /// The `change` call ends: the rows it made (oldest first), marked with its host call (CC-5's C5-3), and as ones
    /// only the DAW's own Undo takes back when the call held such a step. A row that already carries a call's identity
    /// keeps it. A script call that ended this way (it broke part way) is over too.
    pub fn end_call(&self, identity: CallIdentity, snapshots: &Snapshots) -> Vec<Row> {
        *self.scripting.borrow_mut() = None;
        let made = self.collecting.borrow_mut().take().unwrap_or_default();
        let daw_undo = self.collecting_daw_undo.borrow_mut().take();
        let write = made.first().and_then(|id| self.row(id)).map(|row| self.wrote(&row.project_key));
        let rows: Vec<Row> = {
            let mut rows = self.rows.borrow_mut();
            rows.iter_mut()
                .filter(|row| made.contains(&row.id))
                .map(|row| {
                    if row.digest.is_none() {
                        row.profile_id = identity.profile_id.map(str::to_string);
                        row.session_id = identity.session_id.map(str::to_string);
                        row.tool_call_id = identity.tool_call_id.map(str::to_string);
                        row.digest = Some(identity.digest.to_string());
                    }
                    if let Some(note) = &daw_undo {
                        (row.reason, row.note, row.undo_available) = (Some(DAW_UNDO.into()), Some(note.clone()), false);
                        self.daw_undo_writes.borrow_mut().extend(write.map(|write| (row.id.clone(), write)));
                    }
                    row.clone()
                })
                .collect()
        };
        if !rows.is_empty() {
            self.publish(snapshots);
        }
        made.iter().filter_map(|id| rows.iter().find(|row| &row.id == id).cloned()).collect()
    }

    /// Kumi wrote in the project `project_key` (a change that recorded rows, or an undo): a change only the DAW's own
    /// Undo takes back made there before it is no longer the DAW's latest step. The write's number.
    pub fn wrote(&self, project_key: &str) -> u64 {
        let n = self.writes.get() + 1;
        self.writes.set(n);
        self.last_write.borrow_mut().insert(project_key.to_string(), n);
        n
    }

    /// Where a change only the DAW's own Undo takes back stands in that undo, as far as this runtime knows (CC-20's
    /// 20f): `next` while Kumi hasn't written in its project since, `buried` once it has, since that write is the DAW's
    /// latest step then. None for any other row, and for one an earlier runtime made: what happened since is unknown.
    pub fn daw_undo_place(&self, row: &Row) -> Option<&'static str> {
        let made = *self.daw_undo_writes.borrow().get(&row.id)?;
        let last = *self.last_write.borrow().get(&row.project_key)?;
        Some(if last > made { "buried" } else { "next" })
    }

    /// A row as `history`, `undo` and `history.json` give it, with where it stands in the DAW's undo (`daw_undo`) when
    /// it's a change only that undo takes back.
    pub fn row_json(&self, row: &Row) -> Option<Value> {
        let mut value = serde_json::to_value(row).ok()?;
        if let Some(place) = self.daw_undo_place(row) {
            value["daw_undo"] = json!(place);
        }
        Some(value)
    }

    /// A `script` call starts (CC-19): the row the DAW's executor makes from here is its own, titled `title`, until
    /// [`History::end_script`].
    pub fn begin_script(&self, title: &str, script: &ScriptRun) {
        *self.collecting.borrow_mut() = Some(Vec::new());
        *self.scripting.borrow_mut() = Some((title.to_string(), script.clone()));
    }

    /// The `script` call ends: its one row, in `state` (`applied`, `failed` or `unsure`) with `note` (every script row
    /// has one: why Kumi can't undo it), marked with its host call. That's the row the DAW's executor made for it
    /// (REAPER's and Renoise's run_lua), or, when it made none (Live's run_python never does, and nothing may have run),
    /// a row of the runtime's own. A run is Kumi's write in its project: a change only the DAW's own Undo takes back made
    /// there before it is no longer the DAW's latest step (CC-20).
    pub fn end_script(&self, identity: CallIdentity, project_key: &str, state: &str, note: String, snapshots: &Snapshots) -> Row {
        let made = self.collecting.borrow_mut().take().unwrap_or_default();
        let (title, script) = self.scripting.borrow_mut().take().unwrap_or_default();
        self.wrote(project_key);
        let mark = |row: &mut Row| {
            row.state = state.to_string();
            row.undo_available = false;
            row.note = Some(note.clone());
            row.profile_id = identity.profile_id.map(str::to_string);
            row.session_id = identity.session_id.map(str::to_string);
            row.tool_call_id = identity.tool_call_id.map(str::to_string);
            row.digest = Some(identity.digest.to_string());
        };
        let row = {
            let mut rows = self.rows.borrow_mut();
            match rows.iter_mut().find(|row| made.first() == Some(&row.id)) {
                Some(row) => {
                    mark(row);
                    row.clone()
                }
                None => {
                    let n = self.scripts.get() + 1;
                    self.scripts.set(n);
                    let mut row = Row {
                        id: format!("h-{}-s{n}", self.generation),
                        label: String::new(),
                        project_key: project_key.to_string(),
                        runtime_generation: self.generation.clone(),
                        title,
                        family: SCRIPT_FAMILY.into(),
                        at: kumi_common::time::now_ms(),
                        state: String::new(),
                        undo_available: false,
                        track: None,
                        note: None,
                        reason: None,
                        profile_id: None,
                        session_id: None,
                        tool_call_id: None,
                        digest: None,
                        call_outcome: None,
                        change_id: None,
                        sources: None,
                        script: Some(script),
                    };
                    mark(&mut row);
                    rows.insert(0, row.clone());
                    rows.truncate(KEPT);
                    row
                }
            }
        };
        self.publish(snapshots);
        row
    }

    /// The files the call whose rows are `ids` placed (CC-11), kept with them.
    pub fn placed(&self, ids: &[String], sources: &[Value], snapshots: &Snapshots) {
        if sources.is_empty() {
            return;
        }
        let mut marked = false;
        for row in self.rows.borrow_mut().iter_mut().filter(|row| ids.contains(&row.id)) {
            row.sources = Some(sources.to_vec());
            marked = true;
        }
        if marked {
            self.publish(snapshots);
        }
    }

    /// What the call whose rows are `ids` came to: kept with them, for a retry answered from HISTORY.
    pub fn call_came_to(&self, ids: &[String], outcome: &str, snapshots: &Snapshots) {
        let mut marked = false;
        for row in self.rows.borrow_mut().iter_mut().filter(|row| ids.contains(&row.id)) {
            row.call_outcome = Some(outcome.to_string());
            marked = true;
        }
        if marked {
            self.publish(snapshots);
        }
    }

    /// The rows `ids` are one change (what one `change` call recorded, its audit row included): each keeps the same new
    /// [`Row::change_id`], so an undo can take the whole change back and a list can show it as one.
    pub fn one_change(&self, ids: &[String], snapshots: &Snapshots) {
        if ids.is_empty() {
            return;
        }
        let n = self.changes.get() + 1;
        self.changes.set(n);
        let change_id = format!("x-{}-{n}", self.generation);
        let mut marked = false;
        for row in self.rows.borrow_mut().iter_mut().filter(|row| ids.contains(&row.id)) {
            row.change_id = Some(change_id.clone());
            marked = true;
        }
        if marked {
            self.publish(snapshots);
        }
    }

    /// An audit row for a call that ended unsure (CC-2's W1-b): what was asked, by which host call, that it may have
    /// changed the DAW, and that it can't be undone from here.
    pub fn audit(&self, title: &str, project_key: &str, reason: &str, identity: CallIdentity, snapshots: &Snapshots) -> Row {
        let generation = self.generation.clone();
        self.audit_in(&generation, title, project_key, reason, identity, snapshots)
    }

    /// An audit row for a call the runtime of `generation` made (one a stopped runtime left unsettled, R4).
    pub fn audit_in(
        &self,
        generation: &str,
        title: &str,
        project_key: &str,
        reason: &str,
        identity: CallIdentity,
        snapshots: &Snapshots,
    ) -> Row {
        let n = self.audits.get() + 1;
        self.audits.set(n);
        let row = Row {
            id: format!("h-{}-u{n}", self.generation),
            label: String::new(),
            project_key: project_key.to_string(),
            runtime_generation: generation.to_string(),
            title: title.to_string(),
            family: "unsure".into(),
            at: kumi_common::time::now_ms(),
            state: "unsure".into(),
            undo_available: false,
            track: None,
            note: Some(reason.to_string()),
            reason: None,
            profile_id: identity.profile_id.map(str::to_string),
            session_id: identity.session_id.map(str::to_string),
            tool_call_id: identity.tool_call_id.map(str::to_string),
            digest: Some(identity.digest.to_string()),
            call_outcome: None,
            change_id: None,
            sources: None,
            script: None,
        };
        {
            let mut rows = self.rows.borrow_mut();
            rows.insert(0, row.clone());
            rows.truncate(KEPT);
        }
        self.publish(snapshots);
        row
    }

    /// What came of the host call `call` names (its profile, session and id) when its rows are here (CC-5's C5-3: a retry the in-memory table no
    /// longer knows, after a restart say): the outcome again, from its rows, when it asked the same (`digest`); refused
    /// `duplicate` when it asked something else. None when no row came from it.
    pub fn ran(&self, call: CallIdentity) -> Option<Result<Value, String>> {
        let tool_call_id = call.tool_call_id?;
        let rows = self.rows.borrow();
        let mut made: Vec<&Row> = rows
            .iter()
            // A row an earlier runtime wrote without the call's profile and session (from before rows kept them)
            // matches whichever: a retry right after an update isn't run again. This runtime's rows match exactly.
            .filter(|row| {
                let earlier = row.runtime_generation != self.generation;
                row.tool_call_id.as_deref() == Some(tool_call_id)
                    && (row.profile_id.as_deref() == call.profile_id || (earlier && row.profile_id.is_none()))
                    && (row.session_id.as_deref() == call.session_id || (earlier && row.session_id.is_none()))
            })
            .collect();
        if made.is_empty() {
            return None;
        }
        if made.iter().any(|row| row.digest.as_deref() != Some(call.digest)) {
            return Some(Err("This host call already ran with other arguments: a call is run once, as first asked.".into()));
        }
        made.reverse();
        // What the call answered, or (a call a stopped runtime left, which answered nothing) unsure; and each row as it
        // is now (undone since, say).
        let state = made.iter().find_map(|row| row.call_outcome.clone()).unwrap_or_else(|| "unsure".into());
        Some(Ok(json!({
            "state": state,
            "history_ids": made.iter().map(|row| row.id.clone()).collect::<Vec<_>>(),
            "rows": made.iter().map(|row| json!({"id": row.id, "state": row.state, "undo_available": row.undo_available})).collect::<Vec<_>>(),
            "steps": [],
            "summary": made.iter().map(|row| row.title.clone()).collect::<Vec<_>>().join("; "),
            "undo_available": made.iter().any(|row| row.undo_available),
            "replayed": true,
            "reason": "This call already ran: what came of it is in HISTORY, and it isn't run again.",
        })))
    }

    /// `history.json`: this runtime's generation (a row with another is an earlier runtime's) and the rows.
    pub fn publish(&self, snapshots: &Snapshots) {
        let items: Vec<Value> = self.rows.borrow().iter().filter_map(|row| self.row_json(row)).collect();
        if let Err(error) = snapshots.write_owned("history", json!({"generation": self.generation, "items": items})) {
            say(&format!("kumi serve: history.json not written: {error}"));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use kumi_types::contracts::{ChangeFamily, TrackChip};

    fn change(id: &str, state: ChangeState) -> ChangeRecord {
        ChangeRecord {
            id: id.into(),
            family: ChangeFamily::Mixer,
            title: "Drums volume −6 → −4 dB".into(),
            track: Some(TrackChip { name: "Drums".into(), color: None }),
            from: None,
            to: None,
            range: None,
            colors: None,
            clip: None,
            devices: None,
            state,
            score: None,
            note: None,
            at: 1_700_000_000_000,
            picture: None,
            batch: None,
        }
    }

    fn call(tool_call_id: &'static str, digest: &'static str) -> CallIdentity<'static> {
        CallIdentity { profile_id: Some("work"), session_id: Some("chat-1"), tool_call_id: Some(tool_call_id), digest }
    }

    #[test]
    fn a_calls_changes_are_its_rows_and_their_states_follow() {
        let dir = tempfile::tempdir().unwrap();
        let state = dir.path().join("state");
        let snapshots = Snapshots::new(state.clone());
        let history = History::new(&state, "g1".into());
        history.begin_call();
        history.record(&change("c1", ChangeState::Applied), "p-earlier", &snapshots);
        history.end_call(call("tc-0", "d0"), &snapshots);
        history.begin_call();
        history.record(&change("c2", ChangeState::Applied), "p-1", &snapshots);
        // An earlier change's state moving on during this call (an expiry, say) is still the earlier call's.
        history.record(&change("c1", ChangeState::Expired), "p-1", &snapshots);
        let made = history.end_call(call("tc-1", "d1"), &snapshots);
        assert_eq!(made.iter().map(|row| row.id.as_str()).collect::<Vec<_>>(), ["h-g1-c2"]);
        assert_eq!((made[0].tool_call_id.as_deref(), made[0].digest.as_deref(), made[0].undo_available), (Some("tc-1"), Some("d1"), true));
        assert_eq!((made[0].family.as_str(), made[0].track.as_deref()), ("mixer", Some("Drums")));
        assert_eq!(history.row("h-g1-c1").unwrap().tool_call_id.as_deref(), Some("tc-0"), "its call stays");
        let undone = history.record(&change("c2", ChangeState::Undone), "p-other", &snapshots);
        assert_eq!(
            (undone.state.as_str(), undone.undo_available, undone.project_key.as_str()),
            ("undone", false, "p-1"),
            "its project stays"
        );
        assert_eq!(undone.tool_call_id.as_deref(), Some("tc-1"));
        assert!(!history.record(&change("c3", ChangeState::Unsure), "p-1", &snapshots).undo_available, "unsure isn't undone from here");
        snapshots.flush();
        let kept: Value = serde_json::from_slice(&std::fs::read(state.join("history.json")).unwrap()).unwrap();
        assert_eq!(kept["items"][0]["id"], "h-g1-c3", "newest first");
    }

    #[test]
    fn rows_an_earlier_runtime_left_are_for_the_record_only() {
        let dir = tempfile::tempdir().unwrap();
        let state = dir.path().join("state");
        let snapshots = Snapshots::new(state.clone());
        History::new(&state, "g1".into()).record(&change("c1", ChangeState::Applied), "p-1", &snapshots);
        snapshots.flush();
        let later = History::new(&state, "g2".into());
        let row = later.row("h-g1-c1").unwrap();
        assert!(!row.undo_available, "its undo went with its runtime");
        later.record(&change("c1", ChangeState::Applied), "p-1", &snapshots);
        assert!(later.row("h-g2-c1").is_some() && later.row("h-g1-c1").is_some(), "another runtime's c1 is another change");
        let first = later.audit("Raise the drums", "p-1", "REAPER didn't answer", call("tc-2", "d2"), &snapshots);
        let second = later.audit("Raise the bass", "p-1", "REAPER didn't answer", call("tc-3", "d3"), &snapshots);
        assert_eq!((first.state.as_str(), first.undo_available, first.tool_call_id.as_deref()), ("unsure", false, Some("tc-2")));
        assert_ne!(first.id, second.id, "two in one millisecond are two rows");
    }

    #[test]
    fn a_host_call_its_rows_remember_is_answered_from_them_after_a_restart() {
        let dir = tempfile::tempdir().unwrap();
        let state = dir.path().join("state");
        let snapshots = Snapshots::new(state.clone());
        let first = History::new(&state, "g1".into());
        first.begin_call();
        first.record(&change("c1", ChangeState::Applied), "p-1", &snapshots);
        first.record(&change("c2", ChangeState::Applied), "p-1", &snapshots);
        let made: Vec<String> = first.end_call(call("tc-1", "d1"), &snapshots).into_iter().map(|row| row.id).collect();
        first.one_change(&made, &snapshots);
        first.call_came_to(&made, "partial", &snapshots);
        first.record(&change("c2", ChangeState::Undone), "p-1", &snapshots);
        first.audit("Raise the bass", "p-1", "REAPER stopped answering", call("tc-2", "d2"), &snapshots);
        first.begin_call();
        first.record(&change("c3", ChangeState::Applied), "p-1", &snapshots);
        let next: Vec<String> = first.end_call(call("tc-3", "d3"), &snapshots).into_iter().map(|row| row.id).collect();
        first.one_change(&next, &snapshots);
        snapshots.flush();
        let later = History::new(&state, "g2".into());
        // One change's rows keep one id, through an undo of one of them and a restart; another call's is another; a row
        // no `change` recorded is a change of its own.
        let kept = |id: &str| later.row(id).unwrap().change_id;
        let one = kept("h-g1-c1");
        assert!(one.is_some(), "stamped");
        assert_eq!(kept("h-g1-c2"), one, "the same change, undone since");
        assert!(kept("h-g1-c3").is_some_and(|other| Some(&other) != one.as_ref()), "another call's change");
        assert_eq!(later.rows(10).iter().find(|row| row.tool_call_id.as_deref() == Some("tc-2")).unwrap().change_id, None);
        let again = later.ran(call("tc-1", "d1")).unwrap().unwrap();
        assert_eq!(
            (again["state"].clone(), again["history_ids"].clone()),
            (json!("partial"), json!(["h-g1-c1", "h-g1-c2"])),
            "as it answered"
        );
        assert_eq!(again["rows"][1]["state"], "undone", "each row as it is now");
        assert_eq!((again["replayed"].clone(), again["undo_available"].clone()), (json!(true), json!(false)));
        assert_eq!(later.ran(call("tc-2", "d2")).unwrap().unwrap()["state"], "unsure");
        assert!(later.ran(call("tc-1", "another")).unwrap().is_err(), "the same call asking something else");
        assert!(later.ran(call("tc-9", "d1")).is_none(), "a call no row came from runs");
        let elsewhere = CallIdentity { session_id: Some("chat-2"), ..call("tc-1", "d1") };
        assert!(later.ran(elsewhere).is_none(), "another chat's call_0 is another call");
    }

    #[test]
    fn a_change_only_the_daws_undo_takes_back_is_never_undone_here_and_says_where_it_stands() {
        let dir = tempfile::tempdir().unwrap();
        let state = dir.path().join("state");
        let snapshots = Snapshots::new(state.clone());
        let history = History::new(&state, "g1".into());
        history.begin_call();
        history.daw_undo_call("Live's own Undo takes it back.".into());
        history.record(&change("c1", ChangeState::Applied), "p-1", &snapshots);
        history.record(&change("c2", ChangeState::Applied), "p-1", &snapshots);
        let made = history.end_call(call("tc-1", "d1"), &snapshots);
        // Every row of the change, its undoable steps included, says so; and stays so through a later state.
        for row in &made {
            assert_eq!((row.undo_available, row.reason.as_deref()), (false, Some(DAW_UNDO)), "{}", row.id);
            assert_eq!(row.note.as_deref(), Some("Live's own Undo takes it back."));
        }
        let since = history.record(&change("c2", ChangeState::Applied), "p-1", &snapshots);
        assert_eq!((since.undo_available, since.reason.as_deref()), (false, Some(DAW_UNDO)), "a state since keeps it");
        let place = |id: &str| history.daw_undo_place(&history.row(id).unwrap());
        assert_eq!(place("h-g1-c1"), Some("next"));
        // Kumi writing in another project leaves it the latest step in its own; writing in its project buries it.
        history.wrote("p-2");
        assert_eq!(place("h-g1-c1"), Some("next"));
        history.begin_call();
        history.record(&change("c3", ChangeState::Applied), "p-1", &snapshots);
        let later = history.end_call(call("tc-2", "d2"), &snapshots);
        assert!(later[0].undo_available && later[0].reason.is_none(), "the next call is undone as ever");
        assert_eq!((place("h-g1-c1"), place("h-g1-c3")), (Some("buried"), None));
        snapshots.flush();
        let kept: Value = serde_json::from_slice(&std::fs::read(state.join("history.json")).unwrap()).unwrap();
        assert_eq!((kept["items"][1]["id"].clone(), kept["items"][1]["daw_undo"].clone()), (json!("h-g1-c2"), json!("buried")));
        // Another runtime knows the class, never where it stands.
        let after = History::new(&state, "g2".into());
        let old = after.row("h-g1-c1").unwrap();
        assert_eq!((old.reason.as_deref(), after.daw_undo_place(&old)), (Some(DAW_UNDO), None));
    }

    #[test]
    fn a_script_run_buries_a_change_only_the_daws_undo_takes_back() {
        let dir = tempfile::tempdir().unwrap();
        let snapshots = Snapshots::new(dir.path().to_path_buf());
        let history = History::new(dir.path(), "g1".into());
        history.begin_call();
        history.daw_undo_call("Live's own Undo takes it back.".into());
        history.record(&change("c1", ChangeState::Applied), "p-1", &snapshots);
        history.end_call(call("tc-1", "d1"), &snapshots);
        let place = || history.daw_undo_place(&history.row("h-g1-c1").unwrap());
        assert_eq!(place(), Some("next"));
        let run = ScriptRun { sha256: "a".repeat(64), language: "python".into() };
        history.begin_script("Script: Name the drums", &run);
        let row = history.end_script(call("tc-2", "d2"), "p-1", "applied", "Kumi can't undo a script.".into(), &snapshots);
        assert_eq!((row.family.as_str(), row.reason.as_deref(), history.daw_undo_place(&row)), ("script", None, None));
        assert_eq!(place(), Some("buried"), "the script's step is the DAW's latest now");
    }

    #[test]
    fn a_row_without_its_chat_matches_any_only_when_an_earlier_runtime_wrote_it() {
        let dir = tempfile::tempdir().unwrap();
        let state = dir.path().join("state");
        let snapshots = Snapshots::new(state.clone());
        let unscoped = |id| CallIdentity { profile_id: None, session_id: None, tool_call_id: Some(id), digest: "d1" };
        let before = History::new(&state, "g1".into());
        before.begin_call();
        before.record(&change("c1", ChangeState::Applied), "p-1", &snapshots);
        before.end_call(unscoped("call_0"), &snapshots);
        // This runtime's own row without a chat is that call's alone.
        assert!(before.ran(call("call_0", "d1")).is_none(), "another chat's call_0, in the runtime that wrote the row");
        snapshots.flush();
        // A row from before rows kept the chat: a retry right after the update isn't run again.
        let after = History::new(&state, "g2".into());
        assert!(after.ran(call("call_0", "d1")).is_some_and(|ran| ran.is_ok()));
    }
}
