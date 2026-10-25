use super::*;

fn heard(pitch: Option<f64>, level_db: f64) -> Option<HeardSample> {
    Some(HeardSample { pitch, sure: if pitch.is_some() { 0.9 } else { 0. }, level_db, peak_db: level_db + 6., seconds: Some(2.) })
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
    assert_eq!(convention.middle_c, 3, "{convention:?}");
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
    assert_eq!(plan.convention.as_ref().unwrap().middle_c, 4);
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
    let plan = super::plan(&samples, &Asked { middle_c: Some(3), ..asked() }, &name);
    assert_eq!(placed(&plan, "Pad C3").root, 60);
    assert!(plan.questions.is_empty());
}

#[test]
fn tracker_notes_put_middle_c_at_c5_when_nothing_is_heard_and_are_checked_by_ear_when_it_is() {
    // A Renoise render named as Renoise names it: C-4 is MIDI 48.
    let samples: Vec<Sample> = ["Lead C-4", "Lead G-4"].iter().map(|stem| Sample::new(*stem, *stem, None)).collect();
    let plan = plan(&samples, &asked(), &name);
    assert_eq!(plan.convention.as_ref().unwrap().middle_c, 5);
    assert!(plan.questions.is_empty());
    assert_eq!(placed(&plan, "Lead G-4").root, 55);
    // Heard at MIDI 60 and 67, the same names are scientific.
    let heard_ones = vec![Sample::new("a", "Lead C-4", heard(Some(60.), -12.)), Sample::new("b", "Lead G-4", heard(Some(67.), -12.))];
    let plan = super::plan(&heard_ones, &asked(), &name);
    assert_eq!(plan.convention.as_ref().unwrap().middle_c, 4, "{plan:#?}");
    assert_eq!(placed(&plan, "Lead G-4").root, 67);
}

#[test]
fn zero_padded_numbers_are_notes_only_when_heard_so_or_when_they_dont_count_takes() {
    // Takes: 001 to 004, nothing heard.
    let takes: Vec<Sample> = ["Vox_001", "Vox_002", "Vox_003", "Vox_004"].iter().map(|stem| Sample::new(*stem, *stem, None)).collect();
    let plan = plan(&takes, &asked(), &name);
    assert_eq!(plan.layout, Layout::Drums, "no notes: a key each: {plan:#?}");
    assert!(plan.findings.iter().any(|f| f.contains("zero-padded")), "{:#?}", plan.findings);
    // Takes counted from 000, or with gaps (a take left out), nothing heard.
    for stems in [["Vox_000", "Vox_001", "Vox_002"], ["Vox_001", "Vox_002", "Vox_004"]] {
        let takes: Vec<Sample> = stems.iter().map(|stem| Sample::new(*stem, *stem, None)).collect();
        assert_eq!(super::plan(&takes, &asked(), &name).layout, Layout::Drums, "{stems:?} are takes");
    }
    // Notes: 048, 060, 072, nothing heard (not a run from 001).
    let notes: Vec<Sample> = ["Piano_048", "Piano_060", "Piano_072"].iter().map(|stem| Sample::new(*stem, *stem, None)).collect();
    let plan = super::plan(&notes, &asked(), &name);
    assert_eq!(placed(&plan, "Piano_060").root, 60);
    // Heard: a number the pitch agrees with stays (an octave off counts), one it doesn't is dropped for the pitch.
    let heard_ones = vec![Sample::new("a", "Piano_060", heard(Some(72.), -12.)), Sample::new("b", "Piano_002", heard(Some(64.), -12.))];
    let plan = super::plan(&heard_ones, &asked(), &name);
    assert_eq!(placed(&plan, "Piano_060").found, Found::Named);
    assert_eq!((placed(&plan, "Piano_002").root, placed(&plan, "Piano_002").found), (64, Found::Heard));
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
    let plan = plan(&samples, &Asked { middle_c: Some(4), ..asked() }, &name);
    let parts: Vec<Option<&str>> = plan.instruments.iter().map(|i| i.part.as_deref()).collect();
    assert_eq!(parts, vec![Some("plain"), Some("pizzicato"), Some("sustain")]);
    // An untagged sample is a part of its own: never a round robin of an articulation it may not be.
    assert_eq!(plan.instruments[0].samples.len(), 1);
    assert_eq!(plan.instruments[2].samples.len(), 2);
    assert!(plan.findings.iter().any(|f| f.starts_with("3 parts (plain, pizzicato, sustain)")), "{:#?}", plan.findings);
    assert_eq!(plan.left_out.len(), 1);
    assert!(plan.left_out[0].1.contains("release"));
}

#[test]
fn one_articulation_named_beside_plain_samples_is_two_instruments() {
    let samples = vec![Sample::new("a", "Violin C4", heard(Some(60.), -12.)), Sample::new("b", "Violin C4 pizz", heard(Some(60.), -12.))];
    let plan = plan(&samples, &asked(), &name);
    assert_eq!(plan.instruments.len(), 2, "{plan:#?}");
    assert!(plan.instruments.iter().all(|planned| !planned.cycle), "arco and pizz aren't round robins of each other");
}

#[test]
fn a_sample_on_the_note_off_layer_is_left_and_a_transposed_one_gets_a_base_that_allows_for_it() {
    let mut off = Sample::new("a", "Pad C4", heard(Some(60.), -12.));
    off.note_off = true;
    let mut moved = Sample::new("b", "Pad G4", heard(Some(67.), -12.));
    moved.transpose = 2;
    let plan = plan(&[off, moved, Sample::new("c", "Pad C5", heard(Some(72.), -12.))], &asked(), &name);
    assert!(plan.left_out.iter().any(|(sample, why)| sample == "Pad C4" && why.contains("note-off")), "{:#?}", plan.left_out);
    let placed = placed(&plan, "Pad G4");
    assert_eq!((placed.root, placed.base), (67, 69), "it plays as recorded at its root");
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
    assert_eq!(placed(&plan, "Weird Zap").root, 82, "no GM key: past GM's keys, which a GM file's percussion plays");
}

#[test]
fn a_root_outside_the_daws_keys_is_left_out() {
    let samples = vec![Sample::new("a", "Sub C0", heard(Some(12.), -10.)), Sample::new("b", "Sub C1", heard(Some(24.), -10.))];
    let plan = plan(&samples, &Asked { lowest: 13, ..asked() }, &name);
    assert_eq!(plan.left_out.len(), 1);
    assert_eq!(placed(&plan, "Sub C1").zone, (13, 131));
}
