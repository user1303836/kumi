//! The host's journal of changes (M3 contract §3a): one record for each change previewed, applied and maybe undone,
//! so Kumi's undo sends the DAW what it needs (the values before, what a delete removed) and a call sent twice changes
//! the DAW once. The bridge applies and restores; the journal decides what may be asked of it.
use serde_json::{json, Map, Value};
use std::collections::HashMap;
use std::sync::Mutex;

/// The most records kept, and the most JSON they hold: past either, the oldest go (previewed first, then undone, then
/// applied, whose undo then answers `unknown`); a change being applied or undone never does.
pub const MAX_RECORDS: usize = 512;
pub const MAX_BYTES: usize = 64 * 1024 * 1024;
/// The most transactions one `change_release` names.
pub const MAX_RELEASE: usize = 64;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum State {
    Previewed,
    Applying,
    Applied,
    /// The apply was sent but its answer never came: the DAW may or may not have changed.
    Uncertain,
    Undoing,
    Undone,
}

impl State {
    pub fn word(self) -> &'static str {
        match self {
            State::Previewed => "previewed",
            State::Applying => "applying",
            State::Applied => "applied",
            State::Uncertain => "uncertain",
            State::Undoing => "undoing",
            State::Undone => "undone",
        }
    }
}

/// One change, as the journal keeps it.
#[derive(Debug, Clone)]
pub struct Record {
    pub id: String,
    pub kind: String,
    pub input: Value,
    /// The DAW the change was previewed in (its pid and resource folder): its apply and undo go there, or are refused when
    /// it isn't running, and never reach another instance.
    pub instance: crate::host::Pick,
    pub epoch: Option<String>,
    /// The layout the DAW's refs are at as the DAW last answered (the preview's, then the apply's): `target` is in it.
    pub layout: Option<u64>,
    /// The layout Kumi's refs in `input` were made in (Renoise): the apply sends `input` at it, so the DAW carries them
    /// through the tracks moved since, as it did for the preview.
    pub input_layout: Option<u64>,
    pub target: Value,
    pub prior: Value,
    pub fence: Value,
    pub display: Value,
    pub confirmation: String,
    /// `k` and four base-36 characters, in the change's DAW step description, so the DAW's own undo of it is known.
    pub nonce: String,
    pub desc: Option<String>,
    /// The plan's DAW step the change was made in (its label, with the plan's nonce): the DAW's undo names that, not
    /// the change's own description.
    pub step_desc: Option<String>,
    /// The DAW step Kumi's undo of it made ("Kumi: undo …"): the DAW's own undo of that brings the change back.
    pub undo_desc: Option<String>,
    /// The DAW's own undo takes the change back with its step (or its plan's): not for a kind the DAW's undo never takes
    /// back (whether its apply was answered or not), nor when the apply said its step makes no undo point (`undo.entry`
    /// false) or a plan's point doesn't cover it (`undo.covered` false).
    pub in_daw_undo: bool,
    /// Kumi's undo of it was sent and never answered.
    pub undo_unanswered: bool,
    pub applied: Option<Value>,
    pub created: Option<Value>,
    pub material: Option<Value>,
    pub state: State,
    pub created_ms: u64,
    /// The answers given, by idempotency key: the same key gets the same answer and changes nothing again.
    pub apply_answers: HashMap<String, (Value, bool)>,
    pub undo_answers: HashMap<String, (Value, bool)>,
}

impl Record {
    fn bytes(&self) -> usize {
        [&self.input, &self.target, &self.prior, &self.fence, &self.display]
            .into_iter()
            .chain(self.applied.iter())
            .chain(self.created.iter())
            .chain(self.material.iter())
            .map(|value| value.to_string().len())
            .sum::<usize>()
            + self.apply_answers.values().chain(self.undo_answers.values()).map(|(answer, _)| answer.to_string().len()).sum::<usize>()
    }
}

/// An answer a key already got, and whether it was an error.
pub type Answered = (Value, bool);
/// What a call may go on with: the record (and the restore's description), or the answer a key already got.
pub type Begun<T> = Result<Result<T, Answered>, Refusal>;

/// Why the journal refuses a call, with the code the tool answers.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Refusal {
    pub code: &'static str,
    pub message: String,
}

fn refuse(code: &'static str, message: impl Into<String>) -> Refusal {
    Refusal { code, message: message.into() }
}

#[derive(Default)]
struct Inner {
    /// Oldest first.
    order: Vec<String>,
    records: HashMap<String, Record>,
    next: u64,
    /// Each record's JSON size, and their sum.
    sizes: HashMap<String, usize>,
    total: usize,
    /// The plan's DAW step open now (its label, with its nonce), if one is.
    open_step: Option<String>,
}

impl Inner {
    /// Counts a record's size again after it changed.
    fn resize(&mut self, id: &str) {
        let now = self.records.get(id).map_or(0, Record::bytes);
        let before = self.sizes.insert(id.to_string(), now).unwrap_or(0);
        self.total = self.total + now - before;
    }

    fn remove(&mut self, id: &str) -> bool {
        self.order.retain(|kept| kept != id);
        self.total -= self.sizes.remove(id).unwrap_or(0);
        self.records.remove(id).is_some()
    }
}

/// The journal. Shared by every call the host serves at once.
#[derive(Default)]
pub struct Journal {
    inner: Mutex<Inner>,
    /// The change kinds the DAW's own undo never takes back.
    outside_undo: &'static [&'static str],
}

/// Four base-36 characters from a counter and the clock, unique among the records kept.
fn token(seed: u64) -> String {
    const DIGITS: &[u8] = b"0123456789abcdefghijklmnopqrstuvwxyz";
    let mut n = seed.wrapping_mul(0x9E37_79B9_7F4A_7C15) ^ (seed >> 7);
    (0..4)
        .map(|_| {
            let digit = DIGITS[(n % 36) as usize] as char;
            n /= 36;
            digit
        })
        .collect()
}

fn now_ms() -> u64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_millis() as u64).unwrap_or(0)
}

impl Journal {
    /// The journal for a DAW: it knows which kinds the DAW's own undo never takes back.
    pub fn for_daw(daw: daw_bridge::Daw) -> Self {
        Journal { outside_undo: crate::tools::kinds(daw).outside_undo, ..Journal::default() }
    }

    /// Keeps a previewed change (the bridge's `change_preview` answer): its id and confirmation.
    pub fn previewed(
        &self,
        kind: &str,
        input: &Value,
        answer: &Value,
        layout: Option<u64>,
        instance: crate::host::Pick,
    ) -> Result<Record, Refusal> {
        let mut inner = self.inner.lock().expect("journal lock");
        inner.next += 1;
        let seed = inner.next ^ now_ms();
        let mut nonce = format!("k{}", token(seed));
        while inner.records.values().any(|record| record.nonce == nonce) {
            inner.next += 1;
            nonce = format!("k{}", token(inner.next ^ now_ms()));
        }
        let record = Record {
            id: format!("t{}{}", inner.next, token(seed ^ 0x5bd1)),
            kind: kind.to_string(),
            input: input.clone(),
            instance,
            epoch: answer.get("epoch").and_then(Value::as_str).map(str::to_owned),
            layout: answer.get("layout").and_then(Value::as_u64).or(layout),
            input_layout: layout,
            target: answer.get("target").cloned().unwrap_or(Value::Null),
            prior: answer.get("prior").cloned().unwrap_or(Value::Null),
            fence: answer.get("fence").cloned().unwrap_or(Value::Null),
            display: answer.get("display").cloned().unwrap_or(Value::Null),
            confirmation: format!("c{}", token(seed ^ 0xa11ce)),
            nonce,
            desc: None,
            step_desc: None,
            undo_desc: None,
            in_daw_undo: !self.outside_undo.contains(&kind),
            undo_unanswered: false,
            applied: None,
            created: None,
            material: None,
            state: State::Previewed,
            created_ms: now_ms(),
            apply_answers: HashMap::new(),
            undo_answers: HashMap::new(),
        };
        Self::make_room(&mut inner, record.bytes())?;
        inner.order.push(record.id.clone());
        inner.records.insert(record.id.clone(), record.clone());
        inner.resize(&record.id);
        Ok(record)
    }

    /// Room for one more record of `bytes`: previewed ones go first, then undone ones nothing can bring back (the DAW's
    /// undo doesn't reach them, so Kumi's undo made no step of it), then the oldest of the rest, undone or applied alike
    /// (an undone one comes back with the DAW's undo of Kumi's undo step, which the journal follows; one let go, Kumi can't
    /// undo: its undo answers `unknown`). Past the bytes, the largest goes first instead, whatever its state (what a
    /// delete removed is what fills the journal). Only changes in flight stay whatever it takes.
    fn make_room(inner: &mut Inner, bytes: usize) -> Result<(), Refusal> {
        const KEPT: [State; 3] = [State::Undone, State::Applied, State::Uncertain];
        while inner.records.len() >= MAX_RECORDS || inner.total + bytes > MAX_BYTES {
            let oldest = |wanted: &dyn Fn(&Record) -> bool| inner.order.iter().find(|id| wanted(&inner.records[*id])).cloned();
            // (newest first, so that of records as large the oldest is the last found, which `max_by_key` takes)
            let largest = || {
                inner
                    .order
                    .iter()
                    .rev()
                    .filter(|id| KEPT.contains(&inner.records[*id].state))
                    .max_by_key(|id| inner.sizes.get(*id).copied().unwrap_or(0))
                    .cloned()
            };
            let by_size = inner.total + bytes > MAX_BYTES;
            let Some(oldest) = oldest(&|record| record.state == State::Previewed)
                .or_else(|| by_size.then(largest).flatten())
                .or_else(|| oldest(&|record| record.state == State::Undone && !record.in_daw_undo))
                .or_else(|| oldest(&|record| KEPT.contains(&record.state)))
            else {
                return Err(refuse("full", format!("the DAW is busy with {} changes at once: try again in a moment", inner.records.len())));
            };
            inner.remove(&oldest);
        }
        Ok(())
    }

