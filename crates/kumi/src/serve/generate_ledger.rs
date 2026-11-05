//! Read-only authority for Hermes' Mirelo ledger (CC-13). Python is the only writer.
//!
//! Call `read` in `spawn_blocking`, immediately before the operation it authorizes. The returned
//! snapshot owns its data and holds no OS lock. An approval is not a dispatch/replay token: the
//! native job's durable pre-POST checkpoint must still prevent a second submission. Recovery
//! returns routing data only, never permission to upload or POST.

use super::ctx::OpError;
use serde::Deserialize;
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, BTreeSet};
use std::fs::{File, OpenOptions};
use std::io::{self, Read};
#[cfg(any(windows, test))]
use std::path::PathBuf;
use std::path::{Component, Path};
use std::time::{Duration, Instant};

const MAX_INTEGER: u64 = (1_u64 << 53) - 1;
const MAX_BYTES: u64 = 4 * 1024 * 1024;
const LOCK_TIMEOUT: Duration = Duration::from_secs(2);

pub struct Authority {
    ledger: Ledger,
    committed: u64,
    directory: Option<Directory>,
}

/// Policy permission for a quote or local preview. A zero cap does not prevent a quote.
#[derive(Debug, PartialEq, Eq)]
pub struct Policy {
    pub cap: u64,
}

pub struct CreateClaim<'a> {
    pub quote_id: &'a str,
    pub request_sha256: &'a str,
    pub kind: &'a str,
    pub reservation_id: &'a str,
    pub reserved_credits: u64,
    pub host_call_id: &'a str,
    pub now_ms: u64,
    /// None at initial dispatch; the worker's own native ID for its final pre-POST recheck.
    pub expected_job_id: Option<&'a str>,
}

#[derive(Debug, PartialEq, Eq)]
pub struct Approval {
    pub quote_id: String,
    pub reservation_id: String,
    pub kind: String,
    pub reserved_credits: u64,
    pub expires_at: u64,
}

/// Saved routing/receipt information. This type deliberately cannot be used as an Approval.
#[derive(Debug, PartialEq, Eq)]
pub struct Recovery {
    pub reservation_id: String,
    pub quote_id: String,
    pub kind: String,
    pub job_id: Option<String>,
    pub provider_job_id: Option<String>,
    pub reserved_credits: u64,
    pub submission: String,
    pub settlement: String,
    pub actual_credits: Option<u64>,
}

