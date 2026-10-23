//! The M4 kinds of FX and envelopes, by name, and what each one does (in `fx_more`, `fx_params`, `containers`,
//! `modulation` and `envelopes`).

use serde_json::Value;

use super::{containers, envelopes, fx_more, fx_params, modulation, Applied, Ctx, Done, Preview, Restore, Restored};

#[derive(Clone, Copy)]
pub enum Kind {
    Offline,
    Params,
    Wet,
    EqBand,
    Preset,
    State,
    ChainLoad,
    Copy,
    ContainerAdd,
    ContainerMove,
    Parallel,
    ContainerMap,
    ContainerUnmap,
    Modulation(&'static modulation::Family),
    EnvelopeShow,
    Points(envelopes::Edit),
    AutoItemAdd,
    AutoItemSet,
    AutoItemDelete,
    AutomationMode,
    SendAutomationMode,
    AutomationOverride,
}

pub fn kind(name: &str) -> Option<Kind> {
    if let Some(family) = modulation::FAMILIES.iter().find(|family| family.kind == name) {
        return Some(Kind::Modulation(family));
    }
    match name {
        "fx-offline" => Some(Kind::Offline),
        "fx-params" => Some(Kind::Params),
        "fx-wet" => Some(Kind::Wet),
        "fx-eq-band" => Some(Kind::EqBand),
        "fx-preset" => Some(Kind::Preset),
        "fx-state" => Some(Kind::State),
        "fx-chain-load" => Some(Kind::ChainLoad),
        "fx-copy" => Some(Kind::Copy),
        "fx-container-add" => Some(Kind::ContainerAdd),
        "fx-container-move" => Some(Kind::ContainerMove),
        "fx-parallel" => Some(Kind::Parallel),
        "fx-container-map" => Some(Kind::ContainerMap),
        "fx-container-unmap" => Some(Kind::ContainerUnmap),
        "envelope-show" => Some(Kind::EnvelopeShow),
        "envelope-points-add" => Some(Kind::Points(envelopes::Edit::Add)),
        "envelope-points-change" => Some(Kind::Points(envelopes::Edit::Change)),
        "envelope-points-delete" => Some(Kind::Points(envelopes::Edit::Delete)),
        "envelope-range-clear" => Some(Kind::Points(envelopes::Edit::Clear)),
        "automation-item-add" => Some(Kind::AutoItemAdd),
        "automation-item-set" => Some(Kind::AutoItemSet),
        "automation-item-delete" => Some(Kind::AutoItemDelete),
        "track-automation-mode" => Some(Kind::AutomationMode),
        "send-automation-mode" => Some(Kind::SendAutomationMode),
        "automation-override" => Some(Kind::AutomationOverride),
        _ => None,
    }
}

impl Kind {
    /// Whether a change of this kind leaves a point in REAPER's undo history: the global automation override does not.
    pub fn leaves_point(self) -> bool {
        !matches!(self, Kind::AutomationOverride)
    }

    pub fn preview(self, ctx: &Ctx, input: &Value) -> Done<Preview> {
        match self {
            Kind::Offline => fx_more::offline_preview(ctx, input),
            Kind::Params => fx_params::preview(ctx, input),
            Kind::Wet => fx_more::wet_preview(ctx, input),
            Kind::EqBand => fx_more::eq_band_preview(ctx, input),
            Kind::Preset => fx_more::preset_preview(ctx, input),
            Kind::State => fx_more::state_preview(ctx, input),
            Kind::ChainLoad => fx_more::chain_load_preview(ctx, input),
            Kind::Copy => fx_more::copy_preview(ctx, input),
            Kind::ContainerAdd => containers::add_preview(ctx, input),
            Kind::ContainerMove => containers::move_preview(ctx, input),
            Kind::Parallel => containers::parallel_preview(ctx, input),
            Kind::ContainerMap => containers::map_preview(ctx, input),
            Kind::ContainerUnmap => containers::unmap_preview(ctx, input),
            Kind::Modulation(family) => modulation::preview(ctx, family, input),
            Kind::EnvelopeShow => envelopes::show_preview(ctx, input),
            Kind::Points(edit) => envelopes::points_preview(ctx, edit, input),
            Kind::AutoItemAdd => envelopes::item_add_preview(ctx, input),
            Kind::AutoItemSet => envelopes::item_set_preview(ctx, input),
            Kind::AutoItemDelete => envelopes::item_delete_preview(ctx, input),
            Kind::AutomationMode => envelopes::mode_preview(ctx, input),
            Kind::SendAutomationMode => envelopes::send_mode_preview(ctx, input),
            Kind::AutomationOverride => envelopes::override_preview(ctx, input),
        }
    }

