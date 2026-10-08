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
        (windows::WINDOWS_SOURCE, "f16733836d388b86e5293926c645a7d73b45c59027e1b84b945482d19a97d9b2"),
    ] {
        assert_eq!(hex::encode(Sha256::digest(source)), expected);
    }
    // Windows PowerShell reads a script without a BOM in the console's code page: anything past ASCII is misread.
    assert!(windows::WINDOWS_SOURCE.is_ascii());
}
#[tokio::test(flavor = "current_thread")]
async fn the_windows_script_is_written_whole_and_a_cut_off_one_is_written_again() {
    let folder = tempfile::tempdir().unwrap();
    let hands = folder.path().join("hands");
    let script = windows_script(&hands).await.unwrap();
    assert_eq!(std::fs::read_to_string(&script).unwrap(), windows::WINDOWS_SOURCE);
    // What a write cut short left under its name.
    std::fs::write(&script, &windows::WINDOWS_SOURCE[..100]).unwrap();
    assert_eq!(windows_script(&hands).await.unwrap(), script);
    assert_eq!(std::fs::read_to_string(&script).unwrap(), windows::WINDOWS_SOURCE);
    assert_eq!(std::fs::read_dir(&hands).unwrap().count(), 1, "no temporary file left");
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
 *'"button":"latin1"'*) printf 'Caf\351 menu\n{"id":%s,"ok":true,"after":true}\n' "$id";;
 *'"button":"exit"'*) exit 0;;
 *'"op":"dialog"'*) printf '{"id":%s,"ok":true,"open":true,"title":"Export","words":["Choose a file"],"buttons":["Cancel","Export"],"toggles":[{"name":"Merge Stems","on":false,"enabled":false,"id":"MergeStems.MergeStemsCheckControl"},{"name":"Vocals","on":true,"enabled":true}]}\n' "$id";;
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
            buttons: Some(vec!["Cancel".into(), "Export".into()]),
            file: None,
            // A toggle greyed out says so, with its id; one that can be changed says neither.
            toggles: Some(vec![
                Toggle { name: "Merge Stems".into(), on: false, enabled: false, id: Some("MergeStems.MergeStemsCheckControl".into()) },
                Toggle { name: "Vocals".into(), on: true, enabled: true, id: None }
            ])
        }
    );
    assert!(hands.answer("Cancel", None).await.unwrap().ok);
    let set = [
        ToggleSet { name: "Vocals, Include or exclude the Vocals stem.".into(), id: Some("VocalsCheckControl".into()), on: true },
        ToggleSet { name: "Drums".into(), id: None, on: false },
    ];
    let toggled = hands.toggles(&set, None).await.unwrap();
    assert_eq!(
        toggled.fields["request"]["set"],
        json!([{"name":"Vocals, Include or exclude the Vocals stem.","id":"VocalsCheckControl","on":true},{"name":"Drums","on":false}])
    );
    assert_eq!(hands.windows(None).await.unwrap(), vec![Window { title: "Live".into(), subrole: "AXStandardWindow".into() }]);
    assert_eq!(std::fs::read_to_string(folder.path().join("starts")).unwrap(), "start\n");
    let requests: Vec<Value> =
        std::fs::read_to_string(folder.path().join("requests")).unwrap().lines().map(|l| serde_json::from_str(l).unwrap()).collect();
    assert_eq!(requests.iter().map(|r| r["id"].as_u64().unwrap()).collect::<Vec<_>>(), (1..=9).collect::<Vec<_>>());
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
async fn names_past_ascii_go_to_the_helper_as_escapes() {
    let folder = tempfile::tempdir().unwrap();
    let hands = persistent(helper(folder.path()), vec![], Some(1000));
    // As the helper reads them, whatever its code page: ASCII.
    let tracks = hands.tracks(&[Track { name: "Caf\u{e9} \u{65e5}\u{672c} \u{1f3b9}".into(), nth: None }], None).await.unwrap();
    assert_eq!(tracks.fields["request"]["tracks"][0]["name"], "Caf\u{e9} \u{65e5}\u{672c} \u{1f3b9}");
    let sent = std::fs::read(folder.path().join("requests")).unwrap();
    assert!(sent.is_ascii(), "{}", String::from_utf8_lossy(&sent));
    assert!(String::from_utf8_lossy(&sent).contains(r"Caf\u00e9 \u65e5\u672c \ud83c\udfb9"));
    hands.close();
}
#[cfg(unix)]
#[tokio::test(flavor = "current_thread")]
async fn a_line_that_isnt_utf8_doesnt_stop_the_answers() {
    let folder = tempfile::tempdir().unwrap();
    let hands = persistent(helper(folder.path()), vec![], Some(1000));
    // A line in a console's code page (Latin-1) before the answer: the answer still comes, and so do the next ones.
    assert_eq!(hands.answer("latin1", None).await.unwrap().fields["after"], true);
    assert!(hands.trusted(false).await.unwrap());
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

/// A stand-in for Live on Windows: a window with a Win32 menu bar (which Live's UI Automation tree leaves
/// out, #192), a Yes / No / Cancel prompt, and Windows' own Save and Open dialogs, all owned by it (#187,
/// #189). It says what happened on its stdout.
#[cfg(windows)]
const STAND_IN: &str = r#"
Add-Type -AssemblyName System.Windows.Forms
# Windows' file dialogs stop at a home without its usual folders (a test's own home): they're made there.
foreach ($name in 'Desktop', 'Documents', 'Downloads', 'Music', 'Pictures', 'Videos') { New-Item -ItemType Directory -Force (Join-Path $env:USERPROFILE $name) | Out-Null }
$form = New-Object System.Windows.Forms.Form
$form.Text = 'Kumi hands test'
$form.Width = 420; $form.Height = 200
function Say($text) { [Console]::Out.WriteLine($text); [Console]::Out.Flush() }
$menu = New-Object System.Windows.Forms.MainMenu
$file = $menu.MenuItems.Add('&File')
$new = $file.MenuItems.Add('&New Live Set')
$new.Shortcut = [System.Windows.Forms.Shortcut]::CtrlN
$new.add_Click({ $choice = [System.Windows.Forms.MessageBox]::Show($form, 'Save changes to "Test" before closing?', 'Ableton Live', 'YesNoCancel'); Say "answered $choice" })
$saveAs = $file.MenuItems.Add('Save Live Set &As...')
$saveAs.add_Click({ $dialog = New-Object System.Windows.Forms.SaveFileDialog; $dialog.Title = 'Save Live Set As'; $dialog.InitialDirectory = $PSScriptRoot; if ($dialog.ShowDialog($form) -eq 'OK') { Say "saved $($dialog.FileName)" } else { Say 'save cancelled' } })
$open = $file.MenuItems.Add('&Open Live Set...')
$open.add_Click({ $dialog = New-Object System.Windows.Forms.OpenFileDialog; $dialog.Title = 'Open Live Set'; $dialog.InitialDirectory = $PSScriptRoot; if ($dialog.ShowDialog($form) -eq 'OK') { Say "opened $($dialog.FileName)" } else { Say 'open cancelled' } })
$file.MenuItems.Add('-') | Out-Null
$file.MenuItems.Add('E&xit') | Out-Null
$edit = $menu.MenuItems.Add('&Edit')
$group = $edit.MenuItems.Add('&Group')
$group.Shortcut = [System.Windows.Forms.Shortcut]::CtrlG
$group.add_Click({ Say 'group' })
$freeze = $edit.MenuItems.Add('Freeze Track')
$freeze.Enabled = $false
# Titles past ASCII, made from ASCII: like Kumi's script, this file has no BOM.
$cafe = 'Caf' + [char]0x00E9 + ' ' + [char]0x65E5 + [char]0x672C + ' ' + [char]::ConvertFromUtf32(0x1F3B9)
$edit.MenuItems.Add($cafe) | Out-Null
$create = $menu.MenuItems.Add('&Create')
$create.MenuItems.Add('Insert &MIDI Track') | Out-Null
# Live 12.4's Separate Stems as UI Automation reads it. Live draws its own controls, as WPF does (no child windows):
# a check box for each stem, Merge Stems (greyed out unless two or three stems are on, keeping its state), Quality
# Mode, Separate and Cancel. Live remembers what was on last time: Vocals.
$separate = $create.MenuItems.Add('Separate Stems to New Audio Tracks')
$separate.add_Click({
  $window = New-Object System.Windows.Window
  $window.Title = 'Separate Stems'; $window.Width = 320; $window.Height = 340
  $panel = New-Object System.Windows.Controls.StackPanel
  function Box($name, $id, $on) {
    $box = New-Object System.Windows.Controls.CheckBox
    $box.Content = ($name -split ',')[0]
    [System.Windows.Automation.AutomationProperties]::SetName($box, $name)
    [System.Windows.Automation.AutomationProperties]::SetAutomationId($box, $id)
    $box.IsChecked = $on
    [void]$panel.Children.Add($box)
    return $box
  }
  $stems = @(foreach ($name in 'Vocals', 'Drums', 'Bass', 'Others') { Box "$name, Include or exclude the $name stem." "$($name)CheckControl" ($name -eq 'Vocals') })
  $merge = Box 'Merge Stems' 'MergeStems.MergeStemsCheckControl' $false
  $merge.IsEnabled = $false
  $quality = Box 'Quality Mode, Choose between High Speed or High Quality.' 'HighQualityCheckControl' $true
  $press = New-Object System.Windows.Controls.Button
  $press.Content = 'Separate'
  [System.Windows.Automation.AutomationProperties]::SetAutomationId($press, 'SeparateButton')
  # With no stem on, Live greys out Separate.
  $count = { $n = @($stems | Where-Object { $_.IsChecked }).Count; $merge.IsEnabled = ($n -ge 2 -and $n -le 3); $press.IsEnabled = ($n -gt 0) }.GetNewClosure()
  foreach ($box in $stems) { $box.add_Checked($count); $box.add_Unchecked($count) }
  $press.add_Click({
    $on = @($stems | Where-Object { $_.IsChecked } | ForEach-Object { $_.Content }) -join ','
    [Console]::Out.WriteLine("separated $on merge=$($merge.IsChecked) quality=$($quality.IsChecked)"); [Console]::Out.Flush()
    $window.Close()
  }.GetNewClosure())
  [void]$panel.Children.Add($press)
  $cancel = New-Object System.Windows.Controls.Button
  $cancel.Content = 'Cancel'
  $cancel.add_Click({ [Console]::Out.WriteLine('separate cancelled'); [Console]::Out.Flush(); $window.Close() }.GetNewClosure())
  [void]$panel.Children.Add($cancel)
  $window.Content = $panel
  (New-Object System.Windows.Interop.WindowInteropHelper($window)).Owner = $form.Handle
  [void]$window.ShowDialog()
})
$form.Menu = $menu
# Live's track headers as UI Automation reads them, with two tracks whose names differ only in case. A WPF list, whose
# automation name is its own: a WinForms ListBox is a native list box, and UI Automation names those its own way.
Add-Type -AssemblyName PresentationFramework, PresentationCore, WindowsBase, WindowsFormsIntegration
$headers = New-Object System.Windows.Controls.ListBox
[System.Windows.Automation.AutomationProperties]::SetName($headers, 'Track Headers')
$headers.SelectionMode = [System.Windows.Controls.SelectionMode]::Extended
foreach ($name in @('BASS', 'Bass', $cafe)) { [void]$headers.Items.Add($name) }
$wpf = New-Object System.Windows.Forms.Integration.ElementHost
$wpf.Dock = [System.Windows.Forms.DockStyle]::Fill
$wpf.Child = $headers
$form.Controls.Add($wpf)
$form.add_Shown({ Say "ready $PID" })
[System.Windows.Forms.Application]::Run($form)
"#;

#[cfg(windows)]
#[tokio::test(flavor = "current_thread")]
async fn on_windows_the_helper_uses_the_win32_menu_finds_the_owned_dialog_and_fills_a_save_dialog() {
    use tokio::io::{AsyncBufReadExt, BufReader};
    let folder = tempfile::tempdir().unwrap();
    let stand_in = folder.path().join("stand-in.ps1");
    std::fs::write(&stand_in, STAND_IN).unwrap();
    let mut window = tokio::process::Command::new("powershell.exe")
        .args(["-NoProfile", "-NonInteractive", "-ExecutionPolicy", "Bypass", "-File"])
        .arg(&stand_in)
        .stdout(std::process::Stdio::piped())
        .kill_on_drop(true)
        .spawn()
        .unwrap();
    let mut said = BufReader::new(window.stdout.take().unwrap()).lines();
    async fn next(said: &mut tokio::io::Lines<BufReader<tokio::process::ChildStdout>>) -> Option<String> {
        tokio::time::timeout(Duration::from_secs(15), said.next_line()).await.ok().and_then(|line| line.ok().flatten())
    }
    // No desktop to open a window on (a service session): nothing to test here.
    let Some(ready) = next(&mut said).await else {
        eprintln!("the stand-in window didn't open; skipped");
        return;
    };
    let pid = ready.strip_prefix("ready ").unwrap().trim().to_string();
    let script = folder.path().join("kumi-hands.ps1");
    std::fs::write(&script, windows::WINDOWS_SOURCE).unwrap();
    let command = format!("$env:KUMI_HANDS_PID = '{pid}'; & '{}'", script.display());
    let hands = persistent(
        "powershell.exe".into(),
        ["-NoProfile", "-NonInteractive", "-ExecutionPolicy", "Bypass", "-Command", &command].map(str::to_string).into(),
        Some(15_000),
    );

    // The menu bar, read from Win32: titles without their & marks, shortcuts as Windows writes them.
    let menus = hands.menus(None).await.unwrap();
    let find = |path: &[&str]| menus.iter().find(|item| item.path == path).cloned();
    assert_eq!(find(&["File", "New Live Set"]).unwrap().key.as_deref(), Some("Ctrl+N"), "{menus:?}");
    assert!(find(&["Edit", "Group"]).unwrap().enabled);
    assert!(!find(&["Edit", "Freeze Track"]).unwrap().enabled);
    assert!(find(&["Create", "Insert MIDI Track"]).is_some());
    assert!(!menus.iter().any(|item| item.path.last().is_some_and(|title| title.is_empty() || title == "-")), "no separators");
    // Past ASCII, whole: the script writes it as \u escapes, whatever the console's code page.
    assert!(find(&["Edit", "Caf\u{e9} \u{65e5}\u{672c} \u{1f3b9}"]).is_some(), "{menus:?}");

    // Chosen without bringing the window to the front.
    let group = hands.menu(&["Edit".into(), "Group".into()], MenuOptions::default()).await.unwrap();
    assert!(group.ok, "{group:?}");
    assert_eq!(next(&mut said).await.as_deref(), Some("group"));
    let freeze = hands.menu(&["Edit".into(), "Freeze Track".into()], MenuOptions::default()).await.unwrap();
    assert_eq!(freeze.error.as_deref(), Some("disabled"));

    // A track is picked by its name exactly, as Kumi counts nth: "Bass" isn't "BASS", and "bass" is neither.
    let picked = hands.tracks(&[Track { name: "Bass".into(), nth: Some(0.0) }], None).await.unwrap();
    assert!(picked.ok, "{picked:?}");
    assert_eq!(picked.fields["selected"], json!(["Bass"]));
    let picked = hands.tracks(&[Track { name: "bass".into(), nth: Some(0.0) }], None).await.unwrap();
    assert_eq!((picked.ok, &picked.fields["missing"]), (false, &json!(["bass"])), "{picked:?}");
    // A name past ASCII goes to the script as escapes and comes back whole.
    let picked = hands.tracks(&[Track { name: "Caf\u{e9} \u{65e5}\u{672c} \u{1f3b9}".into(), nth: Some(0.0) }], None).await.unwrap();
    assert!(picked.ok, "{picked:?}");
    assert_eq!(picked.fields["selected"], json!(["Caf\u{e9} \u{65e5}\u{672c} \u{1f3b9}"]));
    assert!(
        hands.dialog(None).await.unwrap() == Dialog { open: false, title: None, words: None, buttons: None, file: None, toggles: None }
    );

    // An item that opens a modal prompt answers at once, and the prompt is the dialog: its own words and
    // buttons, not the main window's. "Don't Save" is Windows' No.
    let started = std::time::Instant::now();
    assert!(hands.menu(&["File".into(), "New Live Set".into()], MenuOptions::default()).await.unwrap().ok);
    assert!(started.elapsed() < Duration::from_secs(5), "the menu didn't wait for the dialog");
    let mut dialog = hands.dialog(None).await.unwrap();
    for _ in 0..20 {
        if dialog.open {
            break;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
        dialog = hands.dialog(None).await.unwrap();
    }
    assert!(dialog.open, "{dialog:?}");
    assert_eq!(dialog.title.as_deref(), Some("Ableton Live"));
    assert!(dialog.words.as_ref().unwrap().iter().any(|w| w.contains("Save changes to")), "{dialog:?}");
    let buttons = dialog.buttons.unwrap();
    assert!(["Yes", "No", "Cancel"].iter().all(|b| buttons.iter().any(|x| x == b)), "{buttons:?}");
    let answered = hands.answer("Don't Save", None).await.unwrap();
    assert!(answered.ok, "{answered:?}");
    assert_eq!(answered.fields["pressed"], "No");
    assert_eq!(next(&mut said).await.as_deref(), Some("answered No"));

    // A Save dialog filled with a path and saved. It says it's a Save dialog.
    async fn opened(hands: &dyn Hands, until: impl Fn(&Dialog) -> bool) -> Dialog {
        let mut dialog = hands.dialog(None).await.unwrap();
        for _ in 0..30 {
            if until(&dialog) {
                break;
            }
            tokio::time::sleep(Duration::from_millis(100)).await;
            dialog = hands.dialog(None).await.unwrap();
        }
        dialog
    }
    // Windows' dialogs answer with a folder's long name, where a temporary folder can come as an 8.3 short
    // name (RUNNER~1 on CI).
    let long = folder.path().canonicalize().unwrap().to_string_lossy().into_owned();
    let place = std::path::PathBuf::from(long.strip_prefix(r"\\?\").unwrap_or(&long));
    assert!(hands.menu(&["File".into(), "Save Live Set As".into()], MenuOptions::default()).await.unwrap().ok);
    let saving = opened(&*hands, |d| d.file.is_some()).await;
    assert_eq!(saving.file.as_deref(), Some("save"), "{saving:?}");
    let path = place.join("CHAOS 02.als");
    let saved = hands.file(&path.to_string_lossy(), "save", None).await.unwrap();
    assert!(saved.ok, "{saved:?}");
    assert_eq!(next(&mut said).await, Some(format!("saved {}", path.display())));

    // An Open dialog, whatever Windows' language: a path meant for a Save dialog doesn't go into it, and the
    // one to open does (#189).
    let song = place.join("Song B.als");
    std::fs::write(&song, b"the producer's song").unwrap();
    assert!(hands.menu(&["File".into(), "Open Live Set".into()], MenuOptions::default()).await.unwrap().ok);
    let opening = opened(&*hands, |d| d.file.is_some()).await;
    assert_eq!(opening.file.as_deref(), Some("open"), "{opening:?}");
    let refused = hands.file(&song.to_string_lossy(), "save", None).await.unwrap();
    assert_eq!((refused.ok, refused.error.as_deref()), (false, Some("other-dialog")), "{refused:?}");
    let taken = hands.file(&song.to_string_lossy(), "open", None).await.unwrap();
    assert!(taken.ok, "{taken:?}");
    assert_eq!(next(&mut said).await, Some(format!("opened {}", song.display())));

    // Saving over that file: Windows asks whether to replace it. Neither Save nor OK is its Yes; No keeps
    // the file, and the Save dialog is still there to cancel.
    assert!(hands.menu(&["File".into(), "Save Live Set As".into()], MenuOptions::default()).await.unwrap().ok);
    assert_eq!(opened(&*hands, |d| d.file.is_some()).await.file.as_deref(), Some("save"));
    assert!(hands.file(&song.to_string_lossy(), "save", None).await.unwrap().ok);
    let replace = opened(&*hands, |d| d.open && d.file.is_none()).await;
    assert!(replace.open && replace.file.is_none(), "{replace:?}");
    for not_yes in ["Save", "OK"] {
        assert_eq!(hands.answer(not_yes, None).await.unwrap().error.as_deref(), Some("no-button"), "{not_yes} on {replace:?}");
    }
    assert!(hands.answer("No", None).await.unwrap().ok);
    assert_eq!(opened(&*hands, |d| d.file.is_some()).await.file.as_deref(), Some("save"));
    assert!(hands.answer("Cancel", None).await.unwrap().ok);
    assert_eq!(next(&mut said).await.as_deref(), Some("save cancelled"));
    assert_eq!(std::fs::read(&song).unwrap(), b"the producer's song");

    // Live's Separate Stems asks which stems with toggles of its own: they're read with their ids and whether
    // they're on, set as asked, and Separate pressed.
    assert!(hands.menu(&["Create".into(), "Separate Stems to New Audio Tracks".into()], MenuOptions::default()).await.unwrap().ok);
    let stems = opened(&*hands, |d| d.toggles.is_some()).await;
    let toggles = stems.toggles.clone().unwrap_or_default();
    let toggle = |name: &str| toggles.iter().find(|t| t.name.starts_with(name)).map(|t| (t.on, t.enabled, t.id.clone()));
    assert_eq!(toggle("Vocals, Include"), Some((true, true, Some("VocalsCheckControl".into()))), "{stems:?}");
    assert_eq!(toggle("Drums, Include"), Some((false, true, Some("DrumsCheckControl".into()))));
    assert_eq!(toggle("Merge Stems"), Some((false, false, Some("MergeStems.MergeStemsCheckControl".into()))));
    assert_eq!(toggle("Quality Mode"), Some((true, true, Some("HighQualityCheckControl".into()))));
    assert_eq!(stems.buttons.as_deref(), Some(&["Separate".to_owned(), "Cancel".to_owned()][..]), "{stems:?}");
    let set = |name: &str, id: Option<&str>, on: bool| ToggleSet { name: name.into(), id: id.map(Into::into), on };
    // Merge Stems first, greyed out until Drums is on: it's set once the rest are. A name it doesn't have is missing.
    let reply = hands
        .toggles(
            &[
                set("Merge Stems", None, true),
                set("Drums, Include or exclude the Drums stem.", Some("DrumsCheckControl"), true),
                set("Nope", None, true),
            ],
            None,
        )
        .await
        .unwrap();
    assert!(reply.ok, "{reply:?}");
    assert_eq!((&reply.fields["missing"], &reply.fields["refused"]), (&json!(["Nope"]), &json!([])), "{reply:?}");
    let after: Vec<Toggle> = serde_json::from_value(reply.fields["toggles"].clone()).unwrap();
    assert!(after.iter().any(|t| t.name == "Merge Stems" && t.on && t.enabled), "{after:?}");
    // Four stems grey Merge Stems out again, still on: it can't be turned off then.
    let reply = hands
        .toggles(
            &[set("x", Some("OthersCheckControl"), true), set("x", Some("BassCheckControl"), true), set("Merge Stems", None, false)],
            None,
        )
        .await
        .unwrap();
    assert_eq!(reply.fields["refused"], json!(["Merge Stems"]), "{reply:?}");
    let reply = hands
        .toggles(
            &[
                set("x", Some("OthersCheckControl"), false),
                set("x", Some("BassCheckControl"), false),
                set("Quality Mode, Choose between High Speed or High Quality.", None, false),
            ],
            None,
        )
        .await
        .unwrap();
    assert_eq!(reply.fields["refused"], json!([]), "{reply:?}");
    // With no stem on, Live greys out Separate: it isn't pressed, and the helper says why.
    let none =
        hands.toggles(&[set("x", Some("VocalsCheckControl"), false), set("x", Some("DrumsCheckControl"), false)], None).await.unwrap();
    assert_eq!(none.fields["refused"], json!([]), "{none:?}");
    let greyed = hands.answer("Separate", None).await.unwrap();
    assert_eq!((greyed.ok, greyed.error.as_deref()), (false, Some("disabled")), "{greyed:?}");
    let back = hands.toggles(&[set("x", Some("VocalsCheckControl"), true), set("x", Some("DrumsCheckControl"), true)], None).await.unwrap();
    assert_eq!(back.fields["refused"], json!([]), "{back:?}");
    assert!(hands.answer("Separate", None).await.unwrap().ok);
    assert_eq!(next(&mut said).await.as_deref(), Some("separated Vocals,Drums merge=True quality=False"));

    // Keys land in the window when it can be brought to the front (a locked or busy desktop can refuse).
    let keys = hands.keys(&["ctrl+g".into()], KeysOptions::default()).await.unwrap();
    if keys.ok {
        assert_eq!(next(&mut said).await.as_deref(), Some("group"));
    } else {
        assert_eq!(keys.error.as_deref(), Some("not-front"), "{keys:?}");
    }
    hands.close();
    let _ = window.kill().await;
}
