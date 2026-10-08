//! Knobs named by role in tune, and fixes that name a mapped plug-in's role. A track is read as tune sees it: each
//! device's name, class and whether it's on, the parameters Live lets Kumi turn, and for a plug-in every name Live
//! lists for it, so the plug-in map's names are checked against Live's own before any is used.

use super::tune::{TuneHow, TuneRequest};
use super::*;
use crate::listening::round::Round;
use crate::plugins::roles::{self, is_plugin, Found, Resolved, Seen};
use regex::Regex;
use std::sync::LazyLock;

/// A device's track, from the device's ref read long: "<epoch>:device:<track>:…".
static ON_TRACK: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"^([0-9]+):device:([0-9]+):").unwrap());

impl Rendering {
    /// The tune request with its knobs named by role found: on its device, or else on another of its track, in the
    /// agreed order. None when no knob is named by role; why, when nothing can turn one.
    pub(super) async fn knobs_by_role(
        &self,
        request: &TuneRequest,
        turnable: &[String],
        signal: Signal,
    ) -> Result<Result<Option<TuneRequest>, String>, RuntimeError> {
        if request.how == TuneHow::Fit || request.knobs.iter().all(|word| turnable.iter().any(|name| roles::same(name, word))) {
            return Ok(Ok(None));
        }
        let long = self.long_device(&request.device);
        let (mut devices, asked) = self.track_devices(&long, signal.clone()).await?;
        devices[asked].turnable = turnable.to_vec();
        if is_plugin(&devices[asked].class) {
            devices[asked].listed = self.listed_names(&long, signal.clone()).await?;
        }
        // The rest of the track is read only when the device can't turn a role.
        if request.knobs.iter().any(|word| matches!(roles::find(&devices[asked], word), Some(Found::Missing(_)))) {
            for at in roles::agreed_order(&devices, Some(asked)) {
                self.read_seen(&mut devices[at], signal.clone()).await;
            }
            signal.check()?;
        }
        Ok(match roles::resolve(&devices, asked, &request.knobs) {
            Resolved::AsAsked => Ok(None),
            Resolved::On { device, knobs, read, instead } => {
                let mut found = request.clone();
                if device != asked {
                    found.device = devices[device].reference.clone();
                }
                found.knobs = knobs;
                // The log says why another device, and how each role was read.
                let said: Vec<String> = request.change.iter().cloned().chain(instead).chain(std::iter::once(read.join("; "))).collect();
                found.change = Some(
                    said.iter()
                        .map(|part| part.trim().trim_end_matches('.'))
                        .filter(|part| !part.is_empty())
                        .collect::<Vec<_>>()
                        .join(". "),
                );
                Ok(Some(found))
            }
            Resolved::Refused(why) => Err(why),
        })
    }

    /// A round's next fix names a mapped plug-in's role when one on the run's track does the job (Ozone 12 first, then
    /// the other mapped plug-ins), with the fix it had after it. It stays as it was when none does, or Live can't say.
    pub async fn name_plugin_roles(self: &Rc<Self>, round: &mut Round, signal: Signal) {
        let Some(job) = round.next.as_ref().and_then(|next| roles::job_for_item(&next.id)) else { return };
        let Some(track) = self.judge.borrow().as_ref().map(|run| run.track.clone()) else { return };
        let Ok(parent) = self.scope_ref(track.as_deref(), signal.clone()).await else { return };
        let Ok(rows) = self.rows("device", json!({"parent":parent,"fields":DEVICE_FIELDS}), signal.clone()).await else { return };
        let mut devices: Vec<Seen> = rows.iter().map(seen).collect();
        let doing: Vec<usize> = (0..devices.len())
            .filter(|at| !devices[*at].off && devices[*at].adapter().is_some_and(|adapter| roles::job_role(adapter, job).is_some()))
            .collect();
        if doing.is_empty() {
            return;
        }
        for at in doing {
            self.read_seen(&mut devices[at], signal.clone()).await;
        }
        if let Some(next) = round.next.as_mut() {
            if let Some(fix) = roles::plugin_fix(&devices, job, next.fix.as_deref()) {
                next.fix = Some(fix);
            }
        }
    }

