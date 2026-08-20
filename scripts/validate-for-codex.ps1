param(
    [switch] $SkipRelease,
    [switch] $SkipSmoke,
    [switch] $ResumeAfterBuild,
    [int] $PortableSmokePort = 49120
)

$ErrorActionPreference = 'Stop'
$repoRoot = Split-Path -Parent $PSScriptRoot
$started = Get-Date
$stamp = $started.ToString('yyyyMMdd-HHmmss')
$resultRoot = Join-Path $repoRoot "build\user-validation\$stamp"
$resultPath = Join-Path $resultRoot 'result.json'
$latestPath = Join-Path $repoRoot 'build\user-validation\latest.json'
$logPath = Join-Path $resultRoot 'validation.log'
$steps = [Collections.Generic.List[object]]::new()
$failure = $null

New-Item -ItemType Directory -Force -Path $resultRoot | Out-Null

function Invoke-ValidationStep {
    param(
        [Parameter(Mandatory)] [string] $Name,
        [Parameter(Mandatory)] [scriptblock] $Action
    )
    $stepStarted = Get-Date
    Write-Host "`n== $Name ==" -ForegroundColor Cyan
    try {
        & $Action
        if ($LASTEXITCODE -ne 0) {
            throw "$Name exited with code $LASTEXITCODE"
        }
        $steps.Add([ordered]@{
            name = $Name
            status = 'passed'
            duration_seconds = [Math]::Round(((Get-Date) - $stepStarted).TotalSeconds, 2)
            error = $null
        })
    }
    catch {
        $steps.Add([ordered]@{
            name = $Name
            status = 'failed'
            duration_seconds = [Math]::Round(((Get-Date) - $stepStarted).TotalSeconds, 2)
            error = $_.Exception.Message
        })
        throw
    }
}

Push-Location $repoRoot
try {
    Start-Transcript -LiteralPath $logPath -Force | Out-Null
    try {
        if (-not $ResumeAfterBuild) {
            Invoke-ValidationStep 'Complete source validation' {
                & (Join-Path $PSScriptRoot 'test.ps1')
            }
        }

        if (-not $SkipRelease) {
            Invoke-ValidationStep 'Portable release build' {
                if ($ResumeAfterBuild) {
                    & (Join-Path $PSScriptRoot 'build-release.ps1') -SkipTests -SkipFrontend -SkipBuild
                } else {
                    & (Join-Path $PSScriptRoot 'build-release.ps1') -SkipTests
                }
            }
        }

        if (-not $SkipSmoke) {
            $archive = Join-Path $repoRoot 'build\packages\NFiDB-windows-x64.zip'
            $pointerSink = Join-Path $repoRoot 'target\release\pointer-sink.exe'
            $releaseHost = Join-Path $repoRoot 'target\release\nfidb.exe'
            if (-not (Test-Path -LiteralPath $archive -PathType Leaf)) {
                throw "Release archive is missing: $archive"
            }
            if (-not (Test-Path -LiteralPath $pointerSink -PathType Leaf)) {
                throw "Release pointer sink is missing: $pointerSink"
            }
            if (-not (Test-Path -LiteralPath $releaseHost -PathType Leaf)) {
                throw "Release host is missing: $releaseHost"
            }
            Invoke-ValidationStep 'Native pointer self-test' {
                & $pointerSink --self-test
            }
            Invoke-ValidationStep 'Downloaded-style portable smoke' {
                & (Join-Path $PSScriptRoot 'portable-smoke.ps1') -Archive $archive -Port $PortableSmokePort
            }
            Invoke-ValidationStep 'Bidirectional file-transfer smoke' {
                & (Join-Path $PSScriptRoot 'file-transfer-smoke.ps1') -SkipBuild -ExecutablePath $releaseHost
            }
        }
    }
    catch {
        $failure = $_.Exception.Message
    }
    finally {
        try { Stop-Transcript | Out-Null } catch { }
    }
}
finally {
    Pop-Location
}

$archivePath = Join-Path $repoRoot 'build\packages\NFiDB-windows-x64.zip'
$archiveHash = if (Test-Path -LiteralPath $archivePath -PathType Leaf) {
    (Get-FileHash -LiteralPath $archivePath -Algorithm SHA256).Hash
} else {
    $null
}
$gitCommit = (& git -C $repoRoot rev-parse HEAD 2>$null)
$gitBranch = (& git -C $repoRoot branch --show-current 2>$null)
$gitStatus = @(& git -C $repoRoot status --porcelain=v1 2>$null)
$treeFingerprint = (& git -C $repoRoot diff --binary HEAD | git hash-object --stdin)
$completed = Get-Date
$result = [ordered]@{
    schema_version = 1
    status = if ($failure) { 'failed' } else { 'passed' }
    started_at = $started.ToString('o')
    completed_at = $completed.ToString('o')
    duration_seconds = [Math]::Round(($completed - $started).TotalSeconds, 2)
    git = [ordered]@{
        commit = "$gitCommit".Trim()
        branch = "$gitBranch".Trim()
        dirty = $gitStatus.Count -gt 0
        status = $gitStatus
        tree_fingerprint = "$treeFingerprint".Trim()
    }
    environment = [ordered]@{
        windows = [Environment]::OSVersion.VersionString
        powershell = $PSVersionTable.PSVersion.ToString()
        machine = $env:COMPUTERNAME
    }
    release = [ordered]@{
        archive = if (Test-Path -LiteralPath $archivePath) { $archivePath } else { $null }
        sha256 = $archiveHash
    }
    steps = $steps
    log = $logPath
    error = $failure
}
$json = $result | ConvertTo-Json -Depth 8
Set-Content -LiteralPath $resultPath -Value $json -Encoding utf8
New-Item -ItemType Directory -Force -Path (Split-Path -Parent $latestPath) | Out-Null
Set-Content -LiteralPath $latestPath -Value $json -Encoding utf8

if ($failure) {
    Write-Host "`nValidation failed. Tell Codex to resume; it can inspect:" -ForegroundColor Red
    Write-Host $latestPath
    exit 1
}

Write-Host "`nValidation passed. Tell Codex: resume" -ForegroundColor Green
Write-Host "Result: $latestPath"
Write-Host "Log:    $logPath"
