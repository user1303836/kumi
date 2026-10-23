//! Live as the listening tools meet it: what `Renderer` asks, answered with Live's bridge, its render rigs and Kumi
//! Ears. The renderer is the `Rc` itself ([`LiveHost`]), since the rigs are made and closed through it.
//!
//! Inside this impl the inherent methods are called by their path (`LiveRenderer::tell(self, …)`): a method call on the
//! host finds `Renderer`'s first.
use super::super::bridge_version::RENDER_BRIDGE;
use super::super::connection::NO_CURRENT_LIVE;
use super::{pass::RECORD_LEAD, rig::BEFORE_MAIN, roles::bands_of, *};
use crate::{
    core::daw::DawProfile,
    listening::{
        checklist::MainState,
        fit::Band,
        judging::Placed,
        notes::Kit,
        renderer::{
            AuditionOptions, AuditionPass, Captured, ClipNotes, Copies, DeviceKnob, EqFit, ListenFlag, NotePatch, Pass, Renderer, Window,
        },
    },
    plugins::roles::Seen,
};
use async_trait::async_trait;
use kumi_common::js::string::head;

/// Live's renderer as the listening tools hold it: the `Rc` (listening's `Renderer` is another crate's trait, so it's
/// this crate's type that answers it).
#[derive(Clone)]
pub struct LiveHost(pub Rc<LiveRenderer>);

impl std::ops::Deref for LiveHost {
    type Target = Rc<LiveRenderer>;
    fn deref(&self) -> &Rc<LiveRenderer> {
        &self.0
    }
}

#[async_trait(?Send)]
impl Renderer for LiveHost {
    fn profile(&self) -> &DawProfile {
        &DawProfile::ABLETON
    }
    fn refusal(&self) -> Option<String> {
        (!LiveRenderer::available(self)).then(|| NO_CURRENT_LIVE.to_string())
    }
    fn tempo(&self) -> Option<f64> {
        self.observer.tempo.get()
    }
    fn beats_per_bar(&self) -> f64 {
        self.observer.beats_per_bar.get()
    }
    fn lifetime(&self) -> Signal {
        LiveRenderer::connection(self).lifetime.clone()
    }
    fn cleanup(&self) -> Signal {
        LiveRenderer::cleanup(self)
    }
    fn tell(&self, title: &str, playing: Option<bool>) {
        LiveRenderer::tell(self, title, playing)
    }
    fn flag(&self) -> &ListenFlag {
        &self.rendering
    }
    fn kept_folder(&self) -> PathBuf {
        self.ears_folder.join("judge")
    }

    async fn song_end(&self, signal: Signal) -> Option<f64> {
        LiveRenderer::song_end(self, signal).await
    }
    async fn track_name(&self, named: &str, signal: Signal) -> Option<String> {
        LiveRenderer::track_name(self, named, signal).await
    }
    async fn track_ref(&self, named: &str, signal: Signal) -> Option<String> {
        self.track_ref_long(named, signal).await
    }
    async fn clip_file(&self, named: &str, signal: Signal) -> Result<Option<String>, RuntimeError> {
        (self.clip_file)(named.into(), signal).await
    }

