param(
    [ValidateRange(5, 30)] [int] $MonitorSeconds = 8,
    [ValidateRange(10, 240)] [int] $BenchmarkFrames = 30,
    [switch] $SkipReleaseBuild
)

$ErrorActionPreference = 'Stop'
$ProgressPreference = 'SilentlyContinue'
$repoRoot = Split-Path -Parent $PSScriptRoot
. (Join-Path $PSScriptRoot 'msvc-env.ps1')

$stamp = Get-Date -Format 'yyyyMMdd-HHmmss'
$reportRoot = Join-Path $repoRoot "build\gpu-validation\$stamp"
$latestPath = Join-Path $repoRoot 'build\gpu-validation\latest.json'
New-Item -ItemType Directory -Force -Path $reportRoot | Out-Null

$steps = [System.Collections.Generic.List[object]]::new()
$failure = $null

function Invoke-LoggedNative {
    param(
        [string] $Name,
        [string] $LogName,
        [scriptblock] $Command
    )
    $logPath = Join-Path $reportRoot $LogName
    $started = Get-Date
    Write-Host "`n== $Name =="
    # Windows PowerShell 5.1 wraps every native stderr line as a non-terminating
    # ErrorRecord. With the script-wide Stop policy, Cargo's ordinary
    # "Checking ..." progress would otherwise abort this function before Cargo
    # can emit a compiler result or create the log. Temporarily continue only
    # around the native pipeline, then trust the process exit code.
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
        } | Tee-Object -FilePath $logPath
        $exitCode = $LASTEXITCODE
    }
    finally {
        $ErrorActionPreference = $previousErrorAction
    }
    $steps.Add([pscustomobject]@{
        name = $Name
        passed = ($exitCode -eq 0)
        exit_code = $exitCode
        seconds = [math]::Round(((Get-Date) - $started).TotalSeconds, 3)
        log = $logPath
    })
    if ($exitCode -ne 0) {
        throw "$Name failed with exit code $exitCode"
    }
}

