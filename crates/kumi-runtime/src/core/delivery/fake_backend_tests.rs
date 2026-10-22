//! The render_files tool against a fake DAW (no DAW): items grouped and named, regions made in one change, the dry run
//! measured against the names, the render refused until it would write exactly them.
use super::assets::Item;
use super::backend::{DeliveryBackend, NewRegion, Region, Targets, Words};
use super::tool::Delivery;
use crate::core::contracts::KernelTool;
use async_trait::async_trait;
use kumi_common::abort::Signal;
use serde_json::{json, Value};
use std::cell::RefCell;
use std::rc::Rc;

static WORDS: Words = Words {
    app: "Fake",
    region: "region",
    region_pattern: "$region",
    wildcards: "$region",
    sources: "matrix",
    bounds: "regions",
    source: "matrix",
    source_said: "each region's own tracks",
};

#[derive(Default)]
struct Fake {
    items: Vec<Item>,
    regions: RefCell<Vec<Region>>,
    added: RefCell<Vec<Vec<NewRegion>>>,
    there: RefCell<Vec<String>>,
    rendered: RefCell<Vec<(Value, Vec<String>)>>,
    /// A target the fake's pattern adds that nobody planned.
    stray: RefCell<Option<String>>,
}

#[async_trait(?Send)]
impl DeliveryBackend for Rc<Fake> {
    fn words(&self) -> &'static Words {
        &WORDS
    }

    async fn items(&self, tracks: &[String], selected: bool, _signal: &Signal) -> Result<Vec<Item>, String> {
        let _ = selected;
        Ok(self.items.iter().filter(|item| tracks.is_empty() || tracks.contains(&item.track)).cloned().collect())
    }

    async fn regions(&self, _signal: &Signal) -> Result<Vec<Region>, String> {
        Ok(self.regions.borrow().clone())
    }

    async fn add_regions(&self, regions: &[NewRegion], _signal: &Signal) -> Result<Value, String> {
        self.added.borrow_mut().push(regions.to_vec());
        for region in regions {
            self.regions.borrow_mut().push(Region {
                reference: format!("r:{}", region.name),
                name: region.name.clone(),
                start: region.start,
                end: region.end,
            });
        }
        Ok(json!({"change": "c1"}))
    }

    async fn targets(&self, settings: &Value, _signal: &Signal) -> Result<Targets, String> {
        let folder = settings.get("folder").and_then(Value::as_str).unwrap_or("/renders").to_string();
        let pattern = settings["pattern"].as_str().unwrap_or("$region").to_string();
        // (only the regions asked for render, when some are)
        let only: Option<Vec<String>> = settings
            .get("regionRefs")
            .and_then(Value::as_array)
            .map(|refs| refs.iter().filter_map(Value::as_str).map(str::to_owned).collect());
        let mut files: Vec<String> = self
            .regions
            .borrow()
            .iter()
            .filter(|region| only.as_ref().is_none_or(|only| only.contains(&region.reference)))
            .map(|region| format!("{folder}/{}.wav", pattern.replace("$region", &region.name)))
            .collect();
        if let Some(stray) = self.stray.borrow().clone() {
            files.push(format!("{folder}/{stray}.wav"));
        }
        let there = files.iter().filter(|file| self.there.borrow().contains(file)).cloned().collect();
        Ok(Targets { files, folder, there, notes: vec![] })
    }

    async fn render(&self, settings: &Value, expect: &[String], _signal: &Signal) -> Result<Value, String> {
        self.rendered.borrow_mut().push((settings.clone(), expect.to_vec()));
        Ok(json!(expect.iter().map(|file| json!({"file": file, "seconds": 1.0})).collect::<Vec<_>>()))
    }
}

fn item(track: &str, start: f64, end: f64) -> Item {
    Item { reference: format!("i:{track}{start}"), track: format!("t:{track}"), track_name: track.into(), start, end, name: String::new() }
}

async fn run(tool: &Rc<dyn KernelTool>, input: Value) -> Result<Value, String> {
    let done = tool.execute(input.as_object().cloned().unwrap(), Signal::new()).await.unwrap();
    if done.is_error {
        return Err(done.text);
    }
    Ok(serde_json::from_str(&done.text).unwrap())
}

