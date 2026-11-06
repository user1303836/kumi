//! A tool input checked against the tool's own JSON schema before Kumi runs it, for the tools the model
//! didn't see up front (see `deferred`). It covers what Kumi's tool schemas use: type, properties,
//! required, additionalProperties, items, enum, const, anyOf, oneOf, not, the numeric bounds, lengths, item and field
//! counts, and pattern. The first problem comes back in words naming the field, so the model can fix it.

use std::collections::HashMap;
use std::sync::{LazyLock, Mutex};

use crate::core::text::cut;
use kumi_common::js::number::to_string as number_text;
use regex::Regex;
use serde_json::{Map, Value};

/// A small candidate built from `schema` alone (never by asking `problem`): required fields, the fewest
/// items, the least number, the first choice. None when it can't find one, such as a string for a pattern
/// none of its tries match. This does not synthesize `oneOf`/`not` combinations or intersect alternatives
/// with outer constraints; callers must validate the candidate before treating it as an accepted example.
pub fn example(schema: &Value) -> Option<Value> {
    let schema = schema.as_object()?;
    if let Some(constant) = schema.get("const") {
        return Some(constant.clone());
    }
    if let Some(first) = schema.get("enum").and_then(Value::as_array).and_then(|choices| choices.first()) {
        return Some(first.clone());
    }
    if let Some(forms) = schema.get("anyOf").and_then(Value::as_array) {
        return forms.iter().find_map(example);
    }
    let types: Vec<&str> = match schema.get("type") {
        Some(Value::String(name)) => vec![name.as_str()],
        Some(Value::Array(names)) => names.iter().filter_map(Value::as_str).collect(),
        // No type: the fields say it's an object, or anything goes.
        _ if schema.contains_key("properties") => vec!["object"],
        _ => vec!["string"],
    };
    types.into_iter().find_map(|name| example_of(name, schema))
}

fn example_of(name: &str, schema: &Map<String, Value>) -> Option<Value> {
    match name {
        "object" => {
            let properties = schema.get("properties").and_then(Value::as_object);
            let mut value = Map::new();
            for key in schema.get("required").and_then(Value::as_array).into_iter().flatten().filter_map(Value::as_str) {
                let field = properties.and_then(|properties| properties.get(key)).map_or(Some(Value::Null), example)?;
                value.insert(key.to_string(), field);
            }
            Some(Value::Object(value))
        }
        "array" => {
            let items = count(schema, "minItems").unwrap_or(0);
            let item = match schema.get("items") {
                Some(each) if items > 0 => Some(example(each)?),
                _ => None,
            };
            Some(Value::Array(item.map(|item| vec![item; items]).unwrap_or_default()))
        }
        "string" => {
            let least = count(schema, "minLength").unwrap_or(0).max(1);
            let most = count(schema, "maxLength").unwrap_or(usize::MAX);
            let pattern = schema.get("pattern").and_then(Value::as_str);
            let mut tries: Vec<String> =
                vec!["a".repeat(least), "c1".into(), "a1".into(), "1".into(), "x_1".into(), "a:1".into(), "1:track:0".into(), "C3".into()];
            tries.extend((1..=least.max(8)).map(|n| "1".repeat(n)));
            // Ids such as a note's p1 or a change's c3.
            tries.extend(('a'..='z').map(|letter| format!("{letter}1")));
            tries
                .into_iter()
                .find(|text| {
                    let length = text.chars().count();
                    length >= least && length <= most && pattern.is_none_or(|pattern| matches_pattern(pattern, text))
                })
                .map(Value::String)
        }
        "number" | "integer" => {
            let least = number(schema, "minimum")
                .or_else(|| {
                    number(schema, "exclusiveMinimum").map(|above| if name == "integer" { above.floor() + 1.0 } else { above + 1.0 })
                })
                .unwrap_or(0.0);
            let least = if name == "integer" { least.ceil() } else { least };
            if number(schema, "maximum").is_some_and(|most| least > most) {
                return None;
            }
            Some(if least.fract() == 0.0 && least.abs() < 1e15 { Value::from(least as i64) } else { Value::from(least) })
        }
        "boolean" => Some(Value::Bool(false)),
        "null" => Some(Value::Null),
        _ => None,
    }
}

