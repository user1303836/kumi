//! m4-r-fxenv: the FX and envelope kinds of M4, against the fake REAPER. Each kind is applied, undone by Kumi, refused when
//! the producer changed what it touched (and put back anyway with discard), refused `gone` when the object went, a retry
//! of its undo writes nothing, and an apply is refused when what the preview fenced moved in between.

use std::cell::RefCell;

use serde_json::{json, Value};

use super::*;
use crate::reaper::fake::{self, Display, FakeAutoItem, FakeEnvelope, FakeFx, FakeItem, FakeParam, FakePoint, FakePreset, FakeTake};

#[derive(Default)]
struct Events(RefCell<Vec<(String, Value)>>);

impl Outbox for Events {
    fn event(&self, name: &str, data: Value) {
        self.0.borrow_mut().push((name.to_string(), data));
    }
    fn heartbeat(&self) {}
}

struct Lab {
    changes: Changes,
    reads: Reads,
    events: Events,
    serial: std::cell::Cell<u32>,
}

impl Lab {
    fn new() -> Lab {
        fake::lab_project();
        let lab = Lab { changes: Changes::default(), reads: Reads::default(), events: Events::default(), serial: Default::default() };
        lab.tick();
        lab
    }

    fn call(&self, method: &str, mut params: Value) -> Answer {
        let api = fake::api();
        let reaper = Reaper::new(&api);
        if params.get("epoch").is_none() {
            params["epoch"] = self.reads.epoch().into();
        }
        match method {
            "change_preview" => self.changes.preview(&reaper, &self.reads, &params),
            "change_apply" => self.changes.apply(&reaper, &self.reads, &params),
            "change_restore" => self.changes.restore(&reaper, &self.reads, &params),
            "read" => self.reads.read(&reaper, &params),
            "focus_body" => self.reads.focus_body(&reaper, &params),
            "fx_chain_save" => self.changes.fx_chain_save(&reaper, &self.reads, &params),
            "undo_step_begin" => self.changes.step_begin(&reaper, &self.reads, &params),
            "undo_step_end" => self.changes.step_end(&reaper, &self.reads, &params),
            other => panic!("{other}"),
        }
    }

    fn tick(&self) {
        let api = fake::api();
        let reaper = Reaper::new(&api);
        self.changes.tick(&reaper, &self.reads, reaper.current_project().as_ref(), &self.events);
    }

    fn desc(&self) -> String {
        self.serial.set(self.serial.get() + 1);
        format!("Kumi: change (k{:04})", self.serial.get())
    }

    fn preview(&self, kind: &str, input: Value) -> Answer {
        self.call("change_preview", json!({"kind": kind, "input": input}))
    }

    fn apply(&self, kind: &str, input: &Value, fence: &Value) -> Answer {
        let desc = self.desc();
        self.call("change_apply", json!({"kind": kind, "input": input, "fence": fence, "desc": desc}))
    }

    /// A change as Kumi's host makes it: preview, then apply with the preview's fence and a description.
    fn make(&self, kind: &str, input: Value) -> (Value, Value) {
        let preview = self.preview(kind, input.clone()).unwrap_or_else(|e| panic!("{kind} preview: {e:?}"));
        let applied = self.apply(kind, &input, &preview["fence"]).unwrap_or_else(|e| panic!("{kind} apply: {e:?}"));
        (preview, applied)
    }

    fn restore_params(&self, kind: &str, preview: &Value, applied: &Value, discard: bool) -> Value {
        let mut params = json!({"kind": kind, "target": preview["target"], "prior": preview["prior"], "applied": applied["applied"], "desc": self.desc()});
        for key in ["material", "created"] {
            if let Some(value) = applied.get(key) {
                params[key] = value.clone();
            }
        }
        if discard {
            params["discard"] = true.into();
        }
        params
    }

    fn undo(&self, kind: &str, preview: &Value, applied: &Value) -> Answer {
        let params = self.restore_params(kind, preview, applied, false);
        self.call("change_restore", params)
    }

    fn discard(&self, kind: &str, preview: &Value, applied: &Value) -> Answer {
        let params = self.restore_params(kind, preview, applied, true);
        self.call("change_restore", params)
    }

    fn read(&self, params: Value) -> Value {
        self.call("read", params).unwrap_or_else(|e| panic!("read: {e:?}"))
    }
}

fn state<T>(f: impl FnOnce(&mut fake::State) -> T) -> T {
    fake::with(f)
}

const DRUMS: &str = "r:track:{AAAA}";
const KICK: &str = "r:track:{BBBB}";

fn fx_ref(guid: &str) -> String {
    format!("r:fx:{guid}")
}

/// The three controls REAPER gives every FX after its own: bypass, wet, delta.
fn wrapper() -> Vec<FakeParam> {
    vec![
        FakeParam::steps("Bypass", 0.0, &["normal", "bypassed"]),
        FakeParam::linear("Wet", 1.0, 0.0, 100.0, 0, "%"),
        FakeParam::steps("Delta", 0.0, &["normal", "enabled"]),
    ]
}

/// A plug-in with a gain in dB, a mode and REAPER's wrapper controls.
fn plugin(guid: &str, name: &str) -> FakeFx {
    let mut fx = FakeFx::new(guid, name);
    fx.params = vec![FakeParam::linear("Gain", 0.5, -12.0, 12.0, 1, "dB"), FakeParam::steps("Mode", 0.0, &["Low", "Mid", "High"])];
    fx.params.extend(wrapper());
    fx
}

/// ReaEQ: two bands (frequency in Hz, gain in dB, bandwidth in octaves), its band configuration and a state chunk.
fn reaeq(guid: &str) -> FakeFx {
    let mut fx = FakeFx::new(guid, "VST: ReaEQ (Cockos)");
    let band = |n: usize, t: i32, i: i32| {
        let mut freq = FakeParam::linear(&format!("Freq-Band {n}"), 0.25, 20.0, 20020.0, 1, "Hz");
        freq.eq = Some((t, i, 0));
        let mut gain = FakeParam::linear(&format!("Gain-Band {n}"), 0.5, -24.0, 24.0, 1, "dB");
        gain.eq = Some((t, i, 1));
        let mut bw = FakeParam::linear(&format!("BW-Band {n}"), 0.5, 0.0, 4.0, 2, "");
        bw.eq = Some((t, i, 2));
        [freq, gain, bw]
    };
    fx.params = band(1, 1, 0).into_iter().chain(band(2, 2, 0)).collect();
    fx.params.extend(wrapper());
    for (key, value) in [("BANDTYPE0", "0"), ("BANDENABLED0", "1"), ("BANDTYPE1", "8"), ("BANDENABLED1", "1")] {
        fx.config.insert(key.into(), value.into());
    }
    fx.chunk = Some(String::new());
    fx
}

/// JS: utility/volume, whose slider runs from -150 to 150 dB.
fn js_volume(guid: &str) -> FakeFx {
    let mut fx = FakeFx::new(guid, "JS: Volume Adjustment");
    let mut adjust = FakeParam::linear("Adjustment (dB)", 0.5, -150.0, 150.0, 1, "");
    adjust.min = -150.0;
    adjust.max = 150.0;
    fx.params = vec![adjust];
    fx.params.extend(wrapper());
    fx
}

/// Kick with an FX in its main chain, one in its input chain and a take with an FX.
fn chains_lab() -> Lab {
    let lab = Lab::new();
    state(|s| {
        s.tracks[1].fx = vec![plugin("{K-MAIN}", "VST: ReaComp (Cockos)")];
        s.tracks[1].input_fx = vec![plugin("{K-IN}", "VST: ReaGate (Cockos)")];
        s.tracks[1].items[0].takes[0].fx = vec![plugin("{T-FX}", "VST: ReaDelay (Cockos)")];
        s.state_count += 1;
    });
    lab
}

const TAKE: &str = "r:take:{TAKE-K1}";

fn guids(chain: &[FakeFx]) -> Vec<String> {
    chain.iter().map(|f| f.guid.clone()).collect()
}

// ----- FX in every chain -----

#[test]
fn an_fx_is_added_to_a_tracks_input_chain_a_take_and_a_container_and_each_undo_takes_that_one_off() {
    let lab = chains_lab();
    let (preview, applied) = lab.make("fx-add", json!({"ref": KICK, "name": "ReaEQ", "input": true}));
    assert_eq!(preview["display"]["title"], "Added “ReaEQ” to “Kick”'s input FX");
    let made = applied["created"]["ref"].as_str().unwrap().trim_start_matches("r:fx:").to_string();
    assert_eq!(state(|s| guids(&s.tracks[1].input_fx)), ["{K-IN}".to_string(), made.clone()]);
    assert_eq!(applied["undo"]["entry"], true);
    lab.undo("fx-add", &preview, &applied).unwrap();
    assert_eq!(state(|s| guids(&s.tracks[1].input_fx)), ["{K-IN}"]);
    // A take, at the front; an item stands for its active take.
    let (preview, applied) = lab.make("fx-add", json!({"ref": "r:item:{ITEM-K1}", "name": "ReaEQ", "index": 0}));
    assert_eq!(preview["display"]["title"], "Added “ReaEQ” to take “kick.wav”");
    assert_eq!(state(|s| s.tracks[1].items[0].takes[0].fx.len()), 2);
    assert_eq!(state(|s| s.tracks[1].items[0].takes[0].fx[1].guid.clone()), "{T-FX}");
    lab.undo("fx-add", &preview, &applied).unwrap();
    assert_eq!(state(|s| guids(&s.tracks[1].items[0].takes[0].fx)), ["{T-FX}"]);
    // A container on Drums (ReaComp and a container with ReaDelay inside), at its second place, and in the nested one.
    let (preview, applied) = lab.make("fx-add", json!({"ref": fx_ref("{FX-C}"), "name": "ReaEQ", "index": 1}));
    assert_eq!(preview["display"]["title"], "Added “ReaEQ” to “Container” on “Drums”");
    let inside = state(|s| guids(s.tracks[0].fx[1].children.as_ref().unwrap()));
    assert_eq!(inside.len(), 3);
    assert_eq!((inside[0].as_str(), inside[2].as_str()), ("{FX-COMP}", "{FX-C2}"));
    lab.undo("fx-add", &preview, &applied).unwrap();
    assert_eq!(state(|s| guids(s.tracks[0].fx[1].children.as_ref().unwrap())), ["{FX-COMP}", "{FX-C2}"]);
    let (preview, applied) = lab.make("fx-add", json!({"ref": fx_ref("{FX-C2}"), "name": "ReaEQ"}));
    let nested = state(|s| guids(s.tracks[0].fx[1].children.as_ref().unwrap()[1].children.as_ref().unwrap()));
    assert_eq!(nested.len(), 2);
    assert_eq!(nested[0], "{FX-DLY}");
    lab.undo("fx-add", &preview, &applied).unwrap();
    // A take has no input chain; a plain FX is no container.
    assert_eq!(lab.preview("fx-add", json!({"ref": TAKE, "name": "ReaEQ", "input": true})).unwrap_err().0, "bad-params");
    assert_eq!(lab.preview("fx-add", json!({"ref": fx_ref("{FX-EQ}"), "name": "ReaEQ"})).unwrap_err().0, "bad-params");
    assert_eq!(state(|s| s.copies_into_containers), 0);
}

#[test]
fn fx_in_a_take_an_input_chain_and_a_container_are_bypassed_set_moved_and_deleted_and_put_back_where_they_were() {
    let lab = chains_lab();
    // Bypass and parameter of a take's FX and an input FX.
    let (p, a) = lab.make("fx-bypass", json!({"ref": fx_ref("{T-FX}"), "bypass": true}));
    assert_eq!(p["display"]["title"], "Bypassed “ReaDelay” on take “kick.wav”");
    assert!(!state(|s| s.tracks[1].items[0].takes[0].fx[0].enabled));
    lab.undo("fx-bypass", &p, &a).unwrap();
    assert!(state(|s| s.tracks[1].items[0].takes[0].fx[0].enabled));
    let (p, a) = lab.make("fx-param", json!({"ref": fx_ref("{K-IN}"), "param": "Gain", "value": "-6 dB"}));
    assert_eq!(p["display"]["title"], "ReaGate Gain -6.0 dB on “Kick”'s input FX");
    assert!((state(|s| s.tracks[1].input_fx[0].params[0].value) - 0.25).abs() < 0.01);
    lab.undo("fx-param", &p, &a).unwrap();
    assert_eq!(state(|s| s.tracks[1].input_fx[0].params[0].value), 0.5);
    // A container's child is moved within its container and put back.
    let (p, a) = lab.make("fx-move", json!({"ref": fx_ref("{FX-COMP}"), "index": 1}));
    assert_eq!(p["display"]["title"], "Moved “ReaComp” to place 2 in “Container” on “Drums”");
    assert_eq!(state(|s| guids(s.tracks[0].fx[1].children.as_ref().unwrap())), ["{FX-C2}", "{FX-COMP}"]);
    lab.undo("fx-move", &p, &a).unwrap();
    assert_eq!(state(|s| guids(s.tracks[0].fx[1].children.as_ref().unwrap())), ["{FX-COMP}", "{FX-C2}"]);
    // A take's FX and an input FX moved.
    lab.make("fx-add", json!({"ref": TAKE, "name": "ReaEQ"}));
    let (p, a) = lab.make("fx-move", json!({"ref": fx_ref("{T-FX}"), "index": 1}));
    assert_eq!(state(|s| s.tracks[1].items[0].takes[0].fx[1].guid.clone()), "{T-FX}");
    lab.undo("fx-move", &p, &a).unwrap();
    assert_eq!(state(|s| s.tracks[1].items[0].takes[0].fx[0].guid.clone()), "{T-FX}");
    // Deleted from each chain, and put back with the same id where it was.
    let dir = resource();
    for (guid, chain) in [("{K-IN}", "input"), ("{T-FX}", "take"), ("{FX-COMP}", "container"), ("{FX-DLY}", "nested")] {
        let (p, a) = lab.make("fx-delete", json!({"ref": fx_ref(guid)}));
        assert!(!all_guids().contains(&guid.to_string()), "{chain}: deleted");
        let restored = lab.undo("fx-delete", &p, &a).unwrap();
        assert_eq!(restored["restored"]["ref"], fx_ref(guid), "{chain}");
        assert!(all_guids().contains(&guid.to_string()), "{chain}: back");
    }
    assert_eq!(state(|s| guids(&s.tracks[1].input_fx)), ["{K-IN}"]);
    assert_eq!(state(|s| guids(s.tracks[0].fx[1].children.as_ref().unwrap())), ["{FX-COMP}", "{FX-C2}"]);
    assert_eq!(state(|s| s.tracks[0].fx[1].children.as_ref().unwrap()[1].children.as_ref().unwrap()[0].guid.clone()), "{FX-DLY}");
    assert_eq!(chain_files(&dir), 0, "no FX chain file is left behind");
    assert_eq!(state(|s| s.copies_into_containers), 0);
}

fn all_guids() -> Vec<String> {
    fn walk(chain: &[FakeFx], out: &mut Vec<String>) {
        for fx in chain {
            out.push(fx.guid.clone());
            walk(fx.children.as_deref().unwrap_or_default(), out);
        }
    }
    state(|s| {
        let mut out = Vec::new();
        for t in s.tracks.iter() {
            walk(&t.fx, &mut out);
            walk(&t.input_fx, &mut out);
            for item in &t.items {
                for take in &item.takes {
                    walk(&take.fx, &mut out);
                }
            }
        }
        out
    })
}

/// A resource folder the fake keeps FX chain files in.
fn resource() -> tempfile::TempDir {
    let dir = tempfile::tempdir().unwrap();
    state(|s| s.resource = dir.path().to_string_lossy().into_owned());
    dir
}

fn chain_files(dir: &tempfile::TempDir) -> usize {
    std::fs::read_dir(dir.path().join("FXChains"))
        .map_or(0, |d| d.filter_map(Result::ok).filter(|e| e.file_name().to_string_lossy().starts_with("kumi-")).count())
}

#[test]
fn a_deleted_container_comes_back_with_everything_inside_and_its_automation() {
    let lab = Lab::new();
    let _dir = resource();
    state(|s| s.tracks[0].envelopes.push(FakeEnvelope::for_param("Gain / ReaComp", "{FX-COMP}", 0, 3)));
    let (preview, applied) = lab.make("fx-delete", json!({"ref": fx_ref("{FX-C}")}));
    assert!(preview["display"]["note"].as_str().unwrap().contains("2 FX inside the container"), "{}", preview["display"]);
    assert!(preview["display"]["note"].as_str().unwrap().contains("as new envelopes"), "{}", preview["display"]);
    assert_eq!(state(|s| guids(&s.tracks[0].fx)), ["{FX-EQ}"]);
    assert!(state(|s| s.tracks[0].envelopes.iter().all(|e| e.fx_param.as_ref().is_none_or(|(g, _)| g != "{FX-COMP}"))));
    let restored = lab.undo("fx-delete", &preview, &applied).unwrap();
    assert!(restored["display"]["note"].as_str().unwrap().contains("new envelopes"));
    assert_eq!(state(|s| guids(&s.tracks[0].fx)), ["{FX-EQ}", "{FX-C}"]);
    assert_eq!(state(|s| guids(s.tracks[0].fx[1].children.as_ref().unwrap())), ["{FX-COMP}", "{FX-C2}"]);
    // Its child's automation came back, points and all, as a new envelope.
    let back =
        state(|s| s.tracks[0].envelopes.iter().find(|e| e.fx_param.as_ref().is_some_and(|(g, _)| g == "{FX-COMP}")).cloned()).unwrap();
    assert_eq!(fake::points_of(&back).len(), 3);
}

