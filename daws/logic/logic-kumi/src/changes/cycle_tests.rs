//! Controlled synthetic AX facts, not recordings or acceptance from a running Logic. The helper models only
//! locator stepping and the Cycle button so a refused or uncertain operation cannot be hidden by cleanup.

use super::*;
use crate::ax::strings::Strings;
use crate::ax::Ax;
use crate::helper::{Helper, HelperError};
use crate::project::Project;
use std::time::Duration;

const PATH: &str = "/music/Loop.logicx";
const LEFT: &str = "5.0.3";
const RIGHT: &str = "5.0.4";

fn at_mut<'a>(value: &'a mut Value, path: &str) -> Option<&'a mut Value> {
    if value["i"].as_str() == Some(path) {
        return Some(value);
    }
    value.get_mut("c")?.as_array_mut()?.iter_mut().find_map(|child| at_mut(child, path))
}

fn scrubber(path: &str, values: [u32; 4], editable: bool) -> Value {
    let children: Vec<_> = ["bar", "beat", "division", "tick"]
        .into_iter()
        .zip(values)
        .enumerate()
        .map(|(index, (label, value))| {
            let mut node = json!({"i": format!("{path}.{index}"), "role": "AXSlider", "desc": label, "value": value, "enabled": true});
            if editable {
                node["set"] = json!(["AXValue"]);
                node["act"] = json!(["AXIncrement", "AXDecrement"]);
            }
            node
        })
        .collect();
    let mut group = json!({"i": path, "role": "AXGroup", "c": children});
    if path == "5.0.1" {
        group["desc"] = "Playhead Position".into();
    }
    group
}

fn time_row(path: &str, position: [u32; 4], numerator: u32) -> Value {
    json!({"i": path, "role": "AXRow", "c": [
        {"role": "AXCell", "c": [scrubber(&format!("{path}.0.0"), position, false)]},
        {"role": "AXCell", "c": [{"role": "AXCell", "desc": "Time"}]},
        {"role": "AXCell", "c": [{"role": "AXSlider", "value": numerator}, {"role": "AXPopUpButton", "value": "/4"}]}
    ]})
}

#[derive(Clone)]
enum DispatchFault {
    Helper(HelperError),
    MissingAfter,
    WrongAfter,
}

#[derive(Clone)]
enum ProducerEdit {
    LeftBeat,
    Meter,
    Division,
    Project,
}

struct Synthetic {
    bar: Value,
    tabs: Value,
    list: Value,
    windows: Value,
    calls: Vec<(String, Value)>,
    writes: usize,
    bar_reads: usize,
    fault: Option<(usize, DispatchFault)>,
    after_write: Option<(usize, ProducerEdit)>,
    before_bar: Option<(usize, ProducerEdit)>,
    fail_read_after_write: bool,
    reset_lower: bool,
}

impl Synthetic {
    fn new() -> Self {
        Self {
            bar: json!({"i": "5", "role": "AXGroup", "desc": "Control Bar", "c": [
                {"i": "5.0", "role": "AXGroup", "desc": "Control Bar", "c": [
                    {"i": "5.0.0", "role": "AXPopUpButton", "desc": "Display Mode", "value": "Custom"},
                    scrubber("5.0.1", [1,1,1,1], false),
                    scrubber(LEFT, [1,1,1,1], true), scrubber(RIGHT, [5,1,1,1], true),
                    {"i": "5.0.5", "role": "AXSlider", "desc": "Tempo", "value": 120},
                    {"i": "5.0.7", "role": "AXPopUpButton", "desc": "Time Signature", "value": "4/4"},
                    {"i": "5.0.8", "role": "AXPopUpButton", "value": "/16"}
                ]},
                {"i": "5.1", "role": "AXCheckBox", "desc": "Cycle", "value": 0, "enabled": true, "act": ["AXPress"]},
                {"i": "5.2", "role": "AXCheckBox", "desc": "Record", "value": 0},
                {"i": "5.3", "role": "AXCheckBox", "desc": "Play", "value": 0},
                {"i": "5.16", "role": "AXCheckBox", "desc": "List Editors", "value": 1}
            ]}),
            tabs: json!({"i": "8", "role": "AXRadioGroup", "c": [
                {"role": "AXRadioButton", "desc": "Event", "value": 0},
                {"role": "AXRadioButton", "desc": "Signature", "value": 1}
            ]}),
            list: json!({"i": "9", "role": "AXGroup", "c": [
                {"i": "9.0", "role": "AXTable", "c": [
                    {"role": "AXColumn", "id": "Position"}, {"role": "AXColumn", "id": "Type"}, {"role": "AXColumn", "id": "Val"},
                    time_row("9.0.3", [1,1,1,1], 4)
                ]}
            ]}),
            windows: json!({"windows": [{"role": "AXWindow", "subrole": "AXStandardWindow", "title": "Loop.logicx - Tracks",
                "main": true, "focused": true, "document": format!("file://{PATH}") }], "listed": ["Loop.logicx - Tracks"]}),
            calls: vec![],
            writes: 0,
            bar_reads: 0,
            fault: None,
            after_write: None,
            before_bar: None,
            fail_read_after_write: false,
            reset_lower: false,
        }
    }

