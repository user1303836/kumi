//! What a simulator that replays a bridge's recorded answers shares, whichever DAW: the cases of a vectors file
//! (tests/vectors/<daw>-reads.json, in the Renoise tool's format: `{"cases": [{"request": {"method", "params"},
//! "response": {"ok": true, "result": …} | {"ok": false, "error": {"code", "message"}}}, …]}`), the epoch they were
//! recorded in, the retagging of that epoch in refs and cursors when the simulated DAW moves to another, and
//! parameters made plain so that 3 and 3.0 are the same request.
use serde_json::{json, Value};

/// The epoch the vectors are recorded in: the bridge's first after attaching.
pub const RECORDED_EPOCH: u32 = 1;

/// One recorded request, and what the bridge answered: the result, or the error's code and message.
pub struct Case {
    pub method: String,
    pub params: Value,
    pub response: Result<Value, (String, String)>,
}

/// The cases of a vectors file (`name` says which, for when it isn't one), their parameters made plain.
pub fn load_cases(vectors: &str, name: &str) -> Vec<Case> {
    let vectors: Value = serde_json::from_str(vectors).unwrap_or_else(|_| panic!("{name} is JSON"));
    vectors["cases"]
        .as_array()
        .expect("cases")
        .iter()
        .map(|case| {
            let response = &case["response"];
            Case {
                method: case["request"]["method"].as_str().expect("method").to_string(),
                params: normal(&case["request"]["params"]),
                response: if response["ok"] == true {
                    Ok(response["result"].clone())
                } else {
                    Err((
                        response["error"]["code"].as_str().expect("code").to_string(),
                        response["error"]["message"].as_str().expect("message").to_string(),
                    ))
                },
            }
        })
        .collect()
}

/// The case for a request: the same method, with the same parameters (whole numbers alike, however written).
pub fn find<'a>(cases: &'a [Case], method: &str, params: &Value) -> Option<&'a Case> {
    let wanted = normal(params);
    cases.iter().find(|case| case.method == method && case.params == wanted)
}

/// The epoch a ref (`<epoch>:<kind>:…`) or a cursor (`<epoch>.<layout>.<n>`, `<epoch>.<n>`) names, if the text is one.
pub fn epoch_of(text: &str) -> Option<u32> {
    let end = text.find([':', '.'])?;
    let (digits, rest) = text.split_at(end);
    (!digits.is_empty() && rest.len() > 1 && digits.bytes().all(|b| b.is_ascii_digit())).then(|| digits.parse().ok()).flatten()
}

/// `value` with the epoch `from` made `to`: in every ref and cursor that names it, and in a field called `epoch`.
pub fn retag(value: &Value, from: u32, to: u32) -> Value {
    retag_with(value, from, to, None, &|_, _| None)
}

/// The same, and what else another epoch changes in a DAW: `also` is asked about every text field, by its key, and
/// answers what the field becomes (Renoise's: another song is another identity and another id). `key` is the field
/// `value` sits in, if any.
pub fn retag_with(value: &Value, from: u32, to: u32, key: Option<&str>, also: &dyn Fn(&str, &str) -> Option<Value>) -> Value {
    match value {
        Value::String(text) if key == Some("epoch") && *text == from.to_string() => Value::String(to.to_string()),
        Value::String(text) => {
            if let Some(changed) = key.and_then(|key| also(key, text)) {
                return changed;
            }
            if epoch_of(text) == Some(from) {
                return Value::String(format!("{to}{}", &text[text.find([':', '.']).expect("a ref or cursor")..]));
            }
            value.clone()
        }
        Value::Array(items) => Value::Array(items.iter().map(|item| retag_with(item, from, to, key, also)).collect()),
        Value::Object(map) => Value::Object(map.iter().map(|(k, v)| (k.clone(), retag_with(v, from, to, Some(k), also))).collect()),
        other => other.clone(),
    }
}

/// The first ref or cursor in `params` that is from another epoch than `epoch`.
pub fn from_another_epoch(params: &Value, epoch: u32) -> Option<u32> {
    match params {
        Value::String(text) => epoch_of(text).filter(|from| *from != epoch),
        Value::Array(items) => items.iter().find_map(|item| from_another_epoch(item, epoch)),
        Value::Object(map) => map.values().find_map(|value| from_another_epoch(value, epoch)),
        _ => None,
    }
}

