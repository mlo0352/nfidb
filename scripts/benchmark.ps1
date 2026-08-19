param(
    [ValidateRange(5, 900)] [int] $DurationSeconds = 10,
    [switch] $SkipBuild,
    [switch] $Quick,
    [switch] $Full,
    [switch] $HostOnly,
    [ValidateSet('all', 'auto', 'h264-hardware', 'hevc-hardware', 'av1-hardware', 'h264-software')]
    [string] $Codec = 'all',
    [ValidateSet('all', 'fast', 'balanced', 'sharp')]
    [string] $Profile = 'all',
    [ValidateSet('all', 'static-detail', 'drawing', 'high-motion')]
    [string] $Workload = 'all',
    [string] $ScenarioName = '',
    [int] $Frames = 0
)

$ErrorActionPreference = 'Stop'
if ($Quick -and $Full) { throw 'Choose either -Quick or -Full.' }
if ($Frames -lt 0 -or $Frames -gt 3600 -or ($Frames -gt 0 -and $Frames -lt 10)) {
    throw '-Frames must be 0 (automatic) or between 10 and 3600.'
}
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
    finally { Pop-Location }
    Push-Location $repoRoot
    try {
        cargo build --locked --release -p nfidb -p pointer-sink
        if ($LASTEXITCODE -ne 0) { throw 'release build failed' }
    }
    finally { Pop-Location }
}

$stamp = Get-Date -Format 'yyyyMMdd-HHmmss'
$reportRoot = Join-Path $repoRoot "build\benchmarks\$stamp"
New-Item -ItemType Directory -Force -Path $reportRoot | Out-Null
$exe = Join-Path $repoRoot 'target\release\nfidb.exe'

# Level A: deterministic host encoder/preprocess comparison. Unavailable
# hardware is emitted as a skipped row by NFiDB rather than failing the suite.
$hostSuite = if ($Full) { 'full' } else { 'quick' }
$hostFrames = if ($Frames -gt 0) { $Frames } elseif ($Full) { 120 } else { 180 }
$hostReport = Join-Path $reportRoot 'host'
$hostArguments = @(
    '--benchmark', $hostSuite,
    '--benchmark-frames', "$hostFrames",
    '--benchmark-output', $hostReport
)
if ($Codec -ne 'all' -and $Codec -ne 'auto') { $hostArguments += @('--encoder', $Codec) }
if ($Profile -ne 'all') { $hostArguments += @('--video-profile', $Profile) }
if ($Workload -ne 'all') { $hostArguments += @('--benchmark-workload', $Workload) }

Write-Host "Host codec benchmark ($hostSuite, codec=$Codec, profile=$Profile, workload=$Workload)..."
$hostBenchmarkProcess = Start-Process -FilePath $exe -ArgumentList $hostArguments -PassThru -Wait -WindowStyle Hidden
if ($hostBenchmarkProcess.ExitCode -ne 0) { throw "host codec benchmark failed with exit code $($hostBenchmarkProcess.ExitCode)" }

