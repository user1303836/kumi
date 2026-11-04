//! Live extension folders, as bridge setup and doctor find them.
use kumi_daw_ableton::app::live_extension::*;
use kumi_types::system::Env;
use serde_json::json;
use std::{fs, path::Path};
fn extension(folder: &Path, code: &str) {
    fs::create_dir_all(folder.join("dist")).unwrap();
    fs::write(folder.join("manifest.json"), json!({"name":"kumi","author":"Kumi","version":"1.0.0"}).to_string()).unwrap();
    fs::write(folder.join("dist/extension.js"), code).unwrap();
}
#[test]
fn extension_folders_only_use_the_given_environment() {
    assert_eq!(live_extensions_dir(&Env::new(), "darwin"), None);
    assert_eq!(
        live_extensions_dir(&Env::from([("HOME".into(), "/Users/p".into())]), "darwin"),
        // The source's platform argument selects the layout; node:path.join still uses the host's separators.
        Some(
            if cfg!(windows) {
                r"\Users\p\Library\Application Support\Ableton\Extensions"
            } else {
                "/Users/p/Library/Application Support/Ableton/Extensions"
            }
            .into()
        )
    );
    let env =
        Env::from([("LOCALAPPDATA".into(), "C:/Users/p/AppData/Local".into()), ("APPDATA".into(), "C:/Users/p/AppData/Roaming".into())]);
    assert_eq!(
        live_extensions_dir(&env, "win32"),
        Some(
            if cfg!(windows) { r"C:\Users\p\AppData\Local\Ableton\Extensions" } else { "C:/Users/p/AppData/Local/Ableton/Extensions" }
                .into()
        )
    );
    assert_eq!(live_extensions_dir(&Env::from([("APPDATA".into(), "x".into())]), "win32"), None);
    assert_eq!(live_extensions_dir(&Env::from([("HOME".into(), "/home/p".into())]), "linux"), None);
    assert_eq!(
        live_extensions_dir(
            &Env::from([("HOME".into(), "/Users/p".into()), ("KUMI_LIVE_EXTENSIONS_DIR".into(), "/x/Extensions".into())]),
            "darwin"
        ),
        Some("/x/Extensions".into())
    );
}
#[test]
fn former_windows_extension_is_removed_preserving_other_extensions() {
    let dir = tempfile::tempdir().unwrap();
    let env = Env::from([("APPDATA".into(), dir.path().join("Roaming").display().to_string())]);
    assert_eq!(former_extensions_dir(&env, "darwin"), None);
    let mut custom = env.clone();
    custom.insert("KUMI_LIVE_EXTENSIONS_DIR".into(), "x".into());
    assert_eq!(former_extensions_dir(&custom, "win32"), None);
    let former = former_extensions_dir(&env, "win32").unwrap();
    assert!(!remove_former_extension(&env, "win32").unwrap());
    fs::create_dir_all(Path::new(&former).join("kumi.kumi/dist")).unwrap();
    let data = extension_data_dir(&former);
    fs::create_dir_all(&data).unwrap();
    assert!(remove_former_extension(&env, "win32").unwrap());
    assert!(!Path::new(&former).exists());
    assert!(!Path::new(&data).parent().unwrap().exists());
    assert!(Path::new(&former).parent().unwrap().exists());
    fs::create_dir_all(Path::new(&former).join("kumi.kumi")).unwrap();
    fs::create_dir_all(Path::new(&former).join("someone.else")).unwrap();
    assert!(remove_former_extension(&env, "win32").unwrap());
    assert!(Path::new(&former).join("someone.else").exists());
}
#[test]
fn extension_install_copy_update_noop_and_remove_keep_source_contents() {
    let dir = tempfile::tempdir().unwrap();
    let source = dir.path().join("bridge").join("live-extension");
    extension(&source, "module.exports = {};\n");
    let source = source.to_str().unwrap();
    let extensions = dir.path().join("Ableton/Extensions");
    let extensions = extensions.to_str().unwrap();
    assert_eq!(extension_source(dir.path().join("bridge").to_str().unwrap()).as_deref(), Some(source));
    assert!(install_extension(source, extensions).unwrap_err().to_string().contains("open Live once"));
    assert!(!Path::new(extensions).exists());
    fs::create_dir_all(Path::new(extensions).parent().unwrap()).unwrap();
    let placed = install_extension(source, extensions).unwrap();
    assert!(placed.changed && !placed.replaced);
    assert_eq!(placed.version, "1.0.0");
    assert_eq!(fs::read_to_string(Path::new(&placed.path).join("dist/extension.js")).unwrap(), "module.exports = {};\n");
    let pkg = fs::read_to_string(Path::new(&placed.path).join("package.json")).unwrap();
    assert_eq!(pkg, "{\n  \"name\": \"kumi\",\n  \"version\": \"1.0.0\",\n  \"private\": true,\n  \"main\": \"dist/extension.js\"\n}\n");
    assert!(!install_extension(source, extensions).unwrap().changed);
    fs::write(Path::new(source).join("dist/extension.js"), "newer();\n").unwrap();
    fs::write(Path::new(source).join("package.json"), "{\"private\":true}\n").unwrap();
    let updated = install_extension(source, extensions).unwrap();
    assert!(updated.changed && updated.replaced);
    assert_eq!(fs::read_to_string(Path::new(&placed.path).join("package.json")).unwrap(), "{\"private\":true}\n");
    assert_eq!(installed_extension(extensions).unwrap().digest, read_extension(source).unwrap().digest);
    fs::create_dir_all(extension_data_dir(extensions)).unwrap();
    assert!(remove_extension(extensions).unwrap());
    assert!(!remove_extension(extensions).unwrap());
    assert!(Path::new(extensions).exists());
}
