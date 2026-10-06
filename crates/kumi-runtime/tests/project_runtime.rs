//! Pure catch-up/storage cases; real integration cases remain in the integration parity work.
use kumi_common::js::json::stringify;
use kumi_runtime::{
    core::contracts::{ConversationStore, JsonObject, SavedConversation},
    integrations::ableton::project::*,
};
use serde_json::{json, Value};
use std::sync::LazyLock;
static ORACLE: LazyLock<Value> = LazyLock::new(|| serde_json::from_str(include_str!("support/project-oracle.json")).unwrap());
fn pages(value: &Value) -> Vec<JsonObject> {
    serde_json::from_value(value.clone()).unwrap()
}
fn said(text: &str, saved_at: i64) -> SavedConversation {
    serde_json::from_value(json!({"savedAt":saved_at,"checkpoint":{"version":1,"messages":[{"role":"user","content":[{"type":"text","text":format!("{text}\n\n<current_observation_untrusted>{{}}")}]},{"role":"assistant","content":"ok"}]}})).unwrap()
}

#[test]
fn semantic_diff_and_watch_descriptions_match_the_typescript_reference() {
    for (i, case) in ORACLE["cases"].as_array().unwrap().iter().enumerate() {
        let before = pages(&case["before"]);
        let after = pages(&case["after"]);
        let limit = Some(case["limit"].as_u64().unwrap() as usize);
        let diff = case["diff"].as_object().unwrap();
        assert_eq!(
            stringify(&serde_json::to_value(describe_diff(diff, &before, &after, limit)).unwrap()),
            stringify(&case["described"]),
            "diff case {i}"
        );
        assert_eq!(
            stringify(&serde_json::to_value(describe_watch(diff, &before, &after, limit)).unwrap()),
            stringify(&case["watched"]),
            "watch case {i}"
        );
    }
    let fixture = &ORACLE["fixture"];
    let described = describe_diff(fixture["diff"].as_object().unwrap(), &pages(&fixture["before"]), &pages(&fixture["after"]), None);
    assert_eq!(
        described.lines,
        [
            "Tempo 120 → 124 BPM",
            "“Drums” → “Beats” (renamed and changed)",
            "Added track “Pad”",
            "Changed clip “Bassline” (notes)",
            "Removed device “Utility”"
        ]
    );
    assert_eq!(described.more, 0);
}
#[test]
fn time_since_reads_in_plain_words() {
    for case in ORACLE["times"].as_array().unwrap() {
        assert_eq!(since(0.0, case["delta"].as_f64().unwrap()), case["text"]);
    }
}

#[tokio::test(flavor = "current_thread")]
async fn conversations_are_private_and_long_histories_drop_whole_early_exchanges() {
    let dir = tempfile::tempdir().unwrap();
    let store = create_conversation_store(dir.path());
    let place = project_id_of("/Music/Night Drive.als");
    assert!(store.current(&place).await.unwrap().is_none());
    let hi: SavedConversation = serde_json::from_value(
        json!({"savedAt":5,"checkpoint":{"version":1,"messages":[{"role":"user","content":"hi"}],"origin":"openai-codex"}}),
    )
    .unwrap();
    store.save(&place, "abc123", &hi).await.unwrap();
    let current = store.current(&place).await.unwrap().unwrap();
    assert_eq!(current.id, "abc123");
    assert_eq!(current.conversation.first.as_deref(), Some("hi"));
    assert_eq!(current.conversation.turns, Some(1));
    assert_eq!(current.conversation.checkpoint, hi.checkpoint);
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        assert_eq!(
            std::fs::metadata(dir.path().join(&place).join("conversations/abc123.json")).unwrap().permissions().mode() & 0o777,
            0o600
        );
    }
    for shaped in [false, true] {
        let messages:Vec<_>=(0..40).map(|i|{let text=format!("{i}:{}","x".repeat(10000));json!({"role":if i%2==0{"user"}else{"assistant"},"content":if shaped{json!([{"type":"text","text":text}])}else{json!(text)}})}).collect();
        let mut long = hi.clone();
        long.checkpoint.messages = messages.clone();
        store.save(&place, "long01", &long).await.unwrap();
        let kept = store.load(&place, "long01").await.unwrap().unwrap();
        assert!(kept.checkpoint.messages.len() < messages.len());
        assert!(stringify(&json!(kept.checkpoint.messages)).len() <= 256 * 1024);
        assert_eq!(kept.checkpoint.messages[0]["role"], "user");
        assert_eq!(kept.checkpoint.messages.last().unwrap(), messages.last().unwrap());
        assert_eq!(kept.turns, Some(20));
        if shaped {
            assert!(kept.checkpoint.messages[0]["content"][0]["text"]
                .as_str()
                .unwrap()
                .starts_with("[Kumi removed the earlier part of this conversation to save room.]\n\n"));
        }
    }
    assert!(store.load("../escape", "abc123").await.unwrap().is_none());
    assert!(store.save(&place, "../x", &hi).await.unwrap_err().to_string().contains("invalid"));
}

