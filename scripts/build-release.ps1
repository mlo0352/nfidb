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
$temporaryArchives = [Collections.Generic.List[string]]::new()
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
    $checksum = "$archive.sha256"
    New-Item -ItemType Directory -Force -Path (Join-Path $stage 'docs') | Out-Null

    Copy-Item -LiteralPath $hostExecutable -Destination $stage
    Copy-Item -LiteralPath $pointerExecutable -Destination $stage
    Copy-Item -LiteralPath (Join-Path $repoRoot 'README.md'), (Join-Path $repoRoot 'CHANGELOG.md'), (Join-Path $repoRoot 'LICENSE-MIT'), (Join-Path $repoRoot 'LICENSE-APACHE'), (Join-Path $repoRoot 'THIRD_PARTY.md') -Destination $stage
    Copy-Item -LiteralPath (Join-Path $repoRoot 'docs\SECURITY.md'), (Join-Path $repoRoot 'docs\ARCHITECTURE.md'), (Join-Path $repoRoot 'docs\KNOWN_ISSUES.md'), (Join-Path $repoRoot 'docs\TEST_MATRIX.md'), (Join-Path $repoRoot 'docs\PERFORMANCE.md'), (Join-Path $repoRoot 'docs\CODEC_BENCHMARKS.md'), (Join-Path $repoRoot 'docs\PROTOCOL.md') -Destination (Join-Path $stage 'docs')

    $compressionSucceeded = $false
    for ($attempt = 1; $attempt -le 12; $attempt++) {
        $temporaryArchive = Join-Path $packageRoot ".NFiDB-windows-x64-$packageId-$attempt.zip"
        $temporaryArchives.Add($temporaryArchive)
        try {
            Compress-Archive -Path (Join-Path $stage '*') -DestinationPath $temporaryArchive -CompressionLevel Optimal
            $compressionSucceeded = $true
            break
        }
        catch {
            if ($attempt -eq 12) { throw }
            $delayMilliseconds = [Math]::Min(2500, 250 * $attempt)
            Write-Warning "Portable ZIP source was temporarily busy; retrying in $delayMilliseconds ms (attempt $attempt/12)."
            Start-Sleep -Milliseconds $delayMilliseconds
        }
    }
    if (-not $compressionSucceeded) { throw 'portable ZIP creation did not complete' }
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
        Remove-Item -Recurse -Force -LiteralPath $stageRoot -ErrorAction SilentlyContinue
        if (Test-Path -LiteralPath $stageRoot) {
            Write-Warning "Temporary package staging remains locked and can be removed later: $stageRoot"
        }
    }
    foreach ($candidateArchive in $temporaryArchives) {
        if (Test-Path -LiteralPath $candidateArchive) {
            Remove-Item -Force -LiteralPath $candidateArchive -ErrorAction SilentlyContinue
        }
    }
}
