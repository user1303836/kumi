//! Logic's library profile and its manual, available without a running DAW.
use kumi_library::daws::LibrarySources;
use kumi_types::daw::DawProfile;

pub struct LogicLibrary;
pub static LIBRARY: LogicLibrary = LogicLibrary;
impl LibrarySources for LogicLibrary {
    fn manual_tool(&self, options: kumi_library::daws::ManualOptions) -> Option<std::rc::Rc<dyn kumi_types::contracts::KernelTool>> {
        Some(crate::manual::manual_tool(options))
    }

    fn profile(&self) -> &'static DawProfile {
        &DawProfile::LOGIC
    }
}