#[tokio::test(flavor = "current_thread")]
async fn fresh_starts_preserve_history_latest_twenty_remain_unsaved_moves_and_legacy_migrates() {
    let dir = tempfile::tempdir().unwrap();
    let store = create_conversation_store(dir.path());
    let place = project_id_of("/Music/Night Drive.als");
    store.save(&place, "first1", &said("make the bass wider", 1)).await.unwrap();
    store.fresh(&place).await.unwrap();
    assert!(store.current(&place).await.unwrap().is_none());
    store.save(&place, "second", &said("tighten the drums", 2)).await.unwrap();
    let list = store.list(&place).await.unwrap();
    assert_eq!(
        list.iter().map(|r| (r.id.as_str(), r.first.as_str(), r.turns, r.current)).collect::<Vec<_>>(),
        [("second", "tighten the drums", 1, true), ("first1", "make the bass wider", 1, false)]
    );
    for index in 0..22 {
        store.save(&place, &format!("bulk{index:03}"), &said(&format!("request {index}"), 10 + index)).await.unwrap();
    }
    let all = store.list(&place).await.unwrap();
    assert_eq!(all.len(), 20);
    assert_eq!(all.iter().find(|r| r.current).unwrap().id, "bulk021");
    let saved = project_id_of("/Music/New Idea.als");
    store.save("unsaved", "draft1", &said("sketch a chord progression", 3)).await.unwrap();
    store.move_conversation("draft1", "unsaved", &saved).await.unwrap();
    assert_eq!(store.current(&saved).await.unwrap().unwrap().id, "draft1");
    assert!(store.load("unsaved", "draft1").await.unwrap().is_none());
    assert!(store.list("unsaved").await.unwrap().is_empty());
    let legacy = project_id_of("/Music/Old.als");
    let folder = dir.path().join(&legacy);
    std::fs::create_dir_all(&folder).unwrap();
    std::fs::write(folder.join("conversation.json"), stringify(&serde_json::to_value(said("from before", 4)).unwrap())).unwrap();
    assert_eq!(store.current(&legacy).await.unwrap().unwrap().conversation.saved_at, 4);
    assert!(!folder.join("conversation.json").exists());
    assert_eq!(store.list(&legacy).await.unwrap()[0].first, "from before");
}

#[tokio::test(flavor = "current_thread")]
async fn saved_set_baselines_are_private_kept_by_project_and_bad_or_oversized_files_are_ignored() {
    let dir = tempfile::tempdir().unwrap();
    let store = create_project_store(dir.path());
    let baseline = Baseline {
        version: 1,
        path: "/Music/Night Drive.als".into(),
        name: "Night Drive".into(),
        saved_at: 1,
        artifact_id: ORACLE["fixture"]["before"][0]["artifact"]["id"].as_str().unwrap().into(),
        pages: pages(&ORACLE["fixture"]["before"]),
    };
    let project = project_id_of(&baseline.path);
    store.save(&project, &baseline).await.unwrap();
    assert_eq!(store.load(&project).await.unwrap(), Some(baseline.clone()));
    assert!(store.load(&project_id_of("/Music/Other.als")).await.unwrap().is_none());
    assert!(
        store.load("../elsewhere").await.unwrap().is_none() && store.save("../elsewhere", &baseline).await.is_err(),
        "a project id only"
    );
    // Moved, the Set still finds it: it's kept by project, not by path.
    let moved = Baseline { path: "/Music/Moved/Night Drive.als".into(), ..baseline.clone() };
    store.save(&project, &moved).await.unwrap();
    assert_eq!(store.load(&project).await.unwrap().map(|kept| kept.path), Some(moved.path.clone()));
    store.save(&project, &baseline).await.unwrap();
    let folder = dir.path().join(&project);
    let file = folder.join("last-seen.json");
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        assert_eq!(std::fs::metadata(&folder).unwrap().permissions().mode() & 0o777, 0o700);
        assert_eq!(std::fs::metadata(&file).unwrap().permissions().mode() & 0o777, 0o600);
    }
    std::fs::write(&file, "{broken").unwrap();
    assert!(store.load(&project).await.unwrap().is_none());
    let mut big = baseline.clone();
    big.pages = vec![json!({"text":"x".repeat(8*1024*1024)}).as_object().unwrap().clone()];
    store.save(&project, &big).await.unwrap();
    assert_eq!(std::fs::read_to_string(file).unwrap(), "{broken");
}