#[test]
fn a_deleted_fx_goes_back_after_the_fx_it_followed_wherever_that_one_is_now() {
    let lab = chains_lab();
    let _dir = resource();
    lab.make("fx-add", json!({"ref": KICK, "name": "ReaEQ"}));
    lab.make("fx-add", json!({"ref": KICK, "name": "ReaDelay"}));
    let chain = state(|s| guids(&s.tracks[1].fx));
    let (preview, applied) = lab.make("fx-delete", json!({"ref": fx_ref(&chain[1])}));
    // The producer puts two FX in front: the deleted FX's place by number would now be the wrong one.
    state(|s| {
        s.tracks[1].fx.insert(0, plugin("{P-1}", "VST: ReaVerb (Cockos)"));
        s.tracks[1].fx.insert(0, plugin("{P-2}", "VST: ReaVerb (Cockos)"));
        s.state_count += 1;
    });
    lab.undo("fx-delete", &preview, &applied).unwrap();
    assert_eq!(state(|s| guids(&s.tracks[1].fx)), ["{P-2}", "{P-1}", &chain[0], &chain[1], &chain[2]].map(String::from));
}

#[test]
fn a_take_fxs_pins_are_mapped_and_put_back() {
    let lab = chains_lab();
    let (preview, applied) = lab.make("fx-pins", json!({"ref": fx_ref("{T-FX}"), "pins": [{"pin": 1, "channels": [3]}]}));
    assert_eq!(preview["display"]["title"], "Pins of “ReaDelay” on take “kick.wav”: in 1 ← 3");
    assert_eq!(state(|s| s.tracks[1].items[0].takes[0].fx[0].pins.get(&(false, 0)).copied()), Some(4));
    // REAPER doesn't notice a pin mapping by itself: without the nudge no point is made, and REAPER's undo then takes the
    // producer's step before it (accept-reaper lab, 7.82).
    assert_eq!(applied["undo"]["entry"], true, "the pins' write keeps a point of REAPER's undo");
    let undone = lab.undo("fx-pins", &preview, &applied).unwrap();
    assert_eq!(undone["undo"]["entry"], true, "and so does their undo");
    assert_eq!(state(|s| s.tracks[1].items[0].takes[0].fx[0].pins.get(&(false, 0)).copied()), Some(1));
}

// ----- offline, wet and delta -----

#[test]
fn an_fx_taken_offline_is_put_back_online_by_undo_and_refused_when_the_producer_switched_it() {
    let lab = chains_lab();
    let input = json!({"ref": fx_ref("{K-MAIN}"), "offline": true});
    let (preview, applied) = lab.make("fx-offline", input.clone());
    assert_eq!(preview["display"]["title"], "Took “ReaComp” on “Kick” offline");
    assert!(state(|s| s.tracks[1].fx[0].offline));
    assert_eq!(applied["undo"]["entry"], true);
    // The producer put it online and offline again: the same state, so the undo goes ahead.
    lab.undo("fx-offline", &preview, &applied).unwrap();
    assert!(!state(|s| s.tracks[1].fx[0].offline));
    // A retry writes nothing.
    let again = lab.undo("fx-offline", &preview, &applied).unwrap();
    assert_eq!(again["undo"]["entry"], false);
    // Producer put it back online between the apply and the undo: back already; offline but changed by them elsewhere
    // can't be told apart, so the fence is the offline state itself.
    let (preview, applied) = lab.make("fx-offline", input.clone());
    state(|s| s.tracks[1].fx[0].offline = false);
    let undone = lab.undo("fx-offline", &preview, &applied).unwrap();
    assert!(undone["display"]["note"].as_str().unwrap_or_default().contains("back already"), "{undone}");
    // Fence: switched between the preview and the apply.
    let preview = lab.preview("fx-offline", input.clone()).unwrap();
    state(|s| s.tracks[1].fx[0].offline = true);
    assert_eq!(lab.apply("fx-offline", &input, &preview["fence"]).unwrap_err().0, "changed");
    // Gone.
    state(|s| s.tracks[1].fx[0].offline = false);
    let (preview, applied) = lab.make("fx-offline", input);
    state(|s| s.tracks[1].fx.clear());
    assert_eq!(lab.undo("fx-offline", &preview, &applied).unwrap_err().0, "gone");
}

#[test]
fn wet_and_delta_are_set_together_or_alone_and_undo_puts_back_only_what_it_touched() {
    let lab = chains_lab();
    let input = json!({"ref": fx_ref("{K-MAIN}"), "wet": 0.25, "delta": true});
    let (preview, applied) = lab.make("fx-wet", input.clone());
    assert_eq!(preview["display"]["title"], "“ReaComp” on “Kick” wet 25%, delta on");
    let (wet, delta) = state(|s| (s.tracks[1].fx[0].params[3].value, s.tracks[1].fx[0].params[4].value));
    assert_eq!((wet, delta), (0.25, 1.0));
    assert_eq!(applied["applied"]["wetDisplay"], "25 %");
    // The producer moves the wet: the undo is refused, and discard puts both back.
    state(|s| s.tracks[1].fx[0].params[3].value = 0.6);
    let (code, message) = lab.undo("fx-wet", &preview, &applied).unwrap_err();
    assert_eq!(code, "changed", "{message}");
    lab.discard("fx-wet", &preview, &applied).unwrap();
    assert_eq!(state(|s| (s.tracks[1].fx[0].params[3].value, s.tracks[1].fx[0].params[4].value)), (1.0, 0.0));
    // Delta alone leaves the wet alone.
    let (preview, applied) = lab.make("fx-wet", json!({"ref": fx_ref("{K-MAIN}"), "delta": true}));
    assert!(preview["prior"].get("wet").is_none());
    state(|s| s.tracks[1].fx[0].params[3].value = 0.7);
    lab.undo("fx-wet", &preview, &applied).unwrap();
    assert_eq!(state(|s| (s.tracks[1].fx[0].params[3].value, s.tracks[1].fx[0].params[4].value)), (0.7, 0.0));
    // Fence and bad requests.
    let preview = lab.preview("fx-wet", input.clone()).unwrap();
    state(|s| s.tracks[1].fx[0].params[4].value = 1.0);
    assert_eq!(lab.apply("fx-wet", &input, &preview["fence"]).unwrap_err().0, "changed");
    for bad in [
        json!({"ref": fx_ref("{K-MAIN}")}),
        json!({"ref": fx_ref("{K-MAIN}"), "wet": 1.5}),
        json!({"ref": fx_ref("{K-MAIN}"), "delta": "on"}),
    ] {
        assert_eq!(lab.preview("fx-wet", bad.clone()).unwrap_err().0, "bad-params", "{bad}");
    }
    // An FX REAPER gives no wet control (the fake's ReaVerb from the lab project has no parameters at all).
    assert_eq!(lab.preview("fx-wet", json!({"ref": fx_ref("{FX-VERB}"), "wet": 0.5})).unwrap_err().0, "failed");
}

// ----- parameters in units -----

#[test]
fn reaeq_takes_hz_db_and_octaves_and_a_jsfx_slider_its_own_value_and_other_plug_ins_are_told_to_use_text() {
    let lab = Lab::new();
    state(|s| {
        s.tracks[1].fx = vec![reaeq("{EQ}"), js_volume("{JS}"), plugin("{PLUG}", "VST: Pro-Q 3 (FabFilter)")];
        s.state_count += 1;
    });
    let (preview, applied) = lab.make("fx-param", json!({"ref": "r:param:{EQ}:3", "units": 1000}));
    let freq = state(|s| s.tracks[1].fx[0].params[3].value);
    assert!(((20.0 + freq * 20000.0) - 1000.0).abs() < 0.5, "{freq}");
    assert_eq!(applied["applied"]["display"], "1000.0 Hz");
    assert_eq!(preview["display"]["title"], "ReaEQ Freq-Band 2 1000.0 Hz on “Kick”");
    lab.undo("fx-param", &preview, &applied).unwrap();
    assert_eq!(state(|s| s.tracks[1].fx[0].params[3].value), 0.25);
    lab.make("fx-param", json!({"ref": fx_ref("{EQ}"), "param": "Gain-Band 1", "units": -4.5}));
    let gain = state(|s| s.tracks[1].fx[0].params[1].value);
    assert!((gain - (19.5 / 48.0)).abs() < 1e-3, "{gain}");
    // A JSFX slider in its own units: exact.
    let (preview, applied) = lab.make("fx-param", json!({"ref": "r:param:{JS}:0", "units": -6}));
    let raw = state(|s| s.tracks[1].fx[1].params[0].value * 300.0 - 150.0);
    assert!((raw + 6.0).abs() < 1e-9, "{raw}");
    lab.undo("fx-param", &preview, &applied).unwrap();
    assert_eq!(state(|s| s.tracks[1].fx[1].params[0].value), 0.5);
    for (input, code) in [
        (json!({"ref": "r:param:{JS}:0", "units": 200}), "failed"),
        (json!({"ref": "r:param:{EQ}:3", "units": 99999}), "failed"),
        (json!({"ref": "r:param:{PLUG}:0", "units": 3}), "bad-params"),
        (json!({"ref": "r:param:{JS}:0", "units": "-6"}), "bad-params"),
        (json!({"ref": "r:param:{JS}:0", "units": 1, "normalized": 0.5}), "bad-params"),
    ] {
        assert_eq!(lab.preview("fx-param", input.clone()).unwrap_err().0, code, "{input}");
    }
}

#[test]
fn a_reaeq_bands_type_and_switch_change_and_undo_puts_them_back() {
    let lab = Lab::new();
    state(|s| {
        s.tracks[1].fx = vec![reaeq("{EQ}"), plugin("{PLUG}", "VST: ReaComp (Cockos)")];
        s.state_count += 1;
    });
    let input = json!({"ref": fx_ref("{EQ}"), "band": 1, "type": "notch", "enabled": false});
    let (preview, applied) = lab.make("fx-eq-band", input.clone());
    assert_eq!(preview["display"]["title"], "“ReaEQ” on “Kick” band 2 a notch, off");
    assert_eq!(
        state(|s| (s.tracks[1].fx[0].config["BANDTYPE1"].clone(), s.tracks[1].fx[0].config["BANDENABLED1"].clone())),
        ("6".into(), "0".into())
    );
    assert_eq!(applied["undo"]["entry"], false, "REAPER's undo neither notices nor keeps a band's settings: no point is forced");
    assert!(preview["display"]["note"].as_str().unwrap().contains("REAPER's undo doesn't take"));
    state(|s| s.tracks[1].fx[0].config.insert("BANDTYPE1".into(), "3".into()));
    assert_eq!(lab.undo("fx-eq-band", &preview, &applied).unwrap_err().0, "changed");
    lab.discard("fx-eq-band", &preview, &applied).unwrap();
    assert_eq!(
        state(|s| (s.tracks[1].fx[0].config["BANDTYPE1"].clone(), s.tracks[1].fx[0].config["BANDENABLED1"].clone())),
        ("8".into(), "1".into())
    );
    let preview = lab.preview("fx-eq-band", input.clone()).unwrap();
    state(|s| s.tracks[1].fx[0].config.insert("BANDENABLED1".into(), "0".into()));
    assert_eq!(lab.apply("fx-eq-band", &input, &preview["fence"]).unwrap_err().0, "changed");
    for (input, code) in [
        (json!({"ref": fx_ref("{PLUG}"), "band": 0, "type": "notch"}), "bad-params"),
        (json!({"ref": fx_ref("{EQ}"), "band": 2, "type": "notch"}), "bad-params"),
        (json!({"ref": fx_ref("{EQ}"), "band": 0, "type": "wobble"}), "bad-params"),
        (json!({"ref": fx_ref("{EQ}"), "band": 0}), "bad-params"),
    ] {
        assert_eq!(lab.preview("fx-eq-band", input.clone()).unwrap_err().0, code, "{input}");
    }
}

// ----- presets and state -----

fn with_presets(mut fx: FakeFx, chunk: bool) -> FakeFx {
    fx.presets = vec![
        FakePreset { name: "Soft".into(), values: vec![0.3, 0.0, 0.0, 1.0, 0.0] },
        FakePreset { name: "Hard".into(), values: vec![0.9, 1.0, 0.0, 1.0, 0.0] },
        FakePreset { name: "Wide".into(), values: vec![0.6, 0.5, 0.0, 1.0, 0.0] },
    ];
    if chunk {
        fx.chunk = Some(String::new());
    }
    fx
}

fn values(guid: &str) -> Vec<f64> {
    state(|s| s.tracks[1].fx.iter().find(|f| f.guid == guid).unwrap().params.iter().map(|p| p.value).collect())
}

fn preset_name(guid: &str) -> String {
    state(|s| s.tracks[1].fx.iter().find(|f| f.guid == guid).unwrap().preset.clone())
}

#[test]
fn a_preset_is_loaded_by_name_place_or_step_and_undo_puts_back_the_preset_and_every_value() {
    let lab = Lab::new();
    state(|s| {
        s.tracks[1].fx =
            vec![with_presets(plugin("{P}", "VST: ReaComp (Cockos)"), false), with_presets(plugin("{C}", "VST: ReaEQ (Cockos)"), true)];
        s.tracks[1].fx[0].preset = "Soft".into();
        for (p, v) in s.tracks[1].fx[0].params.iter_mut().zip([0.3, 0.0, 0.0, 1.0, 0.0]) {
            p.value = v;
        }
        s.state_count += 1;
    });
    for guid in ["{P}", "{C}"] {
        let before = (preset_name(guid), values(guid));
        let (preview, applied) = lab.make("fx-preset", json!({"ref": fx_ref(guid), "preset": "Hard"}));
        assert_eq!((preset_name(guid), values(guid)[0]), ("Hard".into(), 0.9), "{guid}");
        assert!(preview["display"]["title"].as_str().unwrap().ends_with("preset “Hard”"), "{}", preview["display"]["title"]);
        lab.undo("fx-preset", &preview, &applied).unwrap();
        assert_eq!((preset_name(guid), values(guid)), before, "{guid}: the preset and every value back");
        // By place and by step.
        let (preview, applied) = lab.make("fx-preset", json!({"ref": fx_ref(guid), "index": 2}));
        assert_eq!(preset_name(guid), "Wide");
        assert!(applied["display"]["title"].as_str().unwrap().ends_with("preset “Wide”"));
        lab.undo("fx-preset", &preview, &applied).unwrap();
        assert_eq!((preset_name(guid), values(guid)), before);
        let (preview, applied) = lab.make("fx-preset", json!({"ref": fx_ref(guid), "step": 1}));
        lab.undo("fx-preset", &preview, &applied).unwrap();
        assert_eq!((preset_name(guid), values(guid)), before);
    }
    // A preset the producer had tweaked: its values come back exactly; a plug-in with a state chunk loses the name.
    state(|s| {
        s.tracks[1].fx[1].preset = "Soft".into();
        for (p, v) in s.tracks[1].fx[1].params.iter_mut().zip([0.35, 0.0, 0.0, 1.0, 0.0]) {
            p.value = v;
        }
    });
    let tweaked = values("{C}");
    let (preview, applied) = lab.make("fx-preset", json!({"ref": fx_ref("{C}"), "preset": "Hard"}));
    let undone = lab.undo("fx-preset", &preview, &applied).unwrap();
    assert_eq!(values("{C}"), tweaked);
    assert!(undone["display"]["note"].as_str().unwrap().contains("no preset name"), "{}", undone["display"]);
    // Changed since: refused; discard puts it back.
    let (preview, applied) = lab.make("fx-preset", json!({"ref": fx_ref("{P}"), "preset": "Hard"}));
    state(|s| s.tracks[1].fx[0].params[0].value = 0.1);
    assert_eq!(lab.undo("fx-preset", &preview, &applied).unwrap_err().0, "changed");
    lab.discard("fx-preset", &preview, &applied).unwrap();
    assert_eq!(preset_name("{P}"), "Soft");
    // No such preset: nothing changes. The fence: tweaked between the preview and the apply.
    let before = values("{P}");
    let input = json!({"ref": fx_ref("{P}"), "preset": "Nope"});
    let preview = lab.preview("fx-preset", input.clone()).unwrap();
    assert_eq!(lab.apply("fx-preset", &input, &preview["fence"]).unwrap_err().0, "failed");
    assert_eq!(values("{P}"), before);
    let input = json!({"ref": fx_ref("{P}"), "preset": "Wide"});
    let preview = lab.preview("fx-preset", input.clone()).unwrap();
    state(|s| s.tracks[1].fx[0].params[1].value = 0.9);
    assert_eq!(lab.apply("fx-preset", &input, &preview["fence"]).unwrap_err().0, "changed");
    for bad in [json!({"ref": fx_ref("{P}")}), json!({"ref": fx_ref("{P}"), "index": 9}), json!({"ref": fx_ref("{P}"), "step": 2})] {
        assert_eq!(lab.preview("fx-preset", bad.clone()).unwrap_err().0, "bad-params", "{bad}");
    }
}

