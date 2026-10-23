use std::sync::LazyLock;

/// How the producer runs Kumi, for every message that says what to type: "kumi bridge" once Kumi is
/// installed (its launcher sets KUMI_INSTALLED), "cargo run -p kumi -- bridge" from a checkout of the
/// repository. KUMI_HOME is where the installer put it.
pub static INSTALLED: LazyLock<bool> = LazyLock::new(|| std::env::var("KUMI_INSTALLED").as_deref() == Ok("1"));

/// The command before a subcommand: "kumi" or "cargo run -p kumi --".
pub static KUMI: LazyLock<&'static str> = LazyLock::new(|| if *INSTALLED { "kumi" } else { "cargo run -p kumi --" });

/// The command that starts Kumi: "kumi" or "cargo run -p kumi --".
pub static KUMI_START: LazyLock<&'static str> = LazyLock::new(|| if *INSTALLED { "kumi" } else { "cargo run -p kumi --" });

/// The one-liners that install Kumi, or repair it, as the READMEs give them: Kumi's one copy of each. install.sh,
/// install.ps1, the launchers they write and the READMEs repeat them, and tests keep every copy the same, so a
/// new address goes here first and the tests name each place to follow.
pub const INSTALL_SH: &str = "curl -fsSL https://github.com/user1303836/kumi/releases/latest/download/install.sh | sh";
pub const INSTALL_PS1: &str = "irm https://github.com/user1303836/kumi/releases/latest/download/install.ps1 | iex";
/// This computer's one-liner.
pub const REINSTALL: &str = if cfg!(windows) { INSTALL_PS1 } else { INSTALL_SH };

/// What puts a missing or broken Kumi back: the installer again, or a rebuild of the checkout.
pub static KUMI_REPAIR: LazyLock<String> =
    LazyLock::new(
        || if *INSTALLED { format!("the Kumi installer again ({REINSTALL})") } else { "cargo build --release --workspace".into() },
    );

/// The one way Kumi says to connect Live: Kumi itself puts the bridge in Live and can restart Live; `kumi bridge` does
/// it by hand, with Live closed.
pub static CONNECT_LIVE: LazyLock<String> = LazyLock::new(|| connect_live(&KUMI_START, &KUMI));

/// That sentence, with `start` starting Kumi and `kumi` before its commands.
pub fn connect_live(start: &str, kumi: &str) -> String {
    format!("Run {start}: it puts the bridge in Live and can restart Live for you (or, with Live closed: {kumi} bridge)")
}

/// The same where Kumi's setup doesn't run (plain mode): the way by hand first.
pub fn connect_live_by_hand(kumi: &str) -> String {
    format!("With Live closed, run: {kumi} bridge (the full-screen app can also restart Live for you)")
}
