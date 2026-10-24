//! Configuration, with full command, settings and error oracles.
use kumi::config::*;
use kumi_ai::providers::Effort;
use kumi_daw_ableton::app::folders::{find_bridge_config, live_user_library, remote_scripts_dir};
use kumi_types::system::{self, Env};
use serde_json::{json, Value};
fn env() -> Env {
    Env::from([
        ("KUMI_SETTINGS_FILE".into(), "/nonexistent-kumi-test/settings.json".into()),
        ("KUMI_REMOTE_SCRIPTS_DIR".into(), "/nonexistent-kumi-test/Remote Scripts".into()),
    ])
}
fn args(args: &[&str]) -> Vec<String> {
    args.iter().map(|s| s.to_string()).collect()
}
fn value(args_: &[&str], env: &Env) -> Value {
    serde_json::to_value(load_config(&args(args_), env).unwrap()).unwrap()
}
fn fixture() -> Value {
    serde_json::from_str(include_str!("../support/config/reference.json")).unwrap()
}
#[test]
fn models_are_provider_qualified_or_unset_and_need_no_gateway_key() {
    let env = env();
    let expected =
        kumi_library::sources::join(&kumi_library::sources::normalize(&home::home_dir().unwrap().to_string_lossy()), ".kumi/auth.json");
    assert_eq!(load_inference_config(&env).unwrap(), InferenceConfig { model: None, auth_file: expected.clone() });
    for model in ["", " \t\n", "guess", "gateway/model", "openai-codex/", "openai-codex/model\nInjected", "openai-codex/ white space"] {
        let mut env = env.clone();
        env.insert("KUMI_MODEL".into(), model.into());
        assert!(load_inference_config(&env).is_err());
    }
    for model in [
        "openai-codex/gpt-6-astra",
        "openai/gpt-6-luna",
        "anthropic/claude-haiku-4-5-20251001",
        "opencode/kimi-k2.6",
        "opencode-go/gpt-5.5",
        "openai-codex/white space",
        "opencode/deepseek/v4",
    ] {
        let mut env = env.clone();
        env.insert("KUMI_MODEL".into(), model.into());
        assert_eq!(load_inference_config(&env).unwrap(), InferenceConfig { model: Some(model.into()), auth_file: expected.clone() });
        let before = load_inference_config(&env).unwrap();
        env.insert("AI_GATEWAY_API_KEY".into(), "private".into());
        assert_eq!(load_inference_config(&env).unwrap(), before);
    }
}
#[test]
fn file_overrides_must_be_absolute_and_invalid_values_stay_private() {
    let mut env = env();
    for file in ["", "relative.json", "/tmp/unsafe\nfile", "~/auth.json"] {
        env.insert("KUMI_AUTH_FILE".into(), file.into());
        assert_eq!(
            load_inference_config(&env).unwrap_err().to_string(),
            "KUMI_AUTH_FILE must be an absolute file path without control characters."
        );
    }
    env.insert("KUMI_AUTH_FILE".into(), "/private/owner/auth.json".into());
    assert_eq!(load_inference_config(&env).unwrap().auth_file, "/private/owner/auth.json");
    env.insert("KUMI_MODEL".into(), "test-only-secret-do-not-log".into());
    assert!(!load_inference_config(&env).unwrap_err().to_string().contains("test-only-secret-do-not-log"));
}
#[test]
fn sign_in_commands_and_update_bridge_library_flags_match_all_source_results() {
    let fixture = fixture();
    let env: Env = serde_json::from_value(fixture["env"].clone()).unwrap();
    for case in fixture["commands"].as_array().unwrap() {
        let input: Vec<String> = serde_json::from_value(case["args"].clone()).unwrap();
        let result = load_config(&input, &env);
        if let Some(error) = case["error"].as_str() {
            // Source behavior is unchanged; checkout command help names the native Cargo entrypoint.
            let error = error.replace("npm run kumi --", "cargo run -p kumi --").replace("npm run kumi", "cargo run -p kumi --");
            assert_eq!(result.unwrap_err().to_string(), error, "{input:?}");
        } else {
            let mut expected = case["expected"].clone();
            if expected.get("piAuthFile").is_some() {
                expected["piAuthFile"] =
                    json!(kumi_library::sources::join(&home::home_dir().unwrap().to_string_lossy(), ".pi/agent/auth.json"));
            }
            assert_eq!(serde_json::to_value(result.unwrap()).unwrap(), expected, "{input:?}");
        }
    }
    assert_eq!(kumi_dir(&Env::from([("KUMI_HOME".into(), "/kumi-home".into())])), "/kumi-home");
    assert_eq!(
        value(&["auth"], &Env::from([("KUMI_HOME".into(), "/kumi-home".into())]))["authFile"],
        kumi_library::sources::join("/kumi-home", "auth.json")
    );
}
#[test]
fn no_arguments_use_the_installed_scripts_reference_and_explicit_inference_does_not() {
    let folder = tempfile::tempdir().unwrap();
    let mut env = env();
    env.insert("KUMI_MODEL".into(), "openai-codex/gpt-6-astra".into());
    assert_eq!(value(&[], &env)["bridgeMissing"], true);
    let file = folder.path().join("bridge-config.json");
    std::fs::write(&file, "{}").unwrap();
    let scripts = folder.path().join("AbletonMcpBridge");
    std::fs::create_dir(&scripts).unwrap();
    let reference = scripts.join("bridge-reference.json");
    env.insert("KUMI_REMOTE_SCRIPTS_DIR".into(), folder.path().to_string_lossy().into());
    for text in
        ["not json".into(), json!({"config":"relative.json"}).to_string(), json!({"config":folder.path().join("missing.json")}).to_string()]
    {
        std::fs::write(&reference, text).unwrap();
        assert!(find_bridge_config(&env).is_none());
    }
    std::fs::write(&reference, json!({"config":file}).to_string()).unwrap();
    assert_eq!(
        value(&[], &env),
        json!({"mode":"live","bridgeConfig":file,"model":"openai-codex/gpt-6-astra","authFile":load_auth_file(&env).unwrap()})
    );
    assert_eq!(value(&["--inference-only"], &env)["mode"], "inference-only");
    assert!(value(&["--inference-only"], &env).get("bridgeMissing").is_none());
}
#[test]
fn settings_persist_privately_effort_is_validated_and_environment_wins() {
    let folder = tempfile::tempdir().unwrap();
    let file = folder.path().join("nested/settings.json").to_string_lossy().into_owned();
    let mut env = env();
    env.insert("KUMI_SETTINGS_FILE".into(), file.clone());
    write_settings(&file, &json!({"model":"anthropic/claude-sonnet-5","effort":"low"})).unwrap();
    assert_eq!(read_settings(&file).effort, Some(Effort::Low));
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        assert_eq!(std::fs::metadata(&file).unwrap().permissions().mode() & 0o777, 0o600);
        assert_eq!(std::fs::metadata(std::path::Path::new(&file).parent().unwrap()).unwrap().permissions().mode() & 0o777, 0o700);
    }
    std::fs::write(&file, json!({"model":"anthropic/claude-sonnet-5","effort":"ludicrous"}).to_string()).unwrap();
    assert_eq!(read_settings(&file).effort, None);
    assert_eq!(load_inference_config(&env).unwrap().model.as_deref(), Some("anthropic/claude-sonnet-5"));
    env.insert("KUMI_MODEL".into(), "openai/gpt-6-luna".into());
    assert_eq!(load_inference_config(&env).unwrap().model.as_deref(), Some("openai/gpt-6-luna"));
}
#[test]
fn instant_phrases_stay_on_unless_turned_off_and_survive_other_writes() {
    let folder = tempfile::tempdir().unwrap();
    let file = folder.path().join("settings.json").to_string_lossy().into_owned();
    assert_eq!(read_settings(&file).instant_phrases, None, "on by default");
    std::fs::write(&file, json!({"instantPhrases": false, "model": "anthropic/claude-sonnet-5"}).to_string()).unwrap();
    assert_eq!(read_settings(&file).instant_phrases, Some(false));
    // Choosing a model keeps it off.
    write_settings(&file, &json!({"model": "openai/gpt-6-luna"})).unwrap();
    let written: Value = serde_json::from_str(&std::fs::read_to_string(&file).unwrap()).unwrap();
    assert_eq!(written["instantPhrases"], json!(false), "{written}");
    write_settings(&file, &json!({"instantPhrases": true})).unwrap();
    assert_eq!(read_settings(&file).instant_phrases, None);
    std::fs::write(&file, json!({"instantPhrases": "no"}).to_string()).unwrap();
    assert_eq!(read_settings(&file).instant_phrases, None, "anything but false leaves them on");
}
#[test]
fn settings_filter_unknown_values_but_preserve_unmentioned_preferences_and_raw_servers() {
    let folder = tempfile::tempdir().unwrap();
    let file = folder.path().join("settings.json").to_string_lossy().into_owned();
    let fixture = fixture();
    for case in fixture["settings"].as_array().unwrap() {
        std::fs::write(&file, case["raw"].to_string()).unwrap();
        assert_eq!(serde_json::to_value(read_settings(&file)).unwrap(), case["expected"], "{}", case["raw"]);
    }
    for case in fixture["writes"].as_array().unwrap() {
        std::fs::write(&file, case["raw"].to_string()).unwrap();
        write_settings(&file, &case["next"]).unwrap();
        assert_eq!(serde_json::from_str::<Value>(&std::fs::read_to_string(&file).unwrap()).unwrap(), case["expected"], "{case}");
    }
}
#[test]
fn unattended_settings_round_trip_and_survive_other_writes() {
    let folder = tempfile::tempdir().unwrap();
    let file = folder.path().join("settings.json").to_string_lossy().into_owned();
    let saved = || serde_json::from_str::<Value>(&std::fs::read_to_string(&file).unwrap()).unwrap();
    let all =
        json!({"notifications":"off","liveStatusBar":false,"keepAwake":false,"answerAfter":90,"fallbackModel":"anthropic/claude-sonnet-5"});
    write_settings(&file, &all).unwrap();
    assert_eq!(saved(), all);
    let settings = read_settings(&file);
    assert_eq!(settings.notifications.as_deref(), Some("off"));
    assert_eq!((settings.live_status_bar, settings.keep_awake, settings.answer_after), (Some(false), Some(false), Some(90)));
    assert_eq!(settings.fallback_model.as_deref(), Some("anthropic/claude-sonnet-5"));
    // Saving the model, a tab or anything else keeps them.
    write_settings(&file, &json!({"model":"openai-codex/gpt-6-sol","panelTab":"goal"})).unwrap();
    assert_eq!(
        saved(),
        json!({"model":"openai-codex/gpt-6-sol","panelTab":"goal","notifications":"off","liveStatusBar":false,"keepAwake":false,"answerAfter":90,"fallbackModel":"anthropic/claude-sonnet-5"})
    );
    // So does the whole of what was read, written back as /model does.
    write_settings(&file, &serde_json::to_value(read_settings(&file)).unwrap()).unwrap();
    assert_eq!(saved()["fallbackModel"], "anthropic/claude-sonnet-5");
    // The defaults ("auto", true) aren't written; null clears the fallback; 0 never answers for the producer.
    write_settings(&file, &json!({"notifications":"auto","liveStatusBar":true,"keepAwake":true,"answerAfter":0,"fallbackModel":null}))
        .unwrap();
    assert_eq!(saved(), json!({"panelTab":"goal","answerAfter":0}));
    assert_eq!(read_settings(&file).answer_after, Some(0));
    // What isn't valid is dropped as it's read.
    std::fs::write(
        &file,
        json!({"notifications":"loud","liveStatusBar":"no","keepAwake":0,"answerAfter":-5,"fallbackModel":"no-provider"}).to_string(),
    )
    .unwrap();
    let settings = read_settings(&file);
    assert_eq!(settings.notifications, None);
    assert_eq!((settings.live_status_bar, settings.keep_awake, settings.answer_after), (None, None, None));
    assert_eq!(settings.fallback_model, None);
    std::fs::write(&file, json!({"answerAfter":3601}).to_string()).unwrap();
    assert_eq!(read_settings(&file).answer_after, None);
}
#[test]
fn background_job_settings_round_trip_and_survive_a_model_change() {
    let folder = tempfile::tempdir().unwrap();
    let file = folder.path().join("settings.json").to_string_lossy().into_owned();
    std::fs::write(&file, json!({"model":"anthropic/claude-sonnet-5","maxJobs":5,"workerModel":"openai-codex/gpt-6-astra"}).to_string())
        .unwrap();
    let settings = read_settings(&file);
    assert_eq!((settings.max_jobs, settings.worker_model.as_deref()), (Some(5), Some("openai-codex/gpt-6-astra")));
    // Choosing a model writes only the model's own keys: the job settings stay.
    write_settings(&file, &json!({"model":"openai/gpt-6-luna"})).unwrap();
    let written: Value = serde_json::from_str(&std::fs::read_to_string(&file).unwrap()).unwrap();
    assert_eq!(written, json!({"model":"openai/gpt-6-luna","maxJobs":5,"workerModel":"openai-codex/gpt-6-astra"}));
    // Out of range, or not a model: left out.
    std::fs::write(&file, json!({"maxJobs":0,"workerModel":"not a model"}).to_string()).unwrap();
    let settings = read_settings(&file);
    assert_eq!((settings.max_jobs, settings.worker_model), (None, None));
    std::fs::write(&file, json!({"maxJobs":9}).to_string()).unwrap();
    assert_eq!(read_settings(&file).max_jobs, None);
}
#[test]
fn local_models_use_named_servers_and_have_no_sign_in() {
    let folder = tempfile::tempdir().unwrap();
    let file = folder.path().join("settings.json");
    let mut env = env();
    env.insert("KUMI_SETTINGS_FILE".into(), file.to_string_lossy().into());
    for model in ["ollama/qwen3:8b", "ollama/hf.co/unsloth/Qwen3-30B-A3B-GGUF:Q4_K_M", "lmstudio/qwen/qwen3-8b"] {
        assert_eq!(value(&["model", model], &env)["model"], model);
    }
    assert!(load_config(&args(&["model", "llama-cpp/qwen3-8b.gguf"]), &env).is_err());
    std::fs::write(&file,json!({"modelServers":[{"name":"llama.cpp","baseURL":"http://127.0.0.1:8080/v1"},{"name":"Studio PC","baseURL":"http://192.168.1.20:8000"}]}).to_string()).unwrap();
    assert_eq!(value(&["model", "studio-pc/Qwen/Qwen3-32B"], &env)["mode"], "model");
    for cmd in [["login", "ollama"], ["logout", "lmstudio"]] {
        assert!(load_config(&args(&cmd), &env).unwrap_err().to_string().contains("needs no sign-in"));
    }
}
#[test]
fn explicit_bridge_files_are_checked_but_their_secrets_are_not_parsed() {
    let folder = tempfile::tempdir().unwrap();
    let file = folder.path().join("bridge config.json");
    std::fs::write(&file, "not parsed by Kumi; owned by the MCP server").unwrap();
    assert_eq!(value(&["--bridge-config", file.to_str().unwrap()], &env())["bridgeConfig"], file.to_string_lossy().as_ref());
    for file in [folder.path().join("missing.json"), folder.path().into(), "relative.json".into()] {
        assert!(load_config(&args(&["--bridge-config", file.to_str().unwrap()]), &env()).is_err());
    }
}
#[test]
fn unknown_repeated_conflicting_and_incomplete_flags_never_echo_secrets() {
    let fixture = fixture();
    let env: Env = serde_json::from_value(fixture["env"].clone()).unwrap();
    for case in fixture["commands"].as_array().unwrap().iter().filter(|c| c.get("error").is_some()) {
        let args: Vec<String> = serde_json::from_value(case["args"].clone()).unwrap();
        let error = load_config(&args, &env).unwrap_err().to_string();
        assert!(!error.contains("test-only-secret-do-not-log"));
    }
}
#[test]
fn errors_redact_keys_headers_control_codes_and_bound_text() {
    for case in fixture()["safe"].as_array().unwrap() {
        assert_eq!(
            safe_error_message(case["message"].as_str(), &["test-only-secret-do-not-log".into()]),
            case["expected"].as_str().unwrap()
        );
    }
    assert_eq!(safe_error(None, &[]), "Unexpected failure");
}
#[test]
fn live_preferences_find_moved_libraries_and_unescape_xml_once() {
    let folder = tempfile::tempdir().unwrap();
    let preferences = folder.path().join(if system::platform() == "win32" {
        "Ableton/Live 12.4.1/Preferences"
    } else {
        "Library/Preferences/Ableton/Live 12.4.1"
    });
    std::fs::create_dir_all(&preferences).unwrap();
    let key = if cfg!(windows) { "APPDATA" } else { "HOME" };
    let env = Env::from([(key.into(), folder.path().to_string_lossy().into_owned())]);
    for moved in [folder.path().join("Big Drive").join("Music & Samples"), folder.path().join("Takes &quot;live&quot;")] {
        std::fs::write(preferences.join("Library.cfg"),format!(r#"<?xml version="1.0"?><Ableton><ContentLibrary><UserLibrary><LibraryProject Id="0"><ProjectLocation /><ProjectName Value="User Library" /><ProjectPath Value="{}" /></LibraryProject></UserLibrary></ContentLibrary></Ableton>"#,moved.to_string_lossy().replace('&',"&amp;"))).unwrap();
        assert_eq!(live_user_library(&env), Some(moved.join("User Library").to_string_lossy().into()));
        assert_eq!(remote_scripts_dir(&env), moved.join("User Library").join("Remote Scripts").to_string_lossy());
    }
    let mut explicit = env.clone();
    explicit.insert("KUMI_REMOTE_SCRIPTS_DIR".into(), "/chosen/Remote Scripts".into());
    assert_eq!(remote_scripts_dir(&explicit), "/chosen/Remote Scripts");
    assert!(live_user_library(&Env::from([(key.into(), folder.path().join("none").to_string_lossy().into())])).is_none());
}
#[test]
fn over_ssh_chatgpt_signs_in_with_a_code() {
    // The browser on a computer reached over SSH isn't the producer's: a code, entered in a browser anywhere.
    let mut env = env();
    assert_eq!(value(&["login", "openai-codex"], &env)["method"], "browser");
    env.insert("SSH_CONNECTION".into(), "192.0.2.1 52000 192.0.2.2 22".into());
    assert_eq!(value(&["login", "openai-codex"], &env)["method"], "device");
    assert_eq!(value(&["login", "openai-codex", "--from-pi"], &env)["method"], "import-pi");
    // The browser all the same, its link forwarded: --browser.
    assert_eq!(value(&["login", "openai-codex", "--browser"], &env)["method"], "browser");
    assert_eq!(chatgpt_sign_in(&env), LoginMethod::Device);
}
#[test]
fn continue_and_resume_take_up_a_kept_conversation_with_or_without_live() {
    let env = env();
    let resume = |args_: &[&str]| value(args_, &env)["resume"].clone();
    assert_eq!(resume(&["-c"]), json!("last"));
    assert_eq!(resume(&["--continue", "--inference-only"]), json!("last"));
    assert_eq!(resume(&["-r"]), json!("pick"));
    assert_eq!(resume(&["--resume", "--inference-only"]), json!("pick"));
    assert_eq!(resume(&["-r", "mv66me0m3fa91c"]), json!({"named":"mv66me0m3fa91c"}));
    // A name of several words, before or after the other options.
    assert_eq!(resume(&["-r", "night", "drive", "--inference-only"]), json!({"named":"night drive"}));
    assert_eq!(resume(&["--inference-only", "--resume=Night drive"]), json!({"named":"Night drive"}));
    assert_eq!(value(&["--inference-only", "-r", "x"], &env)["mode"], "inference-only");
    assert!(value(&[], &env).get("resume").is_none());
    let both = load_config(&args(&["-c", "-r", "x"]), &env).unwrap_err().to_string();
    assert!(both.contains("-c (the conversation you had last) or") && both.contains("-r <id or name>, not both"), "{both}");
    // Not a command's option.
    assert!(load_config(&args(&["doctor", "-c"]), &env).is_err());
    assert!(load_config(&args(&["-r", "x", "-z"]), &env).unwrap_err().to_string().contains("No option called -z"));
}
#[test]
fn conversations_keep_days_is_read_kept_and_bounded() {
    let folder = tempfile::tempdir().unwrap();
    let file = folder.path().join("settings.json").to_string_lossy().into_owned();
    std::fs::write(&file, json!({"conversations":{"keepDays":30}}).to_string()).unwrap();
    assert_eq!(read_settings(&file).conversations.and_then(|c| c.keep_days), Some(30));
    write_settings(&file, &json!({"model":"openai/gpt-6-luna"})).unwrap();
    assert_eq!(read_settings(&file).conversations.and_then(|c| c.keep_days), Some(30));
    for days in [json!(-1), json!(1e9), json!("90")] {
        std::fs::write(&file, json!({"conversations":{"keepDays":days}}).to_string()).unwrap();
        assert_eq!(read_settings(&file).conversations, None, "{days}");
    }
    std::fs::write(&file, json!({"conversations":{"keepDays":0}}).to_string()).unwrap();
    assert_eq!(read_settings(&file).conversations.and_then(|c| c.keep_days), Some(0));
}
#[test]
fn music_settings_round_trip_and_a_newer_kumis_settings_stay() {
    let folder = tempfile::tempdir().unwrap();
    let file = folder.path().join("settings.json").to_string_lossy().into_owned();
    let saved = || serde_json::from_str::<Value>(&std::fs::read_to_string(&file).unwrap()).unwrap();
    // None until the producer agrees, and nothing offered yet.
    std::fs::write(&file, "{}").unwrap();
    assert_eq!((read_settings(&file).music_folders, read_settings(&file).music_offered), (None, None));
    write_settings(&file, &json!({"musicFolders": ["~/Music", "", 7], "musicOffered": true})).unwrap();
    let settings = read_settings(&file);
    assert_eq!((settings.music_folders, settings.music_offered), (Some(vec!["~/Music".to_string()]), Some(true)));
    // Another setting written keeps them.
    write_settings(&file, &json!({"model": "openai/gpt-6-luna"})).unwrap();
    assert_eq!(saved(), json!({"model": "openai/gpt-6-luna", "musicFolders": ["~/Music"], "musicOffered": true}));
    // Every folder taken away is an answer too: agreed, with none.
    write_settings(&file, &json!({"musicFolders": []})).unwrap();
    assert_eq!(read_settings(&file).music_folders, Some(vec![]));
    write_settings(&file, &json!({"musicFolders": null, "musicOffered": false})).unwrap();
    assert_eq!((read_settings(&file).music_folders, read_settings(&file).music_offered), (None, None));
    // A newer Kumi's settings stay through this one's writes, so an update and a rollback keep them.
    std::fs::write(&file, json!({"model": "openai/gpt-6-luna", "laterSetting": {"on": true}}).to_string()).unwrap();
    write_settings(&file, &json!({"model": "anthropic/claude-sonnet-5"})).unwrap();
    assert_eq!(saved(), json!({"model": "anthropic/claude-sonnet-5", "laterSetting": {"on": true}}));
    write_settings(&file, &serde_json::to_value(read_settings(&file)).unwrap()).unwrap();
    assert_eq!(saved()["laterSetting"], json!({"on": true}));
}
#[test]
fn export_and_import_take_a_conversation_a_format_and_a_file() {
    let env = env();
    assert_eq!(value(&["export"], &env), json!({"mode": "export", "format": "md", "full": false, "raw": false}));
    assert_eq!(
        value(&["export", "kick", "fix", "--html", "--full", "-o", "out"], &env),
        json!({"mode": "export", "conversation": "kick fix", "format": "html", "out": "out", "full": true, "raw": false})
    );
    assert_eq!(
        value(&["export", "--last", "--json", "--json", "--raw"], &env),
        json!({"mode": "export", "format": "json", "full": false, "raw": true})
    );
    for wrong in [
        &["export", "--md", "--html"][..],
        &["export", "-o"],
        &["export", "--last", "k3x9"],
        &["export", "--everything"],
        &["export", "-o", "a\u{7}"],
    ] {
        assert!(load_config(&args(wrong), &env).is_err(), "{wrong:?}");
    }
    assert_eq!(value(&["import", "chat.json"], &env), json!({"mode": "import", "file": "chat.json"}));
    for wrong in [&["import"][..], &["import", "a.json", "b.json"], &["import", "--all"]] {
        assert_eq!(
            load_config(&args(wrong), &env).unwrap_err().to_string(),
            "Use: import <file> (a conversation saved with: export --json).",
            "{wrong:?}"
        );
    }
}
#[test]
fn the_model_catalog_stays_on_unless_turned_off_and_old_settings_still_load() {
    let folder = tempfile::tempdir().unwrap();
    let file = folder.path().join("settings.json").to_string_lossy().into_owned();
    let saved = || serde_json::from_str::<Value>(&std::fs::read_to_string(&file).unwrap()).unwrap();
    // A file written before Kumi knew models.dev's catalog: on, and read as it was.
    std::fs::write(&file, json!({"model":"openai-codex/gpt-6-astra","effort":"high","fast":true}).to_string()).unwrap();
    let settings = read_settings(&file);
    assert_eq!(
        (settings.model_catalog, settings.model.as_deref(), settings.effort),
        (None, Some("openai-codex/gpt-6-astra"), Some(Effort::High))
    );
    // Off: kept through other writes, as other preferences are.
    std::fs::write(&file, json!({"modelCatalog":false}).to_string()).unwrap();
    assert_eq!(read_settings(&file).model_catalog, Some(false));
    write_settings(&file, &json!({"model":"openai/gpt-6-luna","effort":"minimal"})).unwrap();
    assert_eq!(saved(), json!({"model":"openai/gpt-6-luna","effort":"minimal","modelCatalog":false}));
    assert_eq!(read_settings(&file).effort, Some(Effort::Minimal));
    // Anything but false leaves it on, and on isn't written.
    write_settings(&file, &json!({"modelCatalog":true})).unwrap();
    assert_eq!(saved(), json!({}));
    std::fs::write(&file, json!({"modelCatalog":"no"}).to_string()).unwrap();
    assert_eq!(read_settings(&file).model_catalog, None);
}