#[test]
fn a_plug_ins_state_chunk_is_written_and_put_back_and_one_without_a_chunk_is_told_so() {
    let lab = Lab::new();
    state(|s| {
        s.tracks[1].fx = vec![reaeq("{EQ}"), plugin("{PLUG}", "VST: ReaComp (Cockos)")];
        s.state_count += 1;
    });
    let read = lab.read(json!({"ref": fx_ref("{EQ}")}));
    let state_text = read["items"][0]["state"]["chunk"].as_str().unwrap().to_string();
    let wanted = fake::chunk_of(&[0.1, 0.2, 0.3, 0.4, 0.5, 0.6, 1.0, 1.0, 0.0]);
    let input = json!({"ref": fx_ref("{EQ}"), "state": wanted});
    assert!(lab.read(json!({"ref": fx_ref("{PLUG}")}))["items"][0].get("state").is_none());
    let (preview, applied) = lab.make("fx-state", input.clone());
    assert_eq!(values("{EQ}")[..3], [0.1, 0.2, 0.3]);
    assert_eq!(applied["undo"]["entry"], true, "a state chunk makes no point by itself: the nudge makes one");
    lab.undo("fx-state", &preview, &applied).unwrap();
    assert_eq!(lab.read(json!({"ref": fx_ref("{EQ}")}))["items"][0]["state"]["chunk"], state_text);
    // Changed since; the fence; a plug-in with no chunk; not base64.
    let (preview, applied) = lab.make("fx-state", input.clone());
    state(|s| s.tracks[1].fx[0].params[0].value = 0.9);
    assert_eq!(lab.undo("fx-state", &preview, &applied).unwrap_err().0, "changed");
    lab.discard("fx-state", &preview, &applied).unwrap();
    let preview = lab.preview("fx-state", input.clone()).unwrap();
    state(|s| s.tracks[1].fx[0].params[2].value = 0.9);
    assert_eq!(lab.apply("fx-state", &input, &preview["fence"]).unwrap_err().0, "changed");
    assert_eq!(lab.preview("fx-state", json!({"ref": fx_ref("{PLUG}"), "state": "AAAA"})).unwrap_err().0, "bad-params");
    assert_eq!(lab.preview("fx-state", json!({"ref": fx_ref("{EQ}"), "state": "not base64!"})).unwrap_err().0, "bad-params");
}

// ----- FX chain files and copies -----

const PRESET_CLASS: &str = "00112233445566778899AABBCCDDEEFF";

fn vst3_file(dir: &tempfile::TempDir, class_id: &str) -> std::path::PathBuf {
    let component = fake::chunk_of(&[0.8, 0.5, 0.0, 1.0, 0.0]);
    let mut bytes = b"VST3".to_vec();
    bytes.extend(1u32.to_le_bytes());
    bytes.extend(class_id.as_bytes());
    bytes.extend((48u64 + component.len() as u64).to_le_bytes());
    bytes.extend(component.as_bytes());
    bytes.extend(b"List");
    bytes.extend(1u32.to_le_bytes());
    bytes.extend(b"Comp");
    bytes.extend(48u64.to_le_bytes());
    bytes.extend((component.len() as u64).to_le_bytes());
    let path = dir.path().join("Created.vstpreset");
    std::fs::write(&path, bytes).unwrap();
    path
}

fn file_input(path: &std::path::Path) -> Value {
    use base64::Engine;
    json!({"ref":fx_ref("{FILE}"),"presetName":path.file_name().unwrap().to_string_lossy(),
        "vst3Data":base64::engine::general_purpose::STANDARD.encode(std::fs::read(path).unwrap())})
}

fn file_plugin() -> FakeFx {
    let mut fx = with_presets(plugin("{FILE}", "VST3: Synthetic (Kumi test)"), true);
    fx.config.insert("fx_ident".into(), format!("Synthetic.vst3{{{PRESET_CLASS}"));
    fx
}

#[test]
fn vst3_preset_files_load_checked_copies_and_undo_accepts_normalization_but_refuses_later_edits() {
    let lab = Lab::new();
    let dir = tempfile::tempdir().unwrap();
    let path = vst3_file(&dir, PRESET_CLASS);
    state(|s| {
        s.tracks[1].fx = vec![file_plugin()];
        s.tracks[1].fx[0].config.insert("fake.swapSource".into(), path.to_string_lossy().into());
        s.state_count += 1;
    });
    let before = values("{FILE}");
    let (preview, applied) = lab.make("fx-preset", file_input(&path));
    assert_eq!(values("{FILE}")[0], 0.8, "the bytes validated before the source swap were loaded");
    assert!(applied["display"]["title"].as_str().unwrap().ends_with("preset “Created.vstpreset”"));
    assert_eq!(applied["applied"]["preset"], "Created.vstpreset");
    let row = lab.call("read", json!({"ref":fx_ref("{FILE}")})).unwrap();
    assert!(!row.to_string().contains("kumi-vst3-"), "{row}");
    assert!(row.to_string().contains("Created.vstpreset"), "{row}");
    let body = lab.call("focus_body", json!({"kind":"fx","ref":"r:track:{BBBB}"})).unwrap();
    assert!(!body.to_string().contains("kumi-vst3-"), "{body}");
    assert!(body.to_string().contains("Created.vstpreset"), "{body}");
    let staged = state(|s| s.tracks[1].fx[0].config["fake.loadedFile"].clone());
    assert_ne!(std::path::Path::new(&staged), path);
    assert!(!std::path::Path::new(&staged).exists(), "the owned copy is removed after the synchronous native load");
    state(|s| s.tracks[1].fx[0].config.insert("fake.stateExtra".into(), "+producerEdit".into()));
    assert_eq!(lab.undo("fx-preset", &preview, &applied).unwrap_err().0, "changed");
    state(|s| {
        s.tracks[1].fx[0].config.remove("fake.stateExtra");
        s.tracks[1].fx[0].config.insert("fake.normalizeState".into(), "1".into());
    });
    let undone = lab.undo("fx-preset", &preview, &applied).unwrap();
    assert_eq!(values("{FILE}"), before);
    assert!(undone["display"]["note"].as_str().unwrap().contains("normalized"));
}

#[test]
fn vst3_preset_files_refuse_wrong_classes_changed_sources_and_roll_back_partial_rejection() {
    let lab = Lab::new();
    let dir = tempfile::tempdir().unwrap();
    state(|s| {
        s.tracks[1].fx = vec![file_plugin()];
        s.state_count += 1;
    });
    let wrong = vst3_file(&dir, "FFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFF");
    assert_eq!(lab.preview("fx-preset", file_input(&wrong)).unwrap_err().0, "bad-params");
    let path = vst3_file(&dir, PRESET_CLASS);
    let mut input = file_input(&path);
    let preview = lab.preview("fx-preset", input.clone()).unwrap();
    let mut changed = std::fs::read(&path).unwrap();
    changed[48] = b'4';
    std::fs::write(&path, changed).unwrap();
    input = file_input(&path);
    assert_eq!(lab.apply("fx-preset", &input, &preview["fence"]).unwrap_err().0, "changed");
    vst3_file(&dir, PRESET_CLASS);
    input = file_input(&path);
    let before = values("{FILE}");
    state(|s| s.tracks[1].fx[0].config.insert("fake.refusePresetFile".into(), "1".into()));
    let preview = lab.preview("fx-preset", input.clone()).unwrap();
    assert_eq!(lab.apply("fx-preset", &input, &preview["fence"]).unwrap_err().0, "failed");
    assert_eq!(values("{FILE}"), before, "a partial native load is completely restored");
}

#[test]
fn a_failed_preset_load_keeps_native_undo_when_rollback_is_refused_ignored_or_normalized() {
    for flag in ["fake.refuseState", "fake.ignoreState", "fake.normalizeState"] {
        for in_plan in [false, true] {
            let lab = Lab::new();
            let dir = tempfile::tempdir().unwrap();
            let path = vst3_file(&dir, PRESET_CLASS);
            state(|s| {
                let mut fx = file_plugin();
                fx.config.insert("fake.refusePresetFile".into(), "1".into());
                fx.config.insert(flag.into(), "1".into());
                s.tracks[1].fx = vec![fx];
                s.state_count += 1;
            });
            if in_plan {
                lab.call("undo_step_begin", json!({"label":"Kumi: attempted preset"})).unwrap();
            }
            let input = file_input(&path);
            let preview = lab.preview("fx-preset", input.clone()).unwrap();
            let before = state(|s| s.undo_entries);
            let error = lab.apply("fx-preset", &input, &preview["fence"]).unwrap_err();
            assert_eq!(error.0, "uncertain", "{flag}/{in_plan}: {error:?}");
            assert_eq!(state(|s| s.undo_depth), 0, "no open block remains");
            assert_eq!(state(|s| s.undo_end_flags), -1, "the partial write remains in native undo");
            assert!(state(|s| s.undo_entries) > before);
        }
    }
}

#[test]
fn vst3_preset_undo_requires_acceptance_and_a_change_back_in_hidden_state() {
    for flag in ["fake.refuseState", "fake.ignoreState"] {
        let lab = Lab::new();
        let dir = tempfile::tempdir().unwrap();
        let path = vst3_file(&dir, PRESET_CLASS);
        state(|s| {
            let mut fx = file_plugin();
            for (p, v) in fx.params.iter_mut().zip([0.8, 0.5, 0.0, 1.0, 0.0]) {
                p.value = v;
            }
            fx.config.insert("fake.fileExtra".into(), "+appliedOpaque".into());
            s.tracks[1].fx = vec![fx];
            s.state_count += 1;
        });
        let (preview, applied) = lab.make("fx-preset", file_input(&path));
        state(|s| s.tracks[1].fx[0].config.insert(flag.into(), "1".into()));
        assert_eq!(lab.undo("fx-preset", &preview, &applied).unwrap_err().0, "failed", "{flag}");
        assert_eq!(state(|s| s.tracks[1].fx[0].config["fake.stateExtra"].clone()), "+appliedOpaque");
    }
}

fn write_chain(dir: &tempfile::TempDir, name: &str, guids: &[&str]) {
    // Entries as the fake writes them (REAPER's lines: BYPASS … FXID … WAK).
    let mut text = String::new();
    for guid in guids {
        text += &format!(
            "BYPASS 0 0 0\n<VST \"VST: ReaEQ (Cockos)\" fake.vst 0 \"\" 0\nPARAM 0.75 Gain\n>\nFLOATPOS 0 0 0 0\nFXID {guid}\nWAK 0 0\n"
        );
    }
    let folder = dir.path().join("FXChains");
    std::fs::create_dir_all(folder.join("Mine")).unwrap();
    std::fs::write(folder.join(name), text).unwrap();
}

#[test]
fn an_fx_chain_file_is_loaded_into_a_chain_with_ids_of_its_own_and_undo_takes_its_fx_off() {
    let lab = chains_lab();
    let dir = resource();
    write_chain(&dir, "Mine/Vocal.RfxChain", &["{FX-EQ}", "{UNSEEN}"]);
    let (preview, applied) = lab.make("fx-chain-load", json!({"ref": KICK, "file": "Mine/Vocal", "index": 0}));
    assert_eq!(preview["display"]["title"], "Loaded the FX chain “Vocal” into “Kick”");
    let chain = state(|s| guids(&s.tracks[1].fx));
    assert_eq!(chain.len(), 3);
    assert_eq!(chain[2], "{K-MAIN}");
    // The file's ids were left out: REAPER gave both FX ids of their own (one of them was Drums' ReaEQ's).
    assert!(!chain.contains(&"{FX-EQ}".to_string()) && !chain.contains(&"{UNSEEN}".to_string()), "{chain:?}");
    assert_eq!(applied["created"]["fxRefs"].as_array().unwrap().len(), 2);
    assert_eq!(state(|s| s.tracks[1].fx[0].params[0].value), 0.75);
    lab.undo("fx-chain-load", &preview, &applied).unwrap();
    assert_eq!(state(|s| guids(&s.tracks[1].fx)), ["{K-MAIN}"]);
    assert_eq!(lab.undo("fx-chain-load", &preview, &applied).unwrap_err().0, "gone");
    // Into a take and into a container; a producer's change to one of the FX keeps them (discard takes them off).
    let (preview, applied) = lab.make("fx-chain-load", json!({"ref": TAKE, "file": "Mine/Vocal.RfxChain"}));
    assert_eq!(state(|s| s.tracks[1].items[0].takes[0].fx.len()), 3);
    state(|s| s.tracks[1].items[0].takes[0].fx[1].params[0].value = 0.1);
    assert_eq!(lab.undo("fx-chain-load", &preview, &applied).unwrap_err().0, "changed");
    lab.discard("fx-chain-load", &preview, &applied).unwrap();
    assert_eq!(state(|s| guids(&s.tracks[1].items[0].takes[0].fx)), ["{T-FX}"]);
    let (preview, applied) = lab.make("fx-chain-load", json!({"ref": fx_ref("{FX-C}"), "file": "Mine/Vocal"}));
    assert_eq!(state(|s| s.tracks[0].fx[1].children.as_ref().unwrap().len()), 4);
    lab.undo("fx-chain-load", &preview, &applied).unwrap();
    // The chain moved between the preview and the apply; a file that isn't there; a name climbing out of the folder.
    let input = json!({"ref": KICK, "file": "Mine/Vocal"});
    let preview = lab.preview("fx-chain-load", input.clone()).unwrap();
    state(|s| s.tracks[1].fx.push(plugin("{P}", "VST: ReaVerb (Cockos)")));
    assert_eq!(lab.apply("fx-chain-load", &input, &preview["fence"]).unwrap_err().0, "changed");
    // The file rewritten between the preview and the apply: it isn't what the preview said it loads.
    let preview = lab.preview("fx-chain-load", input.clone()).unwrap();
    write_chain(&dir, "Mine/Vocal.RfxChain", &["{FX-EQ}"]);
    let (code, message) = lab.apply("fx-chain-load", &input, &preview["fence"]).unwrap_err();
    assert!(code == "changed" && message.contains("file"), "{message}");
    assert_eq!(lab.preview("fx-chain-load", json!({"ref": KICK, "file": "Nope"})).unwrap_err().0, "failed");
    assert_eq!(lab.preview("fx-chain-load", json!({"ref": KICK, "file": "../Vocal"})).unwrap_err().0, "bad-params");
    assert_eq!(chain_files(&dir), 0);
    assert_eq!(state(|s| s.copies_into_containers), 0);
}

#[test]
fn an_fx_is_copied_to_another_track_a_take_and_a_container_and_undo_takes_the_copy_off() {
    let lab = chains_lab();
    let _dir = resource();
    state(|s| s.tracks[1].fx[0].params[0].value = 0.8);
    let (preview, applied) = lab.make("fx-copy", json!({"ref": fx_ref("{K-MAIN}"), "toRef": "r:track:{DDDD}", "index": 0}));
    assert_eq!(preview["display"]["title"], "Copied “ReaComp” on “Kick” to “Reverb”");
    let copy = state(|s| s.tracks[3].fx[0].clone());
    assert!(copy.guid != "{K-MAIN}" && copy.params[0].value == 0.8, "a copy of its own, with the state");
    lab.undo("fx-copy", &preview, &applied).unwrap();
    assert_eq!(state(|s| guids(&s.tracks[3].fx)), ["{FX-VERB}"]);
    let (preview, applied) = lab.make("fx-copy", json!({"ref": fx_ref("{K-MAIN}"), "toRef": TAKE}));
    assert_eq!(state(|s| s.tracks[1].items[0].takes[0].fx.len()), 2);
    lab.undo("fx-copy", &preview, &applied).unwrap();
    // Into a container: never by REAPER's copy, which hangs it.
    let (preview, applied) = lab.make("fx-copy", json!({"ref": fx_ref("{K-MAIN}"), "toRef": fx_ref("{FX-C}"), "index": 0}));
    let inside = state(|s| s.tracks[0].fx[1].children.as_ref().unwrap()[0].clone());
    assert!(inside.guid != "{K-MAIN}" && inside.params[0].value == 0.8);
    assert_eq!(state(|s| s.copies_into_containers), 0);
    lab.undo("fx-copy", &preview, &applied).unwrap();
    assert_eq!(state(|s| guids(s.tracks[0].fx[1].children.as_ref().unwrap())), ["{FX-COMP}", "{FX-C2}"]);
    // Refused: into itself; the source changed between the preview and the apply.
    assert_eq!(lab.preview("fx-copy", json!({"ref": fx_ref("{FX-C}"), "toRef": fx_ref("{FX-C2}")})).unwrap_err().0, "bad-params");
    let input = json!({"ref": fx_ref("{K-MAIN}"), "toRef": "r:track:{DDDD}"});
    let preview = lab.preview("fx-copy", input.clone()).unwrap();
    state(|s| s.tracks[1].fx[0].params[0].value = 0.1);
    assert_eq!(lab.apply("fx-copy", &input, &preview["fence"]).unwrap_err().0, "changed");
}

#[test]
fn deleting_an_fx_says_the_links_and_macros_that_follow_it_and_its_undo_makes_them_again() {
    let lab = chains_lab();
    let _dir = resource();
    // Kick: ReaComp, then a ReaEQ whose first parameter follows the ReaComp's second at half.
    state(|s| {
        let mut eq = plugin("{K-EQ}", "VST: ReaEQ (Cockos)");
        for (key, value) in
            [("mod.active", "1"), ("plink.active", "1"), ("plink.effect", "0"), ("plink.param", "1"), ("plink.scale", "0.5")]
        {
            eq.config.insert(format!("param.0.{key}"), value.into());
        }
        s.tracks[1].fx.push(eq);
    });
    let link =
        |key: &str| state(|s| s.tracks[1].fx.iter().find(|f| f.guid == "{K-EQ}").unwrap().config.get(&format!("param.0.{key}")).cloned());
    let (preview, applied) = lab.make("fx-delete", json!({"ref": fx_ref("{K-MAIN}")}));
    assert!(preview["display"]["note"].as_str().unwrap().contains("1 parameter link(s) of other FX follow it"), "{}", preview["display"]);
    assert_eq!(link("plink.active").as_deref(), Some("0"), "REAPER clears the link with its source");
    lab.undo("fx-delete", &preview, &applied).unwrap();
    assert_eq!(state(|s| guids(&s.tracks[1].fx)), ["{K-MAIN}", "{K-EQ}"]);
    assert_eq!(
        (link("plink.active"), link("plink.effect"), link("plink.param"), link("plink.scale")),
        (Some("1".into()), Some("0".into()), Some("1".into()), Some("0.5".into())),
        "the link follows the ReaComp again"
    );
    // A container's macro that maps the FX deleted from it is mapped again by the undo.
    let api = fake::api();
    let reaper = Reaper::new(&api);
    let proj = reaper.current_project().unwrap().ptr;
    let container = crate::reaper::Fx { owner: crate::reaper::FxOwner::Track(reaper.track_at(proj, 0).unwrap()), index: 1 };
    reaper.fx_config(container, "container_map.add.0.1");
    let mapped = || reaper.fx_config(container, "container_map.get.0.1").is_some();
    assert!(mapped());
    let (preview, applied) = lab.make("fx-delete", json!({"ref": fx_ref("{FX-COMP}")}));
    assert!(preview["display"]["note"].as_str().unwrap().contains("1 macro(s) of its container"), "{}", preview["display"]);
    assert!(!mapped(), "the macro goes with it");
    lab.undo("fx-delete", &preview, &applied).unwrap();
    assert!(mapped(), "mapped again");
}

