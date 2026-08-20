param(
    [string] $ExecutablePath = '',
    [string] $ArchivePath = '',
    [int] $Port = 49121,
    [int] $StartupTimeoutSeconds = 45
)

$ErrorActionPreference = 'Stop'
Add-Type -AssemblyName System.Net.Http
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

function Get-LogTail {
    param([string] $Path, [int] $MaximumCharacters = 8000)

    if (-not (Test-Path -LiteralPath $Path -PathType Leaf)) {
        return ''
    }
    $content = (Get-Content -LiteralPath $Path -Raw -ErrorAction SilentlyContinue)
    if (-not $content) {
        return ''
    }
    if ($content.Length -le $MaximumCharacters) {
        return $content.Trim()
    }
    return $content.Substring($content.Length - $MaximumCharacters).Trim()
}

function Get-ProcessListeningPorts {
    param(
        [int] $ProcessId,
        [int] $MinimumPort,
        [int] $MaximumPort
    )

    $ports = @()
    try {
        $ports = @(
            Get-NetTCPConnection -State Listen -OwningProcess $ProcessId -ErrorAction Stop |
                Where-Object { $_.LocalPort -ge $MinimumPort -and $_.LocalPort -le $MaximumPort } |
                Select-Object -ExpandProperty LocalPort
        )
    }
    catch {
        # The CIM cmdlet can be unavailable or briefly report no instances on
        # hosted runners. netstat provides a permission-free fallback.
    }
    if ($ports.Count -eq 0) {
        $netstatPath = Join-Path $env:SystemRoot 'System32\netstat.exe'
        if (Test-Path -LiteralPath $netstatPath -PathType Leaf) {
            foreach ($line in (& $netstatPath -ano -p TCP 2>$null)) {
                if ($line -match '^\s*TCP\s+\S+:(\d+)\s+\S+\s+LISTENING\s+(\d+)\s*$') {
                    $observedPort = [int]$Matches[1]
                    $owner = [int]$Matches[2]
                    if ($owner -eq $ProcessId -and $observedPort -ge $MinimumPort -and $observedPort -le $MaximumPort) {
                        $ports += $observedPort
                    }
                }
            }
        }
    }
    $ports | Sort-Object -Unique
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
    requested_port = $Port
    port = $null
    pid = $null
    main_window_handle = 0
    main_window_title = ''
    listening_ports = @()
    status_protocol_version = $null
    status_webrtc = $null
    startup_seconds = $null
    exit_code = $null
    stdout = $stdoutPath
    stderr = $stderrPath
    stdout_tail = ''
    stderr_tail = ''
    last_status_error = $null
    error = $null
}
$httpHandler = [Net.Http.HttpClientHandler]::new()
$httpHandler.UseProxy = $false
$httpClient = [Net.Http.HttpClient]::new($httpHandler, $true)
$httpClient.Timeout = [TimeSpan]::FromSeconds(2)
# Windows PowerShell 5.1 decodes UTF-8 scripts without a BOM using the
# active ANSI code page. Construct the em dash explicitly so this check is
# stable in both Windows PowerShell and PowerShell 7.
$expectedWindowTitle = 'NFiDB ' + [char]0x2014 + ' No Frills iPad Drawing Bridge'

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
            $result.main_window_handle = [int64]$process.MainWindowHandle
            $result.main_window_title = $process.MainWindowTitle
            $windowReady = $result.main_window_title -eq $expectedWindowTitle
        }
        if (-not $serverReady) {
            $maximumPort = [Math]::Min(65535, $Port + 99)
            $listeningPorts = @(Get-ProcessListeningPorts -ProcessId $process.Id -MinimumPort $Port -MaximumPort $maximumPort)
            $result.listening_ports = @($listeningPorts)
            if ($listeningPorts.Count -gt 0) {
                $portsToCheck = $listeningPorts
            }
            else {
                # Checking the requested port also keeps the smoke functional
                # if listener ownership inspection is unavailable.
                $portsToCheck = @($Port)
            }
            foreach ($statusPort in $portsToCheck) {
                try {
                    $response = $httpClient.GetAsync("http://127.0.0.1:$statusPort/api/status").GetAwaiter().GetResult()
                    try {
                        if (-not $response.IsSuccessStatusCode) {
                            throw "HTTP status $([int]$response.StatusCode)"
                        }
                        $statusJson = $response.Content.ReadAsStringAsync().GetAwaiter().GetResult()
                        $status = $statusJson | ConvertFrom-Json
                        $serverReady = $status.protocol_version -gt 0 -and $status.webrtc -eq $true
                        if (-not $serverReady) {
                            $result.last_status_error = "Unexpected /api/status response from port ${statusPort}: $statusJson"
                        }
                        else {
                            $result.port = [int]$statusPort
                            $result.status_protocol_version = $status.protocol_version
                            $result.status_webrtc = $status.webrtc
                            $result.last_status_error = $null
                        }
                    }
                    finally {
                        $response.Dispose()
                    }
                }
                catch {
                    $serverReady = $false
                    $result.last_status_error = "Port ${statusPort}: $($_.Exception.Message)"
                }
                if ($serverReady) {
                    break
                }
            }
        }
        if (-not $windowReady -or -not $serverReady) {
            Start-Sleep -Milliseconds 100
        }
    }
    if (-not $windowReady) {
        throw "NFiDB did not create its titled main window within $StartupTimeoutSeconds seconds (last title: $($result.main_window_title))"
    }
    if (-not $serverReady) {
        $observedPorts = if ($result.listening_ports.Count) { $result.listening_ports -join ', ' } else { 'none' }
        throw "NFiDB GUI started but its local server was not ready within $StartupTimeoutSeconds seconds (requested $Port; process listeners: $observedPorts; last error: $($result.last_status_error))"
    }
    if ($result.main_window_title -ne $expectedWindowTitle) {
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
    $httpClient.Dispose()
    $result.stdout_tail = Get-LogTail -Path $stdoutPath
    $result.stderr_tail = Get-LogTail -Path $stderrPath
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
