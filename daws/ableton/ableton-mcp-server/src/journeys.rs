//! Bounded, read-only journey plans with exact negotiated stage availability.

use crate::live::LiveStatus;
use kumi_common::js::{
    json::{stringify, stringify_pretty},
    string::{trim, utf16_len},
};
use regex::Regex;
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::{collections::HashSet, sync::LazyLock};

pub const JOURNEY_PLAN_VERSION: &str = "ableton-user-journey/v1";
pub const JOURNEY_IDS: &[&str] =
    &["create-beat-or-song", "sequence-advanced-drums", "design-owned-sound", "compare-reference-mix", "diagnose-performance-setup"];
static DATA: LazyLock<Value> =
    LazyLock::new(|| serde_json::from_str(include_str!("journeys-data.json")).expect("embedded journey catalog"));
pub static JOURNEY_CATALOG: LazyLock<Vec<Value>> = LazyLock::new(|| DATA["catalog"].as_array().unwrap().clone());
pub static JOURNEY_PROMPTS: LazyLock<Vec<Value>> = LazyLock::new(|| DATA["prompts"].as_array().unwrap().clone());

#[derive(Debug, thiserror::Error)]
#[error("{0}")]
pub struct JourneyError(pub &'static str);

const VOCABULARY: &[(&str, &[&str])] = &[
    ("rhythm", &["straight", "syncopated", "swung", "swing", "half-time", "double-time", "broken", "steady", "offbeat"]),
    ("density", &["sparse", "minimal", "dense", "busy", "layered"]),
    ("energy", &["calm", "relaxed", "driving", "energetic", "aggressive", "gentle"]),
    ("timbre", &["warm", "bright", "dark", "soft", "gritty", "clean", "rounded", "sharp", "organic", "metallic", "airy"]),
    ("space", &["dry", "intimate", "wide", "narrow", "spacious", "reverberant", "distant", "close"]),
    ("dynamics", &["controlled", "punchy", "dynamic", "compressed", "clear", "balanced", "loud", "quiet"]),
    ("harmony", &["major", "minor", "modal", "dissonant", "consonant", "chromatic"]),
    ("arrangement", &["gradual", "contrasting", "repetitive", "evolving", "short", "long"]),
];
// JavaScript regex whitespace and ASCII word boundaries (these expressions have no /u).
const JS_SPACE: &str = r"[\t\n\x0b\x0c\r \u{00a0}\u{1680}\u{2000}-\u{200a}\u{2028}\u{2029}\u{202f}\u{205f}\u{3000}\u{feff}]";
static EXACT_COPY: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(&format!(r"(?-u:\b)(?:copy|replicate|recreate|duplicate|identical|exact(?:ly)?|signature|sound{JS_SPACE}+like|in{JS_SPACE}+the{JS_SPACE}+style{JS_SPACE}+of)(?-u:\b)")).unwrap()
});
static IDENTITY: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?-u:\b)(?:artist|song|record|track|band|producer|composer|singer)(?-u:\b)").unwrap());
static TITLES: LazyLock<Regex> = LazyLock::new(|| Regex::new(&format!(r"(?-u:\b)[A-Z][a-z]+(?:{JS_SPACE}+[A-Z][a-z]+)+(?-u:\b)")).unwrap());
static SPACE: LazyLock<Regex> = LazyLock::new(|| Regex::new(&format!("{JS_SPACE}+")).unwrap());
static TRAIT_PATTERNS: LazyLock<Vec<(&'static str, &'static str, Regex)>> = LazyLock::new(|| {
    VOCABULARY
        .iter()
        .flat_map(|(dimension, values)| {
            values.iter().map(move |value| {
                (*dimension, *value, Regex::new(&format!("(?:^|[^a-z0-9]){}(?:$|[^a-z0-9])", value.replace('-', "[- ]"))).unwrap())
            })
        })
        .collect()
});

