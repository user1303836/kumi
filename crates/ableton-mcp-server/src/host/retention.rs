//! Ordered transaction maps share a budget, while preserving outstanding recovery authority.
use crate::live::LiveError;
use kumi_common::{
    js::{json, string},
    time::now_ms,
};
use serde_json::Value;
use std::{
    cell::{Cell, RefCell},
    collections::HashSet,
    rc::{Rc, Weak},
};

pub const MAX_RETAINED_TRANSACTION_BYTES: usize = 1024 * 1024 * 1024;
pub const ACTIVE_TRANSACTION_STATES: &[&str] = &["applying", "stopping", "undoing", "capturing", "analyzing"];
pub const RECOVERY_PROTECTED_STATES: &[&str] = &["applying", "stopping", "undoing", "capturing", "analyzing", "applied", "uncertain"];
pub type TransactionRecord = Rc<RefCell<Value>>;
pub type DeleteHook = Rc<dyn Fn(TransactionRecord) -> Result<(), LiveError>>;
thread_local! { static IN_FLIGHT_TRANSACTION_IDS: RefCell<HashSet<String>> = RefCell::new(HashSet::new()); }

pub fn mark_in_flight(id: &str) {
    IN_FLIGHT_TRANSACTION_IDS.with(|ids| {
        ids.borrow_mut().insert(id.to_owned());
    });
}
pub fn clear_in_flight(id: &str) {
    IN_FLIGHT_TRANSACTION_IDS.with(|ids| {
        ids.borrow_mut().remove(id);
    });
}
pub fn any_in_flight() -> bool {
    IN_FLIGHT_TRANSACTION_IDS.with(|ids| !ids.borrow().is_empty())
}
pub fn is_in_flight(id: &str) -> bool {
    IN_FLIGHT_TRANSACTION_IDS.with(|ids| ids.borrow().contains(id))
}
pub fn is_retirable_applied_transaction(candidate: &Value) -> bool {
    if candidate["state"] != "applied" {
        return false;
    }
    let action = candidate.get("payload").and_then(|v| v.get("action"));
    match candidate["kind"].as_str() {
        Some("scene-fire" | "transport-action" | "dialog" | "clip-action") => true,
        Some("looper") => action != Some(&Value::String("set".into())),
        Some("rack") => action.is_some() && action != Some(&Value::String("set".into())),
        Some("device-advanced") => {
            action.and_then(Value::as_str).is_some_and(|a| ["re-enable-automation", "save-comparison", "set-bank"].contains(&a))
        }
        Some("drum-pad") => action.and_then(Value::as_str) == Some("delete-all-chains"),
        _ => false,
    }
}
/// The source counts JSON UTF-16 units, despite naming the estimate "bytes".
pub fn retained_bytes(value: &Value) -> usize {
    string::utf16_len(&json::stringify(value))
}

#[derive(Clone)]
struct Kept {
    map_id: u64,
    map: Weak<MapInner>,
    key: String,
    bytes: usize,
}
pub struct TransactionRetention {
    capacity: usize,
    next_id: Cell<u64>,
    retained: Cell<usize>,
    kept: RefCell<Vec<Kept>>,
}
impl Default for TransactionRetention {
    fn default() -> Self {
        Self::new(MAX_RETAINED_TRANSACTION_BYTES)
    }
}
impl TransactionRetention {
    pub fn new(capacity: usize) -> Self {
        Self { capacity, next_id: Cell::new(0), retained: Cell::new(0), kept: RefCell::new(Vec::new()) }
    }
    pub fn bytes(&self) -> usize {
        self.retained.get()
    }
    fn next_map_id(&self) -> u64 {
        let id = self.next_id.get() + 1;
        self.next_id.set(id);
        id
    }
    fn release(&self, map_id: u64, key: &str) -> bool {
        let mut kept = self.kept.borrow_mut();
        if let Some(index) = kept.iter().position(|entry| entry.map_id == map_id && entry.key == key) {
            self.retained.set(self.retained.get() - kept.remove(index).bytes);
            true
        } else {
            false
        }
    }
    fn admit(&self, map: &Rc<MapInner>, key: &str, bytes: usize) -> Result<(), LiveError> {
        let was_kept = self.release(map.id, key);
        if !was_kept && self.retained.get().saturating_add(bytes) > self.capacity {
            // Plan all victims first. A refusal must leave every existing record intact.
            let mut remaining = self.retained.get().saturating_add(bytes).saturating_sub(self.capacity);
            let mut victims = Vec::new();
            for entry in self.kept.borrow().iter() {
                if remaining == 0 {
                    break;
                }
                if let Some(map) = entry.map.upgrade() {
                    if map.evictable(&entry.key) {
                        remaining = remaining.saturating_sub(entry.bytes);
                        victims.push((map, entry.key.clone()));
                    }
                }
            }
            if remaining > 0 {
                return Err(LiveError::error("transaction capacity is exhausted by recovery-protected work"));
            }
            for (map, key) in victims {
                map.delete(&key);
            }
        }
        self.kept.borrow_mut().push(Kept { map_id: map.id, map: Rc::downgrade(map), key: key.into(), bytes });
        self.retained.set(self.retained.get().saturating_add(bytes));
        Ok(())
    }
}