    fn edit(&mut self, edit: ProducerEdit) {
        match edit {
            ProducerEdit::LeftBeat => at_mut(&mut self.bar, "5.0.3.1").unwrap()["value"] = 2.into(),
            ProducerEdit::Meter => {
                at_mut(&mut self.bar, "5.0.7").unwrap()["value"] = "3/4".into();
                self.list["c"][0]["c"][3] = time_row("9.0.3", [1, 1, 1, 1], 3);
            }
            ProducerEdit::Division => at_mut(&mut self.bar, "5.0.8").unwrap()["value"] = "/8".into(),
            ProducerEdit::Project => self.windows["windows"][0]["document"] = "file:///music/Producer.logicx".into(),
        }
    }

    fn range(&mut self, left: [u32; 4], right: [u32; 4], on: bool) {
        *at_mut(&mut self.bar, LEFT).unwrap() = scrubber(LEFT, left, true);
        *at_mut(&mut self.bar, RIGHT).unwrap() = scrubber(RIGHT, right, true);
        at_mut(&mut self.bar, "5.1").unwrap()["value"] = u32::from(on).into();
    }

    fn assert_only_cycle_writes(&self) {
        for (op, params) in &self.calls {
            assert!(matches!(op.as_str(), "windows" | "tree" | "set" | "action"), "unexpected helper operation {op}");
            if matches!(op.as_str(), "set" | "action") {
                let path = params["path"].as_str().unwrap();
                assert!(path.starts_with(&format!("{LEFT}.")) || path.starts_with(&format!("{RIGHT}.")) || path == "5.1");
            }
        }
    }
}

impl Helper for Synthetic {
    fn call(&mut self, op: &str, params: Value) -> Result<Value, HelperError> {
        self.calls.push((op.into(), params.clone()));
        if self.fail_read_after_write && self.writes > 0 && matches!(op, "windows" | "tree") {
            return Err(HelperError::Timeout);
        }
        let path = params["path"].as_str().unwrap_or_default();
        match op {
            "windows" => Ok(self.windows.clone()),
            "tree" => {
                let node = match path {
                    "5" => {
                        self.bar_reads += 1;
                        if let Some((at, edit)) = self.before_bar.clone().filter(|(at, _)| *at == self.bar_reads) {
                            assert_eq!(at, self.bar_reads);
                            self.edit(edit);
                        }
                        &self.bar
                    }
                    "8" => &self.tabs,
                    "9" => &self.list,
                    _ => panic!("unexpected synthetic subtree {path}"),
                };
                Ok(json!({"node": node}))
            }
            "set" | "action" => {
                self.writes += 1;
                let after = if path == "5.1" {
                    assert_eq!(params["name"], "AXPress");
                    let node = at_mut(&mut self.bar, path).unwrap();
                    let next = if node["value"] == 0 { 1 } else { 0 };
                    node["value"] = next.into();
                    next
                } else {
                    let node = at_mut(&mut self.bar, path).expect("a known locator control");
                    let now = node["value"].as_i64().unwrap();
                    let delta = if op == "set" {
                        assert_eq!(params["attr"], "AXValue");
                        (params["value"].as_i64().unwrap() - now).signum()
                    } else {
                        match params["name"].as_str().unwrap() {
                            "AXIncrement" => 10,
                            "AXDecrement" => -10,
                            other => panic!("unexpected synthetic action {other}"),
                        }
                    };
                    let next = now + delta;
                    node["value"] = next.into();
                    if self.reset_lower {
                        let (group, segment) = path.rsplit_once('.').unwrap();
                        for index in segment.parse::<usize>().unwrap() + 1..4 {
                            at_mut(&mut self.bar, &format!("{group}.{index}")).unwrap()["value"] = 1.into();
                        }
                    }
                    next
                };
                if let Some((_, edit)) = self.after_write.clone().filter(|(at, _)| *at == self.writes) {
                    self.edit(edit);
                }
                match self.fault.clone().filter(|(at, _)| *at == self.writes).map(|(_, fault)| fault) {
                    Some(DispatchFault::Helper(error)) => Err(error),
                    Some(DispatchFault::MissingAfter) => Ok(json!({})),
                    Some(DispatchFault::WrongAfter) => Ok(json!({"after": after + 1})),
                    None => Ok(json!({"after": after})),
                }
            }
            other => panic!("unexpected helper operation {other}"),
        }
    }
    fn event(&mut self, _: Duration) -> Option<Value> {
        None
    }
    fn alive(&self) -> bool {
        true
    }
    fn quit(&mut self) {
        panic!("the controller must not quit a helper")
    }
}

