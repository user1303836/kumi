//! Where each DAW's bridge goes, from what detection found.
use crate::daw::Daw;
use crate::detect::{Arch, Os, ReaperInstall, RenoiseInstall, RenoisePrefs, Version, REAPER_MIN, RENOISE_MIN};
use crate::lifecycle::Target;
use std::path::Path;

/// The Renoise tool's id: song `tool_data` is keyed by it, so it never changes.
pub const RENOISE_TOOL_ID: &str = "com.kumi.Bridge";
pub const RENOISE_TOOL_FOLDER: &str = "com.kumi.Bridge.xrnx";
/// Inside the Renoise tool's folder.
pub const RENOISE_REFERENCE: &str = "bridge-reference.json";
/// Beside REAPER's extension in UserPlugins (REAPER loads only `reaper_*` libraries, so it ignores this). In `paths`,
/// which the extension links (it finds its reference by this name).
pub use crate::paths::REAPER_REFERENCE;

/// REAPER loads `reaper_*.dylib|dll|so` from UserPlugins: one file per platform (macOS's is universal).
pub fn reaper_extension_name(os: Os) -> &'static str {
    match os {
        Os::Mac => "reaper_kumi.dylib",
        Os::Windows => "reaper_kumi.dll",
        Os::Linux => "reaper_kumi.so",
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TargetError {
    /// The DAW hasn't run with this folder yet (REAPER makes its resource folder, Renoise its version folder).
    NeverOpened(String),
    /// Every one of the DAW here whose version is known is older than Kumi's bridge needs: the newest one's version.
    Older(Daw, String),
}

/// The oldest version of the DAW Kumi's bridge works with, as people write it ("7", "3.5").
pub fn min_version(daw: Daw) -> &'static str {
    match daw {
        Daw::Reaper => "7",
        Daw::Renoise => "3.5",
    }
}

impl std::fmt::Display for TargetError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            TargetError::NeverOpened(why) => f.write_str(why),
            TargetError::Older(daw, version) => {
                write!(f, "this is {} {version}; Kumi needs {} {} or newer", daw.name(), daw.name(), min_version(*daw))
            }
        }
    }
}

/// The REAPER Kumi's extension goes in: of those Kumi works with (7 or newer, or whose version isn't known), the first
/// that has run. None when there's no REAPER here.
pub fn reaper_install(installs: &[ReaperInstall]) -> Result<&ReaperInstall, Option<TargetError>> {
    let known = |install: &ReaperInstall| install.version.as_deref().and_then(Version::of_app);
    let usable: Vec<&ReaperInstall> =
        installs.iter().filter(|install| known(install).is_none_or(|version| version >= REAPER_MIN)).collect();
    if let Some(install) = usable.iter().find(|install| install.opened).or(usable.first()) {
        return Ok(install);
    }
    let newest =
        installs.iter().filter_map(|install| Some((known(install)?, install.version.clone()?))).max_by_key(|(version, _)| *version);
    Err(newest.map(|(_, version)| TargetError::Older(Daw::Reaper, version)))
}

impl std::error::Error for TargetError {}

/// The extension goes in `<resource>/UserPlugins` (made when the resource folder exists).
pub fn reaper_target(install: &ReaperInstall, os: Os) -> Result<Target, TargetError> {
    if !install.resource.is_dir() {
        return Err(TargetError::NeverOpened(format!("open REAPER once first: {} doesn't exist yet", install.resource.display())));
    }
    let dir = install.user_plugins();
    Ok(Target { daw: Daw::Reaper, reference: dir.join(REAPER_REFERENCE), name: reaper_extension_name(os).into(), folder: false, dir })
}

/// The tool goes in the newest version folder at or above 3.5, in `Scripts/Tools`. Without one, a Renoise older than
/// 3.5 is named (by its app, or with no app found by its newest folder); any other hasn't been opened yet.
pub fn renoise_target(install: &RenoiseInstall) -> Result<Target, TargetError> {
    if let Some(prefs) = &install.target {
        return Ok(tool_in(prefs));
    }
    let versions: Vec<(Option<Version>, String)> = if install.apps.is_empty() {
        install.prefs.iter().map(|prefs| (Some(prefs.version), prefs.version.to_string())).collect()
    } else {
        install.apps.iter().map(|app| (app.version.as_deref().and_then(Version::of_app), app.version.clone().unwrap_or_default())).collect()
    };
    let older = versions.iter().all(|(version, _)| version.is_some_and(|version| version < RENOISE_MIN));
    match versions.into_iter().filter_map(|(version, said)| Some((version?, said))).max_by_key(|(version, _)| *version) {
        Some((_, said)) if older => Err(TargetError::Older(Daw::Renoise, said)),
        _ => Err(TargetError::NeverOpened("open Renoise 3.5 or newer once first: it has no preferences folder for 3.5 yet".into())),
    }
}

