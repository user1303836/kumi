//! Kumi without Live (integrations/none.rs) in Live's words and with Live's instructions, explicitly or while Live is away.
use chrono::{DateTime, Utc};
use kumi_agent::core::contracts::*;
use kumi_agent::integrations::none;
use kumi_types::daw::DawProfile;
use std::rc::Rc;

pub(crate) fn no_access(key: &str, now: DateTime<Utc>, project: Option<ProjectRef>) -> Observation {
    none::no_access(&DawProfile::ABLETON, super::context::INSTRUCTIONS, key, now, project)
}

pub fn create_inference_only_integration(on_connection: Rc<dyn Fn(ConnectionState)>) -> Rc<dyn Integration> {
    none::create_no_daw_integration(DawProfile::ABLETON, super::context::INSTRUCTIONS, on_connection)
}