/// Whole numbers as integers, so 3 and 3.0 are the same parameter.
pub fn normal(value: &Value) -> Value {
    match value {
        Value::Number(n) => match n.as_f64() {
            Some(f) if f.fract() == 0.0 && f.abs() < 1e15 => json!(f as i64),
            _ => value.clone(),
        },
        Value::Array(items) => Value::Array(items.iter().map(normal).collect()),
        Value::Object(map) => Value::Object(map.iter().map(|(k, v)| (k.clone(), normal(v))).collect()),
        other => other.clone(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const VECTORS: &str = r#"{"cases": [
        {"request": {"method": "read", "params": {"kind": "track", "limit": 2.0}}, "response": {"ok": true, "result": {"epoch": "1", "items": [{"ref": "1:track:1"}], "nextCursor": "1.2"}}},
        {"request": {"method": "read", "params": {"ref": "0:track:1"}}, "response": {"ok": false, "error": {"code": "stale", "message": "that ref is from an earlier epoch (0; now 1): read again"}}}
    ]}"#;

    #[test]
    fn the_cases_are_found_by_method_and_params_whole_numbers_alike() {
        let cases = load_cases(VECTORS, "the test's vectors");
        assert_eq!(cases.len(), 2);
        let page = find(&cases, "read", &json!({"kind": "track", "limit": 2})).expect("the page");
        assert_eq!(page.response.as_ref().unwrap()["nextCursor"], "1.2");
        assert!(find(&cases, "read", &json!({"limit": 2.0, "kind": "track"})).is_some(), "the order of the keys is no difference");
        assert!(find(&cases, "read", &json!({"kind": "track", "limit": 3})).is_none());
        assert!(find(&cases, "snapshot", &json!({"kind": "track", "limit": 2})).is_none());
        let stale = find(&cases, "read", &json!({"ref": "0:track:1"})).expect("the refusal");
        assert_eq!(stale.response.as_ref().unwrap_err().0, "stale");
    }

    #[test]
    fn an_epoch_is_retagged_in_refs_cursors_and_the_epoch_field_only() {
        assert_eq!(epoch_of("1:track:3"), Some(1));
        assert_eq!(epoch_of("12.0.40"), Some(12));
        assert_eq!(epoch_of("2.7"), Some(2));
        assert_eq!((epoch_of("track:3"), epoch_of("1:"), epoch_of("Scratch"), epoch_of("")), (None, None, None, None));
        let answer = json!({"epoch": "1", "layout": 1, "items": [{"ref": "1:track:1", "name": "1:1", "id": "3f9c2a7b1d4e5f60718293a4b5c6d7e8"}], "nextCursor": "1.0.3"});
        let moved = retag(&answer, 1, 2);
        assert_eq!(
            moved,
            json!({"epoch": "2", "layout": 1, "items": [{"ref": "2:track:1", "name": "2:1", "id": "3f9c2a7b1d4e5f60718293a4b5c6d7e8"}], "nextCursor": "2.0.3"})
        );
        assert_eq!(retag(&moved, 2, 1), answer, "and back");
        assert_eq!(retag(&json!({"ref": "3:track:1"}), 1, 2), json!({"ref": "3:track:1"}), "another epoch's refs are left alone");
        let also = |key: &str, text: &str| (key == "id").then(|| Value::String(format!("{text}.2")));
        assert_eq!(retag_with(&answer, 1, 2, None, &also)["items"][0]["id"], "3f9c2a7b1d4e5f60718293a4b5c6d7e8.2");
        assert_eq!(from_another_epoch(&json!({"kind": "note", "parent": "1:region:1", "cursor": "2.3"}), 1), Some(2));
        assert_eq!(from_another_epoch(&json!({"kind": "note", "parent": "1:region:1"}), 1), None);
        assert_eq!(
            normal(&json!({"limit": 2.0, "window": {"from": 0.0, "to": 16.5}})),
            json!({"limit": 2, "window": {"from": 0, "to": 16.5}})
        );
    }
}