fn normalize_input(input: &Value) -> Result<Value, JourneyError> {
    let journey = input["journey"]
        .as_str()
        .filter(|id| JOURNEY_IDS.contains(id))
        .ok_or(JourneyError("journey must be one of the five supported user journeys"))?;
    let traits = input["traits"]
        .as_str()
        .filter(|s| {
            !trim(s).is_empty()
                && utf16_len(s) <= 1000
                && !s.chars().any(|c| matches!(c, '\u{0}'..='\u{8}' | '\u{b}' | '\u{c}' | '\u{e}'..='\u{1f}'))
        })
        .ok_or(JourneyError("traits must be 1-1000 printable characters"))?;
    let experience = if input["experienceLevel"].is_null() { "beginner" } else { input["experienceLevel"].as_str().unwrap_or("") };
    if !["beginner", "advanced"].contains(&experience) {
        return Err(JourneyError("experienceLevel must be beginner or advanced"));
    }
    let bars = if input["bars"].is_null() { 4.0 } else { input["bars"].as_f64().unwrap_or(f64::NAN) };
    if !bars.is_finite() || bars.fract() != 0.0 || !(1.0..=16.0).contains(&bars) {
        return Err(JourneyError("bars must be an integer from 1 to 16"));
    }
    Ok(json!({"journey":journey,"traits":trim(traits),"experienceLevel":experience,"bars":bars as u64}))
}

fn translate_intent(original: &str) -> Value {
    let lower = original.to_lowercase();
    let mut traits = Vec::new();
    let mut seen = HashSet::new();
    for (dimension, value, pattern) in TRAIT_PATTERNS.iter() {
        let normalized = if *value == "swing" { "swung" } else { value };
        if pattern.is_match(&lower) && seen.insert((*dimension, normalized)) {
            traits.push(json!({"dimension":dimension,"value":normalized}));
        }
    }
    let ascii_lower = original.to_ascii_lowercase();
    let exact = EXACT_COPY.is_match(&ascii_lower);
    let vocabulary_words: HashSet<_> = VOCABULARY.iter().flat_map(|(_, values)| values.iter().flat_map(|v| v.split('-'))).collect();
    let ambiguous_title =
        TITLES.find_iter(original).any(|m| SPACE.split(m.as_str()).any(|word| !vocabulary_words.contains(word.to_lowercase().as_str())));
    let identity = IDENTITY.is_match(&ascii_lower) || exact || ambiguous_title;
    if exact || identity {
        traits.clear();
    }
    let mut excluded = vec![];
    if exact {
        excluded.push("exact replication or signature-copy request");
    }
    if identity {
        excluded.push("artist/song/person identity reference; all coincident trait words excluded");
    }
    json!({"untrustedOriginalRequest":original,"highLevelTraits":traits,"exactCopyIntentDetected":exact,"identityReferenceMayBePresent":identity,"excludedIntent":excluded,"translationPolicy":"identity/copy detection blocks all extraction; otherwise only allowlisted high-level traits influence guidance","clarificationRequired":traits.is_empty()})
}

