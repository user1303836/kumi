# install.ps1's Swap when Windows holds a folder: the Kumi that was there stays usable whenever the new
# one can't go in. Its own functions run against stand-ins for Rename-Item and Move-Item that fail the way
# Windows can, then against a real file held open. The Windows Install job runs this in Windows
# PowerShell 5.1:  powershell -NoProfile -File scripts/tests/install_swap.ps1
param([string]$Installer = (Join-Path (Split-Path -Parent (Split-Path -Parent $PSScriptRoot)) 'install.ps1'))
$ErrorActionPreference = 'Stop'
Set-StrictMode -Version 2

$ast = [Management.Automation.Language.Parser]::ParseFile($Installer, [ref]$null, [ref]$null)
foreach ($name in 'Fail', 'HasKumi', 'PutBack', 'Swap') {
  $found = $ast.Find({ param($node) $node -is [Management.Automation.Language.FunctionDefinitionAst] -and $node.Name -eq $name }, $true)
  if (-not $found) { throw "install.ps1 has no function $name" }
  . ([ScriptBlock]::Create($found.Extent.Text))
}

# Stand-ins, found before the cmdlets. Each scenario says how many times a step fails.
$script:moveFails = 0; $script:halfMoves = $false; $script:movedThenFailed = $false; $script:putBackFails = 0; $script:clearFails = 0
$script:said = ''
function Start-Sleep { }
function Write-Host { param([Parameter(Position = 0)]$Object, [switch]$NoNewline, $ForegroundColor) $script:said += "$Object" }
function Move-Item([string]$LiteralPath, [string]$Destination) {
  if ($script:moveFails -gt 0) {
    $script:moveFails--
    if ($script:halfMoves) {
      New-Item -ItemType Directory -Force -Path $Destination | Out-Null
      Set-Content -LiteralPath (Join-Path $Destination 'part') -Value 1
    }
    throw 'The process cannot access the file because it is being used by another process.'
  }
  Microsoft.PowerShell.Management\Move-Item -LiteralPath $LiteralPath -Destination $Destination
  if ($script:movedThenFailed) { throw 'Access to the path is denied.' }
}
function Remove-Item([string]$LiteralPath, [switch]$Recurse, [switch]$Force) {
  if ($LiteralPath.EndsWith('.previous') -and $script:clearFails -gt 0) {
    $script:clearFails--
    throw 'The process cannot access the file because it is being used by another process.'
  }
  Microsoft.PowerShell.Management\Remove-Item -LiteralPath $LiteralPath -Recurse:$Recurse -Force:$Force
}
function Rename-Item([string]$LiteralPath, [string]$NewName) {
  if ($LiteralPath.EndsWith('.previous') -and $script:putBackFails -gt 0) {
    $script:putBackFails--
    throw 'The process cannot access the file because it is being used by another process.'
  }
  Microsoft.PowerShell.Management\Rename-Item -LiteralPath $LiteralPath -NewName $NewName
}

$failures = 0
function Check([string]$What, [bool]$Ok) {
  if ($Ok) { Microsoft.PowerShell.Utility\Write-Host "ok    $What" }
  else { Microsoft.PowerShell.Utility\Write-Host "FAIL  $What"; $script:failures++ }
}
function NewHome([string]$App = 'kumi.exe') {
  $dir = Join-Path ([IO.Path]::GetTempPath()) ('kumi-swap-' + [Guid]::NewGuid().ToString('N').Substring(0, 8))
  New-Item -ItemType Directory -Force -Path (Join-Path $dir 'fresh') | Out-Null
  Set-Content -LiteralPath (Join-Path $dir 'fresh\kumi.exe') -Value 'new'
  if ($App) {
    $file = Join-Path (Join-Path $dir 'app') $App
    New-Item -ItemType Directory -Force -Path (Split-Path -Parent $file) | Out-Null
    Set-Content -LiteralPath $file -Value 'old'
  }
  $dir
}
# Swap's outcome: 'in' when the new Kumi went in, 'gave up' when it said why it couldn't.
function Run([string]$Dir) {
  $script:said = ''
  try { Swap (Join-Path $Dir 'fresh') (Join-Path $Dir 'app'); 'in' }
  catch { if ($_.Exception.Message -eq 'KumiInstallFailed') { 'gave up' } else { "error: $($_.Exception.Message)" } }
}
function Holds([string]$Dir, [string]$Which, [string]$Folder = 'app') {
  (Get-Content -LiteralPath (Join-Path (Join-Path $Dir $Folder) 'kumi.exe') -ErrorAction SilentlyContinue) -eq $Which
}

