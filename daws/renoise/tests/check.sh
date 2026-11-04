#!/bin/sh
# Checks the tool with lua-language-server against Renoise's Lua API definitions.
# RENOISE_DEFINITIONS: the definitions' library folder (Renoise.app/Contents/Resources/Scripts/Types/
# renoise/library, or library/ in github.com/renoise/definitions). Defaults to an installed Renoise.
set -eu
here=$(cd "$(dirname "$0")" && pwd)
defs=${RENOISE_DEFINITIONS:-}
if [ -z "$defs" ]; then
  for app in /Applications/Renoise.app "$HOME/Applications/Renoise.app"; do
    if [ -d "$app/Contents/Resources/Scripts/Types/renoise/library" ]; then
      defs="$app/Contents/Resources/Scripts/Types/renoise/library"
      break
    fi
  done
fi
if [ -z "$defs" ] || [ ! -d "$defs" ]; then
  echo "check.sh: set RENOISE_DEFINITIONS to Renoise's API definitions" >&2
  exit 2
fi
tmp=$(mktemp -d "${TMPDIR:-/tmp}/kumi-renoise-luals.XXXXXX")
. "$here/children.sh"
trap 'stop_child; rm -rf "$tmp"' EXIT
cat > "$tmp/luarc.json" <<JSON
{
  "runtime.version": "LuaJIT",
  "workspace.library": ["$defs"],
  "workspace.checkThirdParty": false,
  "diagnostics.globals": ["KUMI_BRIDGE"]
}
JSON
run lua-language-server --check="$here/../com.kumi.Bridge.xrnx" --configpath="$tmp/luarc.json" \
  --checklevel=Information --logpath="$tmp/log" >"$tmp/out" 2>&1 || true
out=$(cat "$tmp/out")
case "$out" in
  *"no problems found"*) echo "lua-language-server: no problems found" ;;
  *) printf '%s\n' "$out" | tr '\r' '\n' | grep -v '^ *$' | grep -v '^>' >&2; exit 1 ;;
esac
