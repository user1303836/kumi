use super::*;

/// An instrument's file as Renoise 3.5.4 writes one (cut to two macros): macro 1 maps an AHDSR's Attack in modulation set
/// 1 (Renoise's own long number kept as written), macro 2 maps nothing.
const INSTRUMENT: &str = "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n<RenoiseInstrument doc_version=\"33\">\n  <Name>Lead</Name>\n  <GlobalProperties>\n    <Macro0>\n      <Value>50</Value>\n      <Visualization>Device only</Visualization>\n      <Name>Tone</Name>\n      <Mappings>\n        <Mapping>\n          <DestChainType>ModulationSet</DestChainType>\n          <DestChainIndex>0</DestChainIndex>\n          <DestDeviceIndex>1</DestDeviceIndex>\n          <DestParameterIndex>1</DestParameterIndex>\n          <Min>0.0</Min>\n          <Max>0.396850258</Max>\n          <Scaling>Linear</Scaling>\n        </Mapping>\n      </Mappings>\n    </Macro0>\n    <Macro1>\n      <Value>50</Value>\n      <Visualization>Device only</Visualization>\n      <Name>Macro 2</Name>\n    </Macro1>\n  </GlobalProperties>\n</RenoiseInstrument>\n";

fn export() -> Value {
    json!({
        "label": "Lead", "print": "p1",
        "macros": [
            {"name": "Tone", "mappings": [{"param": "Attack", "min": 0.0, "max": 0.396850258, "scaling": "linear"}]},
            {"name": "Macro 2", "mappings": []},
        ],
        "chains": {
            "modulation": [{"name": "Volume", "devices": [
                {"name": "AHDSR", "params": ["Attack", "Hold", "Decay", "Sustain", "Release"]},
                {"name": "LFO", "params": ["Amplitude", "Phase", "Frequency"]},
            ]}],
            "fx": [{"name": "FX", "devices": [{"name": "Mixer", "params": ["Panning", "Volume", "Width"]}, {"name": "Gainer", "params": ["Gain", "Panning"]}]}],
        },
    })
}

fn chains() -> String {
    chains_print(&export()["chains"])
}

#[test]
fn a_mapping_is_added_where_renoise_writes_it_and_kumis_undo_puts_the_file_back_byte_for_byte() {
    let input = json!({"macro": 2, "add": [{"fx": 1, "device": 2, "param": "gain", "max": 0.5, "scaling": "exp-fast"}]});
    let mapped = edit(&input, INSTRUMENT, &export()).unwrap();
    let mapping = "      <Mappings>\n        <Mapping>\n          <DestChainType>SampleDSP</DestChainType>\n          <DestChainIndex>0</DestChainIndex>\n          <DestDeviceIndex>1</DestDeviceIndex>\n          <DestParameterIndex>1</DestParameterIndex>\n          <Min>0.0</Min>\n          <Max>0.5</Max>\n          <Scaling>Exp Fast</Scaling>\n        </Mapping>\n      </Mappings>\n";
    let expected = INSTRUMENT.replace("<Name>Macro 2</Name>\n", &format!("<Name>Macro 2</Name>\n{mapping}"));
    assert_eq!(mapped.xml, expected);
    assert_eq!(mapped.title, "Lead macro 2: mapped to Gain (Gainer in sample FX chain 1)");
    assert!(mapped.prior.is_empty());
    assert_eq!(mapped.applied.len(), 1);
    // Kumi's undo, on the file as Renoise saves it again.
    let back = undo(2, &mapped.prior, &mapped.applied, &chains(), &mapped.xml, &export()).unwrap();
    assert_eq!(back.as_deref(), Some(INSTRUMENT));
    // Back already: nothing to write.
    assert_eq!(undo(2, &mapped.prior, &mapped.applied, &chains(), INSTRUMENT, &export()).unwrap(), None);
}

#[test]
fn a_mapping_removed_and_one_added_in_its_place_come_back_as_renoise_wrote_them() {
    // The modulation set's device k is the API's device k; its parameter k the API's parameter k.
    let input = json!({"macro": 1, "remove": [1], "add": [{"modulation": 1, "device": 2, "param": 3}]});
    let mapped = edit(&input, INSTRUMENT, &export()).unwrap();
    assert!(
        mapped.xml.contains("<DestDeviceIndex>2</DestDeviceIndex>\n          <DestParameterIndex>3</DestParameterIndex>"),
        "{}",
        mapped.xml
    );
    assert!(!mapped.xml.contains("0.396850258"));
    assert_eq!(mapped.title, "Lead macro 1: unmapped Attack (AHDSR in modulation set 1); mapped to Frequency (LFO in modulation set 1)");
    assert_eq!(mapped.words[0]["param"], "Attack");
    assert_eq!(mapped.words[0]["modulation"], 1);
    assert_eq!(mapped.words[0]["deviceName"], "AHDSR");
    let back = undo(1, &mapped.prior, &mapped.applied, &chains(), &mapped.xml, &export()).unwrap();
    assert_eq!(back.as_deref(), Some(INSTRUMENT), "Renoise's long number as it wrote it");
    // Every mapping removed: no <Mappings> left, as Renoise writes a macro without.
    let none = edit(&json!({"macro": 1, "remove": [1]}), INSTRUMENT, &export()).unwrap();
    assert!(!none.xml.contains("<Mappings>"));
    assert_eq!(undo(1, &none.prior, &none.applied, &chains(), &none.xml, &export()).unwrap().as_deref(), Some(INSTRUMENT));
}

