//! The providers' table: a provider added after the first five never takes a custom server's ID, and is hidden behind
//! the server that has it.
use kumi_ai::providers::{
    local::{hidden_providers, local_servers, parse_local_model_id, LocalServer, ServerSetting},
    parse_model_id,
    table::{add_test_rows, rows, ProviderRow, API_KEY_ROW},
    ProviderId,
};
use std::collections::HashMap;

/// A provider added later, as Venice was: only this test's thread has it.
static ADDED: [ProviderRow; 1] = [ProviderRow {
    id: "acme",
    name: "Acme AI",
    short_name: "Acme",
    credential: "acme",
    key_env: &["ACME_API_KEY"],
    base_url: "https://api.acme.example.test/v1",
    ..API_KEY_ROW
}];
fn servers(names: &[&str]) -> Vec<LocalServer> {
    let settings: Vec<_> = names
        .iter()
        .map(|name| ServerSetting {
            name: name.to_string(),
            base_url: "https://models.example.test/v1".into(),
            api_key: None,
            format: None,
        })
        .collect();
    local_servers(&settings, &HashMap::new()).unwrap()
}
fn ids(servers: &[LocalServer]) -> Vec<&str> {
    servers.iter().map(|server| server.id.as_str()).collect()
}

#[test]
fn the_first_five_providers_keep_their_ids_from_custom_servers() {
    assert_eq!(
        rows().filter(|row| row.reserved).map(|row| row.id).collect::<Vec<_>>(),
        ["openai-codex", "openai", "anthropic", "opencode", "opencode-go"]
    );
    assert_eq!(ids(&servers(&["OpenAI", "Anthropic", "OpenCode Go"])), ["ollama", "lmstudio", "openai-2", "anthropic-2", "opencode-go-2"]);
    assert!(hidden_providers(&servers(&["OpenAI"])).is_empty());
}

#[test]
fn a_provider_added_later_is_hidden_behind_the_custom_server_that_has_its_id() {
    add_test_rows(&ADDED);
    let acme = ProviderId::parse("acme").unwrap();
    assert!(!acme.row().reserved);
    // The server named Acme keeps the ID it had before Kumi knew Acme; a second one takes the next, as before.
    let named = servers(&["Acme", "Acme", "OpenAI"]);
    assert_eq!(ids(&named), ["ollama", "lmstudio", "acme", "acme-2", "openai-2"]);
    let hidden = hidden_providers(&named);
    assert_eq!(hidden.iter().map(|(provider, server)| (provider.as_str(), server.id.as_str())).collect::<Vec<_>>(), [("acme", "acme")]);
    // Its saved models stay the server's: a server is looked for first, and has them.
    assert_eq!(parse_local_model_id("acme/qwen3-coder", &named).map(|parsed| parsed.server.name), Some("Acme".into()));
    // Without such a server, the provider is there for the producer.
    assert!(hidden_providers(&servers(&["My Server"])).is_empty());
    assert_eq!(parse_model_id("acme/qwen3-coder").map(|parsed| parsed.provider), Some(acme));
    // The same for the providers the table has now: a server named Venice from before keeps venice.
    let named = servers(&["Venice"]);
    assert_eq!(ids(&named), ["ollama", "lmstudio", "venice"]);
    assert_eq!(hidden_providers(&named).iter().map(|(provider, _)| provider.as_str()).collect::<Vec<_>>(), ["venice"]);
}