    async fn hear_pass(&self, candidates: &[AuditionCandidate], window: Window, before_main: bool, signal: Signal) -> Pass {
        let mut rig = None;
        let rendered: Result<(IndexMap<String, Render>, Vec<(String, bool)>), RuntimeError> = async {
            rig = Some(self.open_rig_with(candidates, Some(window.from), Some(window.beats), before_main, signal.clone()).await?);
            let rig = rig.as_mut().unwrap();
            let files = self.render_pass(rig, signal.clone()).await?;
            Ok((files, rig.sources.iter().map(|source| (source.name.clone(), source.mix)).collect()))
        }
        .await;
        let mut notes = vec![];
        // Why the mix wasn't heard as it comes into Main's chain, when Kumi Ears tried to: its device wasn't placed, or
        // nothing came through from it.
        let mut tried = None;
        // The record pass keeps a lead before the part for a hit to rise out of; Kumi Ears measures from the part itself.
        let lead = if rig.as_ref().is_some_and(|rig| rig.ears.is_some()) { 0. } else { RECORD_LEAD };
        if let Some(rig) = rig.as_mut() {
            let placed = rig.ears.as_ref().is_some_and(|ears| ears.before_main.is_some());
            self.close_rig(rig).await;
            notes.extend(rig.notes.clone());
            tried = rig.before_main_missed.clone().or_else(|| placed.then(|| "nothing came through from it".to_string()));
        }
        let rendered = rendered.map(|(mut files, sources)| {
            let before_master = files.shift_remove(BEFORE_MAIN);
            Captured { files, sources, before_master }
        });
        Pass { rendered, notes, tried, lead, audible: true }
    }
    async fn master_chain(&self, signal: Signal) -> MainState {
        self.main_state(signal).await
    }
    async fn fader_of(&self, track: &str, signal: Signal) -> Option<f64> {
        LiveRenderer::fader_of(self, track, signal).await
    }

