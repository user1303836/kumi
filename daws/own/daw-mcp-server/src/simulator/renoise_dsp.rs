//! The Renoise simulator's DSP devices (M5): the chains a test puts on tracks, read as the tool reads them (`device`,
//! `param` with `grid`, a device by its ref) and changed as it changes them (`device-add` onto such a track,
//! `device-param`, `device-params`), so listening's goal and knobs run against Renoise's shapes. A track no test gave
//! devices answers from the vectors, as before.

use serde_json::{json, Value};
use std::collections::HashMap;

/// How a parameter shows its value: dB (the value is in dB), Hz, a plain number, or a linear gain shown in dB (the Gainer's).
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Shows {
    Db,
    Hz,
    Plain,
    Linear,
}

impl Shows {
    fn text(self, value: f64) -> String {
        match self {
            Shows::Db => format!("{value:.3} dB"),
            Shows::Hz => format!("{value:.2} Hz"),
            Shows::Plain => format!("{value:.3}"),
            Shows::Linear if value <= 0. => "-INF dB".into(),
            Shows::Linear => format!("{:.3} dB", 20. * value.log10()),
        }
    }
    /// The value a text means (its leading number, in the parameter's units), within the range.
    fn value(self, text: &str, min: f64, max: f64) -> Option<f64> {
        let number: String =
            text.trim().replace('\u{2212}', "-").chars().take_while(|c| c.is_ascii_digit() || matches!(c, '.' | '-' | '+')).collect();
        let number: f64 = number.parse().ok()?;
        let value = if self == Shows::Linear { 10f64.powf(number / 20.) } else { number };
        Some(value.clamp(min, max))
    }
}

#[derive(Debug, Clone)]
pub struct Param {
    pub name: String,
    pub value: f64,
    pub min: f64,
    pub max: f64,
    pub shows: Shows,
}

#[derive(Debug, Clone)]
pub struct Device {
    pub path: String,
    pub params: Vec<Param>,
}

impl Device {
    fn name(&self) -> &str {
        self.path.rsplit('/').next().unwrap_or(&self.path)
    }
}

/// A native device by its path or name, with the parameters the tool reads for it (names as Renoise 3.5.4 gives them).
pub fn device(path: &str) -> Option<Device> {
    let p = |name: &str, value: f64, min: f64, max: f64, shows: Shows| Param { name: name.into(), value, min, max, shows };
    let name = path.rsplit('/').next().unwrap_or(path);
    let params = match name {
        "TrackVolPan" => {
            vec![p("Panning", 0.5, 0., 1., Shows::Plain), p("Volume", 1., 0., 1.4125, Shows::Linear), p("Width", 0., -1., 1., Shows::Plain)]
        }
        "EQ 10" => {
            let bands = [31., 63., 125., 250., 500., 1000., 2000., 4000., 8000., 16000.];
            let mut params: Vec<Param> =
                bands.iter().enumerate().map(|(n, hz)| p(&format!("Gain {} ({hz} Hz)", n + 1), 0., -20., 20., Shows::Db)).collect();
            params.extend(bands.iter().enumerate().map(|(n, hz)| p(&format!("Frequency {}", n + 1), *hz, 20., 20000., Shows::Hz)));
            params.extend((1..=10).map(|n| p(&format!("Bandwidth {n}"), 1., 0.0001, 4., Shows::Plain)));
            params
        }
        "Maximizer" => vec![
            p("Boost", 0., 0., 24., Shows::Db),
            p("Threshold", 0., -24., 0., Shows::Db),
            p("Peak Release", 1., 1., 500., Shows::Plain),
            p("Slow Release", 100., 1., 5000., Shows::Plain),
            p("Ceiling", -0.1, -24., 0., Shows::Db),
        ],
        "Gainer" => vec![p("Gain", 1., 0., 4., Shows::Linear), p("Panning", 0.5, 0., 1., Shows::Plain)],
        // (a meta device: where it sends values, a track from 0 and a device on it, -1 for none, beside its own knobs)
        "*LFO" => vec![
            p("Dest. Track", -1., -1., 255., Shows::Plain),
            p("Dest. Effect", -1., -1., 255., Shows::Plain),
            p("Dest. Parameter", -1., -1., 255., Shows::Plain),
            p("Amplitude", 0.5, 0., 1., Shows::Plain),
            p("Offset", 0., 0., 1., Shows::Plain),
            p("Frequency", 1., 0.01, 100., Shows::Hz),
        ],
        _ => return None,
    };
    let path = if path.contains('/') { path.to_owned() } else { format!("Audio/Effects/Native/{name}") };
    Some(Device { path, params })
}

