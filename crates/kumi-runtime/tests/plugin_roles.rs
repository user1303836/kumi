//! Knobs found by role: Ozone 12's roles against the names Live listed for it, a role Live hasn't configured said with
//! how to configure it, another device on the track doing the job in the agreed order (Ozone 12, the other mapped
//! plug-ins, Live's own devices), a job on Live's own devices by their Live 12 names, and fixes that name a role.
use kumi_runtime::plugins::roles::{agreed_order, find, job_for_item, job_named, plugin_fix, resolve, Found, Resolved, Seen};
use serde_json::Value;

/// Ozone 12.1's own parameter names, as Live listed them on the machine its format was read on.
fn ozone_names() -> Vec<String> {
    let text =
        std::fs::read_to_string(concat!(env!("CARGO_MANIFEST_DIR"), "/../../plugin-formats/ozone-12/format-1/structure.json")).unwrap();
    let structure: Value = serde_json::from_str(&text).unwrap();
    structure["parameters"].as_object().unwrap().keys().filter(|name| !name.contains(" / ")).cloned().collect()
}

fn strings(items: &[&str]) -> Vec<String> {
    items.iter().map(|item| item.to_string()).collect()
}

fn ozone(turnable: &[&str]) -> Seen {
    Seen {
        reference: "d1".into(),
        name: "Ozone 12".into(),
        class: "PluginDevice".into(),
        turnable: strings(turnable),
        listed: Some(ozone_names()),
    }
}

fn stock(reference: &str, name: &str, class: &str, turnable: &[&str]) -> Seen {
    Seen { reference: reference.into(), name: name.into(), class: class.into(), turnable: strings(turnable), listed: None }
}

/// Live 12.4's Limiter, by the names Live gave its parameters.
fn limiter(reference: &str) -> Seen {
    let names = [
        "Device On",
        "Input Gain",
        "Ceiling",
        "Release",
        "Auto",
        "Link",
        "M/S Link",
        "Lookahead",
        "Routing",
        "Mode",
        "Maximize On",
        "Threshold",
        "Output",
    ];
    stock(reference, "Limiter", "Limiter", &names)
}

fn knob(found: Option<Found>) -> String {
    match found {
        Some(Found::Knob { name, .. }) => name,
        other => panic!("not a knob: {other:?}"),
    }
}

#[test]
fn ozone_12s_roles_land_on_the_names_live_lists_for_it() {
    let names = ozone_names();
    assert!(names.len() > 100, "{} names", names.len());
    let all = ozone(&[]);
    let full = Seen { turnable: names.clone(), ..all.clone() };
    assert_eq!(knob(find(&full, "threshold")), "MAX: Input Gain");
    assert_eq!(knob(find(&full, "ceiling")), "MAX: Output Level");
    assert_eq!(knob(find(&full, "character")), "MAX: Character");
    // A job's word reaches the role that does it.
    assert_eq!(knob(find(&full, "limiter gain")), "MAX: Input Gain");
    assert_eq!(knob(find(&full, "Limiter Ceiling")), "MAX: Output Level");
    // Several knobs (bands, or a role's switches beside its knobs): one has to be named.
    for (word, count) in [("width", 6), ("compressor threshold", 4), ("exciter", 8)] {
        match find(&full, word) {
            Some(Found::Several(why)) => assert!(why.contains(&format!("is {count} knobs")), "{word}: {why}"),
            other => panic!("{word}: {other:?}"),
        }
    }
    // The compressor's threshold isn't the band's limiter's.
    match find(&full, "compressor threshold") {
        Some(Found::Several(why)) => assert!(why.contains("Comp Threshold") && !why.contains("Lim Threshold"), "{why}"),
        other => panic!("{other:?}"),
    }
    // Live listed no Equalizer bands for this Ozone (they're added in its window): said, not guessed.
    match find(&full, "eq gain") {
        Some(Found::Missing(why)) => assert!(why.contains("None of the") && why.contains("Ozone 12's eq gain"), "{why}"),
        other => panic!("{other:?}"),
    }
    // A word that's no role and no job is a knob's name, for tune's own lookup.
    assert_eq!(find(&full, "MAX: Bypass"), None);
}

