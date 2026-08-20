param(
    [string] $ExecutablePath = '',
    [string] $ArchivePath = '',
    [int] $Port = 49121,
    [int] $StartupTimeoutSeconds = 45
)

$ErrorActionPreference = 'Stop'
$repoRoot = Split-Path -Parent $PSScriptRoot
$stamp = Get-Date -Format 'yyyyMMdd-HHmmss'
$resultRoot = Join-Path $repoRoot "build\gui-smoke\$stamp"
New-Item -ItemType Directory -Force -Path $resultRoot | Out-Null

if ($ExecutablePath -and $ArchivePath) {
    throw 'Use either -ExecutablePath or -ArchivePath, not both'
}
if ($ArchivePath) {
    $archive = [IO.Path]::GetFullPath($ArchivePath)
    if (-not (Test-Path -LiteralPath $archive -PathType Leaf)) {
        throw "GUI smoke archive does not exist: $archive"
    }
    $extractRoot = Join-Path $resultRoot 'extracted'
    Expand-Archive -LiteralPath $archive -DestinationPath $extractRoot
    $ExecutablePath = Join-Path $extractRoot 'nfidb.exe'
}
elseif (-not $ExecutablePath) {
    $ExecutablePath = Join-Path $repoRoot 'target\release\nfidb.exe'
}
$hostExecutable = [IO.Path]::GetFullPath($ExecutablePath)
if (-not (Test-Path -LiteralPath $hostExecutable -PathType Leaf)) {
    throw "GUI smoke executable does not exist: $hostExecutable"
}
if ($Port -lt 1024 -or $Port -gt 65535) {
    throw "GUI smoke port is outside 1024-65535: $Port"
}
if ($StartupTimeoutSeconds -lt 5 -or $StartupTimeoutSeconds -gt 180) {
    throw 'GUI smoke startup timeout must be between 5 and 180 seconds'
}

$portProbe = [Net.Sockets.TcpListener]::new([Net.IPAddress]::Any, $Port)
$portProbe.ExclusiveAddressUse = $true
try {
    $portProbe.Start()
}
catch {
    throw "GUI smoke port $Port is already in use"
}
finally {
    $portProbe.Stop()
}

$stdoutPath = Join-Path $resultRoot 'stdout.log'
$stderrPath = Join-Path $resultRoot 'stderr.log'
$resultPath = Join-Path $resultRoot 'result.json'
$latestPath = Join-Path $repoRoot 'build\gui-smoke\latest.json'
$started = Get-Date
$process = $null
$result = [ordered]@{
    passed = $false
    executable = $hostExecutable
    port = $Port
    pid = $null
    main_window_handle = 0
    main_window_title = ''
    status_protocol_version = $null
    status_webrtc = $null
    startup_seconds = $null
    exit_code = $null
    stderr = $stderrPath
    error = $null
}

try {
    $process = Start-Process -FilePath $hostExecutable -WorkingDirectory (Split-Path $hostExecutable) -ArgumentList @(
        '--capture=test-pattern', '--input-sink=log', '--no-mdns', "--port=$Port", '--log-level=debug'
    ) -PassThru -RedirectStandardOutput $stdoutPath -RedirectStandardError $stderrPath
    $result.pid = $process.Id
    $deadline = (Get-Date).AddSeconds($StartupTimeoutSeconds)
    $windowReady = $false
    $serverReady = $false
    $status = $null
    while ((Get-Date) -lt $deadline -and (-not $windowReady -or -not $serverReady)) {
        $process.Refresh()
        if ($process.HasExited) {
            $result.exit_code = $process.ExitCode
            $crashText = if (Test-Path -LiteralPath $stderrPath) {
                (Get-Content -LiteralPath $stderrPath -Raw).Trim()
            } else {
                ''
            }
            throw "NFiDB exited before its GUI became ready (code $($process.ExitCode)). $crashText"
        }
        if ($process.MainWindowHandle -ne 0) {
            $windowReady = $true
            $result.main_window_handle = [int64]$process.MainWindowHandle
            $result.main_window_title = $process.MainWindowTitle
        }
        try {
            $status = Invoke-RestMethod -Uri "http://127.0.0.1:$Port/api/status" -TimeoutSec 2
            $serverReady = $status.protocol_version -gt 0 -and $status.webrtc -eq $true
        }
        catch {
            $serverReady = $false
        }
        if (-not $windowReady -or -not $serverReady) {
            Start-Sleep -Milliseconds 100
        }
    }
    if (-not $windowReady) {
        throw "NFiDB did not create a main window within $StartupTimeoutSeconds seconds"
    }
    if (-not $serverReady) {
        throw "NFiDB GUI started but its local server was not ready on port $Port within $StartupTimeoutSeconds seconds"
    }
    if ($result.main_window_title -ne 'NFiDB — No Frills iPad Drawing Bridge') {
        throw "Unexpected GUI window title: $($result.main_window_title)"
    }
    $result.status_protocol_version = $status.protocol_version
    $result.status_webrtc = $status.webrtc
    $result.startup_seconds = [Math]::Round(((Get-Date) - $started).TotalSeconds, 3)
    $result.passed = $true
}
catch {
    $result.error = $_.Exception.Message
}
finally {
    if ($process -and -not $process.HasExited) {
        Stop-Process -Id $process.Id -Force
        $process.WaitForExit()
    }
    if ($process -and $process.HasExited) {
        $result.exit_code = $process.ExitCode
    }
    $json = $result | ConvertTo-Json -Depth 6
    Set-Content -LiteralPath $resultPath -Value $json -Encoding utf8
    New-Item -ItemType Directory -Force -Path (Split-Path -Parent $latestPath) | Out-Null
    Set-Content -LiteralPath $latestPath -Value $json -Encoding utf8
}

if (-not $result.passed) {
    Write-Host "GUI startup smoke failed. Result: $resultPath" -ForegroundColor Red
    throw $result.error
}

Write-Host "GUI startup smoke passed in $($result.startup_seconds) s: $($result.main_window_title)"
Write-Host "Result: $resultPath"