/// Borrowed active routing evidence; the original approval is not necessarily held credit.
#[derive(Debug, PartialEq, Eq)]
pub struct Unsettled<'a> {
    pub quote_id: &'a str,
    pub kind: &'a str,
    pub job_id: Option<&'a str>,
    pub reserved_credits: u64,
    pub settlement: &'a str,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Ledger {
    schema: u64,
    #[serde(default)]
    policy: Option<StoredPolicy>,
    frozen: bool,
    #[serde(deserialize_with = "required_nullable")]
    freeze_reason: Option<String>,
    #[serde(deserialize_with = "unique_map")]
    quotes: BTreeMap<String, Quote>,
    #[serde(deserialize_with = "unique_map")]
    reservations: BTreeMap<String, Reservation>,
    archived_count: u64,
    archived_spent: u64,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct StoredPolicy {
    acknowledged: bool,
    enabled: Enabled,
    cap: u64,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Enabled {
    sfx: bool,
    audio_to_midi: bool,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Estimate {
    credits: u64,
    max_credits: u64,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Quote {
    state: String,
    quote_id: String,
    kind: String,
    #[serde(deserialize_with = "required_nullable")]
    prompt: Option<String>,
    #[serde(deserialize_with = "required_nullable")]
    file: Option<String>,
    seconds: f64,
    estimate: Estimate,
    credit_unit: String,
    upload: bool,
    expires_at: u64,
    request_sha256: String,
    summary: String,
    host_call_id: String,
    consumed: bool,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Reservation {
    reservation_id: String,
    quote_id: String,
    request_sha256: String,
    host_call_id: String,
    kind: String,
    reserved_credits: u64,
    expires_at: u64,
    created_at: u64,
    #[serde(deserialize_with = "required_nullable")]
    job_id: Option<String>,
    #[serde(deserialize_with = "required_nullable")]
    provider_job_id: Option<String>,
    submission: String,
    settlement: String,
    #[serde(deserialize_with = "required_nullable")]
    actual_credits: Option<u64>,
    #[serde(default, deserialize_with = "present")]
    manually_released: Option<bool>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct HistoryIndex {
    schema: u64,
    sequence: u64,
    reservation_id: String,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct HistoryReceipt {
    schema: u64,
    sequence: u64,
    quote: Quote,
    reservation: Reservation,
}

impl Authority {
    pub fn read(state_dir: &Path) -> Result<Self, OpError> {
        Self::read_with_timeout(state_dir, LOCK_TIMEOUT)
    }

    fn read_with_timeout(state_dir: &Path, timeout: Duration) -> Result<Self, OpError> {
        let data_dir = data_dir(state_dir)?;
        let directory = match Directory::open(data_dir) {
            Ok(directory) => directory,
            Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(Self::empty()),
            Err(_) => return Err(unavailable()),
        };
        // This inode is never truncated, replaced or unlinked. It is the same lock Python uses.
        let lock = directory.open_file("ledger.lock", true).map_err(|_| unavailable())?;
        acquire(&lock, timeout)?;
        let _guard = LockGuard(&lock);
        let file = match directory.open_file("ledger.json", false) {
            Ok(file) => file,
            Err(error) if error.kind() == io::ErrorKind::NotFound && lock.metadata().map_err(|_| unavailable())?.len() == 0 => {
                return Ok(Self::empty());
            }
            Err(_) => return Err(unavailable()),
        };
        let ledger: Ledger = decode_file(file)?;
        let committed = ledger.validate()?;
        // Drop releases the lock even on a parse/validation error. No network under the lock.
        Ok(Self { ledger, committed, directory: Some(directory) })
    }

    fn empty() -> Self {
        Self {
            ledger: Ledger {
                schema: 2,
                policy: None,
                frozen: false,
                freeze_reason: None,
                quotes: BTreeMap::new(),
                reservations: BTreeMap::new(),
                archived_count: 0,
                archived_spent: 0,
            },
            committed: 0,
            directory: None,
        }
    }

    pub fn policy(&self, kind: &str) -> Result<Policy, OpError> {
        if !valid_kind(kind) {
            return Err(refusal("invalid_request", "The generation kind is invalid."));
        }
        let policy = self.ledger.policy.as_ref().ok_or_else(|| refusal("not_enabled", "Mirelo generation is not enabled."))?;
        if !policy.acknowledged {
            return Err(refusal("not_approved", "Mirelo's disclosure has not been acknowledged."));
        }
        let enabled = if kind == "sfx" { policy.enabled.sfx } else { policy.enabled.audio_to_midi };
        if !enabled {
            return Err(refusal("not_enabled", "This Mirelo generation kind is not enabled."));
        }
        Ok(Policy { cap: policy.cap })
    }

    pub fn approve(&self, claim: &CreateClaim<'_>) -> Result<Approval, OpError> {
        let approval = self.prove_own(claim)?;
        let check = || {
            let policy = self.policy(claim.kind)?;
            if self.ledger.frozen {
                return Err(refusal("ledger_frozen", "New generations are paused because a charge exceeded its held credits."));
            }
            if policy.cap == 0 || self.committed > policy.cap {
                return Err(refusal("cap_exceeded", "The credit cap does not permit this generation."));
            }
            let reservation = &self.ledger.reservations[claim.reservation_id];
            if claim.now_ms >= approval.expires_at {
                return Err(refusal("expired_quote", "The generation quote has expired."));
            }
            if claim.now_ms < reservation.created_at {
                return Err(refusal("stale", "The reservation's time does not match this generation."));
            }
            Ok(())
        };
        check().map_err(|error: OpError| refusal("not_submitted", &error.reason()))?;
        Ok(approval)
    }

    /// Prove ownership before returning any refusal that permits the host to release credits.
    /// This proves binding only, never permission to submit (nor absence of a native checkpoint).
    pub fn prove_own(&self, claim: &CreateClaim<'_>) -> Result<Approval, OpError> {
        if !valid_id(claim.quote_id)
            || !valid_id(claim.reservation_id)
            || !valid_id(claim.host_call_id)
            || !valid_digest(claim.request_sha256)
            || claim.reserved_credits > MAX_INTEGER
            || claim.now_ms > MAX_INTEGER
        {
            return Err(refusal("invalid_request", "The generation approval is invalid."));
        }
        let quote =
            self.ledger.quotes.get(claim.quote_id).ok_or_else(|| refusal("unknown_quote", "The generation quote is not recorded."))?;
        let reservation = self
            .ledger
            .reservations
            .get(claim.reservation_id)
            .ok_or_else(|| refusal("not_approved", "The generation has no durable reservation."))?;
        if !quote.consumed || reservation.settlement != "pending" {
            return Err(refusal("not_approved", "The generation reservation is not pending."));
        }
        if claim.expected_job_id.is_some_and(|id| !valid_id(id)) {
            return Err(refusal("invalid_request", "The generation job binding is invalid."));
        }
        // A running preflight receipt can be observed before the worker's durable unknown
        // checkpoint. Only that worker may continue it; it is not a new dispatch authorization.
        let before_submission =
            reservation.submission == "unknown" || (claim.expected_job_id.is_some() && reservation.submission == "not_submitted");
        if !before_submission
            || reservation.provider_job_id.is_some()
            || reservation.job_id.as_deref().is_some_and(|id| Some(id) != claim.expected_job_id)
        {
            return Err(refusal("quote_consumed", "The generation is already tracked; it cannot be submitted again."));
        }
        if quote.quote_id != reservation.quote_id
            || quote.kind != claim.kind
            || quote.request_sha256 != claim.request_sha256
            || reservation.host_call_id != claim.host_call_id
            || reservation.reserved_credits != claim.reserved_credits
            || quote.estimate.max_credits != claim.reserved_credits
        {
            return Err(refusal("stale", "The reservation does not match the approved generation."));
        }
        Ok(Approval {
            quote_id: quote.quote_id.clone(),
            reservation_id: reservation.reservation_id.clone(),
            kind: quote.kind.clone(),
            reserved_credits: reservation.reserved_credits,
            expires_at: quote.expires_at,
        })
    }

    /// The exact public quote the host approved, without its bookkeeping fields. Native creation
    /// compares the complete projection, not merely a digest beside mutable display text.
    pub fn quote(&self, id: &str) -> Result<Option<serde_json::Value>, OpError> {
        let archived;
        let quote = if let Some(quote) = self.ledger.quotes.get(id) {
            quote
        } else {
            archived = self.history("quote", id)?;
            let Some(receipt) = archived.as_ref() else { return Ok(None) };
            &receipt.quote
        };
        Ok(Some(serde_json::json!({
            "state": quote.state, "quote_id": quote.quote_id, "kind": quote.kind,
            "prompt": quote.prompt, "file": quote.file, "seconds": quote.seconds,
            "estimate": {"credits": quote.estimate.credits, "max_credits": quote.estimate.max_credits},
            "credit_unit": quote.credit_unit, "upload": quote.upload, "expires_at": quote.expires_at,
            "request_sha256": quote.request_sha256, "summary": quote.summary
        })))
    }

    /// A job must already be bound by the host before GET-only recovery. An unbound lost reply is
    /// adopted by W2 from the native checkpoint's quote_id, under the writer's ledger lock.
    pub fn for_job(&self, id: &str) -> Result<Option<Recovery>, OpError> {
        if let Some(reservation) = self.ledger.reservations.values().find(|reservation| reservation.job_id.as_deref() == Some(id)) {
            return Ok(Some(reservation.recovery()));
        }
        Ok(self.history("job", id)?.map(|receipt| receipt.reservation.recovery()))
    }

    /// Reservation-id lookup for the Python/Rust storage parity fixtures.
    #[cfg(test)]
    fn recovery(&self, reservation_id: &str) -> Result<Option<Recovery>, OpError> {
        if let Some(reservation) = self.ledger.reservations.get(reservation_id) {
            return Ok(Some(reservation.recovery()));
        }
        Ok(self.history("receipt", reservation_id)?.map(|receipt| receipt.reservation.recovery()))
    }

    /// Active evidence only; callers inspect retained native checkpoints without scanning history.
    pub fn unsettled(&self) -> impl Iterator<Item = Unsettled<'_>> + '_ {
        self.ledger.reservations.values().filter(|reservation| matches!(reservation.settlement.as_str(), "pending" | "manual_release")).map(
            |reservation| Unsettled {
                quote_id: &reservation.quote_id,
                kind: &reservation.kind,
                job_id: reservation.job_id.as_deref(),
                reserved_credits: reservation.reserved_credits,
                settlement: &reservation.settlement,
            },
        )
    }

    fn history(&self, category: &str, id: &str) -> Result<Option<HistoryReceipt>, OpError> {
        let count = self.ledger.archived_count;
        if count == 0 || !valid_id(id) {
            return Ok(None);
        }
        let directory = self.directory.as_ref().ok_or_else(unavailable)?;
        let name = format!(".ledger-{category}-{:x}.json", Sha256::digest(id.as_bytes()));
        let file = match directory.open_file(&name, false) {
            Ok(file) => file,
            Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
            Err(_) => return Err(unavailable()),
        };
        let index: HistoryIndex = decode_file(file)?;
        if index.schema != 1 || index.sequence > MAX_INTEGER || !valid_id(&index.reservation_id) {
            return Err(unavailable());
        }
        if index.sequence >= count {
            return Ok(None);
        }
        let file = directory.open_file(&format!(".ledger-history-{:016x}.json", index.sequence), false).map_err(|_| unavailable())?;
        let receipt: HistoryReceipt = decode_file(file)?;
        if receipt.schema != 1
            || receipt.sequence != index.sequence
            || !receipt.quote.valid()
            || !matches!(receipt.reservation.settlement.as_str(), "charged" | "not_submitted")
        {
            return Err(unavailable());
        }
        receipt.reservation.amount(&receipt.quote)?;
        if receipt.reservation.reservation_id != index.reservation_id {
            return Ok(None); // A stale orphan index for an abandoned, subsequently reused slot.
        }
        let matches = match category {
            "quote" => receipt.reservation.quote_id == id,
            "job" => receipt.reservation.job_id.as_deref() == Some(id),
            "receipt" => receipt.reservation.reservation_id == id,
            _ => false,
        };
        if !matches {
            return Err(unavailable());
        }
        Ok(Some(receipt))
    }
}

impl Ledger {
    fn validate(&self) -> Result<u64, OpError> {
        if self.schema != 2
            || self.archived_count > MAX_INTEGER
            || self.archived_spent > MAX_INTEGER
            || self.policy.as_ref().is_some_and(|policy| policy.cap > MAX_INTEGER)
            || (self.frozen && self.freeze_reason.as_deref() != Some("actual_exceeded_estimate"))
            || (!self.frozen && self.freeze_reason.is_some())
        {
            return Err(unavailable());
        }
        if self.quotes.iter().any(|(id, quote)| id != &quote.quote_id || !quote.valid()) {
            return Err(unavailable());
        }
        let mut quote_ids = BTreeSet::new();
        let mut host_ids = BTreeSet::new();
        let mut job_ids = BTreeSet::new();
        let mut provider_ids = BTreeSet::new();
        let mut committed = self.archived_spent;
        for (id, reservation) in &self.reservations {
            let quote = self.quotes.get(&reservation.quote_id).ok_or_else(unavailable)?;
            if id != &reservation.reservation_id
                || !valid_id(id)
                || !quote_ids.insert(&reservation.quote_id)
                || !host_ids.insert(&reservation.host_call_id)
                || reservation.job_id.as_deref().is_some_and(|id| !job_ids.insert(id))
                || reservation.provider_job_id.as_deref().is_some_and(|id| !provider_ids.insert(id))
            {
                return Err(unavailable());
            }
            let amount = reservation.amount(quote)?;
            committed = committed.checked_add(amount).filter(|total| *total <= MAX_INTEGER).ok_or_else(unavailable)?;
        }
        if self.quotes.values().any(|quote| quote.consumed != quote_ids.contains(&quote.quote_id)) {
            return Err(unavailable());
        }
        Ok(committed)
    }
}

impl Quote {
    fn valid(&self) -> bool {
        let source_valid = match self.kind.as_str() {
            "sfx" => self.prompt.as_deref().is_some_and(|prompt| display_text(prompt, 4096)) && self.file.is_none() && !self.upload,
            "audio_to_midi" => self.prompt.is_none() && self.file.as_deref().is_some_and(basename) && self.upload,
            _ => false,
        };
        valid_id(&self.quote_id)
            && valid_id(&self.host_call_id)
            && self.state == "quoted"
            && source_valid
            && self.seconds.is_finite()
            && self.seconds > 0.0
            && self.seconds <= MAX_INTEGER as f64
            && self.estimate.credits <= self.estimate.max_credits
            && self.estimate.max_credits <= MAX_INTEGER
            && self.credit_unit == "mirelo_credits"
            && self.expires_at <= MAX_INTEGER
            && valid_digest(&self.request_sha256)
            && display_text(&self.summary, 1024)
    }
}

impl Reservation {
    fn amount(&self, quote: &Quote) -> Result<u64, OpError> {
        if !valid_id(&self.reservation_id)
            || !valid_id(&self.host_call_id)
            || self.quote_id != quote.quote_id
            || !quote.consumed
            || self.kind != quote.kind
            || self.request_sha256 != quote.request_sha256
            || self.reserved_credits != quote.estimate.max_credits
            || self.expires_at != quote.expires_at
            || self.created_at >= self.expires_at
            || self.job_id.as_deref().is_some_and(|id| !valid_id(id))
            || self.provider_job_id.as_deref().is_some_and(|id| !valid_id(id))
            || self.actual_credits.is_some_and(|actual| actual > MAX_INTEGER)
            || self.manually_released == Some(false)
            || !matches!(self.submission.as_str(), "unknown" | "submitted" | "not_submitted")
            || (self.provider_job_id.is_some() != (self.submission == "submitted"))
            || (self.provider_job_id.is_some() && self.job_id.is_none())
        {
            return Err(unavailable());
        }
        match self.settlement.as_str() {
            "pending" if self.actual_credits.is_none() => Ok(self.reserved_credits),
            "charged" if self.submission == "submitted" => self.actual_credits.ok_or_else(unavailable),
            "not_submitted" if self.submission == "not_submitted" && self.actual_credits.is_none() => Ok(0),
            "manual_release" if self.actual_credits.is_none() && matches!(self.submission.as_str(), "unknown" | "submitted") => Ok(0),
            _ => Err(unavailable()),
        }
    }

    fn recovery(&self) -> Recovery {
        Recovery {
            reservation_id: self.reservation_id.clone(),
            quote_id: self.quote_id.clone(),
            kind: self.kind.clone(),
            job_id: self.job_id.clone(),
            provider_job_id: self.provider_job_id.clone(),
            reserved_credits: self.reserved_credits,
            submission: self.submission.clone(),
            settlement: self.settlement.clone(),
            actual_credits: self.actual_credits,
        }
    }
}

fn decode_file<T: serde::de::DeserializeOwned>(file: File) -> Result<T, OpError> {
    let size = file.metadata().map_err(|_| unavailable())?.len();
    if size > MAX_BYTES {
        return Err(unavailable());
    }
    let mut bytes = Vec::with_capacity(size as usize);
    file.take(MAX_BYTES + 1).read_to_end(&mut bytes).map_err(|_| unavailable())?;
    if bytes.len() as u64 > MAX_BYTES {
        return Err(unavailable());
    }
    serde_json::from_slice(&bytes).map_err(|_| unavailable())
}

fn valid_kind(kind: &str) -> bool {
    matches!(kind, "sfx" | "audio_to_midi")
}

fn valid_id(id: &str) -> bool {
    !id.is_empty()
        && id.len() <= 256
        && id.as_bytes()[0].is_ascii_alphanumeric()
        && !id.contains("..")
        && id.bytes().all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-' | b'.' | b':'))
}

fn valid_digest(digest: &str) -> bool {
    digest.len() == 64 && digest.bytes().all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}

fn display_text(text: &str, limit: usize) -> bool {
    !text.is_empty() && text.len() <= limit && !text.chars().any(|ch| ch <= '\u{1f}' || ch == '\u{7f}')
}

fn basename(name: &str) -> bool {
    display_text(name, 255) && !matches!(name, "." | "..") && !name.contains(['/', '\\', ':'])
}

fn refusal(code: &'static str, reason: &str) -> OpError {
    OpError::Refused { code, reason: reason.into(), owner: None }
}

fn unavailable() -> OpError {
    refusal("ledger_unavailable", "The Mirelo credit ledger could not be read safely.")
}

fn data_dir(state_dir: &Path) -> Result<&Path, OpError> {
    if state_dir.file_name().is_none_or(|name| name != "state") {
        return Err(unavailable());
    }
    let kumi = state_dir.parent().ok_or_else(unavailable)?;
    if kumi.file_name().is_none_or(|name| name != "kumi") {
        return Err(unavailable());
    }
    kumi.parent().ok_or_else(unavailable)
}

fn present<'de, D, T>(deserializer: D) -> Result<Option<T>, D::Error>
where
    D: serde::Deserializer<'de>,
    T: Deserialize<'de>,
{
    T::deserialize(deserializer).map(Some)
}

fn required_nullable<'de, D, T>(deserializer: D) -> Result<Option<T>, D::Error>
where
    D: serde::Deserializer<'de>,
    T: Deserialize<'de>,
{
    Option::<T>::deserialize(deserializer)
}

// Serde's normal map deserializer silently overwrites duplicate keys. Audit IDs must be unique.
fn unique_map<'de, D, T>(deserializer: D) -> Result<BTreeMap<String, T>, D::Error>
where
    D: serde::Deserializer<'de>,
    T: Deserialize<'de>,
{
    struct Unique<T>(std::marker::PhantomData<T>);
    impl<'de, T: Deserialize<'de>> serde::de::Visitor<'de> for Unique<T> {
        type Value = BTreeMap<String, T>;
        fn expecting(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            formatter.write_str("an object with unique IDs")
        }
        fn visit_map<M: serde::de::MapAccess<'de>>(self, mut map: M) -> Result<Self::Value, M::Error> {
            let mut records = BTreeMap::new();
            while let Some((key, value)) = map.next_entry::<String, T>()? {
                if records.insert(key, value).is_some() {
                    return Err(serde::de::Error::custom("duplicate ledger ID"));
                }
            }
            Ok(records)
        }
    }
    deserializer.deserialize_map(Unique(std::marker::PhantomData))
}

// Pin the Unix directory so ledger and lock open in the same directory without following symlinks.
// On Windows OPEN_REPARSE_POINT and metadata checks reject reparse-point files.
struct Directory {
    #[cfg(unix)]
    file: File,
    #[cfg(windows)]
    path: PathBuf,
}

impl Directory {
    #[cfg(unix)]
    fn open(path: &Path) -> io::Result<Self> {
        use std::os::unix::fs::OpenOptionsExt;
        if path.components().any(|component| matches!(component, Component::ParentDir)) {
            return Err(io::ErrorKind::InvalidInput.into());
        }
        let file = OpenOptions::new().read(true).custom_flags(libc::O_DIRECTORY | libc::O_CLOEXEC | libc::O_NOFOLLOW).open(path)?;
        Ok(Self { file })
    }

    #[cfg(windows)]
    fn open(path: &Path) -> io::Result<Self> {
        use std::os::windows::fs::MetadataExt;
        use windows_sys::Win32::Storage::FileSystem::FILE_ATTRIBUTE_REPARSE_POINT;
        let mut ancestor = PathBuf::new();
        for component in path.components() {
            if matches!(component, Component::ParentDir) {
                return Err(io::ErrorKind::InvalidInput.into());
            }
            ancestor.push(component.as_os_str());
            // A disk/verbatim prefix alone is not rooted until the following RootDir.
            if matches!(component, Component::Prefix(_)) {
                continue;
            }
            let metadata = std::fs::symlink_metadata(&ancestor)?;
            if !metadata.is_dir() || metadata.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT != 0 {
                return Err(io::ErrorKind::InvalidInput.into());
            }
        }
        Ok(Self { path: path.to_owned() })
    }

    #[cfg(unix)]
    fn open_file(&self, name: &str, lock: bool) -> io::Result<File> {
        use std::os::fd::{AsRawFd, FromRawFd};
        use std::os::unix::fs::MetadataExt;
        let name = std::ffi::CString::new(name).map_err(|_| io::ErrorKind::InvalidInput)?;
        let flags =
            libc::O_CLOEXEC | libc::O_NOFOLLOW | libc::O_NONBLOCK | if lock { libc::O_RDWR | libc::O_CREAT } else { libc::O_RDONLY };
        // SAFETY: directory FD and name remain live; mode is supplied for O_CREAT.
        let fd = unsafe { libc::openat(self.file.as_raw_fd(), name.as_ptr(), flags, 0o600 as libc::c_uint) };
        if fd < 0 {
            return Err(io::Error::last_os_error());
        }
        // SAFETY: successful openat returned a new owned descriptor.
        let file = unsafe { File::from_raw_fd(fd) };
        let metadata = file.metadata()?;
        // Never chmod an existing path: refuse unsafe ownership, hardlinks or public permissions.
        if !metadata.is_file() || metadata.nlink() != 1 || metadata.mode() & 0o077 != 0 || metadata.uid() != unsafe { libc::geteuid() } {
            return Err(io::ErrorKind::PermissionDenied.into());
        }
        Ok(file)
    }

    #[cfg(windows)]
    fn open_file(&self, name: &str, lock: bool) -> io::Result<File> {
        use std::os::windows::fs::{MetadataExt, OpenOptionsExt};
        use windows_sys::Win32::Storage::FileSystem::{FILE_ATTRIBUTE_REPARSE_POINT, FILE_FLAG_OPEN_REPARSE_POINT};
        // Python configure creates this stable lock with an owner-only DACL. Rust's normal
        // OpenOptions cannot supply that DACL, so missing authority is refused, not recreated.
        let file = OpenOptions::new().read(true).write(lock).custom_flags(FILE_FLAG_OPEN_REPARSE_POINT).open(self.path.join(name))?;
        let metadata = file.metadata()?;
        if !metadata.is_file() || metadata.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT != 0 {
            return Err(io::ErrorKind::InvalidInput.into());
        }
        Ok(file)
    }
}

struct LockGuard<'a>(&'a File);

impl Drop for LockGuard<'_> {
    fn drop(&mut self) {
        #[cfg(unix)]
        {
            use std::os::fd::AsRawFd;
            // SAFETY: the borrowed file outlives this guard.
            unsafe { libc::flock(self.0.as_raw_fd(), libc::LOCK_UN) };
        }
        #[cfg(windows)]
        {
            use std::os::windows::io::AsRawHandle;
            use windows_sys::Win32::Storage::FileSystem::UnlockFileEx;
            use windows_sys::Win32::System::IO::OVERLAPPED;
            // SAFETY: unlock exactly byte zero while its handle remains live.
            unsafe {
                let mut overlapped: OVERLAPPED = std::mem::zeroed();
                UnlockFileEx(self.0.as_raw_handle() as _, 0, 1, 0, &mut overlapped);
            }
        }
    }
}

fn acquire(file: &File, timeout: Duration) -> Result<(), OpError> {
    let started = Instant::now();
    loop {
        match try_lock(file) {
            Ok(true) => return Ok(()),
            Ok(false) => {}
            Err(error) if error.kind() == io::ErrorKind::Interrupted => {}
            Err(_) => return Err(unavailable()),
        }
        let remaining = timeout.saturating_sub(started.elapsed());
        if remaining.is_zero() {
            return Err(unavailable());
        }
        std::thread::sleep(remaining.min(Duration::from_millis(10)));
    }
}

#[cfg(unix)]
fn try_lock(file: &File) -> io::Result<bool> {
    use std::os::fd::AsRawFd;
    // SAFETY: flock takes a nonblocking lock on a descriptor held open by this process.
    if unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) } == 0 {
        return Ok(true);
    }
    let error = io::Error::last_os_error();
    if error.kind() == io::ErrorKind::WouldBlock {
        Ok(false)
    } else {
        Err(error)
    }
}

