//! Serialized, best-effort snapshots and catch-up for the currently saved Set.
use super::{
    connection::LiveConnection,
    context::{object, payload},
    project::{self, Baseline, DescribedDiff, ProjectStore},
    views::ViewHost,
};
use crate::core::{
    contracts::{CatchUp, JsonObject},
    errors::RuntimeError,
};
use futures::{
    future::{LocalBoxFuture, Shared},
    FutureExt,
};
use kumi_common::abort::{self, Signal};
use serde_json::{json, Value};
use std::{
    cell::{Cell, RefCell},
    panic::{catch_unwind, AssertUnwindSafe},
    rc::Rc,
    time::Duration,
};
#[derive(Debug, Clone)]
pub struct CurrentProject {
    pub identity: String,
    pub path: Option<String>,
    pub name: String,
    /// The project a saved Set is (`project::decide_project`); none while it's unsaved.
    pub project: Option<String>,
    /// Live says the Set was never saved (not only that its file can't be found now).
    pub unsaved: bool,
}
pub type Saving = Shared<LocalBoxFuture<'static, ()>>;
pub struct Remember {
    pub connection: Rc<LiveConnection>,
    pub store: Option<Rc<dyn ProjectStore>>,
    pub current: RefCell<Option<Rc<CurrentProject>>>,
    pub context: RefCell<Option<JsonObject>>,
    pub last_saved: Cell<i64>,
    on_catch_up: Option<Rc<dyn Fn(CatchUp)>>,
    saving: RefCell<Saving>,
    timer: RefCell<Option<Signal>>,
    /// Kumi's own ids for the Set's tracks, kept whole in the background.
    pub track_ids: super::track_ids::TrackIds,
    /// What Kumi keeps of clips its changes cut or delete, for its undo, and in the Set's history.
    pub history: super::snapshots::SetHistory,
}
impl Remember {
    pub fn new(connection: Rc<LiveConnection>, store: Option<Rc<dyn ProjectStore>>, on_catch_up: Option<Rc<dyn Fn(CatchUp)>>) -> Rc<Self> {
        Rc::new(Self {
            connection,
            store,
            current: RefCell::new(None),
            context: RefCell::new(None),
            last_saved: Cell::new(0),
            on_catch_up,
            saving: RefCell::new(async {}.boxed_local().shared()),
            timer: RefCell::new(None),
            track_ids: Default::default(),
            history: Default::default(),
        })
    }
    pub fn current(&self) -> Option<Rc<CurrentProject>> {
        self.current.borrow().clone()
    }
    pub fn pending(&self) -> Saving {
        self.saving.borrow().clone()
    }
    pub fn cancel_timer(&self) {
        if let Some(timer) = self.timer.borrow_mut().take() {
            timer.cancel();
        }
    }
    fn enqueue(self: &Rc<Self>, work: impl FnOnce(Rc<Self>) -> LocalBoxFuture<'static, Result<(), RuntimeError>> + 'static) -> Saving {
        let before = self.pending();
        let this = self.clone();
        let pending = async move {
            before.await;
            let _ = work(this).await;
        }
        .boxed_local()
        .shared();
        *self.saving.borrow_mut() = pending.clone();
        let running = pending.clone();
        tokio::task::spawn_local(running);
        pending
    }
    pub async fn export_pages(&self, signal: Signal) -> Result<Vec<JsonObject>, RuntimeError> {
        let mut pages = Vec::new();
        let mut cursor = None;
        loop {
            let mut args = super::views::object(json!({"profile":"local","limit":200}));
            if let Some(cursor) = cursor {
                args.insert("cursor".into(), json!(cursor));
            }
            let page = payload(&self.connection.call("live_project_snapshot_export", args, signal.clone()).await?)?;
            cursor = object(page.get("page").unwrap_or(&Value::Null))?
                .get("nextCursor")
                .and_then(Value::as_str)
                .filter(|s| !s.is_empty())
                .map(str::to_owned);
            pages.push(page);
            if cursor.is_none() {
                break;
            }
            if pages.len() >= 64 {
                return Err(RuntimeError::plain("The Set is too large to remember yet"));
            }
        }
        Ok(pages)
    }
    pub fn save_now(self: &Rc<Self>, bound: Option<u64>) -> Saving {
        self.enqueue(move |this| {
            async move {
                let Some(known) = this.current() else { return Ok(()) };
                let Some(path) = known.path.as_deref().filter(|s| !s.is_empty()) else { return Ok(()) };
                let Some(project) = known.project.as_deref() else { return Ok(()) };
                let Some(store) = &this.store else { return Ok(()) };
                let connection = &this.connection;
                if !connection.available.get() || connection.lost.get() || connection.closed.get() {
                    return Ok(());
                }
                let signal = abort::any([connection.lifetime.clone(), abort::timeout(bound.unwrap_or(30_000))]);
                connection.ensure_catalog(signal.clone()).await.map_err(RuntimeError::from)?;
                if !connection.has("live_project_snapshot_export") {
                    return Ok(());
                }
                let pages = this.export_pages(signal).await?;
                if !this.current().is_some_and(|current| Rc::ptr_eq(&current, &known)) {
                    return Ok(());
                }
                store
                    .save(
                        project,
                        &Baseline {
                            version: 1,
                            path: path.into(),
                            name: known.name.clone(),
                            saved_at: connection.now().timestamp_millis(),
                            artifact_id: artifact_of(&pages)?,
                            pages,
                        },
                    )
                    .await?;
                this.last_saved.set(kumi_common::time::now_ms());
                Ok(())
            }
            .boxed_local()
        })
    }
    pub fn schedule_save(self: &Rc<Self>, delay: u64) {
        if self.store.is_none() || self.connection.closed.get() {
            return;
        }
        self.cancel_timer();
        let stop = Signal::new();
        *self.timer.borrow_mut() = Some(stop.clone());
        let weak = Rc::downgrade(self);
        tokio::task::spawn_local(async move {
            tokio::select! {biased;_=stop.cancelled()=>{},_=tokio::time::sleep(Duration::from_millis(delay))=>{if let Some(this)=weak.upgrade(){let _=this.save_now(None);}}}
        });
    }
    /// The project of the saved Set `identity` at `path` (`project::decide_project`), from the id kept inside
    /// it, which is kept there when it's new. A Set saved for the first time (unsaved until now) is a new song,
    /// whatever id its template gave it. Read once for a Set. Without the bridge's Set data it's the id the
    /// path gives, as before.
    pub async fn project_of(&self, identity: &str, path: &str, signal: Signal) -> String {
        let first_save = self.current().is_some_and(|set| set.identity == identity && set.unsaved);
        let connection = &self.connection;
        if connection.ensure_catalog(signal.clone()).await.is_err() || !connection.has("live_data_read") {
            return project::project_id_of(path);
        }
        let read = connection.call("live_data_read", super::views::object(json!({"key":project::PROJECT_KEY})), signal.clone()).await;
        let Some(kept) =
            read.ok().and_then(|found| payload(&found).ok()).map(|found| found.get("value").and_then(Value::as_str).map(str::to_owned))
        else {
            return project::project_id_of(path);
        };
        let (mut last, mut seen_here) = (None, false);
        if let (Some(id), Some(store)) = (kept.as_deref().filter(|id| project::project_id(id)), &self.store) {
            last = store.load(id).await.ok().flatten().map(|baseline| baseline.path);
            // Seen here before as a song of its own (a copy whose id never reached its file): it stays one,
            // wherever the Set its id came from is now.
            let mine = project::project_id_of(path);
            seen_here = id != mine
                && store.load_set(&mine, path).await.ok().flatten().is_some_and(|b| b.path == path || project::same_file(&b.path, path));
        }
        let (id, keep) = project::decide_project(kept.as_deref(), path, last.as_deref(), first_save || seen_here);
        if keep && connection.has("live_data_preview") && connection.has("live_data_apply") {
            let kept: Result<(), RuntimeError> = async {
                let preview = payload(
                    &connection
                        .call("live_data_preview", super::views::object(json!({"key":project::PROJECT_KEY,"value":id})), signal.clone())
                        .await?,
                )?;
                let transaction = preview.get("transactionId").cloned().unwrap_or(Value::Null);
                let apply = json!({"transactionId":transaction,"confirmation":"apply","idempotencyKey":format!("project-{id}")});
                connection.call("live_data_apply", super::views::object(apply), signal).await?;
                Ok(())
            }
            .await;
            // Not kept this time: the next look decides the same id again.
            let _ = kept;
        }
        id
    }
    pub async fn project_path(&self, signal: Signal) -> Option<String> {
        self.project_place(signal).await.0
    }
    /// Where the Set is saved (none while its file can't be found), and whether Live says it was never saved
    /// (false when Live can't say).
    pub async fn project_place(&self, signal: Signal) -> (Option<String>, bool) {
        let result: Result<(Option<String>, bool), RuntimeError> = async {
            self.connection.ensure_catalog(signal.clone()).await.map_err(RuntimeError::from)?;
            if !self.connection.has("live_project_info") {
                return Ok((None, false));
            }
            let info =
                payload(&self.connection.call("live_project_info", JsonObject::new(), abort::any([signal, abort::timeout(5000)])).await?)?;
            let file = info.get("path").and_then(Value::as_str).filter(|s| !s.is_empty());
            // A Set never saved has no path at all; a saved one whose file is gone keeps its path.
            let unsaved = info.get("path").is_none() && info.get("exists") == Some(&Value::Bool(false));
            Ok((file.filter(|_| info.get("exists") != Some(&Value::Bool(false))).map(str::to_owned), unsaved))
        }
        .await;
        result.unwrap_or((None, false))
    }
    pub fn catch_up(self: &Rc<Self>, identity: String, name: String, after_reconnect: bool) {
        *self.context.borrow_mut() = None;
        let Some(store) = self.store.clone() else { return };
        let Some((path, project)) = self
            .current()
            .filter(|p| p.identity == identity)
            .and_then(|p| Some((p.path.clone().filter(|s| !s.is_empty())?, p.project.clone()?)))
        else {
            return;
        };
        let _ = self.enqueue(move|this|async move{
   let connection=&this.connection;let signal=abort::any([connection.lifetime.clone(),abort::timeout(60_000)]);connection.ensure_catalog(signal.clone()).await.map_err(RuntimeError::from)?;
   if !["live_project_info","live_project_snapshot_export","live_project_snapshot_diff"].iter().all(|name|connection.has(name)){return Ok(())}if !this.current().is_some_and(|p|p.identity==identity){return Ok(())}
   let pages=this.export_pages(signal.clone()).await?;
   // This Set's own baseline, or, when it moved here, the project's latest: a version first seen here starts fresh.
   let baseline=store.load_set(&project,&path).await?;
   if let Some(baseline)=baseline.filter(|_|this.current().is_some_and(|p|p.identity==identity)){
    let mut described=Some(DescribedDiff{lines:Vec::new(),more:0});
    if baseline.artifact_id!=artifact_of(&pages)?{
     let diff=async{let result=connection.call("live_project_snapshot_diff",super::views::object(json!({"beforePages":baseline.pages,"afterPages":pages,"limit":200})),signal).await?;let diff=payload(&result)?;Ok::<_,RuntimeError>(project::describe_diff(&diff,&baseline.pages,&pages,None))}.await;
     described=match diff{Ok(diff) if diff.lines.is_empty()=>None,Ok(diff)=>Some(diff),Err(_)=>Some(DescribedDiff{lines:vec!["The Set changed, but it's too big for Kumi to compare yet".into()],more:0})};
    }
    if let Some(described)=described{let mut summary=project::catch_up_from(&name,&baseline,described);if after_reconnect{summary.after_reconnect=Some(true);}
     let mut context=super::views::object(json!({"lastSeen":project::since(baseline.saved_at as f64,connection.now().timestamp_millis() as f64),"changes":summary.lines}));if summary.more!=0{context.insert("more".into(),json!(summary.more));}*this.context.borrow_mut()=Some(context);
     if let Some(callback)=&this.on_catch_up{let _=catch_unwind(AssertUnwindSafe(||callback(summary)));}
    }
   }
   store.save(&project,&Baseline{version:1,path,name,saved_at:connection.now().timestamp_millis(),artifact_id:artifact_of(&pages)?,pages}).await?;this.last_saved.set(kumi_common::time::now_ms());Ok(())
  }.boxed_local());
    }
}
fn artifact_of(pages: &[JsonObject]) -> Result<String, RuntimeError> {
    let empty = json!({});
    let artifact = pages.first().and_then(|page| page.get("artifact")).filter(|v| !v.is_null()).unwrap_or(&empty);
    Ok(object(artifact)?.get("id").and_then(Value::as_str).unwrap_or("").into())
}