    pub fn get(&self, id: &str) -> Option<Record> {
        self.inner.lock().expect("journal lock").records.get(id).cloned()
    }

    /// Before an apply is sent: the answer a key already got, or the record (now applying) and its step description.
    pub fn begin_apply(&self, id: &str, confirmation: &str, key: &str, title: Option<&str>) -> Begun<Record> {
        let mut inner = self.inner.lock().expect("journal lock");
        let record = inner.records.get_mut(id).ok_or_else(|| refuse("unknown", format!("no change {id} here: preview it again")))?;
        if let Some(answer) = record.apply_answers.get(key) {
            return Ok(Err(answer.clone()));
        }
        if record.confirmation != confirmation {
            return Err(refuse("confirmation", "that confirmation isn't this change's: apply with the one its preview gave"));
        }
        match record.state {
            State::Previewed => {}
            State::Applying => return Err(refuse("in-flight", "this change is being applied now")),
            other => return Err(refuse("applied", format!("this change was {} already", other.word()))),
        }
        let title = title
            .map(str::to_owned)
            .or_else(|| record.display.get("title").and_then(Value::as_str).map(str::to_owned))
            .unwrap_or_else(|| record.kind.clone());
        record.desc = Some(format!("Kumi: {title} ({})", record.nonce));
        record.state = State::Applying;
        let record = record.clone();
        let step = inner.open_step.clone();
        let record = match inner.records.get_mut(id) {
            Some(kept) => {
                kept.step_desc = step;
                kept.clone()
            }
            None => record,
        };
        Ok(Ok(record))
    }

    /// Before an apply is answered, what Kumi's undo puts back becomes what the apply found (the host makes some kinds from
    /// what it read again at the apply).
    pub fn refresh_prior(&self, id: &str, prior: Value) {
        let mut inner = self.inner.lock().expect("journal lock");
        if let Some(record) = inner.records.get_mut(id).filter(|record| record.state == State::Applying) {
            record.prior = prior;
        }
    }

    /// The bridge's answer to the apply (or None when it never came): the record's state, and the answer kept for its key.
    pub fn end_apply(&self, id: &str, key: &str, answer: Result<&Value, (&Value, bool)>) -> (Value, bool) {
        let mut inner = self.inner.lock().expect("journal lock");
        let Some(record) = inner.records.get_mut(id) else { return (json!({"error": "the change was released meanwhile"}), true) };
        let (reply, is_error) = match answer {
            Ok(applied) => {
                record.state = State::Applied;
                // Where the DAW's refs are now (Renoise: a deleted track moved the layout): an undo is asked from there.
                if let Some(epoch) = applied.get("epoch").and_then(Value::as_str) {
                    record.epoch = Some(epoch.to_string());
                }
                if let Some(layout) = applied.get("layout").and_then(Value::as_u64) {
                    record.layout = Some(layout);
                }
                record.applied = applied.get("applied").cloned();
                let skipped = |field: &str| applied.get("undo").and_then(|undo| undo.get(field)) == Some(&Value::Bool(false));
                record.in_daw_undo = record.in_daw_undo && !skipped("entry") && !skipped("covered");
                // What the apply found (in its answer's layout): the undo is asked for that.
                if let Some(target) = applied.get("target").filter(|target| !target.is_null()) {
                    record.target = target.clone();
                }
                record.created = applied.get("created").cloned().filter(|created| !created.is_null());
                record.material = applied.get("material").cloned().filter(|material| !material.is_null());
                if let Some(display) = applied.get("display").filter(|display| !display.is_null()) {
                    record.display = display.clone();
                }
                let mut reply = Map::new();
                reply.insert("transactionId".into(), record.id.clone().into());
                reply.insert("state".into(), "applied".into());
                // `kept` (Renoise): the epoch moved only for devices, patterns or the sequence, and the tracks' refs hold.
                for field in ["applied", "created", "display", "undo", "epoch", "layout", "kept"] {
                    if let Some(value) = applied.get(field).filter(|value| !value.is_null()) {
                        reply.insert(field.into(), value.clone());
                    }
                }
                (Value::Object(reply), false)
            }
            // Sent, and no answer: the DAW may have changed (whatever else the error says, it isn't "nothing written").
            Err((error, true)) => {
                record.state = State::Uncertain;
                let mut reply = error.clone();
                reply["state"] = "uncertain".into();
                reply["code"] = "uncertain".into();
                reply["transactionId"] = record.id.clone().into();
                if let Some(answer) = reply.as_object_mut() {
                    answer.remove("busy");
                }
                (reply, true)
            }
            // Refused before anything was written: the change can be applied again with another preview.
            Err((error, false)) => {
                record.state = State::Previewed;
                (error.clone(), true)
            }
        };
        record.apply_answers.insert(key.to_string(), (reply.clone(), is_error));
        inner.resize(id);
        (reply, is_error)
    }

    /// Before an undo is sent: the answer a key already got, or the record (now undoing) and the restore's description.
    pub fn begin_undo(&self, id: &str, key: &str) -> Begun<(Record, String)> {
        let mut inner = self.inner.lock().expect("journal lock");
        inner.next += 1;
        let seed = inner.next ^ now_ms();
        let record = inner.records.get_mut(id).ok_or_else(|| refuse("unknown", format!("no change {id} here")))?;
        if let Some(answer) = record.undo_answers.get(key) {
            return Ok(Err(answer.clone()));
        }
        match record.state {
            State::Applied => {}
            State::Undone => return Err(refuse("undone", "this change was undone already")),
            State::Undoing => return Err(refuse("in-flight", "this change is being undone now")),
            State::Uncertain if record.undo_unanswered => {
                return Err(refuse(
                    "undo-uncertain",
                    "whether Kumi's undo of this went through isn't known, so Kumi won't undo it again blindly",
                ))
            }
            State::Uncertain => {
                return Err(refuse("uncertain", "whether this change went through isn't known, so Kumi won't undo it blindly"))
            }
            State::Previewed | State::Applying => return Err(refuse("not-applied", "this change wasn't applied")),
        }
        record.state = State::Undoing;
        let title = record.display.get("title").and_then(Value::as_str).map(str::to_owned).unwrap_or_else(|| record.kind.clone());
        let desc = format!("Kumi: undo {title} (k{})", token(seed));
        record.undo_desc = Some(desc.clone());
        Ok(Ok((record.clone(), desc)))
    }

    /// The bridge's answer to the restore: undone, or still applied (a refusal leaves the change as it was).
    pub fn end_undo(&self, id: &str, key: &str, answer: Result<&Value, &Value>) -> (Value, bool) {
        let mut inner = self.inner.lock().expect("journal lock");
        let Some(record) = inner.records.get_mut(id) else { return (json!({"error": "the change was released meanwhile"}), true) };
        let (reply, is_error) = match answer {
            Ok(restored) => {
                record.state = State::Undone;
                // Where the DAW's refs are now (Renoise: a restore that made devices again moved the epoch).
                if let Some(epoch) = restored.get("epoch").and_then(Value::as_str) {
                    record.epoch = Some(epoch.to_string());
                }
                if let Some(layout) = restored.get("layout").and_then(Value::as_u64) {
                    record.layout = Some(layout);
                }
                let mut reply = json!({"transactionId": record.id, "state": "undone"});
                for field in ["display", "restored", "epoch", "layout", "kept"] {
                    if let Some(value) = restored.get(field).filter(|value| !value.is_null()) {
                        reply[field] = value.clone();
                    }
                }
                (reply, false)
            }
            Err(error) => {
                record.state = State::Applied;
                (error.clone(), true)
            }
        };
        // A refusal isn't kept: the producer may put things back, and the same undo can then go through.
        if !is_error {
            record.undo_answers.insert(key.to_string(), (reply.clone(), false));
        }
        inner.resize(id);
        (reply, is_error)
    }

