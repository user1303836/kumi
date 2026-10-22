//! `docs/<lang>/DAWS.md`, the producer's page of what Kumi does in each DAW, keeps up with the code: each capability
//! flag has a row, ticked exactly for the DAWs whose profiles have it; each FOCUS view REAPER and Renoise draw has a row
//! ticked for them; every kind category of REAPER and Renoise (a change tool's first word: `track_`, `fx_`, `slot_`) is
//! named in that DAW's section, and every kind their own undo skips in its limits; each cell speaks the legend's forms.
//! A new flag, FOCUS view, kind category or undo-skipped kind can't ship unlisted (the bridges' other tools aren't
//! checked here).
use super::{reaper, reaper_changes, renoise, renoise_changes};
use crate::core::daw::{Capabilities, DawProfile};
use crate::core::view::BodyKind;
use std::collections::BTreeSet;

const LANGUAGES: [&str; 3] = ["en", "ja", "zh-CN"];

/// Each capability flag, and the words that start its row in the English table (flags can share a row).
const ROWS: &[(&str, &str)] = &[
    ("undo", "Changes in HISTORY with Kumi's own undo"),
    ("instant", "Instant phrases"),
    ("scripting", "Code run inside the DAW"),
    ("stop_transport", "Stop everything at once"),
    ("variations", "Variations as numbered takes"),
    ("sample_mapping", "Sample maps"),
    ("chop", "Chops a recording at its real hits"),
    ("delivery", "Delivers files: a region per asset"),
    ("routing", "Finds why a track plays twice or nowhere, and builds sidechains, parallel buses and stems"),
    ("scripts", "Keeps a repeated chore as a script"),
    ("delivery_spec", "Meets a loudness spec"),
    ("jam", "Cleans up a jam"),
    ("recording_state", "Knows whether the DAW is recording"),
    ("status_line", "A line in the DAW's status bar"),
    ("open_project", "Opens a saved project"),
    ("catch_up", "Catches up on what changed"),
    ("durable_project_id", "Conversations follow the saved project"),
    ("hear", "Hears tracks or the mix"),
    ("renders_to_listen", "Renders offline to listen"),
    ("audition", "Auditions candidates against a reference"),
    ("goal_rig", "Auditions candidates against a reference, and goals"),
    ("audio_file", "The audio file behind"),
    ("stems", "Splits a track into stems"),
    ("audio_to_midi", "Turns audio into MIDI"),
    ("focus_stream", "FOCUS and the beat light"),
    ("transport_stream", "FOCUS and the beat light"),
    ("device_tree", "A track's devices as a tree"),
    ("note_view", "The notes in the note editor"),
    ("timeline_strip", "The timeline at a glance"),
    ("launcher_grid", "A launcher grid"),
    ("pointing", "Pointing at something in the DAW"),
    ("presets", "Finds and loads the DAW's presets"),
    ("manual", "Searches the DAW's manual"),
    ("browser_items", "Loads Browser items by id"),
    ("configured_plugin_params", "Plug-in parameters shown once configured"),
    ("max_for_live_devices", "Max for Live devices"),
    ("ears", "Max for Live devices, Kumi Ears"),
    ("ui_hands", "Kumi's hands on the DAW's window"),
    ("extension_sdk", "Live's Extensions SDK"),
    ("willington", "Live's Extensions SDK, Willington"),
];

/// The words that start a FOCUS view's row in the English table.
fn view_row(kind: BodyKind) -> &'static str {
    match kind {
        BodyKind::Tree => "A track's devices as a tree",
        BodyKind::Notes => "The notes in the note editor",
        BodyKind::Grid => "A launcher grid",
        BodyKind::Timeline => "The timeline at a glance",
        BodyKind::Tracker => "The Pattern and Phrase Editors' lines",
        BodyKind::Matrix => "The Pattern Matrix",
        BodyKind::Keyzones => "An instrument's keyzones",
        BodyKind::Routing => "A track's routing",
        BodyKind::Mixer => "The mixer",
    }
}

/// How a cell may start in each language, as its legend names the forms (✓ first).
const FORMS: [(&str, &[&str]); 3] = [
    ("en", &["✓", "Live only", "REAPER only", "Renoise only", "not yet", "not needed", "no:", "instead:"]),
    ("ja", &["✓", "Live のみ", "REAPER のみ", "Renoise のみ", "まだ", "不要", "不可", "代わりに"]),
    ("zh-CN", &["✓", "仅 Live", "仅 REAPER", "仅 Renoise", "尚未", "不需要", "不能", "改为"]),
];