/// What a change asks of one parameter: its number (from 1), and a value or the text Renoise shows for it.
type Ask = (usize, Option<f64>, Option<String>);

/// The chains a test made, by track number (from 1); each starts with the track's TrackVolPan.
#[derive(Debug, Default)]
pub struct Chains {
    pub tracks: HashMap<u64, Vec<Device>>,
}

/// A ref's numbers (`<epoch>:<kind>:a.b.c`), whatever its epoch.
fn numbers(reference: &str, kind: &str) -> Option<Vec<u64>> {
    let mut parts = reference.splitn(3, ':');
    let (_, found, rest) = (parts.next()?, parts.next()?, parts.next()?);
    (found == kind).then(|| rest.split('.').filter_map(|n| n.parse().ok()).collect())
}

fn device_row(epoch: u32, track: u64, at: usize, device: &Device) -> Value {
    json!({"active": true, "displayName": device.name(), "index": at + 1, "isDoofer": false, "isSplitter": false, "name": device.name(),
        "paramCount": device.params.len(), "path": device.path, "plugin": false, "preset": null, "ref": format!("{epoch}:device:{track}.{}", at + 1)})
}

fn param_row(epoch: u32, track: u64, at: u64, k: usize, param: &Param, grid: bool) -> Value {
    let shown = param.shows.text(param.value);
    let mut row = json!({"index": k + 1, "max": param.max, "min": param.min, "name": param.name,
        "ref": format!("{epoch}:param:{track}.{at}.{}", k + 1), "value": param.value, "valueString": shown});
    // (Renoise has no text for a value it isn't set to: a grid answers the current point only, and says so)
    if grid {
        row["grid"] = json!([[param.value, shown]]);
        row["gridShort"] = true.into();
    }
    row
}

impl Chains {
    /// A test's device on a track (after what it has); its ref's numbers.
    pub fn add(&mut self, track: u64, path: &str) -> Option<u64> {
        let made = device(path)?;
        let chain = self.tracks.entry(track).or_insert_with(|| vec![device("TrackVolPan").unwrap()]);
        chain.push(made);
        Some(chain.len() as u64)
    }

    /// A device the producer puts at a place in a track's chain (from 1; TrackVolPan is 1).
    pub fn insert(&mut self, track: u64, at: u64, path: &str) {
        let made = device(path).expect("a device the simulator knows");
        let chain = self.tracks.entry(track).or_insert_with(|| vec![device("TrackVolPan").unwrap()]);
        chain.insert(((at.max(2) - 1) as usize).min(chain.len()), made);
    }

    /// The producer turns a parameter (by its device's place and its name) in Renoise.
    pub fn set_value(&mut self, track: u64, at: u64, name: &str, value: f64) {
        let device = self.tracks.get_mut(&track).and_then(|chain| chain.get_mut(at.checked_sub(1)? as usize));
        if let Some(param) = device.and_then(|device| device.params.iter_mut().find(|param| param.name == name)) {
            param.value = value.clamp(param.min, param.max);
        }
    }

    /// A parameter's value now, by its device's place and its name.
    pub fn value(&self, track: u64, at: u64, name: &str) -> Option<f64> {
        let device = self.tracks.get(&track)?.get(at.checked_sub(1)? as usize)?;
        device.params.iter().find(|param| param.name == name).map(|param| param.value)
    }