    fn long_device(&self, device: &str) -> String {
        let long = self.connection().references.borrow().lengthen(&json!({"deviceRef":device}));
        long["deviceRef"].as_str().unwrap_or(device).to_owned()
    }

    /// The devices on a device's track (its top level, in order) by name, class and whether they're on, and where the
    /// device is among them: after them, when it's inside a rack.
    async fn track_devices(&self, long: &str, signal: Signal) -> Result<(Vec<Seen>, usize), RuntimeError> {
        let mut devices: Vec<Seen> = vec![];
        if let Some(on) = ON_TRACK.captures(long) {
            let track = format!("{}:track:{}", &on[1], &on[2]);
            devices = self.rows("device", json!({"parent":track,"fields":DEVICE_FIELDS}), signal.clone()).await?.iter().map(seen).collect();
        }
        if let Some(at) = devices.iter().position(|device| self.long_device(&device.reference) == long) {
            return Ok((devices, at));
        }
        let read = self
            .connection()
            .call(
                "live_run_python",
                object(json!({"code":"result = [str(obj.name), str(obj.class_name)]","mode":"exec","ref":long,"timeoutMs":5000})),
                signal,
            )
            .await?;
        let done = if read.is_error == Some(true) { None } else { super::super::context::payload(&read).ok() };
        let pair = done.filter(|done| done.get("ok") == Some(&Value::Bool(true))).and_then(|done| done.get("result").cloned());
        let text = |at: usize| pair.as_ref().and_then(|pair| pair.get(at)).and_then(Value::as_str).unwrap_or("").to_owned();
        devices.push(Seen { reference: long.to_owned(), name: text(0), class: text(1), ..Default::default() });
        let at = devices.len() - 1;
        Ok((devices, at))
    }

    /// Every name Live lists for a plug-in, configured or not; None when Live can't list them.
    async fn listed_names(&self, long: &str, signal: Signal) -> Result<Option<Vec<String>>, RuntimeError> {
        let read = self.connection().call("live_device_read", object(json!({"deviceRef":long,"what":"parameter-names"})), signal).await?;
        if read.is_error == Some(true) {
            return Ok(None);
        }
        let Ok(done) = super::super::context::payload(&read) else { return Ok(None) };
        Ok(done.get("names").and_then(Value::as_array).map(|names| names.iter().filter_map(Value::as_str).map(str::to_owned).collect()))
    }

    /// A device's turnable parameters, and for a mapped plug-in what Live lists; left empty when Live can't say.
    async fn read_seen(&self, device: &mut Seen, signal: Signal) {
        let long = self.long_device(&device.reference);
        if let Ok(rows) = self.rows("parameter", json!({"parent":long,"fields":["name"]}), signal.clone()).await {
            device.turnable = rows
                .iter()
                .filter_map(|row| row.get("name").and_then(Value::as_str))
                .filter(|name| *name != "Device On")
                .map(str::to_owned)
                .collect();
        }
        if device.adapter().is_some() {
            device.listed = self.listed_names(&long, signal).await.ok().flatten();
        }
    }
}

/// What's read of each device on a track: its name, class, and whether it's on (Live's is_active).
const DEVICE_FIELDS: [&str; 3] = ["name", "className", "enabled"];

/// A device row as tune sees it, before its parameters are read.
fn seen(row: &JsonObject) -> Seen {
    let text = |key: &str| row.get(key).and_then(Value::as_str).unwrap_or("").to_owned();
    let off = row.get("enabled") == Some(&Value::Bool(false));
    Seen { reference: text("ref"), name: text("name"), class: text("className"), off, ..Default::default() }
}
