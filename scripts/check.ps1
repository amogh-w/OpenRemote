# Local quality gate, same steps as CI: .\scripts\check.ps1
$ErrorActionPreference = 'Stop'
$env:PATH = "$env:USERPROFILE\.cargo\bin;$env:PATH"
Set-Location (Split-Path $PSScriptRoot)

function Step($name, [scriptblock]$run) {
    Write-Host "==> $name" -ForegroundColor Cyan
    & $run
    if ($LASTEXITCODE -ne 0) { Write-Host "FAILED: $name" -ForegroundColor Red; exit 1 }
}

Step 'cargo fmt --check'   { cargo fmt --check }
Step 'cargo clippy'        { cargo clippy --all-targets -- -D warnings }
Step 'cargo test'          { cargo test }
Step 'node --test (page vs server)' { node --test "tests/*.test.mjs" }
Step 'node --check app.js' { node --check assets/ui/app.js }
# UI rules (cuppa-ui): needs the cuppa plugin checkout, so CI without it skips this step.
$cuppa = if ($env:CUPPA_HARNESS) { $env:CUPPA_HARNESS } else { "$env:USERPROFILE\Projects\cuppa\scripts\harness.mjs" }
if (Test-Path $cuppa) { Step 'UI lint (cuppa)' { node $cuppa lint openremote } }
else { Write-Host 'skipped UI lint: cuppa plugin not found (set CUPPA_HARNESS)' -ForegroundColor Yellow }
Write-Host 'All checks passed.' -ForegroundColor Green