$h = NewHome; $script:moveFails = 1
Check 'a move that fails once goes through the second time' (((Run $h) -eq 'in') -and (Holds $h 'new') -and (Holds $h 'old' 'app.previous'))

$h = NewHome; $script:moveFails = 5
Check 'a move that keeps failing leaves the Kumi that was there' (((Run $h) -eq 'gave up') -and (Holds $h 'old') -and -not (Test-Path -LiteralPath (Join-Path $h 'app.previous')))
Check '... and says what holds it, and that the Kumi still works' (($script:said -match 'a Kumi window, or an antivirus scan') -and ($script:said -match 'still works'))

$h = NewHome; $script:moveFails = 5; $script:halfMoves = $true
Check 'part of a move is cleared, and the Kumi that was there put back' (((Run $h) -eq 'gave up') -and (Holds $h 'old') -and -not (Test-Path -LiteralPath (Join-Path $h 'app\part')))
$script:halfMoves = $false

$h = NewHome; $script:moveFails = 5; $script:putBackFails = 3
Check 'putting it back waits out a busy folder' (((Run $h) -eq 'gave up') -and (Holds $h 'old'))
$script:putBackFails = 0

$h = NewHome -App 'apps\kumi\bin\kumi.mjs'; $script:moveFails = 5; $script:halfMoves = $true
Check 'an earlier Node Kumi is put back too' (((Run $h) -eq 'gave up') -and (Test-Path -LiteralPath (Join-Path $h 'app\apps\kumi\bin\kumi.mjs')) -and ($script:said -match 'still works'))
$script:halfMoves = $false

$h = NewHome -App ''; $script:moveFails = 5
Check "a first install that can't go in doesn't promise a Kumi" (((Run $h) -eq 'gave up') -and ($script:said -notmatch 'still works'))

$h = NewHome -App ''
New-Item -ItemType Directory -Force -Path (Join-Path $h 'app.previous') | Out-Null
Set-Content -LiteralPath (Join-Path $h 'app.previous\kumi.exe') -Value 'old'
Check 'a run after one that gave up puts that Kumi back first' (((Run $h) -eq 'in') -and (Holds $h 'new') -and (Holds $h 'old' 'app.previous'))

$h = NewHome -App 'part'
New-Item -ItemType Directory -Force -Path (Join-Path $h 'app.previous') | Out-Null
Set-Content -LiteralPath (Join-Path $h 'app.previous\kumi.exe') -Value 'old'
$script:moveFails = 5; $script:putBackFails = 1000
Check "... and when it can't, that Kumi is never what gets cleared" (((Run $h) -eq 'gave up') -and (Holds $h 'old' 'app.previous'))
$script:moveFails = 0; $script:putBackFails = 0

$h = NewHome; $script:movedThenFailed = $true
Check 'a move that went through before its error counts' (((Run $h) -eq 'in') -and (Holds $h 'new'))
$script:movedThenFailed = $false

$h = NewHome
New-Item -ItemType Directory -Force -Path (Join-Path $h 'app.previous') | Out-Null
Set-Content -LiteralPath (Join-Path $h 'app.previous\kumi.exe') -Value 'older'
$script:clearFails = 2
Check 'a busy app.previous is cleared once it lets go, and the new Kumi goes in' (((Run $h) -eq 'in') -and (Holds $h 'new') -and (Holds $h 'old' 'app.previous'))

$h = NewHome
New-Item -ItemType Directory -Force -Path (Join-Path $h 'app.previous') | Out-Null
Set-Content -LiteralPath (Join-Path $h 'app.previous\kumi.exe') -Value 'older'
$script:clearFails = 1000
Check "... and one that stays busy gives up in words, and the Kumi still works" (((Run $h) -eq 'gave up') -and (Holds $h 'old') -and ($script:said -match 'still works'))
$script:clearFails = 0

# Windows itself: a Kumi window holds its kumi.exe while the installer runs again.
Microsoft.PowerShell.Management\Remove-Item -Path Function:\Move-Item, Function:\Rename-Item, Function:\Remove-Item
$h = NewHome
$held = [IO.File]::Open((Join-Path $h 'app\kumi.exe'), 'Open', 'Read', 'None')
try { $outcome = Run $h } finally { $held.Dispose() }
Check "a folder Windows holds: the new Kumi goes in, or the one that was there stays ($outcome)" ((($outcome -eq 'in') -and (Holds $h 'new')) -or (($outcome -eq 'gave up') -and (Holds $h 'old')))
if ($outcome -eq 'gave up') { Check 'after it lets go, the new Kumi goes in' (((Run $h) -eq 'in') -and (Holds $h 'new')) }

if ($failures) { throw "$failures check(s) failed" }
