# Kumi installer for Windows. Open PowerShell (Start menu → type "PowerShell") and run:
#
#   irm https://raw.githubusercontent.com/user1303836/kumi/main/install.ps1 | iex
#
# It puts native Kumi in %USERPROFILE%\.kumi (no admin rights), adds its folder to your
# PATH, and checks every download against its published checksum. Running it again updates or repairs
# Kumi. Your settings, sign-ins and conversations in .kumi are never touched.
#
# Settings (optional, as environment variables): KUMI_HOME, KUMI_VERSION (e.g. 1.1.0), KUMI_RELEASES,
# KUMI_NO_MODIFY_PATH=1.

& {
  Set-StrictMode -Version 2
  $ErrorActionPreference = 'Stop'
  $ProgressPreference = 'SilentlyContinue'   # the progress bar makes downloads many times slower in Windows PowerShell
  try { [Net.ServicePointManager]::SecurityProtocol = [Net.ServicePointManager]::SecurityProtocol -bor [Net.SecurityProtocolType]::Tls12 } catch { }

  function Say([string]$Text = '') { Write-Host $Text }
  # Plain lines, as in Kumi's own setup: the steps aligned, "done" in mint (green where the console has no
  # true colour). No colour with NO_COLOR, or when the output isn't a console.
  $interactive = -not [Console]::IsOutputRedirected
  $colour = $interactive -and -not $env:NO_COLOR
  $esc = [char]27
  $trueColour = $colour -and ($env:WT_SESSION -or $env:COLORTERM -in @('truecolor', '24bit'))
  $step = @{ Name = '' }
  function Doing([string]$Name) {
    $step.Name = $Name
    if ($interactive) { Write-Host ('  ' + $Name.PadRight(20) + '…') -NoNewline -ForegroundColor DarkGray }
  }
  function Done([string]$Detail = '') {
    if ($interactive) { Write-Host "`r" -NoNewline }
    Write-Host ('  ' + $step.Name.PadRight(20)) -NoNewline
    if ($trueColour) { Write-Host "$esc[38;2;134;227;181mdone$esc[0m" -NoNewline }
    elseif ($colour) { Write-Host 'done' -NoNewline -ForegroundColor Green }
    else { Write-Host 'done' -NoNewline }
    if ($Detail) { Write-Host " · $Detail" -ForegroundColor DarkGray } else { Write-Host ' ' }
  }
  function Fail([string]$Text) { Write-Host ''; Write-Host "Kumi couldn't be installed: $Text" -ForegroundColor Red; throw 'KumiInstallFailed' }

  # 'ok', 'missing' (the server answered 404: nothing there to get) or 'failed' (no answer, after three tries).
  function Fetch([string]$Url, [string]$File) {
    for ($attempt = 1; $attempt -le 3; $attempt++) {
      try { Invoke-WebRequest -Uri $Url -OutFile $File -UseBasicParsing -TimeoutSec 600; return 'ok' }
      catch {
        $response = $_.Exception.Response
        if ($response -and [int]$response.StatusCode -eq 404) { return 'missing' }
        if ($attempt -eq 3) { return 'failed' }
        Start-Sleep -Seconds 2
      }
    }
  }
  # Windows tells programs (Explorer, new windows) that the environment changed only when asked to.
  function Send-EnvironmentChange {
    try {
      if (-not ('Kumi.Env' -as [Type])) {
        Add-Type -Namespace Kumi -Name Env -MemberDefinition '[DllImport("user32.dll", CharSet = CharSet.Auto)] public static extern System.IntPtr SendMessageTimeout(System.IntPtr hWnd, uint msg, System.UIntPtr wParam, string lParam, uint flags, uint timeout, out System.UIntPtr result);'
      }
      $result = [UIntPtr]::Zero
      [void][Kumi.Env]::SendMessageTimeout([IntPtr]0xffff, 0x1a, [UIntPtr]::Zero, 'Environment', 2, 5000, [ref]$result)
    } catch { }
  }
  function Sha([string]$File) { (Get-FileHash -Algorithm SHA256 -LiteralPath $File).Hash.ToLowerInvariant() }
  # A folder holds a Kumi when the launcher can start it: native, or an earlier Node one.
  function HasKumi([string]$Folder) {
    (Test-Path -LiteralPath (Join-Path $Folder 'kumi.exe')) -or (Test-Path -LiteralPath (Join-Path $Folder 'apps\kumi\bin\kumi.mjs'))
  }
  # Puts the Kumi that was there back in its place, unless the place already holds one: it can be empty,
  # or hold part of a move. Tries for a few seconds; says whether the place holds a Kumi.
  function PutBack([string]$Target, [string]$Previous) {
    for ($attempt = 1; $attempt -le 5; $attempt++) {
      if (HasKumi $Target) { return $true }
      if (-not (HasKumi $Previous)) { return $false }
      try {
        if (Test-Path -LiteralPath $Target) { Remove-Item -LiteralPath $Target -Recurse -Force }
        Rename-Item -LiteralPath $Previous -NewName (Split-Path $Target -Leaf)
      } catch { Start-Sleep -Seconds 1 }
    }
    HasKumi $Target
  }
  function Swap([string]$Fresh, [string]$Target) {
    # Antivirus can hold new files for a moment, and a Kumi window holds its own: try a few times before
    # giving up, and whenever the new one can't go in, the one that was there stays usable.
    $previous = "$Target.previous"
    # A run that gave up earlier can leave the only whole Kumi in app.previous: it goes back first, and
    # it's never what gets cleared.
    if (-not (HasKumi $Target)) { [void](PutBack $Target $previous) }
    if ((Test-Path -LiteralPath $previous) -and ((HasKumi $Target) -or -not (HasKumi $previous))) { Remove-Item -LiteralPath $previous -Recurse -Force }
    for ($attempt = 1; $attempt -le 5; $attempt++) {
      try {
        if (Test-Path -LiteralPath $Target) { Rename-Item -LiteralPath $Target -NewName (Split-Path $previous -Leaf) }
        Move-Item -LiteralPath $Fresh -Destination $Target
        return
      } catch {
        # A move that went through before its error counts.
        if (-not (Test-Path -LiteralPath $Fresh) -and (HasKumi $Target)) { return }
        $kept = PutBack $Target $previous
        if ($attempt -eq 5) {
          $still = if ($kept) { ' The Kumi you had still works.' } else { '' }
          Fail "Windows kept $Target busy: a Kumi window, or an antivirus scan of the new files. Close every Kumi window, give the scan a minute, then run this again.$still"
        }
        Start-Sleep -Seconds 2
      }
    }
  }

  try {
    $KumiHome = if ($env:KUMI_HOME) { $env:KUMI_HOME } else { Join-Path $env:USERPROFILE '.kumi' }
    $base = if ($env:KUMI_RELEASES) { $env:KUMI_RELEASES.TrimEnd('/') }
      elseif ($env:KUMI_VERSION) { "https://github.com/user1303836/kumi/releases/download/v$($env:KUMI_VERSION.TrimStart('v'))" }
      else { 'https://github.com/user1303836/kumi/releases/latest/download' }

    Say ''

    # ── What this computer is ────────────────────────────────────────────
    if ([Environment]::OSVersion.Version.Major -lt 10) { Fail 'Kumi needs Windows 10 or 11.' }
    $cpu = if ($env:PROCESSOR_ARCHITEW6432) { $env:PROCESSOR_ARCHITEW6432 } else { $env:PROCESSOR_ARCHITECTURE }
    $arch = switch ($cpu) { 'AMD64' { 'x86_64' } 'ARM64' { 'aarch64' } default { Fail "this processor ($cpu) isn't supported by this Kumi release." } }
    $target = "$arch-pc-windows-msvc"
    $tar = Join-Path $env:SystemRoot 'System32\tar.exe'
    if (-not (Test-Path -LiteralPath $tar)) { Fail 'this Windows is missing tar.exe (Windows 10 from 2018 on has it). Update Windows, then run this again.' }

    # ── Room and a place to work ─────────────────────────────────────────
    New-Item -ItemType Directory -Force -Path $KumiHome | Out-Null
    $drive = (Get-Item -LiteralPath $KumiHome).PSDrive
    if ($drive -and $drive.Free -and $drive.Free -lt 300MB) { Fail "there's only $([math]::Floor($drive.Free / 1MB)) MB free; Kumi needs about 300 MB. Free some space and run this again." }
    $work = Join-Path $KumiHome ('.install.' + [Guid]::NewGuid().ToString('N').Substring(0, 8))
    New-Item -ItemType Directory -Force -Path $work | Out-Null

    try {
      # ── Which Kumi ─────────────────────────────────────────────────────
      $manifestFile = Join-Path $work 'release.json'
      switch (Fetch "$base/kumi-release-$target.json" $manifestFile) {
        'missing' { Fail "there's no Kumi release to install at $($base -replace '^https://', '') yet. Try again later." }
        'failed' { Fail "couldn't reach GitHub ($base). Check your internet connection and try again." }
      }
      $release = Get-Content -Raw -LiteralPath $manifestFile | ConvertFrom-Json
      if ($release.runtime -ne 'rust-native' -or $release.target -ne $target -or
          $release.kumi -notmatch '^\d+\.\d+\.\d+(?:-[A-Za-z0-9_.]+)?$' -or
          $release.bundle -notmatch '^[A-Za-z0-9_.-]+\.tar\.gz$' -or
          $release.sha256 -cnotmatch '^[0-9a-f]{64}$') { Fail "the release description didn't match this computer." }
      # The header: the wordmark, and the version at the right of the 60 columns after the 2-column
      # margin, as in Kumi's own setup.
      Write-Host '  kumi' -NoNewline -ForegroundColor White
      Write-Host (' ' * [Math]::Max(1, 56 - $release.kumi.Length)) -NoNewline
      Write-Host $release.kumi -ForegroundColor DarkGray
      Say ''

      # ── Kumi ───────────────────────────────────────────────────────────
      Doing 'Download'
      $bundle = Join-Path $work 'kumi.tar.gz'
      if ((Fetch "$base/$($release.bundle)" $bundle) -ne 'ok') { Fail "couldn't download Kumi from GitHub." }
      if ((Sha $bundle) -ne $release.sha256) { Fail "Kumi's download didn't match its checksum, so it wasn't used. Try again." }
      Done 'checked against its checksum'
      Doing 'Install'
      $freshApp = Join-Path $work 'app'
      New-Item -ItemType Directory -Force -Path $freshApp | Out-Null
      & $tar -xzf $bundle -C $freshApp
      if ($LASTEXITCODE -ne 0) { Fail "couldn't unpack Kumi." }
      $hadHome = $env:KUMI_HOME
      $env:KUMI_INSTALLED = '1'; $env:KUMI_HOME = $KumiHome
      & (Join-Path $freshApp 'kumi.exe') --version | Out-Null
      $started = ($LASTEXITCODE -eq 0)
      Remove-Item Env:KUMI_INSTALLED -ErrorAction SilentlyContinue
      if ($hadHome) { $env:KUMI_HOME = $hadHome } else { Remove-Item Env:KUMI_HOME -ErrorAction SilentlyContinue }
      if (-not $started) { Fail 'the downloaded Kumi didn''t start. Please report this at github.com/user1303836/kumi/issues.' }
      Swap $freshApp (Join-Path $KumiHome 'app')
      Done $KumiHome
    } finally {
      Remove-Item -LiteralPath $work -Recurse -Force -ErrorAction SilentlyContinue
    }

    # ── The kumi command ─────────────────────────────────────────────────
    $bin = Join-Path $KumiHome 'bin'
    New-Item -ItemType Directory -Force -Path $bin | Out-Null
    $launcher = @'
@echo off
goto start
::::::::::::::::::::::::::::::::::::::::::::::::::::::::::::::::
::::::::::::::::::::::::::::::::::::::::::::::::::::::::::::::::
::::::::::::::::::::::::::::::::::::::::::::::::::::::::::::::::
::::::::::::::::::::::::::::::::::::::::::::::::::::::::::::::::
::::::::::::::::::::::::::::::::::::::::::::::::::::::::::::::::
exit /b %errorlevel%
:start
rem Kumi's launcher, written by its installer. cmd reads a running batch file from where it stopped:
rem one replaced while it runs resumes in the colons above (labels) and exits.
setlocal
if not defined KUMI_HOME for %%I in ("%~dp0..") do set "KUMI_HOME=%%~fI"
set "KUMI_INSTALLED=1"
if exist "%KUMI_HOME%\app\kumi.exe" goto native
"%KUMI_HOME%\node\node.exe" "%KUMI_HOME%\app\apps\kumi\bin\kumi.mjs" %*
exit /b %errorlevel%
:native
"%KUMI_HOME%\app\kumi.exe" %*
exit /b %errorlevel%
'@
    # Keep the bytes identical to the native launcher's template, independent of
    # the installer script's checkout/download line endings.
    $launcher = ($launcher -replace '\r?\n', "`r`n") + "`r`n"
    [IO.File]::WriteAllText((Join-Path $bin 'kumi.cmd'), $launcher, [Text.Encoding]::ASCII)

    # ── PATH ─────────────────────────────────────────────────────────────
    Doing 'PATH'
    $added = $false
    if (-not $env:KUMI_NO_MODIFY_PATH) {
      # The user PATH as the registry keeps it: %VAR% entries stay unexpanded, and it's written back as a
      # REG_EXPAND_SZ, so they keep working (the .NET round trip would store them expanded, for good).
      $key = (Get-Item -LiteralPath 'HKCU:\').OpenSubKey('Environment', $true)
      $userPath = [string]$key.GetValue('Path', '', 'DoNotExpandEnvironmentNames')
      $parts = @(); if ($userPath) { $parts = $userPath.Split(';') | Where-Object { $_ } }
      if (-not ($parts | Where-Object { $_.TrimEnd('\') -ieq $bin })) {
        $key.SetValue('Path', ((@($bin) + $parts) -join ';'), [Microsoft.Win32.RegistryValueKind]::ExpandString)
        Send-EnvironmentChange
        $added = $true
      }
      $key.Close()
    }
    # This window too, so kumi works right away.
    if (-not (($env:Path.Split(';')) | Where-Object { $_.TrimEnd('\') -ieq $bin })) { $env:Path = "$bin;$env:Path" }

    if ($added) { Done "added $bin, for new windows" }
    elseif ($env:KUMI_NO_MODIFY_PATH) {
      if ($interactive) { Write-Host "`r" -NoNewline }
      Write-Host ('  ' + 'PATH'.PadRight(20)) -NoNewline
      Write-Host 'left as it is in new windows (KUMI_NO_MODIFY_PATH)' -ForegroundColor DarkGray
    }
    else { Done }

    # ── Done ─────────────────────────────────────────────────────────────
    Say ''
    Write-Host '  Kumi looks best in Windows Terminal (from the Microsoft Store, built into Windows 11).' -ForegroundColor DarkGray
    Write-Host '  Update with: kumi update · Remove with: kumi uninstall' -ForegroundColor DarkGray
    Say ''
    # Kumi starts here and now: it signs you in and connects to Live.
    if (-not $env:KUMI_NO_LAUNCH -and -not $env:CI -and [Environment]::UserInteractive -and -not [Console]::IsInputRedirected -and -not [Console]::IsOutputRedirected) {
      Say '  Starting Kumi…'
      & (Join-Path $bin 'kumi.cmd')
    } else {
      Say '  Next (in this window, or any new one): kumi'
      Write-Host '  It signs you in and connects to Live.' -ForegroundColor DarkGray
    }
  } catch {
    if ($_.Exception.Message -ne 'KumiInstallFailed') {
      Write-Host ''
      Write-Host "Kumi couldn't be installed: $($_.Exception.Message)" -ForegroundColor Red
      Write-Host 'If this keeps happening, please report it at github.com/user1303836/kumi/issues.' -ForegroundColor DarkGray
    }
  }
}