/// The fields an object schema takes, required ones first, then the rest with `?` after them.
pub fn fields(schema: &Map<String, Value>) -> Vec<String> {
    let Some(properties) = schema.get("properties").and_then(Value::as_object) else { return Vec::new() };
    let required: Vec<&str> =
        schema.get("required").and_then(Value::as_array).map(|list| list.iter().filter_map(Value::as_str).collect()).unwrap_or_default();
    let mut fields: Vec<String> =
        required.iter().filter(|field| properties.contains_key(**field)).map(|field| (*field).to_string()).collect();
    fields.extend(properties.keys().filter(|key| !required.contains(&key.as_str())).map(|key| format!("{key}?")));
    fields
}

/// The first problem with `value` against `schema`, or None when it fits.
pub fn problem(schema: &Value, value: &Value) -> Option<String> {
    check::<true>(schema, value, "").err()
}

/// How a field is named in a problem: the input itself, or its path in it ("steps[2].tool").
fn field(path: &str) -> String {
    if path.is_empty() {
        "The input".into()
    } else {
        format!("`{path}`")
    }
}

fn join(path: &str, key: &str) -> String {
    if path.is_empty() {
        key.into()
    } else {
        format!("{path}.{key}")
    }
}

fn kind_of(value: &Value) -> &'static str {
    match value {
        Value::Null => "null",
        Value::Bool(_) => "true or false",
        Value::Number(_) => "a number",
        Value::String(_) => "a string",
        Value::Array(_) => "a list",
        Value::Object(_) => "an object",
    }
}

fn type_words(name: &str) -> &str {
    match name {
        "string" => "a string",
        "number" => "a number",
        "integer" => "a whole number",
        "boolean" => "true or false",
        "object" => "an object",
        "array" => "a list",
        "null" => "null",
        other => other,
    }
}

fn is_type(name: &str, value: &Value) -> bool {
    match name {
        "string" => value.is_string(),
        "number" => value.is_number(),
        "integer" => value.as_f64().is_some_and(|number| number.fract() == 0.0),
        "boolean" => value.is_boolean(),
        "object" => value.is_object(),
        "array" => value.is_array(),
        "null" => value.is_null(),
        _ => true,
    }
}

/// JSON's equality: 1 and 1.0 are the same number (serde_json's `Value` keeps them apart).
fn same(a: &Value, b: &Value) -> bool {
    match (a.as_f64(), b.as_f64()) {
        (Some(a), Some(b)) => a == b,
        _ => a == b,
    }
}

fn shown(value: &Value) -> String {
    match value {
        Value::String(text) => format!("\"{}\"", cut(text, 40)),
        other => cut(&other.to_string(), 40).to_owned(),
    }
}

/// Patterns compiled once each; one that won't compile checks nothing.
static PATTERNS: LazyLock<Mutex<HashMap<String, Option<Regex>>>> = LazyLock::new(|| Mutex::new(HashMap::new()));

fn matches_pattern(pattern: &str, text: &str) -> bool {
    let mut patterns = PATTERNS.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
    let compiled = patterns.entry(pattern.to_string()).or_insert_with(|| Regex::new(pattern).ok());
    compiled.as_ref().is_none_or(|regex| regex.is_match(text))
}

fn number(schema: &Map<String, Value>, key: &str) -> Option<f64> {
    schema.get(key).and_then(Value::as_f64)
}

fn count(schema: &Map<String, Value>, key: &str) -> Option<usize> {
    schema.get(key).and_then(Value::as_u64).map(|n| n as usize)
}

