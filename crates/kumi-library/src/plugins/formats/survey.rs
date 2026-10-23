//! A survey of many decoded files of one format: for each field path, how many files have it, its types, the
//! numbers' range, and its values when they are few and look like an option list. It runs on the presets a
//! machine has, factory ones included, and only its counts and shapes leave that machine: text that reads as a
//! file name, a name or prose is withheld, so no vendor content travels with a survey.

use std::collections::{BTreeMap, BTreeSet};

use serde::Serialize;

use super::tree::{Node, PathRule};

/// Values kept per field before the field counts as having too many to list.
const MAX_VALUES: usize = 32;

#[derive(Debug, Default)]
struct Stats {
    files: usize,
    count: usize,
    kinds: BTreeMap<&'static str, usize>,
    min: Option<f64>,
    max: Option<f64>,
    integral: bool,
    values: BTreeMap<String, usize>,
    too_many: bool,
    withheld: bool,
    tags: BTreeSet<String>,
}

#[derive(Debug)]
pub struct Survey {
    rule: PathRule,
    files: usize,
    failures: Vec<(String, String)>,
    fields: BTreeMap<String, Stats>,
}

/// What a survey found, ready to write out.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct SurveyReport {
    pub files: usize,
    /// Files that didn't decode, with why.
    pub failures: Vec<(String, String)>,
    pub fields: BTreeMap<String, FieldReport>,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct FieldReport {
    /// How many files have it at least once.
    pub files: usize,
    /// How many times it appears in all.
    pub count: usize,
    /// The most common kind ("float", "text", "map" …).
    #[serde(rename = "type")]
    pub kind: String,
    /// Every kind seen, with counts, when there was more than one.
    #[serde(skip_serializing_if = "BTreeMap::is_empty")]
    pub kinds: BTreeMap<String, usize>,
    /// The smallest and largest number seen.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub seen: Option<[f64; 2]>,
    /// Whether every number seen was whole.
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub integral: bool,
    /// Its values and their counts, when there were few and none withheld.
    #[serde(skip_serializing_if = "BTreeMap::is_empty")]
    pub values: BTreeMap<String, usize>,
    /// Whether its values look like a fixed set of options.
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub options: bool,
    /// Whether text values were left out because they read as names, files or prose.
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub withheld: bool,
    /// Which kinds of file it was seen in ("preset", "processor" …).
    #[serde(rename = "in")]
    pub seen_in: Vec<String>,
}

impl Survey {
    pub fn new(rule: PathRule) -> Survey {
        Survey { rule, files: 0, failures: Vec::new(), fields: BTreeMap::new() }
    }

    /// One file's tree, of a kind ("preset", "state" …).
    pub fn add(&mut self, tree: &Node, tag: &str) {
        self.files += 1;
        let fields = &mut self.fields;
        let mut seen_here: BTreeSet<String> = BTreeSet::new();
        tree.walk(self.rule, &mut |path, node| {
            if path.is_empty() {
                return;
            }
            let stats = fields.entry(path.to_string()).or_insert_with(|| Stats { integral: true, ..Stats::default() });
            if seen_here.insert(path.to_string()) {
                stats.files += 1;
                stats.tags.insert(tag.to_string());
            }
            stats.count += 1;
            *stats.kinds.entry(node.kind()).or_default() += 1;
            if let Some(n) = node.as_f64() {
                stats.min = Some(stats.min.map_or(n, |m| m.min(n)));
                stats.max = Some(stats.max.map_or(n, |m| m.max(n)));
                stats.integral &= n.fract() == 0.0;
            }
            let value = match node {
                Node::Bool(b) => Some(b.to_string()),
                Node::Int(_) | Node::Float(_) => node.as_f64().map(|n| n.to_string()),
                Node::Text(text) if withhold(text) => {
                    stats.withheld = true;
                    None
                }
                Node::Text(text) => Some(text.clone()),
                _ => None,
            };
            if let Some(value) = value {
                if !stats.too_many && (stats.values.contains_key(&value) || stats.values.len() < MAX_VALUES) {
                    *stats.values.entry(value).or_default() += 1;
                } else {
                    stats.too_many = true;
                    stats.values.clear();
                }
            }
        });
    }

