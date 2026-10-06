//! The fresh, bounded Set context at the start of a turn.
use super::{
    connection::{LiveConnection, ReadError},
    context::{self, ObservationError, FIELDS, INSTRUCTIONS},
    fold::fold_tracks,
    inference::no_access,
    more_changes::{set_meter, set_scale},
    pins::pin_context,
    project,
    remember::{CurrentProject, Remember},
    set_model::{DeviceNode, SetModel},
    track_ids,
    views::{self, ViewHost},
};
use crate::{
    core::{contracts::*, errors::RuntimeError, timing},
    library::taste,
    mcp::types::CallToolResult,
};
use async_trait::async_trait;
use indexmap::{IndexMap, IndexSet};
use kumi_common::{
    abort::{self, Signal, SignalExt},
    js::{
        json::stringify,
        number::to_string,
        string::{head, trim},
    },
};
use regex::Regex;
use serde_json::{json, Value};
use std::{
    cell::{Cell, RefCell},
    collections::{BTreeMap, BTreeSet, HashMap, HashSet},
    future::Future,
    rc::Rc,
    sync::LazyLock,
    time::UNIX_EPOCH,
};
const MIXER_TRACKS: usize = 64;
/// The kept devices are reused for this long after they were read whole (or checked by the backstop), and older ones
/// are read whole again: Live tells of no device renamed or rack edited.
const REUSE_MS: i64 = 60_000;
/// The backstop: after this many turns reused the kept devices, a whole read beside the turns checks them, and counts
/// what Live's events missed.
const REUSES: u32 = 10;
/// What the observation reads of each device, the Set's at once or one parent's.
const DEVICE_FIELDS: [&str; 4] = ["parentRef", "name", "className", "chainList"];
/// The Set's devices as a turn last had them whole, for the turns after it to reuse while Live tells of no change.
#[derive(Clone)]
struct KeptDevices {
    epoch: f64,
    identity: String,
    /// The track page's revision they were read with: a track added, removed or moved renumbers their refs.
    tracks: Option<String>,
    /// The track rows read with them, for the Set model when some devices are read again between turns.
    track_rows: Vec<JsonObject>,
    rows: Vec<JsonObject>,
    /// Live's structure events heard, and the endpoints attached, when they were read whole.
    structure: u64,
    attachments: u64,
    /// Turns that reused them since, and when they were read whole (the session's clock, in ms).
    reused: u32,
    at: i64,
    /// Tracks (refs) whose devices Kumi changed since: read again on the next turn.
    changed: IndexSet<String>,
}
/// The kept devices and the work around them, shared with the backstop read that runs beside the turns.
#[derive(Default)]
struct Devices {
    kept: Option<KeptDevices>,
    /// Tracks read again this turn before a change acted on them: once a turn is enough.
    refreshed: IndexSet<String>,
    /// The backstop read is running.
    refreshing: bool,
    /// How many rows the last backstop read found changed from the kept ones, for the next turn's timing.
    drift: Option<u32>,
    /// What the turns have shown of each device and chain, by ref, the latest over the earlier (a ref from an earlier
    /// turn is checked as it was shown then): a change is checked against it. For one Set in one epoch.
    shown: HashMap<String, Shown>,
    shown_in: Option<(f64, String)>,
}
/// A device's (or a chain's) name and class (a chain has none), as rows show it.
type Shown = (String, Option<String>);
#[derive(Clone)]
pub struct ObservedChange {
    pub record: ChangeRecord,
    pub within: bool,
}
/// State owned by the integration's change/audition engines, needed while making a fresh observation.
#[async_trait(?Send)]
pub trait ObservationHost {
    fn reset_turn(&self, continuing: bool);
    fn changes(&self) -> Vec<ObservedChange>;
    fn definitions(&self) -> Vec<Rc<dyn KernelTool>>;
    async fn restore_after_crash(&self, identity: &str, path: Option<&str>, signal: Signal) -> Result<Option<String>, RuntimeError>;
}
#[derive(Clone)]
struct Previous {
    key: String,
    identity: String,
    path: Option<String>,
    project: Option<ProjectRef>,
}
pub struct Observer {
    pub connection: Rc<LiveConnection>,
    pub remember: Rc<Remember>,
    pub tempo: Cell<Option<f64>>,
    pub beats_per_bar: Cell<f64>,
    pub last_track_count: Cell<usize>,
    previous: RefCell<Option<Previous>>,
    /// The Set as the last observation read it.
    model: RefCell<Rc<SetModel>>,
    /// The Set's devices kept between turns.
    devices: Rc<RefCell<Devices>>,
    /// The Set's name, tempo, meter and scale as the last observation read them, for its fingerprint.
    song: RefCell<Option<Value>>,
}
impl Observer {
    pub fn new(connection: Rc<LiveConnection>, remember: Rc<Remember>) -> Self {
        Self {
            connection,
            remember,
            tempo: Cell::new(None),
            beats_per_bar: Cell::new(4.),
            last_track_count: Cell::new(0),
            previous: RefCell::new(None),
            model: RefCell::new(Rc::new(SetModel::default())),
            devices: Rc::new(RefCell::new(Devices::default())),
            song: RefCell::new(None),
        }
    }
    /// The Set as the last observation read it: its tracks and their devices, found by id or name.
    pub fn model(&self) -> Rc<SetModel> {
        self.model.borrow().clone()
    }
    /// The Set's fingerprint, as Kumi logs it with the producer's reactions (`fingerprint_of`).
    pub fn fingerprint(&self) -> Option<Value> {
        Some(fingerprint_of(self.song.borrow().clone()?, &self.model()))
    }
    /// The tracks weren't read this turn: what the model holds may be out of date.
    fn model_stale(&self) {
        if self.model.borrow().complete {
            let mut stale = (**self.model.borrow()).clone();
            stale.complete = false;
            *self.model.borrow_mut() = Rc::new(stale);
        }
    }
    /// What a change names of the Set's devices: the tracks they're on (by a device's or a chain's ref, or by the
    /// track's own when `tracks_too`, a device loaded onto a track), and the devices and chains themselves.
    pub fn devices_named(&self, input: &JsonObject, tracks_too: bool) -> (Vec<String>, Vec<String>) {
        let long = self.connection.references.borrow().lengthen(&Value::Object(input.clone()));
        let (mut tracks, mut named) = (IndexSet::new(), IndexSet::new());
        named_devices(&long, tracks_too, &mut tracks, &mut named, 0);
        (tracks.into_iter().collect(), named.into_iter().collect())
    }
    /// Before a change acts on these tracks' devices: reads them again into the kept ones (once a turn each), so what
    /// Kumi holds of them (the Set model's names and chains among it) is current where it acts. Refuses the change
    /// when a device or chain it names isn't what this turn showed: device refs are places, so a device swapped in a
    /// rack keeps the ref.
    pub async fn refresh_devices(&self, tracks: &[String], named: &[String], signal: Signal) -> Result<(), String> {
        let pending = {
            let mut devices = self.devices.borrow_mut();
            let tracks: IndexSet<String> = tracks.iter().filter(|track| devices.refreshed.insert((*track).clone())).cloned().collect();
            match devices.kept.as_ref() {
                None => return Ok(()),
                Some(kept) => (!tracks.is_empty()).then(|| (kept.rows.clone(), kept.epoch, tracks)),
            }
        };
        if let Some((rows, epoch, tracks)) = pending {
            let read = reread(&self.connection, rows, &tracks, epoch, signal).await;
            let mut devices = self.devices.borrow_mut();
            match (read, devices.kept.as_mut().filter(|kept| kept.epoch == epoch)) {
                (Ok(rows), Some(kept)) => {
                    let previous = self.model();
                    *self.model.borrow_mut() = Rc::new(SetModel::next(&previous, &kept.track_rows, &rows, previous.complete));
                    kept.rows = rows;
                }
                // Not read again (the bridge checks the change itself): the next turn reads them all.
                _ => {
                    devices.kept = None;
                    return Ok(());
                }
            }
        }
        let devices = self.devices.borrow();
        let Some(kept) = devices.kept.as_ref() else { return Ok(()) };
        for reference in named {
            let Some((name, class)) = devices.shown.get(reference) else { continue };
            match describe(&kept.rows, reference) {
                None => {
                    return Err(format!(
                        "{name} isn't on the track any more (its devices changed since Kumi read them), so nothing was changed; read the track's devices again"
                    ))
                }
                Some((now, now_class)) if now != *name || now_class != *class => {
                    return Err(format!(
                        "{name} is now {now} (the track's devices changed since Kumi read them), so nothing was changed; read the track's devices again"
                    ))
                }
                Some(_) => {}
            }
        }
        Ok(())
    }
    /// Kumi changed these tracks' devices: the next turn reads them again, since Live tells of no device renamed.
    pub fn devices_changed(&self, tracks: &[String]) {
        if let Some(kept) = self.devices.borrow_mut().kept.as_mut() {
            kept.changed.extend(tracks.iter().cloned());
        }
    }
    /// Something may have changed any device (Python run in Live, an undo): the next turn reads them all.
    pub fn forget_devices(&self) {
        self.devices.borrow_mut().kept = None;
    }
    /// The backstop for Live's events: the Set's devices read whole beside the turns, the rows found changed from
    /// the kept ones counted (the drift the next turn's timing shows), and the read kept in their place.
    fn backstop(&self, args: JsonObject) {
        let (devices, connection) = (self.devices.clone(), self.connection.clone());
        tokio::task::spawn_local(timing::background(async move {
            let structure = connection.structure_events.get();
            let read = views::pages(connection.as_ref(), args, connection.lifetime.clone()).await;
            let mut devices = devices.borrow_mut();
            devices.refreshing = false;
            let Some(kept) = devices.kept.as_ref() else { return };
            let Ok(Ok(Some((rows, false)))) = read.map(|read| device_page(&read, kept.epoch)) else { return };
            let drift = drift(&kept.rows, &rows, &kept.changed);
            let unchanged = connection.structure_events.get() == structure
                && kept.structure == structure
                && connection.attachments.get() == kept.attachments;
            devices.drift = Some(drift);
            match devices.kept.as_mut() {
                Some(kept) if unchanged => {
                    kept.rows = rows;
                    kept.reused = 0;
                    kept.at = connection.now().timestamp_millis();
                }
                _ => devices.kept = None,
            }
        }));
    }
    fn away(&self) -> Observation {
        self.model_stale();
        let previous = self.previous.borrow();
        no_access(
            &previous.as_ref().map(|p| p.key.clone()).unwrap_or_else(|| format!("{}:no-live", self.connection.generation)),
            self.connection.now(),
            previous.as_ref().filter(|p| p.path.is_some()).and_then(|p| p.project.clone()),
        )
    }
    pub async fn observe(
        &self,
        host: &dyn ObservationHost,
        original: Signal,
        hints: Option<ObserveHints>,
    ) -> Result<Observation, RuntimeError> {
        let connection = &self.connection;
        let signal = abort::any([original, connection.lifetime.clone()]);
        signal.check()?;
        if !connection.started.get() || connection.closed.get() {
            return Err(ObservationError("Integration is not open".into()).into());
        }
        connection.invalidate();
        let lease = connection.lease.get();
        host.reset_turn(hints.as_ref().is_some_and(|h| h.continuing == Some(true)));
        if !connection.available.get() || connection.lost.get() {
            return Ok(self.away());
        }
        let result: Result<Observation, ReadError> = async {
            connection.tools().unwrap().refresh(signal.clone()).await?;
            connection.assert_lease(lease, &signal)?;
            connection.ensure_catalog(signal.clone()).await?;
            connection.assert_lease(lease, &signal)?;

            if !connection.has("live_status") {
                return Err(ObservationError("Live status capability is unavailable".into()).into());
            }
            let subscribe = connection.clone();
            let subscribing = signal.clone();
            start_background(async move {
                subscribe.subscribe(subscribing).await;
            })
            .await;

            let mut fields = FIELDS["set"].as_array().unwrap().clone();
            fields.push(json!("filePath"));
            let set_args = context::discovery_args(&object(json!({"kind":"set","fields":fields})))?;

            let mut fields = vec!["name", "kind", "mediaKind", "groupTrackRef"];
            if self.remember.track_ids.reported(connection.epoch.get()) {
                fields.push("kumiTrack");
            }
            if self.last_track_count.get() <= MIXER_TRACKS {
                fields.push("mixer");
            }
            let track_args = context::discovery_args(&object(
                json!({"kind":"track","fields":fields,"limit":connection.page_limit(),"budget":connection.whole_budget()}),
            ))?;

            let device_args = context::discovery_args(&object(
                json!({"kind":"device","fields":DEVICE_FIELDS,"limit":connection.page_limit(),"budget":connection.whole_budget()}),
            ))?;

            let selection_args = context::discovery_args(&object(json!({"kind":"selection","limit":1})))?;

            // The Set's devices are kept from the last whole read while Live tells of no structure change (checked
            // again with what they were read with once this turn's reads are in); otherwise they're read whole, and
            // the kept ones are dropped at once (a gap in Live's events may lie behind them).
            let structure = connection.structure_events.get();
            let reusable = {
                let mut devices = self.devices.borrow_mut();
                devices.refreshed.clear();
                let reusable = connection.hears_live()
                    && devices.kept.as_ref().is_some_and(|kept| {
                        kept.structure == structure
                            && kept.attachments == connection.attachments.get()
                            && connection.now().timestamp_millis() - kept.at <= REUSE_MS
                    });
                if !reusable {
                    devices.kept = None;
                }
                reusable
            };

            // Calling the async song read starts it immediately in JavaScript. A synchronous disconnect
            // from that dispatch invalidates the catalog before discovery availability is captured.
            let mut song_future = Box::pin(async {
                if connection.has("live_song_state") {
                    Some(
                        async {
                            Ok::<_, ReadError>(context::payload(&connection.call("live_song_state", JsonObject::new(), signal.clone()).await?)?)
                        }
                        .await,
                    )
                } else {
                    None
                }
            });

            let song_ready = std::future::poll_fn(|cx| {
                std::task::Poll::Ready(match song_future.as_mut().poll(cx) {
                    std::task::Poll::Ready(value) => Some(value),
                    std::task::Poll::Pending => None,
                })
            })
            .await;

            let discover = connection.has("live_discover");

            let read = |args: JsonObject| {
                let signal = signal.clone();
                async move {
                    if !discover {
                        return Err(ReadError::Observation(ObservationError("Required Set discovery capability is unavailable".into())));
                    }
                    Ok(if matches!(args.get("kind").and_then(Value::as_str), Some("track" | "device")) {
                        views::pages(connection.as_ref(), args, signal).await?
                    } else {
                        connection.call("live_discover", args, signal).await?
                    })
                }
            };

            // All six reads start before a result is interpreted. Failures remain in their own result slots.
            let (song_read, status_read, set_read, tracks_read, devices_read, selection_read) = futures::join!(
                async {
                    match song_ready {
                        Some(value) => value,
                        None => song_future.await,
                    }
                },
                async { Ok::<_, ReadError>(context::status_payload(&connection.call("live_status", JsonObject::new(), signal.clone()).await?)?) },
                read(set_args.clone()),
                read(track_args.clone()),
                async {
                    if reusable {
                        None
                    } else {
                        Some(read(device_args.clone()).await)
                    }
                },
                read(selection_args)
            );

            connection.assert_lease(lease, &signal)?;
            let status = status_read?;
            if status.get("connected") != Some(&Value::Bool(true)) {
                connection.lose_live();
                return Ok(self.away());
            }
            if !discover {
                return Err(ObservationError("Required Set discovery capability is unavailable".into()).into());
            }
            let set_read = set_read?;
            let epoch = status["epoch"].as_f64().unwrap();
            connection.assert_epoch(context::payload(&set_read)?.get("epoch"), epoch)?;

            let page = context::discovery_payload(&set_read, "set", epoch)?;
            let set_rows = objects(&page)?;
            if set_rows.len() != 1 {
                return Err(ObservationError("Current Set discovery did not return one authoritative Set".into()).into());
            }
            let row = &set_rows[0];
            let identity = context::set_identity(row)?;

            connection.epoch.set(Some(epoch));
            *connection.set.borrow_mut() = Some(identity.clone());
            connection.last_epoch.set(Some(epoch));
            self.tempo.set(row.get("tempo").and_then(Value::as_f64));

            let song = song_read.and_then(Result::ok);
            let numerator = song.as_ref().and_then(|s| s.get("signatureNumerator")).and_then(Value::as_f64).unwrap_or(4.);
            let denominator = song.as_ref().and_then(|s| s.get("signatureDenominator")).and_then(Value::as_f64).unwrap_or(4.);
            set_meter(numerator, denominator);
            self.beats_per_bar.set(numerator * 4. / denominator);

            connection.register_rows("set", &set_rows, &set_args, cursor(&page))?;

            // The devices as this turn's tracks have them: read whole this turn, or the kept ones when what they were
            // read with is unchanged, with the selected track's and those Kumi changed read again.
            let mut reusing: Option<KeptDevices> = None;
            let devices_read: Result<Option<(Vec<JsonObject>, bool)>, ReadError> = match devices_read {
                Some(read) => read.and_then(|read| device_page(&read, epoch)),
                None => {
                    let revision = tracks_read
                        .as_ref()
                        .ok()
                        .filter(|read| read.is_error != Some(true))
                        .and_then(|read| context::payload(read).ok())
                        .and_then(|page| page.get("revision").and_then(Value::as_str).map(str::to_owned));
                    // Out of the store while this turn has them: a turn that fails part way leaves none to reuse.
                    let kept = self.devices.borrow_mut().kept.take();
                    let again = match kept.filter(|kept| kept.epoch == epoch && kept.identity == identity && revision.is_some() && kept.tracks == revision) {
                        Some(mut kept) => {
                            let tracks: IndexSet<String> = selected_track(&selection_read, epoch).into_iter().chain(kept.changed.iter().cloned()).collect();
                            let rows = reread(connection, std::mem::take(&mut kept.rows), &tracks, epoch, signal.clone()).await.ok();
                            reusing = rows.is_some().then_some(kept);
                            rows
                        }
                        None => None,
                    };
                    match again {
                        Some(rows) => Ok(Some((rows, false))),
                        None => read(device_args.clone()).await.and_then(|read| device_page(&read, epoch)),
                    }
                }
            };
            connection.assert_lease(lease, &signal)?;

            let mut track_list = None;
            let mut more_tracks = false;
            // What the tracks read says about their ids: the list's revision, and the tracks whose id is missing or shared.
            let mut track_revision: Option<String> = None;
            let mut track_gaps: Vec<String> = vec![];
            // The rows the Set model is built from, as read.
            let mut model_rows: Option<(Vec<JsonObject>, Option<Vec<JsonObject>>)> = None;
            let mut more_devices = false;

            let tracks_result: Result<(), ReadError> = (|| {
                let read = tracks_read?;
                if read.is_error == Some(true) {
                    return Ok(());
                }
                let page = context::discovery_payload(&read, "track", epoch)?;
                let rows = objects(&page)?;
                track_revision = page.get("revision").and_then(Value::as_str).map(str::to_owned);
                track_gaps = track_ids::gaps(&rows);
                model_rows = Some((rows.clone(), None));
                connection.register_rows("track", &rows, &track_args, cursor(&page))?;

                let mut shown = rows.iter().map(|row| track_row(connection, row)).collect::<Result<Vec<_>, _>>()?;

                self.last_track_count.set(rows.len() + if cursor(&page).is_some() { MIXER_TRACKS } else { 0 });
                more_tracks = cursor(&page).is_some() || page.get("truncated") == Some(&Value::Bool(true));

                let devices_result: Result<(), ReadError> = (|| {
                    let Some((devices, more)) = devices_read? else { return Ok(()) };
                    connection.register_rows("device", &devices, &device_args, None)?;
                    if let Some((_, read)) = model_rows.as_mut() {
                        *read = Some(devices.clone());
                    }

                    add_devices(connection, &rows, &mut shown, &devices)?;
                    more_devices = more;
                    Ok(())
                })();
                if let Err(error) = devices_result {
                    if lease != connection.lease.get() {
                        return Err(error);
                    }
                }
                track_list = Some(shown);
                Ok(())
            })();
            if let Err(error) = tracks_result {
                if lease != connection.lease.get() {
                    return Err(error);
                }
                track_list = None;
            }
            if let Some((tracks, devices)) = &model_rows {
                let complete = devices.is_some() && !more_tracks && !more_devices;
                let next = SetModel::next(&self.model.borrow(), tracks, devices.as_deref().unwrap_or(&[]), complete);
                *self.model.borrow_mut() = Rc::new(next);
            } else {
                self.model_stale();
            }
            // What the next turns may reuse: the devices as this turn has them, when it has them all.
            // Kept only while Live's events are heard, since nothing else says when to read them again.
            let whole = model_rows.and_then(|(tracks, devices)| Some((tracks, devices?))).filter(|_| !more_tracks && !more_devices && connection.hears_live());
            let reused = reusing.is_some() && whole.is_some();
            let due = {
                let mut devices = self.devices.borrow_mut();
                devices.kept = whole.map(|(track_rows, rows)| match reusing {
                    Some(kept) => KeptDevices { track_rows, rows, reused: kept.reused + 1, changed: IndexSet::new(), ..kept },
                    None => KeptDevices {
                        epoch,
                        identity: identity.clone(),
                        tracks: track_revision.clone(),
                        track_rows,
                        rows,
                        structure,
                        attachments: connection.attachments.get(),
                        reused: 0,
                        at: connection.now().timestamp_millis(),
                        changed: IndexSet::new(),
                    },
                });
                timing::set_devices(reused, devices.drift.take());
                // Without kept devices nothing is checked, and what a turn showed meanwhile isn't recorded: start over.
                let seen = devices.kept.as_ref().map(|kept| shown(&kept.rows));
                if seen.is_none() || devices.shown_in.as_ref() != Some(&(epoch, identity.clone())) {
                    devices.shown.clear();
                    devices.shown_in = Some((epoch, identity.clone()));
                }
                devices.shown.extend(seen.unwrap_or_default());
                let due = !devices.refreshing && devices.kept.as_ref().is_some_and(|kept| kept.reused >= REUSES);
                devices.refreshing |= due;
                due
            };
            if due {
                self.backstop(device_args.clone());
            }
            let selected: Option<JsonObject> = (|| {
                let read = selection_read.ok()?;
                if read.is_error == Some(true) {
                    return None;
                }
                let page = context::discovery_payload(&read, "selection", epoch).ok()?;
                let row = page.get("items")?.as_array()?.first()?;
                let reference = row.get("selectedTrackRef")?.as_str()?;
                let track = connection.references.borrow().known.get(reference)?.clone();
                let reference = connection.references.borrow_mut().short_ref(reference);

                Some(object(
                    json!({"track":{"ref":reference,"name":track.name},"note":"\"This track\" or \"here\" in the producer's words means this one, unless they pointed at something in Kumi."}),
                ))
            })();

            connection.notify_connected();
            let restored = host.restore_after_crash(&identity, row.get("filePath").and_then(Value::as_str), signal.clone()).await?;
            connection.assert_lease(lease, &signal)?;

            let name = row
                .get("name")
                .and_then(Value::as_str)
                .filter(|s| !trim(s).is_empty())
                .map(|s| head(s, 256))
                .unwrap_or_else(|| "(unnamed/unsaved)".into());

            let project = self.remember.current();
            let new_set = !project.as_ref().is_some_and(|p| p.identity == identity);
            let mut path = if new_set { None } else { project.as_ref().and_then(|p| p.path.clone()) };
            // Never saved, as Live says (a saved Set whose file can't be found now has no path either).
            let mut unsaved = !new_set && project.as_ref().is_some_and(|p| p.unsaved);

            if new_set || project.as_ref().is_some_and(|p| p.name != name) {
                if row.contains_key("filePath") {
                    let file = row.get("filePath").and_then(Value::as_str).filter(|s| !s.is_empty());
                    unsaved = file.is_none();
                    path = file.filter(|s| std::path::Path::new(s).exists()).map(str::to_owned);
                } else {
                    (path, unsaved) = self.remember.project_place(signal.clone()).await;
                    connection.assert_lease(lease, &signal)?;
                }
            }
            let previous = self.previous.borrow().clone();
            let other_file = previous.as_ref().and_then(|p| p.path.as_ref()).zip(path.as_ref()).is_some_and(|(a, b)| a != b);
            let continues = previous.as_ref().is_some_and(|p| p.identity == identity || (connection.reconnected.get() && !other_file));

            let key = if continues { previous.as_ref().unwrap().key.clone() } else { stringify(&json!([connection.generation, epoch, identity])) };
            let after_reconnect = connection.reconnected.replace(false);

            let pinned = if let Some(pin) = hints.as_ref().and_then(|h| h.pinned.as_ref()) {
                let tree = if pin.live == Some(true) {
                    None
                } else {
                    views::device_tree(connection.as_ref(), &pin.track_ref, signal.clone()).await.ok().flatten()
                };
                Some(pin_context(pin, connection.epoch.get(), tree.as_ref(), &mut connection.references.borrow_mut()))
            } else {
                None
            };
            connection.assert_lease(lease, &signal)?;

            // The saved Set's project: decided once for a Set at a path, from the id kept inside it.
            let known_project = project.as_ref().filter(|p| p.identity == identity && p.path == path).and_then(|p| p.project.clone());
            let project_id = match (&path, known_project) {
                (Some(_), Some(known)) => Some(known),
                (Some(path), None) => Some(self.remember.project_of(&identity, path, signal.clone()).await),
                (None, _) => None,
            };
            connection.assert_lease(lease, &signal)?;
            *self.remember.current.borrow_mut() =
                Some(Rc::new(CurrentProject { identity: identity.clone(), name: name.clone(), path: path.clone(), project: project_id.clone(), unsaved }));
            // What an unsaved Set kept is written to its history once it has a project id.
            self.remember.history.identified(self.remember.store.as_ref(), self.remember.current().as_deref());
            // The tracks' own ids, made whole in the background when their list changed or one is missing or
            // shared: within the project (or the unsaved Set), and never written into a template.
            let scope = project_id.clone().unwrap_or_else(|| format!("unsaved:{identity}"));
            let writable = path.as_deref().is_none_or(|path| !project::template_location(path));
            let structure = connection.structure_events.get();
            let gaps = if writable { track_gaps } else { vec![] };
            if self.remember.track_ids.due(&scope, track_revision.as_deref(), &gaps, structure) {
                let remember = self.remember.clone();
                let lifetime = connection.lifetime.clone();
                let keepers = project_id.clone().zip(self.remember.store.clone()).map(|(project, store)| track_ids::Keepers { project, store });
                tokio::task::spawn_local(async move {
                    let _ = remember.track_ids.pass(&remember.connection, &scope, keepers, writable, structure, lifetime).await;
                });
            }
            let project_ref = project_id.map(|id| ProjectRef { id, name: name.clone() });
            *self.previous.borrow_mut() =
                Some(Previous { key: key.clone(), identity: identity.clone(), path: path.clone(), project: project_ref.clone() });

            if new_set {
                self.remember.catch_up(identity.clone(), name.clone(), after_reconnect);
            } else if kumi_common::time::now_ms() - self.remember.last_saved.get() > 5 * 60_000 {
                self.remember.schedule_save(1000);
            }
            connection.ensure_catalog(signal.clone()).await?;
            connection.assert_lease(lease, &signal)?;

            let provenance = status.get("provenance").and_then(Value::as_str).unwrap_or("unknown");
            let source = if provenance == "real-live" && status.get("adapter").and_then(Value::as_str) == Some("remote-script") {
                String::from("Remote Script · real-live")
            } else {
                format!("unverified/synthetic fixture · {provenance}")
            };

            let mut focus_refs = HashSet::new();
            if let Some(selected) = &selected {
                focus_refs.insert(selected["track"]["ref"].as_str().unwrap().to_owned());
            }
            if let Some(pin) = hints.as_ref().and_then(|h| h.pinned.as_ref()) {
                focus_refs.insert(connection.references.borrow_mut().short_ref(&pin.track_ref));
            }
            let changes = host.changes();
            let recent: IndexSet<_> =
                changes.iter().rev().filter_map(|c| c.record.track.as_ref().map(|t| t.name.clone()).filter(|s| !s.is_empty())).collect();

            for name in recent.iter().take(4) {
                if let Some(track) =
                    track_list.as_ref().and_then(|tracks| tracks.iter().find(|t| t.get("name").and_then(Value::as_str) == Some(name)))
                {
                    focus_refs.insert(js_string(track.get("ref")));
                }
            }
            let shown = track_list.as_ref().map(|tracks| fold_tracks(tracks, |t| focus_refs.contains(&js_string(t.get("ref"))), None));

            let mut context = object(json!({"observedAt":connection.iso_now(),"connectionGeneration":connection.generation,"epoch":epoch}));
            if let Some(adapter) = status.get("adapter") {
                context.insert("adapter".into(), adapter.clone());
            }
            context.insert("provenance".into(), json!(provenance));

            let version = match status.get("environment") {
                Some(v) if v.is_object() || v.is_array() => context::object(v)?.get("liveVersion").cloned().unwrap_or(Value::Null),
                _ => Value::Null,
            };
            context.insert("liveVersion".into(), version);

            let mut set = JsonObject::new();
            if let Some(reference) = row.get("ref") {
                set.insert(
                    "ref".into(),
                    reference.as_str().map(|r| json!(connection.references.borrow_mut().short_ref(r))).unwrap_or_else(|| reference.clone()),
                );
            }
            set.insert("name".into(), json!(name));
            set.insert("tempo".into(), row.get("tempo").cloned().unwrap_or(Value::Null));
            set.insert("timeSignature".into(), json!(format!("{}/{}", to_string(numerator), to_string(denominator))));
            let scale = scale(row);
            set_scale(scale.as_ref().and_then(|(_, key)| key.clone()));
            if let Some((shown, _)) = scale {
                set.insert("scale".into(), json!(shown));
            }

            for key in ["playing", "position", "loop"] {
                set.insert(key.into(), row.get(key).cloned().unwrap_or(Value::Null));
            }
            if let Some(song) = &song {
                set.insert("recording".into(),json!({"session":song.get("sessionRecord")==Some(&Value::Bool(true)),"arrangement":row.get("recording").cloned().unwrap_or(Value::Null)}));
                set.insert("swing".into(), song.get("swingAmount").cloned().unwrap_or(Value::Null));
            }
            *self.song.borrow_mut() = Some(json!({"name":set.get("name"),"tempo":set.get("tempo"),"meter":set.get("timeSignature"),"scale":set.get("scale")}));
            context.insert("set".into(), Value::Object(set));

            if let Some(shown) = shown {
                context.insert("tracks".into(), json!(shown.tracks));
                if let Some(folded) = shown.folded {
                    context.insert("folded".into(), json!(folded));
                }
                if more_tracks || shown.more_tracks.is_some() {
                    context.insert(
                        "moreTracks".into(),
                        json!(shown.more_tracks.unwrap_or_else(|| "More tracks than listed; discover the rest".into())),
                    );
                }
                if more_devices {
                    context.insert("moreDevices".into(), json!("Not every device is listed; discover a track's devices"));
                }
            }
            if let Some(since) = self.remember.context.borrow().clone() {
                context.insert("sinceLastTime".into(), Value::Object(since));
            }
            if let Some(pinned) = pinned {
                context.insert("pinned".into(), Value::Object(pinned));
            }
            if let Some(selected) = selected {
                context.insert("selectedInLive".into(), Value::Object(selected));
            }
            if let Some(restored) = restored.filter(|s| !s.is_empty()) {
                context.insert("restoredAfterCrash".into(), json!(restored));
            }
            if after_reconnect {
                context.insert("reconnected".into(),json!("Kumi reconnected to Live since your last answer, so every reference from earlier answers (track:…, device:…, clip:… and the like) is gone. Use the ones listed here, or discover again."));
            }
            if !changes.is_empty() {
                let visible: Vec<_> = changes.iter().filter(|c| !c.within).collect();
                context.insert(
                    "kumiChanges".into(),
                    json!(visible
                        .iter()
                        .skip(visible.len().saturating_sub(12))
                        .map(|c| {
                            let mut row = object(json!({"change":c.record.id,"what":c.record.title,"state":c.record.state}));
                            if let Some(note) = c.record.note.as_ref().filter(|s| !s.is_empty()) {
                                row.insert("note".into(), json!(note));
                            }
                            row
                        })
                        .collect::<Vec<_>>()),
                );
            }
            context.insert("truncated".into(), page["truncated"].clone());
            if let Some(cursor) = cursor(&page) {
                context.insert("nextCursor".into(), json!(cursor));
            }
            context.insert("coverage".into(),json!("Current open Set only. Bounded discovery; details and track counts require fresh paged reads. Names/paths are not durable identity."));

            let saved_at = path
                .as_ref()
                .and_then(|p| std::fs::metadata(p).ok())
                .and_then(|m| m.modified().ok())
                .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
                .map(|t| t.as_secs() as f64 * 1000. + t.subsec_nanos() as f64 / 1_000_000.);

            Ok(Observation {
                key,
                revision: Some(connection.tools().unwrap().generation().to_string()),
                label: format!("Current open Set: {name} — {source}"),
                instructions: INSTRUCTIONS.into(),
                tools: host.definitions(),
                project: project_ref,
                tracks: track_list
                    .filter(|_| !more_tracks)
                    .map(|tracks| tracks.iter().filter_map(|t| t.get("name").and_then(Value::as_str).map(str::to_owned)).collect()),
                saved_at,
                context: stringify(&Value::Object(context)),
            })
        }.await;

        result.map_err(|error| {
            if lease != connection.lease.get() {
                return RuntimeError::plain("Observation changed; late refresh discarded");
            }
            // The Set wasn't read through (Live switching Sets, say): the model may still hold the old one.
            self.model_stale();
            connection.discard_reads();
            connection.epoch.set(None);
            match error {
                ReadError::Observation(error) => error.into(),
                ReadError::Other(_) => RuntimeError::plain("Live observation refresh failed; old observations are not current"),
            }
        })
    }
}
// JavaScript starts an async call immediately through its first await. Preserve dispatch before
// starting the observation reads, then keep the subscription alive if its reply is still pending.
async fn start_background(work: impl std::future::Future<Output = ()> + 'static) {
    let mut task = Box::pin(work);
    let done = std::future::poll_fn(|cx| std::task::Poll::Ready(task.as_mut().poll(cx).is_ready())).await;
    if !done {
        tokio::task::spawn_local(task);
    }
}
fn object(value: Value) -> JsonObject {
    value.as_object().cloned().unwrap_or_default()
}
/// A read of devices: its rows, and whether it was cut short; none when Live answered with an error.
fn device_page(read: &CallToolResult, epoch: f64) -> Result<Option<(Vec<JsonObject>, bool)>, ReadError> {
    if read.is_error == Some(true) {
        return Ok(None);
    }
    let page = context::discovery_payload(read, "device", epoch)?;
    let more = cursor(&page).is_some() || page.get("truncated") == Some(&Value::Bool(true));
    Ok(Some((objects(&page)?, more)))
}
/// The selected track's ref, as this turn's selection read has it.
fn selected_track(read: &Result<CallToolResult, ReadError>, epoch: f64) -> Option<String> {
    let read = read.as_ref().ok().filter(|read| read.is_error != Some(true))?;
    let page = context::discovery_payload(read, "selection", epoch).ok()?;
    page.get("items")?.as_array()?.first()?.get("selectedTrackRef")?.as_str().map(str::to_owned)
}
/// The prefix of every device ref on a track (`7:device:3:`, from `7:track:3`), at any rack level.
fn device_prefix(track: &str) -> Option<String> {
    let mut parts = track.split(':');
    let (epoch, kind, index) = (parts.next()?, parts.next()?, parts.next()?);
    (kind == "track" && parts.next().is_none() && index.parse::<u32>().is_ok()).then(|| format!("{epoch}:device:{index}:"))
}
/// The Set's device rows with these tracks' read again in place of theirs.
async fn reread(
    connection: &LiveConnection,
    mut rows: Vec<JsonObject>,
    tracks: &IndexSet<String>,
    epoch: f64,
    signal: Signal,
) -> Result<Vec<JsonObject>, ReadError> {
    for track in tracks {
        let Some(prefix) = device_prefix(track) else { continue };
        let fresh = track_devices(connection, track, epoch, signal.clone()).await?;
        let on_track = |row: &JsonObject| row.get("ref").and_then(Value::as_str).is_some_and(|reference| reference.starts_with(&prefix));
        let at = rows.iter().position(on_track).unwrap_or(rows.len());
        rows.retain(|row| !on_track(row));
        rows.splice(at.min(rows.len())..at.min(rows.len()), fresh);
    }
    Ok(rows)
}
/// One track's device rows, every rack level down: read a level at a time, since a chain's devices name it as their
/// parent.
async fn track_devices(connection: &LiveConnection, track: &str, epoch: f64, signal: Signal) -> Result<Vec<JsonObject>, ReadError> {
    let mut rows = vec![];
    let mut parents = vec![track.to_owned()];
    for _ in 0..32 {
        if parents.is_empty() {
            break;
        }
        let reads = futures::future::join_all(parents.iter().map(|parent| {
            let args = object(json!({"kind":"device","parent":parent,"fields":DEVICE_FIELDS,"limit":connection.page_limit(),"budget":connection.whole_budget()}));
            views::pages(connection, args, signal.clone())
        }))
        .await;
        parents = vec![];
        for read in reads {
            let Some((level, false)) = device_page(&read?, epoch)? else {
                return Err(ObservationError("Live didn't list a track's devices whole".into()).into());
            };
            for row in &level {
                let chains = row.get("chainList").and_then(Value::as_array).into_iter().flatten();
                parents.extend(chains.filter_map(|chain| chain.get("ref").and_then(Value::as_str).map(str::to_owned)));
            }
            rows.extend(level);
        }
    }
    Ok(rows)
}
/// How many device rows differ between the kept ones and a whole read (changed, added or gone): what Live's events
/// missed. The tracks Kumi changed since are left out, since they're read again anyway.
fn drift(kept: &[JsonObject], read: &[JsonObject], changed: &IndexSet<String>) -> u32 {
    let prefixes: Vec<String> = changed.iter().filter_map(|track| device_prefix(track)).collect();
    let rows = |rows: &[JsonObject]| -> HashMap<String, String> {
        rows.iter()
            .filter_map(|row| {
                let reference = row.get("ref")?.as_str()?;
                (!prefixes.iter().any(|prefix| reference.starts_with(prefix)))
                    .then(|| (reference.to_owned(), stringify(&Value::Object(row.clone()))))
            })
            .collect()
    };
    let (kept, read) = (rows(kept), rows(read));
    (kept.iter().filter(|(reference, row)| read.get(*reference) != Some(*row)).count()
        + read.keys().filter(|reference| !kept.contains_key(*reference)).count()) as u32
}
/// The tracks and the devices and chains a value names (see `Observer::devices_named`), from long refs.
fn named_devices(value: &Value, tracks_too: bool, tracks: &mut IndexSet<String>, named: &mut IndexSet<String>, depth: usize) {
    static NAMED: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"^([0-9]+):(device|chain|track):([0-9]+)").unwrap());
    if depth > 32 {
        return;
    }
    match value {
        Value::String(text) => {
            if let Some(found) = NAMED.captures(text).filter(|found| &found[2] != "track" || tracks_too) {
                tracks.insert(format!("{}:track:{}", &found[1], &found[3]));
                if &found[2] != "track" {
                    named.insert(text.clone());
                }
            }
        }
        Value::Array(items) => items.iter().for_each(|item| named_devices(item, tracks_too, tracks, named, depth + 1)),
        Value::Object(row) => row.values().for_each(|item| named_devices(item, tracks_too, tracks, named, depth + 1)),
        _ => {}
    }
}
/// What device rows show of each device and chain, by ref.
fn shown(rows: &[JsonObject]) -> HashMap<String, Shown> {
    let mut shown = HashMap::new();
    for row in rows {
        if let Some(reference) = row.get("ref").and_then(Value::as_str) {
            shown.insert(reference.to_owned(), described(row));
        }
        for chain in row.get("chainList").and_then(Value::as_array).into_iter().flatten() {
            if let Some(reference) = chain.get("ref").and_then(Value::as_str) {
                shown.insert(reference.to_owned(), (text(chain.get("name")), None));
            }
        }
    }
    shown
}
/// What device rows show of one device or chain, if they have it.
fn describe(rows: &[JsonObject], reference: &str) -> Option<Shown> {
    rows.iter().find_map(|row| {
        if row.get("ref").and_then(Value::as_str) == Some(reference) {
            return Some(described(row));
        }
        let chains = row.get("chainList").and_then(Value::as_array).into_iter().flatten();
        chains
            .into_iter()
            .find(|chain| chain.get("ref").and_then(Value::as_str) == Some(reference))
            .map(|chain| (text(chain.get("name")), None))
    })
}
fn described(row: &JsonObject) -> Shown {
    (text(row.get("name")), row.get("className").and_then(Value::as_str).map(str::to_owned))
}
fn text(value: Option<&Value>) -> String {
    value.and_then(Value::as_str).unwrap_or_default().to_owned()
}
fn objects(page: &JsonObject) -> Result<Vec<JsonObject>, ObservationError> {
    page.get("items").and_then(Value::as_array).into_iter().flatten().map(context::object).collect()
}
/// The Set's scale as the observation shows it, and the key roman numerals start in: only a scale the producer chose,
/// with Scale Mode on. Live's default, C Major, shows only with Scale Mode on and never starts them: a fresh Set can
/// have Scale Mode on with it (Live 12.4.15). A Scale Mode Live doesn't say goes unsaid.
fn scale(row: &JsonObject) -> Option<(String, Option<String>)> {
    const ROOTS: [&str; 12] = ["C", "C#", "D", "D#", "E", "F", "F#", "G", "G#", "A", "A#", "B"];
    let scale = row.get("scale")?.as_object()?;
    let root = ROOTS.get(usize::try_from(scale.get("rootNote")?.as_u64()?).ok()?)?;
    let name = scale.get("scaleName")?.as_str().filter(|name| !name.is_empty())?;
    let named = format!("{root} {name}");
    match (scale.get("scaleMode").and_then(Value::as_bool), (*root, name) == ("C", "Major")) {
        (Some(true), true) => Some((format!("{named} (Live's default)"), None)),
        (Some(true), false) => Some((named.clone(), Some(named))),
        (Some(false), false) => Some((format!("{named} (Scale Mode off)"), None)),
        (None, false) => Some((named, None)),
        (_, true) => None,
    }
}
fn cursor(page: &JsonObject) -> Option<&str> {
    page.get("nextCursor").and_then(Value::as_str).filter(|s| !s.is_empty())
}
fn js_string(value: Option<&Value>) -> String {
    match value {
        None => "undefined".into(),
        Some(Value::String(s)) => s.clone(),
        Some(v) => stringify(v),
    }
}
fn track_row(connection: &LiveConnection, row: &JsonObject) -> Result<JsonObject, ObservationError> {
    let empty = json!({});
    let mixer = context::object(row.get("mixer").filter(|v| !v.is_null()).unwrap_or(&empty))?;
    let mut shown = object(
        json!({"ref":row.get("ref").and_then(Value::as_str).map(|r|connection.references.borrow_mut().short_ref(r)),"name":row.get("name").and_then(Value::as_str).map(|s|head(s,120)),"type":if row.get("kind").and_then(Value::as_str)==Some("group"){json!("group")}else{row.get("mediaKind").filter(|v|!v.is_null()).or_else(||row.get("kind")).cloned().unwrap_or(Value::Null)}}),
    );
    if let Some(group) = row.get("groupTrackRef").and_then(Value::as_str) {
        shown.insert("group".into(), json!(connection.references.borrow_mut().short_ref(group)));
    }
    for (field, key) in [("volumeDisplay", "volume"), ("panDisplay", "pan")] {
        if let Some(value) = mixer.get(field).and_then(Value::as_str) {
            shown.insert(key.into(), json!(head(value, 24)));
        }
    }
    Ok(shown)
}
type Device = Rc<DisplayDevice>;
struct DisplayDevice {
    value: JsonObject,
    chains: Vec<DisplayChain>,
}
struct DisplayChain {
    value: JsonObject,
    devices: Rc<RefCell<Vec<Device>>>,
}
fn parent_key(value: Option<&Value>) -> String {
    match value {
        None => "undefined".into(),
        Some(Value::String(s)) => format!("string:{s}"),
        Some(v) => format!("value:{}", stringify(v)),
    }
}
fn add_devices(
    connection: &LiveConnection,
    tracks: &[JsonObject],
    shown: &mut [JsonObject],
    devices: &[JsonObject],
) -> Result<(), ReadError> {
    let mut on_track: IndexMap<String, (Option<Value>, Vec<Device>)> = IndexMap::new();
    let mut chain_rows = Vec::new();
    for device in devices {
        let Some(reference) = device.get("ref").and_then(Value::as_str) else { continue };
        let name = device.get("name").and_then(Value::as_str).map(|s| head(s, 120));
        let class = device.get("className").and_then(Value::as_str).filter(|s| Some(*s) != name.as_deref()).map(|s| head(s, 64));
        let rows: Vec<_> = device
            .get("chainList")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .filter(|v| v.get("ref").and_then(Value::as_str).is_some())
            .take(32)
            .cloned()
            .collect();
        chain_rows.extend(rows.iter().map(context::object).collect::<Result<Vec<_>, _>>()?);
        let mut value = object(json!({"ref":connection.references.borrow_mut().short_ref(reference),"name":name}));
        if let Some(class) = class.filter(|s| !s.is_empty()) {
            value.insert("type".into(), json!(class));
        }
        let chains=rows.iter().map(|chain|DisplayChain{value:object(json!({"ref":connection.references.borrow_mut().short_ref(chain["ref"].as_str().unwrap()),"name":chain.get("name").and_then(Value::as_str).map(|s|head(s,120))})),devices:Rc::new(RefCell::new(Vec::new()))}).collect();
        on_track
            .entry(parent_key(device.get("parentRef")))
            .or_insert_with(|| (device.get("parentRef").cloned(), Vec::new()))
            .1
            .push(Rc::new(DisplayDevice { value, chains }));
    }
    connection.register_rows("chain", &chain_rows, &JsonObject::new(), None)?;
    let mut by_chain = IndexMap::new();
    for (_, rows) in on_track.values() {
        for row in rows {
            for chain in &row.chains {
                by_chain.insert(chain.value["ref"].as_str().unwrap().to_owned(), chain.devices.clone());
            }
        }
    }
    for key in on_track.keys().cloned().collect::<Vec<_>>() {
        let (parent, rows) = &on_track[&key];
        let destination =
            parent.as_ref().and_then(Value::as_str).and_then(|r| by_chain.get(&connection.references.borrow_mut().short_ref(r)).cloned());
        if let Some(chain) = destination {
            chain.borrow_mut().extend(rows.iter().cloned());
            on_track.shift_remove(&key);
        }
    }
    fn value(device: &Device, visited: &mut HashSet<usize>) -> Result<Value, ReadError> {
        let id = Rc::as_ptr(device) as usize;
        if !visited.insert(id) {
            return Err(RuntimeError::plain("Converting circular structure to JSON").into());
        }
        let mut row = device.value.clone();
        if !device.chains.is_empty() {
            let mut chains = Vec::new();
            for chain in &device.chains {
                let mut row = chain.value.clone();
                row.insert(
                    "devices".into(),
                    Value::Array(chain.devices.borrow().iter().map(|d| value(d, visited)).collect::<Result<Vec<_>, _>>()?),
                );
                chains.push(Value::Object(row));
            }
            row.insert("chains".into(), Value::Array(chains));
        }
        visited.remove(&id);
        Ok(Value::Object(row))
    }
    for (row, track) in shown.iter_mut().zip(tracks) {
        if let Some((_, devices)) = on_track.get(&parent_key(track.get("ref"))) {
            row.insert(
                "devices".into(),
                Value::Array(devices.iter().map(|d| value(d, &mut HashSet::new())).collect::<Result<Vec<_>, _>>()?),
            );
        }
    }
    Ok(())
}