    /// The restore was sent and never answered: whether the change is undone isn't known, so the record is Uncertain (its
    /// undo is refused from now on rather than made twice).
    pub fn end_undo_unanswered(&self, id: &str, error: &Value) -> (Value, bool) {
        let mut inner = self.inner.lock().expect("journal lock");
        let Some(record) = inner.records.get_mut(id) else { return (json!({"error": "the change was released meanwhile"}), true) };
        record.state = State::Uncertain;
        record.undo_unanswered = true;
        let mut reply = error.clone();
        // (the DAW answered, and couldn't say how it ended: its words say what to look for)
        if error.get("code").and_then(Value::as_str) == Some(daw_bridge::wire::code::UNCERTAIN) {
            reply["answered"] = true.into();
        }
        reply["code"] = "undo-uncertain".into();
        reply["state"] = "uncertain".into();
        if let Some(answer) = reply.as_object_mut() {
            answer.remove("busy");
        }
        (reply, true)
    }

    /// Forgets these changes (unknown ids are skipped): how many went.
    pub fn release(&self, ids: &[String]) -> usize {
        let mut inner = self.inner.lock().expect("journal lock");
        ids.iter().filter(|id| inner.remove(id)).count()
    }

    /// The changes whose DAW step description is one of `descs` (the DAW's own undo or redo of them): their own, or
    /// the plan's step they were made in.
    pub fn by_descs(&self, descs: &[String]) -> Vec<String> {
        let inner = self.inner.lock().expect("journal lock");
        let named = |desc: &Option<String>| desc.as_ref().is_some_and(|desc| descs.contains(desc));
        inner.order.iter().filter(|id| named(&inner.records[*id].desc) || named(&inner.records[*id].step_desc)).cloned().collect()
    }

    /// The producer's own undo (or redo) of these DAW steps: which changes it took back and which it put back, each
    /// record following. A change's own step (or its plan's) undone takes the change back; the step Kumi's undo made
    /// (`undo_desc`) undone puts the change back, and redone takes it back again. A change put back can be undone by
    /// Kumi again, as a fresh undo (its earlier undo's answers go).
    pub fn follow_daw(&self, descs: &[String], undo: bool) -> (Vec<String>, Vec<String>) {
        let mut inner = self.inner.lock().expect("journal lock");
        let named = |desc: &Option<String>| desc.as_ref().is_some_and(|desc| descs.contains(desc));
        let (mut back, mut again) = (vec![], vec![]);
        let order = inner.order.clone();
        for id in order {
            let Some(record) = inner.records.get_mut(&id) else { continue };
            // A change the DAW's own undo doesn't take back stays as it is when its step (or its plan's) is undone.
            let own = record.in_daw_undo && (named(&record.desc) || named(&record.step_desc));
            let restore = named(&record.undo_desc);
            let taken_back = match (own, restore) {
                // (r-tracks, NIT-5) Its own step and its undo's step at once, a jump in the DAW's undo history over both:
                // back over them leaves the change not made, forward over them leaves it undone again (its undo's step is
                // the later one). Undone either way.
                (true, true) => true,
                (true, false) => undo,
                (false, true) => !undo,
                (false, false) => continue,
            };
            // (the DAW's undo or redo settles a record whose undo by Kumi was never answered: Kumi may undo it again)
            match (taken_back, record.state) {
                (true, State::Applied | State::Uncertain) => {
                    record.state = State::Undone;
                    record.undo_unanswered = false;
                    back.push(id.clone());
                }
                (false, State::Undone) => {
                    record.state = State::Applied;
                    record.undo_unanswered = false;
                    record.undo_answers.clear();
                    again.push(id.clone());
                }
                // Kumi's undo was never answered, and the DAW's undo just took its step back: it did run, and the
                // change is back.
                (false, State::Uncertain) if record.undo_unanswered && restore => {
                    record.state = State::Applied;
                    record.undo_unanswered = false;
                    record.undo_answers.clear();
                    again.push(id.clone());
                }
                _ => {}
            }
        }
        (back, again)
    }

    /// The DAW's undo or redo put back what an undone change wrote, and can't say which step it took (Renoise's lines: its
    /// redo of the change, or its undo past it of an older edit that moved like lines there): the change may be made again,
    /// so Kumi may undo it, and the bridge's restore decides whether that is safe. The changes it names.
    pub fn follow_unsure(&self, descs: &[String]) -> Vec<String> {
        let mut inner = self.inner.lock().expect("journal lock");
        let order = inner.order.clone();
        let mut unsure = vec![];
        for id in order {
            let Some(record) = inner.records.get_mut(&id) else { continue };
            let named = [&record.desc, &record.step_desc].into_iter().flatten().any(|desc| descs.contains(desc));
            if record.in_daw_undo && named && record.state == State::Undone {
                record.state = State::Applied;
                record.undo_unanswered = false;
                record.undo_answers.clear();
                unsure.push(id.clone());
            }
        }
        unsure
    }

    /// A plan's DAW step label, with a nonce of its own, so the DAW's undo of the plan names its changes.
    pub fn step_label(&self, label: &str) -> String {
        let mut inner = self.inner.lock().expect("journal lock");
        inner.next += 1;
        format!("Kumi: {label} (k{})", token(inner.next ^ now_ms() ^ 0x57e9))
    }

    /// The plan's step opened (Some, its label) or closed (None): the changes made meanwhile are in it.
    pub fn step(&self, open: Option<String>) {
        self.inner.lock().expect("journal lock").open_step = open;
    }

    /// The open step's description if it closes as `label` says (what the plan did), with the nonce it began with.
    /// None when no step is open.
    pub fn step_named(&self, label: &str) -> Option<String> {
        let inner = self.inner.lock().expect("journal lock");
        let before = inner.open_step.clone()?;
        let nonce = before.rfind(" (k").map_or("", |at| &before[at..]).to_string();
        Some(format!("Kumi: {label}{nonce}"))
    }

    /// The open step closed as `named` (the DAW confirmed it): the changes made in it are told by that name now.
    pub fn name_step(&self, named: &str) {
        let mut inner = self.inner.lock().expect("journal lock");
        let Some(before) = inner.open_step.clone() else { return };
        for record in inner.records.values_mut() {
            if record.step_desc.as_deref() == Some(before.as_str()) {
                record.step_desc = Some(named.to_owned());
            }
        }
        inner.open_step = Some(named.to_owned());
    }

    pub fn len(&self) -> usize {
        self.inner.lock().expect("journal lock").records.len()
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
}

/// The producer undid (or redid) Kumi's changes with the DAW's own undo: the bridge names their step descriptions, the
/// journal follows, and Kumi is told which changes that took back (`undone`) and which it put back (`redone`), or may have
/// (`unsure`), with `transactionIds` naming the changes whose own steps those were. Any other event goes on as it came.
pub fn undone_in_daw(journal: &Journal, name: &str, mut data: Value) -> Value {
    if name != "undone_in_daw" {
        return data;
    }
    let descs: Vec<String> =
        data.get("descs").and_then(Value::as_array).into_iter().flatten().filter_map(Value::as_str).map(str::to_owned).collect();
    data["transactionIds"] = json!(journal.by_descs(&descs));
    let how = data.get("how").and_then(Value::as_str);
    if how == Some("unsure") {
        data["unsure"] = json!(journal.follow_unsure(&descs));
        data["undone"] = json!([]);
        data["redone"] = json!([]);
        return data;
    }
    let (undone, redone) = journal.follow_daw(&descs, how != Some("redo"));
    data["undone"] = json!(undone);
    data["redone"] = json!(redone);
    data
}

/// How long a change may take the DAW (it bounds each of its own steps; a big project takes a while).
pub(crate) const CHANGE_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(30);

/// What the bridge's `change_apply` is sent: Kumi's input at the layout Kumi made its refs in (not the preview's, at
/// which the DAW would carry nothing: a track inserted between the two would have the change land on its neighbour),
/// and the fence and description the preview and journal gave.
fn apply_params(record: &Record) -> Value {
    let mut params = json!({"kind": record.kind, "input": record.input, "fence": record.fence, "desc": record.desc});
    place(&mut params, record.epoch.as_deref(), record.input_layout);
    params
}

fn refusal(refusal: Refusal) -> (Value, bool) {
    (json!({"error": refusal.message, "code": refusal.code}), true)
}

fn text<'a>(arguments: &'a Value, name: &str) -> Result<&'a str, (Value, bool)> {
    arguments
        .get(name)
        .and_then(Value::as_str)
        .filter(|text| !text.is_empty() && text.len() <= 256)
        .ok_or_else(|| (json!({"error": format!("{name} must be a string"), "code": "bad-params"}), true))
}

/// What the bridge is told of a change's place: the epoch it was read in, and the layout (Renoise) its refs are at.
pub(crate) fn place(params: &mut Value, epoch: Option<&str>, layout: Option<u64>) {
    if let Some(epoch) = epoch {
        params["epoch"] = epoch.into();
    }
    if let Some(layout) = layout {
        params["layout"] = layout.into();
    }
}

