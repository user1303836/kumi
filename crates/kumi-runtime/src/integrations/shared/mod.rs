//! What every DAW's integration uses: reading parameter display text, finding samples on disk, the change engine's
//! DAW-neutral parts, and a DAW's manual kept and searched.
pub mod changes;
pub mod display;
pub mod manual;
pub mod samples;
pub mod steps;
pub mod undo;