// Alternative probes need only acceptance, not allocated paths or explanations for discarded forms.
fn check<const EXPLAIN: bool>(schema: &Value, value: &Value, path: &str) -> Result<(), String> {
    macro_rules! reject {
        ($($message:tt)*) => {
            return Err(if EXPLAIN { format!($($message)*) } else { String::new() })
        };
    }
    let Some(schema) = schema.as_object() else { return Ok(()) };
    match schema.get("type") {
        Some(Value::String(name)) if !is_type(name, value) => {
            reject!("{} must be {}, not {}.", field(path), type_words(name), kind_of(value));
        }
        Some(Value::Array(names)) if !names.iter().filter_map(Value::as_str).any(|name| is_type(name, value)) => {
            if !EXPLAIN {
                return Err(String::new());
            }
            let words: Vec<&str> = names.iter().filter_map(Value::as_str).map(type_words).collect();
            reject!("{} must be {}, not {}.", field(path), words.join(" or "), kind_of(value));
        }
        _ => {}
    }
    if let Some(allowed) = schema.get("enum").and_then(Value::as_array) {
        if !allowed.iter().any(|choice| same(choice, value)) {
            if !EXPLAIN {
                return Err(String::new());
            }
            let listed: Vec<String> = allowed.iter().take(24).map(shown).collect();
            let more = if allowed.len() > 24 { ", …" } else { "" };
            reject!("{} must be one of {}{more}, not {}.", field(path), listed.join(", "), shown(value));
        }
    }
    if let Some(constant) = schema.get("const") {
        if !same(constant, value) {
            reject!("{} must be {}.", field(path), shown(constant));
        }
    }
    match value {
        Value::Number(_) => {
            let n = value.as_f64().unwrap_or(f64::NAN);
            if let Some(least) = number(schema, "minimum").filter(|least| n < *least) {
                reject!("{} must be at least {}, not {}.", field(path), number_text(least), number_text(n));
            }
            if let Some(most) = number(schema, "maximum").filter(|most| n > *most) {
                reject!("{} must be at most {}, not {}.", field(path), number_text(most), number_text(n));
            }
            if let Some(above) = number(schema, "exclusiveMinimum").filter(|above| n <= *above) {
                reject!("{} must be more than {}, not {}.", field(path), number_text(above), number_text(n));
            }
        }
        Value::String(text) => {
            let length = text.chars().count();
            if let Some(least) = count(schema, "minLength").filter(|least| length < *least) {
                reject!("{} must have at least {least} characters.", field(path));
            }
            if let Some(most) = count(schema, "maxLength").filter(|most| length > *most) {
                reject!("{} must have at most {most} characters, not {length}.", field(path));
            }
            if let Some(pattern) = schema.get("pattern").and_then(Value::as_str) {
                if !matches_pattern(pattern, text) {
                    reject!("{} must match {pattern}, not {}.", field(path), shown(value));
                }
            }
        }
        Value::Array(items) => {
            if let Some(least) = count(schema, "minItems").filter(|least| items.len() < *least) {
                reject!("{} must have at least {least} items.", field(path));
            }
            if let Some(most) = count(schema, "maxItems").filter(|most| items.len() > *most) {
                reject!("{} must have at most {most} items, not {}.", field(path), items.len());
            }
            if let Some(each) = schema.get("items") {
                for (index, item) in items.iter().enumerate() {
                    let child = if EXPLAIN { format!("{path}[{index}]") } else { String::new() };
                    check::<EXPLAIN>(each, item, &child)?;
                }
            }
        }
        Value::Object(fields) => {
            if let Some(required) = schema.get("required").and_then(Value::as_array) {
                if let Some(missing) = required.iter().filter_map(Value::as_str).find(|key| !fields.contains_key(*key)) {
                    reject!("{} is required.", field(&join(path, missing)));
                }
            }
            if let Some(most) = count(schema, "maxProperties").filter(|most| fields.len() > *most) {
                reject!("{} must have at most {most} fields.", field(path));
            }
            let properties = schema.get("properties").and_then(Value::as_object);
            for (key, item) in fields {
                match (properties.and_then(|properties| properties.get(key)), schema.get("additionalProperties")) {
                    (Some(own), _) => {
                        let child = if EXPLAIN { join(path, key) } else { String::new() };
                        check::<EXPLAIN>(own, item, &child)?;
                    }
                    (None, Some(Value::Bool(false))) => {
                        if !EXPLAIN {
                            return Err(String::new());
                        }
                        let known: Vec<&str> =
                            properties.map(|properties| properties.keys().map(String::as_str).collect()).unwrap_or_default();
                        let takes = if known.is_empty() {
                            "no fields".to_string()
                        } else {
                            let more = if known.len() > 24 { ", …" } else { "" };
                            format!("{}{more}", known.iter().take(24).copied().collect::<Vec<_>>().join(", "))
                        };
                        reject!("{} has no field `{key}` (it takes {takes}).", field(path));
                    }
                    (None, Some(other @ Value::Object(_))) => {
                        let child = if EXPLAIN { join(path, key) } else { String::new() };
                        check::<EXPLAIN>(other, item, &child)?;
                    }
                    (None, _) => {}
                }
            }
        }
        _ => {}
    }
    if let Some(forms) = schema.get("anyOf").and_then(Value::as_array) {
        if !forms.iter().any(|form| check::<false>(form, value, "").is_ok()) {
            return Err(if EXPLAIN { nearest_problem(forms, value, path) } else { String::new() });
        }
    }
    if let Some(forms) = schema.get("oneOf").and_then(Value::as_array) {
        let mut matching = forms.iter().filter(|form| check::<false>(form, value, "").is_ok());
        let Some(first) = matching.next() else {
            return Err(if EXPLAIN { nearest_problem(forms, value, path) } else { String::new() });
        };
        if let Some(second) = matching.next() {
            reject!("{} fits both {} and {}; choose only one form.", field(path), form_name(first), form_name(second));
        }
    }
    if let Some(excluded) = schema.get("not").filter(|excluded| check::<false>(excluded, value, "").is_ok()) {
        if !EXPLAIN {
            return Err(String::new());
        }
        // A forbidden union often names each incompatible field separately (for example create + kind).
        let witness = excluded
            .get("anyOf")
            .and_then(Value::as_array)
            .and_then(|forms| forms.iter().find(|form| check::<false>(form, value, "").is_ok()))
            .unwrap_or(excluded);
        reject!("{} includes a disallowed {}.", field(path), form_name(witness));
    }
    Ok(())
}