struct Harness {
    helper: Synthetic,
    project: Project,
}

impl Harness {
    fn new() -> Self {
        let mut project = Project::new(Strings::english());
        project.id = Some(logic::project_id(PATH));
        project.path = Some(PATH.into());
        project.identity = "Loop".into();
        project.layout.control_bar = Some("5".into());
        project.layout.headers = Some("7.0".into());
        project.layout.list_tabs = Some("8".into());
        project.layout.list = Some("9".into());
        Self { helper: Synthetic::new(), project }
    }

    fn with<T>(&mut self, f: impl FnOnce(&mut Ctx<'_, '_>) -> T) -> T {
        let mut ax = Ax::new(&mut self.helper, 7);
        f(&mut Ctx { project: &mut self.project, ax: &mut ax })
    }

    fn restore(&mut self, prior: &Value, applied: &Value, discard: bool) -> Done<Restored> {
        self.with(|ctx| {
            restore(
                ctx,
                &Restore {
                    target: &json!({"ref": null, "kind": "project"}),
                    prior,
                    applied,
                    created: None,
                    material: None,
                    discard,
                    desc: "Kumi: undo cycle",
                    key: "loop-range".into(),
                },
            )
        })
    }
}

#[test]
fn a_known_constant_meter_cycle_is_changed_and_restored_by_complete_value_and_off_keeps_its_locators() {
    for reset_lower in [false, true] {
        let mut h = Harness::new();
        h.helper.reset_lower = reset_lower;
        let input = json!({"startBeats": 16, "endBeats": 32});
        let previewed = h.with(|ctx| preview(ctx, &input)).ok().expect("known synthetic preview");
        assert_eq!(h.helper.writes, 0);
        assert_eq!(previewed.prior, previewed.fence);
        assert_eq!(previewed.prior["start"], Position::START.to_json());
        assert_eq!(previewed.prior["timebase"]["signature"], json!([{"position": Position::START.to_json(), "meter": [4,4]}]));
        let applied = h.with(|ctx| apply(ctx, &input, &previewed.fence)).ok().expect("known synthetic apply");
        assert_eq!(public(&applied.applied), json!({"on": true, "startBeats": 16.0, "endBeats": 32.0}));
        assert_eq!(applied.material, Some(json!({"undo": {"mode": "value"}})));
        let restored = h.restore(&previewed.prior, &applied.applied, false).ok().expect("known synthetic restore");
        assert_eq!(restored.restored, previewed.prior);
        assert!(!restored.undone_in_logic);
        h.helper.range([5, 1, 1, 1], [9, 1, 1, 1], true);
        let off = json!({"startBeats": 0, "endBeats": 0});
        let previewed = h.with(|ctx| preview(ctx, &off)).ok().expect("off preview");
        let before = h.helper.writes;
        let applied = h.with(|ctx| apply(ctx, &off, &previewed.fence)).ok().expect("off apply");
        assert_eq!(h.helper.writes, before + 1, "only the Cycle button changes");
        assert_eq!(applied.applied["start"], previewed.prior["start"]);
        assert_eq!(applied.applied["end"], previewed.prior["end"]);
        assert_eq!(applied.applied["on"], false);
        let before = h.helper.writes;
        let restored = h.restore(&previewed.prior, &applied.applied, false).ok().expect("off restore");
        assert_eq!(restored.restored, previewed.prior);
        assert_eq!(h.helper.writes, before + 1);
        h.helper.assert_only_cycle_writes();
    }
    // A known non-4/4 numerator and sub-beat divisions use the captured timebase, including a control that
    // resets lower segments when the bar changes. These are deliberate fixture facts, not native observations.
    let mut h = Harness::new();
    h.helper.edit(ProducerEdit::Meter);
    h.helper.reset_lower = true;
    let input = json!({"startBeats": 3.25 + 1.0/960.0, "endBeats": 9.5 + 1.0/960.0});
    let previewed = h.with(|ctx| preview(ctx, &input)).ok().expect("known 3/4 fixture");
    let applied = h.with(|ctx| apply(ctx, &input, &previewed.fence)).ok().expect("known fractional fixture");
    assert_eq!(applied.applied["timebase"]["meter"], json!([3, 4]));
    assert_eq!(applied.applied["start"], json!({"bar": 2,"beat": 1,"division": 2,"tick": 2}));
    assert_eq!(applied.applied["end"], json!({"bar": 4,"beat": 1,"division": 3,"tick": 2}));
    assert_eq!(public(&applied.applied), json!({"on": true,"startBeats": 3.251042,"endBeats": 9.501042}));
    assert_eq!(h.restore(&previewed.prior, &applied.applied, false).ok().unwrap().restored, previewed.prior);
    h.helper.assert_only_cycle_writes();
}

