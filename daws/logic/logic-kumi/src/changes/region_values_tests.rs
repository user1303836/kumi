//! Deliberate synthetic complete AX facts. These tests do not claim that the original recordings exposed them,
//! or that a current running Logic has accepted any edit.
use super::*;
use crate::ax::{strings::Strings, Ax};
use crate::helper::{Helper, HelperError};
use crate::project::Project;
use std::time::Duration;

const PATH: &str = "/music/Regions.logicx";
const NAME: &str = "6.0.0";
const LOOP: &str = "6.0.2.1";

struct Native {
    headers: Value,
    contents: Value,
    mixer: Value,
    inspector: Value,
    windows: Value,
    calls: Vec<(String, Value)>,
    writes: usize,
    fault: Option<(usize, HelperError)>,
    malformed: Option<usize>,
    edit_after_set: bool,
    eager_name: bool,
    content_reads: usize,
    edit_on_read: Option<usize>,
    fail_read_after_write: bool,
}

impl Native {
    fn new() -> Self {
        Self {
            headers: json!({"i":"h","role":"AXGroup","c":[{"role":"AXLayoutItem","desc":"Track 1 “Owner”","c":[{"role":"AXTextField","desc":"Owner"}]}]}),
            contents: json!({"i":"c","role":"AXGroup","c":[{"role":"AXLayoutArea","desc":"Track 1 “Owner”","c":[
                {"role":"AXLayoutItem","desc":"Keys","selected":true,"help":"Region starts at 1 bar  and ends at 5 bars , MIDI region. Contains MIDI note and controller events.","c":[{"role":"AXTextField","desc":"Keys"},{"role":"AXHandle","desc":"loop"}]}
            ]}]}),
            mixer: json!({"i":"m","role":"AXLayoutArea","c":[{"role":"AXLayoutItem","c":[
                {"role":"AXTextField","desc":"name","value":"Owner"},{"role":"AXButton","help":"Instrument slot","desc":"instrument"}
            ]}]}),
            inspector: json!({"i":"6","role":"AXGroup","c":[{"i":"6.0","role":"AXGroup","c":[
                {"i":NAME,"role":"AXTextField","desc":"Keys","value":"Keys","enabled":true,"set":["AXValue"],"act":["AXConfirm"]},
                {"i":"6.0.1","role":"AXStaticText","value":"Region:"},
                {"i":"6.0.2","role":"AXRow","c":[{"i":"6.0.2.0","role":"AXTextField","value":"Loop:"},
                    {"i":LOOP,"role":"AXCheckBox","value":0,"valueDesc":"off","enabled":true,"act":["AXPress"]}]},
                {"i":"6.0.3","role":"AXRow","c":[{"role":"AXPopUpButton","value":"Quantize"},{"role":"AXPopUpButton","value":"Off"}]}
            ]}]}),
            windows: json!({"windows":[{"role":"AXWindow","subrole":"AXStandardWindow","title":"Regions.logicx - Tracks","main":true,"document":format!("file://{PATH}")}]}),
            calls: vec![],
            writes: 0,
            fault: None,
            malformed: None,
            edit_after_set: false,
            eager_name: false,
            content_reads: 0,
            edit_on_read: None,
            fail_read_after_write: false,
        }
    }
    fn item(&mut self) -> &mut Value {
        &mut self.contents["c"][0]["c"][0]
    }
    fn rename(&mut self, name: &str) {
        self.item()["desc"] = name.into();
        self.item()["c"][0]["desc"] = name.into();
        self.inspector["c"][0]["c"][0]["value"] = name.into();
        self.inspector["c"][0]["c"][0]["desc"] = name.into();
    }
    fn edit(&mut self, scenario: usize) {
        match scenario {
            0 => self.rename("Producer replacement"),
            1 => self.item()["help"] = "Region starts at 1 bar and ends at 5 bars, audio region.".into(),
            2 => self.item()["help"] = "Region starts at 1 bar 1 beat 1 division 2 ticks and ends at 5 bars, MIDI region.".into(),
            3 => self.headers["c"][0]["c"][0]["desc"] = "Different owner".into(),
            4 => self.windows["windows"][0]["document"] = "file:///music/Producer.logicx".into(),
            5 => self.inspector["c"][0]["c"][3]["c"][1]["value"] = "1/16".into(),
            6 => self.item()["selected"] = false.into(),
            7 => {
                let mut other = self.item().clone();
                other["selected"] = false.into();
                self.contents["c"][0]["c"].as_array_mut().unwrap().push(other);
            }
            8 => self.item()["more"] = 1.into(),
            9 => self.inspector["c"][0]["c"][2]["c"][1]["value"] = Value::Null,
            10 => self.mixer["c"][0]["c"][1] = json!({"role":"AXButton","help":"Input slot","desc":"Input 1"}),
            11 => self.inspector["c"][0]["c"][0]["value"] = "Unrelated selection".into(),
            12 => self.item()["help"] = "Region starts at 1 bar and ends at 5 bars 9 nonsense, MIDI region.".into(),
            13 => self.inspector["more"] = 1.into(),
            14 => self.inspector["c"][0]["c"][0]["act"] = json!([]),
            15 => self.inspector["c"][0]["c"][2]["c"][1]["i"] = NAME.into(),
            16 => self.item()["help"] = "Region starts at 1 bar 2 beats 3 beats and ends at 5 bars, MIDI region.".into(),
            17 => {
                let mut other = self.item().clone();
                other["desc"] = "Other".into();
                other["c"][0]["desc"] = "Other".into();
                other["help"] = "Region starts at 9 bars and ends at 10 bars, MIDI region.".into();
                self.contents["c"][0]["c"].as_array_mut().unwrap().push(other);
            }
            18 => self.item()["c"][1]["value"] = "unknown".into(),
            _ => unreachable!(),
        }
    }
}

