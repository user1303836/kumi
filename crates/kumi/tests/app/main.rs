//! kumi's integration tests for its commands and setup (the CLI, install, update, doctor, config, sign-in, models,
//! sessions, serve's setup and the first request), a module per file: one of kumi's three test binaries, with
//! tests/it and tests/tui (AGENTS.md, Conventions).
mod bridge_setup;
mod cli;
mod cli_binary;
mod config;
mod daw_choice;
mod daw_neutral_guard;
mod daws_doc;
mod doctor;
mod drum_kits;
mod export;
mod install;
mod jobs;
mod live_extension;
mod login;
mod models;
mod native_editing_live;
mod observed_names;
mod pack_tests;
mod pin_first_request;
mod report;
mod serve_setup;
mod session;
mod update;
mod willington;
