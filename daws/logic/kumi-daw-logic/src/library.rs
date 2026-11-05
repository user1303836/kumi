//! Logic's profile only. Library sources and project learning arrive with M6.
use kumi_library::daws::LibrarySources;
use kumi_types::daw::DawProfile;

pub struct LogicLibrary;
pub static LIBRARY: LogicLibrary = LogicLibrary;
impl LibrarySources for LogicLibrary {
    fn profile(&self) -> &'static DawProfile {
        &DawProfile::LOGIC
    }
}
