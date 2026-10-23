//! Live's side of tune's roles: a track read as tune sees it (each device's name, class and whether it's on, the
//! parameters Live lets Kumi turn, and for a plug-in every name Live lists for it), and EQ Eight's bands from its knobs.
//! What is made of them is `Listening`'s.
use super::*;
use crate::{
    listening::{
        knobs::{shown_hz, Unit},
        renderer::DeviceKnob,
    },
    plugins::roles::Seen,
};
use regex::Regex;
use std::sync::LazyLock;

/// A device's track, from the device's ref read long: "<epoch>:device:<track>:…".
static ON_TRACK: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"^([0-9]+):device:([0-9]+):").unwrap());

impl LiveRenderer {
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

    /// A device's turnable parameters and the switches that are on, and for a mapped plug-in what Live lists; left
    /// empty when Live can't say.
    pub(super) async fn read_seen(&self, device: &mut Seen, signal: Signal) {
        let long = self.long_device(&device.reference);
        let fields = json!({"parent":long,"fields":["name","value","max","displayValue"]});
        if let Ok(rows) = self.rows("parameter", fields, signal.clone()).await {
            let name = |row: &JsonObject| row.get("name").and_then(Value::as_str).map(str::to_owned);
            let number = |row: &JsonObject, key: &str| row.get(key).and_then(Value::as_f64);
            device.turnable = rows.iter().filter_map(name).filter(|name| name != "Device On").collect();
            // A switch is a parameter from 0 to 1; on at its top.
            device.switched_on = rows
                .iter()
                .filter(|row| number(row, "max").is_some_and(|max| max <= 1.) && number(row, "value").is_some_and(|value| value >= 0.5))
                .filter_map(name)
                .collect();
            // An EQ Eight's bands, by what Live shows: each band's frequency, and whether it's on and shapes.
            let shown = |named: String| {
                rows.iter()
                    .find(|row| row.get("name").and_then(Value::as_str) == Some(named.as_str()))
                    .and_then(|row| row.get("displayValue").and_then(Value::as_str).map(str::to_owned))
            };
            device.bands = rows
                .iter()
                .filter_map(|row| {
                    let band: u8 = row.get("name").and_then(Value::as_str)?.strip_suffix(" Frequency A")?.parse().ok()?;
                    let at = shown_hz(row.get("displayValue").and_then(Value::as_str)?)?;
                    let on = device.switched_on.iter().any(|name| *name == format!("{band} Filter On A"));
                    Some((band, at, on, shown(format!("{band} Filter Type A")).is_none_or(|kind| shapes(&kind))))
                })
                .collect();
        }
        if device.adapter().is_some() {
            device.listed = self.listed_names(&long, signal).await.ok().flatten();
        }
    }

    /// The devices on a device's track, as tune sees them, and where the device is among them.
    pub(super) async fn track_devices_of(&self, device: &str, signal: Signal) -> Result<(Vec<Seen>, usize), RuntimeError> {
        self.track_devices(&self.long_device(device), signal).await
    }

    /// Every parameter name Live lists for a plug-in; None when Live can't list them.
    pub(super) async fn listed_names_of(&self, device: &str, signal: Signal) -> Result<Option<Vec<String>>, RuntimeError> {
        self.listed_names(&self.long_device(device), signal).await
    }

    /// The devices on the run's track (the master without one) as tune sees them, before their parameters are read.
    pub(super) async fn devices_on(self: &Rc<Self>, track: Option<&str>, signal: Signal) -> Option<Vec<Seen>> {
        let parent = self.scope_ref(track, signal.clone()).await.ok()?;
        let rows = self.rows("device", json!({"parent":parent,"fields":DEVICE_FIELDS}), signal.clone()).await.ok()?;
        Some(rows.iter().map(seen).collect())
    }
}

/// An EQ Eight's bands from its knobs: each band's number, frequency (Hz), whether it's on, and whether its gain shapes
/// (a bell or a shelf: a cut's gain does nothing). None for any other device.
pub(super) fn bands_of(knobs: &[DeviceKnob]) -> Vec<(u8, f64, bool, bool)> {
    let named = |name: String| knobs.iter().find(|knob| knob.name == name);
    knobs
        .iter()
        .filter_map(|frequency| {
            let band: u8 = frequency.name.strip_suffix(" Frequency A")?.parse().ok()?;
            let at = frequency.scale.as_ref().filter(|scale| scale.unit == Unit::Hz)?.shown(frequency.raw);
            let on = named(format!("{band} Filter On A")).is_none_or(|on| on.raw >= 0.5);
            let shapes = named(format!("{band} Filter Type A"))
                .and_then(|kind| kind.items.get(kind.raw.round().max(0.) as usize))
                .is_none_or(|kind| shapes(kind));
            Some((band, at, on, shapes))
        })
        .collect()
}

/// Whether an EQ band's type shapes with its gain: a bell or a shelf, not a cut or a notch.
fn shapes(kind: &str) -> bool {
    let kind = kind.to_lowercase();
    kind.contains("bell") || kind.contains("shelf")
}

/// What's read of each device on a track: its name, class, and whether it's on (Live's is_active).
const DEVICE_FIELDS: [&str; 3] = ["name", "className", "enabled"];

/// A device row as tune sees it, before its parameters are read.
fn seen(row: &JsonObject) -> Seen {
    let text = |key: &str| row.get(key).and_then(Value::as_str).unwrap_or("").to_owned();
    let off = row.get("enabled") == Some(&Value::Bool(false));
    Seen { reference: text("ref"), name: text("name"), class: text("className"), off, ..Default::default() }
}