impl crate::host::Host {
    /// New preset-file fields must never be sent to an old bridge. In particular, an old REAPER accepts a file in its
    /// named-preset argument without checking its class. Advertise the format explicitly rather than guessing versions.
    async fn preset_file_bridge(&self, pick: &crate::host::Pick, kind: &str, input: &Value) -> Result<(), Value> {
        let path = |field: &str| input[field].as_str().map(str::trim);
        let vst3 = |value: Option<&str>| {
            value.is_some_and(|value| std::path::Path::new(value).extension().is_some_and(|ext| ext.eq_ignore_ascii_case("vstpreset")))
        };
        let needed = match (self.daw(), kind) {
            (daw_bridge::Daw::Reaper, "fx-preset") => input.get("path").is_some() || vst3(path("preset")),
            (daw_bridge::Daw::Renoise, "plugin-preset-data") => true,
            (daw_bridge::Daw::Renoise, "device-preset-data") => vst3(path("path")) || input.get("vst3Data").is_some(),
            _ => false,
        };
        if !needed {
            return Ok(());
        }
        let status = self.call("status", json!({}), pick, CHANGE_TIMEOUT).await.map_err(|e| self.call_error(&e))?;
        if !status["presetFiles"].as_array().is_some_and(|formats| formats.iter().any(|format| format == "vst3")) {
            return Err(json!({"code":"bridge-update-required", "error":format!(
                "The bridge running in {} cannot safely load VST3 preset files. Update Kumi's bridge and restart {} before loading this preset.", self.daw(), self.daw())}));
        }
        Ok(())
    }

    /// `change_preview`: what a change would do, and the id and confirmation to apply it by. Nothing changes.
    pub async fn change_preview(&self, arguments: &Value) -> (Value, bool) {
        let pick = match crate::host::Pick::from_arguments(arguments) {
            Ok(pick) => pick,
            Err(why) => return (json!({"error": why, "code": "bad-params"}), true),
        };
        let kind = match text(arguments, "kind") {
            Ok(kind) => kind.to_string(),
            Err(answer) => return answer,
        };
        let Some(input) = arguments.get("input").filter(|input| input.is_object()) else {
            return (json!({"error": "input must be an object", "code": "bad-params"}), true);
        };
        if matches!(kind.as_str(), "fx-preset" | "device-preset-data" | "plugin-preset-data")
            && ["path", "preset"]
                .into_iter()
                .filter_map(|field| input[field].as_str())
                .any(|path| kumi_common::path::network_or_device(path.trim()))
        {
            return (json!({"code":"bad-params", "error":"Preset paths must name local files, not network shares or devices."}), true);
        }
        let layout = arguments.get("layout").and_then(Value::as_u64);
        let pick = match self.pin(&pick).await {
            Ok(pinned) => pinned,
            Err(error) => return (self.call_error(&error), true),
        };
        if let Err(answer) = self.preset_file_bridge(&pick, &kind, input).await {
            return (answer, true);
        }
        // (M4 n-devices: a device's preset data from an .xrdp file in Renoise's library is read here; the tool writes it as
        // preset data given in full)
        // (the library's files are read off the runtime's workers)
        let read;
        let (daw, asked, given) = (self.daw(), kind.clone(), input.clone());
        let with_file = tokio::task::spawn_blocking(move || {
            crate::renoise::library_files::with_file(daw, &asked, &given, &crate::renoise::library_files::library_roots())
        })
        .await
        .unwrap_or_else(|_| Some(Err(crate::renoise::presets::Refusal::Bad("Kumi couldn't read the library file".into()))));
        let input = match with_file {
            Some(Ok(with_data)) => {
                read = with_data;
                &read
            }
            Some(Err(refusal)) => return (crate::renoise::presets::refusal_answer(&refusal), true),
            None => input,
        };
        // (M4 n-devices: the Doofer, Splitter and Notepad edits are made here from the device's preset data, and an
        // instrument's macro mappings added or removed from its file)
        // The instance the change is previewed in: its apply and undo go there.
        if crate::renoise::presets::host_makes(self.daw(), &kind, input) {
            return self.preset_preview(&pick, &kind, input, arguments.get("epoch").and_then(Value::as_str), layout).await;
        }
        let mut params = json!({"kind": kind, "input": input});
        place(&mut params, arguments.get("epoch").and_then(Value::as_str), layout);
        let answer = match self.call("change_preview", params, &pick, CHANGE_TIMEOUT).await {
            Ok(answer) => answer,
            Err(error) => return (self.call_error(&error), true),
        };
        match self.journal().previewed(&kind, input, &answer, layout, pick) {
            Ok(record) => {
                let mut reply = json!({"transactionId": record.id, "confirmation": record.confirmation, "prior": record.prior, "display": record.display, "target": record.target});
                place(&mut reply, record.epoch.as_deref(), record.layout);
                (reply, false)
            }
            Err(why) => refusal(why),
        }
    }

    /// `change_apply`: the previewed change, made in one named DAW step. A key sent again gets the same answer.
    pub async fn change_apply(&self, arguments: &Value) -> (Value, bool) {
        let (id, confirmation, key) =
            match (text(arguments, "transactionId"), text(arguments, "confirmation"), text(arguments, "idempotencyKey")) {
                (Ok(id), Ok(confirmation), Ok(key)) => (id.to_string(), confirmation.to_string(), key.to_string()),
                (Err(answer), _, _) | (_, Err(answer), _) | (_, _, Err(answer)) => return answer,
            };
        let title = arguments.get("title").and_then(Value::as_str);
        let record = match self.journal().begin_apply(&id, &confirmation, &key, title) {
            Ok(Ok(record)) => record,
            Ok(Err(answered)) => return answered,
            Err(why) => return refusal(why),
        };
        if let Some(answer) = self.elsewhere(&record.instance) {
            return self.journal().end_apply(&id, &key, Err((&answer, false)));
        }
        if let Err(answer) = self.preset_file_bridge(&record.instance, &record.kind, &record.input).await {
            return self.journal().end_apply(&id, &key, Err((&answer, false)));
        }
        let params = if crate::renoise::presets::host_makes(self.daw(), &record.kind, &record.input) {
            match self.preset_apply_params(&record).await {
                Ok(params) => params,
                // (refused before anything was written: the change can be previewed again)
                Err(answer) => return self.journal().end_apply(&id, &key, Err((&answer, false))),
            }
        } else {
            apply_params(&record)
        };
        match self.call("change_apply", params, &record.instance, CHANGE_TIMEOUT).await {
            Ok(answer) => {
                let answer = self.with_added_content(&record, answer).await;
                self.journal().end_apply(&id, &key, Ok(&answer))
            }
            // The bridge took it and couldn't say how it ended (a job it claimed and didn't finish in time): it may have
            // changed the DAW.
            Err(error @ crate::host::CallError::Bridge { .. }) if matches!(&error, crate::host::CallError::Bridge { code, .. } if code == daw_bridge::wire::code::UNCERTAIN) => {
                self.journal().end_apply(&id, &key, Err((&self.call_error(&error), true)))
            }
            // Answered, or never sent: nothing was written, and the change can be previewed again.
            Err(error @ (crate::host::CallError::Bridge { .. } | crate::host::CallError::NotConnected(_))) => {
                self.journal().end_apply(&id, &key, Err((&self.call_error(&error), false)))
            }
            // Sent, and no answer came: the DAW may have changed.
            Err(error) => self.journal().end_apply(&id, &key, Err((&self.call_error(&error), true))),
        }
    }

    /// `change_undo`: the change taken back as a new named step, its values put back exactly; refused if they changed since.
    pub async fn change_undo(&self, arguments: &Value) -> (Value, bool) {
        let (id, key) = match (text(arguments, "transactionId"), text(arguments, "idempotencyKey")) {
            (Ok(id), Ok(key)) => (id.to_string(), key.to_string()),
            (Err(answer), _) | (_, Err(answer)) => return answer,
        };
        if arguments.get("confirmation").and_then(Value::as_str) != Some("undo") {
            return (json!({"error": "confirmation must be \"undo\"", "code": "confirmation"}), true);
        }
        let (record, desc) = match self.journal().begin_undo(&id, &key) {
            Ok(Ok(found)) => found,
            Ok(Err(answered)) => return answered,
            Err(why) => return refusal(why),
        };
        if let Some(answer) = self.elsewhere(&record.instance) {
            return self.journal().end_undo(&id, &key, Err(&answer));
        }
        if let Err(answer) = self.preset_file_bridge(&record.instance, &record.kind, &record.input).await {
            return self.journal().end_undo(&id, &key, Err(&answer));
        }
        let discard = arguments.get("discard") == Some(&Value::Bool(true));
        let params = if crate::renoise::presets::composes_undo(self.daw(), &record.kind, &record.input) {
            match self.preset_restore_params(&record, &desc, discard).await {
                Ok(Ok(params)) => params,
                // (it is back already: nothing to write)
                Ok(Err(back)) => return self.journal().end_undo(&id, &key, Ok(&back)),
                Err(answer) => return self.journal().end_undo(&id, &key, Err(&answer)),
            }
        } else {
            // (`of`: the change's own step name, by which the bridge's journal knows whether its DAW's redo put the change back)
            let mut params = json!({"kind": record.kind, "target": record.target, "prior": record.prior, "applied": record.applied, "desc": desc,
                "of": record.desc});
            if let Some(created) = &record.created {
                params["created"] = created.clone();
            }
            if let Some(material) = &record.material {
                params["material"] = material.clone();
            }
            if discard {
                params["discard"] = true.into();
            }
            place(&mut params, record.epoch.as_deref(), record.layout);
            params
        };
        match self.call("change_restore", params, &record.instance, CHANGE_TIMEOUT).await {
            Ok(answer) => self.journal().end_undo(&id, &key, Ok(&answer)),
            // Sent, and no answer (or the bridge couldn't say how it ended): the restore may have run. Kumi won't undo it
            // blindly again.
            Err(error @ (crate::host::CallError::Timeout | crate::host::CallError::Closed(_))) => {
                self.journal().end_undo_unanswered(&id, &self.call_error(&error))
            }
            Err(error @ crate::host::CallError::Bridge { .. }) if matches!(&error, crate::host::CallError::Bridge { code, .. } if code == daw_bridge::wire::code::UNCERTAIN) => {
                self.journal().end_undo_unanswered(&id, &self.call_error(&error))
            }
            Err(error) => self.journal().end_undo(&id, &key, Err(&self.call_error(&error))),
        }
    }