/// Rank only failed alternatives, never the acceptance path. A supplied discriminator that disagrees
/// rules a form out before shared discriminators or required fields make it look superficially close.
fn form_rank(form: &Value, value: &Value) -> (std::cmp::Reverse<usize>, usize, bool, usize, std::cmp::Reverse<usize>) {
    let mut mismatched = 0;
    let mut matched = 0;
    if let Some(properties) = form.get("properties").and_then(Value::as_object) {
        for (key, property) in properties {
            if let (Some(constant), Some(supplied)) = (property.get("const"), value.get(key)) {
                if same(constant, supplied) {
                    matched += 1;
                } else {
                    mismatched += 1;
                }
            }
        }
    }
    let typed = match form.get("type") {
        Some(Value::String(name)) => is_type(name, value),
        Some(Value::Array(names)) => names.iter().filter_map(Value::as_str).any(|name| is_type(name, value)),
        _ => true,
    };
    let mut supplied = 0;
    let mut missing = 0;
    for key in form.get("required").and_then(Value::as_array).into_iter().flatten().filter_map(Value::as_str) {
        if value.get(key).is_some() {
            supplied += 1;
        } else {
            missing += 1;
        }
    }
    (std::cmp::Reverse(mismatched), matched, typed, supplied, std::cmp::Reverse(missing))
}

fn form_name(form: &Value) -> String {
    let mut fields = Vec::new();
    if let Some(properties) = form.get("properties").and_then(Value::as_object) {
        for (key, property) in properties {
            if let Some(constant) = property.get("const") {
                fields.push(format!("{key}: {}", shown(constant)));
            }
        }
    }
    for key in form.get("required").and_then(Value::as_array).into_iter().flatten().filter_map(Value::as_str) {
        if form.get("properties").and_then(|properties| properties.get(key)).and_then(|property| property.get("const")).is_none() {
            fields.push(key.to_owned());
        }
    }
    if !fields.is_empty() {
        format!("form {{{}}}", fields.join(", "))
    } else if let Some(constant) = form.get("const") {
        format!("form {}", shown(constant))
    } else if let Some(name) = form.get("type").and_then(Value::as_str) {
        format!("{} form", type_words(name))
    } else {
        "allowed form".into()
    }
}

fn nearest_problem(forms: &[Value], value: &Value, path: &str) -> String {
    let Some(mut nearest) = forms.first() else {
        return format!("{} has no allowed form.", field(path));
    };
    let mut rank = form_rank(nearest, value);
    for form in &forms[1..] {
        let candidate = form_rank(form, value);
        if candidate > rank {
            nearest = form;
            rank = candidate;
        }
    }
    let why = check::<true>(nearest, value, path).expect_err("every alternative has failed");
    format!("For {}: {why}", form_name(nearest))
}

#[cfg(test)]
mod tests {
    use super::problem;
    use serde_json::json;

    fn schema() -> serde_json::Value {
        json!({
            "type": "object",
            "additionalProperties": false,
            "required": ["track", "beats"],
            "properties": {
                "track": {"type": "string", "minLength": 1, "maxLength": 8, "pattern": "^[a-z]+:\\d+$"},
                "beats": {"type": "number", "exclusiveMinimum": 0, "maximum": 64},
                "bars": {"type": "integer", "minimum": 1},
                "focus": {"type": "string", "enum": ["tone", "section"]},
                "mix": {"const": true},
                "steps": {"type": "array", "maxItems": 2, "items": {"type": "object", "required": ["tool"], "properties": {"tool": {"type": "string"}}}},
                "sample": {"anyOf": [{"type": "string", "minLength": 1}, {"type": "object", "additionalProperties": false, "properties": {"random": {"type": "boolean"}}}]},
                "either": {"type": ["string", "number"]}
            }
        })
    }