#[tokio::test(flavor = "current_thread")]
async fn a_picture_the_producer_added_is_named_not_kept_in_a_saved_conversation() {
    let dir = tempfile::tempdir().unwrap();
    let store = create_conversation_store(dir.path());
    let conversation: SavedConversation = serde_json::from_value(json!({"savedAt":1,"checkpoint":{"version":1,"messages":[
        {"role":"user","content":[{"type":"text","text":"make this"},{"type":"file","filename":"synth.png","data":"iVBORw0KGgo=","mediaType":"image/png"}]},
        {"role":"assistant","content":"ok"}
    ]}}))
    .unwrap();
    store.save("unsaved", "pics01", &conversation).await.unwrap();
    let kept = store.load("unsaved", "pics01").await.unwrap().unwrap();
    assert_eq!(kept.checkpoint.messages[0]["content"][0], json!({"type":"text","text":"make this"}));
    assert_eq!(
        kept.checkpoint.messages[0]["content"][1],
        json!({"type":"text","text":"[The producer showed synth.png here; pictures aren't kept with saved conversations.]"})
    );
}

#[test]
fn a_sets_project_is_kept_inside_it_through_versions_and_moves_and_never_in_a_template() {
    let dir = tempfile::tempdir().unwrap();
    let path = |p: &std::path::Path| p.to_string_lossy().into_owned();
    let song = dir.path().join("Night Drive Project");
    std::fs::create_dir_all(song.join("Ableton Project Info")).unwrap();
    let first = song.join("Night Drive.als");
    std::fs::write(&first, "").unwrap();
    let version = song.join("Night Drive v2.als");
    let kept = "0123456789abcdef0123456789abcdef";
    // None kept: the id the path gave before, now kept in the Set.
    assert_eq!(decide_project(None, &path(&first), None), (project_id_of(&path(&first)), true));
    assert_eq!(decide_project(Some("not an id"), &path(&first), None), (project_id_of(&path(&first)), true));
    // Kept: at the same path, or first seen here.
    assert_eq!(decide_project(Some(kept), &path(&first), Some(&path(&first))), (kept.into(), false));
    assert_eq!(decide_project(Some(kept), &path(&first), None), (kept.into(), false));
    // Saved as a version in the same Project folder: the same song. Moved: its last place is gone.
    assert_eq!(decide_project(Some(kept), &path(&version), Some(&path(&first))), (kept.into(), false));
    assert_eq!(decide_project(Some(kept), &path(&version), Some("/gone/Night Drive.als")), (kept.into(), false));
    // Copied into another Project folder while the first is still there: a new song.
    let other = dir.path().join("Other Project");
    std::fs::create_dir_all(other.join("Ableton Project Info")).unwrap();
    let copy = other.join("Night Drive.als");
    assert_eq!(decide_project(Some(kept), &path(&copy), Some(&path(&first))), (project_id_of(&path(&copy)), true));
    // A template never gets an id; a song started from one that had one is a song of its own.
    let templates = dir.path().join("User Library").join("Templates");
    std::fs::create_dir_all(&templates).unwrap();
    let template = templates.join("Footwork.als");
    std::fs::write(&template, "").unwrap();
    assert_eq!(decide_project(None, &path(&template), None), (project_id_of(&path(&template)), false));
    assert_eq!(decide_project(Some(kept), &path(&first), Some(&path(&template))), (project_id_of(&path(&first)), true));
    assert!(template_location("/Users/p/Library/Preferences/Ableton/Live 12.4.15b5/BaseFiles/DefaultLiveSet.als"));
    assert!(template_location("/Applications/Ableton Live 12 Beta.app/Contents/App-Resources/Templates/Basic.als"));
    assert!(template_location(r"C:\Users\p\Documents\Ableton\User Library\Templates\Footwork.als") || !cfg!(windows));
    assert!(!template_location(&path(&first)));
}