#[test]
fn undoing_an_fx_delete_leaves_a_link_the_producer_set_elsewhere_since() {
    let lab = chains_lab();
    let _dir = resource();
    // Kick: ReaComp, a ReaEQ whose first parameter follows the ReaComp's second, and a ReaVerb.
    state(|s| {
        let mut eq = plugin("{K-EQ}", "VST: ReaEQ (Cockos)");
        for (key, value) in [("mod.active", "1"), ("plink.active", "1"), ("plink.effect", "0"), ("plink.param", "1")] {
            eq.config.insert(format!("param.0.{key}"), value.into());
        }
        s.tracks[1].fx.push(eq);
        s.tracks[1].fx.push(plugin("{K-VERB}", "VST: ReaVerb (Cockos)"));
    });
    let link =
        |key: &str| state(|s| s.tracks[1].fx.iter().find(|f| f.guid == "{K-EQ}").unwrap().config.get(&format!("param.0.{key}")).cloned());
    let (preview, applied) = lab.make("fx-delete", json!({"ref": fx_ref("{K-MAIN}")}));
    // The producer links that parameter to the ReaVerb instead (second in the chain now).
    state(|s| {
        let eq = s.tracks[1].fx.iter_mut().find(|f| f.guid == "{K-EQ}").unwrap();
        eq.config.insert("param.0.plink.active".into(), "1".into());
        eq.config.insert("param.0.plink.effect".into(), "1".into());
        eq.config.insert("param.0.plink.param".into(), "3".into());
    });
    let undone = lab.undo("fx-delete", &preview, &applied).unwrap();
    assert!(undone["display"]["note"].as_str().unwrap().contains("set otherwise in REAPER since"), "{}", undone["display"]);
    // Their link stays theirs: still the ReaVerb (third now that the ReaComp is back), its fourth parameter.
    assert_eq!((link("plink.effect"), link("plink.param")), (Some("2".into()), Some("3".into())));
}

#[test]
fn a_copied_containers_undo_leaves_it_once_the_producer_changed_an_fx_inside_it() {
    let lab = chains_lab();
    let _dir = resource();
    let (preview, applied) = lab.make("fx-copy", json!({"ref": fx_ref("{FX-C}"), "toRef": "r:track:{DDDD}", "index": 0}));
    let inside = |s: &mut fake::State| s.tracks[3].fx[0].children.as_mut().unwrap().clone();
    assert_eq!(state(|s| inside(s).len()), 2, "the copy holds what the container holds");
    // A knob of the copied ReaComp inside it, then a plug-in dropped in beside it.
    state(|s| s.tracks[3].fx[0].children.as_mut().unwrap()[0].params.push(FakeParam::new("Ratio", 0.3, "3:1")));
    let (code, message) = lab.undo("fx-copy", &preview, &applied).unwrap_err();
    assert_eq!(code, "changed", "{message}");
    assert!(message.contains("an FX inside it"), "{message}");
    state(|s| {
        let children = s.tracks[3].fx[0].children.as_mut().unwrap();
        children[0].params.pop();
        children.push(plugin("{THEIRS}", "VST: ReaVerb (Cockos)"));
    });
    assert_eq!(lab.undo("fx-copy", &preview, &applied).unwrap_err().0, "changed");
    state(|s| s.tracks[3].fx[0].children.as_mut().unwrap().pop());
    lab.undo("fx-copy", &preview, &applied).unwrap();
    assert_eq!(state(|s| guids(&s.tracks[3].fx)), ["{FX-VERB}"]);
}

// ----- the guard against recording in touch, latch and write modes -----

#[test]
fn a_parameter_written_on_a_track_in_touch_mode_records_nothing_and_the_override_is_put_back() {
    let lab = chains_lab();
    state(|s| s.tracks[1].extra.insert("I_AUTOMODE".into(), 2.0));
    let envelopes = || state(|s| s.tracks[1].envelopes.len());
    let before = envelopes();
    let (preview, applied) = lab.make("fx-param", json!({"ref": fx_ref("{K-MAIN}"), "param": "Gain", "normalized": 0.9}));
    assert_eq!(envelopes(), before, "REAPER would record Kumi's write into an envelope");
    assert_eq!(state(|s| s.automation_override), -1, "the override is put back");
    lab.undo("fx-param", &preview, &applied).unwrap();
    lab.make("fx-bypass", json!({"ref": fx_ref("{K-MAIN}"), "bypass": true}));
    lab.make("fx-wet", json!({"ref": fx_ref("{K-MAIN}"), "wet": 0.5}));
    assert_eq!(envelopes(), before);
    // The wet nudge that makes REAPER keep a point of a pin map or a modulation is a write too (review R5).
    let (pins_preview, pins) = lab.make("fx-pins", json!({"ref": fx_ref("{K-MAIN}"), "pins": [{"pin": 1, "channels": [2]}]}));
    lab.undo("fx-pins", &pins_preview, &pins).unwrap();
    lab.make("fx-param-lfo", json!({"ref": "r:param:{K-MAIN}:0", "active": true}));
    assert_eq!(envelopes(), before, "the nudge recorded nothing");
    assert_eq!(state(|s| s.automation_override), -1);
    // A global override in latch mode is the producer's: it is put back as it was.
    state(|s| s.automation_override = 4);
    lab.make("fx-param", json!({"ref": fx_ref("{K-MAIN}"), "param": "Gain", "normalized": 0.2}));
    assert_eq!((envelopes(), state(|s| s.automation_override)), (before, 4));
    // The producer's own write still records, as REAPER does: the fake's recording is real.
    let api = fake::api();
    let reaper = Reaper::new(&api);
    let fx = crate::reaper::Fx {
        owner: crate::reaper::FxOwner::Track(reaper.track_at(reaper.current_project().unwrap().ptr, 1).unwrap()),
        index: 0,
    };
    reaper.fx_set_param(fx, 0, 0.4);
    assert_eq!(envelopes(), before + 1);
}

#[test]
fn every_fx_kind_is_refused_for_another_epoch_and_while_reaper_is_busy() {
    let lab = chains_lab();
    let _dir = resource();
    write_chain(&_dir, "Busy.RfxChain", &["{B1}"]);
    for (kind, input) in [
        ("fx-offline", json!({"ref": fx_ref("{K-MAIN}"), "offline": true})),
        ("fx-wet", json!({"ref": fx_ref("{K-MAIN}"), "wet": 0.5})),
        ("fx-copy", json!({"ref": fx_ref("{K-MAIN}"), "toRef": TAKE})),
        ("fx-chain-load", json!({"ref": KICK, "file": "Busy"})),
        ("fx-container-add", json!({"ref": KICK})),
        ("fx-parallel", json!({"ref": fx_ref("{K-MAIN}"), "parallel": "parallel"})),
        ("fx-param-lfo", json!({"ref": "r:param:{K-MAIN}:0", "active": true})),
        ("envelope-show", json!({"trackRef": KICK, "name": "pan"})),
        ("track-automation-mode", json!({"ref": KICK, "mode": "latch"})),
    ] {
        let (code, _) = lab.call("change_preview", json!({"kind": kind, "input": input, "epoch": "old-1"})).unwrap_err();
        assert_eq!(code, "stale", "{kind}");
        let preview = lab.preview(kind, input.clone()).unwrap_or_else(|e| panic!("{kind}: {e:?}"));
        state(|s| s.loading = true);
        assert_eq!(lab.apply(kind, &input, &preview["fence"]).unwrap_err().0, "busy", "{kind}");
        state(|s| s.loading = false);
    }
}

// ----- containers -----

fn drums_chain() -> Vec<String> {
    state(|s| guids(&s.tracks[0].fx))
}

fn container_children(guid: &str) -> Vec<String> {
    fn find(chain: &[FakeFx], guid: &str) -> Option<Vec<String>> {
        chain.iter().find_map(|f| if f.guid == guid { f.children.as_ref().map(|c| guids(c)) } else { find(f.children.as_deref()?, guid) })
    }
    state(|s| s.tracks.iter().find_map(|t| find(&t.fx, guid)).unwrap_or_default())
}

#[test]
fn a_container_is_added_named_and_taken_off_by_undo_unless_the_producer_put_fx_in_it() {
    let lab = chains_lab();
    let (preview, applied) = lab.make("fx-container-add", json!({"ref": KICK, "name": "Parallel", "index": 0}));
    assert_eq!(preview["display"]["title"], "Added the container “Parallel” to “Kick”");
    let made = applied["created"]["ref"].as_str().unwrap().trim_start_matches("r:fx:").to_string();
    let first = state(|s| s.tracks[1].fx[0].clone());
    assert_eq!((first.guid.as_str(), first.renamed.as_str(), first.children.as_ref().map(Vec::len)), (made.as_str(), "Parallel", Some(0)));
    lab.undo("fx-container-add", &preview, &applied).unwrap();
    assert_eq!(state(|s| guids(&s.tracks[1].fx)), ["{K-MAIN}"]);
    // The producer moved an FX into it: Kumi's undo keeps it.
    let (preview, applied) = lab.make("fx-container-add", json!({"ref": TAKE}));
    state(|s| {
        let fx = s.tracks[1].items[0].takes[0].fx.remove(0);
        s.tracks[1].items[0].takes[0].fx[0].children.as_mut().unwrap().push(fx);
    });
    assert_eq!(lab.undo("fx-container-add", &preview, &applied).unwrap_err().0, "changed");
}

#[test]
fn an_fx_moves_into_a_container_keeping_its_id_and_back_out_to_where_it_was() {
    let lab = Lab::new();
    let _dir = resource();
    let eq = "{FX-EQ}";
    let input = json!({"ref": fx_ref(eq), "toRef": fx_ref("{FX-C}"), "index": 1});
    let (preview, applied) = lab.make("fx-container-move", input.clone());
    assert_eq!(preview["display"]["title"], "Moved “ReaEQ” on “Drums” into “Container” on “Drums” at place 2");
    assert_eq!(drums_chain(), ["{FX-C}"]);
    assert_eq!(container_children("{FX-C}"), ["{FX-COMP}", eq, "{FX-C2}"]);
    assert_eq!(applied["applied"]["guid"], eq, "REAPER keeps its id: it was taken out first");
    assert_eq!(state(|s| s.copies_into_containers), 0);
    // Its automation went with it (as a new envelope).
    assert!(state(|s| s.tracks[0].envelopes.iter().any(|e| e.fx_param.as_ref().is_some_and(|(g, _)| g == eq))));
    lab.undo("fx-container-move", &preview, &applied).unwrap();
    assert_eq!(drums_chain(), [eq, "{FX-C}"]);
    assert_eq!(container_children("{FX-C}"), ["{FX-COMP}", "{FX-C2}"]);
    let again = lab.undo("fx-container-move", &preview, &applied).unwrap();
    assert!(again["display"]["title"].as_str().unwrap().contains("back where it was"));
    // Out of a container: REAPER's own move, the id kept; undo puts it back in at its place.
    let (preview, applied) = lab.make("fx-container-move", json!({"ref": fx_ref("{FX-DLY}"), "toRef": DRUMS, "index": 0}));
    assert_eq!(drums_chain(), ["{FX-DLY}", eq, "{FX-C}"]);
    assert!(container_children("{FX-C2}").is_empty());
    lab.undo("fx-container-move", &preview, &applied).unwrap();
    assert_eq!(container_children("{FX-C2}"), ["{FX-DLY}"]);
    assert_eq!(drums_chain(), [eq, "{FX-C}"]);
    // The container it went into was rearranged since: refused.
    let (preview, applied) = lab.make("fx-container-move", input.clone());
    state(|s| s.tracks[0].fx[0].children.as_mut().unwrap().swap(0, 2));
    assert_eq!(lab.undo("fx-container-move", &preview, &applied).unwrap_err().0, "changed");
    // Refused: between tracks, into itself, within its own chain, the FX changed between the preview and the apply.
    for (input, why) in [
        (json!({"ref": fx_ref("{FX-COMP}"), "toRef": KICK}), "another track"),
        (json!({"ref": fx_ref("{FX-C}"), "toRef": fx_ref("{FX-C2}")}), "into itself"),
        (json!({"ref": fx_ref("{FX-COMP}"), "toRef": fx_ref("{FX-C}")}), "its own chain"),
    ] {
        assert_eq!(lab.preview("fx-container-move", input).unwrap_err().0, "bad-params", "{why}");
    }
    assert_eq!(state(|s| s.copies_into_containers), 0);
}

#[test]
fn a_move_that_would_break_a_link_or_a_macro_is_refused_before_anything_is_written() {
    let lab = Lab::new();
    // ReaComp inside the container is mapped as a macro: leaving would drop the macro.
    lab.make(
        "fx-container-map",
        json!({"ref": fx_ref("{FX-C}"), "childRef": fx_ref("{FX-COMP}"), "param": 0}).clone().pipe(|i| {
            state(|s| s.tracks[0].fx[1].children.as_mut().unwrap()[0].params = vec![FakeParam::new("Thresh", 0.5, "-6 dB")]);
            i
        }),
    );
    let (code, message) = lab.preview("fx-container-move", json!({"ref": fx_ref("{FX-COMP}"), "toRef": DRUMS})).unwrap_err();
    assert!(code == "bad-params" && message.contains("macros"), "{message}");
    // Another FX's parameter links to ReaEQ: moving it into the container would break the link.
    state(|s| s.tracks[0].fx[1].config.insert("param.0.plink.active".into(), "1".into()));
    state(|s| s.tracks[0].fx[1].config.insert("param.0.plink.effect".into(), "0".into()));
    let (code, message) = lab.preview("fx-container-move", json!({"ref": fx_ref("{FX-EQ}"), "toRef": fx_ref("{FX-C2}")})).unwrap_err();
    assert!(code == "bad-params" && message.contains("linked"), "{message}");
    assert_eq!(drums_chain(), ["{FX-EQ}", "{FX-C}"]);
}

trait Pipe: Sized {
    fn pipe(self, f: impl FnOnce(Self) -> Self) -> Self {
        f(self)
    }
}
impl Pipe for Value {}

#[test]
fn an_fx_runs_in_parallel_and_back_in_series() {
    let lab = chains_lab();
    lab.make("fx-add", json!({"ref": KICK, "name": "ReaEQ"}));
    let second = state(|s| s.tracks[1].fx[1].guid.clone());
    let input = json!({"ref": fx_ref(&second), "parallel": "parallel-midi"});
    let (preview, applied) = lab.make("fx-parallel", input.clone());
    assert_eq!(preview["display"]["title"], "“ReaEQ” on “Kick” runs in parallel with the FX before it, MIDI merged");
    assert_eq!(state(|s| s.tracks[1].fx[1].parallel), 2);
    state(|s| s.tracks[1].fx[1].parallel = 1);
    assert_eq!(lab.undo("fx-parallel", &preview, &applied).unwrap_err().0, "changed");
    lab.discard("fx-parallel", &preview, &applied).unwrap();
    assert_eq!(state(|s| s.tracks[1].fx[1].parallel), 0);
    let first = lab.preview("fx-parallel", json!({"ref": fx_ref("{K-MAIN}"), "parallel": "parallel"})).unwrap();
    assert!(first["display"]["note"].as_str().unwrap().contains("first in its chain"));
    let preview = lab.preview("fx-parallel", input.clone()).unwrap();
    state(|s| s.tracks[1].fx[1].parallel = 1);
    assert_eq!(lab.apply("fx-parallel", &input, &preview["fence"]).unwrap_err().0, "changed");
    assert_eq!(lab.preview("fx-parallel", json!({"ref": fx_ref(&second), "parallel": "yes"})).unwrap_err().0, "bad-params");
}