if (-not $HostOnly) {
    # Level B: actual WebRTC/desktop-browser measurement. These rows identify
    # Microsoft Edge automation and must never be described as iPad Safari.
    $scenarios = @(
        [pscustomobject]@{ Name = '4k-to-720p-fast'; Width = 3840; Height = 2160; Profile = 'fast'; OutputWidth = 1280; OutputHeight = 720; Viewport = '3840x2160' },
        [pscustomobject]@{ Name = '1080p-balanced'; Width = 1920; Height = 1080; Profile = 'balanced'; OutputWidth = 1920; OutputHeight = 1080; Viewport = '1920x1080' },
        [pscustomobject]@{ Name = '4k-to-1080p-balanced'; Width = 3840; Height = 2160; Profile = 'balanced'; OutputWidth = 1920; OutputHeight = 1080; Viewport = '3840x2160' },
        [pscustomobject]@{ Name = '4k-to-1440p-sharp'; Width = 3840; Height = 2160; Profile = 'sharp'; OutputWidth = 2560; OutputHeight = 1440; Viewport = '3840x2160' }
    )
    if ($Quick -or (-not $Full)) { $scenarios = @($scenarios | Where-Object Name -eq '1080p-balanced') }
    if ($Profile -ne 'all') { $scenarios = @($scenarios | Where-Object Profile -eq $Profile) }
    if ($ScenarioName) {
        $scenarios = @($scenarios | Where-Object Name -eq $ScenarioName)
        if ($scenarios.Count -eq 0) { throw "Unknown or filtered benchmark scenario: $ScenarioName" }
    }

    $browserCodecs = if ($Codec -eq 'all') {
        @('h264-hardware', 'hevc-hardware', 'av1-hardware', 'h264-software')
    } elseif ($Codec -eq 'auto') {
        @('auto')
    } else {
        @($Codec)
    }
    $results = @()
    $port = 48920
    foreach ($case in $scenarios) {
        foreach ($browserCodec in $browserCodecs) {
            Write-Host "End-to-end Edge test: $($case.Name), $browserCodec..."
            $runName = "$($case.Name)-$browserCodec"
            $sessionPath = Join-Path $reportRoot "$runName-session.json"
            $hostMetricsPath = Join-Path $reportRoot "$runName-host.json"
            $browserReportPath = Join-Path $reportRoot "$runName-browser.json"
            $hostSeconds = $DurationSeconds + 30
            $arguments = @(
                '--headless', '--capture', 'test-pattern', '--input-sink', 'log', '--no-mdns',
                '--port', "$port", '--encoder', $browserCodec, '--video-profile', $case.Profile, '--max-fps', '60',
                '--test-width', "$($case.Width)", '--test-height', "$($case.Height)",
                '--run-seconds', "$hostSeconds", '--session-info', $sessionPath, '--metrics-output', $hostMetricsPath
            )
            $hostProcess = Start-Process -FilePath $exe -ArgumentList $arguments -PassThru -WindowStyle Hidden
            $testPassed = $false
            $skipReason = ''
            try {
                $deadline = (Get-Date).AddSeconds(20)
                while (-not (Test-Path -LiteralPath $sessionPath)) {
                    if ($hostProcess.HasExited) { throw "NFiDB exited before $runName started" }
                    if ((Get-Date) -ge $deadline) { throw "NFiDB did not write session info for $runName" }
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
                        $testPassed = $LASTEXITCODE -eq 0
                    }
                    finally { Pop-Location }
                }
                finally {
                    foreach ($name in $savedEnvironment.Keys) {
                        if ($null -eq $savedEnvironment[$name]) {
                            Remove-Item -LiteralPath "env:$name" -ErrorAction SilentlyContinue
                        } else {
                            Set-Item -LiteralPath "env:$name" -Value $savedEnvironment[$name]
                        }
                    }
                }
                if (-not $testPassed -and $browserCodec -in @('hevc-hardware', 'av1-hardware')) {
                    $skipReason = "$browserCodec was not mutually usable in Microsoft Edge automation; see Playwright trace"
                    Write-Host "$browserCodec skipped for Edge — receiver did not complete playback"
                } elseif (-not $testPassed) {
                    throw "Playwright failed for $runName"
                }
                if ($testPassed) {
                    Wait-Process -Id $hostProcess.Id -Timeout ($hostSeconds + 5)
                }
                $hostMetrics = if (Test-Path -LiteralPath $hostMetricsPath) { Get-Content -Raw -LiteralPath $hostMetricsPath | ConvertFrom-Json } else { $null }
                $browserMetrics = if (Test-Path -LiteralPath $browserReportPath) { Get-Content -Raw -LiteralPath $browserReportPath | ConvertFrom-Json } else { $null }
                $results += [pscustomobject]@{
                    environment = 'Microsoft Edge Playwright automation'
                    name = $case.Name
                    codec = $browserCodec
                    status = if ($testPassed) { 'completed' } else { 'skipped' }
                    skip_reason = $skipReason
                    source = "$($case.Width)x$($case.Height)"
                    output = "$($case.OutputWidth)x$($case.OutputHeight)"
                    viewport = $case.Viewport
                    duration_seconds = $DurationSeconds
                    host = $hostMetrics
                    browser = $browserMetrics
                }
            }
            finally {
                if (-not $hostProcess.HasExited) { Stop-Process -Id $hostProcess.Id -Force }
            }
            $port += 1
        }
    }
    $endToEndPath = Join-Path $reportRoot 'end-to-end-edge.json'
    $results | ConvertTo-Json -Depth 14 | Set-Content -LiteralPath $endToEndPath -Encoding utf8
    $flatResults = $results | ForEach-Object {
        $browser = $_.browser
        [pscustomobject]@{
            environment = $_.environment
            name = $_.name
            codec = $_.codec
            status = $_.status
            skip_reason = $_.skip_reason
            source = $_.source
            output = $_.output
            host_capture_fps = $browser.after.host.capture_fps
            host_encoded_fps = $browser.after.host.encoded_fps
            encode_mean_ms = $browser.after.host.average_encode_ms
            encode_p95_ms = $browser.after.host.encode_p95_ms
            process_cpu_percent = $browser.after.host.process_cpu_percent
            working_set_mib = $browser.after.host.working_set_mib
            receive_mbps_p95 = $browser.after.hostDiagnostics.receive_mbps.p95
            presented_fps = $browser.result.presentationFramesPerSecond
            pipeline_estimate_p95_ms = $browser.after.hostDiagnostics.estimated_pipeline_ms.p95
            startup_ms = $browser.after.video.startupMs
            rtp_packet_loss = $browser.after.inboundVideo.packetsLost
            decoder_drops = $browser.after.inboundVideo.framesDropped
            freezes = $browser.after.inboundVideo.freezeCount
            input_samples = $browser.expectedSamples
            input_sequence_gaps = $browser.after.host.sample_sequence_gaps
        }
    }
    $flatResults | Export-Csv -LiteralPath (Join-Path $reportRoot 'end-to-end-edge.csv') -NoTypeInformation -Encoding utf8
    $summaryLines = @(
        '# NFiDB end-to-end codec benchmark',
        '',
        'Microsoft Edge Playwright automation over the normal authenticated LAN/WebRTC path. These are not iPad Safari measurements.',
        '',
        '| Case | Codec | State | Encoded fps | Presented fps | Encode p95 | Receive Mbps p95 | Pipeline estimate p95 | RTP loss | Decoder drops | Freezes |',
        '| --- | --- | --- | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: |'
    )
    foreach ($row in $flatResults) {
        $formatNumber = { param($value, $suffix = '') if ($null -eq $value) { '-' } else { '{0:N2}{1}' -f [double]$value, $suffix } }
        $summaryLines += '| {0} | {1} | {2} | {3} | {4} | {5} | {6} | {7} | {8} | {9} | {10} |' -f `
            $row.name, $row.codec, $row.status, (& $formatNumber $row.host_encoded_fps), (& $formatNumber $row.presented_fps), `
            (& $formatNumber $row.encode_p95_ms ' ms'), (& $formatNumber $row.receive_mbps_p95), `
            (& $formatNumber $row.pipeline_estimate_p95_ms ' ms'), ($row.rtp_packet_loss ?? '-'), `
            ($row.decoder_drops ?? '-'), ($row.freezes ?? '-')
    }
    $summaryLines | Set-Content -LiteralPath (Join-Path $reportRoot 'end-to-end-summary.md') -Encoding utf8
}

Copy-Item -LiteralPath (Join-Path $hostReport 'results.json') -Destination (Join-Path $repoRoot 'build\benchmarks\latest.json') -Force
Write-Host "Benchmark report: $reportRoot"