#[test]
fn a_role_live_hasnt_configured_is_said_with_how_to_configure_it() {
    let device = ozone(&["Device On", "MAX: Input Gain"]);
    assert_eq!(knob(find(&device, "threshold")), "MAX: Input Gain");
    match find(&device, "ceiling") {
        Some(Found::Missing(why)) => {
            assert!(why.contains("MAX: Output Level") && why.contains("isn't configured") && why.contains("Configure"), "{why}");
            assert!(why.contains("set_device_details"), "{why}");
        }
        other => panic!("{other:?}"),
    }
    // A plug-in Kumi has no map of can't be asked by role.
    let unknown = Seen { name: "Mystery Limiter".into(), listed: Some(strings(&["Out"])), ..ozone(&["Out"]) };
    match find(&unknown, "ceiling") {
        Some(Found::Missing(why)) => assert!(why.contains("no map of Mystery Limiter"), "{why}"),
        other => panic!("{other:?}"),
    }
}

#[test]
fn another_device_on_the_track_does_the_job_in_the_agreed_order() {
    // Ozone's ceiling isn't configured; the Limiter after it does the job.
    let devices = vec![ozone(&["Device On", "MAX: Input Gain"]), limiter("d2")];
    match resolve(&devices, 0, &strings(&["ceiling"])) {
        Resolved::On { device, knobs, instead: Some(instead), .. } => {
            assert_eq!((device, knobs), (1, strings(&["Ceiling"])));
            assert!(instead.contains("isn't configured") && instead.contains("Kumi tunes Limiter on the same track instead"), "{instead}");
        }
        other => panic!("{other:?}"),
    }
    // Configured, it's Ozone's own, and the log says how the role was read.
    match resolve(&devices, 0, &strings(&["threshold"])) {
        Resolved::On { device: 0, knobs, read, instead: None } => {
            assert_eq!(knobs, strings(&["MAX: Input Gain"]));
            assert_eq!(read, strings(&["Ozone 12's threshold is MAX: Input Gain"]));
        }
        other => panic!("{other:?}"),
    }
    // Asked of an EQ Eight: Ozone 12 first, then the other mapped plug-ins, then Live's own devices, later ones first.
    let pro_l = |turnable: &[&str]| Seen {
        reference: "d3".into(),
        name: "FabFilter Pro-L 2".into(),
        class: "PluginDevice".into(),
        turnable: strings(turnable),
        listed: Some(strings(&["Gain", "Output Level", "Style", "Lookahead"])),
    };
    let eq = stock("d4", "EQ Eight", "Eq8", &["Device On", "1 Gain A", "1 Frequency A"]);
    let track = |ozone_knobs: &[&str], pro_l_knobs: &[&str]| vec![limiter("d0"), pro_l(pro_l_knobs), ozone(ozone_knobs), eq.clone()];
    assert_eq!(agreed_order(&track(&[], &[]), Some(3)), vec![2, 1, 0]);
    let on = |devices: &[Seen]| match resolve(devices, 3, &strings(&["limiter gain"])) {
        Resolved::On { device, knobs, .. } => (device, knobs),
        other => panic!("{other:?}"),
    };
    assert_eq!(on(&track(&["MAX: Input Gain"], &["Gain"])), (2, strings(&["MAX: Input Gain"])));
    assert_eq!(on(&track(&[], &["Gain"])), (1, strings(&["Gain"])));
    assert_eq!(on(&track(&[], &[])), (0, strings(&["Input Gain"])));
    // Nothing on the track can: Live's own device to put there, said.
    match resolve(&[ozone(&[])], 0, &strings(&["ceiling"])) {
        Resolved::Refused(why) => assert!(why.contains("isn't configured") && why.contains("Live's Limiter (Ceiling)"), "{why}"),
        other => panic!("{other:?}"),
    }
    // Another device does the job but has several knobs for it: which, to name one there.
    let eq_bands = stock("d5", "EQ Eight", "Eq8", &["Device On", "1 Gain A", "2 Gain A"]);
    match resolve(&[limiter("d0"), eq_bands], 0, &strings(&["eq gain"])) {
        Resolved::Refused(why) => {
            assert!(
                why.starts_with("Limiter has no eq gain.") && why.contains("EQ Eight on the same track does it (device \"d5\")"),
                "{why}"
            );
            assert!(why.contains("1 Gain A, 2 Gain A. Name one in knobs."), "{why}");
        }
        other => panic!("{other:?}"),
    }
    // A knob named as the device shows it isn't a role: as asked.
    assert_eq!(resolve(&[limiter("d0")], 0, &strings(&["Input Gain"])), Resolved::AsAsked);
    assert_eq!(resolve(&[limiter("d0")], 0, &strings(&["Release"])), Resolved::AsAsked);
}