impl Helper for Native {
    fn call(&mut self, op: &str, params: Value) -> Result<Value, HelperError> {
        self.calls.push((op.into(), params.clone()));
        if self.fail_read_after_write && self.writes > 0 && matches!(op, "tree" | "windows") {
            return Err(HelperError::Timeout);
        }
        let path = params["path"].as_str().unwrap_or_default();
        match op {
            "windows" => Ok(self.windows.clone()),
            "tree" => {
                if params["root"] == "menubar" {
                    return Ok(json!({"node":{"role":"AXMenuBar","c":[]}}));
                }
                let node = match path {
                    "5" => {
                        json!({"i":"5","role":"AXGroup","c":[{"role":"AXCheckBox","desc":"Inspector","value":1},{"role":"AXCheckBox","desc":"Mixer","value":1}]})
                    }
                    "h" => self.headers.clone(),
                    "m" => self.mixer.clone(),
                    "6" => self.inspector.clone(),
                    "c" => {
                        self.content_reads += 1;
                        if self.edit_on_read == Some(self.content_reads) {
                            self.edit(2);
                        }
                        self.contents.clone()
                    }
                    _ => panic!("unexpected synthetic tree {path}: {params}"),
                };
                Ok(json!({"node":node}))
            }
            "set" | "action" => {
                self.writes += 1;
                let reply = if path == NAME {
                    if op == "set" {
                        let before = self.inspector["c"][0]["c"][0]["value"].clone();
                        let to = params["value"].as_str().unwrap();
                        self.inspector["c"][0]["c"][0]["value"] = to.into();
                        self.inspector["c"][0]["c"][0]["desc"] = to.into();
                        if self.eager_name {
                            self.rename(to);
                        }
                        if self.edit_after_set {
                            self.edit(2);
                        }
                        json!({"before":before,"after":to})
                    } else {
                        assert_eq!(params["name"], "AXConfirm");
                        let name = self.inspector["c"][0]["c"][0]["value"].as_str().unwrap().to_string();
                        self.rename(&name);
                        json!({"before":name,"after":name})
                    }
                } else {
                    assert_eq!(path, LOOP);
                    assert_eq!(params["name"], "AXPress");
                    let node = &mut self.inspector["c"][0]["c"][2]["c"][1];
                    let before = node["value"].as_i64().unwrap();
                    let after = 1 - before;
                    node["value"] = after.into();
                    node["valueDesc"] = if after == 1 { "on" } else { "off" }.into();
                    if !self.item()["c"][1]["value"].is_null() {
                        self.item()["c"][1]["value"] = after.into();
                        self.item()["c"][1]["valueDesc"] = if after == 1 { "on" } else { "off" }.into();
                    }
                    json!({"before":before,"after":after})
                };
                if let Some((_, fault)) = self.fault.clone().filter(|(at, _)| *at == self.writes) {
                    return Err(fault);
                }
                if self.malformed == Some(self.writes) {
                    return Ok(json!({}));
                }
                Ok(reply)
            }
            _ => panic!("forbidden helper operation {op}"),
        }
    }
    fn event(&mut self, _: Duration) -> Option<Value> {
        None
    }
    fn alive(&self) -> bool {
        true
    }
    fn quit(&mut self) {
        panic!("no helper lifecycle action in a region controller")
    }
}