#[test]
fn a_childs_parameter_is_mapped_as_a_macro_and_taken_off_by_its_id_whatever_other_macros_do() {
    let lab = Lab::new();
    state(|s| {
        let children = s.tracks[0].fx[1].children.as_mut().unwrap();
        children[0] = plugin("{FX-COMP}", "VST: ReaComp (Cockos)");
        s.tracks[0].fx[1].params = wrapper();
    });
    let input = json!({"ref": fx_ref("{FX-C}"), "childRef": fx_ref("{FX-COMP}"), "param": "Gain"});
    let (preview, applied) = lab.make("fx-container-map", input.clone());
    assert_eq!(preview["display"]["title"], "Mapped ReaComp Gain as a macro of “Container” on “Drums”");
    let macro_ref = applied["created"]["ref"].as_str().unwrap().to_string();
    assert_eq!(macro_ref, "r:param:{FX-C}:0", "a macro comes before the container's own parameters");
    assert_eq!(applied["undo"]["entry"], true);
    // The macro moves its parameter.
    lab.make("fx-param", json!({"ref": macro_ref, "normalized": 0.75}));
    assert_eq!(state(|s| s.tracks[0].fx[1].children.as_ref().unwrap()[0].params[0].value), 0.75);
    // Another macro mapped before Kumi's undo: Kumi's is taken off by its id, the other stays.
    lab.make("fx-container-map", json!({"ref": fx_ref("{FX-C}"), "childRef": fx_ref("{FX-COMP}"), "param": 1}));
    let mut params = lab.restore_params("fx-container-map", &preview, &applied, false);
    params["discard"] = true.into();
    let undone = lab.call("change_restore", params).unwrap();
    assert_eq!(undone["undo"]["entry"], true, "REAPER keeps no point of a macro taken off by itself: the nudge makes one");
    let names: Vec<String> = lab.read(json!({"kind": "param", "parent": fx_ref("{FX-C}")}))["items"]
        .as_array()
        .unwrap()
        .iter()
        .map(|r| r["name"].as_str().unwrap().to_string())
        .collect();
    assert_eq!(names[0], "VST: ReaComp (Cockos): Mode");
    // Mapped twice is the same macro; a grandchild is mapped at its own container.
    let (again, applied2) = lab.make("fx-container-map", json!({"ref": fx_ref("{FX-C}"), "childRef": fx_ref("{FX-COMP}"), "param": 1}));
    assert_eq!(again["display"]["unchanged"], true);
    assert_eq!(applied2["undo"]["entry"], false);
    let (code, _) =
        lab.preview("fx-container-map", json!({"ref": fx_ref("{FX-C}"), "childRef": fx_ref("{FX-DLY}"), "param": 0})).unwrap_err();
    assert_eq!(code, "bad-params");
    // The fence: the container's macros changed between the preview and the apply.
    let input = json!({"ref": fx_ref("{FX-C}"), "childRef": fx_ref("{FX-COMP}"), "param": "Gain"});
    let preview = lab.preview("fx-container-map", input.clone()).unwrap();
    lab.make("fx-container-unmap", json!({"ref": "r:param:{FX-C}:0"}));
    assert_eq!(lab.apply("fx-container-map", &input, &preview["fence"]).unwrap_err().0, "changed");
}

#[test]
fn a_macro_taken_off_is_mapped_again_by_undo() {
    let lab = Lab::new();
    state(|s| {
        s.tracks[0].fx[1].children.as_mut().unwrap()[0] = plugin("{FX-COMP}", "VST: ReaComp (Cockos)");
        s.tracks[0].fx[1].params = wrapper();
    });
    lab.make("fx-container-map", json!({"ref": fx_ref("{FX-C}"), "childRef": fx_ref("{FX-COMP}"), "param": 1}));
    let (preview, applied) = lab.make("fx-container-unmap", json!({"ref": "r:param:{FX-C}:0"}));
    assert_eq!(preview["display"]["title"], "Unmapped ReaComp Mode as a macro of “Container” on “Drums”");
    assert_eq!(applied["undo"]["entry"], true);
    assert!(fake_macros("{FX-C}").is_empty());
    let restored = lab.undo("fx-container-unmap", &preview, &applied).unwrap();
    assert_eq!(restored["restored"]["ref"], "r:param:{FX-C}:0");
    assert_eq!(fake_macros("{FX-C}").len(), 1);
    let again = lab.undo("fx-container-unmap", &preview, &applied).unwrap();
    assert!(again["display"]["title"].as_str().unwrap().contains("back already"));
    // The child left the container: gone.
    let (preview, applied) = lab.make("fx-container-unmap", json!({"ref": "r:param:{FX-C}:0"}));
    state(|s| {
        s.tracks[0].fx[1].children.as_mut().unwrap().remove(0);
    });
    assert_eq!(lab.undo("fx-container-unmap", &preview, &applied).unwrap_err().0, "gone");
    assert_eq!(lab.preview("fx-container-unmap", json!({"ref": "r:param:{FX-C}:0"})).unwrap_err().0, "bad-params", "not a macro");
}

/// A container's macros as the fake keeps them.
fn fake_macros(guid: &str) -> Vec<String> {
    let text = state(|s| s.tracks[0].fx.iter().find(|f| f.guid == guid).and_then(|f| f.config.get("cmaps").cloned()).unwrap_or_default());
    text.split(';').filter(|m| !m.is_empty()).map(str::to_string).collect()
}

// ----- parameter modulation -----

fn kick_config(key: &str) -> Option<String> {
    state(|s| s.tracks[1].fx[0].config.get(key).cloned())
}

#[test]
fn an_lfo_is_turned_on_with_the_modulation_and_undo_turns_both_off_and_says_reaper_keeps_the_settings() {
    let lab = chains_lab();
    let input = json!({"ref": "r:param:{K-MAIN}:0", "active": true, "shape": "triangle", "speed": 2, "tempoSync": false, "strength": 0.4});
    let (preview, applied) = lab.make("fx-param-lfo", input.clone());
    assert_eq!(preview["display"]["title"], "ReaComp Gain on “Kick” LFO: on, shape triangle, speed 2, strength 0.4, tempoSync false");
    assert!(preview["display"]["note"].as_str().unwrap().contains("modulation is turned on"));
    assert_eq!((kick_config("param.0.mod.active").as_deref(), kick_config("param.0.lfo.shape").as_deref()), (Some("1"), Some("4")));
    assert_eq!(applied["undo"]["entry"], true);
    let undone = lab.undo("fx-param-lfo", &preview, &applied).unwrap();
    assert_eq!((kick_config("param.0.mod.active").as_deref(), kick_config("param.0.lfo.active").as_deref()), (Some("0"), Some("0")));
    assert!(undone["display"]["note"].as_str().unwrap().contains("turned off"), "{}", undone["display"]);
    let again = lab.undo("fx-param-lfo", &preview, &applied).unwrap();
    assert_eq!(again["undo"]["entry"], false);
    // The producer changed the speed since: refused; discard puts it back.
    let (preview, applied) = lab.make("fx-param-lfo", input.clone());
    state(|s| s.tracks[1].fx[0].config.insert("param.0.lfo.speed".into(), "4".into()));
    assert_eq!(lab.undo("fx-param-lfo", &preview, &applied).unwrap_err().0, "changed");
    lab.discard("fx-param-lfo", &preview, &applied).unwrap();
    // The fence: changed between the preview and the apply.
    let preview = lab.preview("fx-param-lfo", json!({"ref": "r:param:{K-MAIN}:0", "speed": 3})).unwrap();
    state(|s| s.tracks[1].fx[0].config.insert("param.0.lfo.speed".into(), "5".into()));
    assert_eq!(lab.apply("fx-param-lfo", &json!({"ref": "r:param:{K-MAIN}:0", "speed": 3}), &preview["fence"]).unwrap_err().0, "changed");
    for bad in [
        json!({"ref": "r:param:{K-MAIN}:0"}),
        json!({"ref": "r:param:{K-MAIN}:0", "shape": "blob"}),
        json!({"ref": "r:param:{K-MAIN}:0", "speed": 50}),
    ] {
        assert_eq!(lab.preview("fx-param-lfo", bad.clone()).unwrap_err().0, "bad-params", "{bad}");
    }
}

#[test]
fn what_reaper_clamps_is_read_back_and_said() {
    let lab = chains_lab();
    let (_, applied) = lab.make(
        "fx-param-acs",
        json!({"ref": "r:param:{K-MAIN}:1", "active": true, "minDb": -60, "attack": 1000, "channel": 3, "stereo": true}),
    );
    assert_eq!(kick_config("param.1.acs.chan").as_deref(), Some("2"), "the pair 3/4 is REAPER's channel 2");
    assert_eq!(applied["applied"]["state"]["param.1.acs.dblo"], "-60");
    // A baseline on a parameter with no modulation turns the modulation on with it (REAPER keeps none otherwise).
    let (preview, applied) = lab.make("fx-param-mod", json!({"ref": "r:param:{K-MAIN}:0", "baseline": 0.3}));
    assert_eq!(kick_config("param.0.mod.active").as_deref(), Some("1"));
    // With no source on, REAPER's undo state doesn't hold it: no point (lab 7.82), and the producer is told.
    assert_eq!(applied["undo"]["entry"], false);
    assert!(preview["display"]["note"].as_str().unwrap().contains("REAPER's undo doesn't keep this"));
    lab.undo("fx-param-mod", &preview, &applied).unwrap();
    assert_eq!(kick_config("param.0.mod.active").as_deref(), Some("0"));
    // With an LFO on, a baseline is part of what REAPER keeps: the nudge makes REAPER notice it.
    lab.make("fx-param-lfo", json!({"ref": "r:param:{K-MAIN}:0", "active": true}));
    let (preview, applied) = lab.make("fx-param-mod", json!({"ref": "r:param:{K-MAIN}:0", "baseline": 0.6}));
    assert_eq!(applied["undo"]["entry"], true, "a baseline makes no point by itself: the nudge makes one");
    assert!(preview["display"].get("note").is_none_or(|note| !note.as_str().unwrap_or_default().contains("doesn't keep")));
}

#[test]
fn in_a_plan_a_write_reapers_undo_state_doesnt_hold_isnt_covered_by_the_plans_point() {
    let lab = chains_lab();
    state(|s| {
        s.tracks[1].fx.push(reaeq("{EQ}"));
        s.state_count += 1;
    });
    let step = lab.call("undo_step_begin", json!({"label": "Kumi: a plan (kcov9)"})).unwrap();
    // A baseline with no source on and a ReaEQ band's type: REAPER's undo never takes them back (accept-reaper lab: a plan
    // of either alone made no point, and REAPER's undo took the producer's step before it).
    let (_, baseline) = lab.make("fx-param-mod", json!({"ref": "r:param:{K-MAIN}:0", "baseline": 0.3}));
    assert_eq!(baseline["undo"]["covered"], false);
    let (_, band) = lab.make("fx-eq-band", json!({"ref": fx_ref("{EQ}"), "band": 1, "type": "notch"}));
    assert_eq!(band["undo"]["covered"], false);
    // An LFO turned on is in REAPER's state, and so is a baseline once a source is on: the plan's point takes them back.
    let (_, lfo) = lab.make("fx-param-lfo", json!({"ref": "r:param:{K-MAIN}:0", "active": true}));
    assert_eq!(lfo["undo"]["covered"], true);
    let (_, again) = lab.make("fx-param-mod", json!({"ref": "r:param:{K-MAIN}:0", "baseline": 0.6}));
    assert_eq!(again["undo"]["covered"], true);
    lab.call("undo_step_end", json!({"stepId": step["stepId"]})).unwrap();
}

#[test]
fn a_link_follows_another_fxs_parameter_and_its_undo_finds_that_fx_wherever_it_moved() {
    let lab = chains_lab();
    lab.make("fx-add", json!({"ref": KICK, "name": "ReaEQ"}));
    lab.make("fx-add", json!({"ref": KICK, "name": "ReaDelay"}));
    let chain = state(|s| guids(&s.tracks[1].fx));
    let input = json!({"ref": "r:param:{K-MAIN}:0", "active": true, "sourceRef": format!("r:param:{}:1", chain[2]), "scale": 0.5});
    let (preview, applied) = lab.make("fx-param-link", input);
    assert_eq!((kick_config("param.0.plink.effect").as_deref(), kick_config("param.0.plink.param").as_deref()), (Some("2"), Some("1")));
    assert_eq!(applied["applied"]["state"]["param.0.plink.effect"], json!({"fx": chain[2]}), "the source is kept by its id");
    // The producer turns the link off and back on, and moves the source FX up one place (REAPER follows it): the undo
    // writes the source's new place, not the old number.
    state(|s| {
        s.tracks[1].fx.swap(1, 2);
        s.tracks[1].fx[0].config.insert("param.0.plink.effect".into(), "1".into());
    });
    lab.undo("fx-param-link", &preview, &applied).unwrap();
    assert_eq!(kick_config("param.0.plink.active").as_deref(), Some("0"));
    // A MIDI link; one link both ways is refused; a source outside the chain is refused.
    let (preview, applied) = lab.make(
        "fx-param-link",
        json!({"ref": "r:param:{K-MAIN}:0", "active": true, "midiMessage": "cc", "midiNumber": 74, "midiChannel": 1}),
    );
    assert_eq!(
        (kick_config("param.0.plink.effect").as_deref(), kick_config("param.0.plink.midi_msg").as_deref()),
        (Some("-100"), Some("176"))
    );
    lab.undo("fx-param-link", &preview, &applied).unwrap();
    let both = json!({"ref": "r:param:{K-MAIN}:0", "sourceRef": format!("r:param:{}:1", chain[1]), "midiNumber": 1});
    assert_eq!(lab.preview("fx-param-link", both).unwrap_err().0, "bad-params");
    let elsewhere = json!({"ref": "r:param:{K-MAIN}:0", "sourceRef": "r:param:{FX-EQ}:0"});
    assert_eq!(lab.preview("fx-param-link", elsewhere).unwrap_err().0, "bad-params");
    // Not inside a container: REAPER's link addresses there aren't known.
    let inside = json!({"ref": "r:param:{FX-COMP}:0", "sourceRef": "r:param:{FX-C2}:0"});
    assert_eq!(lab.preview("fx-param-link", inside).unwrap_err().0, "bad-params");
}

#[test]
fn midi_learn_is_written_and_put_back() {
    let lab = chains_lab();
    let (preview, applied) =
        lab.make("fx-param-learn", json!({"ref": "r:param:{K-MAIN}:1", "midiStatus": 176, "midiNumber": 20, "osc": "/kumi/x"}));
    assert_eq!(
        (kick_config("param.1.learn.midi1").as_deref(), kick_config("param.1.learn.osc").as_deref()),
        (Some("176"), Some("/kumi/x"))
    );
    assert!(kick_config("param.1.mod.active").is_none(), "learn makes no modulation block");
    lab.undo("fx-param-learn", &preview, &applied).unwrap();
    assert_eq!(kick_config("param.1.learn.midi1").as_deref(), Some("0"));
}

// ----- envelopes -----

/// Kick with a volume envelope of three points (0 dB, -6 dB, 0 dB at 0, 1 and 2 s) and an FX.
fn envelope_lab() -> Lab {
    let lab = chains_lab();
    state(|s| {
        let mut volume = FakeEnvelope::own("Volume", "<VOLENV2", 0);
        volume.guid = "{ENV-K-VOL}".into();
        let at = |t: f64, db: f64| FakePoint::at(t, 10f64.powf(db / 20.0).powf(0.25) * 716.217_850_312_61);
        volume.pts = Some(vec![at(0.0, 0.0), at(1.0, -6.0), at(2.0, 0.0)]);
        s.tracks[1].envelopes = vec![volume];
        s.state_count += 1;
    });
    lab
}

const KICK_VOLUME: &str = "r:env:{BBBB}:{ENV-K-VOL}";

fn kick_points() -> Vec<FakePoint> {
    state(|s| fake::points_of(&s.tracks[1].envelopes[0]))
}

#[test]
fn a_tracks_hidden_envelope_is_shown_and_undo_puts_back_its_hidden_empty_state() {
    let lab = chains_lab();
    let count = || lab.read(json!({"kind": "envelope", "parent": KICK}))["total"].as_u64().unwrap();
    assert_eq!(count(), 0);
    let input = json!({"trackRef": KICK, "name": "pan"});
    let (preview, applied) = lab.make("envelope-show", input.clone());
    assert_eq!(preview["display"]["title"], "“Kick”'s Pan envelope shown and on");
    assert_eq!(count(), 1);
    assert_eq!(applied["undo"]["entry"], true);
    let reference = applied["applied"]["ref"].as_str().unwrap().to_string();
    assert!(reference.starts_with("r:env:{BBBB}:{"), "{reference}");
    let undone = lab.undo("envelope-show", &preview, &applied).unwrap();
    assert_eq!(undone["undo"]["entry"], true, "a state chunk written back makes no point by itself: sorting the envelope makes one");
    assert_eq!(count(), 0, "out of the track's list again");
    let again = lab.undo("envelope-show", &preview, &applied).unwrap();
    assert_eq!(again["undo"]["entry"], false);
    // The producer drew a point since: refused, discard or not (the envelope's state from before would take their point).
    let (preview, applied) = lab.make("envelope-show", input.clone());
    state(|s| s.tracks[1].envelopes.iter_mut().find(|e| e.name == "Pan").map(|e| e.pts.as_mut().unwrap().push(FakePoint::at(3.0, 0.5))));
    assert_eq!(lab.undo("envelope-show", &preview, &applied).unwrap_err().0, "changed");
    let (code, why) = lab.discard("envelope-show", &preview, &applied).unwrap_err();
    assert!(code == "changed" && why.contains("even with discard"), "{why}");
    assert_eq!(count(), 1, "the producer's point stays");
    state(|s| s.tracks[1].envelopes.iter_mut().find(|e| e.name == "Pan").map(|e| e.pts.as_mut().unwrap().pop()));
    lab.undo("envelope-show", &preview, &applied).unwrap();
    assert_eq!(count(), 0);
    // The fence: shown by the producer between the preview and the apply.
    let preview = lab.preview("envelope-show", input.clone()).unwrap();
    state(|s| s.tracks[1].envelopes.iter_mut().find(|e| e.name == "Pan").map(|e| e.visible = true));
    assert_eq!(lab.apply("envelope-show", &input, &preview["fence"]).unwrap_err().0, "changed");
    assert_eq!(lab.preview("envelope-show", json!({"trackRef": KICK, "name": "loudness"})).unwrap_err().0, "bad-params");
    assert_eq!(lab.preview("envelope-show", json!({"trackRef": KICK, "takeRef": TAKE, "name": "pan"})).unwrap_err().0, "bad-params");
}

