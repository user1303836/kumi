//! Live's device builder (make_device): Max for Live devices made, played in a harness and checked. The .amxd format
//! itself is shared (crate::devices::amxd).
pub mod amxd;
pub mod existing;
pub mod face;
pub mod findings;
pub mod gen;
pub mod harness;
pub mod harness_child;
pub mod kit;
pub mod look;
pub mod mapping;
pub mod midi;
pub mod patch;
pub mod patched;
pub mod spec;
pub mod tool;