#[test]
fn a_job_on_lives_own_devices_by_their_live_12_names() {
    assert_eq!(knob(find(&limiter("d0"), "limiter gain")), "Input Gain");
    // An earlier Live's Limiter called it Gain.
    assert_eq!(knob(find(&stock("d9", "Limiter", "Limiter", &["Device On", "Gain", "Ceiling"]), "limiter gain")), "Gain");
    // A Limiter showing no gain knob says so, and doesn't send for another Limiter.
    match resolve(&[stock("d9", "Limiter", "Limiter", &["Device On", "Ceiling"])], 0, &strings(&["limiter gain"])) {
        Resolved::Refused(why) => assert_eq!(why, "Limiter shows no limiter gain knob (Input Gain). Nothing else on this track does it."),
        other => panic!("{other:?}"),
    }
    assert_eq!(knob(find(&limiter("d0"), "ceiling")), "Ceiling");
    // Live 12's names, as Live gives them.
    let compressor = stock("d1", "Compressor", "Compressor2", &["Device On", "Threshold", "Ratio", "Expansion Ratio", "Attack", "Release"]);
    assert_eq!(knob(find(&compressor, "compressor ratio")), "Ratio");
    let glue =
        stock("d2", "Glue Compressor", "GlueCompressor", &["Device On", "Threshold", "Range", "Output", "Attack", "Ratio", "Release"]);
    assert_eq!(knob(find(&glue, "compressor attack")), "Attack");
    let saturator = stock("d3", "Saturator", "Saturator", &["Device On", "Drive", "Type", "Output", "Dry/Wet"]);
    assert_eq!(knob(find(&saturator, "saturation")), "Drive");
    let eq = stock("d4", "EQ Eight", "Eq8", &["Device On", "1 Gain A", "2 Gain A", "1 Frequency A"]);
    assert!(matches!(find(&eq, "eq gain"), Some(Found::Several(_))));
    match find(&eq, "ceiling") {
        Some(Found::Missing(why)) => assert_eq!(why, "EQ Eight has no ceiling."),
        other => panic!("{other:?}"),
    }
    assert_eq!(job_named("Comp_Threshold").map(|job| job.name), Some("compressor threshold"));
}

#[test]
fn a_fix_names_a_mapped_plug_ins_role_when_one_on_the_track_does_the_job() {
    assert_eq!(job_for_item("loudness").map(|job| job.name), Some("limiter gain"));
    assert_eq!(job_for_item("true peak").map(|job| job.name), Some("ceiling"));
    assert_eq!(job_for_item("balance low mids").map(|job| job.name), Some("eq gain"));
    assert!(job_for_item("punch").is_none());
    let generic = "the last limiter's gain, homed in (tune with how: home, knobs [\"Gain\"])";
    let loudness = job_for_item("loudness").unwrap();
    let fix = plugin_fix(&[limiter("d0"), ozone(&["MAX: Input Gain"])], loudness, Some(generic)).unwrap();
    assert!(
        fix.starts_with("Ozone 12's threshold (MAX: Input Gain), homed in (tune with how: home, device \"d1\", knobs [\"threshold\"])"),
        "{fix}"
    );
    assert!(fix.ends_with(&format!("; or {generic}")), "{fix}");
    // Not configured: what to do, and the fix it had.
    let fix = plugin_fix(&[ozone(&[])], loudness, Some(generic)).unwrap();
    assert!(fix.contains("would do it") && fix.contains("Configure") && fix.ends_with(generic), "{fix}");
    // Live's own devices only: the fix stays as it was.
    assert_eq!(plugin_fix(&[limiter("d0")], loudness, Some(generic)), None);
}
