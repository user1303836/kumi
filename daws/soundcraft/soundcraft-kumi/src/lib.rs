//! Kumi's bridge beside SoundCraft. SoundCraft can't load anything of Kumi's, so the bridge is a program of its own, which
//! Kumi's host starts. It serves `kumi-daw/1` to the host and drives the app through its control channel. So far it
//! holds the channel's client.

pub mod control;
