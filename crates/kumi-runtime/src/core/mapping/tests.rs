use super::*;

fn heard(pitch: Option<f64>, level_db: f64) -> Option<HeardSample> {
    Some(HeardSample { pitch, sure: if pitch.is_some() { 0.9 } else { 0. }, level_db, peak_db: level_db + 6., seconds: 2. })
}

fn asked() -> Asked {
    Asked { lowest: 12, highest: 131, first_key: 60, ..Asked::default() }
}

fn name(midi: i32) -> String {
    const CLASSES: [&str; 12] = ["C", "C#", "D", "D#", "E", "F", "F#", "G", "G#", "A", "A#", "B"];
    format!("{}{}", CLASSES[midi.rem_euclid(12) as usize], midi.div_euclid(12) - 1)
}

fn placed<'a>(plan: &'a Plan, sample: &str) -> &'a Placed {
    plan.instruments.iter().flat_map(|i| &i.samples).find(|p| p.name == sample).unwrap_or_else(|| panic!("{sample} not placed: {plan:#?}"))
}

#[test]
fn names_an_octave_low_are_found_out_by_their_pitch() {
    // "C3" sounds as MIDI 60: the names use C3 = middle C.
    let samples: Vec<Sample> = [("Piano_C3", 60.), ("Piano_E3", 64.05), ("Piano_G3", 67.), ("Piano_C4", 72.)]
        .iter()
        .map(|(stem, pitch)| Sample::new(*stem, *stem, heard(Some(*pitch), -12.)))
        .collect();
    let plan = plan(&samples, &asked(), &name);
    assert_eq!(plan.layout, Layout::Keys);
    let convention = plan.convention.clone().unwrap();
    assert!(!convention.c4_is_60, "{convention:?}");
    assert_eq!(convention.how, "the pitch heard agrees in 4 of 4");
    assert_eq!(placed(&plan, "Piano_C3").root, 60);
    assert_eq!(placed(&plan, "Piano_C4").root, 72);
    // Zones reach halfway: C (60) to D (62), E (64) from 63 to 65, the last to the top.
    assert_eq!(placed(&plan, "Piano_C3").zone, (12, 62));
    assert_eq!(placed(&plan, "Piano_E3").zone, (63, 65));
    assert_eq!(placed(&plan, "Piano_C4").zone, (70, 131));
    assert!(plan.questions.is_empty());
}

#[test]
fn scientific_names_stay_where_they_say() {
    let samples: Vec<Sample> =
        [("Bass C2", 36.), ("Bass G2", 43.2)].iter().map(|(stem, pitch)| Sample::new(*stem, *stem, heard(Some(*pitch), -10.))).collect();
    let plan = plan(&samples, &asked(), &name);
    assert!(plan.convention.as_ref().unwrap().c4_is_60);
    assert_eq!(placed(&plan, "Bass C2").root, 36);
}

#[test]
fn with_nothing_heard_the_convention_is_asked_and_c4_planned() {
    let samples: Vec<Sample> = ["Pad C3", "Pad C4"].iter().map(|stem| Sample::new(*stem, *stem, None)).collect();
    let plan = plan(&samples, &asked(), &name);
    assert!(plan.convention.is_none());
    assert_eq!(plan.questions.len(), 1, "{plan:#?}");
    assert_eq!(placed(&plan, "Pad C4").root, 60);
    // Asked: C3.
    let plan = super::plan(&samples, &Asked { c4_is_60: Some(false), ..asked() }, &name);
    assert_eq!(placed(&plan, "Pad C3").root, 60);
    assert!(plan.questions.is_empty());
}

#[test]
fn tracker_notes_are_c4_without_asking() {
    let samples: Vec<Sample> = ["Lead C-4", "Lead G-4"].iter().map(|stem| Sample::new(*stem, *stem, None)).collect();
    let plan = plan(&samples, &asked(), &name);
    assert!(plan.convention.as_ref().unwrap().c4_is_60);
    assert!(plan.questions.is_empty());
    assert_eq!(placed(&plan, "Lead G-4").root, 67);
}