/// A Set's fingerprint, as Kumi logs it with the producer's reactions: its name, tempo, meter and scale (`song`), how
/// many tracks play each role, a digest of the kinds of devices on them, and the words in their names.
pub fn fingerprint_of(mut song: Value, model: &SetModel) -> Value {
    fn walk(devices: &[DeviceNode], kinds: &mut BTreeSet<String>, names: &mut Vec<String>) {
        for device in devices {
            kinds.insert(device.class.clone().unwrap_or_else(|| device.name.clone()));
            names.push(device.name.clone());
            names.extend(device.class.clone());
            for chain in &device.chains {
                walk(&chain.devices, kinds, names);
            }
        }
    }
    let (mut roles, mut kinds, mut tokens) = (BTreeMap::<&str, usize>::new(), BTreeSet::new(), Vec::<String>::new());
    for track in &model.tracks {
        let mut names = vec![];
        walk(&track.devices, &mut kinds, &mut names);
        let names: Vec<&str> = names.iter().map(String::as_str).collect();
        if let Some(role) = taste::role_of(&track.name, &names) {
            *roles.entry(role.as_str()).or_default() += 1;
        }
        for word in taste::name_words(&track.name) {
            if word.chars().any(|c| c.is_ascii_alphabetic()) && !tokens.contains(&word) && tokens.len() < 32 {
                tokens.push(word);
            }
        }
    }
    let kinds: Vec<String> = kinds.into_iter().collect();
    song["roles"] = json!(roles);
    song["devices"] = json!(hex::encode(<sha2::Sha256 as sha2::Digest>::digest(kinds.join("\n").as_bytes()))[..16]);
    song["tokens"] = json!(tokens);
    song
}