    #[test]
    fn a_source_union_accepts_later_object_forms_without_bypassing_its_outer_bounds() {
        let source = json!({
            "anyOf": [
                {"type": "object", "required": ["sound_id"], "properties": {"sound_id": {"type": "string"}}, "additionalProperties": false},
                {"type": "object", "required": ["file"], "properties": {"file": {"type": "string"}}, "additionalProperties": false}
            ]
        });
        assert!(problem(&source, &json!({"sound_id": "s1"})).is_none());
        assert!(problem(&source, &json!({"file": "melody.wav"})).is_none());
        assert!(problem(&source, &json!({"file": "melody.wav", "sound_id": "s1"})).is_some());
        assert!(problem(&source, &json!({"file": 7})).is_some());
        let bounded = json!({"anyOf": [{"type": "string", "minLength": 2}], "maxLength": 3});
        assert!(problem(&bounded, &json!("abc")).is_none());
        assert!(problem(&bounded, &json!("abcd")).is_some());
        assert!(problem(&json!({"anyOf": []}), &json!({})).is_some());
    }

    #[test]
    fn exclusive_forms_and_forbidden_combinations_reject_ambiguous_requests() {
        let source = json!({"type": "object", "oneOf": [
            {"required": ["file"], "not": {"required": ["artifact_id"]}},
            {"required": ["artifact_id"], "not": {"required": ["file"]}}
        ]});
        assert!(problem(&source, &json!({"file": "melody.wav"})).is_none());
        assert!(problem(&source, &json!({"artifact_id": "a1"})).is_none());
        assert!(problem(&source, &json!({})).is_some());
        assert!(problem(&source, &json!({"file": "melody.wav", "artifact_id": "a1"})).is_some());
        let overlap = json!({"oneOf": [{"const": 1}, {"type": "integer", "minimum": 0}]});
        assert!(problem(&overlap, &json!(1)).is_some());
        assert!(problem(&overlap, &json!(2)).is_none());
    }

    #[test]
    fn not_rejects_on_its_own_without_an_exclusive_union() {
        let schema = json!({"type": "object", "not": {"required": ["file", "artifact_id"]}});
        for input in [json!({}), json!({"file": "x.wav"}), json!({"artifact_id": "a1"})] {
            assert!(problem(&schema, &input).is_none(), "{input}");
        }
        assert!(problem(&schema, &json!({"file": "x.wav", "artifact_id": "a1"})).is_some());
        let numbers = json!({"type": "integer", "not": {"minimum": 0}});
        assert!(problem(&numbers, &json!(-1)).is_none());
        assert!(problem(&numbers, &json!(0)).is_some());
        assert!(problem(&numbers, &json!(1)).is_some());
    }

    #[test]
    fn alternatives_never_replace_outer_constraints() {
        for combinator in ["anyOf", "oneOf"] {
            let mut schema = json!({
                "type": "object", "required": ["name"], "additionalProperties": false,
                "maxProperties": 2,
                "properties": {
                    "name": {"type": "string", "minLength": 1},
                    "count": {"type": "integer", "minimum": 1},
                    "flag": {"type": "boolean"}
                }
            });
            schema[combinator] = json!([{"properties": {"count": {"maximum": 5}}}]);
            assert!(problem(&schema, &json!({"name": "x", "count": 5})).is_none());
            for input in [
                json!([]),
                json!({}),
                json!({"name": ""}),
                json!({"name": 1}),
                json!({"name": "x", "count": 0}),
                json!({"name": "x", "count": 6}),
                json!({"name": "x", "extra": true}),
                json!({"name": "x", "count": 1, "flag": true}),
            ] {
                assert!(problem(&schema, &input).is_some(), "{combinator}: {input}");
            }
            // Even when no alternative fits, the node's own property error wins.
            schema[combinator] = json!([{"required": ["unrelated"]}]);
            let why = problem(&schema, &json!({"name": 1})).unwrap();
            assert!(why.contains("name"), "{why}");
            assert!(!why.contains("unrelated"), "{why}");
            for (mut bounded, accepted, rejected) in [
                (json!({"type": "number", "maximum": 5}), json!(5), json!(6)),
                (json!({"type": "string", "maxLength": 3}), json!("abc"), json!("abcd")),
                (json!({"type": "array", "maxItems": 1}), json!([1]), json!([1, 2])),
            ] {
                bounded[combinator] = json!([{}]);
                assert!(problem(&bounded, &accepted).is_none(), "{combinator}: {accepted}");
                assert!(problem(&bounded, &rejected).is_some(), "{combinator}: {rejected}");
            }
        }
    }