/// Each language's heading of a DAW's limits.
const LIMITS: [(&str, &str); 3] = [("en", "### Limits"), ("ja", "### 制限"), ("zh-CN", "### 限制")];

const COLUMNS: [(&DawProfile, &str); 3] =
    [(&DawProfile::ABLETON, "Live"), (&DawProfile::REAPER, "REAPER"), (&DawProfile::RENOISE, "Renoise")];

fn doc(language: &str) -> String {
    let path = format!("{}/../../docs/{language}/DAWS.md", env!("CARGO_MANIFEST_DIR"));
    std::fs::read_to_string(&path).unwrap_or_else(|error| panic!("{path}: {error}"))
}

/// The flags as their names and whether they're on (Option fields are on when Some), read from `Debug`, so a flag added
/// to `Capabilities` is checked without naming it here first.
fn flags(caps: &Capabilities) -> Vec<(String, bool)> {
    let text = format!("{caps:?}");
    let fields = text.split_once('{').expect("a struct").1.trim_end_matches('}');
    fields
        .split(',')
        .filter(|pair| !pair.trim().is_empty())
        .map(|pair| {
            let (name, value) = pair.split_once(':').expect("name: value");
            (name.trim().to_owned(), !matches!(value.trim(), "false" | "None"))
        })
        .collect()
}

/// The page's first table: its rows' cells, header first (the separator row left out).
fn table(text: &str) -> Vec<Vec<String>> {
    text.lines()
        .skip_while(|line| !line.starts_with('|'))
        .take_while(|line| line.starts_with('|'))
        .filter(|line| !line.starts_with("| ---"))
        .map(|line| line.trim().trim_matches('|').split('|').map(|cell| cell.trim().to_owned()).collect())
        .collect()
}

fn ticked(cell: &str) -> bool {
    cell.starts_with('✓')
}

/// A `## <heading>` section, up to the next `## `.
fn section<'a>(text: &'a str, heading: &str) -> &'a str {
    let start = text.find(&format!("\n## {heading}\n")).unwrap_or_else(|| panic!("no “## {heading}” section")) + 1;
    let rest = &text[start..];
    let end = rest[3..].find("\n## ").map_or(rest.len(), |at| at + 3);
    &rest[..end]
}

#[test]
fn every_capability_has_a_row_ticked_for_each_daw_whose_profile_has_it() {
    let rows = table(&doc("en"));
    assert_eq!(rows[0][1..], ["Live", "REAPER", "Renoise"], "the table's columns");
    let names: Vec<String> = flags(&Capabilities::NONE).into_iter().map(|(name, _)| name).collect();
    for (flag, _) in ROWS {
        assert!(names.iter().any(|name| name == flag), "ROWS names `{flag}`, which Capabilities no longer has");
    }
    let mut wrong = vec![];
    for name in &names {
        let Some((_, label)) = ROWS.iter().find(|(flag, _)| flag == name) else {
            panic!("Capabilities::{name} isn't in docs/en/DAWS.md: add a row to its table (and the other languages') and name it in ROWS");
        };
        let row = rows
            .iter()
            .find(|row| row[0].starts_with(label))
            .unwrap_or_else(|| panic!("docs/en/DAWS.md has no row starting “{label}” (Capabilities::{name})"));
        for (column, (profile, daw)) in COLUMNS.iter().enumerate() {
            let on = flags(&profile.caps).into_iter().any(|(flag, on)| flag == *name && on);
            let cell = &row[column + 1];
            if on && !ticked(cell) {
                wrong.push(format!("{name}: {daw}'s profile has it, the page says “{cell}”"));
            }
            if !on && ticked(cell) {
                wrong.push(format!("{name}: the page ticks it for {daw}, whose profile hasn't it"));
            }
        }
    }
    assert!(wrong.is_empty(), "docs/en/DAWS.md's table:\n{}", wrong.join("\n"));
}

#[test]
fn each_language_has_the_same_table() {
    let english = table(&doc("en"));
    let ticks = |row: &[String]| row[1..].iter().map(|cell| ticked(cell)).collect::<Vec<bool>>();
    for language in &LANGUAGES[1..] {
        let rows = table(&doc(language));
        assert_eq!(rows.len(), english.len(), "docs/{language}/DAWS.md's table has {} rows, English {}", rows.len(), english.len());
        assert_eq!(rows[0][1..], english[0][1..], "docs/{language}/DAWS.md's columns");
        assert!(rows.iter().all(|row| row.len() == 4), "docs/{language}/DAWS.md: every row has an ability and three DAWs");
        for (at, (row, theirs)) in rows.iter().zip(&english).enumerate() {
            assert_eq!(ticks(row), ticks(theirs), "docs/{language}/DAWS.md row {at} (“{}”) ticks other DAWs than English", row[0]);
        }
    }
}

