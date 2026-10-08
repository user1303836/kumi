//! Knobs found by role: Ozone 12's roles against the names Live listed for it, a role Live hasn't configured said with
//! how to configure it, another device on the track doing the job in the agreed order (Ozone 12, the other mapped
//! plug-ins, Live's own devices; none that's off), Live's own device to add placed where its job goes, a job on Live's
//! own devices by their Live 12 names, and fixes that name a role.
use kumi_runtime::plugins::roles::{
    agreed_order, find, job_for_item, job_named, nearest_band, plugin_fix, resolve, resolve_near, Found, Resolved, Seen,
};
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
        off: false,
        switched_on: vec![],
        bands: vec![],
    }
}

fn stock(reference: &str, name: &str, class: &str, turnable: &[&str]) -> Seen {
    Seen {
        reference: reference.into(),
        name: name.into(),
        class: class.into(),
        turnable: strings(turnable),
        listed: None,
        off: false,
        switched_on: vec![],
        bands: vec![],
    }
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
    for (word, count) in [("width", 4), ("compressor threshold", 4), ("exciter", 8)] {
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
    // Width is a job too, narrowed as the job is: the Stereoizer's delay isn't width, though Ozone's role covers it.
    match find(&ozone(&["IMG: Aux Stereoizer Delay"]), "width") {
        Some(Found::Missing(why)) => assert!(why.contains("isn't configured") && !why.contains("Stereoizer"), "{why}"),
        other => panic!("{other:?}"),
    }
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
        off: false,
        switched_on: vec![],
        bands: vec![],
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
    // Nothing on the track can: Live's own device to put there, said, and where: a limiter's job last on the chain,
    // any other before the track's last limiter.
    match resolve(&[ozone(&[])], 0, &strings(&["ceiling"])) {
        Resolved::Refused(why) => {
            assert!(why.contains("isn't configured") && why.contains("Live's Limiter (Ceiling) last on the chain"), "{why}")
        }
        other => panic!("{other:?}"),
    }
    match resolve(&[limiter("d0")], 0, &strings(&["eq gain"])) {
        Resolved::Refused(why) => {
            assert!(why.contains("Live's EQ Eight (band gain: 1 Gain A … 8 Gain A) before Limiter (device \"d0\")"), "{why}");
            assert!(!why.contains("last on the chain"), "{why}");
        }
        other => panic!("{other:?}"),
    }
    // Ozone 12 is a final limiter too.
    match resolve(&[stock("d0", "Saturator", "Saturator", &["Device On", "Drive"]), ozone(&[])], 0, &strings(&["compressor ratio"])) {
        Resolved::Refused(why) => assert!(why.contains("Live's Compressor (Ratio) before Ozone 12 (device \"d1\")"), "{why}"),
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
fn a_device_thats_off_isnt_tried_and_is_said_when_its_the_only_one() {
    let glue = stock("d1", "Glue Compressor", "GlueCompressor", &["Device On", "Threshold", "Ratio"]);
    let off = |device: Seen| Seen { off: true, ..device };
    // A Limiter kept off for an A/B: said, not tuned.
    match resolve(&[glue.clone(), off(limiter("d2"))], 0, &strings(&["limiter gain"])) {
        Resolved::Refused(why) => assert!(why.contains("Limiter on the same track does it (device \"d2\"), but it's off"), "{why}"),
        other => panic!("{other:?}"),
    }
    // Ozone 12 off, a Limiter on: the Limiter does it.
    let track = vec![off(ozone(&["MAX: Input Gain"])), limiter("d2"), glue];
    assert_eq!(agreed_order(&track, Some(2)), vec![1]);
    match resolve(&track, 2, &strings(&["limiter gain"])) {
        Resolved::On { device, knobs, .. } => assert_eq!((device, knobs), (1, strings(&["Input Gain"]))),
        other => panic!("{other:?}"),
    }
    // And a fix doesn't name it.
    assert_eq!(plugin_fix(&track, job_for_item("loudness").unwrap(), Some("the last limiter's gain")), None);
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
    // Maximizing, Live 12.4's Limiter turns Threshold for its gain and Output for its ceiling (Input Gain and Ceiling
    // do nothing then, as heard on real Live).
    let maximizing = Seen { switched_on: strings(&["Maximize On"]), ..limiter("d0") };
    assert_eq!(knob(find(&maximizing, "limiter gain")), "Threshold");
    assert_eq!(knob(find(&maximizing, "ceiling")), "Output");
    // A Limiter to add is said as it comes, not maximizing.
    match resolve(&[stock("d4", "EQ Eight", "Eq8", &["Device On", "1 Gain A"])], 0, &strings(&["ceiling"])) {
        Resolved::Refused(why) => assert!(why.contains("Live's Limiter (Ceiling) last on the chain"), "{why}"),
        other => panic!("{other:?}"),
    }
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
    let generic = "the last limiter's gain, homed in (tune with how: home, knobs [\"limiter gain\"])";
    let loudness = job_for_item("loudness").unwrap();
    let fix = plugin_fix(&[limiter("d0"), ozone(&["MAX: Input Gain"])], loudness, Some(generic)).unwrap();
    assert!(
        fix.starts_with(
            "Ozone 12's threshold (MAX: Input Gain), homed in (tune with how: home, device \"d1\", knobs [\"MAX: Input Gain\"])"
        ),
        "{fix}"
    );
    assert!(fix.ends_with(&format!("; or {generic}")), "{fix}");
    // Not configured: what to do, and the fix it had.
    let fix = plugin_fix(&[ozone(&[])], loudness, Some(generic)).unwrap();
    assert!(fix.contains("would do it") && fix.contains("Configure") && fix.ends_with(generic) && !fix.contains(".; or"), "{fix}");
    // Live's own devices only: the fix stays as it was.
    assert_eq!(plugin_fix(&[limiter("d0")], loudness, Some(generic)), None);
    // A role Live lists no name for (Ozone's EQ bands here) leads nowhere: the next plug-in that does the job, or the
    // fix as it was.
    let balance = job_for_item("balance low mids").unwrap();
    assert_eq!(plugin_fix(&[ozone(&[])], balance, Some("an EQ Eight")), None);
    let pro_q = Seen {
        reference: "d2".into(),
        name: "FabFilter Pro-Q 4".into(),
        class: "PluginDevice".into(),
        turnable: strings(&["Band 1 Gain"]),
        listed: Some(strings(&["Band 1 Gain", "Band 1 Frequency"])),
        off: false,
        switched_on: vec![],
        bands: vec![],
    };
    let fix = plugin_fix(&[ozone(&[]), pro_q], balance, Some("an EQ Eight")).unwrap();
    assert!(fix.starts_with("Pro-Q 4's gain (Band 1 Gain)") && fix.contains("knobs [\"Band 1 Gain\"]"), "{fix}");
}

#[test]
fn low_width_is_the_lowest_bands_width_only() {
    let low = job_for_item("low width").unwrap();
    assert_eq!(low.name, "low width");
    // Only another band configured: the lowest band's knob is named, to configure.
    let fix = plugin_fix(&[ozone(&["IMG: Aux Band 3 Width Percent"])], low, None).unwrap();
    assert!(fix.contains("would do it") && fix.contains("IMG: Aux Band 1 Width Percent") && !fix.contains("Band 3"), "{fix}");
    // Configured, it's that knob by its name.
    let fix = plugin_fix(&[ozone(&["IMG: Aux Band 1 Width Percent", "IMG: Aux Band 3 Width Percent"])], low, None).unwrap();
    assert!(fix.contains("knobs [\"IMG: Aux Band 1 Width Percent\"]"), "{fix}");
    // A reverb's width isn't the low end's.
    let supermassive = Seen {
        reference: "d3".into(),
        name: "ValhallaSupermassive".into(),
        class: "PluginDevice".into(),
        turnable: strings(&["Width"]),
        listed: Some(strings(&["Width", "Mix"])),
        off: false,
        switched_on: vec![],
        bands: vec![],
    };
    assert_eq!(plugin_fix(&[supermassive], low, Some("the Imager")), None);
}

#[test]
fn an_eq_band_named_by_role_is_the_one_nearest_the_target() {
    // EQ Eight's bands: number, frequency, on, and whether its gain shapes (a bell or a shelf, not a cut).
    let bands = [(1, 40., true, false), (2, 120., true, true), (3, 900., false, true), (4, 2500., true, true), (8, 12000., true, true)];
    assert_eq!(nearest_band(&bands, 3000., true), Some(4));
    assert_eq!(nearest_band(&bands, 100., true), Some(2));
    // Only bands that are on, within about an octave: at 900 Hz band 3 is off, and the others sit further off.
    assert_eq!(nearest_band(&bands, 900., true), None);
    // A gain needs a band that shapes; a frequency can be a cut's.
    assert_eq!(nearest_band(&bands, 40., true), None);
    assert_eq!(nearest_band(&bands, 40., false), Some(1));
    // A 5 kHz target doesn't home a 200 Hz bell.
    assert_eq!(nearest_band(&[(3, 200., true, true)], 5000., true), None);
    // On the EQ Eight asked, or one reached by fallback, "eq gain" is the nearest band's gain.
    let mut eq = stock("d5", "EQ Eight", "Eq8", &["Device On", "2 Gain A", "4 Gain A", "8 Gain A"]);
    eq.bands = bands.to_vec();
    match resolve_near(&[eq.clone()], 0, &strings(&["eq gain"]), Some(2800.)) {
        Resolved::On { device: 0, knobs, read, .. } => {
            assert_eq!(knobs, ["4 Gain A"]);
            assert_eq!(read, ["EQ Eight's eq gain is 4 Gain A, the band nearest 2.8 kHz"]);
        }
        other => panic!("{other:?}"),
    }
    match resolve_near(&[limiter("d0"), eq.clone()], 0, &strings(&["eq gain"]), Some(110.)) {
        Resolved::On { device: 1, knobs, .. } => assert_eq!(knobs, ["2 Gain A"]),
        other => panic!("{other:?}"),
    }
    // None near: Kumi asks which band.
    match resolve_near(&[eq], 0, &strings(&["eq gain"]), Some(500.)) {
        Resolved::Refused(why) => assert!(why.starts_with("None of EQ Eight's bands that are on and shape"), "{why}"),
        other => panic!("{other:?}"),
    }
}
