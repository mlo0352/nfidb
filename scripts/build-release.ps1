param(
    [switch] $SkipTests,
    [switch] $SkipFrontend,
    [switch] $SkipBuild
)

$ErrorActionPreference = 'Stop'
$repoRoot = Split-Path -Parent $PSScriptRoot
. (Join-Path $PSScriptRoot 'msvc-env.ps1')

function New-PortableZip {
    param(
        [Parameter(Mandatory = $true)]
        [string] $SourceDirectory,
        [Parameter(Mandatory = $true)]
        [string] $DestinationPath,
        [int] $OpenAttempts = 12
    )

    Add-Type -AssemblyName System.IO.Compression
    $sourceRoot = [IO.Path]::GetFullPath($SourceDirectory)
    $separator = [IO.Path]::DirectorySeparatorChar.ToString()
    if (-not $sourceRoot.EndsWith($separator)) {
        $sourceRoot += $separator
    }

    $archiveStream = [IO.File]::Open(
        $DestinationPath,
        [IO.FileMode]::CreateNew,
        [IO.FileAccess]::Write,
        [IO.FileShare]::None
    )
    $zip = $null
    try {
        $zip = [IO.Compression.ZipArchive]::new(
            $archiveStream,
            [IO.Compression.ZipArchiveMode]::Create,
            $false
        )
        $sourceFiles = Get-ChildItem -LiteralPath $SourceDirectory -File -Recurse | Sort-Object FullName
        foreach ($sourceFile in $sourceFiles) {
            $sourceStream = $null
            for ($attempt = 1; $attempt -le $OpenAttempts; $attempt++) {
                try {
                    # ReadWrite/Delete sharing is safe for immutable staged files and
                    # cooperates with antivirus/indexers. An exclusive scanner lock is
                    # retried quietly rather than surfacing Compress-Archive internals.
                    $readSharing = [IO.FileShare]::ReadWrite -bor [IO.FileShare]::Delete
                    $sourceStream = [IO.File]::Open(
                        $sourceFile.FullName,
                        [IO.FileMode]::Open,
                        [IO.FileAccess]::Read,
                        $readSharing
                    )
                    break
                }
                catch [IO.IOException] {
                    if ($attempt -eq $OpenAttempts) {
                        throw [IO.IOException]::new(
                            "Package source remained busy after $OpenAttempts attempts: $($sourceFile.FullName)",
                            $_.Exception
                        )
                    }
                    Start-Sleep -Milliseconds ([Math]::Min(2500, 250 * $attempt))
                }
            }

            try {
                $relativePath = $sourceFile.FullName.Substring($sourceRoot.Length).Replace('\', '/')
                $entry = $zip.CreateEntry($relativePath, [IO.Compression.CompressionLevel]::Optimal)
                $entry.LastWriteTime = [DateTimeOffset]$sourceFile.LastWriteTime
                $entryStream = $entry.Open()
                try {
                    $sourceStream.CopyTo($entryStream)
                }
                finally {
                    $entryStream.Dispose()
                }
            }
            finally {
                if ($sourceStream) {
                    $sourceStream.Dispose()
                }
            }
        }
    }
    finally {
        if ($zip) {
            $zip.Dispose()
        }
        else {
            $archiveStream.Dispose()
        }
    }
}

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
    $checksum = "$archive.sha256"
    New-Item -ItemType Directory -Force -Path (Join-Path $stage 'docs') | Out-Null

    Copy-Item -LiteralPath $hostExecutable -Destination $stage
    Copy-Item -LiteralPath $pointerExecutable -Destination $stage
    Copy-Item -LiteralPath (Join-Path $repoRoot 'README.md'), (Join-Path $repoRoot 'CHANGELOG.md'), (Join-Path $repoRoot 'LICENSE-MIT'), (Join-Path $repoRoot 'LICENSE-APACHE'), (Join-Path $repoRoot 'THIRD_PARTY.md') -Destination $stage
    Copy-Item -LiteralPath (Join-Path $repoRoot 'docs\SECURITY.md'), (Join-Path $repoRoot 'docs\ARCHITECTURE.md'), (Join-Path $repoRoot 'docs\KNOWN_ISSUES.md'), (Join-Path $repoRoot 'docs\TEST_MATRIX.md'), (Join-Path $repoRoot 'docs\PERFORMANCE.md'), (Join-Path $repoRoot 'docs\CODEC_BENCHMARKS.md'), (Join-Path $repoRoot 'docs\PROTOCOL.md') -Destination (Join-Path $stage 'docs')

    $temporaryArchive = Join-Path $packageRoot ".NFiDB-windows-x64-$packageId.zip"
    New-PortableZip -SourceDirectory $stage -DestinationPath $temporaryArchive
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
    if ($temporaryArchive -and (Test-Path -LiteralPath $temporaryArchive)) {
        Remove-Item -Force -LiteralPath $temporaryArchive -ErrorAction SilentlyContinue
    }
}
