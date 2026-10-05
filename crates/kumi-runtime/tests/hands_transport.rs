//! The native helper transport; Live command source cases are in the Ableton integration tests.
use kumi_runtime::hands::*;
use serde_json::{json, Value};
use std::{path::Path, time::Duration};
#[test]
fn embedded_helpers_keep_their_hashes() {
    // The Mac helper installs as kumi-hands-<first 12 hex of its hash>, and macOS ties the producer's
    // Accessibility permission to that file: a changed MAC_SOURCE asks for the permission again.
    use sha2::{Digest, Sha256};
    assert_eq!(HANDS_VERSION, 2);
    for (source, expected) in [
        (mac::MAC_SOURCE, "16045380d38220f9dfd1a6dd318dac9b655fc41758f2531efc46ab97f82a7ba3"),
        (windows::WINDOWS_SOURCE, "7cc8e2446ce4202521b8e9238ecf882f42abfb9bc131c25872c4d424f2d323c8"),
    ] {
        assert_eq!(hex::encode(Sha256::digest(source)), expected);
    }
}
#[cfg(unix)]
fn script(folder: &Path, name: &str, text: &str) -> String {
    use std::os::unix::fs::PermissionsExt;
    let path = folder.join(name);
    std::fs::write(&path, text).unwrap();
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o700)).unwrap();
    path.to_string_lossy().into()
}
#[cfg(unix)]
fn helper(folder: &Path) -> String {
    script(
        folder,
        "helper",
        r#"#!/bin/sh
root="$(dirname "$0")"
printf 'start\n' >> "$root/starts"
while IFS= read -r line; do
 printf '%s\n' "$line" >> "$root/requests"
 id="${line#*\"id\":}"; id="${id%%,*}"
 case "$line" in
 *'"op":"trusted"'*) printf '{"id":%s,"ok":true,"trusted":true}\n' "$id";;
 *'"op":"menus"'*) printf 'not json\n{"id":999999,"ok":true}\n{"id":%s,"ok":true,"items":[{"path":["Edit","Group"],"enabled":true,"key":"G","modifiers":0}]}\n' "$id";;
 *'"button":"untrusted"'*) printf '{"id":%s,"ok":false,"error":"untrusted"}\n' "$id";;
 *'"button":"no-live"'*) printf '{"id":%s,"ok":false,"error":"no-live"}\n' "$id";;
 *'"button":"disabled"'*) printf '{"id":%s,"ok":false,"error":"disabled"}\n' "$id";;
 *'"button":"exit"'*) exit 0;;
 *'"op":"dialog"'*) printf '{"id":%s,"ok":true,"open":true,"title":"Export","words":["Choose a file"],"buttons":["Cancel","Export"]}\n' "$id";;
 *'"op":"windows"'*) printf '{"id":%s,"ok":true,"windows":[{"title":"Live","subrole":"AXStandardWindow"}]}\n' "$id";;
 *'"keys":["hang"]'*) :;;
 *'"path":["defer"]'*) held="$id";;
 *) printf '{"id":%s,"ok":true,"ms":1.5,"request":%s}\n' "$id" "$line";if [ -n "$held" ];then printf '{"id":%s,"ok":true,"deferred":true}\n' "$held";held='';fi;;
 esac