#[test]
fn velocity_layers_follow_the_names_and_loudness_is_checked() {
    let samples = vec![
        Sample::new("a", "Piano_C4_v1", heard(Some(60.), -20.)),
        Sample::new("b", "Piano_C4_v2", heard(Some(60.), -14.)),
        Sample::new("c", "Piano_C4_v3", heard(Some(60.), -18.)),
    ];
    let plan = plan(&samples, &asked(), &name);
    assert_eq!(placed(&plan, "Piano_C4_v1").velocity, (0, 41));
    assert_eq!(placed(&plan, "Piano_C4_v2").velocity, (42, 84));
    assert_eq!(placed(&plan, "Piano_C4_v3").velocity, (85, 127));
    assert!(plan.findings.iter().any(|f| f.contains("“Piano_C4_v3” is quieter than “Piano_C4_v2”")), "{:#?}", plan.findings);
    assert!(!plan.instruments[0].cycle, "layers don't overlap");
}

#[test]
fn velocity_values_split_at_their_midpoints() {
    let samples = vec![
        Sample::new("a", "Snare vel40", heard(None, -20.)),
        Sample::new("b", "Snare vel80", heard(None, -14.)),
        Sample::new("c", "Snare vel127", heard(None, -8.)),
    ];
    let plan = plan(&samples, &asked(), &name);
    assert_eq!(plan.layout, Layout::Drums);
    assert_eq!(placed(&plan, "Snare vel40").velocity, (0, 60));
    assert_eq!(placed(&plan, "Snare vel80").velocity, (61, 103));
    assert_eq!(placed(&plan, "Snare vel127").velocity, (104, 127));
    // One key for all three layers.
    assert_eq!(placed(&plan, "Snare vel40").root, placed(&plan, "Snare vel127").root);
}

#[test]
fn round_robins_share_a_layer_and_cycle() {
    let samples = vec![
        Sample::new("a", "Gtr_E2_rr1", heard(Some(40.), -12.)),
        Sample::new("b", "Gtr_E2_rr2", heard(Some(40.), -12.3)),
        Sample::new("c", "Gtr_E2_rr3", heard(Some(40.), -11.8)),
    ];
    let plan = plan(&samples, &asked(), &name);
    assert!(plan.instruments[0].cycle);
    assert_eq!(placed(&plan, "Gtr_E2_rr2").round_robin, Some(2));
    assert_eq!(placed(&plan, "Gtr_E2_rr2").velocity, (0, 127));
}

#[test]
fn unnamed_samples_at_one_key_are_layers_when_loudness_differs_and_round_robins_when_it_doesnt() {
    let layered = vec![Sample::new("a", "Hit A", heard(Some(48.), -24.)), Sample::new("b", "Hit B", heard(Some(48.), -10.))];
    let plan = plan(&layered, &asked(), &name);
    assert_eq!(placed(&plan, "Hit A").velocity, (0, 63), "the quieter one plays soft");
    assert_eq!(placed(&plan, "Hit B").velocity, (64, 127));
    let alike = vec![Sample::new("a", "Hit A", heard(Some(48.), -12.)), Sample::new("b", "Hit B", heard(Some(48.), -12.5))];
    let plan = super::plan(&alike, &asked(), &name);
    assert!(plan.instruments[0].cycle, "{plan:#?}");
    assert!(plan.findings.iter().any(|f| f.contains("round robins")));
}

#[test]
fn a_name_the_pitch_disagrees_with_is_said_and_the_name_wins() {
    let samples = vec![
        Sample::new("a", "Vibe C4", heard(Some(60.), -12.)),
        Sample::new("b", "Vibe E4", heard(Some(66.), -12.)),
        Sample::new("c", "Vibe G4", heard(Some(67.), -12.)),
    ];
    let plan = plan(&samples, &asked(), &name);
    assert_eq!(placed(&plan, "Vibe E4").root, 64);
    assert!(plan.findings.iter().any(|f| f.contains("“Vibe E4” sounds nearer F#4 than E4")), "{:#?}", plan.findings);
}

