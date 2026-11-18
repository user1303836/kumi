//! kumi's integration tests for the terminal app (its screens, keys, panels, pictures and what it prints), a module
//! per file: one of kumi's three test binaries, with tests/it and tests/app (AGENTS.md, Conventions).
mod activity;
mod app_text;
mod app_voice;
mod focus_fill;
mod history;
mod icons;
mod map_server;
mod markdown;
mod pin_model_ids;
mod pin_plain;
mod pin_screens;
mod pin_startup;
mod plain_farewell;
mod print;
mod session_thread;
mod spinner;
mod stdin_handoff;
mod terminal;
mod transcript;
mod tree;
mod tui;
mod tui_app;
mod tui_app_helpers;
mod tui_app_panels;
mod tui_app_voice;
mod tui_away;
mod tui_composer;
mod tui_details;
mod tui_drawer;
mod tui_jobs;
mod tui_keys;
mod tui_map;
mod tui_menu;
mod tui_music;
mod tui_own_bodies;
mod tui_palette;
mod tui_pictures;
mod tui_replay;
mod tui_setup;
mod tui_slots;
mod tui_take_back;
mod tui_theme;
mod tui_threaded;
mod tui_usage;
mod tui_view;
mod voice_input;
#[cfg(windows)]
mod windows_console_control;
#[cfg(windows)]
mod windows_stdin;
#[cfg(windows)]
mod windows_tty;