struct Lab {
    native: Native,
    project: Project,
}
impl Lab {
    fn new() -> Self {
        let mut project = Project::new(Strings::english());
        project.id = Some(crate::logic::project_id(PATH));
        project.path = Some(PATH.into());
        project.identity = "Regions".into();
        project.layout.control_bar = Some("5".into());
        project.layout.headers = Some("h".into());
        project.layout.contents = Some("c".into());
        project.layout.inspector = Some("6".into());
        project.layout.mixer = Some("m".into());
        project.shape = Some(Shape { headers: vec!["Owner".into()], regions: vec![(1, "Keys".into(), 1, 1, 5, 1)] });
        Self { native: Native::new(), project }
    }
    fn with<T>(&mut self, f: impl FnOnce(&mut Ctx<'_, '_>) -> T) -> T {
        let mut ax = Ax::new(&mut self.native, 7);
        f(&mut Ctx { project: &mut self.project, ax: &mut ax })
    }
    fn restore(&mut self, preview: &Preview, applied: &Applied, mode: Mode, discard: bool) -> Done<Restored> {
        self.with(|ctx| {
            restore(
                ctx,
                &Restore {
                    target: applied.target.as_ref().unwrap(),
                    prior: &preview.prior,
                    applied: &applied.applied,
                    created: None,
                    material: applied.material.as_ref(),
                    discard,
                    desc: "Kumi: undo region property",
                    key: "region".into(),
                },
                mode,
            )
        })
    }
}
fn input(mode: Mode) -> Value {
    match mode {
        Mode::Rename => json!({"ref":"1:region:1","name":"Renamed"}),
        Mode::Loop => json!({"ref":"1:region:1","loop":true}),
    }
}