#[test]
fn articulations_become_an_instrument_each_and_releases_are_left_out() {
    let samples = vec![
        Sample::new("a", "Cello_sus_C3", heard(Some(48.), -12.)),
        Sample::new("b", "Cello_sus_G3", heard(Some(55.), -12.)),
        Sample::new("c", "Cello_pizz_C3", heard(Some(48.), -12.)),
        Sample::new("d", "Cello_C3", heard(Some(48.), -12.)),
        Sample::new("e", "Cello_C3_rel", heard(Some(48.), -30.)),
    ];
    let plan = plan(&samples, &Asked { c4_is_60: Some(true), ..asked() }, &name);
    let parts: Vec<Option<&str>> = plan.instruments.iter().map(|i| i.part.as_deref()).collect();
    assert_eq!(parts, vec![Some("pizzicato"), Some("sustain")]);
    // The plain one goes with the articulation most samples have.
    assert_eq!(plan.instruments[1].samples.len(), 3);
    assert_eq!(plan.left_out.len(), 1);
    assert!(plan.left_out[0].1.contains("release"));
}

#[test]
fn unpitched_unnamed_samples_are_left_out_with_why() {
    let samples = vec![
        Sample::new("a", "Pad C3", heard(Some(60.), -12.)),
        Sample::new("b", "Pad C4", heard(Some(72.), -12.)),
        Sample::new("c", "Texture", heard(None, -12.)),
    ];
    let plan = plan(&samples, &Asked { layout: Some(Layout::Keys), ..asked() }, &name);
    assert_eq!(plan.left_out, vec![("Texture".to_string(), "no note in its name and no pitch heard: give its root".to_string())]);
    // Given its root, it's placed.
    let mut roots = BTreeMap::new();
    roots.insert("Texture".to_string(), 84);
    let plan = super::plan(&samples, &Asked { layout: Some(Layout::Keys), roots, ..asked() }, &name);
    assert_eq!(placed(&plan, "Texture").found, Found::Given);
}

#[test]
fn a_drum_kit_goes_in_role_order_or_on_gm_keys() {
    let samples: Vec<Sample> = ["Open Hat", "Snare 01", "Kick", "Closed Hat", "Weird Zap"]
        .iter()
        .map(|stem| Sample::new(*stem, *stem, heard(None, -10.)))
        .collect();
    let plan = plan(&samples, &asked(), &name);
    assert_eq!(plan.layout, Layout::Drums);
    let order: Vec<(&str, i32)> = plan.instruments[0].samples.iter().map(|p| (p.name.as_str(), p.root)).collect();
    assert_eq!(order, vec![("Kick", 60), ("Snare 01", 61), ("Closed Hat", 62), ("Open Hat", 63), ("Weird Zap", 64)]);
    let plan = super::plan(&samples, &Asked { gm: true, ..asked() }, &name);
    assert_eq!(placed(&plan, "Kick").root, 36);
    assert_eq!(placed(&plan, "Snare 01").root, 38);
    assert_eq!(placed(&plan, "Closed Hat").root, 42);
    assert_eq!(placed(&plan, "Open Hat").root, 46);
    assert_eq!(placed(&plan, "Weird Zap").root, 60, "no GM key: from the first key up");
}

#[test]
fn a_root_outside_the_daws_keys_is_left_out() {
    let samples = vec![Sample::new("a", "Sub C0", heard(Some(12.), -10.)), Sample::new("b", "Sub C1", heard(Some(24.), -10.))];
    let plan = plan(&samples, &Asked { lowest: 13, ..asked() }, &name);
    assert_eq!(plan.left_out.len(), 1);
    assert_eq!(placed(&plan, "Sub C1").zone, (13, 131));
}