#[cfg(windows)]
fn try_lock(file: &File) -> io::Result<bool> {
    use std::os::windows::io::AsRawHandle;
    use windows_sys::Win32::Foundation::ERROR_LOCK_VIOLATION;
    use windows_sys::Win32::Storage::FileSystem::{LockFileEx, LOCKFILE_EXCLUSIVE_LOCK, LOCKFILE_FAIL_IMMEDIATELY};
    use windows_sys::Win32::System::IO::OVERLAPPED;
    // SAFETY: the handle is live and the zeroed OVERLAPPED selects byte zero, matching msvcrt.locking.
    let locked = unsafe {
        let mut overlapped: OVERLAPPED = std::mem::zeroed();
        LockFileEx(file.as_raw_handle() as _, LOCKFILE_EXCLUSIVE_LOCK | LOCKFILE_FAIL_IMMEDIATELY, 0, 1, 0, &mut overlapped) != 0
    };
    if locked {
        return Ok(true);
    }
    let error = io::Error::last_os_error();
    if error.raw_os_error() == Some(ERROR_LOCK_VIOLATION as i32) {
        Ok(false)
    } else {
        Err(error)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::{json, Value};

    const DIGEST: &str = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";

    fn record() -> Value {
        json!({
            "schema": 2, "archived_count": 0, "archived_spent": 0,
            "policy": {"acknowledged": true, "enabled": {"sfx": true, "audio_to_midi": false}, "cap": 30},
            "frozen": false, "freeze_reason": null,
            "quotes": {"q1": {
                "state": "quoted", "quote_id": "q1", "kind": "sfx", "prompt": "metal impact", "file": null,
                "seconds": 5, "estimate": {"credits": 15, "max_credits": 15}, "credit_unit": "mirelo_credits",
                "upload": false, "expires_at": 300000, "request_sha256": DIGEST, "summary": "One sound effect",
                "host_call_id": "quote-call", "consumed": true
            }},
            "reservations": {"r1": {
                "reservation_id": "r1", "quote_id": "q1", "request_sha256": DIGEST, "host_call_id": "create-call",
                "kind": "sfx", "reserved_credits": 15, "expires_at": 300000, "created_at": 1000,
                "job_id": null, "provider_job_id": null, "submission": "unknown", "settlement": "pending", "actual_credits": null
            }}
        })
    }

    fn claim() -> CreateClaim<'static> {
        CreateClaim {
            quote_id: "q1",
            request_sha256: DIGEST,
            kind: "sfx",
            reservation_id: "r1",
            reserved_credits: 15,
            host_call_id: "create-call",
            now_ms: 2000,
            expected_job_id: None,
        }
    }

    struct Fixture {
        _temp: tempfile::TempDir,
        data: PathBuf,
        state: PathBuf,
    }

    impl Fixture {
        fn new() -> Self {
            let temp = tempfile::tempdir().unwrap();
            let data = temp.path().canonicalize().unwrap();
            let state = data.join("kumi/state");
            std::fs::create_dir_all(&state).unwrap();
            #[cfg(windows)]
            File::create(data.join("ledger.lock")).unwrap();
            Self { _temp: temp, data, state }
        }

        fn put(&self, value: &Value) {
            let path = self.data.join("ledger.json");
            let mut options = OpenOptions::new();
            options.write(true).create(true).truncate(true);
            #[cfg(unix)]
            {
                use std::os::unix::fs::OpenOptionsExt;
                options.mode(0o600);
            }
            use std::io::Write;
            options.open(path).unwrap().write_all(&serde_json::to_vec(value).unwrap()).unwrap();
        }

        fn authority(&self) -> Authority {
            Authority::read(&self.state).unwrap()
        }
    }

    #[cfg(windows)]
    #[test]
    fn canonical_windows_state_path_reads_existing_authority_without_creating_a_lock() {
        let fixture = Fixture::new();
        assert!(fixture.data.is_absolute());
        assert!(matches!(fixture.data.components().next(), Some(Component::Prefix(_))));
        fixture.put(&record());
        assert!(fixture.authority().approve(&claim()).is_ok());
        let lock = fixture.data.join("ledger.lock");
        std::fs::remove_file(&lock).unwrap();
        assert_eq!(code(Authority::read(&fixture.state)), "ledger_unavailable");
        assert!(!lock.exists(), "the native reader must not create a default-ACL lock");
    }

    fn code<T>(result: Result<T, OpError>) -> &'static str {
        match result {
            Err(OpError::Refused { code, reason, owner }) => {
                assert!(owner.is_none());
                assert!(!reason.contains("sentinel"));
                assert!(!reason.contains("ledger.json"));
                code
            }
            Err(error) => panic!("unexpected error: {error:?}"),
            Ok(_) => panic!("unexpected authority"),
        }
    }

    #[test]
    fn missing_policy_defaults_and_independent_task_switches() {
        let fixture = Fixture::new();
        assert_eq!(code(fixture.authority().policy("sfx")), "not_enabled");
        let mut value = record();
        value["policy"]["acknowledged"] = json!(false);
        fixture.put(&value);
        assert_eq!(code(fixture.authority().approve(&claim())), "not_submitted");
        value["policy"]["acknowledged"] = json!(true);
        fixture.put(&value);
        assert_eq!(code(fixture.authority().policy("audio_to_midi")), "not_enabled");
        assert!(fixture.authority().policy("sfx").is_ok());
        value.as_object_mut().unwrap().remove("policy");
        fixture.put(&value);
        assert_eq!(code(fixture.authority().approve(&claim())), "not_submitted");
    }

    #[test]
    fn approved_quote_uses_real_pending_reservation_and_create_call() {
        let fixture = Fixture::new();
        fixture.put(&record());
        let authority = fixture.authority();
        assert_eq!(authority.approve(&claim()).unwrap().reserved_credits, 15);
        let mut worker = claim();
        worker.expected_job_id = Some("new-native-job");
        assert!(authority.approve(&worker).is_ok(), "the worker may recheck before Python binds its ID");
        let mut forged = claim();
        forged.reservation_id = "forged";
        assert_eq!(code(authority.approve(&forged)), "not_approved");
        for settlement in ["manual_release", "not_submitted"] {
            let mut value = record();
            value["reservations"]["r1"]["settlement"] = json!(settlement);
            if settlement == "not_submitted" {
                value["reservations"]["r1"]["submission"] = json!("not_submitted");
            }
            fixture.put(&value);
            assert_eq!(code(fixture.authority().approve(&claim())), "not_approved");
        }
    }

    #[test]
    fn amount_digest_kind_expiry_and_host_are_bound() {
        let fixture = Fixture::new();
        fixture.put(&record());
        let authority = fixture.authority();
        let mut changed = claim();
        changed.reserved_credits = 14;
        assert_eq!(code(authority.approve(&changed)), "stale");
        changed = claim();
        changed.request_sha256 = "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb";
        assert_eq!(code(authority.approve(&changed)), "stale");
        changed = claim();
        changed.host_call_id = "quote-call";
        assert_eq!(code(authority.approve(&changed)), "stale");
        changed = claim();
        changed.now_ms = 300000;
        assert_eq!(code(authority.approve(&changed)), "not_submitted");
        changed = claim();
        changed.now_ms = 999;
        assert_eq!(code(authority.approve(&changed)), "not_submitted");
        let mut value = record();
        value["policy"]["enabled"]["audio_to_midi"] = json!(true);
        fixture.put(&value);
        changed = claim();
        changed.kind = "audio_to_midi";
        assert_eq!(code(fixture.authority().approve(&changed)), "stale");
    }

    #[test]
    fn current_cap_counts_other_unknowns_and_final_actual_charges() {
        let fixture = Fixture::new();
        for settlement in ["pending", "charged"] {
            let mut value = record();
            value["quotes"]["q2"] = value["quotes"]["q1"].clone();
            value["quotes"]["q2"]["quote_id"] = json!("q2");
            value["reservations"]["r2"] = value["reservations"]["r1"].clone();
            value["reservations"]["r2"]["reservation_id"] = json!("r2");
            value["reservations"]["r2"]["quote_id"] = json!("q2");
            value["reservations"]["r2"]["host_call_id"] = json!("create-two");
            value["reservations"]["r2"]["settlement"] = json!(settlement);
            if settlement == "charged" {
                value["reservations"]["r2"]["submission"] = json!("submitted");
                value["reservations"]["r2"]["job_id"] = json!("native-two");
                value["reservations"]["r2"]["provider_job_id"] = json!("vendor-two");
                value["reservations"]["r2"]["actual_credits"] = json!(15);
            }
            fixture.put(&value);
            assert!(fixture.authority().approve(&claim()).is_ok());
            value["policy"]["cap"] = json!(29);
            fixture.put(&value);
            assert_eq!(code(fixture.authority().approve(&claim())), "not_submitted");
        }
        let mut value = record();
        value["policy"]["cap"] = json!(0);
        value["quotes"]["q1"]["estimate"] = json!({"credits": 0, "max_credits": 0});
        value["reservations"]["r1"]["reserved_credits"] = json!(0);
        fixture.put(&value);
        let mut zero = claim();
        zero.reserved_credits = 0;
        assert!(fixture.authority().policy("sfx").is_ok());
        assert_eq!(code(fixture.authority().approve(&zero)), "not_submitted");
        value["policy"]["cap"] = json!(1000);
        value["frozen"] = json!(true);
        value["freeze_reason"] = json!("actual_exceeded_estimate");
        fixture.put(&value);
        assert_eq!(code(fixture.authority().approve(&zero)), "not_submitted");
    }

    #[test]
    fn binding_precedes_every_refundable_policy_refusal() {
        let fixture = Fixture::new();
        let mut value = record();
        value["policy"]["acknowledged"] = json!(false);
        value["policy"]["cap"] = json!(0);
        value["frozen"] = json!(true);
        value["freeze_reason"] = json!("actual_exceeded_estimate");
        fixture.put(&value);
        let authority = fixture.authority();
        assert!(authority.prove_own(&claim()).is_ok());
        assert_eq!(code(authority.approve(&claim())), "not_submitted");
        let mut forged = claim();
        forged.host_call_id = "foreign-call";
        assert_eq!(code(authority.approve(&forged)), "stale");
        forged = claim();
        forged.reservation_id = "foreign-reservation";
        assert_eq!(code(authority.approve(&forged)), "not_approved");
        value["reservations"]["r1"]["job_id"] = json!("foreign-job");
        fixture.put(&value);
        assert_eq!(code(fixture.authority().approve(&claim())), "quote_consumed");
        value["reservations"]["r1"]["submission"] = json!("submitted");
        value["reservations"]["r1"]["provider_job_id"] = json!("vendor-job");
        fixture.put(&value);
        let mut worker = claim();
        worker.expected_job_id = Some("foreign-job");
        assert_eq!(code(fixture.authority().approve(&worker)), "quote_consumed");
    }

    #[test]
    fn recovery_survives_restart_and_off_but_never_approves_a_second_post() {
        let fixture = Fixture::new();
        for submission in ["unknown", "not_submitted", "submitted"] {
            let mut value = record();
            value["reservations"]["r1"]["job_id"] = json!("native-job");
            value["reservations"]["r1"]["submission"] = json!(submission);
            if submission == "submitted" {
                value["reservations"]["r1"]["provider_job_id"] = json!("vendor-job");
            }
            fixture.put(&value);
            assert_eq!(code(fixture.authority().approve(&claim())), "quote_consumed");
            let mut worker = claim();
            worker.expected_job_id = Some("native-job");
            if submission != "submitted" {
                assert!(fixture.authority().approve(&worker).is_ok());
                worker.expected_job_id = Some("different-job");
                assert_eq!(code(fixture.authority().approve(&worker)), "quote_consumed");
            } else {
                assert_eq!(code(fixture.authority().approve(&worker)), "quote_consumed");
            }
            value["policy"]["enabled"]["sfx"] = json!(false);
            fixture.put(&value);
            let saved = fixture.authority().recovery("r1").unwrap().unwrap();
            assert_eq!(saved.job_id.as_deref(), Some("native-job"));
            assert_eq!(saved.submission, submission);
            assert_eq!(saved.actual_credits, None);
            assert!(fixture.authority().recovery("missing").unwrap().is_none());
        }
    }

    #[test]
    fn pending_preflight_receipt_allows_only_the_owning_worker() {
        let fixture = Fixture::new();
        let mut value = record();
        value["reservations"]["r1"]["submission"] = json!("not_submitted");
        fixture.put(&value);
        assert_eq!(code(fixture.authority().approve(&claim())), "quote_consumed");
        let mut worker = claim();
        worker.expected_job_id = Some("native-job");
        assert!(fixture.authority().approve(&worker).is_ok(), "binding can lag the worker");
        value["reservations"]["r1"]["job_id"] = json!("native-job");
        fixture.put(&value);
        assert!(fixture.authority().approve(&worker).is_ok());
        value["reservations"]["r1"]["settlement"] = json!("not_submitted");
        fixture.put(&value);
        assert_eq!(code(fixture.authority().approve(&worker)), "not_approved");
        value["reservations"]["r1"]["settlement"] = json!("manual_release");
        value["reservations"]["r1"]["submission"] = json!("unknown");
        fixture.put(&value);
        assert_eq!(code(fixture.authority().approve(&worker)), "not_approved");
    }

    #[test]
    fn malformed_records_and_private_payloads_fail_closed_without_echo() {
        let fixture = Fixture::new();
        let mutations = [
            ("/schema", json!(1)),
            ("/policy/cap", json!(true)),
            ("/policy/cap", json!(1.0)),
            ("/policy/cap", json!(MAX_INTEGER + 1)),
            ("/quotes/q1/file", json!("/sentinel/private.wav")),
            ("/quotes/q1/request_sha256", json!("sentinel")),
            ("/quotes/q1/consumed", json!(false)),
            ("/reservations/r1/reserved_credits", json!(14)),
            ("/reservations/r1/expires_at", json!(299999)),
            ("/reservations/r1/actual_credits", json!(0)),
            ("/reservations/r1/submission", json!("sentinel")),
            ("/reservations/r1/quote_id", json!("missing")),
            ("/quotes/q1/seconds", json!(0)),
        ];
        for (pointer, replacement) in mutations {
            let mut value = record();
            *value.pointer_mut(pointer).unwrap() = replacement;
            fixture.put(&value);
            assert_eq!(code(Authority::read(&fixture.state)), "ledger_unavailable");
        }
        let mut value = record();
        value["quotes"]["q1"]["_mirelo"] = json!({"api_key": "sentinel-secret"});
        fixture.put(&value);
        assert_eq!(code(Authority::read(&fixture.state)), "ledger_unavailable");
    }

    #[test]
    fn duplicate_audit_ids_and_oversized_or_truncated_json_are_refused() {
        let fixture = Fixture::new();
        fixture.put(&record());
        let path = fixture.data.join("ledger.json");
        let original = std::fs::read_to_string(&path).unwrap();
        let duplicate = original.replacen("\"quotes\":{", &format!("\"quotes\":{{\"q1\":{},", record()["quotes"]["q1"]), 1);
        std::fs::write(&path, duplicate).unwrap();
        assert_eq!(code(Authority::read(&fixture.state)), "ledger_unavailable");
        std::fs::write(&path, b"{sentinel").unwrap();
        assert_eq!(code(Authority::read(&fixture.state)), "ledger_unavailable");
        let file = OpenOptions::new().write(true).open(path).unwrap();
        file.set_len(MAX_BYTES + 1).unwrap();
        assert_eq!(code(Authority::read(&fixture.state)), "ledger_unavailable");
    }

    #[test]
    fn lock_wait_is_bounded_and_released_after_read_errors() {
        let fixture = Fixture::new();
        fixture.put(&record());
        let directory = Directory::open(&fixture.data).unwrap();
        let held = directory.open_file("ledger.lock", true).unwrap();
        assert!(try_lock(&held).unwrap());
        let start = Instant::now();
        assert_eq!(code(Authority::read_with_timeout(&fixture.state, Duration::from_millis(30))), "ledger_unavailable");
        assert!(start.elapsed() < Duration::from_secs(1));
        drop(held);
        fixture.put(&json!({"sentinel": true}));
        assert_eq!(code(Authority::read(&fixture.state)), "ledger_unavailable");
        fixture.put(&record());
        assert!(fixture.authority().approve(&claim()).is_ok());
    }

    const PYTHON_FIXTURE: &str = r#"