#[cfg(test)]
mod tests {
    #[test]
    fn a_sets_fingerprint_counts_roles_and_names_its_devices_and_words() {
        use super::super::set_model::{ChainNode, TrackNode};
        let device = |name: &str, class: Option<&str>| DeviceNode {
            reference: "d".into(),
            name: name.into(),
            class: class.map(Into::into),
            chains: vec![],
        };
        let track = |name: &str, devices: Vec<DeviceNode>| TrackNode {
            id: None,
            reference: "t".into(),
            index: 0,
            name: name.into(),
            kind: None,
            media: None,
            group: None,
            devices,
            hash: String::new(),
            generation: 0,
        };
        let mut rack = device("Kit", Some("DrumGroupDevice"));
        rack.chains =
            vec![ChainNode { reference: "c".into(), name: "Kick".into(), devices: vec![device("Simpler", Some("OriginalSimpler"))] }];
        let mut model = SetModel::default();
        model.tracks = vec![track("1 Beat", vec![rack]), track("Sub Bass", vec![device("Operator", None)]), track("Bass Fx", vec![])];
        let print = fingerprint_of(json!({"name":"Night Drive","tempo":172.0,"meter":"4/4","scale":null}), &model);
        assert_eq!(print["roles"], json!({"bass":2,"drums":1}));
        assert_eq!(print["tokens"], json!(["beat", "sub", "bass", "fx"]));
        assert_eq!(print["name"], "Night Drive");
        // The device kinds as a digest: the same kinds give the same one, whatever their names.
        let mut again = SetModel::default();
        again.tracks = vec![track(
            "Drums",
            vec![device("Operator", None), device("My Kit", Some("DrumGroupDevice")), device("Simpler", Some("OriginalSimpler"))],
        )];
        assert_eq!(fingerprint_of(json!({}), &again)["devices"], print["devices"]);
    }