#[test]
fn every_kind_category_is_named_in_its_daws_section() {
    for language in LANGUAGES {
        let text = doc(language);
        for (heading, kinds) in [("REAPER", reaper_changes::CHANGES.kinds), ("Renoise", renoise_changes::CHANGES.kinds)] {
            let part = section(&text, heading);
            let categories: BTreeSet<&str> = kinds.iter().map(|kind| kind.tool.split('_').next().unwrap_or(kind.tool)).collect();
            let missing: Vec<&&str> = categories.iter().filter(|category| !part.contains(&format!("`{category}_"))).collect();
            assert!(missing.is_empty(), "docs/{language}/DAWS.md's {heading} section names no `<category>_…` tool for {missing:?}");
        }
    }
}

#[test]
fn the_readmes_and_guides_link_the_page() {
    let root = concat!(env!("CARGO_MANIFEST_DIR"), "/../..");
    for (readme, language) in [("README.md", "en"), ("README.ja.md", "ja"), ("README.zh-CN.md", "zh-CN")] {
        let text = std::fs::read_to_string(format!("{root}/{readme}")).unwrap();
        assert!(text.contains(&format!("(docs/{language}/DAWS.md)")), "{readme} links docs/{language}/DAWS.md");
        let guide = std::fs::read_to_string(format!("{root}/docs/{language}/KUMI_GUIDE.md")).unwrap();
        assert!(guide.contains("(DAWS.md)"), "docs/{language}/KUMI_GUIDE.md links DAWS.md");
    }
}

#[test]
fn each_focus_view_reaper_and_renoise_draw_is_a_row_ticked_for_them() {
    let rows = table(&doc("en"));
    for (column, daw) in [(2, &reaper::DAW), (3, &renoise::DAW)] {
        let app = daw.profile.words.app;
        for kind in [
            BodyKind::Tree,
            BodyKind::Notes,
            BodyKind::Grid,
            BodyKind::Timeline,
            BodyKind::Tracker,
            BodyKind::Matrix,
            BodyKind::Keyzones,
            BodyKind::Routing,
            BodyKind::Mixer,
        ] {
            let label = view_row(kind);
            let row = rows.iter().find(|row| row[0].starts_with(label)).unwrap_or_else(|| panic!("no row “{label}” for {kind:?}"));
            let drawn = daw.bodies.contains(&kind);
            assert_eq!(ticked(&row[column]), drawn, "{app}'s cell for {kind:?} (“{label}”) is “{}”", row[column]);
        }
    }
}

#[test]
fn each_cell_speaks_its_legends_forms() {
    for (language, forms) in FORMS {
        let text = doc(language);
        let legend = &text[..text.find("\n| ").expect("a table")];
        for form in forms {
            assert!(legend.contains(form.trim_end_matches(':')), "docs/{language}/DAWS.md's legend names “{form}”");
        }
        for row in table(&text).iter().skip(1) {
            for cell in row[1..].iter().filter(|cell| !cell.is_empty()) {
                assert!(
                    forms.iter().any(|form| cell.starts_with(form)),
                    "docs/{language}/DAWS.md: “{cell}” (row “{}”) is no legend form",
                    row[0]
                );
            }
        }
    }
}

#[test]
fn every_kind_the_daws_own_undo_skips_is_named_in_its_limits() {
    for (language, heading) in LIMITS {
        let text = doc(language);
        for (name, kinds) in [("REAPER", reaper_changes::CHANGES.kinds), ("Renoise", renoise_changes::CHANGES.kinds)] {
            let part = section(&text, name);
            let limits = &part[part.find(heading).unwrap_or_else(|| panic!("docs/{language}/DAWS.md's {name} has no “{heading}”"))..];
            let limits = limits[4..].find("\n### ").map_or(limits, |end| &limits[..end + 4]);
            let missing: Vec<&str> = kinds
                .iter()
                .filter(|kind| kind.skipped_by_daw_undo && !limits.contains(&format!("`{}`", kind.tool)))
                .map(|kind| kind.tool)
                .collect();
            assert!(missing.is_empty(), "docs/{language}/DAWS.md's {name} limits don't name {missing:?}, which {name}'s undo skips");
        }
    }
}
