#!/bin/sh
# Runs `cargo test` in a home of its own: HOME, USERPROFILE, APPDATA, LOCALAPPDATA, XDG_CONFIG_HOME and
# KUMI_HOME point into a fresh temporary folder, so no test can reach this machine's Live folders,
# Remote Scripts or ~/.kumi through a default it forgot to override (home_dir(), kumi_dir() and the like).
# Live's folders set in this shell (KUMI_REMOTE_SCRIPTS_DIR, KUMI_LIVE_EXTENSIONS_DIR) aren't passed on.
# Cargo and rustup keep their own folders, named before HOME moves. The arguments are `cargo test`'s, or
# `cargo nextest run`'s with KUMI_NEXTEST=1.
set -eu
export CARGO_HOME="${CARGO_HOME:-$HOME/.cargo}" RUSTUP_HOME="${RUSTUP_HOME:-$HOME/.rustup}"
test_home="$(mktemp -d "${TMPDIR:-/tmp}/kumi-test-home-XXXXXX")"
trap 'rm -rf "$test_home"' EXIT INT TERM
export HOME="$test_home" USERPROFILE="$test_home" APPDATA="$test_home/AppData/Roaming" LOCALAPPDATA="$test_home/AppData/Local" XDG_CONFIG_HOME="$test_home/.config" KUMI_HOME="$test_home/.kumi"
unset KUMI_REMOTE_SCRIPTS_DIR KUMI_LIVE_EXTENSIONS_DIR
if [ "${KUMI_NEXTEST:-}" = 1 ]; then
  # A profile that names its test binaries (`binary(…)` in its default-filter, .config/nextest.toml) builds only those:
  # nextest builds every binary it could run, whatever the filter, and runs the ones the filter leaves.
  only=""
  if [ -n "${KUMI_NEXTEST_BUILD_PROFILE:-}" ]; then
    only="$(awk -v p="[profile.$KUMI_NEXTEST_BUILD_PROFILE]" '$0 == p { on = 1; next } /^\[/ { on = 0 } on' .config/nextest.toml \
      | grep -o 'binary([a-z0-9_]*)' | sed 's/^binary(\(.*\))$/--test \1/' | tr '\n' ' ')"
  fi
  # shellcheck disable=SC2086
  cargo nextest run --workspace --locked $only "$@"
else
  cargo test --workspace --locked "$@"
fi