    use super::*;

    #[test]
    fn the_sets_scale_is_named_when_it_says_something() {
        let row = |root: u64, name: &str, mode: Value| object(json!({"scale":{"rootNote":root,"scaleName":name,"scaleMode":mode}}));
        // A chosen scale with Scale Mode on is shown and starts the numerals; off, or unsaid, it's only shown.
        assert_eq!(scale(&row(2, "Dorian", json!(true))), Some(("D Dorian".into(), Some("D Dorian".into()))));
        assert_eq!(scale(&row(9, "Minor", json!(false))), Some(("A Minor (Scale Mode off)".into(), None)));
        assert_eq!(scale(&row(9, "Minor", Value::Null)), Some(("A Minor".into(), None)));
        // Live's default is shown with Scale Mode on (a fresh Set can have it on) but never starts the numerals; off, or
        // unsaid, it says nothing, nor does a row without a scale.
        assert_eq!(scale(&row(0, "Major", json!(true))), Some(("C Major (Live's default)".into(), None)));
        assert_eq!(scale(&row(0, "Major", json!(false))), None);
        assert_eq!(scale(&row(0, "Major", Value::Null)), None);
        assert_eq!(scale(&JsonObject::new()), None);
        assert_eq!(scale(&row(12, "Major", json!(true))), None);
    }
}
