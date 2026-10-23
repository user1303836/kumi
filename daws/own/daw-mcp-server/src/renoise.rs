//! What the host has for Renoise alone: its native devices and how their values read, the Doofer, Splitter, Notepad and
//! instrument changes Kumi makes in Renoise's own files (XML, `.xrni`), the library files it takes for the tool, and the
//! scripted Renoise song its tests and Kumi's run against.

pub mod devices;
pub mod instruments;
pub mod library_files;
pub mod presets;
pub mod simulator;
pub mod texts;
pub mod xml;
pub mod zip;
