# Local gate matching .github/workflows/verify.yml. No GUI is opened and no
# input is synthesized; see docs/SMOKE.md for the manual desktop checks.
param([switch]$Gui)
$ErrorActionPreference = 'Stop'
Set-Location (Join-Path $PSScriptRoot '..')

function Get-GitOutput {
    param([string[]]$Arguments)
    $output = & git @Arguments
    if ($LASTEXITCODE -ne 0) {
        throw "git $($Arguments -join ' ') failed with exit code $LASTEXITCODE"
    }
    return ($output -join "`n").Trim()
}

function Invoke-Cargo {
    param([string[]]$Arguments)
    & cargo @Arguments
    if ($LASTEXITCODE -ne 0) {
        throw "cargo $($Arguments -join ' ') failed with exit code $LASTEXITCODE"
    }
}

# The release binary prints the version derived from HEAD at build time.
$commitDate = Get-GitOutput @('show', '-s', '--format=%cs', 'HEAD')
$shortHash = Get-GitOutput @('rev-parse', '--short=7', 'HEAD')
if ($commitDate -notmatch '^\d{4}-\d{2}-\d{2}$' -or $shortHash -notmatch '^[0-9a-f]{7}$') {
    throw "cannot derive a release version from HEAD: $commitDate $shortHash"
}
$expectedVersion = 'v' + $commitDate.Substring(2, 2) + '-' + $commitDate.Substring(5, 2) + '-' + $shortHash
Write-Host "expected release version: $expectedVersion"

$shim = Join-Path $env:LOCALAPPDATA 'Temp\opencode\windres-shim'
if (Test-Path $shim) {
    $env:PATH = "$shim;$env:PATH"
}

Write-Host '== format =='
Invoke-Cargo @('fmt', '--all', '--', '--check')
Write-Host '== clippy =='
Invoke-Cargo @('clippy', '--workspace', '--all-targets', '--locked', '--', '-D', 'warnings')
Write-Host '== tests =='
Invoke-Cargo @('test', '--workspace', '--all-targets', '--features', 'shell', '--locked')
Write-Host '== performance counters =='
Invoke-Cargo @('test', '--release', '--test', 'm6', '--features', 'shell', '--locked', '--', '--nocapture')
Write-Host '== release build =='
Invoke-Cargo @('build', '--release', '--features', 'shell', '--locked')

$exe = Join-Path (Get-Location) 'target\release\pain.exe'
$versionLine = (& $exe --version | Select-Object -First 1).Trim()
if ($LASTEXITCODE -ne 0) {
    throw "pain --version failed with exit code $LASTEXITCODE"
}
if ($versionLine -notlike "pain $expectedVersion *") {
    throw "release version mismatch: expected '$expectedVersion' in '$versionLine'"
}
Write-Host "version: $versionLine"

if ($Gui) {
    Write-Host '== opt-in window check =='
    & (Join-Path $PSScriptRoot 'smoke.ps1') -Gui
    if ($LASTEXITCODE -ne 0) {
        throw "smoke.ps1 failed with exit code $LASTEXITCODE"
    }
} else {
    Write-Host 'Skipping GUI launch (pass -Gui when you are not using the desktop).'
}

Write-Host 'CI OK.'
