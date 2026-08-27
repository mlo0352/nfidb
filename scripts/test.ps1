$ErrorActionPreference = 'Stop'
$repoRoot = Split-Path -Parent $PSScriptRoot
. (Join-Path $PSScriptRoot 'msvc-env.ps1')

function Invoke-NativeValidation {
    param(
        [Parameter(Mandatory)] [string] $FailureMessage,
        [Parameter(Mandatory)] [scriptblock] $Command
    )
    # Windows PowerShell 5.1 wraps native stderr as ErrorRecord instances.
    # Cargo writes normal progress and every compiler diagnostic to stderr, so
    # normalize those records while preserving the real native exit code. This
    # keeps validation transcripts useful instead of reducing a failure to the
    # generic message below.
    $previousErrorAction = $ErrorActionPreference
    $ErrorActionPreference = 'Continue'
    try {
        & $Command 2>&1 | ForEach-Object {
            if ($_ -is [System.Management.Automation.ErrorRecord]) {
                $_.Exception.Message
            }
            else {
                $_
            }
        }
        $exitCode = $LASTEXITCODE
    }
    finally {
        $ErrorActionPreference = $previousErrorAction
    }
    if ($exitCode -ne 0) {
        throw "$FailureMessage (exit code $exitCode)"
    }
}

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
    Invoke-NativeValidation 'cargo fmt check failed' { cargo fmt --all -- --check }
    Invoke-NativeValidation 'cargo check failed' { cargo check --workspace --all-targets --locked }
    Invoke-NativeValidation 'cargo clippy failed' {
        cargo clippy --workspace --all-targets --all-features --locked -- -D warnings
    }
    Invoke-NativeValidation 'cargo test failed' { cargo test --workspace --locked }
}
finally {
    if ($null -eq $previousCargoColor) { Remove-Item Env:CARGO_TERM_COLOR -ErrorAction SilentlyContinue } else { $env:CARGO_TERM_COLOR = $previousCargoColor }
    if ($null -eq $previousCargoProgress) { Remove-Item Env:CARGO_TERM_PROGRESS_WHEN -ErrorAction SilentlyContinue } else { $env:CARGO_TERM_PROGRESS_WHEN = $previousCargoProgress }
    Pop-Location
}