#[test]
fn showing_an_envelope_isnt_undone_once_the_producer_moved_or_looped_its_automation_item() {
    let lab = envelope_lab();
    let item = FakeAutoItem {
        pool: 1,
        position: 4.0,
        length: 1.0,
        offset: 0.0,
        rate: 1.0,
        baseline: 0.5,
        amplitude: 1.0,
        looped: false,
        mute: false,
        selected: false,
    };
    state(|s| {
        let volume = &mut s.tracks[1].envelopes[0];
        volume.visible = false;
        volume.items = vec![item.clone()];
    });
    let (preview, applied) = lab.make("envelope-show", json!({"ref": KICK_VOLUME}));
    for (what, change) in
        [("moved", FakeAutoItem { position: 6.0, ..item.clone() }), ("looped", FakeAutoItem { looped: true, ..item.clone() })]
    {
        state(|s| s.tracks[1].envelopes[0].items = vec![change.clone()]);
        assert_eq!(lab.undo("envelope-show", &preview, &applied).unwrap_err().0, "changed", "{what}");
        let (code, why) = lab.discard("envelope-show", &preview, &applied).unwrap_err();
        assert!(code == "changed" && why.contains("even with discard"), "{what}: {why}");
        assert!(state(|s| s.tracks[1].envelopes[0].items == vec![change.clone()]), "{what}: the producer's item stays");
    }
    state(|s| s.tracks[1].envelopes[0].items = vec![item.clone()]);
    lab.undo("envelope-show", &preview, &applied).unwrap();
    assert!(state(|s| !s.tracks[1].envelopes[0].visible));
}

#[test]
fn an_fx_parameters_envelope_is_made_and_undo_takes_it_out_of_the_list() {
    let lab = chains_lab();
    let input = json!({"paramRef": "r:param:{K-MAIN}:0"});
    let (preview, applied) = lab.make("envelope-show", input.clone());
    assert_eq!(preview["display"]["title"], "“Kick”'s Gain / ReaComp envelope shown and on");
    let made = state(|s| s.tracks[1].envelopes.iter().find(|e| e.fx_param == Some(("{K-MAIN}".into(), 0))).cloned()).unwrap();
    assert_eq!(fake::points_of(&made).len(), 1, "REAPER makes it with a point at the parameter's value");
    assert_eq!(applied["created"]["kind"], "env");
    assert_eq!(applied["undo"]["entry"], true, "GetFXEnvelope makes no point by itself: sorting the envelope makes one");
    lab.undo("envelope-show", &preview, &applied).unwrap();
    assert_eq!(lab.read(json!({"kind": "envelope", "parent": fx_ref("{K-MAIN}")}))["total"], 0);
    // A take FX's parameter; a container child's is refused (REAPER automates it through a macro).
    let (preview, applied) = lab.make("envelope-show", json!({"paramRef": "r:param:{T-FX}:1"}));
    assert_eq!(state(|s| s.tracks[1].items[0].takes[0].envelopes.len()), 1);
    lab.undo("envelope-show", &preview, &applied).unwrap();
    state(|s| s.tracks[0].fx[1].children.as_mut().unwrap()[0] = plugin("{FX-COMP}", "VST: ReaComp (Cockos)"));
    let (code, message) = lab.preview("envelope-show", json!({"paramRef": "r:param:{FX-COMP}:0"})).unwrap_err();
    assert!(code == "bad-params" && message.contains("macro"), "{message}");
}

#[test]
fn a_takes_envelope_is_made_on_its_item_alone_and_the_producers_selection_is_kept() {
    let lab = chains_lab();
    state(|s| {
        s.tracks[2].items[0].selected = true;
        s.tracks[1].items[1].selected = true;
    });
    let (preview, applied) = lab.make("envelope-show", json!({"takeRef": TAKE, "name": "volume"}));
    assert_eq!(preview["display"]["title"], "Take “kick.wav”'s Volume envelope shown and on");
    assert_eq!(state(|s| s.tracks[1].items[0].takes[0].envelopes.iter().map(|e| e.name.clone()).collect::<Vec<_>>()), ["Volume"]);
    assert_eq!(state(|s| s.tracks[1].items[1].takes[0].envelopes.len()), 0, "only that take's");
    assert_eq!(
        state(|s| (s.tracks[1].items[0].selected, s.tracks[1].items[1].selected, s.tracks[2].items[0].selected)),
        (false, true, true)
    );
    assert!(applied["applied"]["ref"].as_str().unwrap().starts_with("r:env:take:{TAKE-K1}:"));
    let undone = lab.undo("envelope-show", &preview, &applied).unwrap();
    assert!(undone["display"].get("note").is_none(), "{}", undone["display"]);
    assert_eq!(state(|s| s.tracks[1].items[0].takes[0].envelopes.len()), 0, "out of the take's list again");
    assert_eq!(lab.undo("envelope-show", &preview, &applied).unwrap()["undo"]["entry"], false, "a retry writes nothing");
}

#[test]
fn a_sends_envelope_is_shown_by_the_send_ref() {
    let lab = chains_lab();
    state(|s| {
        let mut envelope = FakeEnvelope::for_send("Send Volume: Reverb", 0, "<VOLENV", 0);
        envelope.active = false;
        envelope.visible = false;
        envelope.pts = Some(vec![]);
        s.tracks[0].envelopes.push(envelope);
    });
    let (preview, applied) = lab.make("envelope-show", json!({"sendRef": "r:send:{AAAA}:0:{DDDD}", "name": "volume"}));
    assert_eq!(preview["display"]["title"], "“Drums”'s Send Volume: Reverb envelope shown and on");
    assert!(state(|s| s.tracks[0].envelopes.last().unwrap().active));
    lab.undo("envelope-show", &preview, &applied).unwrap();
    assert!(!state(|s| s.tracks[0].envelopes.last().unwrap().active));
}

#[test]
fn points_are_added_in_the_envelopes_own_units_and_undo_puts_the_lane_back_point_for_point() {
    let lab = envelope_lab();
    let before = kick_points();
    let input = json!({"ref": KICK_VOLUME, "points": [
        {"position": 0.5, "value": -12, "shape": "bezier", "tension": 0.3},
        {"positionBeats": 3, "value": 3},
    ]});
    let (preview, applied) = lab.make("envelope-points-add", input.clone());
    assert_eq!(preview["display"]["title"], "Added 2 points to “Kick”'s Volume envelope");
    assert_eq!(preview["display"]["units"], "dB");
    let points = kick_points();
    assert_eq!(points.len(), 5);
    let gain = |raw: f64| 20.0 * ((raw / 716.217_850_312_61).powi(4)).log10();
    assert!((gain(points[1].value) + 12.0).abs() < 1e-9 && points[1].shape == 5 && points[1].tension == 0.3, "{:?}", points[1]);
    assert!(
        (points[3].time - 1.5).abs() < 1e-9 && (gain(points[3].value) - 3.0).abs() < 1e-9,
        "3 beats at 120 BPM is 1.5 s: {:?}",
        points[3]
    );
    assert_eq!(applied["undo"]["entry"], true);
    lab.undo("envelope-points-add", &preview, &applied).unwrap();
    assert_eq!(kick_points(), before);
    // A producer's edit to the lane since: refused; discard puts it back.
    let (preview, applied) = lab.make("envelope-points-add", input.clone());
    state(|s| s.tracks[1].envelopes[0].pts.as_mut().unwrap()[0].value = 1.0);
    assert_eq!(lab.undo("envelope-points-add", &preview, &applied).unwrap_err().0, "changed");
    lab.discard("envelope-points-add", &preview, &applied).unwrap();
    assert_eq!(kick_points(), before);
    // The fence: the lane changed between the preview and the apply.
    let preview = lab.preview("envelope-points-add", input.clone()).unwrap();
    state(|s| s.tracks[1].envelopes[0].pts.as_mut().unwrap().pop());
    assert_eq!(lab.apply("envelope-points-add", &input, &preview["fence"]).unwrap_err().0, "changed");
    for bad in [
        json!({"ref": KICK_VOLUME, "points": [{"position": 1, "value": 40}]}),
        json!({"ref": KICK_VOLUME, "points": [{"value": 0}]}),
        json!({"ref": KICK_VOLUME, "points": [{"position": 1, "value": 0, "shape": "wiggle"}]}),
        json!({"ref": KICK_VOLUME, "points": [{"position": 1, "value": 0, "text": "-3 dB"}]}),
        json!({"ref": KICK_VOLUME, "points": []}),
    ] {
        assert_eq!(lab.preview("envelope-points-add", bad.clone()).unwrap_err().0, "bad-params", "{bad}");
    }
}

#[test]
fn points_are_changed_deleted_and_cleared_from_a_range_and_each_undo_puts_the_lane_back() {
    let lab = envelope_lab();
    let before = kick_points();
    let (preview, applied) = lab.make(
        "envelope-points-change",
        json!({"ref": KICK_VOLUME, "changes": [{"index": 1, "value": -3, "shape": "square"}, {"index": 2, "position": 4}]}),
    );
    assert_eq!(preview["display"]["title"], "Changed 2 points of “Kick”'s Volume envelope");
    let points = kick_points();
    assert_eq!((points[1].shape, points[2].time), (1, 4.0));
    lab.undo("envelope-points-change", &preview, &applied).unwrap();
    assert_eq!(kick_points(), before);
    let (preview, applied) = lab.make("envelope-points-delete", json!({"ref": KICK_VOLUME, "indexes": [2, 0]}));
    assert_eq!(kick_points().len(), 1);
    lab.undo("envelope-points-delete", &preview, &applied).unwrap();
    assert_eq!(kick_points(), before);
    let (preview, applied) = lab.make("envelope-range-clear", json!({"ref": KICK_VOLUME, "start": 0.5, "end": 2.5}));
    assert_eq!(preview["display"]["title"], "Cleared “Kick”'s Volume envelope from bar 1 beat 2 to bar 2 beat 2 (2 points)");
    assert_eq!(kick_points().len(), 1);
    lab.undo("envelope-range-clear", &preview, &applied).unwrap();
    assert_eq!(kick_points(), before);
    // A range with nothing in it writes nothing and makes no point.
    let (_, applied) = lab.make("envelope-range-clear", json!({"ref": KICK_VOLUME, "start": 10, "end": 12}));
    assert_eq!(applied["undo"]["entry"], false);
    for bad in [
        json!({"ref": KICK_VOLUME, "changes": [{"index": 3, "value": 0}]}),
        json!({"ref": KICK_VOLUME, "changes": [{"index": 1, "value": 0}, {"index": 1, "value": 1}]}),
        json!({"ref": KICK_VOLUME, "indexes": [7]}),
    ] {
        let kind = if bad.get("changes").is_some() { "envelope-points-change" } else { "envelope-points-delete" };
        assert_eq!(lab.preview(kind, bad.clone()).unwrap_err().0, "bad-params", "{bad}");
    }
    assert_eq!(lab.preview("envelope-range-clear", json!({"ref": KICK_VOLUME, "start": 3, "end": 2})).unwrap_err().0, "bad-params");
}

#[test]
fn pan_mute_and_take_envelope_points_are_in_their_own_units_and_a_takes_times_count_from_its_item() {
    let lab = chains_lab();
    let (_, shown) = lab.make("envelope-show", json!({"trackRef": KICK, "name": "pan"}));
    let pan = shown["applied"]["ref"].as_str().unwrap().to_string();
    lab.make("envelope-points-add", json!({"ref": pan, "points": [{"position": 1, "value": -1}]}));
    let raw = state(|s| s.tracks[1].envelopes.iter().find(|e| e.name == "Pan").map(|e| fake::points_of(e).last().unwrap().value)).unwrap();
    assert_eq!(raw, 1.0, "REAPER's pan envelope keeps left as +1");
    let read = lab.read(json!({"kind": "point", "parent": pan}));
    assert_eq!(
        (read["units"].clone(), read["items"][1]["value"].clone(), read["items"][1]["display"].clone()),
        (json!("pan"), json!(-1.0), json!("100%L"))
    );
    // A take envelope's points are kept from the item's start (Kick's second item starts at 2 s).
    let (_, shown) = lab.make("envelope-show", json!({"takeRef": "r:take:{TAKE-K2}", "name": "pitch"}));
    let pitch = shown["applied"]["ref"].as_str().unwrap().to_string();
    lab.make("envelope-points-add", json!({"ref": pitch, "points": [{"position": 3, "value": 7}]}));
    let point = state(|s| fake::points_of(&s.tracks[1].items[1].takes[0].envelopes[0]).last().cloned()).unwrap();
    assert_eq!((point.time, point.value), (1.0, 7.0));
    let read = lab.read(json!({"kind": "point", "parent": pitch}));
    assert_eq!(read["items"][1]["position"], 3.0, "a read gives it on the project's timeline");
    let outside = json!({"ref": pitch, "points": [{"position": 9, "value": 1}]});
    assert_eq!(lab.preview("envelope-points-add", outside).unwrap_err().0, "bad-params", "outside the item");
    // An FX parameter's envelope takes its raw value or what it shows.
    let (_, shown) = lab.make("envelope-show", json!({"paramRef": "r:param:{K-MAIN}:0"}));
    let gain = shown["applied"]["ref"].as_str().unwrap().to_string();
    lab.make("envelope-points-add", json!({"ref": gain, "points": [{"position": 1, "raw": 0.25}, {"position": 2, "value": 0.75}]}));
    assert_eq!(lab.read(json!({"kind": "point", "parent": gain}))["units"], "raw");
}

#[test]
fn an_added_automation_item_the_producer_moved_is_left_not_called_gone() {
    let lab = envelope_lab();
    let input = json!({"ref": KICK_VOLUME, "position": 4.0, "length": 1.0});
    let (preview, applied) = lab.make("automation-item-add", input);
    state(|s| s.tracks[1].envelopes[0].items[0].position = 6.0);
    let (code, message) = lab.undo("automation-item-add", &preview, &applied).unwrap_err();
    assert_eq!(code, "changed", "{message}");
    assert_eq!(state(|s| s.tracks[1].envelopes[0].items.len()), 1, "it stays");
    // Taken off by the producer, it is gone already.
    state(|s| s.tracks[1].envelopes[0].items.clear());
    let gone = lab.undo("automation-item-add", &preview, &applied).unwrap();
    assert!(gone["display"]["title"].as_str().unwrap().contains("gone"), "{gone}");
}

#[test]
fn an_automation_item_is_added_as_a_new_pool_or_a_pooled_copy_changed_and_deleted_and_each_undo_puts_it_back() {
    let lab = envelope_lab();
    let before = kick_points();
    // A new pool over the envelope's points takes them in (REAPER's way); undo gives them back.
    let input = json!({"ref": KICK_VOLUME, "position": 0.5, "length": 1.0, "points": [{"position": 0.75, "value": -20}]});
    let (preview, applied) = lab.make("automation-item-add", input.clone());
    assert!(preview["display"]["note"].as_str().unwrap().contains("go into the new item"));
    // In bars and beats, as items and markers are (review U16).
    assert_eq!(preview["display"]["title"], "Added an automation item to “Kick”'s Volume envelope from bar 1 beat 2 to bar 1 beat 4");
    assert_eq!(applied["display"]["title"], "Added an automation item to “Kick”'s Volume envelope from bar 1 beat 2 to bar 1 beat 4");
    let item = applied["created"]["ref"].as_str().unwrap().to_string();
    assert!(item.starts_with("r:aitem:{BBBB}:{ENV-K-VOL}:1:0.5"), "{item}");
    assert_eq!(kick_points().len(), 2, "the point at 1 s went into the item");
    let read = lab.read(json!({"kind": "autoitem", "parent": KICK_VOLUME}));
    assert_eq!(
        (read["items"][0]["ref"].clone(), read["items"][0]["pool"].clone(), read["items"][0]["loop"].clone()),
        (json!(item), json!(1), json!(true))
    );
    let points = lab.read(json!({"kind": "point", "parent": item}));
    assert_eq!(points["total"], 2, "the point it took in, and the one Kumi wrote");
    let undone = lab.undo("automation-item-add", &preview, &applied).unwrap();
    assert_eq!(undone["undo"]["entry"], true, "taken off through the envelope's state, then sorted for a point");
    assert_eq!(kick_points(), before);
    assert!(state(|s| s.tracks[1].envelopes[0].items.is_empty()));
    // A pooled copy shares its pool's points.
    let (_, first) = lab.make("automation-item-add", json!({"ref": KICK_VOLUME, "position": 4, "length": 1}));
    let first = first["created"]["ref"].as_str().unwrap().to_string();
    lab.make("envelope-points-add", json!({"ref": first, "points": [{"position": 4.5, "value": -6}]}));
    let (preview, applied) =
        lab.make("automation-item-add", json!({"ref": KICK_VOLUME, "positionBeats": 16, "length": 1, "poolRef": first}));
    let copy = applied["created"]["ref"].as_str().unwrap().to_string();
    assert!(copy.contains(":2:8"), "{copy}");
    assert_eq!(lab.read(json!({"kind": "point", "parent": copy}))["items"][0]["position"], 8.5);
    // Changed: loop off, baseline, moved; undo finds it at its new place and puts it back.
    let (p2, a2) = lab.make("automation-item-set", json!({"ref": copy, "loop": false, "baseline": 0.25, "position": 10}));
    assert_eq!(p2["display"]["title"], "The automation item at bar 5 on “Kick”'s Volume envelope: loop off, baseline 0.25, moved to bar 6");
    assert_eq!(a2["display"]["ref"], copy.replace(":8", ":10"));
    lab.undo("automation-item-set", &p2, &a2).unwrap();
    let it = state(|s| s.tracks[1].envelopes[0].items[1].clone());
    assert_eq!((it.position, it.looped, it.baseline), (8.0, true, 0.5));
    assert_eq!(lab.undo("automation-item-set", &p2, &a2).unwrap()["undo"]["entry"], false, "a retry writes nothing");
    lab.undo("automation-item-add", &preview, &applied).unwrap();
    // Deleted while another copy of its pool remains: put back as that pool's copy.
    let (_, again) = lab.make("automation-item-add", json!({"ref": KICK_VOLUME, "position": 8, "length": 1, "poolRef": first}));
    let again = again["created"]["ref"].as_str().unwrap().to_string();
    let (preview, applied) = lab.make("automation-item-delete", json!({"ref": again}));
    assert_eq!(applied["undo"]["entry"], true);
    assert_eq!(state(|s| s.tracks[1].envelopes[0].items.len()), 1);
    let restored = lab.undo("automation-item-delete", &preview, &applied).unwrap();
    assert!(restored["display"].get("note").is_none());
    // The last copy of a pool: REAPER empties the pool, so its points are put back into it.
    lab.make("automation-item-delete", json!({"ref": again}));
    let (preview, applied) = lab.make("automation-item-delete", json!({"ref": first}));
    assert!(state(|s| s.pools[&2].is_empty()), "REAPER empties a pool with no copy left");
    let restored = lab.undo("automation-item-delete", &preview, &applied).unwrap();
    assert!(restored["display"]["note"].as_str().unwrap().contains("put back into the pool"));
    assert_eq!(lab.read(json!({"kind": "point", "parent": first}))["total"], 1);
    // Two of one pool at one place name neither: refused.
    state(|s| {
        let twin = s.tracks[1].envelopes[0].items[0].clone();
        s.tracks[1].envelopes[0].items.push(twin);
    });
    assert_eq!(lab.preview("automation-item-set", json!({"ref": first, "mute": true})).unwrap_err().0, "changed");
    assert_eq!(
        lab.preview("automation-item-add", json!({"ref": "r:env:take:{TAKE-K1}:{X}", "position": 1, "length": 1})).unwrap_err().0,
        "failed"
    );
}

