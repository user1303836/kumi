use std::sync::LazyLock;

/// How the producer runs Kumi, for every message that says what to type: "kumi bridge" once Kumi is
/// installed (its launcher sets KUMI_INSTALLED), "cargo run -p kumi -- bridge" from a checkout of the
/// repository. KUMI_HOME is where the installer put it.
pub static INSTALLED: LazyLock<bool> = LazyLock::new(|| std::env::var("KUMI_INSTALLED").as_deref() == Ok("1"));

/// The command before a subcommand: "kumi" or "cargo run -p kumi --".
pub static KUMI: LazyLock<&'static str> = LazyLock::new(|| if *INSTALLED { "kumi" } else { "cargo run -p kumi --" });

/// The command that starts Kumi: "kumi" or "cargo run -p kumi --".
pub static KUMI_START: LazyLock<&'static str> = LazyLock::new(|| if *INSTALLED { "kumi" } else { "cargo run -p kumi --" });

/// What puts a missing or broken Kumi back: the installer again, or a rebuild of the checkout.
pub static KUMI_REPAIR: LazyLock<&'static str> =
    LazyLock::new(
        || if *INSTALLED { "the Kumi installer again (github.com/user1303836/kumi)" } else { "cargo build --release --workspace" },
    );
