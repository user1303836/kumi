#!/bin/sh
# Kumi installer for macOS (and Linux). Run it with:
#
#   curl -fsSL https://raw.githubusercontent.com/user1303836/kumi/main/install.sh | sh
#
# It puts native Kumi in ~/.kumi (no admin rights, nothing outside your home folder),
# adds ~/.kumi/bin to your PATH, and checks every download against its published checksum. Running it
# again updates or repairs Kumi. Your settings, sign-ins and conversations in ~/.kumi are never touched.
#
# Settings (optional): KUMI_HOME (where to install, default ~/.kumi), KUMI_VERSION (a release such as
# 1.1.0, default the latest), KUMI_RELEASES (where releases are downloaded from), KUMI_NO_MODIFY_PATH=1
# (leave shell startup files alone).
#
# Everything runs inside main(), so a download cut short can't run half a script.

set -u

main() {
  KUMI_HOME="${KUMI_HOME:-$HOME/.kumi}"
  if [ -n "${KUMI_RELEASES:-}" ]; then base="${KUMI_RELEASES%/}"
  elif [ -n "${KUMI_VERSION:-}" ]; then base="https://github.com/user1303836/kumi/releases/download/v${KUMI_VERSION#v}"
  else base="https://github.com/user1303836/kumi/releases/latest/download"; fi

  # Plain lines, as in Kumi's own setup: the steps aligned, "done" in mint. Colour only in a terminal
  # that has it, and never with NO_COLOR.
  bold=""; dim=""; mint=""; reset=""; tty=""
  if [ -t 1 ] && [ "${TERM:-}" != "dumb" ]; then
    tty=1; bold="$(printf '\033[1m')"; dim="$(printf '\033[2m')"; reset="$(printf '\033[0m')"
    if [ -z "${NO_COLOR:-}" ]; then
      case "${COLORTERM:-}" in
        truecolor|24bit) mint="$(printf '\033[38;2;134;227;181m')" ;;
        *) mint="$(printf '\033[38;5;115m')" ;;
      esac
    fi
  fi
  say() { printf '%s\n' "$*"; }
  # A step's line: "…" while it runs (in a terminal), then "done" and what it did.
  begin() { step_name="$1"; if [ -n "$tty" ]; then printf '  %-20s%s' "$1" "${dim}…${reset}"; fi; }
  finish() {
    if [ -n "$tty" ]; then printf '\r\033[K'; fi
    if [ -n "${1:-}" ]; then printf '  %-20s%s%s\n' "$step_name" "${mint}done${reset}" "${dim} · $1${reset}"
    else printf '  %-20s%s\n' "$step_name" "${mint}done${reset}"; fi
  }
  fail() { printf '\n%s\n' "Kumi couldn't be installed: $*" >&2; exit 1; }

  say ""

  # ── What this computer is ──────────────────────────────────────────────
  os="$(uname -s)"; arch="$(uname -m)"
  case "$os" in
    Darwin) platform="darwin" ;;
    Linux) platform="linux"
      if [ -f /etc/alpine-release ] || ldd --version 2>&1 | grep -qi musl; then
        fail "this Linux uses musl (Alpine), which this Kumi release doesn't support."
      fi ;;
    MINGW*|MSYS*|CYGWIN*) fail "on Windows, open PowerShell and run: irm https://raw.githubusercontent.com/user1303836/kumi/main/install.ps1 | iex" ;;
    *) fail "Kumi runs on macOS and Windows (and Linux); this is $os." ;;
  esac
  # A Terminal running under Rosetta says x86_64 on an Apple Silicon Mac; Kumi uses the native architecture.
  if [ "$platform" = "darwin" ] && [ "$(sysctl -n hw.optional.arm64 2>/dev/null || echo 0)" = "1" ]; then arch="arm64"; fi
  case "$arch" in
    arm64|aarch64) arch="aarch64" ;;
    x86_64|amd64) arch="x86_64" ;;
    *) fail "this processor ($arch) isn't supported by this Kumi release." ;;
  esac
  if [ "$platform" = "darwin" ]; then
    macos="$(sw_vers -productVersion 2>/dev/null || echo 0)"
    if [ "${macos%%.*}" -lt 13 ] 2>/dev/null; then fail "Kumi needs macOS 13 (Ventura) or later; this Mac has macOS $macos."; fi
  fi

  if [ "$platform" = "darwin" ]; then target="$arch-apple-darwin"; else target="$arch-unknown-linux-gnu"; fi

  # ── Tools it needs (every Mac has them) ────────────────────────────────
  if command -v curl >/dev/null 2>&1; then
    fetch() { curl -fsSL --retry 3 --retry-delay 2 --connect-timeout 20 -o "$2" "$1"; }
  elif command -v wget >/dev/null 2>&1; then
    fetch() { wget -q --tries=3 --timeout=30 -O "$2" "$1"; }
  else fail "it needs curl or wget to download."; fi
  command -v tar >/dev/null 2>&1 || fail "it needs tar to unpack."
  if command -v shasum >/dev/null 2>&1; then sha() { shasum -a 256 "$1" | cut -d' ' -f1; }
  elif command -v sha256sum >/dev/null 2>&1; then sha() { sha256sum "$1" | cut -d' ' -f1; }
  else fail "it needs shasum or sha256sum to check downloads."; fi

  # ── Room and a place to work ───────────────────────────────────────────
  mkdir -p "$KUMI_HOME" || fail "couldn't make $KUMI_HOME."
  free_kb="$(df -Pk "$KUMI_HOME" 2>/dev/null | awk 'NR==2 {print $4}')"
  if [ -n "$free_kb" ] && [ "$free_kb" -lt 300000 ] 2>/dev/null; then
    fail "there's only $((free_kb / 1024)) MB free; Kumi needs about 300 MB. Free some space and run this again."
  fi
  work="$(mktemp -d "$KUMI_HOME/.install.XXXXXX")" || fail "couldn't make a working folder in $KUMI_HOME."
  trap 'rm -rf "$work"' EXIT INT TERM

  # ── Which Kumi ─────────────────────────────────────────────────────────
  fetch "$base/kumi-release-$target.json" "$work/release.json"; got=$?
  if [ "$got" -ne 0 ]; then
    # curl -f exits 22, and wget 8, when the server answered with an error (no release there) rather than not at all.
    if [ "$got" -eq 22 ] || [ "$got" -eq 8 ]; then fail "there's no Kumi release to install at ${base#https://} yet. Try again later."; fi
    fail "couldn't reach GitHub ($base). Check your internet connection and try again."
  fi
  field() { sed -n "s/.*\"$1\" *: *\"\\([^\"]*\\)\".*/\\1/p" "$work/release.json" | head -n 1; }
  kumi_version="$(field kumi)"; bundle="$(field bundle)"; bundle_sha="$(field sha256)"
  [ "$(field runtime)" = "rust-native" ] && [ "$(field target)" = "$target" ] || fail "the release doesn't match this computer."
  printf '%s\n' "$kumi_version" | grep -Eq '^[0-9]+\.[0-9]+\.[0-9]+(-[A-Za-z0-9_.]+)?$' || fail "the release version didn't make sense."
  printf '%s\n' "$bundle" | grep -Eq '^[A-Za-z0-9_.-]+\.tar\.gz$' || fail "the release archive name didn't make sense."
  printf '%s\n' "$bundle_sha" | grep -Eq '^[0-9a-f]{64}$' || fail "the release checksum didn't make sense."
  # The header: the wordmark, and the version at the right of the 60 columns after the 2-column margin,
  # as in Kumi's own setup.
  pad=$((56 - ${#kumi_version})); [ "$pad" -gt 0 ] || pad=1
  printf "  %s%${pad}s%s\n\n" "${bold}kumi${reset}" "" "${dim}${kumi_version}${reset}"

  # ── Kumi ───────────────────────────────────────────────────────────────
  begin "Download"
  fetch "$base/$bundle" "$work/kumi.tar.gz" || fail "couldn't download Kumi from GitHub."
  [ "$(sha "$work/kumi.tar.gz")" = "$bundle_sha" ] || fail "Kumi's download didn't match its checksum, so it wasn't used. Try again."
  finish "checked against its checksum"
  begin "Install"
  mkdir -p "$work/app" && tar -xzf "$work/kumi.tar.gz" -C "$work/app" || fail "couldn't unpack Kumi."
  KUMI_INSTALLED=1 KUMI_HOME="$KUMI_HOME" "$work/app/kumi" --version >/dev/null 2>&1 || fail "the downloaded Kumi didn't start. Please report this at github.com/user1303836/kumi/issues."
  app="$KUMI_HOME/app"
  rm -rf "$app.previous"; [ -d "$app" ] && mv "$app" "$app.previous"
  mv "$work/app" "$app" || { [ -d "$app.previous" ] && mv "$app.previous" "$app"; fail "couldn't put Kumi in place."; }
  home_shown="$KUMI_HOME"
  case "$home_shown" in "$HOME"/*) home_shown="~/${home_shown#"$HOME"/}" ;; esac
  finish "$home_shown"

  # ── The kumi command ───────────────────────────────────────────────────
  mkdir -p "$KUMI_HOME/bin"
  cat > "$KUMI_HOME/bin/kumi" <<'LAUNCHER'
#!/bin/sh
KUMI_HOME="${KUMI_HOME:-$(cd "$(dirname "$0")/.." && pwd)}"
export KUMI_HOME KUMI_INSTALLED=1
if [ -x "$KUMI_HOME/app/kumi" ]; then
  exec "$KUMI_HOME/app/kumi" "$@"
fi
exec "$KUMI_HOME/node/bin/node" "$KUMI_HOME/app/apps/kumi/bin/kumi.mjs" "$@"
LAUNCHER
  chmod 755 "$KUMI_HOME/bin/kumi"

  # ── PATH ───────────────────────────────────────────────────────────────
  begin "PATH"
  bin="$KUMI_HOME/bin"; marker="# Added by the Kumi installer"; added=""
  case ":$PATH:" in *":$bin:"*) on_path=1 ;; *) on_path="" ;; esac
  if [ -z "${KUMI_NO_MODIFY_PATH:-}" ]; then
    shell_name="$(basename "${SHELL:-sh}")"
    case "$shell_name" in
      zsh) rc="${ZDOTDIR:-$HOME}/.zshrc"; line="export PATH=\"$bin:\$PATH\"" ;;
      bash)
        # A bash login shell (every Terminal window on a Mac) reads only the first of these that exists, so
        # Kumi adds to that one and never makes a .bash_profile that would hide someone's .profile. On Linux,
        # new terminal windows read .bashrc.
        rc="$HOME/.profile"
        if [ "$platform" = "linux" ] && [ -f "$HOME/.bashrc" ]; then rc="$HOME/.bashrc"
        elif [ -f "$HOME/.bash_profile" ]; then rc="$HOME/.bash_profile"
        elif [ -f "$HOME/.bash_login" ]; then rc="$HOME/.bash_login"; fi
        line="export PATH=\"$bin:\$PATH\"" ;;
      # For each fish session (fish_add_path would keep it in fish's own saved PATH, past an uninstall).
      fish) rc="$HOME/.config/fish/conf.d/kumi.fish"; line="contains -- \"$bin\" \$PATH; or set -gx PATH \"$bin\" \$PATH" ;;
      *) rc="$HOME/.profile"; line="export PATH=\"$bin:\$PATH\"" ;;
    esac
    if ! grep -qs "$marker" "$rc"; then
      mkdir -p "$(dirname "$rc")"
      printf '\n%s\n%s\n' "$marker" "$line" >> "$rc" && added="$rc"
    fi
  fi

  case "$added" in "$HOME"/*) added="~/${added#"$HOME"/}" ;; esac
  if [ -n "$added" ]; then finish "added in ${added}, for new terminal windows"
  elif [ -n "${KUMI_NO_MODIFY_PATH:-}" ] && [ -z "$on_path" ]; then
    if [ -n "$tty" ]; then printf '\r\033[K'; fi
    printf '  %-20s%s\n' "PATH" "${dim}left as it is (KUMI_NO_MODIFY_PATH)${reset}"
  else finish; fi

  # ── Done ───────────────────────────────────────────────────────────────
  say ""
  say "  ${dim}Update with: kumi update · Remove with: kumi uninstall${reset}"
  say ""
  # Kumi starts here and now: it signs you in and connects to Live. Its keyboard is the terminal's, since
  # this script's own input is the download.
  if [ -z "${KUMI_NO_LAUNCH:-}" ] && [ -z "${CI:-}" ] && [ -t 1 ] && { : </dev/tty; } 2>/dev/null; then
    say "  Starting Kumi…"
    # exec replaces this shell, so the EXIT trap never runs: the download goes now.
    rm -rf "$work"; trap - EXIT INT TERM
    exec "$bin/kumi" </dev/tty
  fi
  if [ -z "$on_path" ]; then
    say "  Next, in a new terminal window (or after: export PATH=\"$bin:\$PATH\"): kumi"
  else
    say "  Next: kumi"
  fi
  say "  ${dim}It signs you in and connects to Live.${reset}"
}

main "$@"