import importlib.util, json, sys
spec = importlib.util.spec_from_file_location("ledger", sys.argv[1])
ledger = importlib.util.module_from_spec(spec)
spec.loader.exec_module(ledger)
ledger._now = lambda: 1000
store = ledger.Ledger(sys.argv[2], credit_cap=30)
if sys.argv[3] == "hold":
    with store._locked():
        print("locked", flush=True)
        sys.stdin.readline()
    sys.exit(0)
store.configure(acknowledged=True, enabled={"sfx":True, "audio_to_midi":False})
def reserve(name, amount):
    quote = {"state":"quoted", "quote_id":name, "kind":"sfx", "prompt":"metal impact",
             "file":None, "seconds":0.1, "estimate":{"credits":amount, "max_credits":amount},
             "credit_unit":"mirelo_credits", "upload":False, "expires_at":300000,
             "request_sha256":"a"*64, "summary":"One sound effect"}
    store.record_quote(quote, "quote-"+name)
    reservation = store.reserve(name, "a"*64, amount, host_call_id="create-"+name)
    return quote, reservation["reservation_id"]
def receipt(quote, job, actual=None, state="running"):
    return {"job_id":job, "kind":"generate", "state":state, "result":{
        "quote_id":quote["quote_id"], "kind":"sfx", "provider":"mirelo", "provider_job_id":"vendor-"+job,
        "submission":"submitted", "credit_unit":"mirelo_credits",
        "credits":{"estimate":quote["estimate"]["credits"], "reserved":quote["estimate"]["max_credits"], "actual":actual}}}
