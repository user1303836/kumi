//! REAPER's backend for delivery (#401, the render pipeline): only REAPER's facts. Items and regions as its reads give them,
//! the regions of the assets added by regions_add in one change (each with the tracks it renders in the region render
//! matrix), the files a render would write from its own dry run (`deliver_targets`, every render setting put back), and the
//! render as a job (`deliver`), waited for as a listen's render is. Core's tool does the rest.
use super::own_changes::OwnPlan;
use super::own_listening::OwnRenderer;
use crate::core::delivery::{
    assets::Item,
    backend::{DeliveryBackend, NewRegion, Region, Rendered, Targets, Words},
};
use crate::core::refs::RefScheme;
use async_trait::async_trait;
use kumi_common::abort::Signal;
use serde_json::{json, Value};
use std::collections::BTreeMap;
use std::rc::Rc;

pub static WORDS: Words = Words {
    app: "REAPER",
    region: "region",
    region_pattern: "$region",
    wildcards: "REAPER's wildcards: $region, $track, $parenttrack, $project, $timelineorder ($timelineorder[00] zero-pads it); $tracknameornumber isn't one",
    sources: "matrix, the default: each region's tracks (its row in the region render matrix) summed through the master, a file per region; master: the whole mix; stems: a file per track in trackRefs",
    bounds: "regions, the default; selected-regions; project; time-selection; or {start, end} in seconds",
    source: "matrix",
    source_said: "each region's own tracks summed through the master, a file per region",
};

/// Pages of rows read for the items, tracks and regions (200 a page).
const PAGES: usize = 20;

pub struct ReaperDelivery<S: RefScheme + 'static> {
    plan: Rc<OwnPlan<S>>,
    renderer: Rc<OwnRenderer<S>>,
}

impl<S: RefScheme + 'static> ReaperDelivery<S> {
    pub fn new(plan: Rc<OwnPlan<S>>, renderer: Rc<OwnRenderer<S>>) -> Self {
        ReaperDelivery { plan, renderer }
    }

    fn short(&self, long: &str) -> String {
        self.plan.link().book.borrow_mut().short_ref(long)
    }

    /// The settings as the host takes them: its keys as given, the refs (a stems render's tracks, the regions that render)
    /// long, from the epoch they were read in.
    fn host_settings(&self, settings: &Value) -> Result<Value, String> {
        let mut out = settings.clone();
        let mut placed = false;
        for key in ["trackRefs", "regionRefs"] {
            if let Some(list) = settings.get(key).and_then(Value::as_array) {
                let long: Vec<String> =
                    list.iter().filter_map(Value::as_str).map(|reference| self.plan.lengthen(reference)).collect::<Result<_, _>>()?;
                out[key] = json!(long);
                placed = true;
            }
        }
        if placed {
            let input = out.as_object().cloned().unwrap_or_default();
            self.plan.place_request(&mut out, &input);
        }
        Ok(out)
    }

    async fn call(&self, tool: &str, args: Value, signal: &Signal) -> Result<Value, String> {
        match self.plan.link().call(tool, args, signal.clone()).await {
            Ok(Ok(answer)) => Ok(answer),
            Ok(Err(refused)) => Err(refused.why),
            Err(error) => Err(error.to_string()),
        }
    }
}

