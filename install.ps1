# Kumi installer for Windows. Open PowerShell (Start menu > type "PowerShell") and run:
#
#   irm https://github.com/user1303836/kumi/releases/latest/download/install.ps1 | iex
#
# It puts native Kumi in %USERPROFILE%\.kumi (no admin rights), adds its folder to your
# PATH, and checks every download against its published checksum. Running it again updates or repairs
# Kumi. Your settings, sign-ins and conversations in .kumi are never touched.
#
# Settings (optional, as environment variables): KUMI_HOME, KUMI_VERSION (e.g. 1.16.0, or latest),
# KUMI_RELEASES, KUMI_NO_MODIFY_PATH=1.
#
# The file is plain ASCII: Windows PowerShell reads a script file without a byte order mark in the
# system's code page, so what it shows that ASCII hasn't is spelled as [char]s.

& {
  Set-StrictMode -Version 2
  $ErrorActionPreference = 'Stop'
  $ProgressPreference = 'SilentlyContinue'   # the progress bar makes downloads many times slower in Windows PowerShell
  try { [Net.ServicePointManager]::SecurityProtocol = [Net.ServicePointManager]::SecurityProtocol -bor [Net.SecurityProtocolType]::Tls12 } catch { }
  # Run from a file (powershell -File install.ps1), a failure is its exit code; piped into iex, it leaves the
  # window open and sets $LASTEXITCODE.
  $fromFile = [bool]{}.File

  $ellipsis = [string][char]0x2026; $dot = [string][char]0x00B7; $arrow = [string][char]0x2192
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
    if ($interactive) { Write-Host ('  ' + $Name.PadRight(20) + $ellipsis) -NoNewline -ForegroundColor DarkGray }
  }
  function Done([string]$Detail = '') {
    if ($interactive) { Write-Host "`r" -NoNewline }
    Write-Host ('  ' + $step.Name.PadRight(20)) -NoNewline
    if ($trueColour) { Write-Host "$esc[38;2;134;227;181mdone$esc[0m" -NoNewline }
    elseif ($colour) { Write-Host 'done' -NoNewline -ForegroundColor Green }
    else { Write-Host 'done' -NoNewline }
    if ($Detail) { Write-Host " $dot $Detail" -ForegroundColor DarkGray } else { Write-Host ' ' }
  }
  # Why Kumi couldn't be installed, then what Windows said ($Detail), if anything.
  function Fail([string]$Text, [string]$Detail = '') {
    Write-Host ''; Write-Host "Kumi couldn't be installed: $Text" -ForegroundColor Red
    if ($Detail) { Write-Host "  $Detail" -ForegroundColor DarkGray }
    throw 'KumiInstallFailed'
  }
  # Whether version $A comes before $B, compared part by part as numbers (1.9.0 before 1.10.0).
  function Older([string]$A, [string]$B) {
    try { [version]($A -replace '-.*$', '') -lt [version]($B -replace '-.*$', '') } catch { $false }
  }

  # 'ok', 'missing' (the server answered 404: nothing there to get) or 'failed', after three tries, with why in
  # $fetched: the HTTP status, or what Windows said and how (a WebException's Status).
  $fetched = @{ Status = 0; Said = ''; How = '' }
  function Fetch([string]$Url, [string]$File) {
    $fetched.Status = 0; $fetched.Said = ''; $fetched.How = ''
    for ($attempt = 1; $attempt -le 3; $attempt++) {
      try { Invoke-WebRequest -Uri $Url -OutFile $File -UseBasicParsing -TimeoutSec 600; return 'ok' }
      catch {
        $exception = $_.Exception
        $response = if ($exception.PSObject.Properties['Response']) { $exception.Response } else { $null }
        $fetched.Status = if ($response) { [int]$response.StatusCode } else { 0 }
        if ($fetched.Status -eq 404) { return 'missing' }
        $words = @()
        for ($e = $exception; $e; $e = $e.InnerException) { if ($e.Message -and $words -notcontains $e.Message) { $words += $e.Message } }
        $fetched.Said = $words -join ' '
        $fetched.How = if ($exception.PSObject.Properties['Status']) { [string]$exception.Status } else { '' }
        # Asking again doesn't change a refusal.
        $refused = $fetched.Status -ge 400 -and $fetched.Status -lt 500 -and $fetched.Status -notin @(408, 429)
        if ($attempt -eq 3 -or $refused) { return 'failed' }
        Start-Sleep -Seconds 2
      }
    }
  }
  $captive = 'a Wi-Fi sign-in page or proxy answered instead of GitHub. Sign in to the Wi-Fi (open any web page), or check the proxy, then run this again.'
  # A Wi-Fi sign-in page, or a proxy's, in place of what was asked for.
  function IsPage([string]$File) {
    try {
      $stream = [IO.File]::OpenRead($File)
      try { $bytes = New-Object byte[] 512; $count = $stream.Read($bytes, 0, 512) } finally { $stream.Dispose() }
      [Text.Encoding]::ASCII.GetString($bytes, 0, $count) -match '(?i)<(!doctype|html|head|body|title|meta)'
    } catch { $false }
  }
  # The proxy settings in the environment, as NAME=value without any user name or password (with or without a
  # scheme before them).
  function Proxies {
    @(foreach ($name in 'HTTPS_PROXY', 'HTTP_PROXY', 'ALL_PROXY', 'NO_PROXY') {
      $value = [Environment]::GetEnvironmentVariable($name)
      if ($value) { "$name=$($value -replace '^([A-Za-z][A-Za-z0-9+.-]*://)?[^/]*@', '$1')" }
    }) -join ', '
  }
  # Why a download from $Url didn't come through, in Kumi's own words (DNS, TLS, proxy, timeout, offline): the HTTP
  # status when something answered, else what Windows said.
  function Unfetched([string]$Url) {
    $hostName = ([Uri]$Url).Authority
    $status = $fetched.Status
    if ($status -eq 511) { return $captive }
    if ($status -in @(401, 403)) { return "$hostName turned the download away (HTTP $status). A network that blocks GitHub does this: try another network." }
    if ($status -eq 429) { return "$hostName is limiting downloads from this network (HTTP 429). Try again in a few minutes." }
    if ($status -ge 500) { return "$hostName had a problem (HTTP $status). Try again in a few minutes." }
    if ($status -ge 400 -and $status -ne 407) { return "$hostName answered HTTP $status for $Url." }
    $said = $fetched.Said; $how = $fetched.How
    $why = if ($status -eq 407 -or $how -match 'Proxy' -or $said -match 'proxy|tunnel') {
      'the proxy didn''t let it through. Check the proxy setting, or try without the proxy.'
    } elseif ($how -match 'Trust|SecureChannel' -or $said -match 'certificate|SSL|TLS|secure channel|trust relationship') {
      'the secure connection didn''t check out (TLS). A Wi-Fi sign-in page, a school or office network that inspects secure connections, or a wrong clock does this: sign in to the Wi-Fi, try another network, or check the clock.'
    } elseif ($how -eq 'NameResolutionFailure' -or $said -match 'remote name could not be resolved|no such host|name resolution|could not resolve|name or service not known') {
      'its address couldn''t be looked up (DNS). Check that this computer is online; if it is, its DNS isn''t answering, so try another network.'
    } elseif ($how -eq 'Timeout' -or $said -match 'timed out|timeout|did not properly respond') {
      'nothing answered in time. Check the connection, then try again.'
    } else {
      'the connection failed. Check that this computer is online, or try another network.'
    }
    $proxies = Proxies
    "couldn't reach ${hostName}: $why" + $(if ($proxies) { " Proxy settings: $proxies." } else { '' })
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
  # A field of the release's description, or '' when it has none.
  function Field($Object, [string]$Name) {
    if ($Object -and $Object.PSObject.Properties[$Name]) { [string]$Object.$Name } else { '' }
  }
  # A folder holds a Kumi when the launcher can start it: native, or an earlier Node one.
  function HasKumi([string]$Folder) {
    (Test-Path -LiteralPath (Join-Path $Folder 'kumi.exe')) -or (Test-Path -LiteralPath (Join-Path $Folder 'apps\kumi\bin\kumi.mjs'))
  }
  # The version of the Kumi in a folder, or '' when there's none.
  function VersionIn([string]$Folder) {
    if (-not (HasKumi $Folder)) { return '' }
    try { Field (Get-Content -Raw -LiteralPath (Join-Path $Folder 'package.json') | ConvertFrom-Json) 'version' } catch { '' }
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
  # Puts the new Kumi in $Target, and the one that was there in $Previous: app.previous, where a rollback finds it,
  # or for a repair a folder beside it that goes once the new one is in.
  function Swap([string]$Fresh, [string]$Target, [string]$Previous = "$Target.previous") {
    # Antivirus can hold new files for a moment, and a Kumi window holds its own: try a few times before
    # giving up, and whenever the new one can't go in, the one that was there stays usable.
    # A run that gave up earlier can leave the only whole Kumi in app.previous: it goes back first, and
    # it's never what gets cleared. Clearing the one before waits out a busy folder like the rest.
    if (-not (HasKumi $Target)) { [void](PutBack $Target $previous) }
    for ($attempt = 1; $attempt -le 5; $attempt++) {
      try {
        if ((Test-Path -LiteralPath $previous) -and ((HasKumi $Target) -or -not (HasKumi $previous))) { Remove-Item -LiteralPath $previous -Recurse -Force }
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

  $failed = $false
  $madeHome = $false
  try {
    $KumiHome = if ($env:KUMI_HOME) { $env:KUMI_HOME } else { Join-Path $env:USERPROFILE '.kumi' }
    # A folder of Kumi's own: in the home folder itself or at a drive's root, the names Kumi uses (bin, downloads,
    # tools) could be the producer's own, and kumi uninstall takes those names out. . and .. are read as written.
    $own = Join-Path $env:USERPROFILE '.kumi'
    $whole = $KumiHome; $driveRoot = ''
    try {
      $whole = [IO.Path]::GetFullPath($ExecutionContext.SessionState.Path.GetUnresolvedProviderPathFromPSPath($KumiHome))
      $driveRoot = [IO.Path]::GetPathRoot($whole)
    } catch { }
    $whole = "$whole".TrimEnd('\', '/')
    $isHome = [bool]$env:USERPROFILE -and $whole -eq "$env:USERPROFILE".TrimEnd('\', '/')
    # However else it's spelled (a junction, an 8.3 name): a file put in the home folder shows through it.
    if (-not $isHome -and $env:USERPROFILE -and (Test-Path -LiteralPath $whole -PathType Container)) {
      $probe = '.kumi-home-check-' + [Guid]::NewGuid().ToString('N')
      try {
        [IO.File]::WriteAllText((Join-Path $env:USERPROFILE $probe), '')
        $isHome = Test-Path -LiteralPath (Join-Path $whole $probe)
      } catch { } finally { Remove-Item -LiteralPath (Join-Path $env:USERPROFILE $probe) -Force -ErrorAction SilentlyContinue }
    }
    if ($isHome) { Fail "KUMI_HOME is your home folder; Kumi needs a folder of its own, such as $own." }
    if ($driveRoot -and $whole -eq "$driveRoot".TrimEnd('\', '/')) { Fail "KUMI_HOME is $driveRoot, the root of a drive; Kumi needs a folder of its own, such as $own." }
    $releases = 'https://github.com/user1303836/kumi/releases'

    Say ''

    # -- What this computer is ----------------------------------------------
    if ([Environment]::OSVersion.Version.Major -lt 10) { Fail 'Kumi needs Windows 10 or 11.' }
    $cpu = if ($env:PROCESSOR_ARCHITEW6432) { $env:PROCESSOR_ARCHITEW6432 } else { $env:PROCESSOR_ARCHITECTURE }
    $arch = switch ($cpu) { 'AMD64' { 'x86_64' } 'ARM64' { 'aarch64' } default { Fail "this processor ($cpu) isn't supported by this Kumi release." } }
    $target = "$arch-pc-windows-msvc"
    $tar = Join-Path $env:SystemRoot 'System32\tar.exe'
    if (-not (Test-Path -LiteralPath $tar)) { Fail 'this Windows is missing tar.exe (Windows 10 from 2018 on has it). Update Windows, then run this again.' }

    # -- Which release ------------------------------------------------------
    # KUMI_RELEASES (a folder of releases: a mirror, or a test's), else KUMI_VERSION, else the newest.
    $version = "$env:KUMI_VERSION".Trim() -replace '^[vV]', ''
    if ($version -eq 'latest' -or $env:KUMI_RELEASES) { $version = '' }
    if ($version) {
      if ($version -notmatch '^[0-9]+\.[0-9]+\.[0-9]+(-[A-Za-z0-9_.]+)?$') { Fail "KUMI_VERSION is `"$env:KUMI_VERSION`"; it takes a release such as 1.16.0, or latest." }
      if (Older $version '1.1.0') { Fail "Kumi $version came before the installer (1.1.0). The releases are at $releases" }
      if (Older $version '1.7.6') {
        Fail "Kumi $version came before native Kumi (1.7.6): install it with that release's own script: `$env:KUMI_VERSION = '$version'; irm https://raw.githubusercontent.com/user1303836/kumi/v$version/install.ps1 | iex"
      }
    }
    $base = if ($env:KUMI_RELEASES) { $env:KUMI_RELEASES.TrimEnd('/') } elseif ($version) { "$releases/download/v$version" } else { "$releases/latest/download" }

    # -- Room and a place to work -------------------------------------------
    $madeHome = -not (Test-Path -LiteralPath $KumiHome)
    New-Item -ItemType Directory -Force -Path $KumiHome | Out-Null
    # Always a whole path: it goes in PATH, and in the kumi command.
    $KumiHome = (Get-Item -LiteralPath $KumiHome).FullName
    $drive = (Get-Item -LiteralPath $KumiHome).PSDrive
    if ($drive -and $drive.Free -and $drive.Free -lt 300MB) { Fail "there's only $([math]::Floor($drive.Free / 1MB)) MB free; Kumi needs about 300 MB. Free some space and run this again." }
    $work = Join-Path $KumiHome ('.install.' + [Guid]::NewGuid().ToString('N').Substring(0, 8))
    New-Item -ItemType Directory -Force -Path $work | Out-Null

    try {
      # -- Which Kumi -------------------------------------------------------
      if ($interactive) { Write-Host ('  Looking up ' + $(if ($version) { "Kumi $version" } else { 'the newest Kumi' }) + $ellipsis) -NoNewline -ForegroundColor DarkGray }
      $manifestFile = Join-Path $work 'release.json'
      $got = Fetch "$base/kumi-release-$target.json" $manifestFile
      if ($got -eq 'missing') {
        # Nothing there for this computer: is there a release at all?
        if ($env:KUMI_RELEASES) { Fail "there's no Kumi release for this computer ($target) at $base." }
        $any = Join-Path $work 'any.json'
        if ((Fetch "$base/kumi-release.json" $any) -eq 'ok' -and -not (IsPage $any)) {
          Fail "$(if ($version) { "Kumi $version" } else { 'the newest Kumi' }) has no build for this computer ($target). The releases are at $releases"
        }
        if ($version) { Fail "there's no Kumi $version. The releases are at $releases" }
        Fail "there's no Kumi release to install yet. Try again later."
      }
      if ($got -ne 'ok') { Fail (Unfetched $base) $fetched.Said }
      if (IsPage $manifestFile) { Fail $captive }
      try { $release = Get-Content -Raw -LiteralPath $manifestFile | ConvertFrom-Json } catch { $release = $null }
      $kumiVersion = Field $release 'kumi'
      if (-not $kumiVersion) { Fail "what came back from $(([Uri]$base).Authority) wasn't a Kumi release." }
      if ((Field $release 'runtime') -ne 'rust-native' -or (Field $release 'target') -ne $target -or
          $kumiVersion -notmatch '^\d+\.\d+\.\d+(?:-[A-Za-z0-9_.]+)?$' -or
          (Field $release 'bundle') -notmatch '^[A-Za-z0-9_.-]+\.tar\.gz$' -or
          (Field $release 'sha256') -cnotmatch '^[0-9a-f]{64}$') { Fail "the release description didn't match this computer." }
      # The header, in place of the line above: the wordmark, and the version at the right of the 60 columns
      # after the 2-column margin, as in Kumi's own setup.
      if ($interactive) { Write-Host "`r" -NoNewline }
      Write-Host '  kumi' -NoNewline -ForegroundColor White
      Write-Host (' ' * [Math]::Max(1, 56 - $kumiVersion.Length)) -NoNewline
      Write-Host $kumiVersion -ForegroundColor DarkGray
      Say ''

      # -- Kumi -------------------------------------------------------------
      Doing 'Download'
      $bundle = Join-Path $work 'kumi.tar.gz'
      if ((Fetch "$base/$($release.bundle)" $bundle) -ne 'ok') { Fail (Unfetched "$base/$($release.bundle)") $fetched.Said }
      if ((Sha $bundle) -ne $release.sha256) {
        if (IsPage $bundle) { Fail $captive }
        Fail "Kumi's download didn't match its checksum, so it wasn't used. Try again."
      }
      Done 'checked against its checksum'
      Doing 'Install'
      $freshApp = Join-Path $work 'app'
      New-Item -ItemType Directory -Force -Path $freshApp | Out-Null
      # tar.exe reads its arguments in the system's code page, which can't spell every folder name: from the work
      # folder, the paths it's given are plain ASCII.
      Push-Location -LiteralPath $work
      try { & $tar -xzf (Split-Path $bundle -Leaf) -C (Split-Path $freshApp -Leaf) } finally { Pop-Location }
      if ($LASTEXITCODE -ne 0) { Fail "couldn't unpack Kumi." }
      $exe = Join-Path $freshApp 'kumi.exe'
      if (-not (Test-Path -LiteralPath $exe)) { Fail 'the download had no kumi.exe in it. Please report this at github.com/user1303836/kumi/issues.' }
      # Live's bridge comes as the lifecycle's own tarball under app\bridge; its package is unpacked beside it, where
      # Kumi runs it from and the lifecycle copies it into Live. (A release from before carries the package itself.)
      $bridgeFolder = Join-Path $freshApp 'bridge'
      foreach ($bridgeArchive in @(Get-ChildItem -LiteralPath $bridgeFolder -Filter '*.tar.gz' -ErrorAction SilentlyContinue)) {
        Push-Location -LiteralPath $bridgeFolder
        try { & $tar -xzf $bridgeArchive.Name -C . } finally { Pop-Location }
        if ($LASTEXITCODE -ne 0) { Fail "couldn't unpack Live's bridge." }
      }
      $hadHome = $env:KUMI_HOME
      $env:KUMI_INSTALLED = '1'; $env:KUMI_HOME = $KumiHome
      # What it says on its error output is kept for when it doesn't start (Windows PowerShell would stop at its first line).
      $preference = $ErrorActionPreference; $ErrorActionPreference = 'Continue'
      try { $said = @(& $exe --version 2>&1); $code = $LASTEXITCODE }
      catch { $said = @($_.Exception.Message); $code = 1 }
      finally { $ErrorActionPreference = $preference }
      Remove-Item Env:KUMI_INSTALLED -ErrorAction SilentlyContinue
      if ($hadHome) { $env:KUMI_HOME = $hadHome } else { Remove-Item Env:KUMI_HOME -ErrorAction SilentlyContinue }
      if ($code -ne 0) {
        $why = @($said | ForEach-Object { "$_".Trim() } | Where-Object { $_ }) | Select-Object -Last 1
        if (-not $why) { $why = "exit code $code" }
        Fail "the downloaded Kumi didn't start ($why). Please report this at github.com/user1303836/kumi/issues."
      }
      $app = Join-Path $KumiHome 'app'
      # A run that gave up earlier (this one's, or kumi update's) can leave the only whole Kumi aside: in app.replaced
      # (a repair's), app.previous, or the one before it in app.previous.old. It goes back first, as kumi update does,
      # and is never what gets cleared. Once app holds a whole Kumi, what's left aside goes.
      foreach ($kept in "$app.replaced", "$app.previous", "$app.previous.old") { if (-not (HasKumi $app)) { [void](PutBack $app $kept) } }
      if (-not (Test-Path -LiteralPath "$app.previous") -and (HasKumi "$app.previous.old")) { [void](PutBack "$app.previous" "$app.previous.old") }
      if (HasKumi $app) {
        foreach ($aside in "$app.replaced", "$app.previous.old") {
          if (Test-Path -LiteralPath $aside) { Remove-Item -LiteralPath $aside -Recurse -Force -ErrorAction SilentlyContinue }
        }
      }
      $installed = VersionIn $app
      if ($installed -and $installed -eq $kumiVersion -and (HasKumi "$app.previous")) {
        # The same Kumi again (a repair): the new copy replaces it, and the one to roll back to stays.
        Swap $freshApp $app "$app.replaced"
        Remove-Item -LiteralPath "$app.replaced" -Recurse -Force -ErrorAction SilentlyContinue
      } else {
        Swap $freshApp $app
      }
      if (Test-Path -LiteralPath "$app.previous.old") { Remove-Item -LiteralPath "$app.previous.old" -Recurse -Force -ErrorAction SilentlyContinue }
      $madeHome = $false
      # What changed: an update, the same Kumi again, or an older one in place of a newer.
      $outcome = if (-not $installed) { '' }
        elseif ($installed -eq $kumiVersion) { ", reinstalled $kumiVersion" }
        elseif (Older $kumiVersion $installed) { ", replaced $installed with older $kumiVersion" }
        else { ", $installed $arrow $kumiVersion" }
      Done "$KumiHome$outcome"
    } finally {
      Remove-Item -LiteralPath $work -Recurse -Force -ErrorAction SilentlyContinue
    }

    # -- The kumi command ---------------------------------------------------
    # Kumi writes the same one (crates/kumi/src/install/migration.rs), and a test keeps the two the same.
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
goto other
rem With the line above, as many bytes as the old Node line
exit /b %errorlevel%
:native
"%KUMI_HOME%\app\kumi.exe" %*
exit /b %errorlevel%
:other
if exist "%KUMI_HOME%\node\node.exe" if exist "%KUMI_HOME%\app\apps\kumi\bin\kumi.mjs" goto node
if exist "%KUMI_HOME%\app.previous\kumi.exe" goto previous
>&2 echo Kumi's app is missing: run the installer again: irm https://github.com/user1303836/kumi/releases/latest/download/install.ps1 ^| iex
exit /b 1
:node
"%KUMI_HOME%\node\node.exe" "%KUMI_HOME%\app\apps\kumi\bin\kumi.mjs" %*
exit /b %errorlevel%
:previous
>&2 echo Kumi's app is missing, so the Kumi before it starts. To repair Kumi, run the installer again: irm https://github.com/user1303836/kumi/releases/latest/download/install.ps1 ^| iex
"%KUMI_HOME%\app.previous\kumi.exe" %*
exit /b %errorlevel%
'@
    # Keep the bytes identical to the native launcher's template, independent of
    # the installer script's checkout/download line endings.
    $launcher = ($launcher -replace '\r?\n', "`r`n") + "`r`n"
    [IO.File]::WriteAllText((Join-Path $bin 'kumi.cmd'), $launcher, [Text.Encoding]::ASCII)

    # -- PATH ---------------------------------------------------------------
    Doing 'PATH'
    $added = $false
    # The user PATH as the registry keeps it: %VAR% entries stay unexpanded, and it's written back as a
    # REG_EXPAND_SZ, so they keep working (the .NET round trip would store them expanded, for good).
    $key = (Get-Item -LiteralPath 'HKCU:\').OpenSubKey('Environment', -not $env:KUMI_NO_MODIFY_PATH)
    $userPath = if ($key) { [string]$key.GetValue('Path', '', 'DoNotExpandEnvironmentNames') } else { '' }
    $parts = @(); if ($userPath) { $parts = $userPath.Split(';') | Where-Object { $_ } }
    $newWindows = [bool]($parts | Where-Object { $_.TrimEnd('\') -ieq $bin })
    if ($key -and -not $env:KUMI_NO_MODIFY_PATH -and -not $newWindows) {
      $key.SetValue('Path', ((@($bin) + $parts) -join ';'), [Microsoft.Win32.RegistryValueKind]::ExpandString)
      Send-EnvironmentChange
      $added = $true; $newWindows = $true
    }
    if ($key) { $key.Close() }
    # This window too, so kumi works right away.
    if (-not (($env:Path.Split(';')) | Where-Object { $_.TrimEnd('\') -ieq $bin })) { $env:Path = "$bin;$env:Path" }

    if ($added) { Done "added $bin, for new windows" }
    elseif (-not $newWindows) {
      if ($interactive) { Write-Host "`r" -NoNewline }
      Write-Host ('  ' + 'PATH'.PadRight(20)) -NoNewline
      Write-Host 'left as it is in new windows (KUMI_NO_MODIFY_PATH)' -ForegroundColor DarkGray
    }
    else { Done }

    # -- Done ---------------------------------------------------------------
    Say ''
    if (-not $env:WT_SESSION) { Write-Host '  Kumi looks best in Windows Terminal (from the Microsoft Store, built into Windows 11).' -ForegroundColor DarkGray }
    Write-Host "  Update with: kumi update $dot Remove with: kumi uninstall" -ForegroundColor DarkGray
    Say ''
    # Kumi starts here and now: it signs you in and connects to Live.
    if (-not $env:KUMI_NO_LAUNCH -and -not $env:CI -and [Environment]::UserInteractive -and -not [Console]::IsInputRedirected -and -not [Console]::IsOutputRedirected) {
      if (-not $newWindows) { Write-Host "  In a new window, add $bin to PATH, or run $(Join-Path $bin 'kumi')" -ForegroundColor DarkGray }
      Say "  Starting Kumi$ellipsis"
      & (Join-Path $bin 'kumi.cmd')
    } elseif ($newWindows) {
      Say '  Next (in this window, or any new one): kumi'
      Write-Host '  It signs you in and connects to Live.' -ForegroundColor DarkGray
    } else {
      Say "  Next (in this window): kumi. In a new window, add $bin to PATH, or run $(Join-Path $bin 'kumi')"
      Write-Host '  It signs you in and connects to Live.' -ForegroundColor DarkGray
    }
  } catch {
    $failed = $true
    if ($_.Exception.Message -ne 'KumiInstallFailed') {
      Write-Host ''
      Write-Host "Kumi couldn't be installed: $($_.Exception.Message)" -ForegroundColor Red
      Write-Host 'If this keeps happening, please report it at github.com/user1303836/kumi/issues.' -ForegroundColor DarkGray
    }
    # A folder this run made goes too, while it's empty.
    if ($madeHome) { try { [IO.Directory]::Delete($KumiHome) } catch { } }
  }
  if ($failed) {
    $global:LASTEXITCODE = 1
    if ($fromFile) { exit 1 }
  }
}