    pub fn fail(&mut self, file: &str, error: &str) {
        self.failures.push((file.to_string(), error.to_string()));
    }

    pub fn report(&self) -> SurveyReport {
        let fields = self
            .fields
            .iter()
            .map(|(path, stats)| {
                let kind = stats.kinds.iter().max_by_key(|(_, n)| **n).map(|(k, _)| k.to_string()).unwrap_or_default();
                let numeric = matches!(kind.as_str(), "int" | "float");
                let listed = !stats.too_many && !stats.withheld && !stats.values.is_empty();
                let distinct = stats.values.len();
                let options = listed
                    && stats.count >= 2 * distinct
                    && distinct >= 2
                    && match kind.as_str() {
                        "text" => stats.values.keys().all(|v| looks_like_option(v)),
                        "int" | "float" => stats.integral && distinct <= 16,
                        _ => false,
                    };
                let report = FieldReport {
                    files: stats.files,
                    count: stats.count,
                    kinds: if stats.kinds.len() > 1 {
                        stats.kinds.iter().map(|(k, n)| (k.to_string(), *n)).collect()
                    } else {
                        BTreeMap::new()
                    },
                    seen: if numeric { stats.min.zip(stats.max).map(|(a, b)| [a, b]) } else { None },
                    integral: numeric && stats.integral,
                    values: if listed && (options || kind == "bool" || !numeric) { stats.values.clone() } else { BTreeMap::new() },
                    options,
                    withheld: stats.withheld,
                    seen_in: stats.tags.iter().cloned().collect(),
                    kind,
                };
                (path.clone(), report)
            })
            .collect();
        SurveyReport { files: self.files, failures: self.failures.clone(), fields }
    }
}

/// Text a survey doesn't repeat: file names and paths, names and prose (anything with spaces or longer than
/// an identifier).
fn withhold(text: &str) -> bool {
    !looks_like_option(text)
}

/// An identifier-like word: what an option list's values look like ("kBendNeg", "Wave Source" doesn't).
fn looks_like_option(text: &str) -> bool {
    !text.is_empty() && text.len() <= 40 && text.bytes().all(|b| b.is_ascii_alphanumeric() || matches!(b, b'_' | b'-' | b'+'))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn counts_fields_across_files_and_spots_options() {
        let mut survey = Survey::new(PathRule::NumberedSections);
        for (i, warp) in ["kBendNeg", "kSync", "kBendNeg", "kSync"].iter().enumerate() {
            let tree = Node::from(&json!({
                "Osc0": {"warp": warp, "pos": i as f64 * 10.5, "voices": i % 2},
                "Osc1": {"warp": "kSync"},
                "presetName": format!("Patch {i}"),
                "path": "Tables/Basic.wav",
            }));
            survey.add(&tree, if i == 0 { "state" } else { "preset" });
        }
        survey.fail("broken.SerumPreset", "ends early");
        let report = survey.report();
        assert_eq!(report.files, 4);
        assert_eq!(report.failures.len(), 1);
        let warp = &report.fields["Osc{n}/warp"];
        assert_eq!((warp.files, warp.count, warp.kind.as_str(), warp.options), (4, 8, "text", true));
        assert_eq!(warp.values["kSync"], 6);
        assert_eq!(warp.seen_in, ["preset", "state"]);
        let pos = &report.fields["Osc{n}/pos"];
        assert_eq!((pos.seen, pos.integral, pos.options), (Some([0.0, 31.5]), false, false));
        let voices = &report.fields["Osc{n}/voices"];
        assert!(voices.integral && voices.options);
        assert!(report.fields["presetName"].withheld && report.fields["presetName"].values.is_empty());
        assert!(report.fields["path"].withheld);
    }

    #[test]
    fn stops_listing_past_the_cap() {
        let mut survey = Survey::new(PathRule::Exact);
        for i in 0..(MAX_VALUES + 5) {
            survey.add(&Node::from(&json!({"id": format!("v{i}")})), "preset");
        }
        let id = &survey.report().fields["id"];
        assert!(id.values.is_empty() && !id.options);
    }
}
