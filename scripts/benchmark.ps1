param(
    [ValidateRange(5, 900)] [int] $DurationSeconds = 10,
    [switch] $SkipBuild,
    [switch] $Quick,
    [string] $ScenarioName = ''
)

$ErrorActionPreference = 'Stop'
$repoRoot = Split-Path -Parent $PSScriptRoot
. (Join-Path $PSScriptRoot 'msvc-env.ps1')

if (-not $SkipBuild) {
    Push-Location (Join-Path $repoRoot 'apps\ipad-web')
    try {
        npm ci
        if ($LASTEXITCODE -ne 0) { throw 'npm ci failed' }
        npm run build
        if ($LASTEXITCODE -ne 0) { throw 'browser build failed' }
    }
    finally {
        Pop-Location
    }
    Push-Location $repoRoot
    try {
        cargo build --locked --release -p nfidb -p pointer-sink
        if ($LASTEXITCODE -ne 0) { throw 'release build failed' }
    }
    finally {
        Pop-Location
    }
}

$stamp = Get-Date -Format 'yyyyMMdd-HHmmss'
$reportRoot = Join-Path $repoRoot "build\benchmarks\$stamp"
New-Item -ItemType Directory -Force -Path $reportRoot | Out-Null

$scenarios = @(
    [pscustomobject]@{ Name = '4k-to-720p-fast'; Width = 3840; Height = 2160; Profile = 'fast'; OutputWidth = 1280; OutputHeight = 720; Viewport = '3840x2160' },
    [pscustomobject]@{ Name = '1080p-balanced'; Width = 1920; Height = 1080; Profile = 'balanced'; OutputWidth = 1920; OutputHeight = 1080; Viewport = '1920x1080' },
    [pscustomobject]@{ Name = '4k-to-1080p-balanced'; Width = 3840; Height = 2160; Profile = 'balanced'; OutputWidth = 1920; OutputHeight = 1080; Viewport = '3840x2160' },
    [pscustomobject]@{ Name = '4k-to-1440p-sharp'; Width = 3840; Height = 2160; Profile = 'sharp'; OutputWidth = 2560; OutputHeight = 1440; Viewport = '3840x2160' }
)
if ($Quick) {
    $scenarios = @($scenarios | Where-Object Name -eq '1080p-balanced')
}
if ($ScenarioName) {
    $scenarios = @($scenarios | Where-Object Name -eq $ScenarioName)
    if ($scenarios.Count -eq 0) { throw "Unknown benchmark scenario: $ScenarioName" }
}

$results = @()
$hostSeconds = $DurationSeconds + 30
$port = 48920
foreach ($case in $scenarios) {
    Write-Host "Benchmarking $($case.Name)..."
    $sessionPath = Join-Path $reportRoot "$($case.Name)-session.json"
    $hostMetricsPath = Join-Path $reportRoot "$($case.Name)-host.json"
    $browserReportPath = Join-Path $reportRoot "$($case.Name)-browser.json"
    $arguments = @(
        '--headless', '--capture', 'test-pattern', '--input-sink', 'log', '--no-mdns',
        '--port', "$port", '--video-profile', $case.Profile, '--max-fps', '60',
        '--test-width', "$($case.Width)", '--test-height', "$($case.Height)",
        '--run-seconds', "$hostSeconds", '--session-info', $sessionPath, '--metrics-output', $hostMetricsPath
    )
    $hostProcess = Start-Process -FilePath (Join-Path $repoRoot 'target\release\nfidb.exe') -ArgumentList $arguments -PassThru -WindowStyle Hidden
    try {
        $deadline = (Get-Date).AddSeconds(20)
        while (-not (Test-Path -LiteralPath $sessionPath)) {
            if ($hostProcess.HasExited) { throw "NFiDB exited before $($case.Name) started" }
            if ((Get-Date) -ge $deadline) { throw "NFiDB did not write session info for $($case.Name)" }
            Start-Sleep -Milliseconds 100
        }
        $session = Get-Content -Raw -LiteralPath $sessionPath | ConvertFrom-Json
        $savedEnvironment = @{
            NFIDB_E2E_URL = $env:NFIDB_E2E_URL
            NFIDB_E2E_PIN = $env:NFIDB_E2E_PIN
            NFIDB_E2E_DURATION_MS = $env:NFIDB_E2E_DURATION_MS
            NFIDB_E2E_EXPECT_WIDTH = $env:NFIDB_E2E_EXPECT_WIDTH
            NFIDB_E2E_EXPECT_HEIGHT = $env:NFIDB_E2E_EXPECT_HEIGHT
            NFIDB_E2E_VIEWPORT = $env:NFIDB_E2E_VIEWPORT
            NFIDB_E2E_REPORT = $env:NFIDB_E2E_REPORT
        }
        try {
            $env:NFIDB_E2E_URL = $session.url
            $env:NFIDB_E2E_PIN = $session.pin
            $env:NFIDB_E2E_DURATION_MS = "$($DurationSeconds * 1000)"
            $env:NFIDB_E2E_EXPECT_WIDTH = "$($case.OutputWidth)"
            $env:NFIDB_E2E_EXPECT_HEIGHT = "$($case.OutputHeight)"
            $env:NFIDB_E2E_VIEWPORT = $case.Viewport
            $env:NFIDB_E2E_REPORT = $browserReportPath
            Push-Location (Join-Path $repoRoot 'apps\ipad-web')
            try {
                npx playwright test e2e/live-host.spec.ts
                if ($LASTEXITCODE -ne 0) { throw "Playwright failed for $($case.Name)" }
            }
            finally {
                Pop-Location
            }
        }
        finally {
            foreach ($name in $savedEnvironment.Keys) {
                Set-Item -Path "env:$name" -Value $savedEnvironment[$name]
            }
        }
        Wait-Process -Id $hostProcess.Id -Timeout ($hostSeconds + 5)
        $hostMetrics = Get-Content -Raw -LiteralPath $hostMetricsPath | ConvertFrom-Json
        $browserMetrics = Get-Content -Raw -LiteralPath $browserReportPath | ConvertFrom-Json
        $results += [pscustomobject]@{
            name = $case.Name
            source = "$($case.Width)x$($case.Height)"
            output = "$($case.OutputWidth)x$($case.OutputHeight)"
            viewport = $case.Viewport
            duration_seconds = $DurationSeconds
            host = $hostMetrics
            browser = $browserMetrics
        }
    }
    finally {
        if (-not $hostProcess.HasExited) {
            Stop-Process -Id $hostProcess.Id -Force
        }
    }
    $port += 1
}

$summaryPath = Join-Path $reportRoot 'summary.json'
$results | ConvertTo-Json -Depth 12 | Set-Content -LiteralPath $summaryPath -Encoding utf8
Copy-Item -LiteralPath $summaryPath -Destination (Join-Path $repoRoot 'build\benchmarks\latest.json') -Force
Write-Host "Benchmark report: $summaryPath"