#[test]
fn a_tracks_automation_mode_and_the_global_override_are_set_and_put_back() {
    let lab = chains_lab();
    let (preview, applied) = lab.make("track-automation-mode", json!({"ref": KICK, "mode": "latch-preview"}));
    assert_eq!(preview["display"]["title"], "Kick automation latch preview");
    assert_eq!(state(|s| s.tracks[1].extra.get("I_AUTOMODE").copied()), Some(5.0));
    state(|s| s.tracks[1].extra.insert("I_AUTOMODE".into(), 1.0));
    assert_eq!(lab.undo("track-automation-mode", &preview, &applied).unwrap_err().0, "changed");
    lab.discard("track-automation-mode", &preview, &applied).unwrap();
    assert_eq!(state(|s| s.tracks[1].extra.get("I_AUTOMODE").copied()), Some(0.0));
    // The override is REAPER's own setting, no step of its undo: only Kumi's undo takes it back.
    let (preview, applied) = lab.make("automation-override", json!({"mode": "bypass"}));
    assert_eq!(preview["display"]["title"], "Global automation override: bypass all automation");
    assert_eq!(state(|s| s.automation_override), 6);
    assert_eq!(applied["undo"]["entry"], false);
    lab.undo("automation-override", &preview, &applied).unwrap();
    assert_eq!(state(|s| s.automation_override), -1);
    let input = json!({"mode": "read"});
    let preview = lab.preview("automation-override", input.clone()).unwrap();
    state(|s| s.automation_override = 2);
    assert_eq!(lab.apply("automation-override", &input, &preview["fence"]).unwrap_err().0, "changed");
    assert_eq!(lab.preview("automation-override", json!({"mode": "loud"})).unwrap_err().0, "bad-params");
}

#[test]
fn a_sends_automation_mode_is_set_and_put_back() {
    let lab = chains_lab();
    let send = "r:send:{AAAA}:0:{DDDD}";
    let mode = || state(|s| s.tracks[0].sends[0].extra.get("I_AUTOMODE").copied().unwrap_or(-1.0));
    let input = json!({"ref": send, "mode": "touch"});
    let (preview, applied) = lab.make("send-automation-mode", input.clone());
    assert_eq!(preview["display"]["title"], "Send “Drums” → “Reverb” automation touch");
    assert_eq!((mode(), applied["undo"]["entry"].clone()), (2.0, json!(true)));
    lab.undo("send-automation-mode", &preview, &applied).unwrap();
    assert_eq!(mode(), -1.0, "back to its track's mode");
    assert_eq!(lab.undo("send-automation-mode", &preview, &applied).unwrap()["undo"]["entry"], false, "a retry writes nothing");
    // The producer changed it since: refused; discard puts it back.
    let (preview, applied) = lab.make("send-automation-mode", input.clone());
    state(|s| {
        s.tracks[0].sends[0].extra.insert("I_AUTOMODE".into(), 3.0);
    });
    assert_eq!(lab.undo("send-automation-mode", &preview, &applied).unwrap_err().0, "changed");
    lab.discard("send-automation-mode", &preview, &applied).unwrap();
    assert_eq!(mode(), -1.0);
    // The fence: changed between the preview and the apply.
    let preview = lab.preview("send-automation-mode", input.clone()).unwrap();
    state(|s| {
        s.tracks[0].sends[0].extra.insert("I_AUTOMODE".into(), 1.0);
    });
    assert_eq!(lab.apply("send-automation-mode", &input, &preview["fence"]).unwrap_err().0, "changed");
    assert_eq!(mode(), 1.0, "nothing written");
    let (preview, _) = lab.make("send-automation-mode", json!({"ref": send, "mode": "track"}));
    assert_eq!((preview["display"]["title"].clone(), mode()), (json!("Send “Drums” → “Reverb” automation as its track's"), -1.0));
    // The receive's ref names the same send.
    let (preview, applied) = lab.make("send-automation-mode", json!({"ref": "r:send:{DDDD}:-1:{AAAA}", "mode": "read"}));
    assert_eq!((preview["display"]["title"].clone(), mode()), (json!("Send “Drums” → “Reverb” automation read"), 1.0));
    lab.undo("send-automation-mode", &preview, &applied).unwrap();
    assert_eq!(mode(), -1.0);
    // The send gone: `gone`.
    let (preview, applied) = lab.make("send-automation-mode", input.clone());
    state(|s| s.tracks[0].sends.clear());
    assert_eq!(lab.undo("send-automation-mode", &preview, &applied).unwrap_err().0, "gone");
    assert_eq!(lab.preview("send-automation-mode", json!({"ref": send, "mode": "loud"})).unwrap_err().0, "bad-params");
    assert_eq!(lab.preview("send-automation-mode", json!({"ref": KICK, "mode": "read"})).unwrap_err().0, "bad-params");
}

#[test]
fn envelope_reads_give_points_automation_items_and_a_takes_envelopes_by_their_refs() {
    let lab = envelope_lab();
    let rows = lab.read(json!({"kind": "envelope", "parent": KICK}));
    assert_eq!((rows["items"][0]["units"].clone(), rows["items"][0]["automationItems"].clone()), (json!("dB"), json!(0)));
    let points = lab.read(json!({"kind": "point", "parent": KICK_VOLUME, "limit": 2}));
    assert_eq!((points["total"].clone(), points["items"].as_array().unwrap().len(), points["indexFrom"].clone()), (json!(3), 2, json!(0)));
    assert_eq!((points["items"][1]["value"].clone(), points["items"][1]["display"].clone()), (json!(-6.0), json!("-6.00dB")));
    let next = lab.read(json!({"kind": "point", "parent": KICK_VOLUME, "cursor": points["nextCursor"]}));
    assert_eq!(next["items"][0]["index"], 2);
    lab.make("envelope-show", json!({"takeRef": TAKE, "name": "volume"}));
    let take_rows = lab.read(json!({"kind": "envelope", "parent": TAKE}));
    let take_env = take_rows["items"][0]["ref"].as_str().unwrap().to_string();
    assert!(take_env.starts_with("r:env:take:{TAKE-K1}:"), "{take_env}");
    assert_eq!(lab.read(json!({"ref": take_env}))["items"][0]["name"], "Volume");
    let (_, made) = lab.make("automation-item-add", json!({"ref": KICK_VOLUME, "position": 3, "length": 1}));
    let item = made["created"]["ref"].as_str().unwrap();
    assert_eq!(lab.read(json!({"ref": item}))["items"][0]["position"], 3.0);
    assert_eq!(lab.call("read", json!({"kind": "point"})).unwrap_err().0, "bad-params");
    assert_eq!(lab.call("read", json!({"kind": "autoitem", "parent": "r:env:take:{TAKE-K1}:{NOPE}"})).unwrap_err().0, "failed");
}

#[test]
fn reads_of_points_automation_items_and_a_big_state_stay_inside_the_frame_and_every_row_is_reachable() {
    let lab = envelope_lab();
    let walk = |kind: &str, parent: &str, total: usize| {
        let (mut seen, mut cursor) = (0, Value::Null);
        loop {
            let mut params = json!({"kind": kind, "parent": parent, "limit": 200});
            if cursor.is_string() {
                params["cursor"] = cursor.clone();
            }
            let page = lab.read(params);
            assert!(serde_json::to_string(&page).unwrap().len() <= 256 * 1024, "{kind}");
            assert_eq!(page["total"], total, "{kind}");
            seen += page["items"].as_array().unwrap().len();
            cursor = page["nextCursor"].clone();
            if !cursor.is_string() {
                return seen;
            }
        }
    };
    // 20,000 points (the most Kumi changes in a lane) and 300 automation items.
    state(|s| {
        let envelope = &mut s.tracks[1].envelopes[0];
        envelope.pts = Some((0..20_000).map(|i| FakePoint::at(i as f64 * 0.01, 716.0)).collect());
        envelope.items = (0..300)
            .map(|i| FakeAutoItem {
                pool: i + 1,
                position: 300.0 + i as f64,
                length: 0.5,
                offset: 0.0,
                rate: 1.0,
                baseline: 0.5,
                amplitude: 1.0,
                looped: true,
                mute: false,
                selected: false,
            })
            .collect();
        s.state_count += 1;
    });
    assert_eq!(walk("point", KICK_VOLUME, 20_000), 20_000);
    assert_eq!(walk("autoitem", KICK_VOLUME, 300), 300);
    let made_up = lab.call("read", json!({"kind": "point", "parent": KICK_VOLUME, "cursor": "19999"}));
    assert_eq!(made_up.unwrap_err().0, "bad-params", "a cursor is only what a page gave");
    // A plug-in's state past what a read carries (the fake's is its 8,000 values, 17 bytes each): its size and hash.
    state(|s| {
        s.tracks[1].fx[0].chunk = Some(String::new());
        s.tracks[1].fx[0].params = (0..8000).map(|i| FakeParam::new(&format!("P{i}"), 0.5, "50")).collect();
    });
    let answer = lab.read(json!({"ref": fx_ref("{K-MAIN}")}));
    assert!(serde_json::to_string(&answer).unwrap().len() <= 256 * 1024);
    let detail = &answer["items"][0]["state"];
    assert_eq!((detail["bytes"].clone(), detail.get("chunk").is_none(), detail["hash"].is_string()), (json!(8000 * 17 - 1), true, true));
}

/// What REAPER's own undo and redo do to an envelope's numbers: they come back from its state's text (lab 7.82: a point's
/// time, unscaled value and tension to 8 places; an automation item's position to 14, baseline, amplitude and playrate to 6).
fn through_reapers_undo() {
    let round = |v: f64, places: i32| (v * 10f64.powi(places)).round() / 10f64.powi(places);
    let api = fake::api();
    let reaper = Reaper::new(&api);
    let scaled: Vec<f64> = state(|s| fake::points_of(&s.tracks[1].envelopes[0]).iter().map(|p| p.value).collect());
    let kept: Vec<f64> = scaled.iter().map(|v| reaper.scale_to_envelope(1, round(reaper.scale_from_envelope(1, *v), 8))).collect();
    state(|s| {
        let envelope = &mut s.tracks[1].envelopes[0];
        for (point, value) in envelope.pts.as_mut().unwrap().iter_mut().zip(kept) {
            (point.time, point.value, point.tension) = (round(point.time, 8), value, round(point.tension, 8));
        }
        for item in &mut envelope.items {
            item.position = round(item.position, 14) + 2e-15;
            (item.baseline, item.amplitude, item.rate) = (round(item.baseline, 6), round(item.amplitude, 6), round(item.rate, 6));
        }
        s.state_count += 1;
    });
}

#[test]
fn reapers_own_undo_and_redo_in_between_dont_make_kumis_undo_refuse() {
    let lab = envelope_lab();
    let before = state(|s| fake::points_of(&s.tracks[1].envelopes[0]).len());
    let (preview, applied) = lab.make(
        "envelope-points-add",
        json!({"ref": KICK_VOLUME, "points": [{"position": 0.123456789012, "value": -6.123456789, "shape": "bezier", "tension": 0.123456789}]}),
    );
    through_reapers_undo();
    lab.undo("envelope-points-add", &preview, &applied).unwrap();
    assert_eq!(state(|s| fake::points_of(&s.tracks[1].envelopes[0]).len()), before);
    // An automation item at an awkward place, then changed: both undos still find it and see it as Kumi left it.
    let (add_preview, added) = lab.make("automation-item-add", json!({"ref": KICK_VOLUME, "position": 5.123456789012345, "length": 0.75}));
    let item = added["created"]["ref"].as_str().unwrap().to_string();
    assert!(item.ends_with(":5.123456789"), "a ref names its place to a nanosecond: {item}");
    let (set_preview, set) = lab.make("automation-item-set", json!({"ref": item, "baseline": 0.123456789, "playrate": 1.23456789}));
    through_reapers_undo();
    assert_eq!(lab.read(json!({"ref": item}))["items"][0]["ref"], item, "the same ref after REAPER's round trip");
    lab.undo("automation-item-set", &set_preview, &set).unwrap();
    lab.undo("automation-item-add", &add_preview, &added).unwrap();
    assert!(state(|s| s.tracks[1].envelopes[0].items.is_empty()));
}

// ----- several parameters of one FX at once -----

/// An FX of the fake by its GUID, in any chain.
fn fake_fx(guid: &str) -> Option<FakeFx> {
    fn find(chain: &[FakeFx], guid: &str) -> Option<FakeFx> {
        chain.iter().find_map(|f| if f.guid == guid { Some(f.clone()) } else { find(f.children.as_deref()?, guid) })
    }
    state(|s| {
        s.tracks.iter().chain(std::iter::once(&s.master)).find_map(|t| {
            find(&t.fx, guid)
                .or_else(|| find(&t.input_fx, guid))
                .or_else(|| t.items.iter().flat_map(|i| i.takes.iter()).find_map(|take| find(&take.fx, guid)))
        })
    })
}

fn fake_values(guid: &str) -> Vec<f64> {
    fake_fx(guid).unwrap().params.iter().map(|p| p.value).collect()
}

#[test]
fn several_parameters_of_one_fx_are_set_in_one_point_and_undo_puts_each_back() {
    let lab = chains_lab();
    state(|s| s.tracks[1].fx.push(reaeq("{K-EQ}")));
    let before = fake_values("{K-EQ}");
    let input = json!({"ref": fx_ref("{K-EQ}"), "values": [
        {"param": 0, "value": "120 Hz"},
        {"param": "Gain-Band 1", "units": -3.5},
        {"param": 5, "normalized": 0.75},
    ]});
    let entries = state(|s| s.undo_entries);
    let (preview, applied) = lab.make("fx-params", input.clone());
    assert_eq!(preview["display"]["title"], "ReaEQ 3 parameters on “Kick”");
    assert_eq!(preview["display"]["lines"][0], "Freq-Band 1 → 120.0 Hz");
    assert_eq!(preview["display"]["lines"][1], "Gain-Band 1 → -3.5 dB");
    assert_eq!((preview["prior"]["guid"].clone(), preview["prior"]["params"]["5"].clone()), (json!("{K-EQ}"), json!(0.5)));
    assert_eq!(preview["prior"]["chain"]["trackRef"], KICK);
    assert_eq!((applied["undo"]["entry"].clone(), state(|s| s.undo_entries)), (json!(true), entries + 1), "one point for all three");
    assert_eq!(applied["applied"]["params"]["5"], 0.75);
    assert_eq!(applied["display"]["lines"][2], "BW-Band 2 → 3.00");
    let now = fake_values("{K-EQ}");
    assert!(now[0] != before[0] && now[1] != before[1] && now[5] == 0.75 && now[2] == before[2]);
    lab.undo("fx-params", &preview, &applied).unwrap();
    assert_eq!(fake_values("{K-EQ}"), before);
    assert_eq!(lab.undo("fx-params", &preview, &applied).unwrap()["undo"]["entry"], false, "a retry writes nothing");
    // One of them changed since: refused, naming it; discard puts every one back.
    let (preview, applied) = lab.make("fx-params", input.clone());
    state(|s| s.tracks[1].fx[1].params[1].value = 0.9);
    let (code, message) = lab.undo("fx-params", &preview, &applied).unwrap_err();
    assert!(code == "changed" && message.contains("Gain-Band 1"), "{message}");
    lab.discard("fx-params", &preview, &applied).unwrap();
    assert_eq!(fake_values("{K-EQ}"), before);
    // The fence: one of them moved between the preview and the apply; nothing is written.
    let preview = lab.preview("fx-params", input.clone()).unwrap();
    state(|s| s.tracks[1].fx[1].params[5].value = 0.1);
    assert_eq!(lab.apply("fx-params", &input, &preview["fence"]).unwrap_err().0, "changed");
    assert_eq!(fake_values("{K-EQ}")[..2], before[..2]);
    // A take's ReaEQ (REAPER gives a take's FX no GetEQParam): its units found by its band parameters' names.
    state(|s| s.tracks[1].items[0].takes[0].fx.push(reaeq("{T-EQ}")));
    let (_, applied) =
        lab.make("fx-params", json!({"ref": fx_ref("{T-EQ}"), "values": [{"param": 0, "units": 120}, {"param": 1, "units": -3.5}]}));
    assert_eq!(applied["display"]["lines"], json!(["Freq-Band 1 → 120.0 Hz", "Gain-Band 1 → -3.5 dB"]));
    // The FX gone: its undo is `gone`.
    let (preview, applied) = lab.make("fx-params", json!({"ref": fx_ref("{K-EQ}"), "values": [{"param": 3, "normalized": 0.1}]}));
    assert_eq!(preview["display"]["title"], "ReaEQ 1 parameter on “Kick”");
    state(|s| s.tracks[1].fx.retain(|f| f.guid != "{K-EQ}"));
    assert_eq!(lab.undo("fx-params", &preview, &applied).unwrap_err().0, "gone");
    for bad in [
        json!({"ref": fx_ref("{K-MAIN}"), "values": []}),
        json!({"ref": fx_ref("{K-MAIN}"), "values": [{"param": 0, "normalized": 0.2}, {"param": "Gain", "normalized": 0.3}]}),
        json!({"ref": fx_ref("{K-MAIN}"), "values": [{"param": 99, "normalized": 0.2}]}),
        json!({"ref": fx_ref("{K-MAIN}"), "values": [{"param": 0, "normalized": 2}]}),
        json!({"ref": fx_ref("{K-MAIN}"), "values": [{"param": 0, "normalized": 0.2, "value": "-3 dB"}]}),
        json!({"ref": fx_ref("{K-MAIN}"), "values": [{"param": 1, "units": 3}]}),
        json!({"ref": "r:param:{K-MAIN}:0", "values": [{"param": 0, "normalized": 0.2}]}),
    ] {
        assert_eq!(lab.preview("fx-params", bad.clone()).unwrap_err().0, "bad-params", "{bad}");
    }
}

