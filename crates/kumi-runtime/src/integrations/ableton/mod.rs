pub mod action_execution;
pub mod actions;
pub mod arrange;
pub mod audition;
pub mod bridge_version;
pub mod change_context;
pub mod changes;
pub mod command_tools;
mod concurrent;
pub mod connection;
pub mod context;
pub mod cuts;
pub mod display;
pub mod execution_services;
pub mod fast;
pub mod focus;
pub mod fold;
pub mod history;
mod inference;
pub mod integration;
pub use integration::create_ableton_integration;
pub mod pins;
pub mod references;
pub mod remember;
pub mod views;
pub mod watch;
pub use inference::create_inference_only_integration;
pub mod live_command;
pub mod more_changes;
pub mod mutations;
pub mod notes;
pub mod observation;
pub mod options;
pub mod parameters;
pub use options::AbletonOptions;
pub mod plan_execution;
pub mod plan_stream;
pub mod plugin_tool;
pub mod project;
pub mod samples;
pub mod set_model;
pub mod snapshots;
pub mod track_ids;
pub mod willington;

/// The host-authorized bridge surface, ordered as in the source integration.
pub static BRIDGE_TOOLS: std::sync::LazyLock<Vec<String>> = std::sync::LazyLock::new(|| {
    let data: serde_json::Value = serde_json::from_str(include_str!("assets/changes.json")).unwrap();
    let mut seen = std::collections::HashSet::new();
    crate::mcp::allowed_tools::MODEL_TOOLS
        .iter()
        .copied()
        .chain(data["hostTools"].as_array().unwrap().iter().filter_map(serde_json::Value::as_str))
        .chain([
            "live_project_info",
            "live_project_snapshot_export",
            "live_project_snapshot_diff",
            "live_project_backup_preview",
            "live_project_backup_apply",
            "live_run_python",
            // A saved Set's project id, kept inside the Set (`project::PROJECT_KEY`).
            "live_data_read",
            "live_data_preview",
            "live_data_apply",
        ])
        .filter(|name| seen.insert((*name).to_owned()))
        .map(str::to_owned)
        .collect()
});

pub mod rendering;
