# Runs `cargo test` in a home of its own (the Windows side of scripts/test-isolated.sh): HOME, USERPROFILE,
# APPDATA, LOCALAPPDATA, XDG_CONFIG_HOME and KUMI_HOME point into a fresh temporary folder, so no test can
# reach this machine's Live folders, Remote Scripts or ~/.kumi through a default it forgot to override.
# Cargo and rustup keep their own folders, named before the profile moves. The arguments are `cargo test`'s,
# or `cargo nextest run`'s with KUMI_NEXTEST=1.
$ErrorActionPreference = 'Stop'
if (-not $env:CARGO_HOME) { $env:CARGO_HOME = Join-Path $env:USERPROFILE '.cargo' }
if (-not $env:RUSTUP_HOME) { $env:RUSTUP_HOME = Join-Path $env:USERPROFILE '.rustup' }
$home_ = Join-Path ([IO.Path]::GetTempPath()) ("kumi-test-home-" + [IO.Path]::GetRandomFileName())
New-Item -ItemType Directory -Path $home_ | Out-Null
try {
  $env:HOME = $home_; $env:USERPROFILE = $home_
  $env:APPDATA = Join-Path $home_ 'AppData\Roaming'; $env:LOCALAPPDATA = Join-Path $home_ 'AppData\Local'
  $env:XDG_CONFIG_HOME = Join-Path $home_ '.config'; $env:KUMI_HOME = Join-Path $home_ '.kumi'
  Remove-Item Env:KUMI_REMOTE_SCRIPTS_DIR -ErrorAction SilentlyContinue
  Remove-Item Env:KUMI_LIVE_EXTENSIONS_DIR -ErrorAction SilentlyContinue
  if ($env:KUMI_NEXTEST -eq '1') { & cargo nextest run --workspace --locked @args } else { & cargo test --workspace --locked @args }
  if ($LASTEXITCODE -ne 0) { exit $LASTEXITCODE }
} finally {
  Remove-Item -Recurse -Force $home_ -ErrorAction SilentlyContinue
}