    pub fn apply(self, ctx: &Ctx, input: &Value, fence: &Value) -> Done<Applied> {
        match self {
            Kind::Offline => fx_more::offline_apply(ctx, input, fence),
            Kind::Params => fx_params::apply(ctx, input, fence),
            Kind::Wet => fx_more::wet_apply(ctx, input, fence),
            Kind::EqBand => fx_more::eq_band_apply(ctx, input, fence),
            Kind::Preset => fx_more::preset_apply(ctx, input, fence),
            Kind::State => fx_more::state_apply(ctx, input, fence),
            Kind::ChainLoad => fx_more::chain_load_apply(ctx, input, fence),
            Kind::Copy => fx_more::copy_apply(ctx, input, fence),
            Kind::ContainerAdd => containers::add_apply(ctx, input, fence),
            Kind::ContainerMove => containers::move_apply(ctx, input, fence),
            Kind::Parallel => containers::parallel_apply(ctx, input, fence),
            Kind::ContainerMap => containers::map_apply(ctx, input, fence),
            Kind::ContainerUnmap => containers::unmap_apply(ctx, input, fence),
            Kind::Modulation(family) => modulation::apply(ctx, family, input, fence),
            Kind::EnvelopeShow => envelopes::show_apply(ctx, input, fence),
            Kind::Points(edit) => envelopes::points_apply(ctx, edit, input, fence),
            Kind::AutoItemAdd => envelopes::item_add_apply(ctx, input, fence),
            Kind::AutoItemSet => envelopes::item_set_apply(ctx, input, fence),
            Kind::AutoItemDelete => envelopes::item_delete_apply(ctx, input, fence),
            Kind::AutomationMode => envelopes::mode_apply(ctx, input, fence),
            Kind::SendAutomationMode => envelopes::send_mode_apply(ctx, input, fence),
            Kind::AutomationOverride => envelopes::override_apply(ctx, input, fence),
        }
    }

    pub fn restore(self, ctx: &Ctx, restore: &Restore) -> Done<Restored> {
        match self {
            Kind::Offline => fx_more::offline_restore(ctx, restore),
            Kind::Params => fx_params::restore(ctx, restore),
            Kind::Wet => fx_more::wet_restore(ctx, restore),
            Kind::EqBand => fx_more::eq_band_restore(ctx, restore),
            Kind::Preset => fx_more::preset_restore(ctx, restore),
            Kind::State => fx_more::state_restore(ctx, restore),
            Kind::ChainLoad => fx_more::chain_load_restore(ctx, restore),
            Kind::Copy => fx_more::copy_restore(ctx, restore),
            Kind::ContainerAdd => containers::add_restore(ctx, restore),
            Kind::ContainerMove => containers::move_restore(ctx, restore),
            Kind::Parallel => containers::parallel_restore(ctx, restore),
            Kind::ContainerMap => containers::map_restore(ctx, restore),
            Kind::ContainerUnmap => containers::unmap_restore(ctx, restore),
            Kind::Modulation(family) => modulation::restore(ctx, family, restore),
            Kind::EnvelopeShow => envelopes::show_restore(ctx, restore),
            Kind::Points(_) => envelopes::points_restore(ctx, restore),
            Kind::AutoItemAdd => envelopes::item_add_restore(ctx, restore),
            Kind::AutoItemSet => envelopes::item_set_restore(ctx, restore),
            Kind::AutoItemDelete => envelopes::item_delete_restore(ctx, restore),
            Kind::AutomationMode => envelopes::mode_restore(ctx, restore),
            Kind::SendAutomationMode => envelopes::send_mode_restore(ctx, restore),
            Kind::AutomationOverride => envelopes::override_restore(ctx, restore),
        }
    }
}
