//! Serum 2's mod matrix. Serum drops a slot that has no source, so one edit writes a whole slot: a measured
//! source routed to a measured destination parameter at an amount in the measured range. The names come from
//! structure.json's `modulation` table; range-verification.json keeps the measurements.

use serde_json::{json, Value as Json};

use super::cbor::{Float, Head, Length, Value};
use super::serum2::SerumPreset;
use super::structure::{self, Modulation, ModulationDestination, ModulationSource, Status};
use super::FormatError;

/// The edit path that writes one slot.
pub const EDIT: &str = "$modulation";
const AMOUNT: &str = "ModSlot{n}/plainParams/kParamAmount";
const OUTPUT: &str = "ModSlot{n}/plainParams/kParamOut";
const KEYS: &[&str] = &["slot", "source", "destination", "amount", "bipolar", "output"];

/// One slot as an edit asks for it: `{"slot": 1, "source": "LFO 1", "destination": "Filter 1 Freq", "amount": 35}`,
/// optionally `"bipolar": true` and `"output"` (0–100, default 100).
#[derive(Debug, Clone, PartialEq)]
pub struct Route {
    /// 1–64, as Serum and its host parameters ("Mod 1 Amount") number them.
    pub slot: u32,
    pub source: &'static str,
    pub destination: &'static str,
    pub amount: f64,
    pub bipolar: bool,
    pub output: f64,
}

pub fn matrix() -> Result<&'static Modulation, FormatError> {
    structure::builtin("serum-2")
        .and_then(|format| format.structure.modulation.as_ref())
        .ok_or_else(|| FormatError::new("Serum 2's mod matrix description is missing"))
}

fn fail(message: impl Into<String>) -> FormatError {
    FormatError::new(format!("{EDIT}: {}", message.into()))
}

impl Route {
    /// Check an edit's value against the measured tables and ranges; nothing unmeasured gets through.
    pub fn parse(value: &Json) -> Result<Route, FormatError> {
        let matrix = matrix()?;
        let object = value.as_object().ok_or_else(|| fail("give an object with slot, source, destination and amount"))?;
        if let Some(key) = object.keys().find(|key| !KEYS.contains(&key.as_str())) {
            return Err(fail(format!("{key} isn't a slot setting; use slot, source, destination, amount, bipolar and output")));
        }
        let slot = object
            .get("slot")
            .and_then(Json::as_u64)
            .filter(|slot| (1..=u64::from(matrix.slots)).contains(slot))
            .ok_or_else(|| fail(format!("slot is a number from 1 to {}", matrix.slots)))? as u32;
        let source = object.get("source").and_then(Json::as_str).ok_or_else(|| fail("source names a mod source, such as LFO 1"))?;
        let (source, _) = matrix.sources.get_key_value(source).filter(|(_, entry)| entry.status == Status::Verified).ok_or_else(|| {
            fail(format!("{source} isn't a measured Serum 2 mod source; measured: {}", verified_sources(matrix).join(", ")))
        })?;
        let destination =
            object.get("destination").and_then(Json::as_str).ok_or_else(|| fail("destination names a control, such as Filter 1 Freq"))?;
        let (destination, _) = matrix
            .destinations
            .get_key_value(destination)
            .filter(|(_, entry)| entry.status == Status::Verified)
            .ok_or_else(|| fail(format!("{destination} isn't a measured Serum 2 mod destination")))?;
        let number = |key: &str, default: Option<f64>| -> Result<f64, FormatError> {
            match object.get(key) {
                None => default.ok_or_else(|| fail(format!("{key} is required"))),
                Some(value) => value.as_f64().filter(|n| n.is_finite()).ok_or_else(|| fail(format!("{key} must be a number"))),
            }
        };
        let amount = number("amount", None)?;
        structure::validate_number("serum-2", AMOUNT, amount).map_err(|e| fail(e.to_string()))?;
        let output = number("output", Some(100.0))?;
        structure::validate_number("serum-2", OUTPUT, output).map_err(|e| fail(e.to_string()))?;
        let bipolar = match object.get("bipolar") {
            None => false,
            Some(value) => value.as_bool().ok_or_else(|| fail("bipolar is true or false"))?,
        };
        Ok(Route { slot, source, destination, amount, bipolar, output })
    }

    /// The slot's section in the preset ("ModSlot0" for slot 1).
    pub fn section(&self) -> String {
        format!("ModSlot{}", self.slot - 1)
    }
}

fn verified_sources(matrix: &Modulation) -> Vec<&str> {
    let mut names: Vec<(&ModulationSource, &str)> =
        matrix.sources.iter().filter(|(_, s)| s.status == Status::Verified).map(|(name, s)| (s, name.as_str())).collect();
    names.sort_by_key(|(source, _)| source.id);
    names.into_iter().map(|(_, name)| name).collect()
}

