//! Where REAPER keeps its library, for the library to learn: its resource folders with Kumi's extension in them, found
//! where daw-bridge's detection finds REAPER (a producer who has REAPER but never works in it with Kumi doesn't wait
//! while its library is measured), and how it's laid out: its FX chains, track and project templates, JSFX and FX preset
//! banks.

use std::path::{Path, PathBuf};

use daw_bridge::detect::{detect_reaper, Env};
use daw_bridge::install::reaper_extension_name;
use kumi_daw_own::library::{os_of, source};
use kumi_library::daws::{Layout, LibrarySources, ProjectFormat};
use kumi_library::sources::{env_of, home_of, Source, SourceKind, SourceOptions};
use kumi_types::daw::{DawId, DawProfile};

/// REAPER's library: its resource folders with Kumi's extension in them.
pub struct ReaperLibrary;

/// REAPER's library, as the registry gives it to the library.
pub static LIBRARY: ReaperLibrary = ReaperLibrary;

/// REAPER's resource folders Kumi reads, and the file type each holds ("" is a JSFX: its file has no extension); a JSFX's
/// format is "jsfx", whatever its name ends with.
static LAYOUT: Layout = Layout {
    folders: &[
        ("fxchains", &[".rfxchain"]),
        ("tracktemplates", &[".rtracktemplate"]),
        ("projecttemplates", &[".rpp"]),
        ("effects", &["", ".jsfx"]),
        ("presets", &[".ini"]),
    ],
    sounds: &[],
    formats: &[("effects", "jsfx")],
};

impl LibrarySources for ReaperLibrary {
    fn profile(&self) -> &'static DawProfile {
        &DawProfile::REAPER
    }

    /// REAPER's resource folders with Kumi's extension in them (the default one, portable installs, or the one
    /// `KUMI_REAPER_RESOURCE_DIR` names).
    fn own_library(&self, options: &SourceOptions) -> Vec<Source> {
        let reapers = resources(options);
        reapers
            .iter()
            .map(|resource| {
                let label = if reapers.len() == 1 {
                    "REAPER".to_string()
                } else {
                    format!("REAPER ({})", resource.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default())
                };
                source(resource.clone(), label, SourceKind::Daw(DawId::Reaper))
            })
            .collect()
    }

    /// The projects REAPER opened lately, newest first: its `reaper.ini`'s recent list, in each REAPER with Kumi's
    /// extension in it.
    fn recent_projects(&self, options: &SourceOptions) -> Vec<String> {
        resources(options).iter().flat_map(|resource| recent_in(&resource.join("reaper.ini"))).collect()
    }

    fn layout(&self) -> &'static Layout {
        &LAYOUT
    }

    fn projects(&self) -> Option<&'static ProjectFormat> {
        Some(&crate::projects::FORMAT)
    }
}

/// REAPER's resource folders with Kumi's extension in them.
fn resources(options: &SourceOptions) -> Vec<PathBuf> {
    let os = os_of(options);
    let var = |name: &str| env_of(options, name);
    let env = Env { os, home: PathBuf::from(home_of(options)), var: &var, root: None };
    detect_reaper(&env)
        .into_iter()
        .map(|install| install.resource)
        .filter(|resource| resource.join("UserPlugins").join(reaper_extension_name(os)).is_file())
        .collect()
}

/// The projects a `reaper.ini` lists in its `[Recent]` section, the newest first: REAPER numbers them oldest first
/// (`recent01=…`, as measured on 7.82).
fn recent_in(ini: &Path) -> Vec<String> {
    let Ok(bytes) = std::fs::read(ini) else { return vec![] };
    let text = String::from_utf8_lossy(&bytes);
    let mut recent: Vec<(u32, String)> = vec![];
    let mut inside = false;
    for line in text.lines().map(str::trim) {
        if line.starts_with('[') {
            inside = line.eq_ignore_ascii_case("[Recent]");
            continue;
        }
        let Some((key, path)) = line.split_once('=').filter(|_| inside) else { continue };
        let number = key.trim().to_ascii_lowercase().strip_prefix("recent").and_then(|n| n.parse::<u32>().ok());
        if let (Some(number), false) = (number, path.trim().is_empty()) {
            recent.push((number, path.trim().to_string()));
        }
    }
    recent.sort_by_key(|(number, _)| std::cmp::Reverse(*number));
    recent.into_iter().map(|(_, path)| path).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    #[test]
    fn recent_projects_are_reapers_own_list_newest_first_where_kumis_extension_is() {
        let home = tempfile::tempdir().unwrap();
        let resource = home.path().join("REAPER");
        std::fs::create_dir_all(resource.join("UserPlugins")).unwrap();
        std::fs::write(
            resource.join("reaper.ini"),
            "[REAPER]\nrecent01=/not/this.rpp\n[Recent]\nrecent02=/Music/Newest.rpp\nrecent01=/Music/Older.rpp\nrecent10=\nother=/x.rpp\n[RecentFX]\nrecent03=/fx.rpp\n",
        )
        .unwrap();
        let options = SourceOptions {
            home: Some(home.path().to_string_lossy().into()),
            platform: Some("darwin".into()),
            env: Some(HashMap::from([("KUMI_REAPER_RESOURCE_DIR".into(), resource.to_string_lossy().into_owned())])),
            ..Default::default()
        };
        // Not while Kumi's extension isn't in that REAPER.
        assert!(LIBRARY.recent_projects(&options).is_empty());
        std::fs::write(resource.join("UserPlugins").join(reaper_extension_name(daw_bridge::detect::Os::Mac)), "an extension").unwrap();
        assert_eq!(LIBRARY.recent_projects(&options), ["/Music/Newest.rpp", "/Music/Older.rpp"]);
    }
}