fn footsteps() -> Rc<Fake> {
    Rc::new(Fake {
        items: vec![
            item("Heel", 10.0, 10.4),
            item("Toe", 10.05, 10.3),
            item("Gravel", 10.38, 10.9),
            item("Heel", 12.0, 12.5),
            item("Gravel", 12.1, 12.7),
            item("Heel", 14.0, 14.3),
        ],
        regions: RefCell::new(vec![
            Region { reference: "r:1".into(), name: "Footstep_Gravel_01".into(), start: 0.0, end: 1.0 },
            Region { reference: "r:2".into(), name: "Footstep_Gravel_02".into(), start: 2.0, end: 3.0 },
        ]),
        ..Fake::default()
    })
}

#[tokio::test(flavor = "current_thread")]
async fn assets_are_planned_in_the_projects_convention_regions_made_in_one_change_and_rendered_once_the_dry_run_matches() {
    let fake = footsteps();
    let tool = Delivery::new(fake.clone()).tool();
    let planned = run(&tool, json!({"onTracks": ["t:Heel", "t:Toe", "t:Gravel"], "tailMs": 2500})).await.unwrap();
    let assets = planned["assets"].as_array().unwrap();
    assert_eq!(assets.len(), 3, "{planned}");
    assert_eq!(assets[0]["name"], "Footstep_Gravel_03", "on from the project's highest");
    assert_eq!(assets[0]["layers"], 3);
    assert_eq!(assets[0]["tracks"], json!(["Heel", "Toe", "Gravel"]));
    assert_eq!(assets[0]["to"], 11.99, "the tail stops before the next sound");
    assert_eq!(assets[2]["to"], 16.8, "the last one's tail rings out");
    assert!(planned["naming"].as_str().unwrap().starts_with("Footstep_Gravel_NN"));
    assert!(fake.added.borrow().is_empty(), "a plan makes nothing");

    let made = run(&tool, json!({"action": "regions", "tailMs": 2500})).await.unwrap();
    assert_eq!(made["made"]["change"], "c1");
    assert_eq!(fake.added.borrow().len(), 1, "one change");
    let added = fake.added.borrow()[0].clone();
    assert_eq!(added.len(), 3);
    assert_eq!(added[0].renders, ["t:Heel", "t:Toe", "t:Gravel"], "each region renders its asset's own tracks");
    assert_eq!(added[2].renders, ["t:Heel"]);
    assert!(made["next"].as_str().unwrap().contains("source matrix, the default"), "{made}");
    // The same again: each has its region already.
    let again = run(&tool, json!({"action": "regions", "tailMs": 2500})).await.unwrap_err();
    assert!(again.contains("already"), "{again}");

    // The dry run, against the names planned: only their regions render, not the project's older two.
    let names = json!(["Footstep_Gravel_03", "Footstep_Gravel_04", "Footstep_Gravel_05"]);
    let checked = run(&tool, json!({"action": "check", "names": names, "folder": "/game"})).await.unwrap();
    assert_eq!(checked["ok"], true, "{checked}");
    // Without names, every region renders.
    assert_eq!(run(&tool, json!({"action": "check", "folder": "/game"})).await.unwrap()["count"], 5);
    // A file nobody planned is extra, and stops the render.
    *fake.stray.borrow_mut() = Some("Footstep_Gravel_03_alt".into());
    let refused = run(&tool, json!({"action": "render", "names": names, "folder": "/game"})).await.unwrap_err();
    assert!(refused.starts_with("nothing was rendered") && refused.contains("extra"), "{refused}");
    assert!(fake.rendered.borrow().is_empty());
    *fake.stray.borrow_mut() = None;
    // A region named that isn't there is said.
    let unknown = run(&tool, json!({"action": "check", "regions": ["Footstep_Gravel_09"]})).await.unwrap_err();
    assert!(unknown.contains("no region is named"), "{unknown}");
    let rendered = run(&tool, json!({"action": "render", "names": names, "folder": "/game", "format": "wav", "bits": 24, "tailMs": 500}))
        .await
        .unwrap();
    assert_eq!(rendered["count"], 3);
    let (settings, expect) = fake.rendered.borrow()[0].clone();
    assert_eq!(settings["pattern"], "$region", "a file per region by default");
    assert_eq!(settings["source"], "matrix", "from the DAW's delivery source by default");
    assert_eq!(settings["regionRefs"], json!(["r:Footstep_Gravel_03", "r:Footstep_Gravel_04", "r:Footstep_Gravel_05"]));
    assert!(settings.get("tailMs").is_none(), "a tail is the regions', never the render's: {settings}");
    assert_eq!(settings["bits"], 24);
    assert_eq!(expect, ["/game/Footstep_Gravel_03.wav", "/game/Footstep_Gravel_04.wav", "/game/Footstep_Gravel_05.wav"]);
}