fn int(n: u32) -> Value {
    Value::Unsigned(u64::from(n), Head::shortest(u64::from(n)))
}

/// The format catalog's description of the edit: its value and every measured name.
pub fn catalog() -> Json {
    let Ok(matrix) = matrix() else { return json!({}) };
    let destinations: Vec<&str> =
        matrix.destinations.iter().filter(|(_, d)| d.status == Status::Verified).map(|(name, _)| name.as_str()).collect();
    json!({
        "value": format!("one whole mod slot: {{\"slot\": 1–{}, \"source\", \"destination\", \"amount\": -100–100 (%), \"bipolar\": false, \"output\": 0–100 (%, default 100)}}; it replaces that slot", matrix.slots),
        "sources": verified_sources(matrix),
        "destinations": destinations,
    })
}
fn float(n: f64) -> Value {
    Value::Float(Float::Double(n.to_bits()))
}
fn map(entries: Vec<(&str, Value)>) -> Value {
    let length = Length::Definite(Head::shortest(entries.len() as u64));
    Value::Map(entries.into_iter().map(|(key, value)| (Value::text(key), value)).collect(), length)
}

impl SerumPreset {
    /// Replace one mod slot with a measured route, written as Serum 2.1.5 saves one: its keys in Serum's order,
    /// no aux source, and defaults (amount 0, unipolar, output 100) left out.
    pub fn set_modulation(&mut self, value: &Json) -> Result<(), FormatError> {
        let route = Route::parse(value)?;
        let matrix = matrix()?;
        let (source, destination) = (&matrix.sources[route.source], &matrix.destinations[route.destination]);
        let mut params = Vec::new();
        if route.amount != 0.0 {
            params.push(("kParamAmount", float(route.amount)));
        }
        if route.bipolar {
            params.push(("kParamBipolar", float(1.0)));
        }
        if route.output != 100.0 {
            params.push(("kParamOut", float(route.output)));
        }
        let plain = if params.is_empty() { Value::text("default") } else { map(params) };
        let slot = map(vec![
            ("destModuleID", int(destination.index)),
            ("destModuleParamID", int(destination.param)),
            ("destModuleParamName", Value::text(destination.name.as_str())),
            ("destModuleTypeString", Value::text(destination.module.as_str())),
            ("plainParams", plain),
            ("source", Value::Array(vec![int(source.id), int(0)], Length::Definite(Head::Inline))),
        ]);
        let name = route.section();
        let Value::Map(sections, _) = &mut self.file.body else {
            return Err(FormatError::new("Serum preset body must be a map"));
        };
        let section = sections
            .iter_mut()
            .find(|(key, _)| key.as_str() == Some(name.as_str()))
            .map(|(_, value)| value)
            .ok_or_else(|| fail(format!("the preset has no {name}")))?;
        *section = slot;
        Ok(())
    }
}

/// Whether a slot section has a source; Serum saves nothing else of a slot without one.
pub fn routed(section: Option<&Value>) -> bool {
    section
        .and_then(|slot| slot.get("source"))
        .and_then(Value::as_array)
        .and_then(|source| source.first())
        .and_then(Value::as_i64)
        .is_some_and(|id| id > 0)
}

/// A slot read back from a decoded preset, in words: "LFO 1 to Filter 1 Freq, amount 35%". A route outside the
/// measured tables (from someone else's preset) is named by its stored numbers.
pub fn describe(section: Option<&Json>) -> Result<String, FormatError> {
    let matrix = matrix()?;
    let Some(section) = section else { return Ok("not routed".into()) };
    let source = section.get("source").and_then(|s| s.get(0)).and_then(Json::as_u64).unwrap_or(0);
    if source == 0 {
        return Ok("not routed".into());
    }
    let source = matrix
        .sources
        .iter()
        .find(|(_, s)| u64::from(s.id) == source && s.status == Status::Verified)
        .map_or_else(|| format!("source {source}"), |(name, _)| name.clone());
    let text = |key: &str| section.get(key).and_then(Json::as_str).unwrap_or_default();
    let number = |key: &str| section.get(key).and_then(Json::as_u64);
    let found = |d: &&ModulationDestination| {
        d.status == Status::Verified
            && d.module == text("destModuleTypeString")
            && Some(u64::from(d.index)) == number("destModuleID")
            && d.name == text("destModuleParamName")
    };
    let destination = matrix.destinations.iter().find(|(_, d)| found(d)).map_or_else(
        || format!("{} {} {}", text("destModuleTypeString"), number("destModuleID").unwrap_or_default(), text("destModuleParamName")),
        |(name, _)| name.clone(),
    );
    let plain = section.get("plainParams");
    let param = |key: &str| plain.and_then(|p| p.get(key)).and_then(Json::as_f64);
    let mut words = format!("{source} to {destination}, amount {}%", round(param("kParamAmount").unwrap_or(0.0)));
    if param("kParamBipolar").is_some_and(|b| b != 0.0) {
        words.push_str(", bipolar");
    }
    if let Some(output) = param("kParamOut").filter(|o| *o != 100.0) {
        words.push_str(&format!(", output {}%", round(output)));
    }
    Ok(words)
}

