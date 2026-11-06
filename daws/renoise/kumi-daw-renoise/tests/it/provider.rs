//! Renoise's provider finds the tool a release carries beside Kumi's executable, though a release has no checkout.
use kumi_daw_own::provider::Own;

/// A release isn't a checkout: the tool it brings is the copy in `daws/renoise/` beside Kumi's executable. Before, a Kumi
/// that found no checkout to build from didn't look for that copy, and said it carried no tool.
#[test]
fn a_release_without_a_checkout_brings_the_tool_it_carries() {
    let release = Own { built: |_| None, ..kumi_daw_renoise::PROVIDER };
    let app = tempfile::tempdir().unwrap();
    assert_eq!(release.bundled_beside(app.path()), None, "nothing carried");
    let carried = app.path().join("daws").join("renoise").join((release.carried)());
    std::fs::create_dir_all(&carried).unwrap();
    assert_eq!(release.bundled_beside(app.path()), Some(carried));
}

/// Setup reads CC-8's mark in the tool's `payload/status.lua` ([`daw_bridge::contract::KEEPS_IDENTITY_ONLY_WHEN_ASKED`])
/// to say the tool keeps a song's id only while a connection asks: the tool Kumi ships carries it word for word.
#[test]
fn the_tool_kumi_ships_carries_the_mark_setup_reads_word_for_word() {
    let status = kumi_testkit::workspace_path("daws/renoise/com.kumi.Bridge.xrnx/payload/status.lua");
    let mark = daw_bridge::contract::KEEPS_IDENTITY_ONLY_WHEN_ASKED;
    assert!(std::fs::read_to_string(&status).unwrap().contains(&format!("\"{mark}\"")), "{} doesn't carry {mark:?}", status.display());
}