#[test]
fn missing_or_unsupported_native_facts_and_unbounded_ranges_refuse_before_any_project_write() {
    for scenario in 0..19 {
        let mut h = Harness::new();
        match scenario {
            0 => at_mut(&mut h.helper.bar, "5.0.0").unwrap()["value"] = "Beats & Project".into(),
            1 => at_mut(&mut h.helper.bar, "5.0.8").unwrap()["value"] = "/8".into(),
            2 => at_mut(&mut h.helper.bar, "5.0.3.0").unwrap()["value"] = Value::Null,
            3 => at_mut(&mut h.helper.bar, "5.0.4.1").unwrap()["value"] = 1.5.into(),
            4 => at_mut(&mut h.helper.bar, "5.0.3.0").unwrap()["set"] = json!([]),
            5 => at_mut(&mut h.helper.bar, "5.1").unwrap()["act"] = json!([]),
            6 => at_mut(&mut h.helper.bar, "5.3").unwrap()["value"] = 1.into(),
            7 => at_mut(&mut h.helper.bar, "5.2").unwrap()["value"] = 1.into(),
            8 => h.helper.list["c"][0]["more"] = 1.into(),
            9 => h.helper.list["c"][0]["c"].as_array_mut().unwrap().push(time_row("9.0.4", [9, 1, 1, 1], 4)),
            10 => h.helper.list["c"][0]["c"][3]["c"][0]["c"][0]["c"].as_array_mut().unwrap().pop().map(|_| ()).unwrap(),
            11 => h.helper.list["c"][0]["c"][3]["c"][1]["c"][0]["desc"] = "Unknown".into(),
            12 => h.helper.windows["windows"][0]["document"] = Value::Null,
            13 => {
                h.helper.edit(ProducerEdit::Meter);
                at_mut(&mut h.helper.bar, "5.0.7").unwrap()["value"] = "6/8".into();
            }
            14 => h.helper.tabs["c"][1]["value"] = 0.into(),
            15 => at_mut(&mut h.helper.bar, "5.0.4.0").unwrap()["i"] = "5.0.3.0".into(),
            16 => at_mut(&mut h.helper.bar, "5.1").unwrap()["i"] = "".into(),
            17 => h.helper.list["more"] = 1.into(),
            18 => {
                let another = h.helper.list["c"][0].clone();
                h.helper.list["c"].as_array_mut().unwrap().push(another);
            }
            _ => unreachable!(),
        }
        let result = h.with(|ctx| preview(ctx, &json!({"startBeats": 16, "endBeats": 32})));
        assert!(result.is_err(), "unsupported synthetic case {scenario}");
        assert_eq!(h.helper.writes, 0, "unsupported synthetic case {scenario}");
        h.helper.assert_only_cycle_writes();
    }
    let mut h = Harness::new();
    for input in [
        json!({}),
        json!({"start": 1,"end": 2}),
        json!({"startBeats": -1,"endBeats": 2}),
        json!({"startBeats": 2,"endBeats": 2}),
        json!({"startBeats": 0.0001,"endBeats": 16}),
    ] {
        assert!(h.with(|ctx| preview(ctx, &input)).is_err());
    }
    assert!(h.helper.calls.is_empty(), "invalid input is checked before reading the native UI");
    let error = h.with(|ctx| preview(ctx, &json!({"startBeats": 0,"endBeats": 40000}))).err().expect("bounded refusal");
    assert_eq!(error.0, "bad-params");
    assert_eq!(h.helper.writes, 0);
    // A valid final range can still require an unsafe intermediate locator crossing when a bar step resets
    // its beat/division/tick. Refuse it before the first write, on apply and restore as well as preview.
    let mut h = Harness::new();
    h.helper.range([2, 4, 4, 145], [3, 1, 1, 97], true); // 7.9 to 8.1 beats.
    let input = json!({"startBeats": 7.5,"endBeats": 7.75});
    let fence = h.with(|ctx| observe(ctx)).ok().unwrap().state;
    assert!(h.with(|ctx| preview(ctx, &input)).is_err());
    assert!(h.with(|ctx| apply(ctx, &input, &fence)).is_err());
    let prior = after_input(&input, &fence).unwrap();
    assert!(h.restore(&prior, &fence, true).is_err());
    assert_eq!(h.helper.writes, 0, "a predicted intermediate crossing never starts a partial edit");
    // Tens-plus-ones cost is nonmonotonic in numeric distance. A preserved tick20 ->11 costs nine steps,
    // whereas a reset tick1 ->11 costs one. Taking the larger distance first would budget54 instead of70.
    let mut h = Harness::new();
    h.helper.range([1, 1, 1, 20], [251, 1, 1, 20], false);
    let input = json!({"startBeats": 796.0 + 10.0/960.0,"endBeats": 1596.0 + 10.0/960.0});
    assert_eq!(h.with(|ctx| preview(ctx, &input)).err().expect("worst-case step budget").0, "bad-params");
    assert_eq!(h.helper.writes, 0, "every possible lower-segment behavior must fit the preflight bound");
}

