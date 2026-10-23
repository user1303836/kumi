//! Knobs named by role in tune, and fixes that name a mapped plug-in's role. A track is read as tune sees it (the DAW
//! reads it): each device's name, class and whether it's on, the parameters the DAW lets Kumi turn, and for a plug-in
//! every name it lists, so the plug-in map's names are checked against the DAW's own before any is used.

use super::Listening;
use crate::{
    core::errors::RuntimeError,
    listening::{
        renderer::DeviceKnob,
        round::Round,
        tools::{TuneHow, TuneRequest},
    },
    plugins::roles::{self, is_plugin, Found, Resolved},
};
use kumi_common::abort::{Signal, SignalExt};
use std::rc::Rc;

impl Listening {
    /// The tune request with its knobs named by role found: on its device, or else on another of its track, in the
    /// agreed order. An EQ band named by role on an EQ Eight is the band nearest `near` (the target's frequency, Hz)
    /// when it's known. None when no knob is named by role; why, when nothing can turn one.
    pub(crate) async fn knobs_by_role(
        &self,
        request: &TuneRequest,
        knobs: &[DeviceKnob],
        near: Option<f64>,
        signal: Signal,
    ) -> Result<Result<Option<TuneRequest>, String>, RuntimeError> {
        let turnable: Vec<String> = knobs.iter().map(|knob| knob.name.clone()).collect();
        if request.how == TuneHow::Fit || request.knobs.iter().all(|word| turnable.iter().any(|name| roles::same(name, word))) {
            return Ok(Ok(None));
        }
        let (mut devices, asked) = self.host.track_devices(&request.device, signal.clone()).await?;
        devices[asked].turnable = turnable.to_vec();
        devices[asked].bands = self.host.eq_bands(knobs);
        if is_plugin(&devices[asked].class) {
            devices[asked].listed = self.host.listed_names(&request.device, signal.clone()).await?;
        }
        // The rest of the track is read only when the device can't turn a role.
        if request.knobs.iter().any(|word| matches!(roles::find(&devices[asked], word), Some(Found::Missing(_)))) {
            for at in roles::agreed_order(&devices, Some(asked)) {
                self.host.read_seen(&mut devices[at], signal.clone()).await;
            }
            signal.check()?;
        }
        Ok(match roles::resolve_near(&devices, asked, &request.knobs, near) {
            Resolved::AsAsked => Ok(None),
            Resolved::On { device, knobs, read, instead } => {
                let mut found = request.clone();
                if device != asked {
                    found.device = devices[device].reference.clone();
                }
                found.knobs = knobs;
                // The log says why another device, and how each role was read.
                let said: Vec<String> = request.change.iter().cloned().chain(instead).chain(std::iter::once(read.join("; "))).collect();
                found.change = Some(
                    said.iter()
                        .map(|part| part.trim().trim_end_matches('.'))
                        .filter(|part| !part.is_empty())
                        .collect::<Vec<_>>()
                        .join(". "),
                );
                Ok(Some(found))
            }
            Resolved::Refused(why) => Err(why),
        })
    }

    /// A round's next fix names a mapped plug-in's role when one on the run's track does the job (Ozone 12 first, then
    /// the other mapped plug-ins), with the fix it had after it. It stays as it was when none does, or Live can't say.
    pub async fn name_plugin_roles(self: &Rc<Self>, round: &mut Round, signal: Signal) {
        let Some(job) = round.next.as_ref().and_then(|next| roles::job_for_item(&next.id)) else { return };
        let Some(track) = self.judge.borrow().as_ref().map(|run| run.track.clone()) else { return };
        let Some(mut devices) = self.host.devices_seen(track.as_deref(), signal.clone()).await else { return };
        let doing: Vec<usize> = (0..devices.len())
            .filter(|at| !devices[*at].off && devices[*at].adapter().is_some_and(|adapter| roles::job_role(adapter, job).is_some()))
            .collect();
        if doing.is_empty() {
            return;
        }
        for at in doing {
            self.host.read_seen(&mut devices[at], signal.clone()).await;
        }
        if let Some(next) = round.next.as_mut() {
            if let Some(fix) = roles::plugin_fix(&devices, job, next.fix.as_deref()) {
                next.fix = Some(fix);
            }
        }
    }
}
