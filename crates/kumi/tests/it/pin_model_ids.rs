//! Pins which model IDs Kumi takes: what `parse_model_id`, `parse_local_model_id` (with Ollama, LM Studio and four
//! custom servers) and the settings' `valid_model` make of each, the IDs those servers get from their names, and the
//! errors that name the providers. The golden is tests/goldens/models/ids.json; `KUMI_BLESS=1` writes it afresh.
use kumi::config::{load_config, load_inference_config, valid_model};
use kumi_ai::providers::{
    local::{local_servers, parse_local_model_id, ServerSetting},
    parse_model_id,
};
use kumi_types::system::Env;
use serde_json::{json, Map, Value};

fn golden(name: &str, value: &Value) {
    let file = kumi_testkit::workspace_path("crates/kumi/tests/goldens/models").join(name);
    let mut actual = serde_json::to_string_pretty(value).unwrap();
    actual.push('\n');
    if std::env::var_os("KUMI_BLESS").is_some() {
        std::fs::create_dir_all(file.parent().unwrap()).unwrap();
        std::fs::write(&file, actual).unwrap();
        return;
    }
    let expected =
        std::fs::read_to_string(&file).unwrap_or_else(|_| panic!("{} is missing: run this test with KUMI_BLESS=1", file.display()));
    assert!(
        actual == expected,
        "{} changed: check the difference, then run this test with KUMI_BLESS=1\n{}",
        file.display(),
        difference(&expected, &actual)
    );
}
fn difference(expected: &str, actual: &str) -> String {
    let (expected, actual): (Vec<_>, Vec<_>) = (expected.lines().collect(), actual.lines().collect());
    let line = expected.iter().zip(&actual).position(|(a, b)| a != b).unwrap_or(expected.len().min(actual.len()));
    let mut text = format!("first difference at line {} (- expected, + actual):\n", line + 1);
    for (mark, lines) in [('-', &expected), ('+', &actual)] {
        for (i, differs) in lines.iter().enumerate().skip(line).take(3) {
            text += &format!("  {mark} {:>5} {differs}\n", i + 1);
        }
    }
    text
}
/// Custom servers named as a producer might: one after a provider, two alike, one in two words.
fn settings() -> Vec<ServerSetting> {
    [
        ("OpenAI", "http://localhost:8000"),
        ("Venice", "https://venice.example.test/v1"),
        ("Venice", "http://10.0.0.2:9000"),
        ("My Server", "http://192.168.1.20:8080"),
    ]
    .map(|(name, url)| ServerSetting { name: name.into(), base_url: url.into(), api_key: None })
    .into()
}
/// A long ID as its start and its length, so the golden stays readable.
fn shown(text: &str) -> String {
    let units = text.encode_utf16().count();
    if text.chars().count() <= 48 {
        return text.into();
    }
    format!("{}… ({units} UTF-16 units)", text.chars().take(16).collect::<String>())
}
fn env() -> Env {
    Env::from([("KUMI_SETTINGS_FILE".into(), "/nonexistent-kumi-test/settings.json".into())])
}
fn refused(args: &[&str]) -> String {
    let args: Vec<String> = args.iter().map(|arg| arg.to_string()).collect();
    load_config(&args, &env()).err().map(|error| error.to_string()).unwrap_or_else(|| "accepted".into())
}

#[test]
fn model_ids_are_taken_or_refused_and_servers_named_as_they_were() {
    let a = |n: usize| "a".repeat(n);
    let ids: Vec<String> = [
        "openai-codex/gpt-5.5",
        "openai/gpt-6-luna",
        "anthropic/claude-sonnet-5-5",
        "opencode/kimi-k2.6",
        "opencode-go/glm-5",
        "openai/o5-mini",
        "openai/ft:gpt-4.1:acme::x1",
        "opencode/qwen3.5_coder",
        "openai/a/b",
        "opencode/deepseek/v4",
        "openai//x",
        "openai-codex/has space",
        "openai/é",
        "openai/_x",
        "openai/-x",
        "openai/.x",
        "openai/x@q4",
        "openai/✦",
        "openai/ x",
        "openai/x ",
        "openai/a\n",
        "openai/a\tb",
        "openai/a\u{7f}",
        "openai/",
        "/gpt-5",
        "openai",
        "",
        "OpenAI/gpt-5",
        "open ai/x",
        "openrouter/moonshotai/kimi-k3.1-pro",
        "venice/x",
        "venice-2/x",
        "venice-3/x",
        "openai-2/gpt-5",
        "my-server/llama 4",
        "server/x",
        "ollama/qwen3:8b",
        "ollama/hf.co/unsloth/Qwen3-30B:Q4_K_M",
        "lmstudio/qwen/qwen3-8b@q4_k_m",
        "lmstudio/ x",
    ]
    .into_iter()
    .map(String::from)
    .chain([
        format!("openai/{}", a(128)),
        format!("openai/{}", a(129)),
        format!("openai/{}", a(249)),
        format!("openai/{}", a(250)),
        format!("openai/{}", a(256)),
        format!("ollama/{}", a(249)),
        format!("ollama/{}", a(256)),
        format!("ollama/{}", a(257)),
        format!("ollama/{}", "é".repeat(256)),
        format!("ollama/{}", "😀".repeat(128)),
        format!("ollama/{}", "😀".repeat(129)),
        format!("{}/x", a(40)),
        format!("{}/x", a(41)),
    ])
    .collect();
    let settings = settings();
    let servers = local_servers(&settings, &Env::new()).unwrap();
    let rows: Vec<Value> = ids
        .iter()
        .map(|id| {
            let cloud = parse_model_id(id).map(|parsed| json!([parsed.provider.as_str(), shown(&parsed.model)]));
            let local = parse_local_model_id(id, &servers).map(|parsed| json!([parsed.server.id, shown(&parsed.model)]));
            json!({"id": shown(id), "cloud": cloud, "local": local, "settings": valid_model(Some(id), &settings)})
        })
        .collect();
    let named: Vec<Value> = servers.iter().map(|server| json!({"name": server.name, "id": server.id, "where": server.r#where})).collect();
    let mut env = env();
    env.insert("KUMI_MODEL".into(), "nope".into());
    let mut errors = Map::new();
    errors.insert("KUMI_MODEL=nope".into(), json!(load_inference_config(&env).err().map(|error| error.to_string())));
    for args in [["login", "nope"], ["logout", "nope"], ["model", "nope"]] {
        errors.insert(args.join(" "), json!(refused(&args)));
    }
    golden("ids.json", &json!({"ids": rows, "servers": named, "errors": errors}));
}
