//! Continue without Live when the primary integration reports a Live setup failure.
use crate::core::{
    contracts::*,
    errors::{FailureKind, KumiError, RuntimeError},
};
use async_trait::async_trait;
use kumi_common::abort::Signal;
use std::{cell::RefCell, rc::Rc};

pub struct FallbackIntegration {
    primary: Rc<dyn Integration>,
    current: RefCell<Rc<dyn Integration>>,
    fallback: Rc<dyn Fn() -> Rc<dyn Integration>>,
    on_fallback: Rc<dyn Fn(String)>,
}
pub fn with_fallback(
    primary: Rc<dyn Integration>,
    fallback: Rc<dyn Fn() -> Rc<dyn Integration>>,
    on_fallback: Rc<dyn Fn(String)>,
) -> Rc<FallbackIntegration> {
    Rc::new(FallbackIntegration { current: RefCell::new(primary.clone()), primary, fallback, on_fallback })
}
impl FallbackIntegration {
    fn current(&self) -> Rc<dyn Integration> {
        self.current.borrow().clone()
    }
}
#[async_trait(?Send)]
impl Integration for FallbackIntegration {
    async fn start(&self, signal: Signal) -> Result<(), RuntimeError> {
        if let Err(error) = self.primary.start(signal.clone()).await {
            if signal.is_cancelled() || !error.kumi().is_some_and(|e| e.kind == FailureKind::Live) {
                return Err(error);
            }
            let _ = self.primary.close().await;
            let fallback = (self.fallback)();
            *self.current.borrow_mut() = fallback.clone();
            fallback.start(signal).await?;
            (self.on_fallback)(error.message());
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
    fn has_undo(&self) -> bool {
        true
    }
    async fn undo(&self, id: Option<&str>, signal: Signal) -> Result<ChangeRecord, RuntimeError> {
        let current = self.current();
        if !current.has_undo() {
            return Err(KumiError::new(FailureKind::Request, "Kumi isn't connected to Live, so it can't undo.").into());
        }
        current.undo(id, signal).await
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
    fn has_audition(&self) -> bool {
        true
    }
    async fn audition(&self, request: &AuditionRequest, signal: Signal) -> Result<Result<AuditionResult, String>, RuntimeError> {
        let c = self.current();
        if c.has_audition() {
            c.audition(request, signal).await
        } else {
            Ok(Err("Kumi isn't connected to Live, so it can't render anything to hear.".into()))
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
            Ok(Err("Kumi isn't connected to Live, so it can't pursue a goal.".into()))
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
            Ok(Err("Kumi isn't connected to Live, so it can't hear the Set.".into()))
        }
    }
}
