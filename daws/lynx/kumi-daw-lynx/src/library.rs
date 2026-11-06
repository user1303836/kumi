//! LYNX contributes its profile only; project and preset learning is not implemented.
use kumi_library::daws::LibrarySources;
use kumi_types::daw::DawProfile;

pub struct LynxLibrary;
pub static LIBRARY: LynxLibrary = LynxLibrary;
impl LibrarySources for LynxLibrary {
    fn profile(&self) -> &'static DawProfile {
        &DawProfile::LYNX
    }
}