#[test]
fn a_ranges_set_with_an_add_keeps_the_other_mappings_text() {
    let input = json!({"macro": 1, "set": [{"mapping": 1, "min": 0.25}], "add": [{"fx": 1, "device": 1, "param": "Volume"}]});
    let mapped = edit(&input, INSTRUMENT, &export()).unwrap();
    assert!(mapped.xml.contains("<Min>0.25</Min>\n          <Max>0.396850258</Max>"), "{}", mapped.xml);
    assert!(mapped.xml.contains("<DestChainType>SampleDSP</DestChainType>\n          <DestChainIndex>0</DestChainIndex>\n          <DestDeviceIndex>0</DestDeviceIndex>\n          <DestParameterIndex>2</DestParameterIndex>"), "the mixer device is the file's device 0");
    assert!(
        mapped.title.starts_with("Lead macro 1: set Attack (AHDSR in modulation set 1) to 0.25–0.396850258 linear; mapped to Volume"),
        "{}",
        mapped.title
    );
}

#[test]
fn kumis_undo_is_refused_when_the_mappings_or_the_chains_they_point_into_changed() {
    let mapped = edit(&json!({"macro": 2, "add": [{"fx": 1, "device": 2, "param": 1}]}), INSTRUMENT, &export()).unwrap();
    let theirs = mapped.xml.replace("<Max>1.0</Max>", "<Max>0.75</Max>");
    assert!(
        matches!(undo(2, &mapped.prior, &mapped.applied, &chains(), &theirs, &export()), Err(Refusal::Changed(m)) if m.contains("changed in Renoise since Kumi mapped them"))
    );
    let mut moved = export();
    moved["chains"]["fx"][0]["devices"].as_array_mut().unwrap().insert(1, json!({"name": "EQ 5", "params": ["Gain 1"]}));
    assert!(
        matches!(undo(2, &mapped.prior, &mapped.applied, &chains(), &mapped.xml, &moved), Err(Refusal::Changed(m)) if m.contains("point into them by number"))
    );
}

#[test]
fn mappings_asked_of_what_isnt_there_are_refused_before_anything_is_made() {
    let refused = |input: Value| match edit(&input, INSTRUMENT, &export()) {
        Err(Refusal::Bad(m)) => m,
        other => panic!("{other:?}"),
    };
    assert!(refused(json!({"macro": 9, "add": []})).contains("1 to 8"));
    assert!(refused(json!({"macro": 2, "add": [{"fx": 2, "device": 1, "param": 1}]})).contains("has 1 sample FX chains"));
    assert!(refused(json!({"macro": 2, "add": [{"modulation": 1, "device": 3, "param": 1}]})).contains("from 1 to 2"));
    assert!(refused(json!({"macro": 2, "add": [{"modulation": 1, "device": 1, "param": "Cutoff"}]}))
        .contains("Attack, Hold, Decay, Sustain, Release"));
    assert!(refused(json!({"macro": 2, "add": [{"modulation": 1, "device": 1, "param": 6}]})).contains("from 0 (its on/off) to 5"));
    assert!(refused(json!({"macro": 2, "add": [{"device": 1, "param": 1}]})).contains("modulation"));
    assert!(refused(json!({"macro": 2, "add": [{"fx": 1, "device": 2, "param": 1, "max": 2}]})).contains("from 0 to 1"));
    assert!(refused(json!({"macro": 1, "add": [{"modulation": 1, "device": 1, "param": "Attack"}]}))
        .contains("maps Attack (AHDSR in modulation set 1) already"));
    assert!(refused(json!({"macro": 1, "remove": [1, 1]})).contains("named twice"));
    assert!(refused(json!({"macro": 1, "remove": [2]})).contains("from 1 to 1"));
    // The API's view must be the file's.
    let mut other = export();
    other["macros"][0]["mappings"][0]["param"] = "Hold".into();
    assert!(matches!(edit(&json!({"macro": 1, "remove": [1]}), INSTRUMENT, &other), Err(Refusal::Bad(m)) if m.contains("line up")));
    other["macros"][0]["mappings"] = json!([]);
    assert!(matches!(edit(&json!({"macro": 1, "remove": [1]}), INSTRUMENT, &other), Err(Refusal::Bad(m)) if m.contains("line up")));
}

#[test]
fn only_mappings_added_or_removed_are_the_hosts() {
    let renoise = daw_bridge::Daw::Renoise;
    assert!(is_host_change(renoise, KIND, &json!({"add": []})));
    assert!(is_host_change(renoise, KIND, &json!({"remove": [1]})));
    assert!(!is_host_change(renoise, KIND, &json!({"set": [{"mapping": 1}]})));
    assert!(!is_host_change(renoise, KIND, &json!({"add": null})));
    assert!(!is_host_change(daw_bridge::Daw::Reaper, KIND, &json!({"add": []})));
}

#[test]
fn an_empty_list_of_mappings_reads_the_same_whichever_way_the_tool_wrote_it() {
    // (an older tool's JSON wrote an empty list as {})
    let mut older = export();
    older["macros"][1]["mappings"] = json!({});
    let mapped = edit(&json!({"macro": 2, "add": [{"fx": 1, "device": 2, "param": "Gain"}]}), INSTRUMENT, &older).unwrap();
    assert_eq!(mapped.applied.len(), 1);
}
