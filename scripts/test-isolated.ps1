# Runs the tests in a home of their own (the Windows side of scripts/test-isolated.sh): HOME, USERPROFILE,
# APPDATA, LOCALAPPDATA, XDG_CONFIG_HOME and KUMI_HOME point into a fresh temporary folder, so no test can
# reach this machine's Live folders, Remote Scripts or ~/.kumi through a default it forgot to override. KUMI_PROCESSES
# stands in for the list of running programs, so no test sees a DAW open on this machine.
# Cargo and rustup keep their own folders, named before the profile moves. The tests run on cargo-nextest, each in a
# process of its own: a crate's tests are one binary, and some set the environment. The arguments are `cargo nextest
# run`'s; KUMI_NEXTEST_LIST names a file for the tests the run selects (`cargo nextest list`'s JSON), written first, to
# compare one run's tests with another's. KUMI_NEXTEST_ARCHIVE names tests built elsewhere (`cargo nextest archive`):
# they run from it, and nothing is built. KUMI_NEXTEST=0 runs `cargo test` instead, with its arguments: a crate's tests
# then share one process.
$ErrorActionPreference = 'Stop'
if (-not $env:CARGO_HOME) { $env:CARGO_HOME = Join-Path $env:USERPROFILE '.cargo' }
if (-not $env:RUSTUP_HOME) { $env:RUSTUP_HOME = Join-Path $env:USERPROFILE '.rustup' }
$home_ = Join-Path ([IO.Path]::GetTempPath()) ("kumi-test-home-" + [IO.Path]::GetRandomFileName())
New-Item -ItemType Directory -Path $home_ | Out-Null
try {
  $env:HOME = $home_; $env:USERPROFILE = $home_
  $env:APPDATA = Join-Path $home_ 'AppData\Roaming'; $env:LOCALAPPDATA = Join-Path $home_ 'AppData\Local'
  # As a profile has them: Windows' known-folder lookups refuse a profile's folders that don't exist, and the tools the
  # build runs (sccache's cache and settings) look theirs up there.
  New-Item -ItemType Directory -Force -Path $env:APPDATA, $env:LOCALAPPDATA | Out-Null
  $env:XDG_CONFIG_HOME = Join-Path $home_ '.config'; $env:KUMI_HOME = Join-Path $home_ '.kumi'
  Remove-Item Env:KUMI_REMOTE_SCRIPTS_DIR -ErrorAction SilentlyContinue
  Remove-Item Env:KUMI_LIVE_EXTENSIONS_DIR -ErrorAction SilentlyContinue
  $env:KUMI_PROCESSES = 'none'
  # A profile that names its test binaries (`binary(…)`, or a crate's `binary_id(<crate>::it)`, in its default-filter,
  # .config/nextest.toml) builds only those: nextest builds every binary it could run, whatever the filter, and runs the
  # ones the filter leaves.
  [string[]]$only = @()
  if ($env:KUMI_NEXTEST_BUILD_PROFILE) {
    $config = Get-Content -Raw (Join-Path $PSScriptRoot '..\.config\nextest.toml')
    $section = [regex]::Match($config, '(?ms)^\[profile\.' + [regex]::Escape($env:KUMI_NEXTEST_BUILD_PROFILE) + '\]\s*$(.*?)(?=^\[|\z)').Groups[1].Value
    $names = [regex]::Matches($section, 'binary\(([a-z0-9_]+)\)|binary_id\([a-z0-9_-]+::(it)\)') |
      ForEach-Object { if ($_.Groups[1].Success) { $_.Groups[1].Value } else { $_.Groups[2].Value } } | Sort-Object -Unique
    $only = $names | ForEach-Object { '--test'; $_ }
  }
  if ($env:KUMI_NEXTEST -ne '0') {
    # (Windows PowerShell 5.1 can throw on a native command's stderr under 'Stop')
    $nextest = try { & cargo nextest --version 2>$null | Out-Null; $LASTEXITCODE -eq 0 } catch { $false }
    if (-not $nextest) {
      [Console]::Error.WriteLine('test-isolated.ps1: the tests run on cargo-nextest: cargo install cargo-nextest --locked (KUMI_NEXTEST=0: cargo test)')
      exit 1
    }
    # The whole workspace, or the crates named (-p).
    [string[]]$scope = @('--workspace')
    foreach ($arg in $args) { if ($arg -ceq '-p' -or $arg -clike '-p?*' -or $arg -ceq '--package' -or $arg -clike '--package=*') { $scope = @() } }
    [string[]]$build = $scope + @('--locked') + $only
    if ($env:KUMI_NEXTEST_ARCHIVE) {
      # Unpacked once into this checkout's target\, as where it was built (so the paths the tests were built with hold:
      # the workspace's programs beside them, the checkout's files), then read there by each run.
      & cargo nextest list --archive-file $env:KUMI_NEXTEST_ARCHIVE --workspace-remap . --extract-to . --extract-overwrite | Out-Null
      if ($LASTEXITCODE -ne 0) { exit $LASTEXITCODE }
      $build = @('--binaries-metadata', 'target/nextest/binaries-metadata.json', '--cargo-metadata', 'target/nextest/cargo-metadata.json',
        '--workspace-remap', '.', '--target-dir-remap', 'target')
    }
    if ($env:KUMI_NEXTEST_LIST) {
      $listed = & cargo nextest list @build @args --message-format json
      if ($LASTEXITCODE -ne 0) { exit $LASTEXITCODE }
      # UTF-8 without a BOM, at the path as PowerShell resolves it (.NET's working folder can differ).
      [IO.File]::WriteAllLines($ExecutionContext.SessionState.Path.GetUnresolvedProviderPathFromPSPath($env:KUMI_NEXTEST_LIST), [string[]]$listed)
    }
    & cargo nextest run @build @args
  } else { & cargo test --workspace --locked @args }
  if ($LASTEXITCODE -ne 0) { exit $LASTEXITCODE }
} finally {
  Remove-Item -Recurse -Force $home_ -ErrorAction SilentlyContinue
}