#[test]
fn the_two_properties_restore_by_full_value_without_selection_or_navigation_and_rename_returns_its_new_epoch_target() {
    for mode in [Mode::Rename, Mode::Loop] {
        for eager in [false, true] {
            let mut lab = Lab::new();
            lab.native.eager_name = eager;
            if mode == Mode::Loop && eager {
                lab.native.item()["c"][1]["value"] = 0.into();
                lab.native.item()["c"][1]["valueDesc"] = "off".into();
            }
            let preview = lab.with(|ctx| super::preview(ctx, &input(mode), mode)).ok().expect("complete synthetic preview");
            assert_eq!(lab.native.writes, 0);
            let applied = lab.with(|ctx| apply(ctx, &input(mode), &preview.fence, mode)).ok().expect("complete synthetic apply");
            assert_eq!(applied.applied["identity"], preview.prior["identity"]);
            assert_eq!(applied.applied[mode.property()], input(mode)[mode.property()]);
            assert_eq!(applied.target.as_ref().unwrap()["ref"], if mode == Mode::Rename { "2:region:1" } else { "1:region:1" });
            assert_eq!(applied.material.as_ref().unwrap(), &json!({"undo":{"mode":"value"}}));
            let back = lab.restore(&preview, &applied, mode, false).ok().expect("complete synthetic restore");
            assert_eq!(back.restored, preview.prior);
            assert!(!back.undone_in_logic);
            let writes = lab.native.writes;
            assert_eq!(lab.restore(&preview, &applied, mode, true).ok().unwrap().restored, preview.prior);
            assert_eq!(lab.native.writes, writes, "already-restored still observes the full identity without a write");
            assert!(lab.native.calls.iter().all(|(op, p)| op == "tree"
                || op == "windows"
                || matches!(op.as_str(), "set" | "action") && [NAME, LOOP].contains(&p["path"].as_str().unwrap())));
        }
    }
    // Exercise the actual public applied-target passthrough, including the post-rename epoch in the host's answer.
    let mut lab = Lab::new();
    let mut changes = super::super::Changes::default();
    let preview = lab.with(|ctx| changes.preview(ctx, &json!({"kind":"region-rename","input":input(Mode::Rename),"epoch":"1"}))).unwrap();
    let applied = lab
        .with(|ctx| {
            changes.apply(
                ctx,
                &json!({"kind":"region-rename","input":input(Mode::Rename),"fence":preview["fence"],"epoch":"1","desc":"Kumi: rename"}),
            )
        })
        .unwrap();
    assert_eq!(applied["epoch"], "2");
    assert_eq!(applied["target"], json!({"ref":"2:region:1","kind":"region"}));
}