#[test]
fn changed_endpoints_projects_and_timebases_never_pass_apply_or_discard_or_already_restored_fences() {
    for edit in [ProducerEdit::LeftBeat, ProducerEdit::Meter, ProducerEdit::Division, ProducerEdit::Project] {
        let mut h = Harness::new();
        let input = json!({"startBeats": 16,"endBeats": 32});
        let previewed = h.with(|ctx| preview(ctx, &input)).ok().unwrap();
        h.helper.edit(edit.clone());
        assert!(h.with(|ctx| apply(ctx, &input, &previewed.fence)).is_err());
        assert_eq!(h.helper.writes, 0);
        for discard in [false, true] {
            // The locators already match the old prior for the timebase/project cases. That resemblance cannot
            // bypass a different project, meter or division. A changed endpoint must likewise survive discard.
            let applied = after_input(&input, &previewed.prior).unwrap();
            assert!(h.restore(&previewed.prior, &applied, discard).is_err());
            assert_eq!(h.helper.writes, 0);
        }
    }
    let mut h = Harness::new();
    let input = json!({"startBeats": 16,"endBeats": 32});
    let previewed = h.with(|ctx| preview(ctx, &input)).ok().unwrap();
    h.helper.before_bar = Some((h.helper.bar_reads + 2, ProducerEdit::LeftBeat));
    let error = h.with(|ctx| apply(ctx, &input, &previewed.fence)).err().expect("fresh dispatch fence");
    assert_eq!(error.0, "changed");
    assert_eq!(h.helper.writes, 0);
    for discard in [false, true] {
        let mut h = Harness::new();
        for missing in [json!({"on": false,"startBeats": 0,"endBeats": 16}), Value::Null] {
            assert!(h.restore(&missing, &missing, discard).is_err());
        }
        assert!(h.helper.calls.is_empty(), "legacy material refuses even before current-state observations");
    }
}

