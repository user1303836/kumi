//! What REAPER's and Renoise's turns tell the model (each DAW's instructions, with its pack of power users' rules, and its
//! tools' descriptions): held to the budget Live's instructions keep, in the DAW's own words, and claiming nothing the
//! DAW lacks.
use kumi_daw_reaper as reaper;
use kumi_daw_renoise as renoise;
use kumi_runtime::integrations::own_bridge::{instructions_of, OwnDaw};
use regex::Regex;

const DAWS: [&OwnDaw; 2] = [&reaper::DAW, &renoise::DAW];

/// Live's words, whole (as the app's own leak tests have them).
fn live_words() -> Regex {
    Regex::new(r"\b(Live|Ableton|Sets?|Session|Arrangement|Main|[Ss]cenes?|[Rr]acks?|[Cc]lip slots?|Control Surfaces?|Remote Scripts?|Max for Live|M4L|Operator)\b").unwrap()
}

/// Everything a DAW's turns tell the model about it.
fn texts(daw: &OwnDaw) -> Vec<(String, String)> {
    let changes = daw.changes;
    let mut texts = vec![
        ("instructions".to_string(), instructions_of(daw)),
        (daw.read_tool.to_string(), daw.read_description.to_string()),
        ("make_changes".to_string(), changes.plan_description.to_string()),
        ("undo_change".to_string(), changes.undo_description.to_string()),
    ];
    texts.extend(changes.kinds.iter().map(|kind| (kind.tool.to_string(), kind.description.to_string())));
    texts.extend(changes.actions.iter().map(|action| (action.tool.to_string(), action.description.to_string())));
    texts
}

#[test]
fn reaper_and_renoise_are_told_of_in_their_own_words() {
    let live = live_words();
    for daw in DAWS {
        for (place, text) in texts(daw) {
            let found: Vec<&str> = live.find_iter(&text).map(|m| m.as_str()).collect();
            assert!(found.is_empty(), "{}'s {place} speaks Live's words: {found:?}", daw.profile.words.app);
        }
    }
}

/// What each DAW doesn't have: its instructions may name one only to say it isn't there.
const LACKS: [(&str, &[&str]); 2] = [
    ("REAPER", &["session view", "clip launcher", "pattern editor", "phrase editor", "drum rack", "simpler"]),
    ("Renoise", &["piano roll", "audio track", "clap", "rewire", "lv2", "clip launcher", "session view"]),
];

#[test]
fn the_instructions_name_what_a_daw_lacks_only_as_lacking() {
    let negation = Regex::new(r"\b(no|never|not|without|lacks|can't|isn't|doesn't|has no)\b").unwrap();
    for daw in DAWS {
        let app = daw.profile.words.app;
        let lacks = LACKS.iter().find(|(name, _)| *name == app).expect("each DAW's list").1;
        let text = instructions_of(daw).to_lowercase();
        for word in lacks {
            for (at, _) in text.match_indices(word) {
                let before: Vec<&str> = text[..at].split_whitespace().rev().take(6).collect();
                assert!(
                    before.iter().any(|w| negation.is_match(w)),
                    "{app}'s instructions name “{word}” as if {app} had it: …{}",
                    &text[at.saturating_sub(80)..(at + word.len() + 20).min(text.len())]
                );
            }
        }
    }
}

/// What the lab found wrong in the research the packs came from: never told to the model.
#[test]
fn what_the_lab_found_wrong_stays_out() {
    let reaper = instructions_of(&reaper::DAW);
    // REAPER 7.82 renders $tracknameornumber as "Master" and the letters "ornumber" (KP3).
    assert!(!reaper.contains("$tracknameornumber"));
    // REAPER does ship instruments (ReaSynth, ReaSynDr, ReaSamplOmatic5000).
    assert!(!reaper.contains("no instruments") && !reaper.contains("no stock instrument"));
    let renoise = instructions_of(&renoise::DAW);
    // Renoise 3.5.4 takes a #Send aimed at its own track without an error (KN7): it isn't refused.
    assert!(!Regex::new(r"(?i)send[^.]*\brefuse").unwrap().is_match(&renoise));
}