#[test]
fn full_observable_fences_refuse_replacements_unknowns_and_producer_edits_even_for_discard_or_already_restored_values() {
    for mode in [Mode::Rename, Mode::Loop] {
        for scenario in 0..19 {
            if mode == Mode::Loop && scenario == 14 {
                continue;
            } // Name Confirm isn't the Loop action's readiness.
            let mut lab = Lab::new();
            let preview = lab.with(|ctx| super::preview(ctx, &input(mode), mode)).ok().unwrap();
            let applied = lab.with(|ctx| apply(ctx, &input(mode), &preview.fence, mode)).ok().unwrap();
            lab.native.edit(scenario);
            let writes = lab.native.writes;
            for discard in [false, true] {
                assert!(lab.restore(&preview, &applied, mode, discard).is_err(), "{mode:?}/{scenario}");
            }
            assert_eq!(lab.native.writes, writes, "{mode:?}/{scenario}");
            // Matching the old property value is insufficient when the identity changed in the meantime.
            let mut lab = Lab::new();
            let preview = lab.with(|ctx| super::preview(ctx, &input(mode), mode)).ok().unwrap();
            let applied = lab.with(|ctx| apply(ctx, &input(mode), &preview.fence, mode)).ok().unwrap();
            if mode == Mode::Rename {
                lab.native.rename("Keys");
            } else {
                lab.native.inspector["c"][0]["c"][2]["c"][1]["value"] = 0.into();
                lab.native.inspector["c"][0]["c"][2]["c"][1]["valueDesc"] = "off".into();
            }
            lab.native.edit(scenario);
            let writes = lab.native.writes;
            assert!(lab.restore(&preview, &applied, mode, true).is_err(), "already restored {mode:?}/{scenario}");
            assert_eq!(lab.native.writes, writes);
            let mut lab = Lab::new();
            let preview = lab.with(|ctx| super::preview(ctx, &input(mode), mode)).ok().unwrap();
            lab.native.edit(scenario);
            assert!(lab.with(|ctx| apply(ctx, &input(mode), &preview.fence, mode)).is_err(), "apply {mode:?}/{scenario}");
            assert_eq!(lab.native.writes, 0);
        }
    }
    let mut lab = Lab::new();
    let preview = lab.with(|ctx| super::preview(ctx, &input(Mode::Rename), Mode::Rename)).ok().unwrap();
    lab.native.edit_on_read = Some(lab.native.content_reads + 3); // Apply observes twice, fresh dispatch guard reads next.
    assert!(lab.with(|ctx| apply(ctx, &input(Mode::Rename), &preview.fence, Mode::Rename)).is_err());
    assert_eq!(lab.native.writes, 0);
    for mode in [Mode::Rename, Mode::Loop] {
        let mut lab = Lab::new();
        let full = format!("{}A", "x".repeat(1100));
        lab.native.rename(&full);
        lab.project.shape.as_mut().unwrap().regions[0].1 = rows::clip(&full);
        let preview = lab.with(|ctx| super::preview(ctx, &input(mode), mode)).ok().unwrap();
        assert_eq!(preview.prior["name"], full);
        lab.native.rename(&format!("{}B", "x".repeat(1100)));
        assert!(lab.with(|ctx| apply(ctx, &input(mode), &preview.fence, mode)).is_err());
        assert_eq!(lab.native.writes, 0, "equal clipped prefixes never identify different full names");
    }
    let mut lab = Lab::new();
    assert!(lab
        .with(|ctx| restore(
            ctx,
            &Restore {
                target: &json!({"ref":"1:region:1"}),
                prior: &json!({"name":"Old"}),
                applied: &json!({"name":"New"}),
                created: None,
                material: None,
                discard: true,
                desc: "undo",
                key: "x".into()
            },
            Mode::Rename
        ))
        .is_err());
    assert!(lab.native.calls.is_empty());
    // Shape is validated without native calls; only structurally complete records proceed to fresh semantic
    // comparison. Empty version-one containers must not turn a malformed journal into a UI observation request.
    for mode in [Mode::Rename, Mode::Loop] {
        let mut lab = Lab::new();
        let complete = lab.with(|ctx| super::preview(ctx, &input(mode), mode)).ok().unwrap().prior;
        for (pointer, replacement) in [
            ("/identity/project", json!({})),
            ("/identity/project/path", Value::Null),
            ("/identity/project/id", Value::Null),
            ("/identity/project/name", json!("")),
            ("/identity/headers", json!([])),
            ("/identity/lanes", json!([])),
            ("/identity/headers/0", json!({})),
            ("/identity/lanes/0/node", json!({})),
            ("/identity/lanes/0/regions", json!([])),
            ("/identity/lanes/0/regions/0/start", json!({})),
            ("/identity/owner", json!({})),
            ("/identity/owner/strip", json!({})),
            ("/identity/owner/strip/children", json!([])),
            ("/identity/inspector", json!({})),
            ("/identity/inspector/children", json!([])),
            ("/identity/target", json!(0)),
            ("/identity/target", json!(99)),
        ] {
            let mut malformed = complete.clone();
            *malformed.pointer_mut(pointer).unwrap() = replacement;
            let mut lab = Lab::new();
            assert!(lab.with(|ctx| apply(ctx, &input(mode), &malformed, mode)).is_err(), "{mode:?} {pointer}");
            assert!(
                lab.with(|ctx| restore(
                    ctx,
                    &Restore {
                        target: &json!({"ref":"1:region:1"}),
                        prior: &malformed,
                        applied: &malformed,
                        created: None,
                        material: None,
                        discard: true,
                        desc: "undo",
                        key: "x".into()
                    },
                    mode
                ))
                .is_err(),
                "{mode:?} {pointer}"
            );
            assert!(lab.native.calls.is_empty(), "malformed {mode:?} {pointer} never calls the helper");
        }
    }
}