    fn applied(&self) -> Vec<String> {
        LiveRenderer::applied_ids(self)
    }
    fn applied_since(&self, mark: &[String]) -> Vec<(String, String)> {
        LiveRenderer::applied_since(self, mark)
    }
    fn audible(&self, id: &str) -> Option<bool> {
        self.audible_change(id)
    }
    fn outside(&self) -> usize {
        self.history.outside.borrow().len()
    }
    fn outside_since(&self, mark: usize) -> Vec<&'static str> {
        self.history.outside_since(mark)
    }
    async fn undo(&self, id: &str) -> Result<(), String> {
        self.undo_change(id).await
    }
    fn made(&self, changes: &[(String, String)]) -> Vec<String> {
        self.devices_made_by(changes)
    }
    fn devices_made(&self, changes: &[(String, String)]) -> usize {
        self.created_by(changes)
    }
    fn renames(&self, before: &[String]) -> Vec<(String, String, String)> {
        self.renames_since(before)
    }

    async fn turn_gain(&self, gain: f64, track: Option<&str>, signal: Signal) -> Result<(String, bool), String> {
        self.rebalance(gain, track, signal).await
    }
    async fn limited(&self, track: Option<&str>, signal: Signal) -> bool {
        matches!(self.find_gain_stage(track, signal).await, Ok(Some(stage)) if stage.limited)
    }
    async fn chain(&self, track: Option<&str>) -> Result<Vec<Placed>, String> {
        self.chain_now(track, LiveRenderer::cleanup(self)).await.map_err(|error| head(&error.to_string(), 120))
    }
    async fn delete_device(&self, reference: &str) -> bool {
        self.step("delete_device", json!({"ref":reference}), LiveRenderer::cleanup(self)).await.is_ok()
    }

    async fn knobs(&self, device: &str, signal: Signal) -> Result<Result<Vec<DeviceKnob>, String>, RuntimeError> {
        self.device_knobs(device, signal).await
    }
    async fn set_knobs(&self, device: &str, values: &[(&str, f64)], signal: Signal) -> Result<(), RuntimeError> {
        self.set_knob_values(device, values, signal).await
    }
    async fn device_kind(&self, device: &str, signal: Signal) -> Option<String> {
        LiveRenderer::device_kind(self, device, signal).await
    }
    async fn fit_eq(
        &self,
        device: &str,
        knobs: &[DeviceKnob],
        bands: &[Band],
        signal: Signal,
    ) -> Result<Result<EqFit, String>, RuntimeError> {
        self.fit_eq_bands(device, knobs, bands, signal).await
    }
    async fn on_track(&self, device: &str, track: &str, signal: Signal) -> bool {
        // The copies are of the device's own track, so it has to be the run's (refs read long: a short one is a counter).
        let run_track = self.scope_ref(Some(track), signal).await.ok();
        run_track
            .is_some_and(|track| super::super::mutations::same_track(&LiveRenderer::connection(self).references.borrow(), device, &track))
    }
    async fn open_copies(
        &self,
        device: &str,
        count: usize,
        label: &str,
        passes: u32,
        window: Window,
        signal: Signal,
    ) -> Result<Result<Copies, String>, RuntimeError> {
        LiveRenderer::open_copies(self, device, count, (label, passes, window), signal).await
    }
    async fn set_copies(
        &self,
        copies: &Copies,
        values: serde_json::Map<String, Value>,
        signal: Signal,
    ) -> Result<Result<(), String>, RuntimeError> {
        LiveRenderer::set_copies(self, copies, values, signal).await
    }
    async fn close_copies(&self, copies: Copies) {
        LiveRenderer::close_copies(self, copies).await
    }

    async fn track_devices(&self, device: &str, signal: Signal) -> Result<(Vec<Seen>, usize), RuntimeError> {
        self.track_devices_of(device, signal).await
    }
    async fn listed_names(&self, device: &str, signal: Signal) -> Result<Option<Vec<String>>, RuntimeError> {
        self.listed_names_of(device, signal).await
    }
    async fn read_seen(&self, device: &mut Seen, signal: Signal) {
        LiveRenderer::read_seen(self, device, signal).await
    }
    fn eq_bands(&self, knobs: &[DeviceKnob]) -> Vec<(u8, f64, bool, bool)> {
        bands_of(knobs)
    }
    async fn devices_seen(&self, track: Option<&str>, signal: Signal) -> Option<Vec<Seen>> {
        self.devices_on(track, signal).await
    }

    async fn clip_notes(&self, clip: &str, signal: Signal) -> Result<ClipNotes, String> {
        self.clip_notes_of(clip, signal).await
    }
    async fn lanes_of(&self, clip: &str, signal: Signal) -> Kit {
        LiveRenderer::lanes_of(self, clip, signal).await
    }
    async fn reread_clip(&self, clip: &str, signal: Signal) -> Result<(), RuntimeError> {
        self.read_again(clip, signal).await
    }
    async fn change_notes(&self, clip: &str, patches: &[NotePatch], signal: Signal) -> Result<(), RuntimeError> {
        self.change_note_patches(clip, patches, signal).await
    }
    fn clip_id(&self, clip: &str) -> Option<String> {
        self.clip_long(clip)
    }
    fn note_changes(&self, checkpoint: &[String], clip: Option<&str>) -> (Vec<(String, String)>, Vec<(String, String)>) {
        self.note_changes_since(checkpoint, clip)
    }
    async fn stem_track(&self, audio: &str, signal: Signal) -> Result<String, String> {
        self.stem_track_name(audio, signal).await
    }

    fn render_refusal(&self) -> Option<String> {
        if !LiveRenderer::available(self) {
            return Some(NO_CURRENT_LIVE.to_string());
        }
        (!LiveRenderer::supported(self, RENDER_BRIDGE)).then(|| LiveRenderer::too_old(self, RENDER_BRIDGE))
    }
    async fn audition_pass(
        &self,
        candidates: &[AuditionCandidate],
        from_beat: Option<f64>,
        beats: Option<f64>,
        options: AuditionOptions,
        signal: Signal,
    ) -> AuditionPass {
        self.audition_pass_of(candidates, from_beat, beats, options, signal).await
    }
    async fn render(&self, track: &str, from: f64, beats: f64, signal: Signal) -> Result<Result<JsonObject, String>, RuntimeError> {
        self.render_offline(track, from, beats, signal).await
    }
    fn short_ref(&self, long: &str) -> String {
        LiveRenderer::connection(self).references.borrow_mut().short_ref(long)
    }
    fn emit(&self, record: ChangeRecord) {
        self.history.emit(&record)
    }
    fn now_ms(&self) -> i64 {
        LiveRenderer::connection(self).now().timestamp_millis()
    }

    async fn clips_by_track(&self, signal: Signal) -> Vec<(String, Vec<(f64, f64)>)> {
        self.arrangement_clips(signal).await
    }
}
