#requires -Version 7
# Regression smoke check for the Windows release. Does not synthesize input.
# By default it runs headless checks only; -Gui opts into a windowed launch.
param([switch]$Gui)
$ErrorActionPreference = 'Stop'
Set-Location (Join-Path $PSScriptRoot '..')

Write-Host '== headless and integration tests =='
cargo test --workspace --all-targets --features shell

Write-Host '== performance stress counters =='
cargo test --release --test m6 --features shell -- --nocapture

Write-Host '== release build =='
cargo build --release --features shell
$exe = Join-Path (Get-Location) 'target\release\pain.exe'

if ($Gui) {
    Write-Host '== launch and close check (foregrounds a window; opt-in only) =='
    $process = Start-Process -FilePath $exe -PassThru
    Start-Sleep -Seconds 4
    $alive = Get-Process -Id $process.Id -ErrorAction SilentlyContinue
    if (-not $alive) {
        throw "pain exited immediately; check $env:LOCALAPPDATA\pain\pain.log"
    }
    Write-Host "pid=$($process.Id) title=$($alive.MainWindowTitle)"
    if ([string]::IsNullOrWhiteSpace($alive.MainWindowTitle)) {
        Stop-Process -Id $process.Id -ErrorAction SilentlyContinue
        throw 'pain opened no titled window'
    }
    Stop-Process -Id $process.Id -ErrorAction SilentlyContinue
    Start-Sleep -Milliseconds 500
    if (Get-Process -Id $process.Id -ErrorAction SilentlyContinue) {
        throw 'pain did not exit on Stop-Process'
    }
} else {
    Write-Host 'Skipping GUI launch (pass -Gui to opt in when you are not using the desktop).'
}

Write-Host 'SMOKE OK: tests, counters and release build passed.'
Write-Host "Manual checks: docs/SMOKE.md"