def settle(quote, reservation, job, actual):
    return store.reconcile(reservation, receipt(quote, job, actual, "done"))
if sys.argv[3] == "released":
    quote, rid = reserve("q1", 15)
    store.release(rid)
    rows = store.reservations()
    assert len(rows) == 1 and rows[0]["settlement"] == "manual_release" and rows[0]["job_id"] is None
    wrong = receipt(quote, "j-released")
    wrong["result"]["credits"]["reserved"] = 14
    try:
        store.bind(rid, "j-released", receipt=wrong)
        raise AssertionError("mismatched receipt accepted")
    except ledger.LedgerError as error:
        assert error.code == "stale"
    assert store.reservations() == rows
    bound = store.bind(rid, "j-released", receipt=receipt(quote, "j-released"))
    assert bound["reserved"] == 0 and bound["spent"] == 0 and not bound["frozen"]
    assert store.reconcile(rid, receipt(quote, "j-released")) == bound
    print(json.dumps({"reservation":rid}), flush=True)
    sys.exit(0)
if sys.argv[3] in {"late-charge", "cleared"}:
    row = store.find_job("j-released")
    quote, rid = store.quote(row["quote_id"]), row["reservation_id"]
    if sys.argv[3] == "cleared":
        store.unfreeze()
    first = settle(quote, rid, "j-released", 12)
    assert settle(quote, rid, "j-released", 12) == first
    assert store.bind(rid, "j-released", receipt=receipt(quote, "j-released", 12, "done")) == first
    print(json.dumps(first), flush=True)
    sys.exit(0)
