//! Records a running SoundCraft's answers into a file of vectors, which the tests replay through a fake of its control
//! server. Each line holds a request this scene makes and the app's whole answer to it, `{"method", "params", "reply"}`,
//! with the home folder written `~`.
//!
//! Run it against the lab's copy of SoundCraft, never a producer's: the scene changes the session, then takes the change
//! back. Start the app on the demo song with its channel on and a settings folder of its own, then give the recorder the
//! port it printed:
//!
//! ```sh
//! open -g -n -a SoundCraft --env SOUNDCRAFT_CONFIG_DIR="$LAB/profile" --stdout "$LAB/app.out" --args --control 0 --no-audio --demo
//! cargo run -p soundcraft-kumi --example record -- <port> daws/soundcraft/soundcraft-kumi/tests/vectors/soundcraft-<version>.ndjson
//! ```
use serde_json::{json, Value};
use soundcraft_kumi::control::Control;
use std::time::Duration;

/// The scene: what Kumi reads, a change and its undo, and the app's refusals (an unknown command, bad parameters, a
/// command with nothing to work on, a method the channel hasn't got).
fn scene() -> Vec<(&'static str, Value)> {
    let execute = |command: &str, params: Value| json!({"command": command, "params": params});
    vec![
        ("engine.commands", json!({})),
        ("session.inspect", json!({})),
        ("session.inspect", json!({"detail": "full"})),
        ("ui.inspect", json!({})),
        ("engine.execute", execute("engine.history", json!({}))),
        ("engine.execute", execute("mix.volume", json!({"tracks": ["Kick"], "db": -6}))),
        ("engine.execute", execute("engine.history", json!({}))),
        ("engine.execute", execute("edit.undo", json!({}))),
        ("engine.execute", execute("mix.nope", json!({}))),
        ("engine.execute", execute("mix.volume", json!({"tracks": ["Kick"]}))),
        ("engine.execute", execute("edit.cut", json!({}))),
        ("mix.volume", json!({"tracks": ["Kick"], "db": -6})),
    ]
}

#[tokio::main(flavor = "current_thread")]
async fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let [port, out] = args.as_slice() else {
        eprintln!("usage: record <port> <vectors.ndjson>   (SOUNDCRAFT_CONTROL_TOKEN, when the app needs one)");
        std::process::exit(2);
    };
    let port: u16 = port.parse().unwrap_or_else(|_| panic!("{port} isn't a port"));
    let control = Control::new(port, std::env::var("SOUNDCRAFT_CONTROL_TOKEN").ok().filter(|token| !token.is_empty()));
    let home = std::env::var("HOME").unwrap_or_default();
    let mut lines = String::new();
    for (method, params) in scene() {
        let reply = match control.exchange(method, params.clone(), Duration::from_secs(30)).await {
            Ok(reply) => reply,
            Err(error) => {
                eprintln!("{method} {params}: {error}");
                std::process::exit(1);
            }
        };
        let line = json!({"method": method, "params": params, "reply": reply}).to_string();
        lines.push_str(&if home.is_empty() { line } else { line.replace(&home, "~") });
        lines.push('\n');
        eprintln!("{method} {params}: {}", if reply["ok"] == true { "ok" } else { reply["error"].as_str().unwrap_or("?") });
    }
    std::fs::write(out, lines).unwrap_or_else(|error| panic!("{out}: {error}"));
}
