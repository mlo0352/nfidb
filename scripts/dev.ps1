param(
    [Parameter(ValueFromRemainingArguments = $true)]
    [string[]] $HostArgs
)

$ErrorActionPreference = 'Stop'
$repoRoot = Split-Path -Parent $PSScriptRoot
. (Join-Path $PSScriptRoot 'msvc-env.ps1')

Push-Location (Join-Path $repoRoot 'apps\ipad-web')
try {
    npm install
    if ($LASTEXITCODE -ne 0) { throw 'npm install failed' }
    npm run build
    if ($LASTEXITCODE -ne 0) { throw 'browser build failed' }
}
finally {
    Pop-Location
}

Push-Location $repoRoot
try {
    cargo run -p nfidb -- @HostArgs
    if ($LASTEXITCODE -ne 0) { throw 'NFiDB exited with an error' }
}
finally {
    Pop-Location
}