fn derive_guidance(journey: &str, intent: &Value, bars: usize) -> Value {
    let traits: Vec<_> = intent["highLevelTraits"].as_array().unwrap().iter().filter_map(|v| v["value"].as_str()).collect();
    let has = |value| traits.contains(&value);
    let dense = has("dense") || has("busy") || has("layered");
    let sparse = has("sparse") || has("minimal");
    let syncopated = has("syncopated") || has("broken") || has("offbeat");
    let swung = has("swung");
    let energetic = has("energetic") || has("driving") || has("aggressive");
    let calm = has("calm") || has("relaxed") || has("gentle");
    let tempo = if energetic {
        [120, 138]
    } else if calm {
        [78, 108]
    } else {
        [96, 124]
    };
    let event = |role, start: f64, duration: f64, velocity: [u32; 2], probability: f64, offset: f64| json!({"role":role,"startBeat":start,"durationBeats":duration,"velocityRange":velocity,"probability":probability,"fractionalOffsetBeats":offset});
    let mut per_bar = vec![];
    for start in if syncopated { vec![0.0, 2.0, 2.75] } else { vec![0.0, 2.0] } {
        per_bar.push(event("kick", start, 0.25, if energetic { [96, 116] } else { [82, 106] }, 1.0, 0.0));
    }
    for start in [1.0, 3.0] {
        per_bar.push(event("snare-or-clap", start, 0.25, [88, 112], 1.0, if swung { 0.02 } else { 0.0 }));
    }
    let hat_count = if dense {
        8
    } else if sparse {
        4
    } else {
        6
    };
    for i in 0..hat_count {
        per_bar.push(event(
            "closed-hat",
            i as f64
                * if dense {
                    0.5
                } else if sparse {
                    1.0
                } else {
                    2.0 / 3.0
                },
            0.125,
            [58, 88],
            if i % 4 == 3 { 0.75 } else { 0.95 },
            if swung && i % 2 == 1 { 0.03 } else { 0.0 },
        ));
    }
    let mut events = vec![];
    for bar in 0..bars {
        for source in &per_bar {
            let mut e = source.clone();
            let start = source["startBeat"].as_f64().unwrap() + bar as f64 * 4.0;
            e["startBeat"] = json!((start * 10000.0).round() / 10000.0);
            e["pitch"] = Value::Null;
            events.push(e);
        }
    }
    events.truncate(512);
    match journey {
        "create-beat-or-song" => {
            json!({"kind":"editable-song-draft","derivedFromAllowlistedTraits":traits,"tempoRangeBpm":tempo,"meter":"4/4","bars":bars,"drumRoleEvents":events,"pitchMapping":"unset-until-authoritative-pad-or-instrument-discovery","harmonicDirection":if has("minor") {"minor"} else if has("major") {"major"} else {"ask-for-key-or-keep-pitch-content-unset"},"sections":if bars>=8 {vec![json!({"name":"A","startBar":1,"lengthBars":bars/2}),json!({"name":"B-variation","startBar":bars/2+1,"lengthBars":bars.div_ceil(2)})]} else {vec![json!({"name":"A","startBar":1,"lengthBars":bars})]}})
        }
        "sequence-advanced-drums" => {
            json!({"kind":"editable-drum-role-pattern","derivedFromAllowlistedTraits":traits,"bars":bars,"drumRoleEvents":events,"pitchMapping":"unset-until-authoritative-drum-pad-discovery","expressiveFields":["fractional-start","velocity","probability","velocity-deviation","release-velocity","mute"],"notClaimed":["MPE","groove-extraction","per-note-modulation"]})
        }
        "design-owned-sound" => {
            let query: Vec<_> = traits
                .iter()
                .filter(|v| ["warm", "bright", "dark", "soft", "gritty", "clean", "organic", "metallic", "airy"].contains(v))
                .take(4)
                .collect();
            let mut directions = vec![];
            if has("bright") || has("sharp") {
                directions.push(
                    json!({"semanticControl":"filter-cutoff-or-high-frequency-balance","direction":"increase-within-published-bounds"}),
                );
            }
            if has("warm") || has("dark") || has("soft") {
                directions.push(json!({"semanticControl":"filter-cutoff-or-high-frequency-balance","direction":"decrease-moderately-within-published-bounds"}));
            }
            if has("punchy") {
                directions.push(json!({"semanticControl":"amplitude-envelope-attack","direction":"shorten-within-published-bounds"}));
            }
            json!({"kind":"semantic-sound-design-directions","derivedFromAllowlistedTraits":traits,"browserQueryTerms":query,"topology":if has("wide") || has("spacious") {vec!["instrument-or-source","tone-shaping","bounded-stereo-or-space-stage"]} else {vec!["instrument-or-source","tone-shaping"]},"controlDirections":directions,"exactValues":"unset-until-published-parameter-discovery"})
        }
        "compare-reference-mix" => {
            let mut focus = vec![];
            if has("clear") || has("balanced") {
                focus.push("spectral-balance-and-dynamics");
            }
            if has("punchy") {
                focus.push("transient-and-crest-factor");
            }
            if has("wide") || has("narrow") {
                focus.push("channel-correlation-and-width-proxies");
            }
            json!({"kind":"measurement-focus","derivedFromAllowlistedTraits":traits,"compare":["integrated/momentary/short-term loudness","true peak","LRA/dynamics","spectrum","transients","alignment confidence"],"focus":focus,"causalClaim":false})
        }
        _ => {
            json!({"kind":"performance-risk-checklist","derivedFromAllowlistedTraits":traits,"orderedChecks":["owned playback/recording","arm and monitoring","input/output routes and feedback","mixer clipping/mute/solo","device and automation state","recording destination","realtime TTL/ports/targets","independent stop and residual state"],"latency":"unknown-unless-measured-by-an-external-authoritative-path"})
        }
    }
}

fn strings(value: &Value) -> Vec<&str> {
    value.as_array().into_iter().flatten().filter_map(Value::as_str).collect()
}
fn stage_availability(stage: &Value, status: &LiveStatus) -> Value {
    let capabilities = strings(&stage["capabilities"]);
    let operations = strings(&stage["operations"]);
    let needs_live = !capabilities.is_empty() || !operations.is_empty() || stage.get("provenance").is_some();
    let missing_capabilities: Vec<_> = capabilities.into_iter().filter(|c| !status.capabilities.iter().any(|v| v.as_str() == *c)).collect();
    let missing_operations: Vec<_> = operations.into_iter().filter(|op| !status.has_operation(op)).collect();
    let provenance_available =
        stage.get("provenance").is_none() || stage["provenance"].as_str() == status.provenance.as_ref().map(|p| p.as_str());
    let available = !needs_live
        || (status.connected
            && status.epoch.is_some()
            && missing_capabilities.is_empty()
            && missing_operations.is_empty()
            && provenance_available);
    json!({"available":available,"missingCapabilities":missing_capabilities,"missingOperations":missing_operations,"requiredProvenance":stage["provenance"],"provenanceAvailable":provenance_available})
}