charged, charged_id = reserve("q-charged", 15)
settle(charged, charged_id, "j-charged", 12)
pending, pending_id = reserve("q1", 15)
manual, manual_id = reserve("q-manual", 1)
store.release(manual_id)
late, late_id = reserve("q-late", 1)
store.release(late_id)
settle(late, late_id, "j-late", 1)
assert store.snapshot()["frozen"]
store.unfreeze()  # Keep the pending-approval fixture independent of late-charge freeze.
print(json.dumps({"charged":charged_id, "pending":pending_id, "manual":manual_id, "late":late_id,
                  "pending_quote":pending, "charged_quote":charged}), flush=True)
"#;

    fn python(fixture: &Fixture, mode: &str) -> std::process::Command {
        let mut command = std::process::Command::new(if cfg!(windows) { "python" } else { "python3" });
        command
            .args(["-I", "-S", "-c", PYTHON_FIXTURE])
            .arg(Path::new(env!("CARGO_MANIFEST_DIR")).join("../../hermes-plugin/ledger.py"))
            .arg(&fixture.data)
            .arg(mode)
            .current_dir(&fixture.data)
            .env("HOME", &fixture.data)
            .env("USERPROFILE", &fixture.data);
        command
    }

    fn python_written(fixture: &Fixture) -> Value {
        let output = python(fixture, "write").output().expect("isolated stdlib Python is required");
        assert!(output.status.success(), "{}", String::from_utf8_lossy(&output.stderr));
        serde_json::from_slice(&output.stdout).unwrap()
    }

    #[test]
    fn python_written_active_and_archived_receipts_are_native_authority() {
        let fixture = Fixture::new();
        let ids = python_written(&fixture);
        let authority = fixture.authority();
        assert_eq!(authority.committed, 28);
        assert_eq!(authority.ledger.archived_count, 2);
        assert_eq!(authority.ledger.archived_spent, 13);
        let mut own = claim();
        own.reservation_id = ids["pending"].as_str().unwrap();
        own.host_call_id = "create-q1";
        assert!(authority.approve(&own).is_ok());
        assert_eq!(authority.quote("q1").unwrap().unwrap(), ids["pending_quote"]);
        assert_eq!(authority.quote("q-charged").unwrap().unwrap(), ids["charged_quote"]);
        let charged = authority.for_job("j-charged").unwrap().unwrap();
        assert_eq!(charged.reservation_id, ids["charged"].as_str().unwrap());
        assert_eq!(charged.actual_credits, Some(12));
        assert_eq!(authority.recovery(&charged.reservation_id).unwrap().unwrap(), charged);
        let late = authority.for_job("j-late").unwrap().unwrap();
        assert_eq!(late.actual_credits, Some(1));
        let archived = authority.history("receipt", &late.reservation_id).unwrap().unwrap();
        assert_eq!(archived.reservation.manually_released, Some(true));
        assert_eq!(authority.unsettled().count(), 2);
        own.quote_id = "q-charged";
        own.reservation_id = ids["charged"].as_str().unwrap();
        own.host_call_id = "create-q-charged";
        assert_ne!(code(authority.approve(&own)), "not_submitted");
        let mut manifest: Value = serde_json::from_slice(&std::fs::read(fixture.data.join("ledger.json")).unwrap()).unwrap();
        manifest["policy"]["cap"] = json!(27);
        fixture.put(&manifest);
        own.quote_id = "q1";
        own.reservation_id = ids["pending"].as_str().unwrap();
        own.host_call_id = "create-q1";
        assert_eq!(code(fixture.authority().approve(&own)), "not_submitted");
    }

    #[test]
    fn python_released_binding_holds_zero_then_books_late_charge_once() {
        let fixture = Fixture::new();
        let run = |mode| {
            let output = python(&fixture, mode).output().expect("isolated stdlib Python is required");
            assert!(output.status.success(), "{}", String::from_utf8_lossy(&output.stderr));
            serde_json::from_slice::<Value>(&output.stdout).unwrap()
        };
        let ids = run("released");
        let authority = fixture.authority();
        assert_eq!(authority.committed, 0);
        let released = authority.for_job("j-released").unwrap().unwrap();
        assert_eq!(released.reservation_id, ids["reservation"].as_str().unwrap());
        assert_eq!(released.submission, "submitted");
        assert_eq!(released.settlement, "manual_release");
        assert_eq!(released.provider_job_id.as_deref(), Some("vendor-j-released"));
        assert_eq!(released.reserved_credits, 15);
        assert_eq!(released.actual_credits, None);
        assert_eq!(
            authority.unsettled().collect::<Vec<_>>(),
            vec![Unsettled { quote_id: "q1", kind: "sfx", job_id: Some("j-released"), reserved_credits: 15, settlement: "manual_release" }]
        );
        let mut own = claim();
        own.reservation_id = &released.reservation_id;
        own.host_call_id = "create-q1";
        own.expected_job_id = Some("j-released");
        assert_eq!(code(authority.approve(&own)), "not_approved");
        let charged = run("late-charge");
        assert_eq!(charged["frozen"], true);
        for (mode, frozen) in [(None, true), (Some("cleared"), false)] {
            if let Some(mode) = mode {
                assert_eq!(run(mode)["frozen"], false);
            }
            let authority = fixture.authority();
            assert_eq!(authority.committed, 12);
            assert_eq!(authority.ledger.archived_count, 1);
            assert_eq!(authority.ledger.archived_spent, 12);
            assert_eq!(authority.ledger.frozen, frozen);
            assert_eq!(authority.unsettled().count(), 0);
            let row = authority.for_job("j-released").unwrap().unwrap();
            assert_eq!(row.actual_credits, Some(12));
            assert_eq!(row.reserved_credits, 15);
            assert_eq!(row.settlement, "charged");
            assert_eq!(authority.history("receipt", &row.reservation_id).unwrap().unwrap().reservation.manually_released, Some(true));
        }
    }

    #[test]
    fn released_submitted_rows_require_consistent_identity_and_unknown_actual() {
        let fixture = Fixture::new();
        let mut value = record();
        let row = &mut value["reservations"]["r1"];
        row["settlement"] = json!("manual_release");
        row["submission"] = json!("submitted");
        row["job_id"] = json!("j-one");
        row["provider_job_id"] = json!("vendor-one");
        fixture.put(&value);
        assert_eq!(fixture.authority().committed, 0);
        for (field, bad) in
            [("job_id", Value::Null), ("provider_job_id", Value::Null), ("submission", json!("unknown")), ("actual_credits", json!(0))]
        {
            let mut invalid = value.clone();
            invalid["reservations"]["r1"][field] = bad;
            fixture.put(&invalid);
            assert_eq!(code(Authority::read(&fixture.state)), "ledger_unavailable");
        }
    }

    #[test]
    fn python_holds_shared_os_lock_native_times_out_then_succeeds() {
        use std::io::{BufRead, Write};
        use std::process::{Child, Stdio};
        struct ChildGuard(Child);
        impl Drop for ChildGuard {
            fn drop(&mut self) {
                let _ = self.0.kill();
                let _ = self.0.wait();
            }
        }
        let fixture = Fixture::new();
        python_written(&fixture);
        let mut child = ChildGuard(
            python(&fixture, "hold")
                .stdin(Stdio::piped())
                .stdout(Stdio::piped())
                .stderr(Stdio::piped())
                .spawn()
                .expect("isolated stdlib Python is required"),
        );
        let stdout = child.0.stdout.take().unwrap();
        let (send, receive) = std::sync::mpsc::channel();
        let reader = std::thread::spawn(move || {
            let mut line = String::new();
            let result = std::io::BufReader::new(stdout).read_line(&mut line).map(|_| line);
            let _ = send.send(result);
        });
        assert_eq!(receive.recv_timeout(Duration::from_secs(10)).unwrap().unwrap().trim(), "locked");
        reader.join().unwrap();
        let started = Instant::now();
        assert_eq!(code(Authority::read_with_timeout(&fixture.state, Duration::from_millis(100))), "ledger_unavailable");
        assert!(started.elapsed() >= Duration::from_millis(75));
        assert!(started.elapsed() < Duration::from_secs(3));
        child.0.stdin.take().unwrap().write_all(b"release\n").unwrap();
        assert!(child.0.wait().unwrap().success());
        assert_eq!(fixture.authority().committed, 28);
    }

    #[test]
    fn archive_damage_is_reported_on_demand_without_scanning_or_resetting_totals() {
        let fixture = Fixture::new();
        python_written(&fixture);
        std::fs::write(fixture.data.join(".ledger-history-0000000000000000.json"), b"corrupt").unwrap();
        let authority = fixture.authority();
        assert_eq!(authority.committed, 28);
        assert!(authority.quote("q1").unwrap().is_some());
        assert_eq!(code(authority.quote("q-charged")), "ledger_unavailable");
        assert_eq!(code(authority.for_job("j-charged")), "ledger_unavailable");
        std::fs::remove_file(fixture.data.join("ledger.json")).unwrap();
        assert_eq!(code(Authority::read(&fixture.state)), "ledger_unavailable");
    }

    #[test]
    fn archived_indexes_require_both_snapshot_commit_and_canonical_identity() {
        let fixture = Fixture::new();
        let ids = python_written(&fixture);
        let authority = fixture.authority();
        let path = fixture.data.join(format!(".ledger-quote-{:x}.json", Sha256::digest(b"q-charged")));
        let mut index = json!({"schema":1, "sequence":2, "reservation_id":ids["charged"]});
        std::fs::write(&path, serde_json::to_vec(&index).unwrap()).unwrap();
        assert!(authority.quote("q-charged").unwrap().is_none());
        index["sequence"] = json!(1); // committed slot belongs to the late-charge reservation
        std::fs::write(&path, serde_json::to_vec(&index).unwrap()).unwrap();
        assert!(authority.quote("q-charged").unwrap().is_none());
        assert_eq!(authority.committed, 28);
    }

    #[test]
    fn bom_and_utf16_are_refused_like_python() {
        let fixture = Fixture::new();
        fixture.put(&record());
        let path = fixture.data.join("ledger.json");
        let utf8 = serde_json::to_vec(&record()).unwrap();
        let mut bom = vec![0xef, 0xbb, 0xbf];
        bom.extend_from_slice(&utf8);
        std::fs::write(&path, bom).unwrap();
        assert_eq!(code(Authority::read(&fixture.state)), "ledger_unavailable");
        let mut utf16 = vec![0xff, 0xfe];
        for unit in String::from_utf8(utf8).unwrap().encode_utf16() {
            utf16.extend_from_slice(&unit.to_le_bytes());
        }
        std::fs::write(path, utf16).unwrap();
        assert_eq!(code(Authority::read(&fixture.state)), "ledger_unavailable");
    }

    #[test]
    fn midi_source_is_a_basename_and_nullable_fields_are_required() {
        let fixture = Fixture::new();
        let mut value = record();
        value["policy"]["enabled"]["audio_to_midi"] = json!(true);
        value["quotes"]["q1"]["kind"] = json!("audio_to_midi");
        value["quotes"]["q1"]["prompt"] = Value::Null;
        value["quotes"]["q1"]["file"] = json!("My audio [take 2].wav");
        value["quotes"]["q1"]["upload"] = json!(true);
        value["reservations"]["r1"]["kind"] = json!("audio_to_midi");
        fixture.put(&value);
        let mut midi = claim();
        midi.kind = "audio_to_midi";
        assert!(fixture.authority().approve(&midi).is_ok());
        for name in ["../sentinel.wav", "C:\\sentinel.wav", "https://sentinel/file", ".."] {
            value["quotes"]["q1"]["file"] = json!(name);
            fixture.put(&value);
            assert_eq!(code(Authority::read(&fixture.state)), "ledger_unavailable");
        }
        for key in ["job_id", "provider_job_id", "actual_credits"] {
            let mut value = record();
            value["reservations"]["r1"].as_object_mut().unwrap().remove(key);
            fixture.put(&value);
            assert_eq!(code(Authority::read(&fixture.state)), "ledger_unavailable");
        }
        assert_eq!(code(Authority::read(&fixture.data)), "ledger_unavailable");
        assert_eq!(code(Authority::read(&fixture.data.join("other/state"))), "ledger_unavailable");
    }

    #[test]
    fn overflow_and_duplicate_create_identities_fail_closed() {
        let fixture = Fixture::new();
        let mut value = record();
        value["quotes"]["q2"] = value["quotes"]["q1"].clone();
        value["quotes"]["q2"]["quote_id"] = json!("q2");
        value["reservations"]["r2"] = value["reservations"]["r1"].clone();
        value["reservations"]["r2"]["reservation_id"] = json!("r2");
        value["reservations"]["r2"]["quote_id"] = json!("q2");
        fixture.put(&value);
        assert_eq!(code(Authority::read(&fixture.state)), "ledger_unavailable", "CREATE host IDs are unique");
        value["reservations"]["r2"]["host_call_id"] = json!("create-two");
        value["quotes"]["q2"]["estimate"] = json!({"credits": MAX_INTEGER, "max_credits": MAX_INTEGER});
        value["reservations"]["r2"]["reserved_credits"] = json!(MAX_INTEGER);
        fixture.put(&value);
        assert_eq!(code(Authority::read(&fixture.state)), "ledger_unavailable", "totals cannot lose JSON integer precision");
    }

    #[cfg(unix)]
    #[test]
    fn symlinks_non_regular_files_and_public_permissions_are_refused() {
        use std::os::unix::fs::{symlink, PermissionsExt};
        let fixture = Fixture::new();
        let target = fixture.data.join("sentinel-target");
        std::fs::write(&target, b"sentinel-private").unwrap();
        let original_mode = std::fs::metadata(&target).unwrap().permissions().mode();
        for name in ["ledger.lock", "ledger.json"] {
            let path = fixture.data.join(name);
            symlink(&target, &path).unwrap();
            assert_eq!(code(Authority::read(&fixture.state)), "ledger_unavailable");
            assert_eq!(std::fs::metadata(&target).unwrap().permissions().mode(), original_mode);
            std::fs::remove_file(path).unwrap();
        }
        let path = fixture.data.join("ledger.json");
        std::fs::create_dir(&path).unwrap();
        assert_eq!(code(Authority::read(&fixture.state)), "ledger_unavailable");
        std::fs::remove_dir(&path).unwrap();
        fixture.put(&record());
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o644)).unwrap();
        assert_eq!(code(Authority::read(&fixture.state)), "ledger_unavailable");
        assert_eq!(std::fs::metadata(&path).unwrap().permissions().mode() & 0o777, 0o644);
    }
}
