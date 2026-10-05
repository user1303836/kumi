//! Configuration, with full command, settings and error oracles.
use kumi::config::*;
use kumi_runtime::{
    providers::Effort,
    system::{self, Env},
};
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
    serde_json::from_str(include_str!("support/config/reference.json")).unwrap()
}
#[test]
fn models_are_provider_qualified_or_unset_and_need_no_gateway_key() {
    let env = env();
    let expected = kumi_runtime::library::sources::join(
        &kumi_runtime::library::sources::normalize(&home::home_dir().unwrap().to_string_lossy()),
        ".kumi/auth.json",
    );
    assert_eq!(load_inference_config(&env).unwrap(), InferenceConfig { model: None, auth_file: expected.clone() });
    for model in ["", " \t\n", "guess", "gateway/model", "openai-codex/", "openai-codex/model\nInjected", "openai-codex/white space"] {
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
                    json!(kumi_runtime::library::sources::join(&home::home_dir().unwrap().to_string_lossy(), ".pi/agent/auth.json"));
            }
            assert_eq!(serde_json::to_value(result.unwrap()).unwrap(), expected, "{input:?}");
        }
    }
    assert_eq!(kumi_dir(&Env::from([("KUMI_HOME".into(), "/kumi-home".into())])), "/kumi-home");
    assert_eq!(
        value(&["auth"], &Env::from([("KUMI_HOME".into(), "/kumi-home".into())]))["authFile"],
        kumi_runtime::library::sources::join("/kumi-home", "auth.json")
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
