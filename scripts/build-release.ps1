param(
    [switch] $SkipTests,
    [switch] $SkipFrontend,
    [switch] $SkipBuild
)

$ErrorActionPreference = 'Stop'
$repoRoot = Split-Path -Parent $PSScriptRoot
. (Join-Path $PSScriptRoot 'msvc-env.ps1')

if (-not $SkipTests) {
    & (Join-Path $PSScriptRoot 'test.ps1')
    if ($LASTEXITCODE -ne 0) { throw 'test suite failed' }
}

if (-not $SkipFrontend) {
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
}

$stageRoot = $null
$temporaryArchive = $null
Push-Location $repoRoot
try {
    if (-not $SkipBuild) {
        cargo build --locked --release -p nfidb -p pointer-sink
        if ($LASTEXITCODE -ne 0) { throw 'release build failed' }
    }

    $hostExecutable = Join-Path $repoRoot 'target\release\nfidb.exe'
    $pointerExecutable = Join-Path $repoRoot 'target\release\pointer-sink.exe'
    if (-not (Test-Path -LiteralPath $hostExecutable -PathType Leaf)) {
        throw "release host executable is missing: $hostExecutable"
    }
    if (-not (Test-Path -LiteralPath $pointerExecutable -PathType Leaf)) {
        throw "release pointer sink is missing: $pointerExecutable"
    }

    $packageRoot = Join-Path $repoRoot 'build\packages'
    New-Item -ItemType Directory -Force -Path $packageRoot | Out-Null
    $packageId = [guid]::NewGuid().ToString('N')
    $stageRoot = Join-Path $packageRoot ".staging-$packageId"
    $stage = Join-Path $stageRoot 'NFiDB-windows-x64'
    $archive = Join-Path $packageRoot 'NFiDB-windows-x64.zip'
    $temporaryArchive = Join-Path $packageRoot ".NFiDB-windows-x64-$packageId.zip"
    $checksum = "$archive.sha256"
    New-Item -ItemType Directory -Force -Path (Join-Path $stage 'docs') | Out-Null

    Copy-Item -LiteralPath $hostExecutable -Destination $stage
    Copy-Item -LiteralPath $pointerExecutable -Destination $stage
    Copy-Item -LiteralPath (Join-Path $repoRoot 'README.md'), (Join-Path $repoRoot 'CHANGELOG.md'), (Join-Path $repoRoot 'LICENSE-MIT'), (Join-Path $repoRoot 'LICENSE-APACHE'), (Join-Path $repoRoot 'THIRD_PARTY.md') -Destination $stage
    Copy-Item -LiteralPath (Join-Path $repoRoot 'docs\SECURITY.md'), (Join-Path $repoRoot 'docs\ARCHITECTURE.md'), (Join-Path $repoRoot 'docs\KNOWN_ISSUES.md'), (Join-Path $repoRoot 'docs\TEST_MATRIX.md'), (Join-Path $repoRoot 'docs\PERFORMANCE.md'), (Join-Path $repoRoot 'docs\CODEC_BENCHMARKS.md'), (Join-Path $repoRoot 'docs\PROTOCOL.md') -Destination (Join-Path $stage 'docs')

    Compress-Archive -Path (Join-Path $stage '*') -DestinationPath $temporaryArchive -CompressionLevel Optimal
    Move-Item -LiteralPath $temporaryArchive -Destination $archive -Force
    $temporaryArchive = $null
    $hash = (Get-FileHash -Algorithm SHA256 -LiteralPath $archive).Hash.ToLowerInvariant()
    Set-Content -LiteralPath $checksum -Value "$hash  NFiDB-windows-x64.zip" -Encoding ascii
    Write-Host "Created $archive"
    Write-Host "SHA-256 $hash"
}
finally {
    Pop-Location
    if ($stageRoot -and (Test-Path -LiteralPath $stageRoot)) {
        Remove-Item -Recurse -Force -LiteralPath $stageRoot
    }
    if ($temporaryArchive -and (Test-Path -LiteralPath $temporaryArchive)) {
        Remove-Item -Force -LiteralPath $temporaryArchive
    }
}
