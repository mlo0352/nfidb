param(
    [string] $Archive = '',
    [int] $Port = 49050
)

$ErrorActionPreference = 'Stop'
$repoRoot = Split-Path -Parent $PSScriptRoot
. (Join-Path $PSScriptRoot 'msvc-env.ps1')

if (-not $Archive) {
    $Archive = Join-Path $repoRoot 'build\packages\NFiDB-windows-x64.zip'
}
$archivePath = [System.IO.Path]::GetFullPath($Archive)
if (-not (Test-Path -LiteralPath $archivePath -PathType Leaf)) {
    throw "Portable archive does not exist: $archivePath"
}

$stamp = Get-Date -Format 'yyyyMMdd-HHmmss'
$smokeRoot = Join-Path $repoRoot "build\portable-smoke\$stamp"
$extractRoot = Join-Path $smokeRoot 'extracted'
New-Item -ItemType Directory -Force -Path $extractRoot | Out-Null
Expand-Archive -LiteralPath $archivePath -DestinationPath $extractRoot

$executable = Join-Path $extractRoot 'nfidb.exe'
$pointerExecutable = Join-Path $extractRoot 'pointer-sink.exe'
$sessionPath = Join-Path $smokeRoot 'session.json'
$metricsPath = Join-Path $smokeRoot 'metrics.json'
$dependencyPath = Join-Path $smokeRoot 'dependencies.txt'
if (-not (Test-Path -LiteralPath $executable -PathType Leaf)) {
    throw 'The portable archive does not contain nfidb.exe at its root'
}
if (-not (Test-Path -LiteralPath $pointerExecutable -PathType Leaf)) {
    throw 'The portable archive does not contain pointer-sink.exe at its root'
}

$arguments = @(
    '--headless', '--capture', 'test-pattern', '--input-sink', 'log', '--no-mdns',
    '--port', "$Port", '--video-profile', 'fast', '--max-fps', '30',
    '--run-seconds', '12', '--session-info', $sessionPath, '--metrics-output', $metricsPath
)
$process = Start-Process -FilePath $executable -ArgumentList $arguments -WorkingDirectory $extractRoot -PassThru -WindowStyle Hidden
try {
    $deadline = (Get-Date).AddSeconds(10)
    while (-not (Test-Path -LiteralPath $sessionPath)) {
        if ($process.HasExited) { throw "Portable host exited early with code $($process.ExitCode)" }
        if ((Get-Date) -ge $deadline) { throw 'Portable host did not become ready within 10 seconds' }
        Start-Sleep -Milliseconds 100
    }
    $session = Get-Content -Raw -LiteralPath $sessionPath | ConvertFrom-Json
    $baseUrl = "http://127.0.0.1:$($session.port)"
    $index = Invoke-WebRequest -UseBasicParsing -Uri "$baseUrl/" -TimeoutSec 5
    $clientMatch = [regex]::Match($index.Content, '<script[^>]+src="([^"]+\.js)"')
    if (-not $clientMatch.Success) {
        throw 'Embedded iPad page did not reference its JavaScript client'
    }
    $clientRelativePath = $clientMatch.Groups[1].Value -replace '^\./', '/'
    if (-not $clientRelativePath.StartsWith('/')) { $clientRelativePath = "/$clientRelativePath" }
    $client = Invoke-WebRequest -UseBasicParsing -Uri "$baseUrl$clientRelativePath" -TimeoutSec 5
    $status = Invoke-RestMethod -Uri "$baseUrl/api/status" -TimeoutSec 5
    if ($index.StatusCode -ne 200 -or $index.Content -notmatch 'No Frills iPad Drawing Bridge') {
        throw 'Embedded iPad page was not served correctly'
    }
    if ($client.StatusCode -ne 200 -or $client.Content.Length -lt 1000) {
        throw 'Embedded browser client was not served correctly'
    }
    if (-not $status.webrtc -or $status.protocol_version -le 0) {
        throw 'Portable host status endpoint did not report a usable transport'
    }
    Wait-Process -Id $process.Id -Timeout 20
    if ($process.ExitCode -ne 0) { throw "Portable host exited with code $($process.ExitCode)" }
    $metrics = Get-Content -Raw -LiteralPath $metricsPath | ConvertFrom-Json
    if ($metrics.encoded_frames -le 0 -or $metrics.encoded_bytes -le 0) {
        throw 'Portable host did not encode video during the smoke test'
    }

    $dependencyOutput = @(& dumpbin.exe /DEPENDENTS $executable)
    if ($LASTEXITCODE -ne 0) { throw 'nfidb.exe dependency inspection failed' }
    $dependencyOutput += & dumpbin.exe /DEPENDENTS $pointerExecutable
    if ($LASTEXITCODE -ne 0) { throw 'pointer-sink.exe dependency inspection failed' }
    $dependencyOutput | Set-Content -LiteralPath $dependencyPath -Encoding utf8
    $nonSystemDependencies = $dependencyOutput | Where-Object {
        $_ -match '^\s+[^\s]+\.dll\s*$' -and $_ -notmatch '(?i)(api-ms-win-[^\s]+|kernel32|user32|gdi32|advapi32|ole32|oleaut32|combase|shell32|comdlg32|ws2_32|dwmapi|shcore|bcrypt|bcryptprimitives|crypt32|iphlpapi|ntdll|secur32|userenv|winmm|version|uxtheme|winhttp|coremessaging|d3d11|opengl32|imm32)\.dll'
    }
    if ($nonSystemDependencies) {
        throw "Unexpected non-system DLL dependencies: $($nonSystemDependencies -join ', ')"
    }

    [pscustomobject]@{
        archive = $archivePath
        extracted_executable = $executable
        version = $session.version
        embedded_index_bytes = $index.RawContentLength
        embedded_client_bytes = $client.RawContentLength
        encoded_frames = $metrics.encoded_frames
        dependency_report = $dependencyPath
    } | ConvertTo-Json | Set-Content -LiteralPath (Join-Path $smokeRoot 'summary.json') -Encoding utf8
    Write-Host "Portable smoke test passed: $smokeRoot"
}
finally {
    if (-not $process.HasExited) {
        Stop-Process -Id $process.Id -Force
    }
}
