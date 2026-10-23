//! Live's side of probing a device: the scratch copies of its track a probe or a search hears (made and taken away in
//! one Live undo step) and what kind of device it is. What a probe does with them is `Listening`'s.
use super::{tune::COPIES_BUDGET, *};
use kumi_common::js::string::head;
use kumi_listening::renderer::{Copies, Window};

/// What Live keeps of scratch copies of a device's track, made for one search or probe and taken away after it, in one
/// Live undo step.
struct LiveCopies {
    device: String,
    prefix: String,
    known: Value,
    step: Option<String>,
    /// The copies' track names, in order.
    pub names: Vec<String>,
    position: u64,
}

impl LiveRenderer {
    /// Makes `count` copies of a device's track, for work heard in `passes` passes over `window`. The tracks there
    /// before are noted first (by Live's identity), so a crash or a lost connection still leaves the copies to be
    /// swept, and the one undo step lasts about as long as the work.
    pub(super) async fn open_copies(
        &self,
        device: &str,
        count: usize,
        (label, passes, window): (&str, u32, Window),
        signal: Signal,
    ) -> Result<Result<Copies, String>, RuntimeError> {
        let tag = uuid::Uuid::new_v4().to_string()[..4].to_owned();
        let prefix = format!("Kumi · try {tag}");
        let tempo = self.observer.tempo.get().filter(|tempo| *tempo > 0.).unwrap_or(120.);
        // Each pass is the window heard plus setting up; making and dropping the copies, a minute.
        let step = self.open_undo_step(label, passes as f64 * (window.beats * 60. / tempo + 10.) + 60.).await;
        let known = match self.copies(device, json!({"action":"before"}), signal.clone()).await {
            Ok(Ok(known)) => known,
            Ok(Err(why)) => {
                self.close_undo_step(step).await;
                return Ok(Err(why));
            }
            Err(error) => {
                self.close_undo_step(step).await;
                return Err(error);
            }
        };
        self.note_copies(&prefix, Some(json!({"prefix":prefix,"before":known["before"],"source":known["source"]})));
        self.copies_live.borrow_mut().push(prefix.clone());
        let mut copies = LiveCopies { device: device.into(), prefix, known, step, names: vec![], position: 0 };
        let made = self.copies(device, json!({"action":"make","count":count,"prefix":copies.prefix,"budget":COPIES_BUDGET}), signal).await;
        match made {
            Ok(Ok(made)) => {
                copies.position = made.get("position").and_then(Value::as_u64).unwrap_or(0);
                copies.names = made
                    .get("names")
                    .and_then(Value::as_array)
                    .into_iter()
                    .flatten()
                    .filter_map(Value::as_str)
                    .map(str::to_owned)
                    .collect();
                Ok(Ok(Copies { names: copies.names.clone(), held: Box::new(copies) }))
            }
            Ok(Err(why)) => {
                self.drop_copies(copies).await;
                Ok(Err(why))
            }
            Err(error) => {
                self.drop_copies(copies).await;
                Err(error)
            }
        }
    }

    /// Sets each copy's knobs (by copy name, knob name and raw value).
    pub(super) async fn set_copies(
        &self,
        copies: &Copies,
        values: serde_json::Map<String, Value>,
        signal: Signal,
    ) -> Result<Result<(), String>, RuntimeError> {
        let held = copies.held.downcast_ref::<LiveCopies>().expect("copies made by Live");
        Ok(self.copies(&held.device, json!({"action":"set","position":held.position,"values":values}), signal).await?.map(|_| ()))
    }

    /// Takes the copies away, whatever happened.
    pub(super) async fn close_copies(&self, copies: Copies) {
        if let Ok(held) = copies.held.downcast::<LiveCopies>() {
            self.drop_copies(*held).await;
        }
    }

    /// Takes the copies away, whatever happened (a make cut off partway takes its own back), and closes the undo step.
    async fn drop_copies(&self, copies: LiveCopies) {
        let dropped = self
            .copies(
                &copies.device,
                json!({"action":"drop","prefix":copies.prefix,"before":copies.known["before"],"source":copies.known["source"],"by_name":true}),
                self.cleanup(),
            )
            .await;
        self.close_undo_step(copies.step).await;
        self.copies_live.borrow_mut().retain(|live| *live != copies.prefix);
        match dropped {
            // Forgotten only once nothing of them is left.
            Ok(Ok(done)) if done["left"].as_u64() == Some(0) => self.note_copies(&copies.prefix, None),
            Ok(Ok(done)) => self.tell(
                format!("{} of Kumi's scratch copies are still in the Set; it'll remove them when Live reconnects.", done["left"]),
                None,
            ),
            Ok(Err(why)) => {
                self.tell(format!("Kumi couldn't remove its scratch copies ({why}); it'll try again when Live reconnects."), None)
            }
            Err(error) => self.tell(
                format!(
                    "Kumi couldn't remove its scratch copies ({}); it'll try again when Live reconnects.",
                    head(&error.to_string(), 160)
                ),
                None,
            ),
        }
    }

    /// What kind of device it is, as its probes are saved: Live's class for its own devices, the name for a plug-in
    /// or a Max device.
    pub(super) async fn device_kind(&self, device: &str, signal: Signal) -> Option<String> {
        let long = self.connection().references.borrow().lengthen(&json!({"deviceRef":device}));
        let long = long["deviceRef"].as_str().unwrap_or(device).to_owned();
        let code = "result = [str(obj.class_name), str(obj.name)]";
        let read = self
            .connection()
            .call("live_run_python", object(json!({"code":code,"mode":"exec","ref":long,"timeoutMs":5000})), signal)
            .await
            .ok()?;
        if read.is_error == Some(true) {
            return None;
        }
        let done = super::super::context::payload(&read).ok()?;
        let pair = done.get("result")?.as_array()?;
        let (class, name) = (pair.first()?.as_str()?, pair.get(1)?.as_str()?);
        // A plug-in (VST "PluginDevice", AU "AuPluginDevice") or a Max device goes by its name; Live's own by class. A
        // rack isn't saved: its macros are each rack's own.
        if class.ends_with("GroupDevice") {
            return None;
        }
        let own = !class.is_empty() && !class.contains("PluginDevice") && !class.starts_with("Mx");
        Some(if own { class.to_owned() } else { name.to_owned() }).filter(|kind| !kind.is_empty())
    }
}
