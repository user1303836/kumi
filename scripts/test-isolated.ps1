# Runs `cargo test` in a home of its own (the Windows side of scripts/test-isolated.sh): HOME, USERPROFILE,
# APPDATA, LOCALAPPDATA, XDG_CONFIG_HOME and KUMI_HOME point into a fresh temporary folder, so no test can
# reach this machine's Live folders, Remote Scripts or ~/.kumi through a default it forgot to override.
# Cargo and rustup keep their own folders, named before the profile moves. The arguments are `cargo test`'s,
# or `cargo nextest run`'s with KUMI_NEXTEST=1; then KUMI_NEXTEST_LIST names a file for the tests the run selects
# (`cargo nextest list`'s JSON), written first, to compare one run's tests with another's.
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
  # A profile that names its test binaries (`binary(…)` in its default-filter, .config/nextest.toml) builds only those:
  # nextest builds every binary it could run, whatever the filter, and runs the ones the filter leaves.
  [string[]]$only = @()
  if ($env:KUMI_NEXTEST_BUILD_PROFILE) {
    $config = Get-Content -Raw (Join-Path $PSScriptRoot '..\.config\nextest.toml')
    $section = [regex]::Match($config, '(?ms)^\[profile\.' + [regex]::Escape($env:KUMI_NEXTEST_BUILD_PROFILE) + '\]\s*$(.*?)(?=^\[|\z)').Groups[1].Value
    $only = [regex]::Matches($section, 'binary\(([a-z0-9_]+)\)') | ForEach-Object { '--test'; $_.Groups[1].Value }
  }
  if ($env:KUMI_NEXTEST -eq '1') {
    # The whole workspace, or the crates named (-p).
    [string[]]$scope = @('--workspace')
    foreach ($arg in $args) { if ($arg -ceq '-p' -or $arg -clike '-p?*' -or $arg -ceq '--package' -or $arg -clike '--package=*') { $scope = @() } }
    if ($env:KUMI_NEXTEST_LIST) {
      $listed = & cargo nextest list @scope --locked @only @args --message-format json
      if ($LASTEXITCODE -ne 0) { exit $LASTEXITCODE }
      # UTF-8 without a BOM, at the path as PowerShell resolves it (.NET's working folder can differ).
      [IO.File]::WriteAllLines($ExecutionContext.SessionState.Path.GetUnresolvedProviderPathFromPSPath($env:KUMI_NEXTEST_LIST), [string[]]$listed)
    }
    & cargo nextest run @scope --locked @only @args
  } else { & cargo test --workspace --locked @args }
  if ($LASTEXITCODE -ne 0) { exit $LASTEXITCODE }
} finally {
  Remove-Item -Recurse -Force $home_ -ErrorAction SilentlyContinue
}
