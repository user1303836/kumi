//! Host helpers are shared by the exact request and transaction families.

#![allow(dead_code)]
mod advanced_devices;
mod apply_failure;
mod arrangement;
mod arrangement_clip;
mod arrangement_midi;
pub mod audio;
mod audio_clip;
mod audio_import;
mod audition;
mod automation;
mod browser_render;
mod capture;
mod clip_action;
mod clip_duplicate;
mod clip_launch;
mod clip_move;
mod clip_properties;
mod data;
mod deletion;
mod device_basic;
mod device_copy;
mod device_edit;
mod device_lifecycle;
mod device_parameter;
pub mod device_state;
mod dialog;
mod dispatch;
mod drum_pad;
mod events;
mod extended_mixer;
mod fire_button;
mod follow;
mod groove;
pub mod helpers;
mod import_files;
pub mod json_diagnostics;
mod looper;
mod managed;
mod midi_plan;
#[cfg(test)]
mod midi_plan_tests;
mod midi_transform;
mod mixer;
pub mod mutations;
mod native_editing;
mod note_edit;
mod note_target;
mod object_view;
mod probe_library;
mod probes;
mod project;
mod protocol;
mod racks;
mod reads;
mod realtime;
mod record_operation;
mod recording;
mod recovery;
mod rename;
mod resources;
pub mod retention;
mod routing;
mod scene;
mod selection;
mod session_capture;
mod simpler;
mod song_settings;
mod specialized_devices;
mod structure;
mod tempo;
mod track_properties;
mod track_structure;
mod track_view;
mod transport;
mod transport_action;
mod tuning;
mod ui;
mod warp;
mod willington;

use crate::{
    live::*,
    protocol::mcp::{is_modern_unavailable_tool, ProtocolEra},
    protocol::registry::live_registry_operations,
    protocol::tool_catalog::{self, ToolPolicySpec, ToolVisibilityRow},
    transactions::{
        batch::{BatchTransactionManager, BATCH_OPERATION_POLICY_TOOLS},
        device_state::DeviceStateTransactionManager,
        session_midi::SessionMidiTransactionManager,
    },
};
pub use events::EventEmitter;
use helpers::*;
pub use protocol::{HostRequest, RequestDecision, ToolCall};
use retention::{BoundedTransactionMap, TransactionRetention};
use serde_json::{json, Value};
use std::{
    cell::{Cell, OnceCell, RefCell},
    collections::{HashMap, HashSet, VecDeque},
    rc::Rc,
    sync::LazyLock,
};

pub const SERVER_VERSION: &str = env!("CARGO_PKG_VERSION");
pub const TRANSACTION_TTL_MS: f64 = 600_000.0;
pub const MAX_TRACKED_REQUEST_IDS: usize = 4096;
static DATA: LazyLock<Value> = LazyLock::new(|| serde_json::from_str(include_str!("host/data.json")).expect("embedded host tables"));
/// The tables' lists of names as sets: a request looks its tool up in two to four of them.
static NAME_TABLES: LazyLock<HashMap<&'static str, HashSet<&'static str>>> = LazyLock::new(|| {
    DATA.as_object()
        .expect("embedded host tables")
        .iter()
        .filter_map(|(table, names)| Some((table.as_str(), names.as_array()?.iter().filter_map(Value::as_str).collect())))
        .collect()
});
fn table_has(table: &str, name: &str) -> bool {
    NAME_TABLES[table].contains(name)
}
fn policy_error(error: tool_catalog::ToolPolicyError) -> LiveError {
    LiveError::type_error(error.to_string())
}

