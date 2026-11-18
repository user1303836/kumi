//! kumi's integration tests for the DAWs, the library, serve and the sessions over them, a module per file: one of
//! kumi's three test binaries, with tests/tui and tests/app. kumi's tests are the last link of the build's chain, so
//! they're three binaries that compile at once (AGENTS.md, Conventions).
mod app_sessions;
mod daw_words;
mod fake_daw_session;
mod library;
mod library_daws;
mod library_features;
mod library_handle;
mod library_learn;
mod library_learner;
#[cfg(unix)]
mod library_room;
mod library_search;
mod library_state_plan;
#[cfg(unix)]
mod library_stop;
mod music_learner;
mod own_bridge;
mod own_bridge_e2e;
mod own_catchup;
mod own_catchup_e2e;
mod own_changes;
mod own_daw_places;
mod own_ears;
mod own_field_reads;
mod own_listening_e2e;
mod own_providers;
mod own_session_catchup;
mod serve;
mod serve_daw;
mod take_back_daws;
