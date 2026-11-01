#!/bin/sh
# Runs the Renoise tool's Lua tests on LuaJIT 2.1, then lua-language-server's check when it's installed (KUMI_REQUIRE_LUALS=1
# fails when it isn't; KUMI_SKIP_LUALS=1 skips the check).
set -eu
here=$(cd "$(dirname "$0")" && pwd)
tmp=$(mktemp -d "${TMPDIR:-/tmp}/kumi-renoise-tests.XXXXXX")
. "$here/children.sh"
trap 'stop_child; rm -rf "$tmp"' EXIT
run luajit "$here/run.lua" "$tmp" "$@"
if [ "${KUMI_SKIP_LUALS:-}" != 1 ]; then
  if command -v lua-language-server >/dev/null 2>&1; then
    run "$here/check.sh"
  elif [ "${KUMI_REQUIRE_LUALS:-}" = 1 ]; then
    echo "run.sh: KUMI_REQUIRE_LUALS=1 and lua-language-server isn't installed" >&2
    exit 1
  fi
fi
