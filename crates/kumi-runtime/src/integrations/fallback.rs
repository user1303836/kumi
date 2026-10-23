//! Continue without the DAW when the primary integration reports a DAW setup failure.
use crate::core::{
    contracts::*,
    daw::cant_yet,
    errors::{FailureKind, KumiError, RuntimeError},
};
use async_trait::async_trait;
use kumi_common::abort::Signal;
use serde_json::Value;
use std::{cell::RefCell, rc::Rc};

pub struct FallbackIntegration {
    primary: Rc<dyn Integration>,
    current: RefCell<Rc<dyn Integration>>,
    fallback: Rc<dyn Fn() -> Rc<dyn Integration>>,
    on_fallback: Rc<dyn Fn(&KumiError)>,
}
/// `on_fallback` is told what the primary said, in its own words.
pub fn with_fallback(
    primary: Rc<dyn Integration>,
    fallback: Rc<dyn Fn() -> Rc<dyn Integration>>,
    on_fallback: Rc<dyn Fn(String)>,
) -> Rc<FallbackIntegration> {
    with_fallback_failure(primary, fallback, Rc::new(move |failure: &KumiError| on_fallback(failure.message.clone())))
}
/// `on_fallback` is told the primary's whole failure: its words, and the setup fix it has (restart the DAW).
pub fn with_fallback_failure(
    primary: Rc<dyn Integration>,
    fallback: Rc<dyn Fn() -> Rc<dyn Integration>>,
    on_fallback: Rc<dyn Fn(&KumiError)>,
) -> Rc<FallbackIntegration> {
    Rc::new(FallbackIntegration { current: RefCell::new(primary.clone()), primary, fallback, on_fallback })
}
impl FallbackIntegration {
    fn current(&self) -> Rc<dyn Integration> {
        self.current.borrow().clone()
    }
    /// "Kumi isn't connected to Live, so it can't …", in the primary's words (Live's when it doesn't say).
    fn not_connected(&self, what: &str) -> String {
        format!("Kumi isn't connected to {}, so it can't {what}.", self.primary.profile().or_live().words.app)
    }
    /// (M5) Why it can't do what the integration answering now doesn't do: the DAW's own integration answers (it
    /// started), and it can't do that yet ("Kumi can't pursue a goal in Logic Pro yet."); else Kumi isn't connected.
    fn cant(&self, what: &str) -> String {
        let profile = self.primary.profile();
        if Rc::ptr_eq(&self.current(), &self.primary) && profile.id.is_some() {
            return cant_yet(profile, what);
        }
        self.not_connected(what)
    }
}
#[async_trait(?Send)]
impl Integration for FallbackIntegration {
    /// The primary's DAW, whichever answers now: what it can do is checked at each call.
    fn profile(&self) -> &DawProfile {
        self.primary.profile()
    }
    async fn start(&self, signal: Signal) -> Result<(), RuntimeError> {
        if let Err(error) = self.primary.start(signal.clone()).await {
            if signal.is_cancelled() || !error.kumi().is_some_and(|e| e.kind == FailureKind::Daw) {
                return Err(error);
            }
            let _ = self.primary.close().await;
            let fallback = (self.fallback)();
            *self.current.borrow_mut() = fallback.clone();
            fallback.start(signal).await?;
            if let Some(failure) = error.kumi() {
                (self.on_fallback)(failure);
            }
        }
        Ok(())
    }
    async fn observe(&self, signal: Signal, hints: Option<ObserveHints>) -> Result<Observation, RuntimeError> {
        self.current().observe(signal, hints).await
    }
    async fn close(&self) -> Result<(), RuntimeError> {
        self.current().close().await
    }
    async fn settled(&self) {
        self.current().settled().await
    }
    fn steered(&self) {
        self.current().steered()
    }
    fn conversation(&self, id: &str) {
        self.current().conversation(id)
    }
    fn instructions_addendum(&self, offered: &dyn Fn(&str) -> bool) -> String {
        self.current().instructions_addendum(offered)
    }
    fn worker_tools(&self) -> Vec<Rc<dyn KernelTool>> {
        self.current().worker_tools()
    }
    fn has_undo(&self) -> bool {
        true
    }
    async fn undo(&self, id: Option<&str>, signal: Signal) -> Result<ChangeRecord, RuntimeError> {
        let current = self.current();
        if !current.has_undo() {
            return Err(KumiError::new(FailureKind::Request, self.cant("undo")).into());
        }
        current.undo(id, signal).await
    }
    fn fingerprint(&self) -> Option<Value> {
        self.current().fingerprint()
    }
    fn first_heard(&self, since: i64) -> Option<i64> {
        self.current().first_heard(since)
    }
    fn has_audio_file(&self) -> bool {
        true
    }
    async fn audio_file(&self, named: &str, signal: Signal) -> Result<Option<String>, RuntimeError> {
        let c = self.current();
        if c.has_audio_file() {
            c.audio_file(named, signal).await
        } else {
            Ok(None)
        }
    }
    fn has_stop_live(&self) -> bool {
        true
    }
    async fn stop_live(&self, signal: Signal) -> Result<bool, RuntimeError> {
        let c = self.current();
        if c.has_stop_live() {
            c.stop_live(signal).await
        } else {
            Ok(false)
        }
    }
    fn open_set(&self) -> Option<String> {
        self.current().open_set()
    }
    fn words(&self) -> (&'static str, &'static str) {
        self.primary.words()
    }
    /// The DAW's own answer, as its question and wire are its own: Live's stays Live's while it's closed and the session
    /// falls back to chat (opening then goes to what's current, which says why it can't).
    fn has_open_place(&self) -> bool {
        self.primary.has_open_place()
    }
    async fn open_place(&self, place: &str, signal: Signal) -> Result<Result<(), String>, RuntimeError> {
        self.current().open_place(place, signal).await
    }
    fn has_show_message(&self) -> bool {
        true
    }
    async fn show_message(&self, text: &str, signal: Signal) -> Result<bool, RuntimeError> {
        let c = self.current();
        if c.has_show_message() {
            c.show_message(text, signal).await
        } else {
            Ok(false)
        }
    }
    async fn recording(&self, signal: Signal) -> Result<Option<bool>, RuntimeError> {
        self.current().recording(signal).await
    }
    fn middle_c_octave(&self) -> Option<i8> {
        self.current().middle_c_octave()
    }
    fn has_instant(&self) -> bool {
        true
    }
    async fn instant(&self, intent: &crate::core::phrases::Intent, signal: Signal) -> Result<Option<InstantOutcome>, RuntimeError> {
        let c = self.current();
        if c.has_instant() {
            c.instant(intent, signal).await
        } else {
            Ok(None)
        }
    }
    fn has_device_tree(&self) -> bool {
        true
    }
    async fn device_tree(&self, track_ref: &str, signal: Signal) -> Result<Option<DeviceTree>, RuntimeError> {
        let c = self.current();
        if c.has_device_tree() {
            c.device_tree(track_ref, signal).await
        } else {
            Ok(None)
        }
    }
    fn has_clip_view(&self) -> bool {
        true
    }
    async fn clip_view(&self, slot_ref: &str, signal: Signal) -> Result<Option<ClipView>, RuntimeError> {
        let c = self.current();
        if c.has_clip_view() {
            c.clip_view(slot_ref, signal).await
        } else {
            Ok(None)
        }
    }
    fn has_session_strip(&self) -> bool {
        true
    }
    async fn session_strip(&self, track_ref: &str, scene: f64, signal: Signal) -> Result<Option<SessionStrip>, RuntimeError> {
        let c = self.current();
        if c.has_session_strip() {
            c.session_strip(track_ref, scene, signal).await
        } else {
            Ok(None)
        }
    }
    fn has_arrangement_strip(&self) -> bool {
        true
    }
    async fn arrangement_strip(&self, signal: Signal) -> Result<Option<ArrangementStrip>, RuntimeError> {
        let c = self.current();
        if c.has_arrangement_strip() {
            c.arrangement_strip(signal).await
        } else {
            Ok(None)
        }
    }
    fn can_show(&self, kind: BodyKind) -> bool {
        self.current().can_show(kind)
    }
    fn pin_for_recipe(&self, pin: &PinnedNode) -> Option<PinnedNode> {
        self.current().pin_for_recipe(pin)
    }
    async fn focus_body(&self, request: &BodyRequest, signal: Signal) -> Result<Option<FocusBody>, RuntimeError> {
        let c = self.current();
        if c.can_show(request.kind()) {
            c.focus_body(request, signal).await
        } else {
            Ok(None)
        }
    }
    fn has_audition(&self) -> bool {
        true
    }
    async fn audition(&self, request: &AuditionRequest, signal: Signal) -> Result<Result<AuditionResult, String>, RuntimeError> {
        let c = self.current();
        if c.has_audition() {
            c.audition(request, signal).await
        } else {
            Ok(Err(self.cant("render anything to hear")))
        }
    }
    fn has_goal(&self) -> bool {
        true
    }
    async fn goal(&self, request: &AuditionRequest, signal: Signal) -> Result<Result<Rc<dyn GoalRig>, String>, RuntimeError> {
        let c = self.current();
        if c.has_goal() {
            c.goal(request, signal).await
        } else {
            Ok(Err(self.cant("pursue a goal")))
        }
    }
    fn has_hear(&self) -> bool {
        true
    }
    async fn hear(&self, request: &HearRequest, signal: Signal) -> Result<Result<Vec<HeardTake>, String>, RuntimeError> {
        let c = self.current();
        if c.has_hear() {
            c.hear(request, signal).await
        } else {
            Ok(Err(self.cant(&format!("hear the {}", self.primary.profile().or_live().words.place))))
        }
    }
}
