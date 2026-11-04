//! What the integrations of the DAWs whose bridges are Kumi's own (REAPER's kumi-daw-reaper, Renoise's
//! kumi-daw-renoise) share, host side: the integration itself (`bridge`), each change as the shared engine plans and
//! takes it back (`changes`), catching up, listening, the ears (audio to MIDI, stems), instant phrases, the pictures and
//! FOCUS views of their changes and projects, kept scripts, delivery to a spec, goals and groove, and what their
//! libraries share. What every DAW's integration shares, Live's too, is the runtime's (`integrations::shared`). Their
//! providers: `provider`'s `Own` for a bridge loaded into the DAW, `beside`'s `Beside` for a program beside Kumi.
pub mod beside;
pub mod bridge;
pub mod catchup;
pub mod changes;
pub mod ears;
mod goal;
mod groove;
pub mod library;
pub mod listening;
pub mod phrases;
pub mod pictures;
pub mod provider;
mod scripts;
pub mod spec;
pub mod views;
