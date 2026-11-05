//! Logic's provider uses only disposable files and authenticated stand-in bridges. It never starts a real app.
use daw_bridge::{
    daw::Daw,
    detect::{Arch, Version},
    files::{self, Discovery, Reference},
    paths::BridgePaths,
};
use kumi_daw::{
    io::{DawIo, Kept},
    provider::{BridgeCommand, DawProvider, Installed, RemovedBridge},
};
use kumi_daw_logic::{DAW, PROVIDER};
use kumi_types::{daw::DawId, system::Env};
use std::{
    path::{Path, PathBuf},
    rc::Rc,
};

fn home(dir: &Path) -> Env {
    Env::from([
        ("HOME".into(), dir.to_string_lossy().into_owned()),
        ("KUMI_HOME".into(), dir.join(".kumi").to_string_lossy().into_owned()),
        ("KUMI_LOGIC_APP".into(), dir.join("No Logic.app").to_string_lossy().into_owned()),
    ])
}
fn source(dir: &Path) -> PathBuf {
    let source = dir.join("release/logic-kumi");
    std::fs::create_dir_all(&source).unwrap();
    std::fs::write(source.join("logic-kumi"), b"#!/bin/sh\necho logic-kumi 0.1.0\n").unwrap();
    std::fs::write(source.join("kumi-logic-hands-test"), b"#!/bin/sh\nexit 0\n").unwrap();
    std::fs::write(source.join("version"), b"0.1.0\n").unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        for name in ["logic-kumi", "kumi-logic-hands-test"] {
            std::fs::set_permissions(source.join(name), std::fs::Permissions::from_mode(0o755)).unwrap();
        }
    }
    source
}
fn app(dir: &Path, version: &str) -> PathBuf {
    let app = dir.join("Logic Pro.app");
    std::fs::create_dir_all(app.join("Contents/MacOS")).unwrap();
    std::fs::write(app.join("Contents/Info.plist"), format!("<plist><dict><key>CFBundleIdentifier</key><string>com.apple.logic10</string><key>CFBundleShortVersionString</key><string>{version}</string></dict></plist>")).unwrap();
    std::fs::write(app.join("Contents/MacOS/Logic Pro"), [0xcf, 0xfa, 0xed, 0xfe, 0x0c, 0, 0, 0x01]).unwrap();
    app
}

#[tokio::test(flavor = "current_thread")]
async fn logics_bridge_is_installed_in_kumis_folder_without_restarting_logic() {
    let dir = tempfile::tempdir().unwrap();
    let mut env = home(dir.path());
    let source = source(dir.path());
    env.insert("KUMI_LOGIC_BRIDGE_SOURCE".into(), source.to_string_lossy().into_owned());
    let out = Rc::new(Kept::default());
    let mut io = DawIo::new(env.clone());
    io.out = out.clone();
    assert!(!PROVIDER.bridge_installed(&env));
    assert!(!PROVIDER.reads_only());
    assert_eq!(PROVIDER.id(), DawId::Logic);
    assert_eq!(PROVIDER.setup_bridge(io.clone(), BridgeCommand::default()).await.unwrap(), 1);
    assert!(out.text().contains("Kumi's own folder"), "{}", out.text());
    let paths = BridgePaths::new(&dir.path().join(".kumi"), Daw::Logic);
    let installed = paths.bridge().join("logic-kumi");
    assert!(!installed.exists());
    assert_eq!(PROVIDER.setup_bridge(io.clone(), BridgeCommand { yes: true, ..Default::default() }).await.unwrap(), 0);
    assert!(PROVIDER.bridge_installed(&env));
    for name in ["logic-kumi", "kumi-logic-hands-test", "version", "bridge-reference.json"] {
        assert!(installed.join(name).is_file(), "{name}");
    }
    assert_eq!(std::fs::read(installed.join("logic-kumi")).unwrap(), std::fs::read(source.join("logic-kumi")).unwrap());
    assert_eq!(PROVIDER.bridge_config(&env).as_deref(), Some(installed.join("bridge-reference.json").to_str().unwrap()));
    let reference = Reference::read(&installed.join("bridge-reference.json")).unwrap();
    assert_eq!((reference.daw.as_str(), reference.port), ("logic", Some(9790)));
    assert_eq!(Path::new(&reference.secret_file), paths.secret());
    assert_eq!(Path::new(&reference.discovery_dir), paths.discovery());
    assert!(files::read_secret(&paths.secret()).is_ok());
    let setup = PROVIDER.setup(&env, Rc::default(), false, Rc::default());
    assert!(!setup.needs_restart());
    assert_eq!(setup.installed().await, Installed::Missing, "the bridge needs no installed Logic to go in Kumi's folder");
    assert_eq!(PROVIDER.enable_step().unwrap().label, "Accessibility");
    assert_eq!(PROVIDER.remove_bridge(&io).await.unwrap(), RemovedBridge::Removed);
    assert!(!installed.exists() && !PROVIDER.bridge_installed(&env));
    for word in ["restart Logic", "quit Logic", "Control Surface", "Remote Script"] {
        assert!(!out.text().contains(word), "{}", out.text());
    }
}