done
"#,
    )
}
#[cfg(unix)]
#[tokio::test(flavor = "current_thread")]
async fn helpers_are_reused_and_each_operation_has_source_fields_and_results() {
    let folder = tempfile::tempdir().unwrap();
    let hands = persistent(helper(folder.path()), vec![], Some(1000));
    assert!(hands.trusted(false).await.unwrap());
    let menus = hands.menus(None).await.unwrap();
    assert_eq!(
        menus,
        vec![MenuItem { path: vec!["Edit".into(), "Group".into()], enabled: true, key: Some("G".into()), modifiers: Some(0.0) }]
    );
    let tracks =
        hands.tracks(&[Track { name: "Bass".into(), nth: Some(0.0) }, Track { name: "Drums".into(), nth: None }], None).await.unwrap();
    assert!(tracks.ok);
    assert_eq!(tracks.ms, Some(1.5));
    assert_eq!(tracks.fields["request"]["tracks"], json!([{"name":"Bass","nth":0},{"name":"Drums"}]));
    let menu = hands
        .menu(&["Edit".into(), "Group".into()], MenuOptions { front: Some(true), titles: vec!["Group Tracks".into()], signal: None })
        .await
        .unwrap();
    assert_eq!(menu.fields["request"]["front"], true);
    assert_eq!(menu.fields["request"]["titles"], json!(["Group Tracks"]));
    let keys = hands.keys(&["cmd+g".into()], KeysOptions { gap_ms: Some(60.0), signal: None }).await.unwrap();
    assert_eq!(keys.fields["request"]["keys"], json!(["cmd+g"]));
    assert_eq!(keys.fields["request"]["gapMs"], 60);
    assert_eq!(
        hands.dialog(None).await.unwrap(),
        Dialog {
            open: true,
            title: Some("Export".into()),
            words: Some(vec!["Choose a file".into()]),
            buttons: Some(vec!["Cancel".into(), "Export".into()])
        }
    );
    assert!(hands.answer("Cancel", None).await.unwrap().ok);
    assert_eq!(hands.windows(None).await.unwrap(), vec![Window { title: "Live".into(), subrole: "AXStandardWindow".into() }]);
    assert_eq!(std::fs::read_to_string(folder.path().join("starts")).unwrap(), "start\n");
    let requests: Vec<Value> =
        std::fs::read_to_string(folder.path().join("requests")).unwrap().lines().map(|l| serde_json::from_str(l).unwrap()).collect();
    assert_eq!(requests.iter().map(|r| r["id"].as_u64().unwrap()).collect::<Vec<_>>(), (1..=8).collect::<Vec<_>>());
    assert_eq!(requests[0], json!({"id":1,"op":"trusted","prompt":false}));
    hands.close();
}
#[cfg(unix)]
#[tokio::test(flavor = "current_thread")]
async fn requests_correlate_out_of_order_and_errors_timeout_and_cancellation_do_not_break_the_helper() {
    let folder = tempfile::tempdir().unwrap();
    let hands = persistent(helper(folder.path()), vec![], Some(1000));
    let path = ["defer".into()];
    let combos = ["cmd+g".into()];
    let (first, second) = tokio::join!(hands.menu(&path, MenuOptions::default()), hands.keys(&combos, KeysOptions::default()));
    assert_eq!(first.unwrap().fields["deferred"], true);
    assert_eq!(second.unwrap().fields["request"]["op"], "keys");
    let error = hands.answer("untrusted", None).await.unwrap_err();
    assert_eq!(error.kind, HandsErrorKind::Untrusted);
    assert!(error.message.contains(if kumi_runtime::system::platform() == "darwin" { "Accessibility" } else { "Windows refused" }));
    let error = hands.answer("no-live", None).await.unwrap_err();
    assert_eq!(error.kind, HandsErrorKind::NoLive);
    assert_eq!(error.message, "Live isn't running.");
    assert_eq!(hands.answer("disabled", None).await.unwrap().error.as_deref(), Some("disabled"));
    let error = hands.keys(&["hang".into()], KeysOptions::default()).await.unwrap_err();
    assert_eq!(error.message, "Live didn't answer in time; is a dialog open in Live?");
    let error =
        hands.keys(&["hang".into()], KeysOptions { signal: Some(kumi_common::abort::timeout(10)), gap_ms: None }).await.unwrap_err();
    assert_eq!(error.message, "Stopped");
    assert_eq!(hands.menus(None).await.unwrap().len(), 1);
    hands.close();
}
#[cfg(unix)]
#[tokio::test(flavor = "current_thread")]
async fn exited_helpers_restart_and_missing_helpers_report_failure() {
    let folder = tempfile::tempdir().unwrap();
    let hands = persistent(helper(folder.path()), vec![], Some(1000));
    assert!(hands.trusted(false).await.unwrap());
    assert_eq!(hands.answer("exit", None).await.unwrap().error.as_deref(), Some("helper-ended"));
    assert!(hands.trusted(true).await.unwrap());
    assert_eq!(std::fs::read_to_string(folder.path().join("starts")).unwrap(), "start\nstart\n");
    hands.close();
    tokio::time::sleep(Duration::from_millis(10)).await;
    assert!(hands.trusted(false).await.unwrap());
    hands.close();
    let absent = persistent(folder.path().join("absent").to_string_lossy().into(), vec![], Some(100));
    assert!(!absent.trusted(false).await.unwrap());
    assert_eq!(absent.answer("x", None).await.unwrap().error.as_deref(), Some("helper-failed"));
    absent.close();
}
#[cfg(target_os = "macos")]
#[tokio::test(flavor = "current_thread")]
async fn native_mac_helper_compiles_and_answers_without_requesting_access() {
    if !can_build_hands() {
        eprintln!("Xcode command line tools are unavailable");
        return;
    }
    let folder = tempfile::tempdir().unwrap();
    let source = folder.path().join("KumiHands.swift");
    let command = folder.path().join("kumi-hands");
    std::fs::write(&source, mac::MAC_SOURCE).unwrap();
    let result = tokio::process::Command::new("xcrun").args(["swiftc", "-O", "-o"]).arg(&command).arg(&source).output().await.unwrap();
    assert!(result.status.success(), "{}", String::from_utf8_lossy(&result.stderr));
    let hands = persistent(command.to_string_lossy().into(), vec![], Some(4000));
    let _allowed = hands.trusted(false).await.unwrap();
    hands.close();
}
