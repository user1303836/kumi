//! Exact nested snapshot schema and digest verification.
use super::*;
pub(super) fn only(value: &Value, allowed: &[&str], label: &str) -> Result<(), ProjectError> {
    if value.as_object().is_none_or(|o| o.keys().any(|k| !allowed.contains(&k.as_str()))) {
        return Err(fail(format!("semantic snapshot {label} has unknown or malformed fields")));
    }
    Ok(())
}
pub(super) fn required(value: &Value, keys: &[&str], label: &str) -> Result<(), ProjectError> {
    only(value, keys, label)?;
    if keys.iter().any(|k| value.get(*k).is_none()) {
        return Err(fail(format!("semantic snapshot {label} is missing required fields")));
    }
    Ok(())
}
fn string_value(v: &Value, label: &str, pattern: Option<&Regex>) -> Result<(), ProjectError> {
    if v.as_str().is_none_or(|s| s.is_empty() || utf16_len(s) > 4096 || pattern.is_some_and(|p| !p.is_match(s))) {
        return Err(fail(format!("semantic snapshot {label} is invalid")));
    }
    Ok(())
}
fn shape(v: &Value, label: &str, depth: usize) -> Result<(), ProjectError> {
    if depth > 24 {
        return Err(fail(format!("semantic snapshot {label} exceeds depth")));
    }
    match v {
        Value::String(s) if utf16_len(s) > 4096 => return Err(fail(format!("semantic snapshot {label} string exceeds bound"))),
        Value::Array(rows) => {
            if rows.len() > 256 {
                return Err(fail(format!("semantic snapshot {label} array exceeds bound")));
            }
            for row in rows {
                shape(row, label, depth + 1)?;
            }
        }
        Value::Object(o) => {
            if o.len() > 64 || o.keys().any(|k| utf16_len(k) > 128) {
                return Err(fail(format!("semantic snapshot {label} object exceeds bound")));
            }
            for child in o.values() {
                shape(child, label, depth + 1)?;
            }
        }
        _ => {}
    }
    Ok(())
}
fn nullable_number(v: &Value) -> bool {
    v.is_null() || v.as_f64().is_some_and(f64::is_finite)
}
fn nullable_boolean(v: &Value) -> bool {
    v.is_null() || v.is_boolean()
}
fn nullable_string(v: &Value) -> bool {
    v.is_null() || v.is_string()
}
fn values(v: &Value, keys: &[&str], predicate: fn(&Value) -> bool, label: &str) -> Result<(), ProjectError> {
    if keys.iter().any(|k| v.get(*k).is_none_or(|v| !predicate(v))) {
        return Err(fail(format!("semantic snapshot {label} has invalid field types")));
    }
    Ok(())
}
fn one_of(v: &Value, allowed: &[&str]) -> bool {
    v.as_str().is_some_and(|s| allowed.contains(&s))
}
fn hash(v: &Value) -> bool {
    static P: LazyLock<Regex> = LazyLock::new(|| regex(r"^sha256:[a-f0-9]{64}$"));
    v.as_str().is_some_and(|s| P.is_match(s))
}
fn raw_hash(v: &Value) -> bool {
    static P: LazyLock<Regex> = LazyLock::new(|| regex(r"^[a-f0-9]{64}$"));
    v.as_str().is_some_and(|s| P.is_match(s))
}
fn coordinate(v: &Value) -> bool {
    static P: LazyLock<Regex> = LazyLock::new(|| regex(r"^(?:track|device|chain|drum-pad)-snapshot:[a-f0-9]{20}(?:-[1-9][0-9]*)?$"));
    v.is_null() || v.as_str().is_some_and(|s| P.is_match(s))
}
fn same_fields(a: &Value, b: &Value, keys: &[&str]) -> bool {
    keys.iter().all(|k| js_equal(&a[*k], &b[*k]))
}
fn nested(record: &Value) -> Result<(), ProjectError> {
    let data = &record["data"];
    let matching = &record["matching"];
    let kind = record["kind"].as_str().unwrap_or("");
    let keys = match kind {
        "set" => &["kind"][..],
        "track" => &["trackKind", "structureHash"],
        "scene" => &["structureHash"],
        "locator" => &["position"],
        "clip" => &["clipKind", "noteHash", "length", "audioMetadataHash"],
        "device" => &["deviceKind", "className", "parameterSchemaHash", "opaqueState"],
        "dependency" if data["category"] == "media" => &["category", "origin", "locatorDigest"],
        "dependency" => &["category", "className", "opaqueState"],
        "unavailable" => &["field", "source"],
        _ => &[],
    };
    required(matching, keys, &format!("{kind} matching data"))?;
    let (valid, label) = match kind {
        "set" => {
            required(data, &["tempo", "arrangementLength", "trackCount", "sceneCount"], "Set record data")?;
            values(data, &["tempo", "arrangementLength"], nullable_number, "Set record data")?;
            (
                nonnegative_integer(&data["trackCount"]) && nonnegative_integer(&data["sceneCount"]) && matching["kind"] == "set",
                "Set record values",
            )
        }
        "track" => {
            required(
                data,
                &["kind", "mixer", "routing", "armed", "monitoring", "clipCount", "deviceCount", "structureHash", "groupSnapshotId"],
                "track record data",
            )?;
            let mixer = &data["mixer"];
            let routing = &data["routing"];
            only(
                mixer,
                &[
                    "volume",
                    "pan",
                    "cueVolume",
                    "mute",
                    "solo",
                    "sends",
                    "trackActivator",
                    "crossfader",
                    "crossfadeAssign",
                    "panningMode",
                    "panningLeft",
                    "panningRight",
                ],
                "track mixer",
            )?;
            only(routing, &["inputType", "inputSubRouting", "outputType", "outputSubRouting"], "track routing")?;
            let number_keys: Vec<_> =
                ["volume", "pan", "cueVolume", "crossfader", "crossfadeAssign", "panningMode", "panningLeft", "panningRight"]
                    .into_iter()
                    .filter(|k| mixer.get(*k).is_some())
                    .collect();
            values(mixer, &number_keys, nullable_number, "track mixer")?;
            let bool_keys: Vec<_> = ["mute", "solo", "trackActivator"].into_iter().filter(|k| mixer.get(*k).is_some()).collect();
            values(mixer, &bool_keys, nullable_boolean, "track mixer")?;
            if mixer["sends"].as_array().is_none_or(|rows| rows.len() > 128 || !rows.iter().all(nullable_number)) {
                return Err(fail("semantic snapshot track sends are invalid"));
            }
            let keys: Vec<_> = routing.as_object().unwrap().keys().map(String::as_str).collect();
            values(routing, &keys, nullable_string, "track routing")?;
            (
                one_of(&data["kind"], &["audio", "midi", "group", "return", "main", "master", "regular"])
                    && nullable_boolean(&data["armed"])
                    && (data["monitoring"].is_null() || one_of(&data["monitoring"], &["in", "auto", "off"]))
                    && nonnegative_integer(&data["clipCount"])
                    && nonnegative_integer(&data["deviceCount"])
                    && hash(&data["structureHash"])
                    && coordinate(&data["groupSnapshotId"])
                    && matching["trackKind"] == data["kind"]
                    && matching["structureHash"] == data["structureHash"],
                "track values",
            )
        }
        "scene" => {
            required(
                data,
                &["colorIndex", "tempo", "tempoEnabled", "signatureNumerator", "signatureDenominator", "isEmpty", "structureHash"],
                "scene record data",
            )?;
            values(data, &["colorIndex", "tempo", "signatureNumerator", "signatureDenominator"], nullable_number, "scene data")?;
            values(data, &["tempoEnabled", "isEmpty"], nullable_boolean, "scene data")?;
            (
                hash(&data["structureHash"])
                    && matching["structureHash"] == data["structureHash"]
                    && ["colorIndex", "signatureNumerator", "signatureDenominator"]
                        .iter()
                        .all(|k| data[k].is_null() || nonnegative_integer(&data[k])),
                "scene values",
            )
        }
        "locator" => {
            required(data, &["position"], "locator record data")?;
            (data["position"].is_number() && js_equal(&matching["position"], &data["position"]), "locator values")
        }
        "clip" => {
            required(
                data,
                &[
                    "clipKind",
                    "parentSnapshotId",
                    "location",
                    "start",
                    "length",
                    "loopStart",
                    "loopEnd",
                    "looping",
                    "muted",
                    "notes",
                    "automation",
                    "audioMetadataHash",
                    "rawAudioContent",
                ],
                "clip record data",
            )?;
            let location = &data["location"];
            let notes = &data["notes"];
            let automation = &data["automation"];
            only(location, &["lane", "sceneOrder", "laneOrder"], "clip location")?;
            required(notes, &["count", "pitchMin", "pitchMax", "end", "hash"], "clip note summary")?;
            required(automation, &["envelopeCount", "pointCount", "contentHash"], "clip automation summary")?;
            (
                one_of(&data["clipKind"], &["midi", "audio"])
                    && coordinate(&data["parentSnapshotId"])
                    && one_of(&location["lane"], &["session", "take-lane", "arrangement"])
                    && location.get("sceneOrder").is_none_or(|v| v.is_null() || nonnegative_integer(v))
                    && location.get("laneOrder").is_none_or(nonnegative_integer)
                    && ["start", "length"].iter().all(|k| data[k].as_f64().is_some_and(f64::is_finite))
                    && data["length"].as_f64().is_some_and(|n| n >= 0.)
                    && ["loopStart", "loopEnd"].iter().all(|k| nullable_number(&data[k]))
                    && ["looping", "muted"].iter().all(|k| nullable_boolean(&data[k]))
                    && nonnegative_integer(&notes["count"])
                    && ["pitchMin", "pitchMax", "end"].iter().all(|k| nullable_number(&notes[k]))
                    && hash(&notes["hash"])
                    && nonnegative_integer(&automation["envelopeCount"])
                    && nonnegative_integer(&automation["pointCount"])
                    && hash(&automation["contentHash"])
                    && hash(&data["audioMetadataHash"])
                    && data["rawAudioContent"] == "unavailable-not-read"
                    && same_fields(matching, data, &["clipKind", "length", "audioMetadataHash"])
                    && matching["noteHash"] == notes["hash"],
                "clip values",
            )
        }
        "device" => {
            required(
                data,
                &[
                    "deviceKind",
                    "className",
                    "parentSnapshotId",
                    "depth",
                    "siblingOrder",
                    "parameterSchemaHash",
                    "parameterStateHash",
                    "opaqueState",
                    "state",
                ],
                "device record data",
            )?;
            let state = &data["state"];
            required(
                state,
                &[
                    "enabled",
                    "latencySamples",
                    "parameterCount",
                    "pluginPresetIndex",
                    "pluginPresetCount",
                    "rackVariationCount",
                    "selectedVariationIndex",
                    "specializedHash",
                ],
                "device visible state",
            )?;
            (
                one_of(&data["deviceKind"], &["instrument", "audio-effect", "midi-effect", "plugin", "rack", "device"])
                    && data["className"].is_string()
                    && coordinate(&data["parentSnapshotId"])
                    && nonnegative_integer(&data["depth"])
                    && data["depth"].as_f64().is_some_and(|n| n <= 8.)
                    && nonnegative_integer(&data["siblingOrder"])
                    && hash(&data["parameterSchemaHash"])
                    && hash(&data["parameterStateHash"])
                    && data["opaqueState"].is_boolean()
                    && nullable_boolean(&state["enabled"])
                    && [
                        "latencySamples",
                        "parameterCount",
                        "pluginPresetIndex",
                        "pluginPresetCount",
                        "rackVariationCount",
                        "selectedVariationIndex",
                    ]
                    .iter()
                    .all(|k| nullable_number(&state[k]))
                    && ["parameterCount", "pluginPresetIndex", "pluginPresetCount", "rackVariationCount", "selectedVariationIndex"]
                        .iter()
                        .all(|k| state[k].is_null() || nonnegative_integer(&state[k]))
                    && hash(&state["specializedHash"])
                    && same_fields(matching, data, &["deviceKind", "className", "parameterSchemaHash", "opaqueState"]),
                "device values",
            )
        }
        "dependency" => {
            let keys =
                ["category", "origin", "availability", "stateVisibility", "locator", "evidence", "classificationEvidence", "portability"];
            let mut allowed = keys.to_vec();
            allowed.push("locatorDigest");
            only(data, &allowed, "dependency record data")?;
            let media = data["category"] == "media";
            (
                keys.iter().all(|k| data.get(*k).is_some())
                    && one_of(&data["category"], &["media", "plug-in", "max-device"])
                    && one_of(&data["origin"], &["project-local", "external", "pack", "user-library", "unknown", "plug-in", "max"])
                    && one_of(&data["availability"], &["missing", "discovered", "unknown"])
                    && one_of(&data["stateVisibility"], &["opaque", "semantic"])
                    && ["locator", "evidence", "classificationEvidence", "portability"].iter().all(|k| data[k].is_string())
                    && data["portability"] == "unknown"
                    && one_of(&data["evidence"], &["live-device", "als-file-ref", "live-clip"])
                    && one_of(
                        &data["classificationEvidence"],
                        &[
                            "live-generic-class",
                            "verified-realpath",
                            "missing-lexical-project-path",
                            "path-segment-heuristic",
                            "network-reference-blocked",
                            "oversized-reference-blocked",
                            "path-evidence",
                        ],
                    )
                    && if media {
                        hash(&data["locatorDigest"])
                            && matching["category"] == "media"
                            && same_fields(matching, data, &["origin", "locatorDigest"])
                    } else {
                        data.get("locatorDigest").is_none()
                            && one_of(&matching["category"], &["plug-in", "max-device"])
                            && matching["className"].is_string()
                            && matching["opaqueState"] == true
                            && data["availability"] == "discovered"
                            && data["stateVisibility"] == "opaque"
                    }
                    && (data["category"] != "plug-in" || data["origin"] == "plug-in")
                    && (data["category"] != "max-device" || data["origin"] == "max"),
                "dependency record data",
            )
        }
        "unavailable" => {
            required(data, &["field", "reason", "source", "state"], "unavailable record data")?;
            (
                ["field", "reason", "source"].iter().all(|k| data[k].is_string())
                    && data["state"] == "unavailable"
                    && same_fields(matching, data, &["field", "source"]),
                "unavailable values",
            )
        }
        _ => return Err(fail("semantic snapshot record kind is invalid")),
    };
    if !valid {
        return Err(fail(format!("semantic snapshot {label} are invalid").replace("record data are invalid", "record data is invalid")));
    }
    shape(data, &format!("{kind} data"), 0)?;
    shape(matching, &format!("{kind} matching data"), 0)?;
    Ok(())
}
pub fn validate_semantic_project_artifact(artifact: &Value) -> Result<(), ProjectError> {
    if artifact["schema"] != SEMANTIC_PROJECT_SNAPSHOT_SCHEMA
        || artifact["records"].as_array().is_none_or(|r| r.len() > SEMANTIC_PROJECT_MAX_RECORDS)
    {
        return Err(fail("semantic snapshot artifact schema or record bound is invalid"));
    }
    required(artifact, &["schema", "artifact", "policy", "provenance", "set", "manifest", "safety", "records"], "artifact")?;
    required(&artifact["artifact"], &["id", "semanticHash", "exporterVersion"], "identity")?;
    required(&artifact["policy"], &["profile", "names", "paths"], "policy")?;
    let provenance = &artifact["provenance"];
    only(provenance, &["source", "live", "setFileSha256", "ableton", "limitations"], "provenance")?;
    if ["source", "live", "limitations"].iter().any(|k| provenance.get(*k).is_none()) {
        return Err(fail("semantic snapshot provenance is incomplete"));
    }
    let live = &provenance["live"];
    let mut live_keys = vec!["protocol", "adapter", "provenance"];
    for key in ["registryHash", "version"] {
        if live.get(key).is_some() {
            live_keys.push(key);
        }
    }
    required(live, &live_keys, "Live provenance")?;
    static VERSION: LazyLock<Regex> = LazyLock::new(|| regex(r"^[A-Za-z0-9 ._+()-]+$"));
    static HASH: LazyLock<Regex> = LazyLock::new(|| regex(r"^sha256:[a-f0-9]{64}$"));
    if let Some(ableton) = provenance.get("ableton") {
        only(ableton, &["creator", "majorVersion", "minorVersion", "schemaChangeCount"], "Ableton provenance")?;
        for value in ableton.as_object().unwrap().values() {
            string_value(value, "Ableton provenance value", Some(&VERSION))?;
        }
    }
    required(&artifact["set"], &["name", "tempo", "arrangementLength", "trackCount", "sceneCount"], "Set summary")?;
    required(
        &artifact["safety"],
        &["readOnly", "containsSessionReferences", "containsMutationAuthority", "crossRunIdentityClaimed", "mergeProposed"],
        "safety contract",
    )?;
    string_value(&artifact["artifact"]["id"], "artifact id", Some(&HASH))?;
    string_value(&artifact["artifact"]["semanticHash"], "semantic hash", Some(&HASH))?;
    string_value(&artifact["artifact"]["exporterVersion"], "exporter version", None)?;
    let policy = &artifact["policy"];
    let tuple = match policy["profile"].as_str() {
        Some("strict") => Some(("typed-aliases", "typed-digests")),
        Some("collaboration") => Some(("retained", "basenames")),
        Some("local") => Some(("retained", "project-relative-or-basename")),
        _ => None,
    };
    if tuple.is_none_or(|(a, b)| policy["names"] != a || policy["paths"] != b) {
        return Err(fail("semantic snapshot privacy policy tuple is invalid"));
    }
    static PROTOCOL: LazyLock<Regex> = LazyLock::new(|| regex(r"^[a-z][a-z0-9-]*/v[1-9][0-9]*$"));
    if !one_of(&provenance["source"], &["live-only", "live+als", "offline-file"])
        || live["protocol"].as_str().is_none_or(|s| !PROTOCOL.is_match(s))
        || !one_of(&live["adapter"], &["simulator", "remote-script", "extension", "unavailable", "offline-file"])
        || !one_of(&live["provenance"], &["real-live", "fake-live", "simulator", "unknown"])
    {
        return Err(fail("semantic snapshot Live provenance values are invalid"));
    }
    if live.get("registryHash").is_some_and(|v| !raw_hash(v)) {
        return Err(fail("semantic snapshot registry hash is invalid"));
    }
    if let Some(version) = live.get("version") {
        string_value(version, "Live version", Some(&VERSION))?;
    }
    if provenance["limitations"].as_array().is_none_or(|a| a.is_empty() || a.len() > 16) {
        return Err(fail("semantic snapshot provenance limitations are invalid"));
    }
    for row in array(&provenance["limitations"]) {
        string_value(row, "provenance limitation", None)?;
    }
    if one_of(&provenance["source"], &["live+als", "offline-file"]) != provenance["setFileSha256"].is_string() {
        return Err(fail("semantic snapshot Set-file provenance relationship is invalid"));
    }
    if provenance.get("setFileSha256").is_some_and(|v| !raw_hash(v)) {
        return Err(fail("semantic snapshot Set SHA is invalid"));
    }
    let set = &artifact["set"];
    string_value(&set["name"], "Set name", None)?;
    if !nullable_number(&set["tempo"])
        || !nullable_number(&set["arrangementLength"])
        || !nonnegative_integer(&set["trackCount"])
        || !nonnegative_integer(&set["sceneCount"])
    {
        return Err(fail("semantic snapshot Set summary values are invalid"));
    }
    let safety = &artifact["safety"];
    if safety["readOnly"] != true
        || ["containsMutationAuthority", "containsSessionReferences", "crossRunIdentityClaimed", "mergeProposed"]
            .iter()
            .any(|k| safety[k] != false)
    {
        return Err(fail("semantic snapshot safety contract is invalid"));
    }
    authority_audit(artifact, 0)?;
    let canonical = CanonicalArtifact::new(artifact)?;
    required(&artifact["manifest"], &SECTION_ORDER, "manifest")?;
    let mut ids = HashSet::new();
    let mut counts: HashMap<String, usize> = HashMap::new();
    let records = array(&artifact["records"]);
    static ID: LazyLock<Regex> = LazyLock::new(|| regex(r"^semantic-[a-z-]+-[a-f0-9]{20}-[1-9][0-9]*$"));
    static ALIAS: LazyLock<Regex> = LazyLock::new(|| regex(r"^[a-z-]+-[a-f0-9]{20}$"));
    for record in records {
        let required_keys =
            ["section", "kind", "snapshotId", "order", "contentFingerprint", "semanticFingerprint", "nameFingerprint", "matching", "data"];
        let mut allowed = required_keys.to_vec();
        allowed.push("name");
        only(record, &allowed, "record")?;
        let kind = record["kind"].as_str().unwrap_or("");
        if required_keys.iter().any(|k| record.get(*k).is_none())
            || !one_of(&record["section"], &SECTION_ORDER)
            || record["section"] != section_for_record(kind)
            || !nonnegative_integer(&record["order"])
            || record["order"].as_f64().is_none_or(|n| n > 9_007_199_254_740_991.)
        {
            return Err(fail("semantic snapshot record identity is invalid"));
        }
        if let Some(name) = record.get("name") {
            string_value(name, "record name", None)?;
            if utf16_len(name.as_str().unwrap()) > 512 {
                return Err(fail("semantic snapshot record name exceeds the bound"));
            }
            if policy["profile"] == "strict" && !ALIAS.is_match(name.as_str().unwrap()) {
                return Err(fail("strict semantic snapshot record name is not a typed alias"));
            }
        }
        string_value(&record["snapshotId"], "snapshot-local id", Some(&ID))?;
        if !ids.insert(record["snapshotId"].as_str().unwrap()) {
            return Err(fail("semantic snapshot IDs must be unique"));
        }
        nested(record)?;
        if record["contentFingerprint"]
            != digest_fields(&[("kind", &record["kind"]), ("name", &record["name"]), ("data", &record["data"])])?
            || record["semanticFingerprint"] != digest(&record["matching"])?
            || record["nameFingerprint"] != digest_array(&[&record["kind"], &record["name"]])?
        {
            return Err(fail("semantic snapshot record fingerprint is invalid"));
        }
        let base = format!("semantic-{kind}-{}", &record["semanticFingerprint"].as_str().unwrap()[7..27]);
        let count = counts.entry(base.clone()).or_default();
        *count += 1;
        if record["snapshotId"] != format!("{base}-{count}") {
            return Err(fail("semantic snapshot ID occurrence derivation is invalid"));
        }
    }
    for section in SECTION_ORDER {
        let rows: Vec<_> = records.iter().enumerate().filter(|(_, r)| r["section"] == section).map(|(i, _)| i).collect();
        let manifest = &artifact["manifest"][section];
        required(manifest, &["observed", "included", "omitted", "complete", "digest"], &format!("{section} manifest"))?;
        if !["observed", "included", "omitted"].iter().all(|k| nonnegative_integer(&manifest[k]))
            || !manifest["complete"].is_boolean()
            || !hash(&manifest["digest"])
            || manifest["included"].as_f64() != Some(rows.len() as f64)
            || manifest["digest"] != digest_canonical_array(rows.iter().map(|index| canonical.record(*index)))
            || manifest["observed"].as_f64() != Some(manifest["included"].as_f64().unwrap() + manifest["omitted"].as_f64().unwrap())
            || manifest["complete"].as_bool() != Some(manifest["omitted"].as_f64() == Some(0.))
        {
            return Err(fail(format!("semantic snapshot {section} manifest is invalid")));
        }
    }
    let set_record = records.iter().find(|r| r["kind"] == "set");
    let mut summary = set_record.map(|r| r["data"].clone()).unwrap_or_else(|| json!({}));
    if let Some(row) = set_record {
        if let Some(name) = row.get("name") {
            summary["name"] = name.clone();
        }
    }
    if set_record.is_none()
        || artifact["manifest"]["set"]["included"].as_f64() != Some(1.)
        || canonical_semantic_json(set)? != canonical_semantic_json(&summary)?
    {
        return Err(fail("semantic snapshot Set summary does not match its record"));
    }
    let hash =
        digest_canonical_fields(&["schema", "policy", "set", "manifest", "safety", "records"].map(|key| (key, canonical.field(key))));
    // These fields move one level outward in the identity projection. Their earlier
    // canonical validation is stricter; reusing their bytes cannot bypass a bound.
    if artifact["artifact"]["semanticHash"] != hash {
        return Err(fail("semantic snapshot artifact digest is invalid"));
    }
    let exporter_version = canonical_semantic_json(&artifact["artifact"]["exporterVersion"])?;
    let semantic_hash = canonical_semantic_json(&artifact["artifact"]["semanticHash"])?;
    if artifact["artifact"]["id"]
        != digest_canonical_fields(&[
            ("schema", canonical.field("schema")),
            ("policy", canonical.field("policy")),
            ("provenance", canonical.field("provenance")),
            ("set", canonical.field("set")),
            ("manifest", canonical.field("manifest")),
            ("safety", canonical.field("safety")),
            ("records", canonical.field("records")),
            ("exporterVersion", &exporter_version),
            ("semanticHash", &semantic_hash),
        ])
    {
        return Err(fail("semantic snapshot artifact digest is invalid"));
    }
    Ok(())
}