    /// `transport_stop`: the DAW stops playing and recording (the emergency stop).
    pub async fn transport_stop(&self, arguments: &Value) -> (Value, bool) {
        let pick = match crate::host::Pick::from_arguments(arguments) {
            Ok(pick) => pick,
            Err(why) => return (json!({"error": why, "code": "bad-params"}), true),
        };
        match self.call("transport_stop", json!({}), &pick, CHANGE_TIMEOUT).await {
            Ok(answer) => (answer, false),
            Err(error) => (self.call_error(&error), true),
        }
    }

    /// `transport_play` / `cursor_set`: the DAW starts playing (Renoise: from a slot and line, if given), or its edit cursor
    /// moves (the play position follows while it plays). Neither is a step of the DAW's undo.
    pub async fn transport(&self, method: &str, arguments: &Value) -> (Value, bool) {
        let pick = match crate::host::Pick::from_arguments(arguments) {
            Ok(pick) => pick,
            Err(why) => return (json!({"error": why, "code": "bad-params"}), true),
        };
        let mut params = json!({});
        let fields: &[&str] = if method == "cursor_set" { &["positionBeats", "position", "epoch"] } else { &["sequence", "line", "epoch"] };
        for field in fields {
            if let Some(value) = arguments.get(*field) {
                params[*field] = value.clone();
            }
        }
        match self.call(method, params, &pick, CHANGE_TIMEOUT).await {
            Ok(answer) => (answer, false),
            Err(error) => (self.call_error(&error), true),
        }
    }

    // m4-r-fxenv
    /// `fx_chain_save` (REAPER): a chain saved as an FX chain file. A file, not the project: no step of the DAW's undo,
    /// nothing to journal.
    pub async fn fx_chain_save(&self, arguments: &Value) -> (Value, bool) {
        let pick = match crate::host::Pick::from_arguments(arguments) {
            Ok(pick) => pick,
            Err(why) => return (json!({"error": why, "code": "bad-params"}), true),
        };
        let mut params = json!({});
        for field in ["ref", "input", "fxRefs", "name", "overwrite", "epoch"] {
            if let Some(value) = arguments.get(field) {
                params[field] = value.clone();
            }
        }
        match self.call("fx_chain_save", params, &pick, CHANGE_TIMEOUT).await {
            Ok(answer) => (answer, false),
            Err(error) => (self.call_error(&error), true),
        }
    }

    /// (r-tracks) A bridge method that writes no change (REAPER's saves): its arguments go on as they came, less the instance
    /// pick, and the bridge's answer comes back. Never asked again on a new connection (it writes a file).
    pub async fn passthrough(&self, method: &str, arguments: &Value) -> (Value, bool) {
        let pick = match crate::host::Pick::from_arguments(arguments) {
            Ok(pick) => pick,
            Err(why) => return (json!({"error": why, "code": "bad-params"}), true),
        };
        let mut params = arguments.clone();
        if let Some(fields) = params.as_object_mut() {
            fields.remove("pid");
            fields.remove("resource");
        }
        match self.call(method, params, &pick, CHANGE_TIMEOUT).await {
            Ok(answer) => (answer, false),
            Err(error) => (self.call_error(&error), true),
        }
    }

    /// `audition_note` / `select` / `instrument_save` (Renoise, M4): a note played out loud for a while, things selected or shown,
    /// an instrument written to an .xrni file (over a file already there only with overwrite). None is a change: the bridge
    /// answers what it did. Each method forwards only its own fields.
    pub async fn action(&self, method: &str, arguments: &Value) -> (Value, bool) {
        if self.daw() != daw_bridge::Daw::Renoise {
            return (json!({"error": format!("{method} is Renoise's"), "code": "bad-params"}), true);
        }
        let pick = match crate::host::Pick::from_arguments(arguments) {
            Ok(pick) => pick,
            Err(why) => return (json!({"error": why, "code": "bad-params"}), true),
        };
        let fields: &[&str] = match method {
            // (Kumi names refs as …Ref: both are passed as they come)
            "audition_note" => &[
                "instrument",
                "instrumentRef",
                "track",
                "trackRef",
                "note",
                "notes",
                "volume",
                "durationMs",
                "sample",
                "sampleRef",
                "stop",
                "epoch",
                "layout",
            ],
            "select" => &[
                "track",
                "trackRef",
                "device",
                "deviceRef",
                "instrument",
                "instrumentRef",
                "sample",
                "sampleRef",
                "phrase",
                "phraseRef",
                "sequence",
                "sequenceRef",
                "pattern",
                "patternRef",
                "line",
                "frame",
                "lower",
                "matrix",
                "epoch",
                "layout",
            ],
            "instrument_save" => &["ref", "instrumentRef", "path", "overwrite", "epoch", "layout"],
            _ => &[],
        };
        let mut params = json!({});
        // (a field and its …Ref name the same thing: one of the two, or both the same)
        for field in fields.iter().filter(|field| !field.ends_with("Ref")) {
            let named = |name: &str| arguments.get(name).filter(|value| !value.is_null());
            if let (Some(plain), Some(aliased)) = (named(field), named(&format!("{field}Ref"))) {
                if plain != aliased {
                    return (
                        json!({"error": format!("{field} and {field}Ref name different things: pass one of the two"), "code": "bad-params"}),
                        true,
                    );
                }
            }
        }
        for field in fields {
            if let Some(value) = arguments.get(*field) {
                params[*field] = value.clone();
            }
        }
        match self.call(method, params, &pick, CHANGE_TIMEOUT).await {
            Ok(answer) => (answer, false),
            Err(error) => (self.call_error(&error), true),
        }
    }

    /// `undo_step_begin` / `undo_step_end`: one DAW undo step around a plan's changes, where the DAW gives one. Its
    /// label carries a nonce, so the DAW's own undo of the plan names the plan's changes.
    pub async fn undo_step(&self, method: &str, arguments: &Value) -> (Value, bool) {
        let (params, label, named) = match method {
            "undo_step_begin" => match text(arguments, "label") {
                Ok(label) => {
                    let label = self.journal().step_label(label);
                    (json!({"label": label}), Some(label), None)
                }
                Err(answer) => return answer,
            },
            _ => match text(arguments, "stepId") {
                Ok(step) => {
                    let mut params = json!({"stepId": step});
                    // Named by what the plan did, as it closes (REAPER names a block when it ends it); the changes made in it
                    // are told by that name once the DAW confirms it.
                    let named = arguments.get("label").and_then(Value::as_str).and_then(|label| self.journal().step_named(label));
                    if let Some(named) = &named {
                        params["label"] = named.clone().into();
                    }
                    (params, None, named)
                }
                Err(answer) => return answer,
            },
        };
        match self.call(method, params, &crate::host::Pick::default(), CHANGE_TIMEOUT).await {
            Ok(answer) => {
                if let Some(named) = &named {
                    self.journal().name_step(named);
                }
                // Opened, or closed (now, or at the DAW's first free moment: `deferred`): the changes made from now on are in
                // it, or aren't.
                self.journal().step(label);
                (answer, false)
            }
            Err(error) => {
                // A DAW too busy to close it keeps it open: Kumi asks again. Anything else, and the step is over for the journal.
                let busy = matches!(&error, crate::host::CallError::Bridge { code, .. } if code == daw_bridge::wire::code::BUSY)
                    || matches!(error, crate::host::CallError::Timeout);
                if method == "undo_step_end" && !busy {
                    self.journal().step(None);
                }
                (self.call_error(&error), true)
            }
        }
    }