    /// The tool's answer to a read about a track with a chain: `device` of the track, `param` of one of its devices, a
    /// device by its ref. None for anything else (the vectors answer it).
    pub fn read(&self, epoch: u32, params: &Value) -> Option<Value> {
        let kind = params["kind"].as_str();
        let page = |items: Vec<Value>| json!({"epoch": epoch.to_string(), "layout": 0, "items": items});
        match kind {
            Some("device") => {
                let track = *numbers(params["parent"].as_str()?, "track")?.first()?;
                let chain = self.tracks.get(&track)?;
                Some(page(chain.iter().enumerate().map(|(at, device)| device_row(epoch, track, at, device)).collect()))
            }
            Some("param") => {
                let at = numbers(params["parent"].as_str()?, "device")?;
                let (track, place) = (*at.first()?, *at.get(1)?);
                let device = self.tracks.get(&track)?.get(place.checked_sub(1)? as usize)?;
                let grid = params.get("grid").is_some_and(|grid| !grid.is_null());
                Some(page(device.params.iter().enumerate().map(|(k, param)| param_row(epoch, track, place, k, param, grid)).collect()))
            }
            None => {
                let at = numbers(params["ref"].as_str()?, "device")?;
                let (track, place) = (*at.first()?, *at.get(1)?);
                let device = self.tracks.get(&track)?.get(place.checked_sub(1)? as usize)?;
                Some(page(vec![device_row(epoch, track, place as usize - 1, device)]))
            }
            _ => None,
        }
    }

    /// A device-add onto a track with a chain: the device's place, or None when the track has none (the counts answer).
    pub fn added(&mut self, track: u64, path: &str) -> Option<u64> {
        self.tracks.contains_key(&track).then(|| self.add(track, path)).flatten()
    }

    /// A device-add's restore: the device at that place goes.
    pub fn removed(&mut self, track: u64, at: u64) -> bool {
        match self.tracks.get_mut(&track) {
            Some(chain) if at >= 2 && (at as usize) <= chain.len() => {
                chain.remove(at as usize - 1);
                true
            }
            _ => false,
        }
    }

