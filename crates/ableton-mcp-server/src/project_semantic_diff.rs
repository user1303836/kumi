//! Observational semantic Set comparison with explicit ambiguity and bounded paging.
use crate::{
    project::ProjectError,
    project_semantic::{
        self as semantic, array, canonical_semantic_json as canonical, compare_semantic_strings as cmp, digest, fail, js_equal,
        validate_semantic_project_artifact, SemanticPageOptions, SEMANTIC_PROJECT_MAX_PAGE_BYTES, SEMANTIC_PROJECT_MAX_PAGE_RECORDS,
        SEMANTIC_PROJECT_SNAPSHOT_SCHEMA,
    },
};
use regex::Regex;
use serde_json::{json, Value};
use std::{
    collections::{HashMap, HashSet},
    sync::LazyLock,
};
pub const SEMANTIC_PROJECT_DIFF_SCHEMA: &str = "ableton-mcp-semantic-set-diff/v1";
pub type SemanticProjectDiff = Value;
pub type SemanticProjectDiffPage = Value;
pub type SemanticProjectChange = Value;
pub type SemanticProjectAmbiguity = Value;
fn name_compatible(a: &Value, b: &Value) -> bool {
    if a["kind"] != b["kind"] {
        return false;
    }
    match a["kind"].as_str() {
        Some("track") => a["data"]["kind"] == b["data"]["kind"],
        Some("clip") => a["data"]["clipKind"] == b["data"]["clipKind"],
        Some("device") => a["data"]["deviceKind"] == b["data"]["deviceKind"] && a["data"]["className"] == b["data"]["className"],
        Some("dependency") => {
            a["data"]["category"] == b["data"]["category"]
                && a["data"]["origin"] == b["data"]["origin"]
                && a["matching"]["className"] == b["matching"]["className"]
        }
        _ => true,
    }
}
fn semantic_compatible(a: &Value, b: &Value) -> bool {
    name_compatible(a, b)
        && !((a["kind"] == "device" || a["kind"] == "dependency")
            && a["matching"]["opaqueState"] == true
            && b["matching"]["opaqueState"] == true
            && a.get("name") != b.get("name"))
}
fn compatibility_key(record: &Value) -> String {
    let kind = record["kind"].as_str().unwrap();
    let data = &record["data"];
    match kind {
        "track" => format!("{kind}:{}", data["kind"].as_str().unwrap()),
        "clip" => format!("{kind}:{}", data["clipKind"].as_str().unwrap()),
        "device" => format!("{kind}:{}:{}", data["deviceKind"].as_str().unwrap(), data["className"].as_str().unwrap()),
        "dependency" => format!(
            "{kind}:{}:{}:{}",
            data["category"].as_str().unwrap(),
            data["origin"].as_str().unwrap(),
            record["matching"]["className"].as_str().unwrap_or("")
        ),
        _ => kind.into(),
    }
}
fn changed_details(before: &Value, after: &Value) -> Result<Vec<Value>, ProjectError> {
    fn visit(a: &Value, b: &Value, path: &str, depth: usize, details: &mut Vec<Value>) -> Result<(), ProjectError> {
        if details.len() >= 64 || canonical(a)? == canonical(b)? {
            return Ok(());
        }
        if depth < 4 {
            if let (Some(a), Some(b)) = (a.as_object(), b.as_object()) {
                let mut keys: Vec<_> = a.keys().chain(b.keys()).map(String::as_str).collect();
                keys.sort_by(|a, b| cmp(a, b));
                keys.dedup();
                for key in keys {
                    let path = format!("{path}/{key}");
                    match (a.get(key), b.get(key)) {
                        (None, Some(b)) => details.push(json!({"path":path,"before":"absent","after":b})),
                        (Some(a), None) => details.push(json!({"path":path,"before":a,"after":"absent"})),
                        (Some(a), Some(b)) => visit(a, b, &path, depth + 1, details)?,
                        _ => {}
                    }
                    if details.len() >= 64 {
                        break;
                    }
                }
                return Ok(());
            }
        }
        details.push(json!({"path":path,"before":a,"after":b}));
        Ok(())
    }
    let mut details = vec![];
    visit(
        &json!({"name":before["name"],"order":before["order"],"data":before["data"]}),
        &json!({"name":after["name"],"order":after["order"],"data":after["data"]}),
        "",
        0,
        &mut details,
    )?;
    Ok(details)
}
fn change_for(before: &Value, after: &Value, confidence: &str) -> Result<Option<Value>, ProjectError> {
    let mut facets = vec![];
    if before.get("name") != after.get("name") {
        facets.push("renamed");
    }
    if !js_equal(&before["order"], &after["order"]) {
        facets.push("reordered");
    }
    if canonical(&before["data"])? != canonical(&after["data"])? {
        if before["kind"] == "device" {
            facets.push("state-changed");
        } else if before["kind"] == "dependency" {
            facets.push("dependency-changed");
            if before["data"]["availability"] != after["data"]["availability"] {
                facets.push("availability-changed");
            }
        } else {
            facets.push("content-changed");
        }
    }
    if facets.is_empty() {
        return Ok(None);
    }
    let details = changed_details(before, after)?;
    Ok(Some(
        json!({"type":"change","section":before["section"],"kind":before["kind"],"beforeSnapshotId":before["snapshotId"],"afterSnapshotId":after["snapshotId"],"facets":facets,"confidence":confidence,"evidence":["snapshot-local IDs are coordinates only",match confidence{"exact-content"=>"unique equal content fingerprint","unique-semantic"=>"unique equal semantic structure fingerprint",_=>"unique compatible name fingerprint"}],"detailsTruncated":details.len()>=64,"details":details}),
    ))
}
fn absence(record: &Value, facet: &str) -> Value {
    let mut row = json!({"type":"change","section":record["section"],"kind":record["kind"],"facets":[facet],"confidence":"absence","evidence":["no compatible remaining candidate in a complete section","snapshot-local IDs were not used for matching"],"details":[],"detailsTruncated":false});
    row[if facet == "removed" { "beforeSnapshotId" } else { "afterSnapshotId" }] = record["snapshotId"].clone();
    row
}
fn ambiguity(
    section: &str,
    kind: &str,
    reason: &str,
    before: &[&Value],
    after: &[&Value],
    evidence: &[&str],
) -> Result<Value, ProjectError> {
    let ids = |rows: &[&Value]| {
        let mut ids: Vec<_> = rows.iter().map(|r| r["snapshotId"].as_str().unwrap().to_owned()).collect();
        ids.sort_by(|a, b| cmp(a, b));
        ids
    };
    let before = ids(before);
    let after = ids(after);
    Ok(
        json!({"type":"ambiguity","section":section,"kind":kind,"reason":reason,"beforeCandidateCount":before.len(),"afterCandidateCount":after.len(),"candidateSetDigest":digest(&json!({"before":before,"after":after}))?,"beforeSnapshotIds":before.iter().take(128).collect::<Vec<_>>(),"afterSnapshotIds":after.iter().take(128).collect::<Vec<_>>(),"candidatesTruncated":before.len()>128||after.len()>128,"evidence":evidence}),
    )
}
fn components(before: &[&Value], after: &[&Value]) -> Vec<(Vec<usize>, Vec<usize>)> {
    let total = before.len() + after.len();
    let mut parents: Vec<_> = (0..total).collect();
    fn find(parents: &mut [usize], mut n: usize) -> usize {
        while parents[n] != n {
            parents[n] = parents[parents[n]];
            n = parents[n];
        }
        n
    }
    fn union(parents: &mut [usize], a: usize, b: usize) {
        let a = find(parents, a);
        let b = find(parents, b);
        if a != b {
            parents[b] = a;
        }
    }
    let mut buckets: HashMap<String, (Vec<usize>, Vec<usize>)> = HashMap::new();
    for (index, record) in before.iter().chain(after).enumerate() {
        let key = compatibility_key(record);
        for (kind, field) in [("content", "contentFingerprint"), ("semantic", "semanticFingerprint"), ("name", "nameFingerprint")] {
            let bucket = buckets.entry(format!("{kind}:{key}:{}", record[field].as_str().unwrap())).or_default();
            if index < before.len() {
                bucket.0.push(index);
            } else {
                bucket.1.push(index);
            }
        }
    }
    let mut participating = HashSet::new();
    for (a, b) in buckets.values() {
        if a.is_empty() || b.is_empty() {
            continue;
        }
        for index in a.iter().chain(b) {
            participating.insert(*index);
            union(&mut parents, a[0], *index);
        }
    }
    let mut grouped: HashMap<usize, (Vec<usize>, Vec<usize>)> = HashMap::new();
    for index in participating {
        let root = find(&mut parents, index);
        let group = grouped.entry(root).or_default();
        if index < before.len() {
            group.0.push(index);
        } else {
            group.1.push(index - before.len());
        }
    }
    grouped
        .into_values()
        .filter(|(a, b)| !a.is_empty() && !b.is_empty())
        .map(|(mut a, mut b)| {
            a.sort_unstable();
            b.sort_unstable();
            (a, b)
        })
        .collect()
}
pub fn diff_semantic_project_snapshots(before: &Value, after: &Value) -> Result<Value, ProjectError> {
    validate_semantic_project_artifact(before)?;
    validate_semantic_project_artifact(after)?;
    if before["schema"] != after["schema"] || before["schema"] != SEMANTIC_PROJECT_SNAPSHOT_SCHEMA {
        return Err(fail("semantic snapshots use incompatible schemas"));
    }
    if before["policy"]["profile"] != after["policy"]["profile"] {
        return Err(fail("semantic snapshots use different privacy profiles"));
    }
    let mut items = vec![];
    let mut sections: Vec<_> =
        before["manifest"].as_object().unwrap().keys().chain(after["manifest"].as_object().unwrap().keys()).map(String::as_str).collect();
    sections.sort_by(|a, b| cmp(a, b));
    sections.dedup();
    let incomplete: Vec<_> = sections
        .iter()
        .filter(|s| before["manifest"][**s]["complete"] != true || after["manifest"][**s]["complete"] != true)
        .copied()
        .collect();
    for section in sections {
        let mut a: Vec<_> = array(&before["records"]).iter().filter(|r| r["section"] == section).collect();
        let mut b: Vec<_> = array(&after["records"]).iter().filter(|r| r["section"] == section).collect();
        let mut matches = vec![];
        for (field, confidence, compatible) in [
            ("contentFingerprint", "exact-content", (0u8)),
            ("semanticFingerprint", "unique-semantic", 1),
            ("nameFingerprint", "unique-name", 2),
        ] {
            let group = |rows: &[&Value]| {
                let mut groups: HashMap<String, Vec<usize>> = HashMap::new();
                for (i, r) in rows.iter().enumerate() {
                    groups.entry(r[field].as_str().unwrap().to_owned()).or_default().push(i);
                }
                groups
            };
            let ag = group(&a);
            let bg = group(&b);
            let mut am = HashSet::new();
            let mut bm = HashSet::new();
            for (i, r) in a.iter().enumerate() {
                let key = r[field].as_str().unwrap();
                let Some(candidates) = bg.get(key).filter(|rows| rows.len() == 1) else {
                    continue;
                };
                if ag[key].len() != 1 {
                    continue;
                }
                let j = candidates[0];
                let valid = match compatible {
                    0 => r["kind"] == b[j]["kind"],
                    1 => semantic_compatible(r, b[j]),
                    _ => name_compatible(r, b[j]),
                };
                if valid {
                    matches.push((*r, b[j], confidence));
                    am.insert(i);
                    bm.insert(j);
                }
            }
            a = a.into_iter().enumerate().filter(|(i, _)| !am.contains(i)).map(|(_, r)| r).collect();
            b = b.into_iter().enumerate().filter(|(i, _)| !bm.contains(i)).map(|(_, r)| r).collect();
        }
        for (a, b, confidence) in matches {
            if let Some(change) = change_for(a, b, confidence)? {
                items.push(change);
            }
        }
        let mut ambiguous_a = HashSet::new();
        let mut ambiguous_b = HashSet::new();
        for (ai, bi) in components(&a, &b) {
            let ar: Vec<_> = ai.iter().map(|i| a[*i]).collect();
            let br: Vec<_> = bi.iter().map(|i| b[*i]).collect();
            let kind = ar.first().or(br.first()).and_then(|r| r["kind"].as_str()).unwrap_or("record");
            items.push(ambiguity(
                section,
                kind,
                "indistinguishable-candidates",
                &ar,
                &br,
                &[
                    "multiple candidates share content, structure, or name evidence",
                    "candidate counts and full-set digest cover any IDs omitted from the bounded sample",
                    "incidental order and snapshot-local IDs were not used to choose a match",
                ],
            )?);
            ambiguous_a.extend(ai);
            ambiguous_b.extend(bi);
        }
        a = a.into_iter().enumerate().filter(|(i, _)| !ambiguous_a.contains(i)).map(|(_, r)| r).collect();
        b = b.into_iter().enumerate().filter(|(i, _)| !ambiguous_b.contains(i)).map(|(_, r)| r).collect();
        let before_complete = before["manifest"][section]["complete"] == true;
        let after_complete = after["manifest"][section]["complete"] == true;
        if !before_complete || !after_complete {
            items.push(ambiguity(
                section,
                "section-scope",
                "incomplete-section",
                if after_complete { &[] } else { &a },
                if before_complete { &[] } else { &b },
                &[
                    "at least one artifact truncated or omitted this section",
                    "candidate counts and full-set digest cover any IDs omitted from the bounded sample",
                    "definitive absence claims are suppressed",
                ],
            )?);
        }
        if after_complete {
            items.extend(a.into_iter().map(|r| absence(r, "removed")));
        }
        if before_complete {
            items.extend(b.into_iter().map(|r| absence(r, "added")));
        }
    }
    let mut keyed = items.into_iter().map(|v| Ok((canonical(&v)?, v))).collect::<Result<Vec<_>, ProjectError>>()?;
    keyed.sort_by(|a, b| {
        cmp(a.1["section"].as_str().unwrap(), b.1["section"].as_str().unwrap())
            .then_with(|| cmp(a.1["kind"].as_str().unwrap(), b.1["kind"].as_str().unwrap()))
            .then_with(|| cmp(&a.0, &b.0))
    });
    let items: Vec<_> = keyed.into_iter().map(|(_, v)| v).collect();
    let changes = items.iter().filter(|i| i["type"] == "change").count();
    let summary = json!({"changed":!items.is_empty(),"changes":changes,"ambiguities":items.len()-changes,"incompleteSections":incomplete});
    let safety = json!({"comparisonOnly":true,"mergeProposed":false,"crossRunSessionIdentityUsed":false,"mutationAuthorityGranted":false});
    let limitations = json!([
        "observational comparison only",
        "no .als edit or merge is proposed",
        "opaque plug-in and Max state is not decoded",
        "ambiguous candidates remain unresolved"
    ]);
    let identity = json!({"beforeArtifactId":before["artifact"]["id"],"afterArtifactId":after["artifact"]["id"]});
    let id = digest(
        &json!({"schema":SEMANTIC_PROJECT_DIFF_SCHEMA,"identity":identity,"policy":before["policy"]["profile"],"summary":summary,"safety":safety,"limitations":limitations,"items":items}),
    )?;
    let mut identity = identity;
    identity["id"] = json!(id);
    let result = json!({"schema":SEMANTIC_PROJECT_DIFF_SCHEMA,"diff":identity,"policy":{"profile":before["policy"]["profile"]},"summary":summary,"safety":safety,"limitations":limitations,"items":items});
    audit_diff_output(&result)?;
    Ok(result)
}
fn exact(v: &Value, required: &[&str], optional: &[&str]) -> bool {
    v.as_object().is_some_and(|o| {
        required.iter().all(|k| o.contains_key(*k)) && o.keys().all(|k| required.contains(&k.as_str()) || optional.contains(&k.as_str()))
    })
}
fn one_of(v: &Value, values: &[&str]) -> bool {
    v.as_str().is_some_and(|s| values.contains(&s))
}
fn integer(v: &Value) -> bool {
    v.as_f64().is_some_and(|n| n.fract() == 0.)
}
fn nonnegative(v: &Value) -> bool {
    integer(v) && v.as_f64().is_some_and(|n| n >= 0.)
}
fn matches(v: &Value, r: &Regex) -> bool {
    v.as_str().is_some_and(|s| r.is_match(s))
}
fn strings(v: &Value, min: usize, max: usize) -> bool {
    v.as_array().is_some_and(|a| {
        (min..=max).contains(&a.len())
            && a.iter().all(|s| s.as_str().is_some_and(|s| !s.is_empty() && kumi_common::js::string::utf16_len(s) <= 4096))
    })
}
fn regex(pattern: &str) -> Regex {
    Regex::new(&pattern.replace(r"\s", semantic::JS_SPACE)).unwrap()
}
fn audit_diff_output(diff: &Value) -> Result<(), ProjectError> {
    static HASH: LazyLock<Regex> = LazyLock::new(|| regex(r"^sha256:[a-f0-9]{64}$"));
    static ID: LazyLock<Regex> = LazyLock::new(|| regex(r"^semantic-[a-z-]+-[a-f0-9]{20}-[1-9][0-9]*$"));
    if !exact(diff, &["schema", "diff", "policy", "summary", "safety", "limitations", "items"], &[])
        || diff["schema"] != SEMANTIC_PROJECT_DIFF_SCHEMA
        || diff["items"].as_array().is_none_or(|a| a.len() > 24000)
    {
        return Err(fail("semantic diff schema or bounds are invalid"));
    }
    let identity = &diff["diff"];
    if !exact(identity, &["id", "beforeArtifactId", "afterArtifactId"], &[])
        || ["id", "beforeArtifactId", "afterArtifactId"].iter().any(|k| !matches(&identity[k], &HASH))
    {
        return Err(fail("semantic diff identity is invalid"));
    }
    if !exact(&diff["policy"], &["profile"], &[]) || !one_of(&diff["policy"]["profile"], &["strict", "collaboration", "local"]) {
        return Err(fail("semantic diff policy is invalid"));
    }
    let summary = &diff["summary"];
    if !exact(summary, &["changed", "changes", "ambiguities", "incompleteSections"], &[])
        || !summary["changed"].is_boolean()
        || !nonnegative(&summary["changes"])
        || !nonnegative(&summary["ambiguities"])
        || summary["incompleteSections"].as_array().is_none_or(|a| a.iter().any(|s| !one_of(s, &semantic::SECTION_ORDER)))
    {
        return Err(fail("semantic diff summary is invalid"));
    }
    let safety = &diff["safety"];
    if !exact(safety, &["comparisonOnly", "mergeProposed", "crossRunSessionIdentityUsed", "mutationAuthorityGranted"], &[])
        || safety["comparisonOnly"] != true
        || ["mergeProposed", "crossRunSessionIdentityUsed", "mutationAuthorityGranted"].iter().any(|k| safety[k] != false)
    {
        return Err(fail("semantic diff safety is invalid"));
    }
    if !strings(&diff["limitations"], 1, 16) {
        return Err(fail("semantic diff limitations are invalid"));
    }
    let items = array(&diff["items"]);
    for item in items {
        if item["type"] == "change" {
            if !exact(
                item,
                &["type", "section", "kind", "facets", "confidence", "evidence", "details", "detailsTruncated"],
                &["beforeSnapshotId", "afterSnapshotId"],
            ) || !one_of(&item["confidence"], &["exact-content", "unique-semantic", "unique-name", "absence"])
                || item["facets"].as_array().is_none_or(|a| {
                    a.is_empty()
                        || a.iter().any(|v| {
                            !one_of(
                                v,
                                &[
                                    "added",
                                    "removed",
                                    "renamed",
                                    "reordered",
                                    "content-changed",
                                    "state-changed",
                                    "dependency-changed",
                                    "availability-changed",
                                ],
                            )
                        })
                })
                || !item["detailsTruncated"].is_boolean()
            {
                return Err(fail("semantic diff change item is invalid"));
            }
            if ["beforeSnapshotId", "afterSnapshotId"].iter().any(|k| item.get(*k).is_some_and(|v| !matches(v, &ID))) {
                return Err(fail("semantic diff change coordinate is invalid"));
            }
            if item["details"].as_array().is_none_or(|a| {
                a.len() > 64
                    || a.iter().any(|d| {
                        !exact(d, &["path", "before", "after"], &[])
                            || d["path"].as_str().is_none_or(|s| !s.is_empty() && !s.starts_with('/'))
                    })
            }) {
                return Err(fail("semantic diff change details are invalid"));
            }
        } else if item["type"] == "ambiguity" {
            if !exact(
                item,
                &[
                    "type",
                    "section",
                    "kind",
                    "reason",
                    "beforeCandidateCount",
                    "afterCandidateCount",
                    "candidateSetDigest",
                    "beforeSnapshotIds",
                    "afterSnapshotIds",
                    "candidatesTruncated",
                    "evidence",
                ],
                &[],
            ) || !one_of(&item["reason"], &["indistinguishable-candidates", "incomplete-section"])
                || !nonnegative(&item["beforeCandidateCount"])
                || !nonnegative(&item["afterCandidateCount"])
                || !matches(&item["candidateSetDigest"], &HASH)
                || ["beforeSnapshotIds", "afterSnapshotIds"]
                    .iter()
                    .any(|k| item[k].as_array().is_none_or(|a| a.len() > 128 || a.iter().any(|v| !matches(v, &ID))))
                || item["beforeCandidateCount"].as_f64().unwrap() < array(&item["beforeSnapshotIds"]).len() as f64
                || item["afterCandidateCount"].as_f64().unwrap() < array(&item["afterSnapshotIds"]).len() as f64
                || item["candidatesTruncated"].as_bool()
                    != Some(
                        item["beforeCandidateCount"].as_f64().unwrap() > array(&item["beforeSnapshotIds"]).len() as f64
                            || item["afterCandidateCount"].as_f64().unwrap() > array(&item["afterSnapshotIds"]).len() as f64,
                    )
            {
                return Err(fail("semantic diff ambiguity item is invalid"));
            }
            for key in ["beforeSnapshotIds", "afterSnapshotIds"] {
                let rows = array(&item[key]);
                if rows.windows(2).any(|p| cmp(p[0].as_str().unwrap(), p[1].as_str().unwrap()).is_gt()) {
                    return Err(fail("semantic diff ambiguity samples are not deterministic"));
                }
            }
        } else {
            return Err(fail("semantic diff item kind is invalid"));
        }
        if !strings(&item["evidence"], 1, 16) {
            return Err(fail("semantic diff evidence is invalid"));
        }
    }
    let changes = items.iter().filter(|i| i["type"] == "change").count();
    let sections = array(&summary["incompleteSections"]);
    let unique: HashSet<_> = sections.iter().map(|s| s.as_str().unwrap()).collect();
    if summary["changed"].as_bool() != Some(!items.is_empty())
        || summary["changes"].as_f64() != Some(changes as f64)
        || summary["ambiguities"].as_f64() != Some((items.len() - changes) as f64)
        || unique.len() != sections.len()
    {
        return Err(fail("semantic diff summary does not match its items"));
    }
    fn visit(value: &Value, key: &str, depth: usize) -> Result<(), ProjectError> {
        if depth > 24 {
            return Err(fail("semantic diff exceeds audit depth"));
        }
        if let Some(s) = value.as_str() {
            static AUTH: LazyLock<Regex> =
                LazyLock::new(|| regex(r"(?i)(?:reusable[-_ ]?(?:token|secret|confirmation)|bearer\s+[A-Za-z0-9._-]{8,})"));
            // The exporter's own test: a name such as "Kick / Snare" is kept there, so it isn't a path here.
            if key != "path" && semantic::absolute_path(s) {
                return Err(fail("semantic diff contains an absolute or network path"));
            }
            if AUTH.is_match(s) {
                return Err(fail("semantic diff contains authority-like content"));
            }
            return Ok(());
        }
        if let Some(a) = value.as_array() {
            if a.len() > 24000 {
                return Err(fail("semantic diff array exceeds bound"));
            }
            for child in a {
                visit(child, key, depth + 1)?;
            }
            return Ok(());
        }
        if let Some(o) = value.as_object() {
            if o.len() > 64 || o.keys().any(|s| kumi_common::js::string::utf16_len(s) > 128) {
                return Err(fail("semantic diff object exceeds bound"));
            }
            static CAMEL: LazyLock<Regex> = LazyLock::new(|| regex("([a-z])([A-Z])"));
            static FORBIDDEN: LazyLock<Regex> =
                LazyLock::new(|| regex("(?i)(?:^|_)(?:session_ref|access_token|confirmation|secret|idempotency_key|mutation_authority)$"));
            for (name, child) in o {
                if FORBIDDEN.is_match(&CAMEL.replace_all(name, "${1}_${2}")) {
                    return Err(fail("semantic diff contains forbidden authority fields"));
                }
                visit(child, name, depth + 1)?;
            }
        }
        Ok(())
    }
    visit(diff, "", 0)?;
    canonical(diff)?;
    let identity = json!({"beforeArtifactId":identity["beforeArtifactId"],"afterArtifactId":identity["afterArtifactId"]});
    if diff["diff"]["id"]
        != digest(
            &json!({"schema":SEMANTIC_PROJECT_DIFF_SCHEMA,"identity":identity,"policy":diff["policy"]["profile"],"summary":summary,"safety":safety,"limitations":diff["limitations"],"items":items}),
        )?
    {
        return Err(fail("semantic diff digest is invalid"));
    }
    Ok(())
}
fn encode_cursor(diff: &Value, offset: usize) -> Result<String, ProjectError> {
    use base64::Engine;
    let mut payload = json!({"diffId":diff["diff"]["id"],"offset":offset,"schema":SEMANTIC_PROJECT_DIFF_SCHEMA});
    payload["checksum"] = json!(&digest(&payload)?[7..27]);
    Ok(base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(canonical(&payload)?))
}
fn decode_cursor(diff: &Value, cursor: &str) -> Result<f64, ProjectError> {
    let row = semantic::cursor_json(cursor).filter(Value::is_object).ok_or_else(|| fail("semantic diff cursor is malformed"))?;
    if ["diffId", "offset", "schema"].iter().any(|k| row.get(*k).is_none()) {
        return Err(fail("semantic artifact contains an unsupported value"));
    }
    let payload = json!({"diffId":row["diffId"],"offset":row["offset"],"schema":row["schema"]});
    if row["checksum"] != digest(&payload)?[7..27]
        || row["diffId"] != diff["diff"]["id"]
        || row["schema"] != SEMANTIC_PROJECT_DIFF_SCHEMA
        || !nonnegative(&row["offset"])
    {
        return Err(fail("semantic diff cursor does not match the diff"));
    }
    Ok(row["offset"].as_f64().unwrap())
}
fn make_page(diff: &Value, offset: usize, count: usize) -> Result<Value, ProjectError> {
    let mut header = diff.clone();
    header.as_object_mut().unwrap().remove("items");
    let items = array(&diff["items"]);
    let complete = offset + count == items.len();
    header["page"] = json!({"offset":offset,"returned":count,"total":items.len(),"complete":complete});
    if !complete {
        header["page"]["nextCursor"] = json!(encode_cursor(diff, offset + count)?);
    }
    header["items"] = json!(&items[offset..offset + count]);
    Ok(header)
}
pub fn page_semantic_project_diff(diff: &Value, options: &SemanticPageOptions) -> Result<Value, ProjectError> {
    audit_diff_output(diff)?;
    let limit = options.limit.unwrap_or(100.);
    if limit.fract() != 0. || !(1. ..=SEMANTIC_PROJECT_MAX_PAGE_RECORDS as f64).contains(&limit) {
        return Err(fail("semantic diff or page limit is invalid"));
    }
    let offset = options.cursor.as_deref().filter(|s| !s.is_empty()).map(|s| decode_cursor(diff, s)).transpose()?.unwrap_or(0.);
    let total = array(&diff["items"]).len();
    if offset > total as f64 {
        return Err(fail("semantic diff cursor offset is outside the diff"));
    }
    let offset = offset as usize;
    let mut count = (limit as usize).min(total - offset);
    while count > 0 {
        if canonical(&make_page(diff, offset, count)?)?.len() <= SEMANTIC_PROJECT_MAX_PAGE_BYTES {
            break;
        }
        count -= 1;
    }
    if total > offset && count == 0 {
        return Err(fail("one semantic diff item exceeds the page byte bound"));
    }
    make_page(diff, offset, count)
}
