//! Where REAPER keeps its library, for the library to learn: its resource folders with Kumi's extension in them, found
//! where daw-bridge's detection finds REAPER (a producer who has REAPER but never works in it with Kumi doesn't wait
//! while its library is measured). How the library is read, by its layout, is the library's own
//! (`kumi_library::daw_sources`).

use std::path::PathBuf;

use daw_bridge::detect::{detect_reaper, Env};
use daw_bridge::install::reaper_extension_name;
use kumi_library::daws::LibrarySources;
use kumi_library::sources::{env_of, home_of, Source, SourceKind, SourceOptions};
use kumi_runtime::integrations::own_library::{os_of, source};
use kumi_types::daw::DawProfile;

/// REAPER's library: its resource folders with Kumi's extension in them.
pub struct ReaperLibrary;

/// REAPER's library, as the registry gives it to the library.
pub static LIBRARY: ReaperLibrary = ReaperLibrary;

impl LibrarySources for ReaperLibrary {
    fn profile(&self) -> &'static DawProfile {
        &DawProfile::REAPER
    }

    /// REAPER's resource folders with Kumi's extension in them (the default one, portable installs, or the one
    /// `KUMI_REAPER_RESOURCE_DIR` names).
    fn own_library(&self, options: &SourceOptions) -> Vec<Source> {
        let os = os_of(options);
        let var = |name: &str| env_of(options, name);
        let env = Env { os, home: PathBuf::from(home_of(options)), var: &var, root: None };
        let reapers: Vec<PathBuf> = detect_reaper(&env)
            .into_iter()
            .map(|install| install.resource)
            .filter(|resource| resource.join("UserPlugins").join(reaper_extension_name(os)).is_file())
            .collect();
        reapers
            .iter()
            .map(|resource| {
                let label = if reapers.len() == 1 {
                    "REAPER".to_string()
                } else {
                    format!("REAPER ({})", resource.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default())
                };
                source(resource.clone(), label, SourceKind::Reaper)
            })
            .collect()
    }
}