    /// `device-param {ref: param | device with param, value | valueString}` and `device-params {ref: device, values:
    /// [{param, value | valueString}]}` as the tool makes them: fenced on the values before, one step of Renoise's undo,
    /// put back while each is still as applied. `undo` takes the step's name.
    pub fn change(&mut self, epoch: u32, method: &str, params: &Value, undo: &mut Vec<String>) -> Result<Value, (String, String)> {
        let bad = |message: String| ("bad-params".to_string(), message);
        let kind = params["kind"].as_str().unwrap_or_default();
        let desc = params["desc"].as_str().unwrap_or_default().to_string();
        let source = if method == "change_restore" { &params["target"] } else { &params["input"] };
        let reference = source["ref"].as_str().unwrap_or_default();
        // The device and what was asked of it: (parameter number, value) pairs.
        let (track, place, asks): (u64, u64, Vec<Ask>) = {
            let (at, single) = match numbers(reference, "param") {
                Some(at) if at.len() == 3 => {
                    let single = Some(at[2] as usize);
                    (at, single)
                }
                _ => (numbers(reference, "device").ok_or_else(|| bad(format!("{reference} isn't a device or a parameter")))?, None),
            };
            let (track, place) = (at[0], *at.get(1).ok_or_else(|| bad("a device's place".into()))?);
            let asks = if method == "change_restore" {
                vec![]
            } else if kind == "device-params" {
                params["input"]["values"]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .map(|value| {
                        (
                            value["param"].as_u64().unwrap_or(0) as usize,
                            value["value"].as_f64(),
                            value["valueString"].as_str().map(str::to_owned),
                        )
                    })
                    .collect()
            } else {
                let number = single.or(source["param"].as_u64().map(|n| n as usize)).ok_or_else(|| bad("param".into()))?;
                vec![(number, source["value"].as_f64(), source["valueString"].as_str().map(str::to_owned))]
            };
            (track, place, asks)
        };
        let device = self
            .tracks
            .get_mut(&track)
            .and_then(|chain| chain.get_mut((place as usize).checked_sub(1)?))
            .ok_or(("gone".to_string(), "that device isn't there".to_string()))?;
        let name = device.name().to_owned();
        let values = |device: &Device, numbers: &[usize]| -> serde_json::Map<String, Value> {
            numbers.iter().map(|n| (n.to_string(), json!(device.params[n - 1].value))).collect()
        };
        let line = |device: &Device, n: usize| {
            format!("{} → {}", device.params[n - 1].name, device.params[n - 1].shows.text(device.params[n - 1].value))
        };
        let answer = |value: Value| {
            let mut value = value;
            value["epoch"] = epoch.to_string().into();
            value["layout"] = 0.into();
            value
        };
        if method == "change_restore" {
            let prior = params["prior"]["params"].as_object().ok_or_else(|| bad("prior.params".into()))?;
            let numbers: Vec<usize> = prior.keys().filter_map(|key| key.parse().ok()).collect();
            let now = values(device, &numbers);
            let moved: Vec<&str> = numbers
                .iter()
                .filter(|n| now[&n.to_string()].as_f64() != params["applied"]["params"][n.to_string()].as_f64())
                .map(|n| device.params[n - 1].name.as_str())
                .collect();
            if !moved.is_empty() && params["discard"] != true {
                return Err(("changed".into(), format!("{} changed in Renoise since Kumi set it", moved.join(", "))));
            }
            for n in &numbers {
                device.params[n - 1].value = prior[&n.to_string()].as_f64().unwrap_or_default();
            }
            undo.push(desc);
            let lines: Vec<String> = numbers.iter().map(|n| line(device, *n)).collect();
            return Ok(answer(
                json!({"restored": {"params": values(device, &numbers)}, "display": {"title": format!("{name} put back"), "lines": lines}}),
            ));
        }
        // Each asked value, in the parameter's own range (a text by what Renoise shows for it).
        let mut wanted: Vec<(usize, f64)> = vec![];
        for (n, value, text) in asks {
            let param = device.params.get(n.wrapping_sub(1)).ok_or_else(|| bad(format!("{name} has no parameter {n}")))?;
            let value = match (value, text) {
                (Some(value), None) if (param.min..=param.max).contains(&value) => value,
                (None, Some(text)) => param
                    .shows
                    .value(&text, param.min, param.max)
                    .ok_or_else(|| bad(format!("{text:?} isn't a value of {}", param.name)))?,
                _ => return Err(bad(format!("{}: value within {}..{} or valueString", param.name, param.min, param.max))),
            };
            if wanted.iter().any(|(seen, _)| *seen == n) {
                return Err(bad(format!("{} is given twice", param.name)));
            }
            wanted.push((n, value));
        }
        if wanted.is_empty() || wanted.len() > 256 {
            return Err(bad("values: 1 to 256".into()));
        }
        let numbers: Vec<usize> = wanted.iter().map(|(n, _)| *n).collect();
        let before = values(device, &numbers);
        let title = if kind == "device-params" {
            format!("{name} {} parameter{} on “Track {track}”", wanted.len(), if wanted.len() == 1 { "" } else { "s" })
        } else {
            format!("{name} on Track {track} {}", device.params[wanted[0].0 - 1].name)
        };
        let target = json!({"ref": format!("{epoch}:device:{track}.{place}"), "kind": "device"});
        match method {
            "change_preview" => Ok(answer(
                json!({"target": target, "prior": {"params": before.clone()}, "fence": {"params": before}, "display": {"title": title}}),
            )),
            _ => {
                if params["fence"]["params"].as_object() != Some(&before) {
                    return Err(("changed".into(), format!("{name}'s parameters changed since the preview")));
                }
                for (n, value) in &wanted {
                    device.params[n - 1].value = *value;
                }
                undo.push(desc.clone());
                let lines: Vec<String> = numbers.iter().map(|n| line(device, *n)).collect();
                Ok(answer(
                    json!({"applied": {"params": values(device, &numbers)}, "display": {"title": title, "lines": lines}, "undo": {"desc": desc, "entry": true}}),
                ))
            }
        }
    }
}