pub fn plan_user_journey(input: &Value, status: &LiveStatus) -> Result<Value, JourneyError> {
    let normalized = normalize_input(input)?;
    let journey = normalized["journey"].as_str().unwrap();
    let bars = normalized["bars"].as_u64().unwrap() as usize;
    let selected = JOURNEY_CATALOG.iter().find(|entry| entry["id"] == journey).unwrap();
    let intent = translate_intent(normalized["traits"].as_str().unwrap());
    let blocked = intent["clarificationRequired"].as_bool().unwrap();
    let stages: Vec<_> = selected["stages"]
        .as_array()
        .unwrap()
        .iter()
        .enumerate()
        .map(|(i, stage)| {
            let availability = stage_availability(stage, status);
            let capability = availability["available"].as_bool().unwrap();
            let mut row =
                json!({"order":i+1,"status":if blocked {"blocked-by-intent"} else if capability {"planned"} else {"unavailable"}});
            row.as_object_mut().unwrap().extend(stage.as_object().unwrap().clone());
            row.as_object_mut().unwrap().extend(availability.as_object().unwrap().clone());
            row["capabilityAvailable"] = json!(capability);
            row["available"] = json!(capability && !blocked);
            row["blockedByIntent"] = json!(blocked);
            row
        })
        .collect();
    let core: Vec<_> = stages.iter().filter(|s| s["requiredForCore"] == true).collect();
    let core_available = core.iter().all(|s| s["capabilityAvailable"] == true);
    let all_available = stages.iter().all(|s| s["capabilityAvailable"] == true);
    let mode = if blocked {
        "intent-clarification-required"
    } else if !core_available {
        "capability-limited"
    } else if all_available {
        "capability-complete"
    } else if journey == "compare-reference-mix" && !status.connected {
        "local-analysis"
    } else {
        "core-capability-complete"
    };
    let collect = |key: &str| {
        let mut seen = HashSet::new();
        core.iter().flat_map(|s| strings(&s[key])).filter(|v| seen.insert(*v)).collect::<Vec<_>>()
    };
    let optional: Vec<_> = stages.iter().filter(|s| s["requiredForCore"] != true && s["capabilityAvailable"] != true).map(|s| json!({"id":s["id"],"missingCapabilities":s["missingCapabilities"],"missingOperations":s["missingOperations"],"fallback":s["unavailableFallback"]})).collect();
    let mut operations: Vec<_> = status.operations.clone().unwrap_or_default();
    operations.sort_by_cached_key(|s| s.encode_utf16().collect::<Vec<_>>());
    let mut capabilities: Vec<_> = status.capabilities.iter().map(|v| v.as_str()).collect();
    capabilities.sort();
    let provenance = status.provenance.as_ref().map_or("unknown", |p| p.as_str());
    let identity = json!({"normalized":normalized,"translated":intent["highLevelTraits"],"connected":status.connected,"adapter":status.adapter,"epoch":status.epoch,"provenance":provenance,"registryHash":status.registry_hash,"operations":operations,"capabilities":capabilities});
    let hash = hex::encode(Sha256::digest(stringify(&identity)));
    let plan_id = format!("journey_{}", &hash[..24]);
    let summary = if blocked {
        "No allowlisted high-level trait could be derived safely. Ask for musical, rhythmic, timbral, spatial, dynamic, harmonic, energy, density, or arrangement traits without relying on identity or exact copying.".into()
    } else if !core_available {
        format!("This Live setup cannot complete the core journey. {}", selected["fallback"].as_str().unwrap())
    } else {
        selected["summary"].as_str().unwrap().to_owned()
    };
    let mut beginner = json!({"summary":summary});
    if blocked {
        beginner["nextAction"] = json!("Request high-level traits; do not forward identity/exact-copy wording into creation.");
    } else if let Some(next) = stages.iter().find(|s| s["status"] == "planned") {
        beginner["nextAction"] = next["announcement"].clone();
    }
    beginner["consequentialActionsRequirePurposeSpecificConfirmation"] = json!(true);
    Ok(json!({
        "version":JOURNEY_PLAN_VERSION,"planId":plan_id,"journey":selected["id"],"title":selected["title"],"intent":intent,"guidance":derive_guidance(journey,&intent,bars),"mode":mode,"executable":core_available && !blocked,"beginner":beginner,
        "advanced":{"adapter":status.adapter,"connected":status.connected,"epoch":status.epoch,"provenance":provenance,"registryHash":status.registry_hash,"requiredCapabilities":collect("capabilities"),"requiredOperations":collect("operations"),"missingCapabilities":collect("missingCapabilities"),"missingOperations":collect("missingOperations"),"unavailableOptionalStages":optional,"exactRefsRequiredBeforeMutation":true,"staleEpochOrRevisionPolicy":"refuse-and-replan"},
        "bounds":{"bars":bars,"maximumBars":16,"maximumNotes":512,"maximumConsequentialAppliesPerStage":16},"stages":stages,
        "progress":{"orderedStatuses":["discovering","planned","awaiting_confirmation","applying","verifying","completed","recovered","uncertain"],"templateStatusOnly":true,"executionStatusSource":"client-or-agent-must-derive-from-actual-purpose-specific-tool-results","announcementsAreText":true,"statusIsNeverColorOnly":true,"terminalResultRequiresResidualState":true,"cancellationRule":"stop-advancing-read-fresh-state-and-use-only-the-stage-recovery-authority"},
        "rights":{"translationPerformed":true,"exactReplicationDelivered":false,"protectedExpressionAccessClaimed":false,"legalClearanceClaimed":false,"userSuppliedReferenceMustBeAuthorizedByUser":true},
        "accessibility":{"semanticTitle":selected["title"],"orderedStageAnnouncements":true,"nonColorStatusLabels":true,"boundedVisualsRequireTextAlternatives":true,"mouseOnlyInstructions":false,"stdioFocusManagement":"not-applicable-no-shipped-interactive-ui","clientAndLiveScreenReaderSupport":"client-and-Live-version-dependent-see-documented-limitations"},
        "fallback":selected["fallback"],"residualStateTemplate":{"status":"not-started","requiredAtTerminal":true,"items":[]}
    }))
}