/// Serum stores 35 as 35.00000000000001; say what was asked for.
fn round(n: f64) -> Json {
    let rounded = (n * 1e6).round() / 1e6;
    if rounded.fract() == 0.0 {
        json!(rounded as i64)
    } else {
        json!(rounded)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn route(value: Json) -> Result<Route, FormatError> {
        Route::parse(&value)
    }

    #[test]
    fn a_route_takes_only_measured_names_and_ranges() {
        let ok = route(json!({"slot": 1, "source": "LFO 1", "destination": "Filter 1 Freq", "amount": 35})).unwrap();
        assert_eq!((ok.section(), ok.amount, ok.bipolar, ok.output), ("ModSlot0".into(), 35.0, false, 100.0));
        assert!(route(json!({"slot": 64, "source": "Macro 8", "destination": "A WT Pos", "amount": -100, "output": 0, "bipolar": true}))
            .is_ok());
        for bad in [
            json!({"slot": 0, "source": "LFO 1", "destination": "Filter 1 Freq", "amount": 35}),
            json!({"slot": 65, "source": "LFO 1", "destination": "Filter 1 Freq", "amount": 35}),
            json!({"slot": 1, "source": "LFO 11", "destination": "Filter 1 Freq", "amount": 35}),
            json!({"slot": 1, "source": "Aftertouch", "destination": "Filter 1 Freq", "amount": 35}),
            json!({"slot": 1, "source": "LFO 1", "destination": "Main Vol", "amount": 35}),
            json!({"slot": 1, "source": "LFO 1", "destination": "A Hz Offset", "amount": 35}),
            json!({"slot": 1, "source": "LFO 1", "destination": "Filter 1 Freq", "amount": 100.5}),
            json!({"slot": 1, "source": "LFO 1", "destination": "Filter 1 Freq", "amount": 35, "output": 101}),
            json!({"slot": 1, "source": "LFO 1", "destination": "Filter 1 Freq"}),
            json!({"slot": 1, "source": "LFO 1", "destination": "Filter 1 Freq", "amount": 35, "aux": "Velocity"}),
            json!({"slot": 1, "source": "LFO 1", "destination": "Filter 1 Freq", "amount": 35, "bipolar": 1}),
        ] {
            assert!(route(bad.clone()).is_err(), "{bad}");
        }
    }

    #[test]
    fn a_slot_is_written_as_serum_saves_it_and_reads_back_in_words() {
        let mut preset = SerumPreset::init("Mod").unwrap();
        preset
            .set_modulation(&json!({"slot": 3, "source": "LFO 1", "destination": "Filter 1 Freq", "amount": 35, "bipolar": true}))
            .unwrap();
        let slot = preset.file.body.get("ModSlot2").unwrap();
        let keys: Vec<_> = slot.entries().map(|(key, _)| key).collect();
        assert_eq!(keys, ["destModuleID", "destModuleParamID", "destModuleParamName", "destModuleTypeString", "plainParams", "source"]);
        assert_eq!(slot.at("destModuleParamName").and_then(Value::as_str), Some("kParamFreq"));
        assert_eq!(slot.at("destModuleParamID").and_then(Value::as_i64), Some(3));
        assert_eq!(slot.at("source/0").and_then(Value::as_i64), Some(6));
        assert_eq!(slot.at("source/1").and_then(Value::as_i64), Some(0));
        assert_eq!(slot.at("plainParams/kParamAmount").and_then(Value::as_f64), Some(35.0));
        assert!(slot.at("plainParams/kParamOut").is_none());
        assert!(routed(Some(slot)) && !routed(preset.file.body.get("ModSlot3")));
        let bytes = preset.to_vstpreset().unwrap().write();
        let decoded = crate::plugins::preset::inspect("serum-2", &bytes, None).unwrap()["data"].take();
        assert_eq!(describe(decoded.get("ModSlot2")).unwrap(), "LFO 1 to Filter 1 Freq, amount 35%, bipolar");
        assert_eq!(describe(decoded.get("ModSlot3")).unwrap(), "not routed");
        assert_eq!(
            describe(Some(&json!({"destModuleID": 0, "destModuleParamName": "kParamFoo", "destModuleTypeString": "FXHyperD",
                "plainParams": {"kParamAmount": -12.5, "kParamOut": 50.0}, "source": [21, 0]})))
            .unwrap(),
            "source 21 to FXHyperD 0 kParamFoo, amount -12.5%, output 50%"
        );
    }
}
