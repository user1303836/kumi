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
        let (class, name) = self.class_and_name(device, signal).await?;
        // A plug-in (VST "PluginDevice", AU "AuPluginDevice") or a Max device goes by its name; Live's own by class. A
        // rack isn't saved: its macros are each rack's own.
        if class.ends_with("GroupDevice") {
            return None;
        }
        let own = !class.is_empty() && !class.contains("PluginDevice") && !class.starts_with("Mx");
        Some(if own { class } else { name }).filter(|kind| !kind.is_empty())
    }

    /// Who a device is, for what Kumi remembers of a search on it (`identity_of`).
    pub(super) async fn device_identity(&self, device: &str, signal: Signal) -> Option<String> {
        let (class, name) = self.class_and_name(device, signal).await?;
        identity_of(&class, &name)
    }

    /// A device's class and its name in Live.
    async fn class_and_name(&self, device: &str, signal: Signal) -> Option<(String, String)> {
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
        Some((pair.first()?.as_str()?.to_owned(), pair.get(1)?.as_str()?.to_owned()))
    }
}

/// Who a Live device is, the same in every session and wherever it sits: Live's own device by its class ("Eq8"), a
/// plug-in by its format and name ("AU: Pro-Q 3": Live doesn't say a plug-in's maker, nor a VST's version), a Max
/// device and a rack by their names ("Max: …", "Rack: …": a rack's macros are its own).
pub(super) fn identity_of(class: &str, name: &str) -> Option<String> {
    let name = name.trim();
    let named = |kind: &str| (!name.is_empty()).then(|| format!("{kind}: {name}"));
    if class.ends_with("GroupDevice") {
        named("Rack")
    } else if class == "AuPluginDevice" {
        named("AU")
    } else if class.contains("PluginDevice") {
        named("VST")
    } else if class.starts_with("Mx") {
        named("Max")
    } else {
        (!class.is_empty()).then(|| class.to_owned())
    }
}

#[cfg(test)]
mod tests {
    use super::identity_of;

    #[test]
    fn a_device_is_who_it_is_not_where_it_sits_or_what_it_was_renamed() {
        assert_eq!(identity_of("Eq8", "Kick EQ").as_deref(), Some("Eq8"), "Live's own by its class, whatever it's called");
        assert_eq!(identity_of("AuPluginDevice", "Pro-Q 3").as_deref(), Some("AU: Pro-Q 3"));
        assert_eq!(identity_of("PluginDevice", "Serum 2").as_deref(), Some("VST: Serum 2"));
        assert_eq!(identity_of("MxDeviceAudioEffect", "LFO").as_deref(), Some("Max: LFO"));
        assert_eq!(identity_of("AudioEffectGroupDevice", "Bass Rack").as_deref(), Some("Rack: Bass Rack"));
        assert_eq!(identity_of("PluginDevice", " "), None, "a plug-in with no name says nothing of who it is");
        assert_eq!(identity_of("", "EQ Eight"), None);
    }
}
