//! ableton-mcp-server's integration tests, one module per file: one test binary for the crate, built and linked once.
mod als;
mod analysis;
mod analysis_runner;
mod audio_diagnosis;
mod audio_standards;
mod batch;
mod benchmark;
mod cli;
mod delivery;
mod delivery_cli;
mod device_sample_lifecycle;
mod device_state;
mod drum_sampler_preset;
mod extension_channel;
mod extension_launcher;
mod fixture_paths;
mod follow_actions;
mod framing;
mod host_advanced_devices;
mod host_arrangement;
mod host_arrangement_clip;
mod host_arrangement_midi;
mod host_audio;
mod host_audio_clip;
mod host_audio_import;
mod host_automation;
mod host_browser_load;
mod host_browser_render;
mod host_clip_duplicate;
mod host_clip_move;
mod host_clip_properties;
mod host_data;
mod host_deletion;
mod host_device_state;
mod host_drum_pad;
mod host_extended_mixer;
mod host_groove;
mod host_helpers;
mod host_midi_transform;
mod host_mutations;
mod host_native_editing;
mod host_note_edit;
mod host_note_target;
mod host_probes;
mod host_project;
mod host_protocol;
mod host_racks;
mod host_routing;
mod host_selection;
mod host_session_capture;
mod host_simpler;
mod host_structure;
mod host_track_structure;
mod host_transport;
mod host_tuning;
mod host_warp;
mod journeys;
mod json_diagnostics;
mod key_estimation;
mod library_search;
mod lifecycle;
mod lifecycle_binary;
mod lifecycle_migration;
mod lifecycle_release;
mod live;
mod loopback;
mod mcp_protocol;
mod midi_transforms;
mod native_connected;
mod observed_evidence;
mod observed_notes;
/// The tool descriptors from before where play starts and a load's undo keeping the selection, put back for the oracles' replays.
#[path = "../support/play_start_and_selection.rs"]
mod play_start_and_selection;
mod project;
mod project_semantic;
mod project_semantic_diff;
mod real_live_snapshots;
mod reference_analysis;
mod registry;
mod remote_adapter;
mod session_midi;
mod simulator;
mod sqlite_reader;
mod stdio;
mod tool_catalog;

use std::path::Path;

/// The analysis worker Cargo built, beside this test binary, before any test runs (test-only; nothing shipped changes).
/// An installed bridge keeps the worker beside itself, and the runner looks there and one folder up (Cargo's
/// `<profile>/deps/` before 1.100). Cargo 1.100 builds test executables away from the programs it builds, in
/// `<profile>/build/<package>/<hash>/out/`, so the runner would find no worker. Once per process (nextest runs each test
/// in one of its own, at once): a link made under a name of this process's and renamed into place, kept while it's as
/// new as the worker; a hard link, or a copy where the folders can't share one.
fn analysis_worker_beside_this_binary() {
    let worker = Path::new(env!("CARGO_BIN_EXE_ableton-mcp-analysis-worker"));
    let (Some(name), Some(folder)) = (worker.file_name(), std::env::current_exe().ok().and_then(|exe| exe.parent().map(Path::to_path_buf)))
    else {
        return;
    };
    let beside = folder.join(name);
    let modified = |path: &Path| std::fs::metadata(path).and_then(|metadata| metadata.modified()).ok();
    let current = |path: &Path| modified(path).is_some_and(|at| modified(worker).is_some_and(|built| at >= built));
    if current(&beside) || folder.parent().is_some_and(|up| current(&up.join(name))) {
        return;
    }
    let temporary = folder.join(format!(".{}.{}", name.to_string_lossy(), std::process::id()));
    let _ = std::fs::remove_file(&temporary);
    if std::fs::hard_link(worker, &temporary).is_err() && std::fs::copy(worker, &temporary).is_err() {
        return;
    }
    if std::fs::rename(&temporary, &beside).is_err() {
        let _ = std::fs::remove_file(&temporary); // (Windows: another test's process put it in place first)
    }
}

// Run as the process starts, before the test harness: no test has to remember to ask for the worker.
#[used]
#[cfg_attr(any(target_os = "linux", target_os = "android", target_os = "freebsd"), link_section = ".init_array")]
#[cfg_attr(target_vendor = "apple", link_section = "__DATA,__mod_init_func")]
#[cfg_attr(windows, link_section = ".CRT$XCU")]
static ANALYSIS_WORKER_BESIDE_THIS_BINARY: extern "C" fn() = {
    extern "C" fn place() {
        analysis_worker_beside_this_binary();
    }
    place
};