struct MapInner {
    id: u64,
    retention: Rc<TransactionRetention>,
    records: RefCell<Vec<(String, TransactionRecord)>>,
    on_delete: Option<DeleteHook>,
}
impl MapInner {
    fn evictable(&self, key: &str) -> bool {
        let value = self.records.borrow().iter().find(|(id, _)| id == key).map(|(_, value)| value.clone());
        value
            .is_some_and(|value| !RECOVERY_PROTECTED_STATES.contains(&value.borrow()["state"].as_str().unwrap_or("")) && !is_in_flight(key))
    }
    fn delete(&self, key: &str) -> bool {
        self.retention.release(self.id, key);
        let removed = {
            let mut records = self.records.borrow_mut();
            records.iter().position(|(id, _)| id == key).map(|index| records.remove(index).1)
        };
        let Some(value) = removed else { return false };
        if let Some(hook) = &self.on_delete {
            // Hooks release staged files. They must not leave bookkeeping half-updated.
            let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| hook(value)));
        }
        true
    }
}
impl Drop for MapInner {
    fn drop(&mut self) {
        for (key, _) in self.records.get_mut().iter() {
            self.retention.release(self.id, key);
        }
    }
}
#[derive(Clone)]
pub struct BoundedTransactionMap(Rc<MapInner>);
impl BoundedTransactionMap {
    pub fn new(retention: Rc<TransactionRetention>, on_delete: Option<DeleteHook>) -> Self {
        Self(Rc::new(MapInner { id: retention.next_map_id(), retention, records: RefCell::new(Vec::new()), on_delete }))
    }
    pub fn get(&self, key: &str) -> Option<TransactionRecord> {
        self.0.records.borrow().iter().find(|(id, _)| id == key).map(|(_, value)| value.clone())
    }
    /// `get` without waiting on a borrow: for code that runs while a panic unwinds, where a second panic aborts.
    pub fn try_get(&self, key: &str) -> Option<TransactionRecord> {
        self.0.records.try_borrow().ok()?.iter().find(|(id, _)| id == key).map(|(_, value)| value.clone())
    }
    pub fn contains_key(&self, key: &str) -> bool {
        self.get(key).is_some()
    }
    pub fn len(&self) -> usize {
        self.0.records.borrow().len()
    }
    pub fn is_empty(&self) -> bool {
        self.0.records.borrow().is_empty()
    }
    pub fn entries(&self) -> Vec<(String, TransactionRecord)> {
        self.0.records.borrow().clone()
    }
    pub fn evictable(&self, key: &str) -> bool {
        self.0.evictable(key)
    }
    pub fn delete(&self, key: &str) -> bool {
        self.0.delete(key)
    }
    pub fn set(&self, key: &str, value: TransactionRecord) -> Result<(), LiveError> {
        self.set_at(key, value, now_ms() as f64)
    }
    pub fn insert(&self, key: &str, value: Value) -> Result<TransactionRecord, LiveError> {
        let record = Rc::new(RefCell::new(value));
        self.set(key, record.clone())?;
        Ok(record)
    }
    pub fn set_at(&self, key: &str, value: TransactionRecord, now: f64) -> Result<(), LiveError> {
        let expired: Vec<String> = self
            .entries()
            .into_iter()
            .filter_map(|(key, record)| {
                let candidate = record.borrow();
                let expires = candidate["expiresAt"].as_f64().unwrap_or(f64::NAN);
                let protected = RECOVERY_PROTECTED_STATES.contains(&candidate["state"].as_str().unwrap_or(""));
                (expires <= now && (!protected || is_retirable_applied_transaction(&candidate)) && !is_in_flight(&key)).then_some(key)
            })
            .collect();
        for key in expired {
            self.delete(&key);
        }
        self.0.retention.admit(&self.0, key, retained_bytes(&value.borrow()))?;
        let mut records = self.0.records.borrow_mut();
        if let Some((_, old)) = records.iter_mut().find(|(id, _)| id == key) {
            *old = value;
        } else {
            records.push((key.into(), value));
        }
        Ok(())
    }
}