pub fn journey_resource(status: &LiveStatus) -> Value {
    let journeys: Vec<_> = JOURNEY_CATALOG.iter().map(|j| {
        let plan = plan_user_journey(&json!({"journey":j["id"],"traits":"controlled clear balanced","experienceLevel":"beginner","bars":4}),status).expect("static journey input");
        json!({"id":j["id"],"title":j["title"],"summary":j["summary"],"mode":plan["mode"],"executable":plan["executable"],"missingCapabilities":plan["advanced"]["missingCapabilities"],"missingOperations":plan["advanced"]["missingOperations"],"unavailableOptionalStages":plan["advanced"]["unavailableOptionalStages"],"fallback":j["fallback"]})
    }).collect();
    json!({"version":JOURNEY_PLAN_VERSION,"description":"Five bounded journeys over purpose-specific guarded tools; this read-only resource grants no mutation authority.","journeys":journeys,"rightsPolicy":"Only allowlisted high-level traits influence guidance; names and exact-copy language are excluded. No exact replication, protected-expression access, or legal-clearance claim.","authority":"Read-only catalog. Every mutation still requires its purpose-specific preview, real authority mechanism, idempotency key, verification, and recovery path."})
}

pub fn render_journey_prompt(input: &Value, status: &LiveStatus) -> Result<String, JourneyError> {
    let plan = plan_user_journey(input, status)?;
    Ok([
        format!("# {}",plan["title"].as_str().unwrap()), String::new(), format!("Status: {}.",plan["mode"].as_str().unwrap()),format!("Beginner summary: {}",plan["beginner"]["summary"].as_str().unwrap()),String::new(),
        "Follow only stages whose status is planned. Announce them in order, mark unavailable stages as skipped with their fallback, stop at every listed per-tool authority gate, and never substitute a generic mutation.".into(),
        "Use only intent.highLevelTraits and guidance for creative decisions. Never forward untrustedOriginalRequest names or exact-copy wording into creation. Do not promise exact replication or legal clearance.".into(),
        "The returned stage statuses are a planning template, not execution truth. Derive progress from actual purpose-specific tool results. If any apply is cancelled, times out, loses acknowledgement, or fails verification, report uncertain state, perform fresh readback, and use only that stage's listed recovery authority.".into(),
        "Provide text alternatives for waveform/spectral summaries and never communicate status by color alone. Every terminal report must enumerate residual state.".into(),String::new(),stringify_pretty(&plan,2)
    ].join("\n"))
}
