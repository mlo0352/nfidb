$ErrorActionPreference = 'Stop'
$repoRoot = Split-Path -Parent $PSScriptRoot
. (Join-Path $PSScriptRoot 'msvc-env.ps1')

Push-Location (Join-Path $repoRoot 'apps\ipad-web')
try {
    npm ci
    if ($LASTEXITCODE -ne 0) { throw 'npm ci failed' }
    npm run typecheck
    if ($LASTEXITCODE -ne 0) { throw 'frontend typecheck failed' }
    npm test
    if ($LASTEXITCODE -ne 0) { throw 'frontend tests failed' }
    npm run build
    if ($LASTEXITCODE -ne 0) { throw 'frontend build failed' }
}
finally {
    Pop-Location
}

$previousCargoColor = [Environment]::GetEnvironmentVariable('CARGO_TERM_COLOR', 'Process')
$previousCargoProgress = [Environment]::GetEnvironmentVariable('CARGO_TERM_PROGRESS_WHEN', 'Process')
Push-Location $repoRoot
try {
    $env:CARGO_TERM_COLOR = 'never'
    $env:CARGO_TERM_PROGRESS_WHEN = 'never'
    cargo fmt --all -- --check
    if ($LASTEXITCODE -ne 0) { throw 'cargo fmt check failed' }
    cargo check --workspace --all-targets --locked
    if ($LASTEXITCODE -ne 0) { throw 'cargo check failed' }
    cargo clippy --workspace --all-targets --all-features --locked -- -D warnings
    if ($LASTEXITCODE -ne 0) { throw 'cargo clippy failed' }
    cargo test --workspace --locked
    if ($LASTEXITCODE -ne 0) { throw "cargo test failed with exit code $LASTEXITCODE" }
}
finally {
    if ($null -eq $previousCargoColor) { Remove-Item Env:CARGO_TERM_COLOR -ErrorAction SilentlyContinue } else { $env:CARGO_TERM_COLOR = $previousCargoColor }
    if ($null -eq $previousCargoProgress) { Remove-Item Env:CARGO_TERM_PROGRESS_WHEN -ErrorAction SilentlyContinue } else { $env:CARGO_TERM_PROGRESS_WHEN = $previousCargoProgress }
    Pop-Location
}