try {
    Push-Location $repoRoot
    try {
        Invoke-LoggedNative 'Rust source and test-target check' 'cargo-check.log' {
            cargo check --locked --workspace --all-targets
        }
        Invoke-LoggedNative 'GPU surface encoder test' 'gpu-test.log' {
            cargo test --locked -p nfidb-host-windows available_hardware_encoders_accept_the_gpu_pipeline -- --nocapture
        }
        if (-not $SkipReleaseBuild) {
            Invoke-LoggedNative 'Optimized Windows host build' 'release-build.log' {
                cargo build --locked --release -p nfidb
            }
        }
    }
    finally {
        Pop-Location
    }

    $exe = Join-Path $repoRoot 'target\release\nfidb.exe'
    if (-not (Test-Path -LiteralPath $exe)) {
        throw "Release executable does not exist: $exe"
    }

    $runtimePath = Join-Path $reportRoot 'monitor-runtime.json'
    $runtimeStarted = Get-Date
    Write-Host "`n== Real-monitor GPU path =="
    $runtimeArgs = @(
        '--headless', '--capture', 'monitor', '--input-sink', 'log', '--no-mdns',
        '--encoder', 'h264-hardware', '--video-profile', 'balanced', '--max-fps', '60',
        '--run-seconds', "$MonitorSeconds", '--runtime-output', $runtimePath
    )
    $runtimeProcess = Start-Process -FilePath $exe -ArgumentList $runtimeArgs -PassThru -Wait -WindowStyle Hidden
    if ($runtimeProcess.ExitCode -ne 0) {
        throw "Real-monitor GPU run failed with exit code $($runtimeProcess.ExitCode)"
    }
    if (-not (Test-Path -LiteralPath $runtimePath)) {
        throw 'Real-monitor GPU run did not write its runtime report'
    }
    $runtime = Get-Content -Raw -LiteralPath $runtimePath | ConvertFrom-Json
    $memoryPath = [string] $runtime.video.pipeline_memory_mode
    $encodedFrames = [int64] $runtime.metrics.encoded_frames
    $capturedFrames = [int64] $runtime.metrics.capture_frames
    $runtimePassed = $capturedFrames -gt 0 -and $encodedFrames -gt 0 -and $memoryPath -in @('gpu-zero-copy', 'gpu-assisted')
    $steps.Add([pscustomobject]@{
        name = 'Real-monitor GPU path'
        passed = $runtimePassed
        seconds = [math]::Round(((Get-Date) - $runtimeStarted).TotalSeconds, 3)
        memory_path = $memoryPath
        captured_frames = $capturedFrames
        encoded_frames = $encodedFrames
        encoder = [string] $runtime.video.encoder_name
        error = $runtime.capture.error
        report = $runtimePath
    })
    if (-not $runtimePassed) {
        throw "Real-monitor run did not prove a GPU path (path=$memoryPath, captured=$capturedFrames, encoded=$encodedFrames, error=$($runtime.capture.error))"
    }
    Write-Host "GPU monitor path: $memoryPath; $encodedFrames encoded frames; $($runtime.video.encoder_name)"

    $benchmarkPath = Join-Path $reportRoot 'quick-benchmark'
    $benchmarkStarted = Get-Date
    Write-Host "`n== Quick GPU codec benchmark =="
    $benchmarkArgs = @(
        '--benchmark', 'quick', '--benchmark-frames', "$BenchmarkFrames", '--benchmark-output', $benchmarkPath
    )
    $benchmarkProcess = Start-Process -FilePath $exe -ArgumentList $benchmarkArgs -PassThru -Wait -WindowStyle Hidden
    if ($benchmarkProcess.ExitCode -ne 0) {
        throw "Quick GPU benchmark failed with exit code $($benchmarkProcess.ExitCode)"
    }
    $benchmarkResultsPath = Join-Path $benchmarkPath 'results.json'
    $benchmark = Get-Content -Raw -LiteralPath $benchmarkResultsPath | ConvertFrom-Json
    $hardwareRows = @($benchmark.results | Where-Object { $_.hardware -and $_.state -eq 'completed' })
    $badHardwareRows = @($hardwareRows | Where-Object { $_.pipeline_memory_mode -notin @('gpu-zero-copy', 'gpu-assisted') })
    $benchmarkPassed = $hardwareRows.Count -gt 0 -and $badHardwareRows.Count -eq 0
    $steps.Add([pscustomobject]@{
        name = 'Quick GPU codec benchmark'
        passed = $benchmarkPassed
        seconds = [math]::Round(((Get-Date) - $benchmarkStarted).TotalSeconds, 3)
        completed_hardware_rows = $hardwareRows.Count
        paths = @($hardwareRows | ForEach-Object { "$($_.mode):$($_.pipeline_memory_mode)" })
        report = $benchmarkResultsPath
    })
    if (-not $benchmarkPassed) {
        throw "Quick benchmark did not complete a valid GPU hardware row; inspect $benchmarkResultsPath"
    }
    $hardwareRows | Format-Table mode, pipeline_memory_mode, actual_fps, preprocess_p95_ms, encode_p95_ms, process_cpu_percent, working_set_peak_mib
}
catch {
    $failure = $_.Exception.Message
}

$gitCommit = (& git -C $repoRoot rev-parse HEAD 2>$null)
$gitTree = (& git -C $repoRoot status --short 2>$null) -join "`n"
$report = [ordered]@{
    schema_version = 1
    generated_at = (Get-Date).ToString('o')
    repository = $repoRoot
    commit = $gitCommit
    working_tree = $gitTree
    passed = ($null -eq $failure -and @($steps | Where-Object { -not $_.passed }).Count -eq 0)
    failure = $failure
    steps = $steps
    report_directory = $reportRoot
}
New-Item -ItemType Directory -Force -Path (Split-Path -Parent $latestPath) | Out-Null
$report | ConvertTo-Json -Depth 8 | Set-Content -LiteralPath (Join-Path $reportRoot 'result.json') -Encoding utf8
$report | ConvertTo-Json -Depth 8 | Set-Content -LiteralPath $latestPath -Encoding utf8

if ($report.passed) {
    Write-Host "`nGPU validation passed. Tell Codex: resume"
    Write-Host "Report: $latestPath"
    exit 0
}

Write-Host "`nGPU validation failed. Tell Codex: resume"
Write-Host "Reason: $failure"
Write-Host "Report: $latestPath"
exit 1