#[test]
fn the_packs_carry_what_the_lab_checked() {
    for (daw, words) in [
        (
            &reaper::DAW,
            &["razor area", "$parenttrack", "$timelineorder", "-001", "ReaSynDr", "ACX", "−60 dB RMS", "120 minutes", "dynamic split"][..],
        ),
        (&renoise::DAW, &["0S40", "Autoseek", "8 macros", "0Zxx", "no CLAP", "no ReWire", "Bus Compressor"][..]),
    ] {
        let text = instructions_of(daw);
        for word in words {
            assert!(text.contains(word), "{}'s instructions say {word}", daw.profile.words.app);
        }
    }
    let lua = |daw: &OwnDaw| daw.changes.kinds.iter().find(|kind| kind.tool == "run_lua").unwrap().description;
    assert!(lua(&reaper::DAW).contains("APIExists") && lua(&reaper::DAW).contains("NamedCommandLookup"));
    assert!(lua(&renoise::DAW).contains("postfx_volume") && lua(&renoise::DAW).contains("load_plugin"));
}

/// The packs' rules read as meant (PR #400 review D13–D17).
#[test]
fn the_packs_rules_read_as_meant() {
    let reaper = instructions_of(&reaper::DAW);
    // What the producer marked is the target only when they say this or these (or name nothing), and selected items come
    // before a time selection left over from earlier.
    let rule = &reaper[reaper.find("this or these").expect("the edit target rule is scoped")..];
    let rule = &rule[..rule.find(';').unwrap_or(rule.len())];
    assert!(rule.find("selected items").unwrap() < rule.find("else the time selection").unwrap(), "{rule}");
    // A fade-in on a hit is what Kumi must never add, not something REAPER never does.
    assert!(!reaper.contains("no fade-in on a hit") && reaper.contains("never a fade-in on a drum hit"));
    // No spaces in file names is game audio's (or a spec's) rule, not every render's.
    let names = &reaper[reaper.find("become file names").unwrap()..];
    assert!(names[..names.find('.').unwrap()].contains("for game audio"));
    // REAPER's words: an FX bus fed by sends, never Live's returns.
    assert!(!reaper.contains("returns") && !reaper.to_lowercase().contains("return track"));
    let renoise = instructions_of(&renoise::DAW);
    // A pattern has no LPB of its own (only phrases do).
    assert!(!renoise.contains("patternLines and the song's LPB"));
    // Line numbers stay decimal, as the reads give them; hex is for values.
    assert!(!renoise.contains("LPB and hex") && renoise.contains("line numbers in decimal"));
}

/// The rules every own DAW shares are core's (PR #400 review D24): written once (in `own_rules`), in each DAW's
/// composed instructions exactly once and in the DAW's words, and in neither DAW's own text. A mechanical mistake with one
/// obvious fix is fixed undoably and said so; a choice between the producer's own routes or settings (one route through
/// FX) is named and asked once (D19, D23); nothing is asked only because nothing is selected (D14).
#[test]
fn the_shared_rules_are_written_once_and_composed_into_each_daw() {
    const RULES: [&str; 8] = [
        "factory defaults",
        "a clear mechanical mistake with one obvious fix",
        "how to take it back (Kumi's undo or",
        "one route running through FX",
        "name them exactly and ask one question",
        "Never ask only because nothing is selected",
        "Describe each change in words",
        "Offer alternatives as separate takes",
    ];
    let source = |file: &str| std::fs::read_to_string(format!("{}/../../{file}", env!("CARGO_MANIFEST_DIR"))).unwrap();
    let sources = [
        source("crates/kumi-runtime/src/integrations/own_bridge.rs"),
        source("daws/reaper/kumi-daw-reaper/src/lib.rs"),
        source("daws/renoise/kumi-daw-renoise/src/lib.rs"),
    ];
    for rule in RULES.iter().skip(1) {
        let written: usize = sources.iter().map(|text| text.matches(rule).count()).sum();
        assert_eq!(written, 1, "“{rule}” is written once, in own_rules");
    }
    for daw in [&reaper::DAW, &renoise::DAW] {
        let app = daw.profile.words.app;
        let composed = instructions_of(daw);
        for rule in RULES {
            assert_eq!(composed.matches(rule).count(), 1, "{app}'s instructions hold “{rule}” once");
            assert!(!daw.instructions.contains(rule) && !daw.changes.instructions.contains(rule), "{app}'s own text repeats “{rule}”");
        }
        // In the DAW's words, with its own examples of a mechanical mistake (equal routes among them, D23).
        for words in
            [format!("set {app} up"), format!("{app}'s own"), format!("Never change {app}'s preferences"), daw.mechanical.to_owned()]
        {
            assert!(composed.contains(&words), "{app}: {words}");
        }
        assert!(daw.mechanical.contains("equal routes"), "{app}: equal routes are the mechanical case");
    }
    let lines = renoise::DAW.changes.kinds.iter().find(|kind| kind.tool == "notes_write").unwrap().description;
    assert!(lines.contains("09xx") && lines.contains("0Sxx"), "notes_write says the older spellings are written as 0Sxx");
}