#[derive(Default, Clone)]
pub struct McpHostOptions {
    pub tool_policy: Option<Value>,
    pub import_staging_dir: Option<String>,
    pub user_library_dir: Option<String>,
    pub live_resources_dir: Option<String>,
    /// The version the host reports (`serverInfo`, `exporterVersion`); the bridge's own when unset.
    /// Golden-file tests pin it, so a version bump changes none of them.
    pub server_version: Option<String>,
    /// The clock (milliseconds) the Browser search's kept walk is aged by; the system's when unset.
    pub now: Option<std::rc::Rc<dyn Fn() -> f64>>,
}
/// Shared state of one stdio host. Protocol decisions retain their request lease until execution ends.
pub struct McpHost {
    browser_search_cache: RefCell<VecDeque<(String, browser_render::BrowserCache)>>,
    fused_changes: RefCell<VecDeque<(String, Value)>>,
    record_operations: RefCell<Vec<record_operation::RecordOperation>>,
    undo_recovery_plans: RefCell<Vec<recovery::RecoveryPlan>>,
    undo_refusals: RefCell<std::collections::HashMap<String, recovery::UndoRefusal>>,
    undo_watches: RefCell<Vec<Rc<Cell<usize>>>>,
    in_flight_mutations: RefCell<std::collections::HashMap<String, Rc<mutations::MutationFlight>>>,
    open_undo_step: RefCell<Option<Value>>,
    song_history_calls: RefCell<VecDeque<(String, Value)>>,
    analysis_runner: crate::analysis::runner::AnalysisRunner,
    adapter: Rc<dyn AsyncLiveAdapter>,
    /// The last adapter call that failed (see `apply_failure`).
    last_live_failure: Rc<RefCell<Option<apply_failure::NotedFailure>>>,
    views: Rc<LiveViews>,
    initialized: Cell<bool>,
    initialized_notification: Cell<bool>,
    protocol_era: Cell<Option<ProtocolEra>>,
    modern_in_flight_ids: Rc<RefCell<HashSet<String>>>,
    shutting_down: Cell<bool>,
    seen_ids: RefCell<HashSet<String>>,
    id_order: RefCell<VecDeque<String>>,
    tool_policy: Rc<RefCell<ToolPolicySpec>>,
    tool_gate: RefCell<Option<Rc<ToolGate>>>,
    tool_list_fingerprint: RefCell<Option<String>>,
    events: Rc<events::EventState>,
    retention: Rc<TransactionRetention>,
    transactions: BoundedTransactionMap,
    audio_capture_transactions: BoundedTransactionMap,
    capture_controllers: RefCell<Vec<capture::CaptureController>>,
    arrangement_transactions: BoundedTransactionMap,
    session_structure_transactions: BoundedTransactionMap,
    device_parameter_transactions: BoundedTransactionMap,
    device_parameters_transactions: BoundedTransactionMap,
    audition_transactions: BoundedTransactionMap,
    transport_transactions: BoundedTransactionMap,
    clip_launch_transactions: BoundedTransactionMap,
    note_edit_transactions: BoundedTransactionMap,
    clip_lifecycle_transactions: BoundedTransactionMap,
    midi_transactions: SessionMidiTransactionManager,
    batch_transactions: BatchTransactionManager,
    device_state_transactions: DeviceStateTransactionManager,
    recovery_finalization_in_flight: Cell<bool>,
    active_async_operations: Cell<usize>,
    options: McpHostOptions,
    import_files: Rc<import_files::ImportFiles>,
    semantic_exports: RefCell<VecDeque<project::SemanticExport>>,
    library_reads: probe_library::LibraryReads,
}
impl Default for McpHost {
    fn default() -> Self {
        Self::new(Rc::new(UnavailableLiveAdapter), McpHostOptions::default()).expect("default policy")
    }
}
impl McpHost {
    pub fn server_version(&self) -> &str {
        self.options.server_version.as_deref().unwrap_or(SERVER_VERSION)
    }
    pub fn new(adapter: Rc<dyn AsyncLiveAdapter>, options: McpHostOptions) -> Result<Self, LiveError> {
        let policy = Rc::new(RefCell::new(tool_catalog::parse_tool_policy_spec(options.tool_policy.as_ref()).map_err(policy_error)?));
        let last_live_failure = Rc::new(RefCell::new(None));
        let adapter: Rc<dyn AsyncLiveAdapter> =
            Rc::new(apply_failure::FailureNotingAdapter { adapter, last_failure: last_live_failure.clone() });
        let provider = adapter.clone();
        let views = Rc::new(LiveViews::new(move || provider.clone()));
        let policy_for_batch = policy.clone();
        let adapter_for_batch = adapter.clone();
        let batch = BatchTransactionManager::new(
            adapter.clone(),
            Some(Rc::new(move |kinds| {
                let rows = tool_catalog::resolve_tool_visibility(&safe_adapter_status(&*adapter_for_batch), &policy_for_batch.borrow())
                    .map_err(policy_error)?;
                for kind in kinds {
                    let owner = BATCH_OPERATION_POLICY_TOOLS.iter().find(|(candidate, _)| candidate == kind).map(|(_, owner)| *owner);
                    if let Some(owner) = owner {
                        if !rows.iter().any(|row| row.entry.name == owner && row.policy_allowed) {
                            return Err(LiveError::error(format!(
                                "transaction batch contains an operation denied by the deployment policy ({owner})"
                            )));
                        }
                    }
                }
                Ok(())
            })),
            Some(views.clone()),
        );
        let retention = Rc::new(TransactionRetention::default());
        let map = || BoundedTransactionMap::new(retention.clone(), None);
        let import_files = Rc::new(import_files::ImportFiles::new(&options));
        let cleanup_imports = import_files.clone();
        let clip_lifecycle_transactions = BoundedTransactionMap::new(
            retention.clone(),
            Some(Rc::new(move |value| {
                cleanup_imports.release_unused(&value.borrow());
                Ok(())
            })),
        );
        Ok(Self {
            browser_search_cache: RefCell::new(VecDeque::new()),
            fused_changes: RefCell::new(VecDeque::new()),
            record_operations: RefCell::new(Vec::new()),
            undo_recovery_plans: RefCell::new(Vec::new()),
            undo_refusals: RefCell::new(Default::default()),
            undo_watches: RefCell::new(Vec::new()),
            in_flight_mutations: RefCell::new(Default::default()),
            open_undo_step: RefCell::new(None),
            song_history_calls: RefCell::new(VecDeque::new()),
            analysis_runner: crate::analysis::runner::AnalysisRunner::new(),
            initialized: Cell::new(false),
            initialized_notification: Cell::new(false),
            protocol_era: Cell::new(None),
            modern_in_flight_ids: Rc::new(RefCell::new(HashSet::new())),
            shutting_down: Cell::new(false),
            seen_ids: RefCell::new(HashSet::new()),
            id_order: RefCell::new(VecDeque::new()),
            tool_policy: policy,
            tool_gate: RefCell::new(None),
            tool_list_fingerprint: RefCell::new(None),
            events: Rc::new(events::EventState::default()),
            transactions: map(),
            audio_capture_transactions: map(),
            capture_controllers: RefCell::new(Vec::new()),
            arrangement_transactions: map(),
            session_structure_transactions: map(),
            device_parameter_transactions: map(),
            device_parameters_transactions: map(),
            audition_transactions: map(),
            transport_transactions: map(),
            clip_launch_transactions: map(),
            note_edit_transactions: map(),
            clip_lifecycle_transactions,
            midi_transactions: SessionMidiTransactionManager::new(adapter.clone(), Some(views.clone())),
            device_state_transactions: DeviceStateTransactionManager::new(adapter.clone(), Some(views.clone())),
            batch_transactions: batch,
            views,
            adapter,
            last_live_failure,
            retention,
            recovery_finalization_in_flight: Cell::new(false),
            active_async_operations: Cell::new(0),
            options,
            import_files,
            semantic_exports: RefCell::new(VecDeque::new()),
            library_reads: Default::default(),
        })
    }
    pub fn effective_tool_policy(&self) -> ToolPolicySpec {
        self.tool_policy.borrow().clone()
    }
    pub fn set_tool_policy(&self, policy: &Value) -> Result<ToolPolicySpec, LiveError> {
        let parsed = tool_catalog::parse_tool_policy_spec(Some(policy)).map_err(policy_error)?;
        *self.tool_policy.borrow_mut() = parsed.clone();
        self.note_tool_list_changed()?;
        Ok(parsed)
    }
    pub fn is_shutting_down(&self) -> bool {
        self.shutting_down.get()
    }
    pub fn safe_adapter_status(&self) -> LiveStatus {
        safe_adapter_status(&*self.adapter)
    }
    pub fn require_connected(&self, capability: Option<&str>) -> Result<LiveStatus, LiveError> {
        let status = self.safe_adapter_status();
        if !status.connected || status.epoch.is_none() {
            return Err(LiveError::error("live-adapter-unavailable"));
        }
        if let Some(capability) = capability {
            if !status.capabilities.iter().any(|c| c.as_str() == capability) {
                return Err(LiveError::error(format!("live-capability-unavailable:{capability}")));
            }
        }
        Ok(status)
    }
    pub async fn fresh_status(&self, context: Option<&LiveOperationContext>) -> Result<LiveStatus, LiveError> {
        if !self.adapter.has_refresh_status_async() {
            return self.adapter.status();
        }
        let status = self.adapter.refresh_status_async(context).await?;
        self.note_tool_list_changed()?;
        Ok(status)
    }
    pub fn deadline(&self, base: f64) -> f64 {
        kumi_common::time::now_ms_f64() + (base + 20.0 * self.views.track_count.get() as f64).min(60_000.0)
    }
    /// The tool gate for the adapter's status, the policy and the protocol era now: the kept one while all three are
    /// what it was resolved from (the status compared whole, as the adapter reports it), else a new one.
    fn tool_gate(&self) -> Rc<ToolGate> {
        let read = self.adapter.status().ok();
        let era = self.protocol_era.get();
        if let Some(gate) = self.tool_gate.borrow().as_ref() {
            if gate.era == era && gate.read == read && gate.policy == *self.tool_policy.borrow() {
                return gate.clone();
            }
        }
        let gate = Rc::new(ToolGate {
            status: checked_status(read.clone()),
            read,
            policy: self.tool_policy.borrow().clone(),
            era,
            rows: OnceCell::new(),
            fingerprint: OnceCell::new(),
        });
        *self.tool_gate.borrow_mut() = Some(gate.clone());
        gate
    }
    pub fn tool_visibility_rows(&self) -> Result<Vec<ToolVisibilityRow>, LiveError> {
        Ok(self.tool_gate().rows()?.to_vec())
    }
    pub fn tool_callable(&self, name: &str) -> Result<bool, LiveError> {
        Ok(self.tool_gate().row(name)?.is_some_and(|row| row.visible))
    }
    pub fn policy_allows_tool(&self, name: Option<&str>) -> Result<bool, LiveError> {
        let Some(name) = name else { return Ok(true) };
        Ok(self.tool_gate().row(name)?.is_some_and(|row| row.policy_allowed))
    }
    pub fn tool_gate_error(&self, id: &Value, name: &str) -> Result<Value, LiveError> {
        if tool_catalog::tool_catalog_entry(name).is_none() {
            return Ok(error(id, -32601, "Tool not found", None));
        }
        let denied = self.tool_gate().row(name)?.is_some_and(|row| row.executable && !row.policy_allowed);
        Ok(reason_error(id,if denied {"tool-denied-by-deployment-policy"}else{"tool-unavailable-in-current-live-shape"},"Consult the capabilities resource for the executable tools and effective deployment policy; hidden tools are never dispatched."))
    }
    pub fn transaction_owner_tool(transaction_id: &str, kind: Option<&str>) -> Option<&'static str> {
        DATA["transactionOwnerPrefixes"]
            .as_object()
            .unwrap()
            .iter()
            .filter(|(prefix, _)| transaction_id.starts_with(prefix.as_str()))
            .max_by_key(|(prefix, _)| prefix.len())
            .and_then(|(_, name)| name.as_str())
            .or_else(|| kind.and_then(|kind| DATA["transactionOwnerKinds"].get(kind)).and_then(Value::as_str))
    }
    pub fn live_status(&self, id: &Value) -> Value {
        success_text(id, &serde_json::to_value(self.safe_adapter_status()).unwrap())
    }
    pub async fn live_status_async(&self, id: &Value) -> Value {
        let _ = self.fresh_status(Some(&LiveOperationContext::with_deadline(kumi_common::time::now_ms_f64() + 5000.0))).await;
        self.live_status(id)
    }
}
/// Which tools Live's status and the deployment policy let through, resolved once for each status the adapter reports:
/// every request asks, and the status seldom changes. The gate is kept with the status it was resolved from and is used
/// again only while the adapter reports that same status, so a change no listener heard of is never missed.
struct ToolGate {
    /// What the adapter reported (nothing when it failed), before `checked_status` judged it.
    read: Option<LiveStatus>,
    status: LiveStatus,
    policy: ToolPolicySpec,
    era: Option<ProtocolEra>,
    rows: OnceCell<Result<Vec<ToolVisibilityRow>, tool_catalog::ToolPolicyError>>,
    fingerprint: OnceCell<String>,
}
impl ToolGate {
    /// Every catalog tool's row, in the catalog's order.
    fn rows(&self) -> Result<&[ToolVisibilityRow], LiveError> {
        let rows = self.rows.get_or_init(|| {
            let mut rows = tool_catalog::resolve_tool_visibility(&self.status, &self.policy)?;
            if self.era == Some(ProtocolEra::Modern) {
                for row in &mut rows {
                    if is_modern_unavailable_tool(&row.entry.name) {
                        row.executable = false;
                        row.visible = false;
                    }
                }
            }
            Ok(rows)
        });
        rows.as_deref().map_err(|error| policy_error(error.clone()))
    }
    /// The row of the tool called `name`, if the catalog has one.
    fn row(&self, name: &str) -> Result<Option<&ToolVisibilityRow>, LiveError> {
        let rows = self.rows()?;
        Ok(tool_catalog::tool_catalog_index(name).and_then(|index| rows.get(index)))
    }
    /// What `tools/list_changed` follows: the parts of Live's status and the policy a tool listing is made of.
    fn fingerprint(&self) -> Result<String, LiveError> {
        use sha2::Digest;
        if let Some(fingerprint) = self.fingerprint.get() {
            return Ok(fingerprint.clone());
        }
        let status = &self.status;
        let fingerprint = hex::encode(sha2::Sha256::digest(
            canonical_mutation_identity(&json!({
                "connected":status.connected,"epoch":status.epoch,"adapter":status.adapter,
                "operations":status.operations.as_deref().unwrap_or_default(),"capabilities":status.capabilities,"policy":self.policy
            }))?
            .as_bytes(),
        ));
        Ok(self.fingerprint.get_or_init(|| fingerprint).clone())
    }
}
fn safe_adapter_status(adapter: &dyn AsyncLiveAdapter) -> LiveStatus {
    checked_status(adapter.status().ok())
}
/// The status the adapter reported, when the host can trust it; Live's unavailable status otherwise.
fn checked_status(read: Option<LiveStatus>) -> LiveStatus {
    if let Some(status) = read {
        let capabilities: HashSet<_> = status.capabilities.iter().collect();
        let operations_valid = status.operations.as_ref().is_none_or(|operations| {
            let unique: HashSet<_> = operations.iter().collect();
            unique.len() == operations.len()
                && operations.iter().all(|operation| crate::protocol::registry::is_live_registry_operation(operation))
        });
        let hash_valid = status
            .registry_hash
            .as_ref()
            .is_none_or(|hash| hash.len() == 64 && hash.bytes().all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b)));
        if status.protocol == LIVE_PROTOCOL_VERSION
            && status.epoch.is_none_or(|e| (1..=9_007_199_254_740_991).contains(&e))
            && (!status.connected || status.epoch.is_some())
            && capabilities.len() == status.capabilities.len()
            && operations_valid
            && hash_valid
        {
            return status;
        }
    }
    let mut unavailable = UnavailableLiveAdapter.status().expect("unavailable status");
    unavailable.reason = Some("live-adapter-status-unavailable".into());
    unavailable
}