#[async_trait(?Send)]
impl<S: RefScheme + 'static> DeliveryBackend for ReaperDelivery<S> {
    fn words(&self) -> &'static Words {
        &WORDS
    }

    async fn items(&self, tracks: &[String], selected: bool, signal: &Signal) -> Result<Vec<Item>, String> {
        let track_rows = self.plan.rows_through(json!({"kind": "track", "limit": 200}), PAGES, signal).await?;
        let names: BTreeMap<String, String> = track_rows
            .iter()
            .filter_map(|row| Some((row.get("ref")?.as_str()?.to_string(), row.get("name")?.as_str()?.to_string())))
            .collect();
        // (a muted track's items are no asset's: a guide, a reference)
        let muted: Vec<&str> = track_rows
            .iter()
            .filter(|row| row.get("mute") == Some(&Value::Bool(true)))
            .filter_map(|row| row.get("ref")?.as_str())
            .collect();
        let mut rows = Vec::new();
        if tracks.is_empty() {
            rows = self.plan.rows_through(json!({"kind": "item", "limit": 200}), PAGES, signal).await?;
        } else {
            for track in tracks {
                let long = self.plan.lengthen(track)?;
                rows.extend(self.plan.rows_through(json!({"kind": "item", "parent": long, "limit": 200}), PAGES, signal).await?);
            }
        }
        Ok(rows
            .iter()
            .filter(|row| !selected || row.get("selected") == Some(&Value::Bool(true)))
            .filter(|row| row.get("muted") != Some(&Value::Bool(true)))
            .filter_map(|row| {
                let track = row.get("trackRef")?.as_str()?.to_string();
                if muted.contains(&track.as_str()) {
                    return None;
                }
                let start = row.get("position")?.as_f64()?;
                let length = row.get("length")?.as_f64()?;
                Some(Item {
                    reference: self.short(row.get("ref")?.as_str()?),
                    track_name: names.get(&track).cloned().unwrap_or_default(),
                    track: self.short(&track),
                    start,
                    end: start + length,
                    name: row.get("name").and_then(Value::as_str).unwrap_or("").to_string(),
                })
            })
            .collect())
    }

    async fn regions(&self, signal: &Signal) -> Result<Vec<Region>, String> {
        let rows = self.plan.rows_through(json!({"kind": "region", "limit": 200}), PAGES, signal).await?;
        Ok(rows
            .iter()
            .filter_map(|row| {
                Some(Region {
                    reference: self.short(row.get("ref")?.as_str()?),
                    name: row.get("name").and_then(Value::as_str).unwrap_or("").to_string(),
                    start: row.get("position")?.as_f64()?,
                    end: row.get("end")?.as_f64()?,
                })
            })
            .collect())
    }

    async fn add_regions(&self, regions: &[NewRegion], signal: &Signal) -> Result<Value, String> {
        let input = regions_input(regions);
        let outcome = self.plan.change_by("regions_add", input.as_object().cloned().unwrap_or_default(), signal.clone()).await;
        let answer: Value = serde_json::from_str(&outcome.text).unwrap_or_else(|_| json!({"said": outcome.text}));
        if outcome.is_error || outcome.stops {
            return Err(answer.get("changed").and_then(Value::as_str).map(str::to_owned).unwrap_or(outcome.text));
        }
        Ok(answer)
    }

    async fn targets(&self, settings: &Value, signal: &Signal) -> Result<Targets, String> {
        let answer = self.call("deliver_targets", self.host_settings(settings)?, signal).await?;
        let files =
            answer.get("targets").and_then(Value::as_array).into_iter().flatten().filter_map(Value::as_str).map(str::to_owned).collect();
        let list = |key: &str| -> Vec<String> {
            answer.get(key).and_then(Value::as_array).into_iter().flatten().filter_map(Value::as_str).map(str::to_owned).collect()
        };
        let (there, notes) = (list("there"), list("notes"));
        Ok(Targets { files, folder: answer.get("folder").and_then(Value::as_str).unwrap_or("").to_string(), there, notes })
    }

    async fn render(&self, settings: &Value, expect: &[String], signal: &Signal) -> Result<Rendered, String> {
        let mut args = self.host_settings(settings)?;
        args["expect"] = json!(expect);
        let started = self.call("deliver", args, signal).await?;
        match self.renderer.wait(&started, signal).await {
            Ok(Ok(done)) => Ok(Rendered {
                files: done.get("files").cloned().unwrap_or(Value::Null),
                notes: done
                    .get("notes")
                    .and_then(Value::as_array)
                    .into_iter()
                    .flatten()
                    .filter_map(Value::as_str)
                    .map(str::to_owned)
                    .collect(),
            }),
            Ok(Err(why)) => Err(why),
            Err(error) => Err(error.to_string()),
        }
    }
}

/// regions_add's input for the assets' regions: each one's stretch, name and the tracks it renders.
fn regions_input(regions: &[NewRegion]) -> Value {
    let list: Vec<Value> = regions
        .iter()
        .map(|region| {
            let mut row = json!({"position": region.start, "end": region.end, "name": region.name});
            if !region.renders.is_empty() {
                row["renderRefs"] = json!(region.renders);
            }
            row
        })
        .collect();
    json!({"regions": list})
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::integrations::reaper_changes::CHANGES;
    use crate::kernel::schema::problem;

    #[test]
    fn the_assets_regions_fit_regions_add() {
        let regions = vec![
            NewRegion { start: 10.0, end: 11.99, name: "Footstep_Gravel_03".into(), renders: vec![] },
            NewRegion { start: 12.0, end: 13.99, name: "Footstep_Gravel_04".into(), renders: vec!["track:2".into()] },
        ];
        let input = regions_input(&regions);
        let schema = Value::Object(CHANGES.kinds.iter().find(|kind| kind.tool == "regions_add").unwrap().input_schema());
        assert_eq!(problem(&schema, &input), None, "{input}");
        assert!(input["regions"][0].get("renderRefs").is_none(), "the master mix by default");
        assert_eq!(input["regions"][1]["renderRefs"], json!(["track:2"]));
    }
}