#[test]
fn unverified_or_partial_region_dispatches_stop_without_confirm_retry_compensation_or_other_helper_calls() {
    for mode in [Mode::Rename, Mode::Loop] {
        for at in 1..=if mode == Mode::Rename { 2 } else { 1 } {
            for fault in [
                HelperError::Timeout,
                HelperError::Gone("lost reply".into()),
                HelperError::Refused {
                    code: "attributeUnsupported".into(),
                    reply: json!({"before":0,"after":1,"error":"attributeUnsupported"}),
                },
            ] {
                let mut lab = Lab::new();
                let preview = lab.with(|ctx| super::preview(ctx, &input(mode), mode)).ok().unwrap();
                lab.native.fault = Some((at, fault));
                assert_eq!(lab.with(|ctx| apply(ctx, &input(mode), &preview.fence, mode)).err().unwrap().0, "uncertain");
                assert_eq!(lab.native.writes, at);
                assert!(matches!(lab.native.calls.last().unwrap().0.as_str(), "set" | "action"));
            }
        }
    }
    for mode in [Mode::Rename, Mode::Loop] {
        let mut lab = Lab::new();
        let preview = lab.with(|ctx| super::preview(ctx, &input(mode), mode)).ok().unwrap();
        lab.native.malformed = Some(1);
        assert_eq!(lab.with(|ctx| apply(ctx, &input(mode), &preview.fence, mode)).err().unwrap().0, "uncertain");
        assert_eq!(lab.native.writes, 1);
        assert!(matches!(lab.native.calls.last().unwrap().0.as_str(), "set" | "action"));
        let mut lab = Lab::new();
        let preview = lab.with(|ctx| super::preview(ctx, &input(mode), mode)).ok().unwrap();
        let applied = lab.with(|ctx| apply(ctx, &input(mode), &preview.fence, mode)).ok().unwrap();
        lab.native.fault = Some((lab.native.writes + 1, HelperError::Timeout));
        let before = lab.native.writes;
        assert_eq!(lab.restore(&preview, &applied, mode, true).err().unwrap().0, "uncertain");
        assert_eq!(lab.native.writes, before + 1);
    }
    let mut lab = Lab::new();
    let preview = lab.with(|ctx| super::preview(ctx, &input(Mode::Rename), Mode::Rename)).ok().unwrap();
    lab.native.edit_after_set = true;
    assert_eq!(lab.with(|ctx| apply(ctx, &input(Mode::Rename), &preview.fence, Mode::Rename)).err().unwrap().0, "uncertain");
    assert_eq!(lab.native.writes, 1, "changed complete identity after SET never receives Confirm");
    for mode in [Mode::Rename, Mode::Loop] {
        let mut lab = Lab::new();
        let preview = lab.with(|ctx| super::preview(ctx, &input(mode), mode)).ok().unwrap();
        lab.native.fail_read_after_write = true;
        assert_eq!(lab.with(|ctx| apply(ctx, &input(mode), &preview.fence, mode)).err().unwrap().0, "uncertain");
        assert_eq!(lab.native.writes, 1);
        assert_eq!(lab.native.calls.last().unwrap().0, "windows", "a failed readback stops before any further helper operation");
    }
}

#[test]
fn only_the_known_complete_position_grammar_is_accepted_and_omitted_one_units_keep_their_full_tuple() {
    let (start, end, midi) = bounds("Region starts at 1 bar and ends at 5 bars, MIDI region.").unwrap();
    assert_eq!(start, Position::START);
    assert_eq!(end, Position { bar: 5, ..Position::START });
    assert!(midi);
    let (start, _, _) = bounds("Region starts at 2 bars 3 beats 2 divisions 121 ticks and ends at 5 bars, audio region.").unwrap();
    assert_eq!(start, Position { bar: 2, beat: 3, division: 2, tick: 121 });
    for bad in [
        "Region starts at 1 bar, MIDI region.",
        "Region starts at 1 bar 3 nonsense and ends at 5 bars, MIDI region.",
        "Region starts at 1 bar 2 beats 1 beat and ends at 5 bars, MIDI region.",
        "Region starts at 1 bar and ends at 5 bars trailing, MIDI region.",
    ] {
        assert!(bounds(bad).is_err(), "{bad}");
    }
}