/// The tool's place in each version folder at or above 3.5: where a newer Renoise may have copied Kumi's tool, for the
/// lifecycle to follow and take out with it.
pub fn renoise_targets(install: &RenoiseInstall) -> Vec<Target> {
    install.prefs.iter().filter(|prefs| prefs.version >= RENOISE_MIN).map(tool_in).collect()
}

fn tool_in(prefs: &RenoisePrefs) -> Target {
    let dir = prefs.tools();
    Target {
        daw: Daw::Renoise,
        reference: dir.join(RENOISE_TOOL_FOLDER).join(RENOISE_REFERENCE),
        name: RENOISE_TOOL_FOLDER.into(),
        folder: true,
        dir,
    }
}

/// Whether an extension can load into an app: they share an architecture (REAPER under Rosetta runs x86_64, so a
/// universal extension covers both). Unknown on either side counts as fitting.
pub fn arch_fits(extension: &Path, app_archs: &[Arch]) -> bool {
    let ours = crate::detect::binary_archs(extension);
    ours.is_empty() || app_archs.is_empty() || ours.iter().any(|arch| app_archs.contains(arch))
}

/// Why the REAPER Kumi would put its extension in can't load it (a 32-bit REAPER on Windows can't load the x64 DLL),
/// in words; None when it can, or either side's architecture isn't known. Install refuses then, rather than say the
/// extension is in a REAPER that will never load it.
pub fn unloadable(extension: &Path, install: &ReaperInstall) -> Option<String> {
    if arch_fits(extension, &install.archs) {
        return None;
    }
    let names = |archs: &[Arch]| {
        let words: Vec<&str> = archs
            .iter()
            .map(|arch| match arch {
                Arch::X86_64 => "x64",
                Arch::Arm64 => "ARM64",
                Arch::Arm64ec => "ARM64EC",
                Arch::X86 => "32-bit x86",
                Arch::Arm => "32-bit ARM",
                Arch::Other => "another architecture",
            })
            .collect();
        words.join(" and ")
    };
    let app = install.app.as_ref().map(|app| format!(" ({})", app.display())).unwrap_or_default();
    Some(format!(
        "this REAPER{app} is {}, and Kumi's extension is {}, which it can't load",
        names(&install.archs),
        names(&crate::detect::binary_archs(extension))
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::detect::{RenoiseApp, RenoisePrefs, ResourceFrom, Version};

    #[test]
    fn a_reaper_that_cant_load_the_extension_is_said_before_it_goes_in() {
        let root = tempfile::tempdir().unwrap();
        // A PE header for x64, as the Windows extension has.
        let mut pe = vec![0u8; 0x200];
        pe[..2].copy_from_slice(b"MZ");
        pe[0x3c..0x40].copy_from_slice(&0x80u32.to_le_bytes());
        pe[0x80..0x84].copy_from_slice(b"PE\0\0");
        pe[0x84..0x86].copy_from_slice(&0x8664u16.to_le_bytes());
        let extension = root.path().join("reaper_kumi.dll");
        std::fs::write(&extension, &pe).unwrap();
        let reaper = |archs: Vec<Arch>| ReaperInstall {
            app: Some(root.path().join("REAPER").join("reaper.exe")),
            version: None,
            archs,
            resource: root.path().join("REAPER"),
            resource_from: ResourceFrom::Portable,
            opened: true,
        };
        let said = unloadable(&extension, &reaper(vec![Arch::X86])).expect("a 32-bit REAPER can't load it");
        assert!(said.contains("is 32-bit x86, and Kumi's extension is x64, which it can't load"), "{said}");
        assert!(said.contains("reaper.exe"), "{said}");
        assert_eq!(unloadable(&extension, &reaper(vec![Arch::X86_64])), None);
        assert_eq!(unloadable(&extension, &reaper(vec![])), None, "an app whose architecture isn't known");
    }

    #[test]
    fn targets_need_the_folder_the_daw_makes() {
        let root = tempfile::tempdir().unwrap();
        let mut reaper = ReaperInstall {
            app: None,
            version: None,
            archs: vec![],
            resource: root.path().join("REAPER"),
            resource_from: ResourceFrom::Default,
            opened: false,
        };
        assert!(matches!(reaper_target(&reaper, Os::Mac), Err(TargetError::NeverOpened(_))));
        std::fs::create_dir(&reaper.resource).unwrap();
        reaper.opened = true;
        let target = reaper_target(&reaper, Os::Linux).unwrap();
        assert_eq!(target.path(), root.path().join("REAPER/UserPlugins/reaper_kumi.so"));
        assert_eq!(target.reference, root.path().join("REAPER/UserPlugins/kumi-bridge-reference.json"));

        let mut renoise = RenoiseInstall { apps: vec![], prefs: vec![], target: None, overridden: false };
        assert!(renoise_target(&renoise).is_err());
        renoise.target = Some(RenoisePrefs { version: Version(3, 5, 4), dir: root.path().join("V3.5.4") });
        let target = renoise_target(&renoise).unwrap();
        assert_eq!(target.path(), root.path().join("V3.5.4/Scripts/Tools/com.kumi.Bridge.xrnx"));
        assert!(target.folder);
    }

    /// A REAPER 6 or a Renoise 3.4 is named by its version, never taken for one that hasn't been opened yet; one whose
    /// version isn't known, or a newer one beside it, is used.
    #[test]
    fn an_older_daw_is_named_by_its_version() {
        let reaper = |version: Option<&str>, opened: bool| ReaperInstall {
            app: Some("REAPER.app".into()),
            version: version.map(str::to_string),
            archs: vec![],
            resource: "REAPER".into(),
            resource_from: ResourceFrom::Default,
            opened,
        };
        let older = [reaper(Some("6.83"), true), reaper(Some("5.99"), true)];
        assert_eq!(reaper_install(&older), Err(Some(TargetError::Older(Daw::Reaper, "6.83".into()))));
        assert_eq!(TargetError::Older(Daw::Reaper, "6.83".into()).to_string(), "this is REAPER 6.83; Kumi needs REAPER 7 or newer");
        let with_seven = [reaper(Some("6.83"), true), reaper(Some("7.22"), false)];
        assert_eq!(reaper_install(&with_seven).unwrap().version.as_deref(), Some("7.22"), "the REAPER Kumi works with, though not opened");
        assert!(reaper_install(&[reaper(None, false)]).is_ok(), "a version that isn't known (Windows, Linux) isn't older");
        assert_eq!(reaper_install(&[]), Err(None));

        let app = |version: Option<&str>| RenoiseApp {
            path: "Renoise.app".into(),
            version: version.map(str::to_string),
            demo: None,
            archs: vec![],
        };
        let prefs = |version: Version| RenoisePrefs { version, dir: format!("V{version}").into() };
        let renoise =
            |apps: Vec<RenoiseApp>, folders: Vec<RenoisePrefs>| RenoiseInstall { apps, prefs: folders, target: None, overridden: false };
        let said = renoise_target(&renoise(vec![app(Some("3.4.4")), app(Some("3.3.2"))], vec![prefs(Version(3, 4, 4))])).unwrap_err();
        assert_eq!(said.to_string(), "this is Renoise 3.4.4; Kumi needs Renoise 3.5 or newer");
        let said = renoise_target(&renoise(vec![], vec![prefs(Version(3, 4, 4))])).unwrap_err();
        assert_eq!(said, TargetError::Older(Daw::Renoise, "3.4.4".into()), "no app found: its newest folder says");
        for apps in [vec![app(Some("3.4.4")), app(Some("3.5.4 Demo"))], vec![app(None)], vec![]] {
            let said = renoise_target(&renoise(apps.clone(), vec![])).unwrap_err();
            assert!(matches!(said, TargetError::NeverOpened(_)), "{apps:?}: {said}");
        }
    }
}