#[test]
fn unverified_dispatches_stop_immediately_and_partial_changes_never_trigger_cleanup_or_a_resend() {
    for at in [1, 2] {
        for fault in [
            DispatchFault::Helper(HelperError::Timeout),
            DispatchFault::Helper(HelperError::Gone("lost reply".into())),
            DispatchFault::MissingAfter,
            DispatchFault::WrongAfter,
        ] {
            let mut h = Harness::new();
            let input = json!({"startBeats": 16,"endBeats": 32});
            let previewed = h.with(|ctx| preview(ctx, &input)).ok().unwrap();
            h.helper.fault = Some((at, fault));
            let error = h.with(|ctx| apply(ctx, &input, &previewed.fence)).err().expect("uncertain dispatch");
            assert_eq!(error.0, "uncertain");
            assert_eq!(h.helper.writes, at);
            let (last, _) = h.helper.calls.last().unwrap();
            assert!(matches!(last.as_str(), "set" | "action"), "there is no helper call after an unverified dispatch");
            assert_ne!(at_mut(&mut h.helper.bar, "5.0.4.0").unwrap()["value"], 5, "the simulated edit can already have happened");
            h.helper.assert_only_cycle_writes();
        }
    }
    for edit in [ProducerEdit::LeftBeat, ProducerEdit::Meter, ProducerEdit::Division, ProducerEdit::Project] {
        let mut h = Harness::new();
        let input = json!({"startBeats": 16,"endBeats": 32});
        let previewed = h.with(|ctx| preview(ctx, &input)).ok().unwrap();
        h.helper.after_write = Some((1, edit));
        let error = h.with(|ctx| apply(ctx, &input, &previewed.fence)).err().expect("changed post-dispatch facts");
        assert_eq!(error.0, "uncertain");
        assert_eq!(h.helper.writes, 1, "no compensating or cleanup write after changed readback");
    }
    let mut h = Harness::new();
    let input = json!({"startBeats": 16,"endBeats": 32});
    let previewed = h.with(|ctx| preview(ctx, &input)).ok().unwrap();
    h.helper.fail_read_after_write = true;
    assert_eq!(h.with(|ctx| apply(ctx, &input, &previewed.fence)).err().unwrap().0, "uncertain");
    assert_eq!(h.helper.writes, 1);
    assert_eq!(h.helper.calls.last().unwrap().0, "windows", "the failed first read ends the operation");
    for fault in [DispatchFault::Helper(HelperError::Timeout), DispatchFault::MissingAfter] {
        let mut h = Harness::new();
        h.helper.range([1, 1, 1, 1], [5, 1, 1, 1], true);
        let off = json!({"startBeats": 0,"endBeats": 0});
        let previewed = h.with(|ctx| preview(ctx, &off)).ok().unwrap();
        h.helper.fault = Some((1, fault));
        assert_eq!(h.with(|ctx| apply(ctx, &off, &previewed.fence)).err().unwrap().0, "uncertain");
        assert_eq!(h.helper.writes, 1);
        assert_eq!(h.helper.calls.last().unwrap().0, "action", "a lost Cycle-button reply also stops all helper calls");
    }
    let mut h = Harness::new();
    let input = json!({"startBeats": 16,"endBeats": 32});
    let previewed = h.with(|ctx| preview(ctx, &input)).ok().unwrap();
    let applied = h.with(|ctx| apply(ctx, &input, &previewed.fence)).ok().unwrap();
    h.helper.calls.clear();
    h.helper.writes = 0;
    h.helper.fault = Some((1, DispatchFault::Helper(HelperError::Timeout)));
    assert_eq!(h.restore(&previewed.prior, &applied.applied, true).err().unwrap().0, "uncertain");
    assert_eq!(h.helper.writes, 1);
    assert_eq!(h.helper.calls.last().unwrap().0, "set", "restore cannot retry or clean up a lost dispatched reply");
}
