param([switch] $SkipTests)

$ErrorActionPreference = 'Stop'
$repoRoot = Split-Path -Parent $PSScriptRoot
. (Join-Path $PSScriptRoot 'msvc-env.ps1')

if (-not $SkipTests) {
    & (Join-Path $PSScriptRoot 'test.ps1')
    if ($LASTEXITCODE -ne 0) { throw 'test suite failed' }
}

Push-Location (Join-Path $repoRoot 'apps\ipad-web')
try {
    npm ci
    if ($LASTEXITCODE -ne 0) { throw 'npm ci failed' }
    npm run build
    if ($LASTEXITCODE -ne 0) { throw 'frontend build failed' }
}
finally {
    Pop-Location
}

Push-Location $repoRoot
try {
    cargo build --locked --release -p nfidb -p pointer-sink
    if ($LASTEXITCODE -ne 0) { throw 'release build failed' }

    $packageRoot = Join-Path $repoRoot 'build\packages'
    $stage = Join-Path $packageRoot 'NFiDB-windows-x64'
    $archive = Join-Path $packageRoot 'NFiDB-windows-x64.zip'
    $checksum = "$archive.sha256"
    if (Test-Path -LiteralPath $stage) { Remove-Item -Recurse -Force -LiteralPath $stage }
    New-Item -ItemType Directory -Force -Path (Join-Path $stage 'docs') | Out-Null

    Copy-Item -LiteralPath (Join-Path $repoRoot 'target\release\nfidb.exe') -Destination $stage
    Copy-Item -LiteralPath (Join-Path $repoRoot 'target\release\pointer-sink.exe') -Destination $stage
    Copy-Item -LiteralPath (Join-Path $repoRoot 'README.md'), (Join-Path $repoRoot 'CHANGELOG.md'), (Join-Path $repoRoot 'LICENSE-MIT'), (Join-Path $repoRoot 'LICENSE-APACHE'), (Join-Path $repoRoot 'THIRD_PARTY.md') -Destination $stage
    Copy-Item -LiteralPath (Join-Path $repoRoot 'docs\SECURITY.md'), (Join-Path $repoRoot 'docs\KNOWN_ISSUES.md'), (Join-Path $repoRoot 'docs\TEST_MATRIX.md'), (Join-Path $repoRoot 'docs\PERFORMANCE.md'), (Join-Path $repoRoot 'docs\PROTOCOL.md') -Destination (Join-Path $stage 'docs')

    Compress-Archive -Path (Join-Path $stage '*') -DestinationPath $archive -CompressionLevel Optimal -Force
    $hash = (Get-FileHash -Algorithm SHA256 -LiteralPath $archive).Hash.ToLowerInvariant()
    Set-Content -LiteralPath $checksum -Value "$hash  NFiDB-windows-x64.zip" -Encoding ascii
    Write-Host "Created $archive"
    Write-Host "SHA-256 $hash"
}
finally {
    Pop-Location
}