#[test]
fn up_to_256_values_go_in_one_point_on_an_fx_in_any_chain() {
    let lab = chains_lab();
    let big = |guid: &str| {
        let mut fx = FakeFx::new(guid, "VST: Big (Lab)");
        fx.params = (0..300).map(|i| FakeParam::linear(&format!("P{i}"), 0.5, 0.0, 100.0, 0, "%")).collect();
        fx.params.extend(wrapper());
        fx
    };
    state(|s| {
        s.tracks[1].fx.push(big("{BIG-MAIN}"));
        s.tracks[1].input_fx.push(big("{BIG-IN}"));
        s.tracks[1].items[0].takes[0].fx.push(big("{BIG-TAKE}"));
        s.tracks[0].fx[1].children.as_mut().unwrap().push(big("{BIG-BOX}"));
        s.state_count += 1;
    });
    let values: Vec<Value> = (0..256).map(|i| json!({"param": i, "normalized": 0.25})).collect();
    for guid in ["{BIG-MAIN}", "{BIG-IN}", "{BIG-TAKE}", "{BIG-BOX}"] {
        let entries = state(|s| s.undo_entries);
        let (preview, applied) = lab.make("fx-params", json!({"ref": fx_ref(guid), "values": values}));
        assert_eq!(state(|s| s.undo_entries), entries + 1, "{guid}: one point");
        assert_eq!(applied["applied"]["params"].as_object().unwrap().len(), 256, "{guid}");
        assert_eq!(preview["display"]["lines"].as_array().unwrap().len(), 256, "{guid}");
        let set = fake_values(guid);
        assert!(set[..256].iter().all(|v| *v == 0.25) && set[256] == 0.5, "{guid}");
        lab.undo("fx-params", &preview, &applied).unwrap();
        assert!(fake_values(guid)[..300].iter().all(|v| *v == 0.5), "{guid}: every one back");
    }
    let too_many: Vec<Value> = (0..257).map(|i| json!({"param": i, "normalized": 0.25})).collect();
    let (code, message) = lab.preview("fx-params", json!({"ref": fx_ref("{BIG-MAIN}"), "values": too_many})).unwrap_err();
    assert!(code == "bad-params" && message.contains("256"), "{message}");
}

// ----- FX chain files saved -----

#[test]
fn a_chain_is_saved_as_a_file_without_ids_or_automation_and_a_file_there_is_replaced_only_when_asked() {
    let lab = Lab::new();
    let dir = resource();
    state(|s| {
        s.tracks[0].fx[0].parallel = 1;
        s.tracks[0].fx[0].config.insert("param.0.lfo.active".into(), "1".into());
        s.tracks[0].fx[1].children.as_mut().unwrap()[1].parallel = 1;
    });
    let marks = || state(|s| (s.undo_entries, s.state_count));
    let before = marks();
    let saved = lab.call("fx_chain_save", json!({"ref": DRUMS, "name": "Kumi Test/Drums 1.5"})).unwrap();
    let path = dir.path().join("FXChains").join("Kumi Test").join("Drums 1.5.RfxChain");
    assert_eq!(
        (saved["file"].clone(), saved["name"].clone(), saved["count"].clone(), saved["replaced"].clone()),
        (json!(path.display().to_string()), json!("Drums 1.5"), json!(2), json!(false))
    );
    assert_eq!(marks(), before, "a file, not the project: no undo point");
    let text = std::fs::read_to_string(&path).unwrap();
    assert!(!text.contains("FXID ") && !text.contains("<PARMENV"), "no ids and no automation: {text}");
    assert!(text.contains("<CONTAINER") && text.contains("<PROGRAMENV 0"), "the container with what's inside, and modulation");
    assert_eq!(text.matches("PARALLEL ").count(), 1, "the first FX runs in parallel with nothing: only the child's stays\n{text}");
    // Loaded back, it is the same FX with ids of their own.
    lab.make("fx-chain-load", json!({"ref": KICK, "file": "Kumi Test/Drums 1.5"}));
    let loaded = state(|s| s.tracks[1].fx.clone());
    assert_eq!(loaded.len(), 2);
    assert!(loaded[0].guid != "{FX-EQ}" && loaded[0].preset == "Warm" && loaded[1].children.as_ref().map(Vec::len) == Some(2));
    // A file of that name is never written over unless asked; then some FX, in the chain's order.
    let (code, message) = lab.call("fx_chain_save", json!({"ref": DRUMS, "name": "Kumi Test/Drums 1.5"})).unwrap_err();
    assert!(code == "failed" && message.contains("overwrite"), "{message}");
    assert_eq!(std::fs::read_to_string(&path).unwrap(), text);
    let again =
        json!({"ref": DRUMS, "name": "Kumi Test/Drums 1.5.RfxChain", "fxRefs": [fx_ref("{FX-C}"), fx_ref("{FX-EQ}")], "overwrite": true});
    assert_eq!(lab.call("fx_chain_save", again).unwrap()["replaced"], true);
    let text = std::fs::read_to_string(&path).unwrap();
    assert!(text.find("ReaEQ").unwrap() < text.find("<CONTAINER").unwrap(), "in the chain's order");
    assert_eq!(std::fs::read_dir(path.parent().unwrap()).unwrap().count(), 1, "nothing left beside it");
    // A saved FX whose parameter links to one left out: refused (the link would follow whatever lands in its place).
    state(|s| {
        s.tracks[0].fx[0].config.insert("param.1.plink.active".into(), "1".into());
        s.tracks[0].fx[0].config.insert("param.1.plink.effect".into(), "1".into());
    });
    let (code, message) = lab.call("fx_chain_save", json!({"ref": DRUMS, "name": "EQ", "fxRefs": [fx_ref("{FX-EQ}")]})).unwrap_err();
    assert!(code == "bad-params" && message.contains("link"), "{message}");
    // A take's and a container's chains; refusals.
    lab.call("fx_chain_save", json!({"ref": fx_ref("{FX-C}"), "name": "Inside"})).unwrap();
    assert!(dir.path().join("FXChains").join("Inside.RfxChain").exists());
    lab.call("fx_chain_save", json!({"ref": TAKE, "name": "Take"})).unwrap_err();
    for bad in [
        json!({"ref": DRUMS, "name": "../Elsewhere"}),
        json!({"ref": DRUMS, "name": "/tmp/Elsewhere"}),
        json!({"ref": DRUMS, "name": ""}),
        json!({"ref": DRUMS, "name": "Comp", "fxRefs": [fx_ref("{FX-COMP}")]}),
        json!({"ref": fx_ref("{FX-EQ}"), "name": "Not a chain"}),
    ] {
        assert_eq!(lab.call("fx_chain_save", bad.clone()).unwrap_err().0, "bad-params", "{bad}");
    }
    assert_eq!(lab.call("fx_chain_save", json!({"ref": DRUMS, "name": "Old", "epoch": "old-1"})).unwrap_err().0, "stale");
    state(|s| s.loading = true);
    assert_eq!(lab.call("fx_chain_save", json!({"ref": DRUMS, "name": "Busy"})).unwrap_err().0, "busy");
}

// ----- fences -----

/// Each kind's apply re-reads what its preview fenced and refuses `changed` before writing anything (here, the kinds not
/// fenced in their own tests).
#[test]
fn an_apply_is_refused_when_what_its_preview_fenced_moved_and_nothing_is_written() {
    let lab = envelope_lab();
    let _dir = resource();
    let refused = |kind: &str, input: Value, disturb: &dyn Fn()| {
        let preview = lab.preview(kind, input.clone()).unwrap_or_else(|e| panic!("{kind}: {e:?}"));
        disturb();
        let entries = state(|s| s.undo_entries);
        let (code, message) = lab.apply(kind, &input, &preview["fence"]).unwrap_err();
        assert_eq!(code, "changed", "{kind}: {message}");
        assert_eq!(state(|s| s.undo_entries), entries, "{kind}: no undo point");
    };
    refused("fx-container-add", json!({"ref": KICK}), &|| state(|s| s.tracks[1].fx.push(plugin("{P1}", "VST: ReaVerb (Cockos)"))));
    refused("fx-container-move", json!({"ref": fx_ref("{FX-EQ}"), "toRef": fx_ref("{FX-C}")}), &|| {
        state(|s| s.tracks[0].fx[1].children.as_mut().unwrap().swap(0, 1))
    });
    state(|s| {
        s.tracks[0].fx[1].children.as_mut().unwrap()[0] = plugin("{FX-COMP}", "VST: ReaComp (Cockos)");
        s.tracks[0].fx[1].params = wrapper();
    });
    lab.make("fx-container-map", json!({"ref": fx_ref("{FX-C}"), "childRef": fx_ref("{FX-COMP}"), "param": 0}));
    // (the producer took the macro off and mapped the other parameter: macro 0 is another mapping now)
    refused("fx-container-unmap", json!({"ref": "r:param:{FX-C}:0"}), &|| {
        state(|s| s.tracks[0].fx[1].config.insert("cmaps".into(), "7|{FX-COMP}|1".into())).map_or((), |_| ())
    });
    refused("automation-item-add", json!({"ref": KICK_VOLUME, "position": 5, "length": 1}), &|| {
        state(|s| s.tracks[1].envelopes[0].pts.as_mut().unwrap().push(FakePoint::at(9.0, 1.0)))
    });
    let (_, made) = lab.make("automation-item-add", json!({"ref": KICK_VOLUME, "position": 6, "length": 1}));
    let item = made["created"]["ref"].as_str().unwrap().to_string();
    refused("automation-item-set", json!({"ref": item, "baseline": 0.2}), &|| state(|s| s.tracks[1].envelopes[0].items[0].amplitude = 0.5));
    refused("automation-item-delete", json!({"ref": item}), &|| state(|s| s.tracks[1].envelopes[0].items[0].mute = true));
    refused("track-automation-mode", json!({"ref": KICK, "mode": "write"}), &|| {
        state(|s| s.tracks[1].extra.insert("I_AUTOMODE".into(), 2.0)).map_or((), |_| ())
    });
    refused("envelope-points-change", json!({"ref": KICK_VOLUME, "changes": [{"index": 0, "value": -1}]}), &|| {
        state(|s| s.tracks[1].envelopes[0].pts.as_mut().unwrap()[1].shape = 3)
    });
}

/// Each kind's undo refuses `changed` when what it wrote no longer holds (unless discard), and `gone` when its object went.
#[test]
fn an_undo_is_refused_when_its_object_went() {
    let lab = envelope_lab();
    let _dir = resource();
    let (p, a) = lab.make("envelope-points-add", json!({"ref": KICK_VOLUME, "points": [{"position": 3, "value": 0}]}));
    let (sp, sa) = lab.make("envelope-show", json!({"paramRef": "r:param:{K-MAIN}:1"}));
    let (mp, ma) = lab.make("track-automation-mode", json!({"ref": KICK, "mode": "touch"}));
    let (lp, la) = lab.make("fx-param-lfo", json!({"ref": "r:param:{K-MAIN}:0", "active": true}));
    let (wp, wa) = lab.make("fx-wet", json!({"ref": fx_ref("{K-MAIN}"), "wet": 0.3}));
    let (cp, ca) = lab.make("fx-container-add", json!({"ref": KICK}));
    state(|s| {
        s.tracks[1].envelopes.clear();
        s.tracks[1].fx.retain(|f| f.guid != "{K-MAIN}");
    });
    assert_eq!(lab.undo("envelope-points-add", &p, &a).unwrap_err().0, "gone");
    // An envelope Kumi made that is gone already: nothing to take off.
    assert_eq!(lab.undo("envelope-show", &sp, &sa).unwrap()["undo"]["entry"], false);
    assert_eq!(lab.undo("fx-param-lfo", &lp, &la).unwrap_err().0, "gone");
    assert_eq!(lab.undo("fx-wet", &wp, &wa).unwrap_err().0, "gone");
    state(|s| s.tracks.remove(1));
    assert_eq!(lab.undo("track-automation-mode", &mp, &ma).unwrap_err().0, "gone");
    assert_eq!(lab.undo("fx-container-add", &cp, &ca).unwrap_err().0, "gone");
}

#[test]
fn an_fx_whose_track_reaper_replaced_is_never_read_through_the_old_track() {
    // (7.82 lab, 2026-10-19: REAPER's undo and redo put tracks back as new objects while the extension's watch followed
    // their FX; reading the old track's chain crashed REAPER)
    let lab = chains_lab();
    let last = state(|s| {
        let last = s.tracks.len() - 1;
        s.tracks[last].fx = vec![plugin("{LAST}", "VST: ReaComp (Cockos)")];
        s.state_count += 1;
        last
    });
    let api = fake::api();
    let reaper = Reaper::new(&api);
    let proj = reaper.current_project().unwrap().ptr;
    assert!(lab.reads.fx_by_guid(&reaper, proj, "{LAST}").is_ok());
    // The last track goes: the handle the registry kept is no track of REAPER's any more.
    state(|s| s.tracks.truncate(last));
    assert!(lab.reads.fx_by_guid(&reaper, proj, "{LAST}").is_err());
    assert_eq!(state(|s| s.dead_handle_reads), 0, "no chain read through the track REAPER no longer has");
}

// (read by the state test)
#[allow(dead_code)]
fn unused(_: Display, _: FakeItem, _: FakeTake, _: FakePoint, _: FakeAutoItem) {}

// ----- what HISTORY draws (m4-tui) -----

#[test]
fn parameter_moves_and_a_lanes_points_are_told_to_history_in_reapers_words() {
    let lab = chains_lab();
    state(|s| s.tracks[1].fx.push(reaeq("{K-EQ}")));
    let input = json!({"ref": fx_ref("{K-EQ}"), "values": [{"param": 0, "value": "120 Hz"}, {"param": 5, "normalized": 0.75}]});
    let (_, applied) = lab.make("fx-params", input);
    let values = applied["display"]["values"].as_array().unwrap();
    assert_eq!(values.len(), 2);
    assert_eq!((values[0]["name"].as_str(), values[0]["toText"].as_str()), (Some("Freq-Band 1"), Some("120.0 Hz")));
    assert_eq!((values[1]["from"].as_f64(), values[1]["to"].as_f64(), values[1]["max"].as_f64()), (Some(0.5), Some(0.75), Some(1.0)));
    assert!(values[1]["fromText"].is_string(), "what it showed before, too");
    let (_, applied) = lab.make("fx-param", json!({"ref": fx_ref("{K-IN}"), "param": "Gain", "value": "-6 dB"}));
    let moved = &applied["display"]["values"][0];
    assert_eq!((moved["name"].as_str(), moved["toText"].as_str()), (Some("Gain"), Some("-6.0 dB")));
    assert_eq!(moved["from"], 0.5);

    let lab = envelope_lab();
    let input = json!({"ref": KICK_VOLUME, "points": [{"position": 0.5, "value": -12}]});
    let (_, applied) = lab.make("envelope-points-add", input);
    let curve = &applied["display"]["curve"];
    assert_eq!((curve["parameter"].as_str(), curve["min"].as_f64(), curve["max"].as_f64()), (Some("Volume"), Some(-60.0), Some(12.0)));
    let (before, after) = (curve["before"].as_array().unwrap(), curve["after"].as_array().unwrap());
    assert_eq!(after.len(), before.len() + 1);
    let added = after.iter().find(|point| point[0] == 0.5).expect("the new point, at its time");
    assert!((added[1].as_f64().unwrap() + 12.0).abs() < 0.01, "in dB, as the producer asked it: {added}");
    // (review U20) And in words, for a picture drawn as words: what the points move between, in dB, and where.
    assert_eq!(curve["words"], "-12 to 0 dB over bars 1–2");
}