    /// `change_release`: changes Kumi won't undo any more, forgotten.
    pub fn change_release(&self, arguments: &Value) -> (Value, bool) {
        let Some(ids) = arguments.get("transactionIds").and_then(Value::as_array) else {
            return (json!({"error": "transactionIds must be a list", "code": "bad-params"}), true);
        };
        if ids.is_empty() || ids.len() > MAX_RELEASE || ids.iter().any(|id| !id.is_string()) {
            return (json!({"error": format!("transactionIds must be 1 to {MAX_RELEASE} ids"), "code": "bad-params"}), true);
        }
        let ids: Vec<String> = ids.iter().filter_map(Value::as_str).map(str::to_owned).collect();
        (json!({"released": self.journal().release(&ids)}), false)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::host::Pick;

    fn preview(journal: &Journal) -> Record {
        let answer = json!({"epoch": "e1", "target": {"ref": "r:track:{A}", "kind": "track"}, "prior": {"volumeDb": 0.0}, "fence": {"volumeDb": 0.0}, "display": {"title": "Bass volume −6 dB"}});
        journal.previewed("track-volume", &json!({"ref": "r:track:{A}", "volumeDb": -6.0}), &answer, None, Pick::default()).unwrap()
    }

    #[test]
    fn a_change_goes_previewed_applying_applied_undoing_undone_and_a_key_sent_twice_answers_once() {
        let journal = Journal::default();
        let record = preview(&journal);
        assert_eq!((record.state, record.epoch.as_deref()), (State::Previewed, Some("e1")));
        assert!(record.nonce.starts_with('k') && record.nonce.len() == 5, "{}", record.nonce);
        assert_eq!(journal.begin_apply(&record.id, "wrong", "a1", None).unwrap_err().code, "confirmation");
        let Ok(Ok(applying)) = journal.begin_apply(&record.id, &record.confirmation, "a1", None) else { panic!("applying") };
        assert_eq!(applying.desc.as_deref(), Some(format!("Kumi: Bass volume −6 dB ({})", record.nonce).as_str()));
        assert_eq!(journal.begin_apply(&record.id, &record.confirmation, "a2", None).unwrap_err().code, "in-flight");
        let (reply, is_error) =
            journal.end_apply(&record.id, "a1", Ok(&json!({"applied": {"volumeDb": -6.0}, "undo": {"desc": "x", "entry": true}})));
        assert!(!is_error && reply["state"] == "applied", "{reply}");
        // The same key: the same answer, nothing sent.
        assert_eq!(journal.begin_apply(&record.id, &record.confirmation, "a1", None).unwrap().unwrap_err(), (reply, false));
        assert_eq!(journal.begin_apply(&record.id, &record.confirmation, "a3", None).unwrap_err().code, "applied");
        let Ok(Ok((undoing, desc))) = journal.begin_undo(&record.id, "u1") else { panic!("undoing") };
        assert!(desc.starts_with("Kumi: undo Bass volume −6 dB (k"), "{desc}");
        assert_eq!(undoing.applied, Some(json!({"volumeDb": -6.0})));
        // Refused by the DAW (it changed since): still applied, and the same undo can go again.
        let (refused, _) = journal.end_undo(&record.id, "u1", Err(&json!({"code": "changed", "error": "changed since"})));
        assert_eq!(refused["code"], "changed");
        assert_eq!(journal.get(&record.id).unwrap().state, State::Applied);
        assert!(journal.begin_undo(&record.id, "u1").unwrap().is_ok());
        let kept = json!({"from": "e1", "tracks": true});
        let (undone, is_error) =
            journal.end_undo(&record.id, "u1", Ok(&json!({"restored": {"volumeDb": 0.0}, "epoch": "e2", "kept": kept})));
        assert!(!is_error && undone["state"] == "undone");
        // What the restore put back, and where the DAW's refs are now (and that its tracks' refs hold), for Kumi to follow.
        assert_eq!((&undone["restored"], &undone["epoch"], &undone["kept"]), (&json!({"volumeDb": 0.0}), &json!("e2"), &kept));
        assert_eq!(journal.get(&record.id).unwrap().epoch.as_deref(), Some("e2"));
        assert_eq!(journal.begin_undo(&record.id, "u1").unwrap().unwrap_err(), (undone, false));
        assert_eq!(journal.begin_undo(&record.id, "u2").unwrap_err().code, "undone");
    }

    #[test]
    fn an_undo_is_asked_from_the_epoch_and_layout_the_apply_answered() {
        let journal = Journal::default();
        let answer = json!({"epoch": "e1", "layout": 4, "target": {"ref": "e1:track:3", "kind": "track"}, "prior": {"deleted": false}, "fence": {}, "display": {"title": "Deleted track Bass"}});
        let record = journal.previewed("track-delete", &json!({"ref": "e1:track:3"}), &answer, Some(4), Pick::default()).unwrap();
        let Ok(Ok(_)) = journal.begin_apply(&record.id, &record.confirmation, "a1", None) else { panic!("applying") };
        // Deleting the track moved Renoise's layout from 4 to 5 (and a device list, in another change, would move the epoch).
        journal.end_apply(
            &record.id,
            "a1",
            Ok(&json!({"epoch": "e1", "layout": 5, "applied": {"deleted": true}, "undo": {"desc": "x", "entry": true}})),
        );
        let Ok(Ok((undoing, _))) = journal.begin_undo(&record.id, "u1") else { panic!("undoing") };
        assert_eq!((undoing.epoch.as_deref(), undoing.layout), (Some("e1"), Some(5)));
    }

    #[test]
    fn an_apply_without_an_answer_is_uncertain_and_kumi_won_t_undo_it_blindly() {
        let journal = Journal::default();
        let record = preview(&journal);
        journal.begin_apply(&record.id, &record.confirmation, "a1", None).unwrap().unwrap();
        let (reply, is_error) =
            journal.end_apply(&record.id, "a1", Err((&json!({"error": "REAPER didn't answer in time", "busy": true}), true)));
        assert!(is_error && reply["state"] == "uncertain");
        assert_eq!(journal.begin_undo(&record.id, "u1").unwrap_err().code, "uncertain");
        // Refused before writing: back to previewed, applicable again.
        let other = preview(&journal);
        journal.begin_apply(&other.id, &other.confirmation, "b1", None).unwrap().unwrap();
        journal.end_apply(&other.id, "b1", Err((&json!({"error": "changed", "code": "changed"}), false)));
        assert_eq!(journal.get(&other.id).unwrap().state, State::Previewed);
    }

    #[test]
    fn a_full_journal_forgets_its_oldest_previewed_then_what_cant_come_back_then_the_oldest_and_never_refuses_one() {
        let journal = Journal::default();
        let first = preview(&journal);
        for _ in 1..MAX_RECORDS {
            preview(&journal);
        }
        assert_eq!(journal.len(), MAX_RECORDS);
        preview(&journal);
        assert!(journal.get(&first.id).is_none(), "the oldest previewed one went");
        // Every record applied; the second undone (the DAW's undo of Kumi's undo step would bring it back); the third,
        // which the DAW's undo doesn't reach, undone too: that one goes first, then the oldest, and nothing is refused.
        let ids: Vec<String> = journal.inner.lock().unwrap().order.clone();
        for (n, id) in ids.iter().enumerate() {
            let record = journal.get(id).unwrap();
            journal.begin_apply(id, &record.confirmation, "k", None).unwrap().unwrap();
            let entry = if n == 2 { json!(false) } else { json!(true) };
            journal.end_apply(id, "k", Ok(&json!({"applied": {}, "undo": {"entry": entry}})));
        }
        for id in &ids[1..3] {
            journal.begin_undo(id, "u").unwrap().unwrap();
            journal.end_undo(id, "u", Ok(&json!({"restored": {}})));
        }
        let answer = json!({"prior": {}, "fence": {}});
        let make = || {
            let newer = journal.previewed("track-volume", &json!({}), &answer, None, Pick::default()).unwrap();
            journal.begin_apply(&newer.id, &newer.confirmation, "k", None).unwrap().unwrap();
            journal.end_apply(&newer.id, "k", Ok(&json!({"applied": {}})));
        };
        make();
        assert!(journal.get(&ids[2]).is_none() && journal.get(&ids[0]).is_some(), "the undone one nothing brings back went first");
        make();
        assert!(journal.get(&ids[0]).is_none(), "then the oldest applied one (Kumi's undo of it answers unknown)");
        assert!(journal.get(&ids[1]).is_some(), "the one Kumi undid stays as long as its age keeps it");
        assert_eq!(journal.begin_undo(&ids[0], "u").unwrap_err().code, "unknown");
        make();
        assert!(journal.get(&ids[1]).is_none(), "then it, by its age");
        assert_eq!(journal.len(), MAX_RECORDS);
    }

    #[test]
    fn an_apply_sends_kumis_input_at_the_layout_kumi_made_its_refs_in_and_the_undo_takes_what_the_apply_found() {
        let journal = Journal::default();
        // Kumi read Bass as track 3 at layout 0; the producer inserted a track, so the preview answered at layout 1.
        let answer = json!({"epoch": "e1", "layout": 1, "target": {"ref": "e1:track:4", "kind": "track"}, "prior": {"volumeDb": 0.0}, "fence": {}, "display": {"title": "Bass volume −6 dB"}});
        let record =
            journal.previewed("track-volume", &json!({"ref": "e1:track:3", "volumeDb": -6.0}), &answer, Some(0), Pick::default()).unwrap();
        let Ok(Ok(applying)) = journal.begin_apply(&record.id, &record.confirmation, "a", None) else { panic!("applying") };
        let params = apply_params(&applying);
        assert_eq!((&params["input"]["ref"], &params["layout"]), (&json!("e1:track:3"), &json!(0)), "the DAW carries it again");
        // Another track moved before the apply landed: the apply says where it found Bass, and the undo is asked there.
        journal.end_apply(
            &record.id,
            "a",
            Ok(&json!({"layout": 2, "target": {"ref": "e1:track:5", "kind": "track"}, "applied": {"volumeDb": -6.0}})),
        );
        let kept = journal.get(&record.id).unwrap();
        assert_eq!((&kept.target["ref"], kept.layout), (&json!("e1:track:5"), Some(2)));
    }

    #[test]
    fn an_undo_sent_and_never_answered_is_uncertain_and_never_sent_again() {
        let journal = Journal::default();
        let record = preview(&journal);
        journal.begin_apply(&record.id, &record.confirmation, "a", None).unwrap().unwrap();
        journal.end_apply(&record.id, "a", Ok(&json!({"applied": {"volumeDb": -6.0}})));
        journal.begin_undo(&record.id, "u1").unwrap().unwrap();
        let (reply, is_error) = journal.end_undo_unanswered(&record.id, &json!({"error": "REAPER didn't answer in time", "busy": true}));
        assert!(is_error && reply["code"] == "undo-uncertain" && reply.get("busy").is_none(), "{reply}");
        assert_eq!(journal.begin_undo(&record.id, "u2").unwrap_err().code, "undo-uncertain");
    }

    #[test]
    fn a_change_the_daws_undo_skips_stays_as_it_is_when_its_plan_is_undone() {
        let journal = Journal::default();
        let label = journal.step_label("Kumi");
        journal.step(Some(label.clone()));
        let (volume, repeat) = (preview(&journal), preview(&journal));
        journal.begin_apply(&volume.id, &volume.confirmation, "a", None).unwrap().unwrap();
        journal.end_apply(&volume.id, "a", Ok(&json!({"applied": {}, "undo": {"entry": null, "inPlan": true, "covered": true}})));
        journal.begin_apply(&repeat.id, &repeat.confirmation, "a", None).unwrap().unwrap();
        journal.end_apply(&repeat.id, "a", Ok(&json!({"applied": {}, "undo": {"entry": null, "inPlan": true, "covered": false}})));
        journal.step(None);
        let (undone, _) = journal.follow_daw(std::slice::from_ref(&label), true);
        assert_eq!(undone, vec![volume.id.clone()], "the repeat isn't in the plan's undo point");
        assert_eq!(journal.get(&repeat.id).unwrap().state, State::Applied);
    }

    #[test]
    fn a_kind_the_daws_undo_never_takes_back_is_never_counted_undone_answered_or_not() {
        let journal = Journal::for_daw(daw_bridge::Daw::Reaper);
        let label = journal.step_label("Kumi");
        journal.step(Some(label.clone()));
        let volume = preview(&journal);
        let repeat_answer = json!({"epoch": "e1", "target": {"ref": "r:project", "kind": "project"}, "prior": {"on": false}, "fence": {}, "display": {"title": "Repeat on"}});
        let unanswered = journal.previewed("loop-repeat", &json!({"on": true}), &repeat_answer, None, Pick::default()).unwrap();
        let unflagged = journal.previewed("loop-repeat", &json!({"on": true}), &repeat_answer, None, Pick::default()).unwrap();
        journal.begin_apply(&volume.id, &volume.confirmation, "a", None).unwrap().unwrap();
        journal.end_apply(&volume.id, "a", Ok(&json!({"applied": {}, "undo": {"entry": null, "inPlan": true}})));
        // One repeat is sent and never answered; the other is answered without saying whether the plan's point covers it.
        journal.begin_apply(&unanswered.id, &unanswered.confirmation, "a", None).unwrap().unwrap();
        journal.end_apply(&unanswered.id, "a", Err((&json!({"error": "REAPER didn't answer in time"}), true)));
        journal.begin_apply(&unflagged.id, &unflagged.confirmation, "a", None).unwrap().unwrap();
        journal.end_apply(&unflagged.id, "a", Ok(&json!({"applied": {"on": true}, "undo": {"entry": null, "inPlan": true}})));
        journal.step(None);
        let (undone, _) = journal.follow_daw(std::slice::from_ref(&label), true);
        assert_eq!(undone, vec![volume.id.clone()], "the plan's Cmd-Z takes back the volume only");
        assert_eq!(journal.get(&unanswered.id).unwrap().state, State::Uncertain);
        assert_eq!(journal.get(&unflagged.id).unwrap().state, State::Applied);
        // Renoise's mutes, solos and loops alike; its other kinds are in its undo.
        let renoise = Journal::for_daw(daw_bridge::Daw::Renoise);
        assert!(!renoise.previewed("track-mute", &json!({}), &repeat_answer, None, Pick::default()).unwrap().in_daw_undo);
        assert!(renoise.previewed("track-volume", &json!({}), &repeat_answer, None, Pick::default()).unwrap().in_daw_undo);
    }

    #[test]
    fn past_its_bytes_the_journal_lets_what_deletes_removed_go_first() {
        let journal = Journal::default();
        let small = preview(&journal);
        journal.begin_apply(&small.id, &small.confirmation, "a", None).unwrap().unwrap();
        journal.end_apply(&small.id, "a", Ok(&json!({"applied": {}})));
        let big = preview(&journal);
        journal.begin_apply(&big.id, &big.confirmation, "a", None).unwrap().unwrap();
        let material = "x".repeat(MAX_BYTES - 64 * 1024);
        journal.end_apply(&big.id, "a", Ok(&json!({"applied": {}, "material": {"chunk": material}})));
        let roomy = json!({"prior": {}, "fence": {}, "display": {"title": "x".repeat(70_000)}});
        assert!(journal.previewed("track-volume", &json!({}), &roomy, None, Pick::default()).is_ok());
        assert!(journal.get(&big.id).is_none() && journal.get(&small.id).is_some(), "the big delete went, the older small change stays");
    }

    #[test]
    fn past_its_bytes_size_decides_and_a_small_undone_record_stays() {
        let journal = Journal::default();
        // Thirty small changes Kumi undid (the DAW's undo of each undo step would bring it back), then a big delete.
        let small: Vec<Record> = (0..30)
            .map(|_| {
                let record = preview(&journal);
                journal.begin_apply(&record.id, &record.confirmation, "a", None).unwrap().unwrap();
                journal.end_apply(&record.id, "a", Ok(&json!({"applied": {}})));
                journal.begin_undo(&record.id, "u").unwrap().unwrap();
                journal.end_undo(&record.id, "u", Ok(&json!({"restored": {}})));
                record
            })
            .collect();
        let big = preview(&journal);
        journal.begin_apply(&big.id, &big.confirmation, "a", None).unwrap().unwrap();
        journal.end_apply(&big.id, "a", Ok(&json!({"applied": {}, "material": {"chunk": "x".repeat(MAX_BYTES - 64 * 1024)}})));
        let roomy = json!({"prior": {}, "fence": {}, "display": {"title": "x".repeat(70_000)}});
        assert!(journal.previewed("track-volume", &json!({}), &roomy, None, Pick::default()).is_ok());
        assert!(journal.get(&big.id).is_none(), "the big delete went");
        assert!(small.iter().all(|record| journal.get(&record.id).is_some()), "every small undone change stays");
    }

    #[test]
    fn past_its_bytes_the_largest_record_goes_first_whatever_small_material_older_ones_hold() {
        let journal = Journal::default();
        // Thirty adds and moves, each with a small material of its own (Renoise's `{track}`), then a big delete.
        let small: Vec<Record> = (0..30)
            .map(|_| {
                let record = preview(&journal);
                journal.begin_apply(&record.id, &record.confirmation, "a", None).unwrap().unwrap();
                journal.end_apply(&record.id, "a", Ok(&json!({"applied": {}, "material": {"track": 3}})));
                record
            })
            .collect();
        let big = preview(&journal);
        journal.begin_apply(&big.id, &big.confirmation, "a", None).unwrap().unwrap();
        journal.end_apply(&big.id, "a", Ok(&json!({"applied": {}, "material": {"chunk": "x".repeat(MAX_BYTES - 64 * 1024)}})));
        let roomy = json!({"prior": {}, "fence": {}, "display": {"title": "x".repeat(70_000)}});
        assert!(journal.previewed("track-volume", &json!({}), &roomy, None, Pick::default()).is_ok());
        assert!(journal.get(&big.id).is_none(), "the big delete went");
        assert!(small.iter().all(|record| journal.get(&record.id).is_some()), "every small change stays");
    }

    #[test]
    fn the_daws_own_undo_of_kumis_undo_puts_the_change_back_for_kumi_to_undo_again() {
        let journal = Journal::default();
        let record = preview(&journal);
        let Ok(Ok(applying)) = journal.begin_apply(&record.id, &record.confirmation, "a", None) else { panic!("applying") };
        journal.end_apply(&record.id, "a", Ok(&json!({"applied": {"volumeDb": -6.0}})));
        let own = applying.desc.unwrap();
        let Ok(Ok((_, restore))) = journal.begin_undo(&record.id, "u1") else { panic!("undoing") };
        journal.end_undo(&record.id, "u1", Ok(&json!({"restored": {"volumeDb": 0.0}})));
        // The producer's Cmd-Z of Kumi's undo (REAPER names the restore's step): the change is back.
        assert_eq!(journal.follow_daw(std::slice::from_ref(&restore), true), (vec![], vec![record.id.clone()]));
        assert_eq!(journal.get(&record.id).unwrap().state, State::Applied);
        // Kumi's next undo is sent again (the cached answer went), even with the same key.
        let Ok(Ok((_, restore))) = journal.begin_undo(&record.id, "u1") else { panic!("a fresh undo, not the old answer") };
        journal.end_undo(&record.id, "u1", Ok(&json!({"restored": {"volumeDb": 0.0}})));
        // Renoise tells the same as a redo of the change's own step; and the DAW's undo of the change itself takes it back.
        let (_, again) = journal.follow_daw(std::slice::from_ref(&own), false);
        assert_eq!((again, journal.get(&record.id).unwrap().state), (vec![record.id.clone()], State::Applied));
        let (back, _) = journal.follow_daw(std::slice::from_ref(&own), true);
        assert_eq!((back, journal.get(&record.id).unwrap().state), (vec![record.id.clone()], State::Undone));
        // The event Kumi gets says which were taken back and which put back.
        let data = undone_in_daw(&journal, "undone_in_daw", json!({"descs": [restore], "how": "undo"}));
        assert_eq!((&data["undone"], &data["redone"]), (&json!([]), &json!([record.id])));
    }

    // (r-tracks, NIT-5)
    #[test]
    fn a_jump_over_a_change_and_kumis_undo_of_it_leaves_it_undone_either_way() {
        let journal = Journal::default();
        let record = preview(&journal);
        let Ok(Ok(applying)) = journal.begin_apply(&record.id, &record.confirmation, "a", None) else { panic!("applying") };
        journal.end_apply(&record.id, "a", Ok(&json!({"applied": {"volumeDb": -9.0}})));
        let own = applying.desc.unwrap();
        let Ok(Ok((_, restore))) = journal.begin_undo(&record.id, "u1") else { panic!("undoing") };
        journal.end_undo(&record.id, "u1", Ok(&json!({"restored": {"volumeDb": 0.0}})));
        let both = [own.clone(), restore.clone()];
        // Back over both (REAPER's Undo History window): the change was never made, so it stays undone.
        assert_eq!(journal.follow_daw(&both, true), (vec![], vec![]));
        assert_eq!(journal.get(&record.id).unwrap().state, State::Undone);
        // Forward over both: made, then undone again by Kumi's step, which comes after it.
        assert_eq!(journal.follow_daw(&both, false), (vec![], vec![]));
        assert_eq!(journal.get(&record.id).unwrap().state, State::Undone);
        // Its own step alone still follows the DAW's undo and redo.
        assert_eq!(journal.follow_daw(std::slice::from_ref(&own), false), (vec![], vec![record.id.clone()]));
        assert_eq!(journal.follow_daw(&both, false), (vec![record.id.clone()], vec![]), "forward over both from applied: undone");
        let data = undone_in_daw(&journal, "undone_in_daw", json!({"descs": both, "how": "redo"}));
        assert_eq!((&data["undone"], &data["redone"]), (&json!([]), &json!([])));
        assert_eq!(journal.get(&record.id).unwrap().state, State::Undone);
    }

    #[test]
    fn lines_back_as_kumi_wrote_them_after_an_undo_are_unsure_and_kumi_may_undo_them_again() {
        let journal = Journal::default();
        let record = preview(&journal);
        let Ok(Ok(applying)) = journal.begin_apply(&record.id, &record.confirmation, "a", None) else { panic!("applying") };
        journal.end_apply(&record.id, "a", Ok(&json!({"applied": {"cells": []}})));
        let own = applying.desc.unwrap();
        journal.follow_daw(std::slice::from_ref(&own), true);
        assert_eq!(journal.get(&record.id).unwrap().state, State::Undone);
        // Renoise's undo past Kumi's step, or its redo of it: the bridge can't tell, so neither is told.
        let data = undone_in_daw(&journal, "undone_in_daw", json!({"descs": [own], "how": "unsure"}));
        assert_eq!((&data["undone"], &data["redone"], &data["unsure"]), (&json!([]), &json!([]), &json!([record.id])));
        // Kumi's undo is sent (the bridge reads back and decides), and the DAW's undo still takes it back.
        assert!(matches!(journal.begin_undo(&record.id, "u1"), Ok(Ok(_))));
        journal.end_undo(&record.id, "u1", Err(&json!({"error": "moved", "code": "changed"})));
        let data = undone_in_daw(&journal, "undone_in_daw", json!({"descs": [own], "how": "undo"}));
        assert_eq!(data["undone"], json!([record.id]));
    }

    #[test]
    fn the_daws_undo_settles_a_change_whose_undo_by_kumi_was_never_answered() {
        let journal = Journal::default();
        let record = preview(&journal);
        let Ok(Ok(applying)) = journal.begin_apply(&record.id, &record.confirmation, "a", None) else { panic!("applying") };
        journal.end_apply(&record.id, "a", Ok(&json!({"applied": {}})));
        let Ok(Ok((_, restore))) = journal.begin_undo(&record.id, "u1") else { panic!("undoing") };
        journal.end_undo_unanswered(&record.id, &json!({"error": "stopped part way", "code": "uncertain"}));
        assert_eq!(journal.begin_undo(&record.id, "u2").unwrap_err().code, "undo-uncertain");
        // The producer's undo takes Kumi's undo step back: it ran, and the change is back, so Kumi may undo it again.
        let (back, again) = journal.follow_daw(std::slice::from_ref(&restore), true);
        assert_eq!((back, again), (vec![], vec![record.id.clone()]));
        assert_eq!(journal.get(&record.id).unwrap().state, State::Applied);
        assert!(matches!(journal.begin_undo(&record.id, "u3"), Ok(Ok(_))), "a fresh undo is sent");
        journal.end_undo_unanswered(&record.id, &json!({"error": "stopped part way", "code": "uncertain"}));
        // Its own step undone takes it back for good, and the never-answered undo goes with it.
        journal.follow_daw(std::slice::from_ref(applying.desc.as_ref().unwrap()), true);
        let settled = journal.get(&record.id).unwrap();
        assert_eq!((settled.state, settled.undo_unanswered), (State::Undone, false));
    }

    #[test]
    fn the_daws_undo_event_names_kumis_changes_and_other_events_go_on_as_they_came() {
        let journal = Journal::default();
        let record = preview(&journal);
        let Ok(Ok(applying)) = journal.begin_apply(&record.id, &record.confirmation, "a", None) else { panic!() };
        journal.end_apply(&record.id, "a", Ok(&json!({"applied": {}})));
        let data =
            undone_in_daw(&journal, "undone_in_daw", json!({"descs": [applying.desc.unwrap(), "Adjust track volume"], "how": "undo"}));
        assert_eq!((&data["transactionIds"], &data["how"]), (&json!([record.id]), &json!("undo")));
        assert_eq!(undone_in_daw(&journal, "tracks", json!({"count": 3})), json!({"count": 3}));
    }

    #[test]
    fn the_daws_undo_of_a_plan_names_the_changes_made_in_its_step() {
        let journal = Journal::default();
        let label = journal.step_label("Tighten the drums");
        assert!(label.starts_with("Kumi: Tighten the drums (k") && label.ends_with(')'), "{label}");
        journal.step(Some(label.clone()));
        let (first, second) = (preview(&journal), preview(&journal));
        for record in [&first, &second] {
            journal.begin_apply(&record.id, &record.confirmation, "a", None).unwrap().unwrap();
            journal.end_apply(&record.id, "a", Ok(&json!({"applied": {}})));
        }
        // Named, as it closes, by what it did: the nonce stays, and the plan's changes are found by the new name.
        let named = journal.step_named("Bass volume −6 dB and 1 more").unwrap();
        journal.name_step(&named);
        assert_eq!(named, format!("Kumi: Bass volume −6 dB and 1 more{}", &label[label.rfind(" (k").unwrap()..]));
        journal.step(None);
        assert_eq!(journal.step_named("nothing open"), None);
        let after = preview(&journal);
        journal.begin_apply(&after.id, &after.confirmation, "a", None).unwrap().unwrap();
        journal.end_apply(&after.id, "a", Ok(&json!({"applied": {}})));
        assert_eq!(journal.by_descs(std::slice::from_ref(&named)), vec![first.id, second.id], "the plan's changes, not the one after");
        assert!(journal.by_descs(std::slice::from_ref(&label)).is_empty(), "the name it began with is gone");
    }

    #[test]
    fn the_daws_own_undo_finds_its_changes_by_their_step_description() {
        let journal = Journal::default();
        let record = preview(&journal);
        let Ok(Ok(applying)) = journal.begin_apply(&record.id, &record.confirmation, "a", Some("Bass −6 dB")) else { panic!() };
        journal.end_apply(&record.id, "a", Ok(&json!({"applied": {}})));
        let found = journal.by_descs(std::slice::from_ref(applying.desc.as_ref().unwrap()));
        assert_eq!(found, vec![record.id.clone()]);
        assert!(journal.by_descs(&["Kumi: something else (kzzzz)".into()]).is_empty());
    }
}
