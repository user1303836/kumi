//! Capture a routine the producer performs and describe the newly added devices' settings.
use super::{
    concurrent::eager_all,
    context::{object, payload, ObservationError},
    mutations::Mutations,
    project::describe_watch,
    views::{self, ViewHost},
};
use crate::core::{
    contracts::{JsonObject, ToolResult},
    errors::RuntimeError,
};
use futures::{future::LocalBoxFuture, FutureExt};
use kumi_common::{
    abort::{self, Signal, SignalExt},
    js::{
        json::stringify,
        number::{round, to_string},
    },
};
use serde_json::{json, Value};
use std::{
    cell::RefCell,
    collections::HashSet,
    panic::{catch_unwind, AssertUnwindSafe},
    rc::Rc,
};
struct Watching {
    set: Option<String>,
    pages: Vec<JsonObject>,
    devices: HashSet<String>,
    at: i64,
}
pub struct Watch {
    mutations: Rc<Mutations>,
    watching: RefCell<Option<Watching>>,
}
fn string(value: Option<&Value>) -> String {
    match value {
        None => "undefined".into(),
        Some(Value::Null) => "null".into(),
        Some(Value::String(s)) => s.clone(),
        Some(Value::Object(_)) => "[object Object]".into(),
        Some(Value::Array(a)) => a.iter().map(|v| if v.is_null() { String::new() } else { string(Some(v)) }).collect::<Vec<_>>().join(","),
        Some(v) => stringify(v),
    }
}
impl Watch {
    pub fn new(mutations: Rc<Mutations>) -> Self {
        Self { mutations, watching: RefCell::new(None) }
    }
    fn watching_now(&self, on: bool) {
        if let Some(listener) = &self.mutations.options.on_watch {
            let _ = catch_unwind(AssertUnwindSafe(|| listener(on)));
        }
    }
    async fn all_devices(&self, signal: Signal) -> Result<Vec<JsonObject>, RuntimeError> {
        let connection = &self.mutations.parameters.history.connection;
        let mut rows = Vec::new();
        let mut cursor = None;
        for _ in 0..10_000 {
            let mut args = object(
                &json!({"kind":"device","fields":["ref","parentRef","objectIdentity","name","className"],"limit":connection.page_limit()}),
            )?;
            if let Some(cursor) = cursor.take() {
                args.insert("cursor".into(), json!(cursor));
            }
            let read = payload(&connection.call("live_discover", args, signal.clone()).await?)?;
            rows.extend(read.get("items").and_then(Value::as_array).into_iter().flatten().map(object).collect::<Result<Vec<_>, _>>()?);
            cursor = read.get("nextCursor").and_then(Value::as_str).filter(|s| !s.is_empty()).map(str::to_owned);
            if cursor.is_none() {
                break;
            }
        }
        Ok(rows)
    }
    pub async fn execute(&self, input: JsonObject, original: Signal) -> Result<ToolResult, RuntimeError> {
        let signal = abort::any([original, self.mutations.parameters.history.connection.lifetime.clone(), abort::timeout(60_000)]);
        match self.run(&input, signal.clone()).await {
            Ok(result) => Ok(result),
            Err(error) => {
                signal.check()?;
                Ok(ToolResult::error(match error {
                    RuntimeError::Observation(text) => text,
                    _ => "Kumi couldn't compare the Set just now; try again.".into(),
                }))
            }
        }
    }
    async fn run(&self, input: &JsonObject, signal: Signal) -> Result<ToolResult, RuntimeError> {
        let history = &self.mutations.parameters.history;
        let connection = &history.connection;
        if !connection.available.get() || connection.lost.get() || connection.tools().is_none() {
            return Err(ObservationError("Live isn't connected, so Kumi can't watch it.".into()).into());
        }
        connection.ensure_catalog(signal.clone()).await?;
        if !["live_project_snapshot_export", "live_project_snapshot_diff"].iter().all(|name| connection.has(name)) {
            return Err(
                ObservationError("This bridge can't compare the Set's states, so Kumi can't learn routines by watching.".into()).into()
            );
        }
        if input.get("action").and_then(Value::as_str) == Some("start") {
            let work: Vec<LocalBoxFuture<'_, Result<Vec<JsonObject>, RuntimeError>>> =
                vec![history.remember.export_pages(signal.clone()).boxed_local(), self.all_devices(signal.clone()).boxed_local()];
            let mut read = eager_all(work).await?.into_iter();
            let pages = read.next().unwrap();
            let devices = read.next().unwrap();
            *self.watching.borrow_mut() = Some(Watching {
                set: connection.set.borrow().clone(),
                pages,
                devices: devices.iter().map(|device| string(device.get("objectIdentity"))).collect(),
                at: connection.now().timestamp_millis(),
            });
            self.watching_now(true);
            return Ok(ToolResult::text(stringify(
                &json!({"watching":true,"note":"Tell the producer to go ahead in Live and to say when they're done."}),
            )));
        }
        if self.watching.borrow().is_none() {
            return Ok(ToolResult::error("Kumi isn't watching yet: start first, before the producer does it."));
        }
        if self.watching.borrow().as_ref().unwrap().set != *connection.set.borrow() {
            *self.watching.borrow_mut() = None;
            self.watching_now(false);
            return Ok(ToolResult::error("A different Set is open now, so there's nothing to compare; start again."));
        }
        // Clone the baseline before awaiting: a second start can replace the active watch while this
        // comparison still refers to the earlier one, as the source closure does.
        let (before_pages, before_devices, before_at) = {
            let watching = self.watching.borrow();
            let before = watching.as_ref().unwrap();
            (before.pages.clone(), before.devices.clone(), before.at)
        };
        let track_read = async {
            let read=views::pages(connection.as_ref(),object(&json!({"kind":"track","fields":["ref","name","mediaKind"],"limit":connection.page_limit(),"budget":connection.whole_budget()}))?,signal.clone()).await?;
            Ok::<_, RuntimeError>(payload(&read)?.get("items").cloned().unwrap_or(Value::Null))
        };
        let work: Vec<LocalBoxFuture<'_, Result<Value, RuntimeError>>> = vec![
            history.remember.export_pages(signal.clone()).map(|v| v.map(|v| json!(v))).boxed_local(),
            self.all_devices(signal.clone()).map(|v| v.map(|v| json!(v))).boxed_local(),
            track_read.boxed_local(),
        ];
        let mut reads = eager_all(work).await?.into_iter();
        let exported: Vec<JsonObject> = serde_json::from_value(reads.next().unwrap()).unwrap();
        let devices: Vec<JsonObject> = serde_json::from_value(reads.next().unwrap()).unwrap();
        let tracks = reads.next().unwrap();
        let diff = payload(
            &connection
                .call(
                    "live_project_snapshot_diff",
                    object(&json!({"beforePages":before_pages,"afterPages":exported,"limit":200}))?,
                    signal.clone(),
                )
                .await?,
        )?;
        // The source dereferences each diff item before filtering its kind. A malformed
        // null item is a failed comparison, not an empty, successful watch.
        for item in diff.get("items").and_then(Value::as_array).into_iter().flatten() {
            if item.is_null() {
                return Err(RuntimeError::plain("Cannot read properties of null (reading 'kind')"));
            }
        }
        let mut described = describe_watch(&diff, &before_pages, &exported, None);
        let tracks = tracks.as_array().into_iter().flatten().map(object).collect::<Result<Vec<_>, _>>()?;
        for change in &mut described.changes {
            if change.get("added").and_then(Value::as_str) != Some("track") {
                continue;
            }
            if let Some(media) = tracks
                .iter()
                .find(|track| track.get("name") == change.get("name"))
                .and_then(|track| track.get("mediaKind"))
                .filter(|v| v.is_string())
            {
                change.insert("media".into(), media.clone());
            }
        }
        let settings=eager_all(devices.iter().filter(|device|!before_devices.contains(&string(device.get("objectIdentity")))).take(12).map(|device|{
            let signal=signal.clone();let tracks=&tracks;
            async move{
                let parameters=self.mutations.parameters.device_parameters(device.get("ref").cloned().unwrap_or(Value::Null),["name","value","defaultValue","displayValue"].into_iter().map(str::to_owned).collect(),signal).await?;
                let knobs:Vec<_>=parameters.iter().filter(|p|match (p.get("value").and_then(Value::as_f64),p.get("defaultValue").and_then(Value::as_f64)){(Some(value),Some(default))=>(value-default).abs()>1e-6,_=>false}).take(24).map(|p|{
                    let mut knob=object(&json!({"name":p.get("name").unwrap_or(&Value::Null),"value":p["value"]})).unwrap();if let Some(display)=p.get("displayValue").filter(|v|v.is_string()){knob.insert("shows".into(),display.clone());}json!(knob)
                }).collect();
                let owner=tracks.iter().rev().find(|track|string(track.get("ref"))==string(device.get("parentRef"))).map(|track|string(track.get("name").filter(|v|!v.is_null()).or(Some(&json!(""))))).filter(|s|!s.is_empty());
                let mut setting=object(&json!({"device":device.get("name").filter(|v|!v.is_null()).or_else(||device.get("className")).unwrap_or(&Value::Null),"className":device.get("className").unwrap_or(&Value::Null)}))?;
                if let Some(owner)=owner{setting.insert("on".into(),json!(owner));}else{setting.insert("inside".into(),json!("a rack"));}setting.insert("knobs".into(),json!(knobs));Ok::<_,RuntimeError>(setting)
            }
        })).await?;
        *self.watching.borrow_mut() = None;
        self.watching_now(false);
        let mut reply = object(
            &json!({"watched":format!("{} s",to_string(round((connection.now().timestamp_millis()-before_at) as f64/1000.0))),"changes":described.changes}),
        )?;
        if described.more > 0 {
            reply.insert("more".into(), json!(described.more));
        }
        if !settings.is_empty() {
            reply.insert("devicesAdded".into(), json!(settings));
        }
        if described.changes.is_empty() {
            reply.insert("note".into(), json!("Nothing in the Set changed while Kumi watched."));
        }
        Ok(ToolResult::text(stringify(&json!(reply))))
    }
}
