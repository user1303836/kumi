#!/bin/sh
# Runs the tests in a home of their own: HOME, USERPROFILE, APPDATA, LOCALAPPDATA, XDG_CONFIG_HOME and
# KUMI_HOME point into a fresh temporary folder, so no test can reach this machine's Live folders,
# Remote Scripts or ~/.kumi through a default it forgot to override (home_dir(), kumi_dir() and the like).
# Live's folders set in this shell (KUMI_REMOTE_SCRIPTS_DIR, KUMI_LIVE_EXTENSIONS_DIR) aren't passed on.
# Cargo and rustup keep their own folders, named before HOME moves. The tests run on cargo-nextest, each in a process of
# its own: a crate's tests are one binary, and some set the environment. The arguments are `cargo nextest run`'s;
# KUMI_NEXTEST_LIST names a file for the tests the run selects (`cargo nextest list`'s JSON), written first, to compare
# one run's tests with another's. KUMI_NEXTEST=0 runs `cargo test` instead, with its arguments: a crate's tests then
# share one process. KUMI_PACKAGES narrows the run to those packages.
set -eu
export CARGO_HOME="${CARGO_HOME:-$HOME/.cargo}" RUSTUP_HOME="${RUSTUP_HOME:-$HOME/.rustup}"
test_home="$(mktemp -d "${TMPDIR:-/tmp}/kumi-test-home-XXXXXX")"
trap 'rm -rf "$test_home"' EXIT INT TERM
export HOME="$test_home" USERPROFILE="$test_home" APPDATA="$test_home/AppData/Roaming" LOCALAPPDATA="$test_home/AppData/Local" XDG_CONFIG_HOME="$test_home/.config" KUMI_HOME="$test_home/.kumi"
unset KUMI_REMOTE_SCRIPTS_DIR KUMI_LIVE_EXTENSIONS_DIR
# The whole workspace, or only the packages named in KUMI_PACKAGES (space separated), with every test of theirs: a
# bridge lane's crates, or the Logic bridge's CI job.
scope="--workspace"
if [ -n "${KUMI_PACKAGES:-}" ]; then
  scope=""
  for package in $KUMI_PACKAGES; do scope="$scope -p $package"; done
fi
if [ "${KUMI_NEXTEST:-1}" != 0 ]; then
  if ! cargo nextest --version > /dev/null 2>&1; then
    echo "test-isolated.sh: the tests run on cargo-nextest: cargo install cargo-nextest --locked (KUMI_NEXTEST=0: cargo test)" >&2
    exit 1
  fi
  # A profile that names its test binaries (`binary(…)`, or a crate's `binary_id(<crate>::it)`, in its default-filter,
  # .config/nextest.toml) builds only those: nextest builds every binary it could run, whatever the filter, and runs the
  # ones the filter leaves.
  only=""
  if [ -n "${KUMI_NEXTEST_BUILD_PROFILE:-}" ]; then
    only="$(awk -v p="[profile.$KUMI_NEXTEST_BUILD_PROFILE]" '$0 == p { on = 1; next } /^\[/ { on = 0 } on' .config/nextest.toml \
      | grep -o -E 'binary\([a-z0-9_]*\)|binary_id\([a-z0-9_-]*::it\)' | sed -E 's/^binary(_id)?\(([a-z0-9_-]*::)?([a-z0-9_]*)\)$/--test \3/' \
      | sort -u | tr '\n' ' ')"
  fi
  # The scope above, or the crates named (-p): CI's Linux parts each take some (--exclude works with the workspace).
  for arg in "$@"; do case "$arg" in -p | -p* | --package | --package=*) scope= ;; esac; done
  if [ -n "${KUMI_NEXTEST_LIST:-}" ]; then
    # shellcheck disable=SC2086
    cargo nextest list $scope --locked $only "$@" --message-format json > "$KUMI_NEXTEST_LIST"
  fi
  # shellcheck disable=SC2086
  cargo nextest run $scope --locked $only "$@"
else
  # shellcheck disable=SC2086
  cargo test $scope --locked "$@"
fi
