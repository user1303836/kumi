//! Kumi's shared types and settings, the lowest layer of its own crates: what the DAWs are and how each is worded
//! (`daw`), the contracts between the agent and its tools (`contracts`), errors, disk and private files, places, the
//! system's environment, the version and the command's name; and Max for Live's device file (`amxd`), which the web
//! tool reads as well as Live's device builder.

pub mod amxd;
pub mod command;
pub mod contracts;
pub mod daw;
pub mod disk;
pub mod errors;
pub mod matching;
pub mod notes;
pub mod picture;
pub mod place;
pub mod private;
pub mod system;
pub mod version;