#[tokio::test(flavor = "current_thread")]
async fn logic_detection_reads_xml_and_binary_plists_and_setup_names_an_old_app() {
    let dir = tempfile::tempdir().unwrap();
    let mut env = home(dir.path());
    let app = app(dir.path(), "11.2");
    env.insert("KUMI_LOGIC_APP".into(), app.to_string_lossy().into_owned());
    let found = kumi_daw_logic::detect_logic(&env);
    assert_eq!(found.len(), 1);
    assert_eq!(
        (found[0].bundle.as_str(), found[0].version, found[0].archs.as_slice()),
        ("com.apple.logic10", Some(Version(11, 2, 0)), [Arch::Arm64].as_slice())
    );
    let setup = || PROVIDER.setup(&env, Rc::default(), false, Rc::default());
    assert_eq!(setup().installed().await, Installed::Older("11.2.0".into()));
    // Same original M2 binary-plist generator, rather than a subprocess or this machine's app metadata.
    std::fs::write(
        app.join("Contents/Info.plist"),
        bplist(&[("CFBundleIdentifier", "com.apple.mobilelogic"), ("CFBundleShortVersionString", "12.4")]),
    )
    .unwrap();
    let found = kumi_daw_logic::detect_logic(&env);
    assert_eq!((found[0].bundle.as_str(), found[0].version), ("com.apple.mobilelogic", Some(Version(12, 4, 0))));
    assert_eq!(setup().installed().await, Installed::Ready);
    std::fs::write(app.join("Contents/Info.plist"), bplist(&[("CFBundleIdentifier", "com.example.Unrelated")])).unwrap();
    assert!(kumi_daw_logic::detect_logic(&env).is_empty(), "an override cannot identify an unrelated app as Logic");
    let mut malformed = bplist(&[("CFBundleIdentifier", "com.apple.logic10")]);
    let trailer = malformed.len() - 32;
    malformed[trailer + 24..].copy_from_slice(&u64::MAX.to_be_bytes());
    std::fs::write(app.join("Contents/Info.plist"), malformed).unwrap();
    assert!(kumi_daw_logic::detect_logic(&env).is_empty());
}

#[tokio::test(flavor = "current_thread")]
async fn accessibility_is_polled_from_authenticated_status_separately_from_busy() {
    let dir = tempfile::tempdir().unwrap();
    let env = home(dir.path());
    let setup = PROVIDER.setup(&env, Rc::default(), false, Rc::default());
    assert_eq!(setup.enabled().await, Some(false), "not attached yet");
    let paths = BridgePaths::new(&dir.path().join(".kumi"), Daw::Logic);
    let secret = files::ensure_secret(&paths.secret()).unwrap();
    let identity = daw_bridge::wire::Identity {
        preset_files: Vec::new(),
        daw: "logic".into(),
        bridge: "0.1.0".into(),
        daw_version: "12.3".into(),
        pid: std::process::id(),
        resource: "com.apple.logic10".into(),
        project: None,
    };
    for permission in [serde_json::json!("accessibility"), serde_json::Value::Null] {
        let bridge = daw_bridge::probe::testing::serve_with(
            secret.clone(),
            identity.clone(),
            serde_json::json!({"permission":permission,"busy":"modal"}),
        );
        files::write_discovery(
            &paths.discovery(),
            &Discovery {
                schema: files::DISCOVERY_SCHEMA.into(),
                daw: "logic".into(),
                pid: std::process::id(),
                host: "127.0.0.1".into(),
                port: bridge.address.port(),
                protocol: daw_bridge::wire::PROTOCOL.into(),
                version: "0.1.0".into(),
                daw_version: "12.3".into(),
                resource: "com.apple.logic10".into(),
                project: None,
                started_ms: 1,
            },
        )
        .unwrap();
        assert_eq!(setup.enabled().await, Some(permission.is_null()), "a dialog does not revoke Accessibility");
    }
}

#[tokio::test]
async fn an_opener_refuses_a_missing_project_and_an_unrelated_bundle_without_opening_an_app() {
    let dir = tempfile::tempdir().unwrap();
    let opener = DAW.open_project.unwrap();
    let missing = dir.path().join("gone.logicx").to_string_lossy().into_owned();
    assert_eq!(opener(None, missing.clone()).await, Err(format!("That project isn't at {missing} any more.")));
    let file = dir.path().join("A project.logicx");
    std::fs::create_dir(&file).unwrap();
    let refused = opener(Some("com.example.Unrelated".into()), file.to_string_lossy().into_owned()).await.unwrap_err();
    assert!(refused.contains("isn't a Logic Pro bundle") || (!cfg!(target_os = "macos") && refused.contains("can't open")), "{refused}");
    if !cfg!(target_os = "macos") {
        assert!(opener(None, file.to_string_lossy().into_owned()).await.unwrap_err().contains("can't open"));
    }
}

fn bplist(pairs: &[(&str, &str)]) -> Vec<u8> {
    let ascii = |text: &str| -> Vec<u8> {
        let mut object = if text.len() < 15 { vec![0x50 | text.len() as u8] } else { vec![0x5F, 0x10, text.len() as u8] };
        object.extend(text.as_bytes());
        object
    };
    let mut out = b"bplist00".to_vec();
    let mut offsets = vec![out.len()];
    out.push(0xD0 | pairs.len() as u8);
    out.extend((1..=pairs.len()).map(|i| i as u8));
    out.extend((1..=pairs.len()).map(|i| (pairs.len() + i) as u8));
    for text in pairs.iter().map(|(key, _)| *key).chain(pairs.iter().map(|(_, value)| *value)) {
        offsets.push(out.len());
        out.extend(ascii(text));
    }
    let table = out.len();
    out.extend(offsets.iter().map(|offset| *offset as u8));
    out.extend([0, 0, 0, 0, 0, 0, 1, 1]);
    out.extend((1 + 2 * pairs.len() as u64).to_be_bytes());
    out.extend(0u64.to_be_bytes());
    out.extend((table as u64).to_be_bytes());
    out
}
