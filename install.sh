#!/bin/sh
# Kumi installer for macOS (and Linux). Run it with:
#
#   curl -fsSL https://github.com/user1303836/kumi/releases/latest/download/install.sh | sh
#
# It puts native Kumi in ~/.kumi (no admin rights, nothing outside your home folder),
# adds ~/.kumi/bin to your PATH, and checks every download against its published checksum. Running it
# again updates or repairs Kumi. Your settings, sign-ins and conversations in ~/.kumi are never touched.
#
# Settings (optional): KUMI_HOME (where to install, default ~/.kumi), KUMI_VERSION (a release such as
# 1.16.0, default latest), KUMI_RELEASES (where releases are downloaded from), KUMI_NO_MODIFY_PATH=1
# (leave shell startup files alone), KUMI_ALLOW_ROOT=1 (install for root when run with sudo).
#
# Everything runs inside main(), so a download cut short can't run half a script.

# Run by zsh (curl … | zsh), it splits words and expands globs as sh does: normal() below counts on it.
if [ -n "${ZSH_VERSION:-}" ]; then emulate sh; fi
set -u

main() {
  # Plain lines, as in Kumi's own setup: the steps aligned, "done" in mint. Colour only in a terminal
  # that has it, and never with NO_COLOR. Curl's progress bar shows while Kumi downloads, in a terminal.
  bold=""; dim=""; mint=""; reset=""; tty=""; bar=""
  if [ -t 1 ] && [ "${TERM:-}" != "dumb" ]; then
    tty=1; bold="$(printf '\033[1m')"; dim="$(printf '\033[2m')"; reset="$(printf '\033[0m')"
    if [ -z "${NO_COLOR:-}" ]; then
      case "${COLORTERM:-}" in
        truecolor|24bit) mint="$(printf '\033[38;2;134;227;181m')" ;;
        *) mint="$(printf '\033[38;5;115m')" ;;
      esac
    fi
    if [ -t 2 ]; then bar=1; fi
  fi
  # What's on screen last: a blank line (blank), or a line still being written, in a terminal (open).
  blank=""; open=""
  say() { printf '%s\n' "$*"; if [ -z "$*" ]; then blank=1; else blank=""; fi; open=""; }
  # A step's line: "…" while it runs (in a terminal), then "done" and what it did.
  begin() { step_name="$1"; if [ -n "$tty" ]; then printf '  %-20s%s' "$1" "${dim}…${reset}"; open=1; fi; }
  finish() {
    if [ -n "$tty" ]; then printf '\r\033[K'; fi
    if [ -n "${1:-}" ]; then say "$(printf '  %-20s%s%s' "$step_name" "${mint}done${reset}" "${dim} · $1${reset}")"
    else say "$(printf '  %-20s%s' "$step_name" "${mint}done${reset}")"; fi
  }
  # Why Kumi couldn't be installed, after a blank line, then what the program that failed said ($2), if anything.
  # The EXIT trap tidies up.
  fail() {
    if [ -n "$open" ]; then printf '\n' >&2; fi
    if [ -z "$blank" ] || [ -n "$open" ]; then printf '\n' >&2; fi
    printf '%s\n' "Kumi couldn't be installed: $1" >&2
    if [ -n "${2:-}" ]; then printf '  %s\n' "$2" >&2; fi
    exit 1
  }
  # A path with . and .. taken as written, for one that may not exist yet (KUMI_HOME=$HOME/new/..).
  normal() {
    # shellcheck disable=SC2086 # Split on / on purpose, with globbing off.
    (set -f; IFS=/; out=""
     for part in $1; do
       case "$part" in ""|.) ;; ..) out="${out%/*}" ;; *) out="$out/$part" ;; esac
     done
     printf '%s\n' "${out:-/}")
  }
  # The device a folder is on (GNU and busybox stat, then BSD's).
  device() { stat -c %d "$1" 2>/dev/null || stat -f %d "$1" 2>/dev/null; }
  # Whether a disk is mounted at a folder: the folder (/. follows a link to it) is on another device than its parent.
  mounted() { [ -d "$1" ] && [ "$(device "$1/.")" != "$(device "$1/..")" ]; }
  # A path as it reads on screen: ~/ for the home folder.
  # shellcheck disable=SC2088
  shown() { case "$1" in "$HOME"/*) printf '~/%s' "${1#"$HOME"/}" ;; *) printf '%s' "$1" ;; esac; }
  # A path as the producer would type it: the same, or the whole path in quotes when it has a space.
  typed() { case "$(shown "$1")" in *" "*) printf '"%s"' "$1" ;; *) shown "$1" ;; esac; }
  # Whether version $1 comes before $2, compared part by part as numbers (1.9.0 before 1.10.0).
  older() {
    awk -v a="${1%%-*}" -v b="${2%%-*}" 'BEGIN {
      n = split(a, x, "."); m = split(b, y, "."); if (m > n) n = m
      for (i = 1; i <= n; i++) { if (x[i] + 0 < y[i] + 0) exit 0; if (x[i] + 0 > y[i] + 0) exit 1 }
      exit 1 }'
  }
  releases="https://github.com/user1303836/kumi/releases"

  # ── Ctrl-C ─────────────────────────────────────────────────────────────
  # Nothing in Kumi's folder changes until the new Kumi is in place, so stopping before then changes nothing:
  # this run's working folder goes, and so does Kumi's folder when this run made it and it's still empty.
  work=""; made_home=""
  tidy() {
    if [ -n "$work" ]; then rm -rf "$work"; fi
    if [ -n "$made_home" ]; then rmdir "$KUMI_HOME" 2>/dev/null; fi
    return 0
  }
  stopped() { tidy; trap - EXIT; printf '\n%s\n' "Stopped; nothing was changed." >&2; exit 130; }
  trap tidy EXIT
  trap stopped INT TERM HUP

  say ""

  # ── What this computer is ──────────────────────────────────────────────
  os="$(uname -s)"; arch="$(uname -m)"
  case "$os" in
    Darwin) platform="darwin" ;;
    Linux) platform="linux"
      if [ -f /etc/alpine-release ] || ldd --version 2>&1 | grep -qi musl; then
        fail "this Linux uses musl (Alpine), which this Kumi release doesn't support."
      fi ;;
    MINGW*|MSYS*|CYGWIN*) fail "on Windows, open PowerShell and run: irm https://github.com/user1303836/kumi/releases/latest/download/install.ps1 | iex" ;;
    *) fail "Kumi runs on macOS and Windows (and Linux); this is $os." ;;
  esac
  # A Terminal running under Rosetta says x86_64 on an Apple Silicon Mac; Kumi uses the native architecture.
  if [ "$platform" = "darwin" ] && { [ "$(sysctl -n sysctl.proc_translated 2>/dev/null)" = "1" ] || [ "$(sysctl -n hw.optional.arm64 2>/dev/null)" = "1" ]; }; then
    arch="arm64"
  fi
  case "$arch" in
    arm64|aarch64) arch="aarch64" ;;
    x86_64|amd64) arch="x86_64" ;;
    *) fail "this processor ($arch) isn't supported by this Kumi release." ;;
  esac
  if [ "$platform" = "darwin" ]; then
    macos="$(sw_vers -productVersion 2>/dev/null || echo 0)"
    if [ "${macos%%.*}" -lt 13 ] 2>/dev/null; then fail "Kumi needs macOS 13 (Ventura) or later; this Mac has macOS $macos."; fi
  else
    # Linux's Kumi is built against glibc 2.35.
    glibc="$(getconf GNU_LIBC_VERSION 2>/dev/null | sed -n 's/^glibc //p')"
    if [ -z "$glibc" ]; then glibc="$(ldd --version 2>/dev/null | sed -n '1s/.* \([0-9][0-9]*\.[0-9][0-9]*\)$/\1/p')"; fi
    if [ -n "$glibc" ] && older "$glibc" 2.35; then
      fail "Kumi needs glibc 2.35 or later (Ubuntu 22.04, Debian 12 or newer); this Linux has glibc $glibc."
    fi
  fi
  if [ "$platform" = "darwin" ]; then target="$arch-apple-darwin"; else target="$arch-unknown-linux-gnu"; fi

  # Kumi installs for whoever runs this, with no admin rights: run with sudo, it would install for root rather than
  # the one who ran sudo. Root itself (a container, a root-only machine) is fine.
  if [ "$(id -u 2>/dev/null)" = "0" ] && [ -n "${SUDO_USER:-}" ] && [ "$SUDO_USER" != "root" ] && [ -z "${KUMI_ALLOW_ROOT:-}" ]; then
    fail "it's running with sudo, so Kumi would be installed for root, not for $SUDO_USER. Kumi needs no admin rights: run it again without sudo. To install it for root anyway, set KUMI_ALLOW_ROOT=1."
  fi

  # ── Which release ──────────────────────────────────────────────────────
  # KUMI_RELEASES (a folder of releases: a mirror, or a test's), else KUMI_VERSION, else the newest.
  version="${KUMI_VERSION:-}"; version="${version#[vV]}"
  if [ "$version" = "latest" ] || [ -n "${KUMI_RELEASES:-}" ]; then version=""; fi
  if [ -n "$version" ]; then
    printf '%s\n' "$version" | grep -Eq '^[0-9]+\.[0-9]+\.[0-9]+(-[A-Za-z0-9_.]+)?$' \
      || fail "KUMI_VERSION is \"${KUMI_VERSION:-}\"; it takes a release such as 1.16.0, or latest."
    if older "$version" 1.1.0; then
      fail "Kumi $version came before the installer (1.1.0). The releases are at $releases"
    elif older "$version" 1.7.6; then
      fail "Kumi $version came before native Kumi (1.7.6): install it with that release's own script: curl -fsSL https://raw.githubusercontent.com/user1303836/kumi/v$version/install.sh | KUMI_VERSION=$version sh"
    fi
  fi
  if [ -n "${KUMI_RELEASES:-}" ]; then base="${KUMI_RELEASES%/}"
  elif [ -n "$version" ]; then base="$releases/download/v$version"
  else base="$releases/latest/download"; fi

  # ── Tools it needs (every Mac has them) ────────────────────────────────
  # fetch gets $1 into $2, and says how in got (the program's exit status, curl's or wget's) and code (the HTTP
  # status; 000 for none). It's the status that tells a missing release: macOS's curl exits 56, not 22, for
  # GitHub's 404. With $3, curl draws its progress bar.
  if command -v curl >/dev/null 2>&1; then
    fetcher=curl
    fetch() {
      : > "$work/fetch.err"
      if [ -n "${3:-}" ]; then
        code="$(curl -#SL --retry 3 --retry-delay 2 --connect-timeout 20 -w '%{http_code} %{http_connect}' -o "$2" "$1")"; got=$?
      else
        code="$(curl -sSL --retry 3 --retry-delay 2 --connect-timeout 20 -w '%{http_code} %{http_connect}' -o "$2" "$1" 2>"$work/fetch.err")"; got=$?
      fi
      connect="${code#* }"; code="${code%% *}"
    }
  elif command -v wget >/dev/null 2>&1; then
    fetcher=wget; bar=""
    fetch() {
      wget --tries=3 --timeout=30 -S -O "$2" "$1" 2>"$work/fetch.err"; got=$?
      code="$(sed -n 's/^ *HTTP\/[0-9.]* \([0-9][0-9][0-9]\).*/\1/p' "$work/fetch.err" | tail -n 1)"; connect=""
      if [ -z "$code" ]; then code=000; fi
    }
  else fail "it needs curl or wget to download."; fi
  command -v tar >/dev/null 2>&1 || fail "it needs tar to unpack."
  if command -v shasum >/dev/null 2>&1; then sha() { shasum -a 256 "$1" | cut -d' ' -f1; }
  elif command -v sha256sum >/dev/null 2>&1; then sha() { sha256sum "$1" | cut -d' ' -f1; }
  else fail "it needs shasum or sha256sum to check downloads."; fi
  # Whether a download came back whole: an HTTP status that's an error (curl answers 0 for those) or a
  # program that failed isn't.
  fetched() { case "$code" in [45][0-9][0-9]) return 1 ;; esac; [ "$got" -eq 0 ]; }
  # A Wi-Fi sign-in page, or a proxy's, in place of what was asked for.
  html() { dd if="$1" bs=512 count=1 2>/dev/null | grep -Eqi '<(!doctype|html|head|body|title|meta)'; }
  captive="a Wi-Fi sign-in page or proxy answered instead of GitHub. Sign in to the Wi-Fi (open any web page), or check the proxy, then run this again."
  # Why a download from $1 didn't come through, in Kumi's own words (DNS, TLS, proxy, timeout, offline), into
  # message: the HTTP status when something answered, else what curl or wget said (kept in said, for after).
  unfetched() {
    host="${1#*://}"; host="${host%%/*}"
    said="$(sed '/^[[:space:]]*$/d' "$work/fetch.err" 2>/dev/null | tail -n 1 | cut -c1-300)"
    cause=""; message=""
    case "$code" in
      407) cause=proxy ;;
      511) message="$captive" ;;
      401|403) message="$host turned the download away (HTTP $code). A network that blocks GitHub does this: try another network." ;;
      429) message="$host is limiting downloads from this network (HTTP 429). Try again in a few minutes." ;;
      5[0-9][0-9]) message="$host had a problem (HTTP $code). Try again in a few minutes." ;;
      4[0-9][0-9]) message="$host answered HTTP $code for $1." ;;
    esac
    if [ "${connect:-000}" = 407 ]; then cause=proxy; message=""; fi
    if [ -z "$cause$message" ]; then
      # Each program's exit statuses mean their own things.
      case "$fetcher:$got" in
        curl:5|curl:97|wget:6) cause=proxy ;;
        curl:6) cause=dns ;;
        curl:7) if [ -n "$(proxies)" ]; then cause=proxy; else cause=offline; fi ;;
        curl:28) cause=timeout ;;
        curl:35|curl:51|curl:53|curl:54|curl:58|curl:59|curl:60|curl:64|curl:66|curl:77|curl:80|curl:82|curl:83|curl:90|curl:91|curl:98|wget:5) cause=tls ;;
        curl:23|wget:3) message="it couldn't save the download in $(shown "$KUMI_HOME") (is the disk full?)." ;;
        curl:1|curl:3) message="$1 isn't an address it can download from." ;;
        curl:47) message="$host kept redirecting: $captive" ;;
        *) case "$said" in
             *[Rr]esolv*|*"ame or service not known"*|*"nodename nor servname"*) cause=dns ;;
             *[Cc]ertificate*|*SSL*|*TLS*) cause=tls ;;
             *[Pp]roxy*) cause=proxy ;;
             *"timed out"*|*[Tt]imeout*) cause=timeout ;;
             *) cause=offline ;;
           esac ;;
      esac
    fi
    case "$cause" in
      dns) message="its address couldn't be looked up (DNS). Check that this computer is online; if it is, its DNS isn't answering, so try another network." ;;
      tls) message="the secure connection didn't check out (TLS). A Wi-Fi sign-in page, a school or office network that inspects secure connections, or a wrong clock does this: sign in to the Wi-Fi, try another network, or check the clock." ;;
      proxy) message="the proxy didn't let it through. Check the proxy setting, or try without the proxy." ;;
      timeout) message="nothing answered in time. Check the connection, then try again." ;;
      offline) message="the connection failed. Check that this computer is online, or try another network." ;;
    esac
    if [ -n "$cause" ]; then
      message="couldn't reach $host: $message"
      if [ -n "$(proxies)" ]; then message="$message Proxy settings: $(proxies)."; fi
    fi
  }
  # The proxy settings curl and wget read, as NAME=value without any user name or password (with or without a
  # scheme before them).
  proxies() {
    listed=""
    for name in HTTPS_PROXY https_proxy ALL_PROXY all_proxy NO_PROXY no_proxy; do
      eval "value=\${$name:-}"
      if [ -n "$value" ]; then
        printf '%s%s=%s' "${listed:+, }" "$name" "$(printf '%s' "$value" | sed -E 's#^([A-Za-z][A-Za-z0-9+.-]*://)?[^/]*@#\1#')"; listed=1
      fi
    done
  }

  # ── Room and a place to work ───────────────────────────────────────────
  KUMI_HOME="${KUMI_HOME:-$HOME/.kumi}"
  # Always an absolute path: it goes in PATH, and in the kumi command.
  # shellcheck disable=SC2088
  case "$KUMI_HOME" in "~") KUMI_HOME="$HOME" ;; "~/"*) KUMI_HOME="$HOME/${KUMI_HOME#"~/"}" ;; esac
  case "$KUMI_HOME" in /*) ;; *) KUMI_HOME="$(pwd)/$KUMI_HOME" ;; esac
  # One slash between names and none at the end: a HOME ending in / would otherwise give …//.kumi/bin in PATH.
  KUMI_HOME="$(printf '%s\n' "$KUMI_HOME" | sed 's#//*#/#g; s#\(.\)/$#\1#')"
  # A folder of Kumi's own: in the home folder itself or a disk's root, the names Kumi uses (bin, downloads, tools)
  # could be the producer's own, and kumi uninstall takes those names out. Compared as folders, however they're
  # spelled (other case, a link, /System/Volumes/Data/…): as written, where .. after a link is the link's own
  # parent, and as given, where it's the parent of where the link leads, once it's there to follow.
  own_kumi="$(normal "$KUMI_HOME")"
  # shellcheck disable=SC3013 # -ef isn't in POSIX, but dash, bash, zsh, ksh and busybox sh all have it.
  if [ "$own_kumi" = / ] || [ "$own_kumi" -ef / ] || [ "$KUMI_HOME" -ef / ]; then
    fail "KUMI_HOME is /, the root of the disk; Kumi needs a folder of its own, such as $(shown "$HOME/.kumi")."
  elif [ "$own_kumi" -ef "$HOME" ] || [ "$KUMI_HOME" -ef "$HOME" ]; then
    fail "KUMI_HOME is your home folder; Kumi needs a folder of its own, such as $(shown "$HOME/.kumi")."
  elif mounted "$own_kumi" || mounted "$KUMI_HOME"; then
    fail "KUMI_HOME is $(shown "$own_kumi"), where a disk is mounted; Kumi needs a folder of its own there, such as $(shown "$own_kumi")/kumi."
  fi
  if [ ! -d "$KUMI_HOME" ]; then made_home=1; fi
  mkdir -p "$KUMI_HOME" || fail "couldn't make $KUMI_HOME."
  free_kb="$(df -Pk "$KUMI_HOME" 2>/dev/null | awk 'NR==2 {print $4}')"
  if [ -n "$free_kb" ] && [ "$free_kb" -lt 300000 ] 2>/dev/null; then
    fail "there's only $((free_kb / 1024)) MB free; Kumi needs about 300 MB. Free some space and run this again."
  fi
  work="$(mktemp -d "$KUMI_HOME/.install.XXXXXX")" || { work=""; fail "couldn't make a working folder in $KUMI_HOME."; }

  # ── Which Kumi ─────────────────────────────────────────────────────────
  if [ -n "$tty" ]; then printf '  %s' "${dim}Looking up ${version:+Kumi }${version:-the newest Kumi}…${reset}"; open=1; fi
  fetch "$base/kumi-release-$target.json" "$work/release.json"
  if [ "$code" = 404 ]; then
    # Nothing there for this computer: is there a release at all?
    if [ -n "${KUMI_RELEASES:-}" ]; then fail "there's no Kumi release for this computer ($target) at $base."; fi
    fetch "$base/kumi-release.json" "$work/any.json"
    if fetched && ! html "$work/any.json"; then
      fail "${version:+Kumi }${version:-the newest Kumi} has no build for this computer ($target). The releases are at $releases"
    fi
    if [ -n "$version" ]; then fail "there's no Kumi $version. The releases are at $releases"; fi
    fail "there's no Kumi release to install yet. Try again later."
  fi
  if ! fetched; then unfetched "$base"; fail "$message" "$said"; fi
  if html "$work/release.json"; then fail "$captive"; fi
  field() { sed -n "s/.*\"$1\" *: *\"\\([^\"]*\\)\".*/\\1/p" "$work/release.json" | head -n 1; }
  kumi_version="$(field kumi)"; bundle="$(field bundle)"; bundle_sha="$(field sha256)"
  [ -n "$kumi_version" ] || fail "what came back from ${base#*://} wasn't a Kumi release."
  [ "$(field runtime)" = "rust-native" ] && [ "$(field target)" = "$target" ] || fail "the release doesn't match this computer."
  printf '%s\n' "$kumi_version" | grep -Eq '^[0-9]+\.[0-9]+\.[0-9]+(-[A-Za-z0-9_.]+)?$' || fail "the release version didn't make sense."
  printf '%s\n' "$bundle" | grep -Eq '^[A-Za-z0-9_.-]+\.tar\.gz$' || fail "the release archive name didn't make sense."
  printf '%s\n' "$bundle_sha" | grep -Eq '^[0-9a-f]{64}$' || fail "the release checksum didn't make sense."
  # The same bundle xz-compressed, a third smaller, for a tar that reads xz: bsdtar with liblzma (macOS's), or GNU tar
  # with xz beside it. Every other tar, and a release from before 1.24, gets the gzipped one.
  archive="$work/kumi.tar.gz"; unpack="-xzf"
  bundle_xz="$(field bundle_xz)"; bundle_xz_sha="$(field sha256_xz)"
  if printf '%s\n' "$bundle_xz" | grep -Eq '^[A-Za-z0-9_.-]+\.tar\.xz$' && printf '%s\n' "$bundle_xz_sha" | grep -Eq '^[0-9a-f]{64}$'; then
    if tar --version 2>/dev/null | grep -q liblzma || command -v xz >/dev/null 2>&1; then
      bundle="$bundle_xz"; bundle_sha="$bundle_xz_sha"; archive="$work/kumi.tar.xz"; unpack="-xf"
    fi
  fi
  # The header, in place of the line above: the wordmark, and the version at the right of the 60 columns
  # after the 2-column margin, as in Kumi's own setup.
  if [ -n "$tty" ]; then printf '\r\033[K'; fi
  pad=$((56 - ${#kumi_version})); [ "$pad" -gt 0 ] || pad=1
  say "$(printf "  %s%${pad}s%s" "${bold}kumi${reset}" "" "${dim}${kumi_version}${reset}")"; say ""

  # ── Kumi ───────────────────────────────────────────────────────────────
  begin "Download"
  # Curl's bar takes the line below the step's, and goes once it's done.
  if [ -n "$bar" ]; then printf '\n'; open=""; blank=""; fetch "$base/$bundle" "$archive" bar; else fetch "$base/$bundle" "$archive"; fi
  if ! fetched; then unfetched "$base/$bundle"; fail "$message" "$said"; fi
  if [ -n "$bar" ]; then printf '\033[2A\r\033[J'; begin "Download"; fi
  if [ "$(sha "$archive")" != "$bundle_sha" ]; then
    if html "$archive"; then fail "$captive"; fi
    fail "Kumi's download didn't match its checksum, so it wasn't used. Try again."
  fi
  finish "checked against its checksum"
  begin "Install"
  if ! { mkdir -p "$work/app" && tar $unpack "$archive" -C "$work/app"; }; then fail "couldn't unpack Kumi."; fi
  # Live's bridge comes as the lifecycle's own tarball under app/bridge; its package is unpacked beside it, where Kumi
  # runs it from and the lifecycle copies it into Live. (A release from before carries the package itself.)
  for artifact in "$work/app/bridge/"*.tar.gz; do
    [ -f "$artifact" ] || continue
    tar -xzf "$artifact" -C "$work/app/bridge" || fail "couldn't unpack Live's bridge."
  done
  KUMI_INSTALLED=1 KUMI_HOME="$KUMI_HOME" "$work/app/kumi" --version >/dev/null 2>"$work/start.err"; started=$?
  if [ "$started" -ne 0 ]; then
    said="$(sed '/^[[:space:]]*$/d' "$work/start.err" | tail -n 1 | cut -c1-300)"
    if [ -z "$said" ] && [ "$started" -gt 128 ]; then said="it was stopped by signal $((started - 128))"; fi
    fail "the downloaded Kumi didn't start${said:+ ($said)}. Please report this at github.com/user1303836/kumi/issues."
  fi

  # The new Kumi goes in app, and the one that was there in app.previous, where kumi update --rollback finds
  # it, as kumi update does. Ctrl-C waits until it's in: these are renames, and nothing is deleted until then
  # (what goes waits in the working folder).
  trap '' INT TERM HUP
  app="$KUMI_HOME/app"; previous="$app.previous"; older="$previous.old"
  # A whole Kumi: a native one, or an earlier Node one.
  has_kumi() { [ -x "$1/kumi" ] || [ -f "$1/apps/kumi/bin/kumi.mjs" ]; }
  # Moves a whole Kumi back from where it waited; if it can't go, the working folder keeps it, and stays.
  restore() { mv "$1" "$2" || work=""; }
  # A run that gave up earlier (this one's, or kumi update's) can leave the only whole Kumi in app.previous, or
  # the one before it in app.previous.old: it goes back first, and is never what gets cleared.
  for kept in "$previous" "$older"; do
    if ! has_kumi "$app" && has_kumi "$kept"; then
      if [ -e "$app" ]; then mv "$app" "$work/part.1" || fail "couldn't put Kumi in place."; fi
      mv "$kept" "$app" || fail "couldn't put Kumi in place."
    fi
  done
  if [ ! -e "$previous" ] && has_kumi "$older"; then mv "$older" "$previous"; fi
  if [ -e "$older" ] && { has_kumi "$app" || has_kumi "$previous"; }; then mv "$older" "$work/older"; fi
  installed=""
  if has_kumi "$app"; then installed="$(sed -n 's/.*"version" *: *"\([^"]*\)".*/\1/p' "$app/package.json" 2>/dev/null | head -n 1)"; fi
  aside=""; before=""
  if has_kumi "$app" && [ "$installed" = "$kumi_version" ] && has_kumi "$previous"; then
    # The same Kumi again (a repair): the new copy replaces it, and the one to roll back to stays.
    aside="$work/replaced"
  elif has_kumi "$app"; then
    # The one before waits aside until the new one is in.
    if [ -e "$previous" ]; then before="$work/before"; mv "$previous" "$before" || fail "couldn't put Kumi in place."; fi
    aside="$previous"
  elif [ -e "$app" ]; then
    # Part of a Kumi, none whole: the new one replaces it, and app.previous stays as it is.
    mv "$app" "$work/part.2" || fail "couldn't put Kumi in place."
  fi
  held="$work"
  if [ -n "$aside" ] && ! mv "$app" "$aside"; then
    if [ -n "$before" ]; then restore "$before" "$previous"; fi
    if [ -z "$work" ]; then fail "couldn't put Kumi in place; the Kumi you had still works, and the one before it is in $held."; fi
    fail "couldn't put Kumi in place; the Kumi you had still works."
  fi
  if ! mv "$work/app" "$app"; then
    # Whatever went in part of the way goes, and the Kumi that was there goes back.
    if [ -n "$aside" ]; then rm -rf "$app"; restore "$aside" "$app"; fi
    if [ -n "$before" ]; then restore "$before" "$previous"; fi
    if [ -z "$work" ]; then fail "couldn't put Kumi in place, nor put back the Kumi you had: it's in $held."; fi
    fail "couldn't put Kumi in place${aside:+; the Kumi you had still works}."
  fi
  made_home=""
  # What changed: an update, the same Kumi again, or an older one in place of a newer.
  if [ -z "$installed" ]; then outcome=""
  elif [ "$installed" = "$kumi_version" ]; then outcome="reinstalled $kumi_version"
  elif older "$kumi_version" "$installed"; then outcome="replaced $installed with older $kumi_version"
  else outcome="${installed} → ${kumi_version}"; fi
  finish "$(shown "$KUMI_HOME")${outcome:+, $outcome}"

  # ── The kumi command ───────────────────────────────────────────────────
  # Kumi writes the same one (crates/kumi/src/install/migration.rs), and a test keeps the two the same.
  mkdir -p "$KUMI_HOME/bin" || fail "couldn't make $KUMI_HOME/bin."
  cat > "$work/launcher" <<'LAUNCHER'
#!/bin/sh
KUMI_HOME="${KUMI_HOME:-$(cd "$(dirname "$0")/.." && pwd)}"
export KUMI_HOME KUMI_INSTALLED=1
if [ -x "$KUMI_HOME/app/kumi" ]; then
  exec "$KUMI_HOME/app/kumi" "$@"
fi
# Rolled back to a JavaScript release: it runs on its own Node.
if [ -x "$KUMI_HOME/node/bin/node" ] && [ -f "$KUMI_HOME/app/apps/kumi/bin/kumi.mjs" ]; then
  exec "$KUMI_HOME/node/bin/node" "$KUMI_HOME/app/apps/kumi/bin/kumi.mjs" "$@"
fi
if [ -x "$KUMI_HOME/app.previous/kumi" ]; then
  echo "Kumi's app is missing, so the Kumi before it starts. To repair Kumi, run the installer again: curl -fsSL https://github.com/user1303836/kumi/releases/latest/download/install.sh | sh" >&2
  exec "$KUMI_HOME/app.previous/kumi" "$@"
fi
echo "Kumi's app is missing: run the installer again: curl -fsSL https://github.com/user1303836/kumi/releases/latest/download/install.sh | sh" >&2
exit 1
LAUNCHER
  if ! { chmod 755 "$work/launcher" && mv -f "$work/launcher" "$KUMI_HOME/bin/kumi"; }; then fail "couldn't write the kumi command in $KUMI_HOME/bin."; fi

  # ── PATH ───────────────────────────────────────────────────────────────
  begin "PATH"
  bin="$KUMI_HOME/bin"; marker="# Added by the Kumi installer"; added=""; ready=""; unwritten=""
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
    # Once per file: the marker with this Kumi's bin on the line after it, as written or spelled from $HOME or ~
    # (someone may have edited it), as kumi uninstall finds it. One for a Kumi elsewhere doesn't count.
    # A line an earlier installer wrote with // in it (from a HOME ending in /) counts too.
    rel="${bin#"${HOME%/}"/}"; [ "$rel" != "$bin" ] || rel=""
    if awk -v marker="$marker" -v bin="$bin" -v rel="$rel" '
      { line = $0; gsub(/\/\/+/, "/", line) }
      index(after, marker) && (index(line, bin) || (rel != "" && (index(line, "$HOME/" rel) || index(line, "${HOME}/" rel) || index(line, "~/" rel)))) { found = 1 }
      { after = $0 } END { exit !found }' "$rc" 2>/dev/null; then ready=1
    elif mkdir -p "$(dirname "$rc")" 2>/dev/null && printf '\n%s\n%s\n' "$marker" "$line" 2>/dev/null >> "$rc"; then added="$rc"; ready=1
    else unwritten="$rc"; fi
  fi
  if [ -n "$added" ]; then finish "added in $(shown "$added"), for new terminal windows"
  elif [ -n "$unwritten" ] && [ -z "$on_path" ]; then
    if [ -n "$tty" ]; then printf '\r\033[K'; fi
    say "$(printf '  %-20s%s' "PATH" "${dim}left as it is: $(shown "$unwritten") couldn't be written${reset}")"
  elif [ -n "${KUMI_NO_MODIFY_PATH:-}" ] && [ -z "$on_path" ]; then
    if [ -n "$tty" ]; then printf '\r\033[K'; fi
    say "$(printf '  %-20s%s' "PATH" "${dim}left as it is (KUMI_NO_MODIFY_PATH)${reset}")"
  else finish; fi

  # ── Done ───────────────────────────────────────────────────────────────
  # What the new Kumi replaced goes now, before Ctrl-C works again.
  tidy; work=""
  trap - INT TERM HUP
  say ""
  say "  ${dim}Update with: kumi update · Remove with: kumi uninstall${reset}"
  say ""
  # Kumi starts here and now: it signs you in and connects to Live. Its keyboard is the terminal's, since
  # this script's own input is the download. Its farewell says how to start it next time, when this
  # window's PATH doesn't have kumi yet: new windows' do (new-window), or nothing changed PATH (no-path).
  if [ -n "$on_path" ]; then KUMI_FROM_INSTALLER=1; elif [ -n "$ready" ]; then KUMI_FROM_INSTALLER=new-window; else KUMI_FROM_INSTALLER=no-path; fi
  if [ -z "${KUMI_NO_LAUNCH:-}" ] && [ -z "${CI:-}" ] && [ -t 1 ] && { : </dev/tty; } 2>/dev/null; then
    say "  Starting Kumi…"
    trap - EXIT
    export KUMI_FROM_INSTALLER
    exec "$bin/kumi" </dev/tty
  fi
  case "$KUMI_FROM_INSTALLER" in
    1) say "  Next: kumi" ;;
    new-window) say "  Next, in a new terminal window (or after: export PATH=\"$bin:\$PATH\"): kumi" ;;
    *) say "  Next: add $(shown "$bin") to PATH, or run $(typed "$bin/kumi")" ;;
  esac
  say "  ${dim}It signs you in and connects to Live.${reset}"
}

main "$@"