    #[test]
    fn an_input_that_fits_has_no_problem() {
        let fits = json!({"track": "track:3", "beats": 8, "bars": 2, "focus": "tone", "mix": true, "steps": [{"tool": "x"}], "sample": {"random": true}, "either": 4});
        assert_eq!(problem(&schema(), &fits), None);
    }

    #[test]
    fn each_problem_names_its_field() {
        let cases = [
            (json!({"beats": 8}), "track"),
            (json!({"track": "track:3", "beats": "8"}), "beats"),
            (json!({"track": "track:3", "beats": 0}), "beats"),
            (json!({"track": "track:3", "beats": 65}), "beats"),
            (json!({"track": "track:3", "beats": 8, "bars": 1.5}), "bars"),
            (json!({"track": "track:3", "beats": 8, "bars": 0}), "bars"),
            (json!({"track": "track:3", "beats": 8, "focus": "loud"}), "focus"),
            (json!({"track": "track:3", "beats": 8, "mix": false}), "mix"),
            (json!({"track": "trk", "beats": 8}), "track"),
            (json!({"track": "track:123", "beats": 8}), "track"),
            (json!({"track": "track:3", "beats": 8, "loud": true}), "loud"),
            (json!({"track": "track:3", "beats": 8, "steps": [{}, {}]}), "steps[0].tool"),
            (json!({"track": "track:3", "beats": 8, "steps": [{"tool": 1}]}), "steps[0].tool"),
            (json!({"track": "track:3", "beats": 8, "steps": [{"tool": "a"}, {"tool": "b"}, {"tool": "c"}]}), "steps"),
            (json!({"track": "track:3", "beats": 8, "sample": ""}), "sample"),
            (json!({"track": "track:3", "beats": 8, "sample": {"words": []}}), "sample"),
            (json!({"track": "track:3", "beats": 8, "either": true}), "either"),
        ];
        for (input, want) in cases {
            let why = problem(&schema(), &input).expect("invalid input must be rejected");
            assert!(why.contains(want), "{input}: {why}");
        }
        assert!(problem(&schema(), &json!(["track"])).is_some());
    }

    #[test]
    fn a_number_matches_an_enum_or_const_whatever_its_form() {
        let schema = json!({"type":"object","properties":{"steps":{"enum":[1, 2.5, "4"]},"one":{"const":1.0}}});
        assert_eq!(problem(&schema, &json!({"steps": 1.0, "one": 1})), None);
        assert_eq!(problem(&schema, &json!({"steps": 2.5})), None);
        assert!(problem(&schema, &json!({"steps": 4})).is_some(), "a number isn't the string \"4\"");
        assert!(problem(&schema, &json!({"one": "1"})).is_some());
    }

    #[test]
    fn a_schemas_own_example_fits_it() {
        let example = super::example(&schema()).unwrap();
        assert_eq!(example, json!({"track": "a:1", "beats": 1}));
        assert_eq!(problem(&schema(), &example), None);
        assert_eq!(super::example(&json!({"type": "string", "pattern": "^q[0-9]{9}$"})), None, "no try fits");
    }

    #[test]
    fn a_pattern_that_wont_compile_checks_nothing() {
        let schema = json!({"type": "string", "pattern": "(unclosed"});
        assert_eq!(problem(&schema, &json!("anything")), None);
    }

    /// A long value an error shows is cut between characters, never through one (#675).
    #[test]
    fn a_long_value_an_error_shows_is_cut_between_characters() {
        let input = json!({"track": "track:3", "beats": 8, "focus": format!("{}🎹", "a".repeat(39))});
        let why = problem(&schema(), &input).unwrap();
        assert!(why.contains(&"a".repeat(39)));
        assert!(!why.contains('🎹'));
        assert!(why.len() < 200);
    }
}