#[tokio::test(flavor = "current_thread")]
async fn a_render_never_writes_over_files_unless_asked_and_a_clash_or_a_bad_name_stops_it() {
    let fake = footsteps();
    fake.regions.borrow_mut().clear();
    let tool = Delivery::new(fake.clone()).tool();
    // No convention to go on from: a name is asked for.
    assert!(run(&tool, json!({})).await.unwrap_err().starts_with("name:"));
    run(&tool, json!({"action": "regions", "name": "UI Click", "pad": 3})).await.unwrap_err();
    run(&tool, json!({"action": "regions", "name": "UI_Click", "pad": 3})).await.unwrap();
    assert_eq!(fake.added.borrow()[0][0].name, "UI_Click_001");
    fake.there.borrow_mut().push("/renders/UI_Click_001.wav".into());
    let refused = run(&tool, json!({"action": "render"})).await.unwrap_err();
    assert!(refused.contains("there already"), "{refused}");
    run(&tool, json!({"action": "render", "overwrite": true})).await.unwrap();
    // A clash the DAW numbers "-001" stops it.
    *fake.stray.borrow_mut() = Some("UI_Click_001-001".into());
    let checked = run(&tool, json!({"action": "check"})).await.unwrap();
    assert_eq!(checked["clashes"], json!(["UI_Click_001-001"]));
    assert!(run(&tool, json!({"action": "render", "overwrite": true})).await.unwrap_err().starts_with("nothing was rendered"));
}

#[tokio::test(flavor = "current_thread")]
async fn assets_with_regions_keep_their_names_only_new_ones_are_numbered_and_a_bed_under_them_is_said() {
    let fake = footsteps();
    // The first two footsteps have their regions from an earlier delivery.
    fake.regions.borrow_mut().extend([
        Region { reference: "r:3".into(), name: "Footstep_Gravel_03".into(), start: 10.0, end: 10.9 },
        Region { reference: "r:4".into(), name: "Footstep_Gravel_04".into(), start: 12.0, end: 12.7 },
    ]);
    let tool = Delivery::new(fake.clone()).tool();
    let planned = run(&tool, json!({})).await.unwrap();
    let names: Vec<&str> = planned["assets"].as_array().unwrap().iter().map(|a| a["name"].as_str().unwrap()).collect();
    assert_eq!(names, ["Footstep_Gravel_03", "Footstep_Gravel_04", "Footstep_Gravel_05"], "{planned}");
    assert!(planned["assets"][0]["region"].as_str().unwrap().starts_with("there already"));
    assert!(planned["assets"][2].get("region").is_none());
    run(&tool, json!({"action": "regions"})).await.unwrap();
    let added = fake.added.borrow()[0].clone();
    assert_eq!(added.iter().map(|r| r.name.as_str()).collect::<Vec<_>>(), ["Footstep_Gravel_05"], "only the new one, next in the row");

    // A first that lands on a name a region has is a problem, and makes nothing.
    let fake = footsteps();
    let tool = Delivery::new(fake.clone()).tool();
    let planned = run(&tool, json!({"first": 2})).await.unwrap();
    assert!(planned["problems"].to_string().contains("a region's name already"), "{planned}");
    assert!(run(&tool, json!({"action": "regions", "first": 2})).await.unwrap_err().contains("won't do"));
    assert!(fake.added.borrow().is_empty());

    // A long item under the footsteps makes them one asset: said, with the way out.
    let mut bed = footsteps();
    Rc::get_mut(&mut bed).unwrap().items.push(Item { name: "Ambience".into(), ..item("Bed", 9.0, 15.0) });
    let tool = Delivery::new(bed).tool();
    let planned = run(&tool, json!({})).await.unwrap();
    assert_eq!(planned["assets"].as_array().unwrap().len(), 1);
    assert!(planned["said"].to_string().contains("“Ambience” on “Bed” (6.0 s) runs under 3 sounds"), "{planned}");
}
