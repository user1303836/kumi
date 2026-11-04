//! What SoundCraft tells the library: its profile only. Its sessions, presets and samples aren't learned yet, so nothing
//! the library keeps names SoundCraft (an older Kumi, rolled back to, reads all of it).
use kumi_library::daws::LibrarySources;
use kumi_types::daw::DawProfile;

pub struct SoundCraftLibrary;

pub static LIBRARY: SoundCraftLibrary = SoundCraftLibrary;

impl LibrarySources for SoundCraftLibrary {
    fn profile(&self) -> &'static DawProfile {
        &DawProfile::SOUNDCRAFT
    }
}
