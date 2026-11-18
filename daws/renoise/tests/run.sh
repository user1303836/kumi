#!/bin/sh
# Runs the Renoise tool's Lua tests on LuaJIT 2.1 and, at once, lua-language-server's check when it's installed
# (KUMI_REQUIRE_LUALS=1 fails when it isn't; KUMI_SKIP_LUALS=1 skips the check). The check's words come after the tests'.
set -eu
here=$(cd "$(dirname "$0")" && pwd)
tmp=$(mktemp -d "${TMPDIR:-/tmp}/kumi-renoise-tests.XXXXXX")
. "$here/children.sh"
check=
trap 'stop_child; if [ -n "$check" ]; then stop_tree "$check"; wait "$check" 2>/dev/null || true; fi; rm -rf "$tmp"' EXIT
if [ "${KUMI_SKIP_LUALS:-}" != 1 ]; then
  if command -v lua-language-server >/dev/null 2>&1; then
    "$here/check.sh" > "$tmp/check.out" 2>&1 &
    check=$!
  elif [ "${KUMI_REQUIRE_LUALS:-}" = 1 ]; then
    echo "run.sh: KUMI_REQUIRE_LUALS=1 and lua-language-server isn't installed" >&2
    exit 1
  fi
fi
mkdir "$tmp/tests"
tests=0
run luajit "$here/run.lua" "$tmp/tests" "$@" || tests=$?
checked=0
if [ -n "$check" ]; then
  wait "$check" || checked=$?
  check=
  cat "$tmp/check.out"
fi
if [ "$tests" != 0 ]; then exit "$tests"; fi
exit "$checked"
