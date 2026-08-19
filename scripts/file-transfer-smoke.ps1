param(
    [switch] $SkipBuild,
    [string] $ExecutablePath = ''
)

$ErrorActionPreference = 'Stop'
$repoRoot = Split-Path -Parent $PSScriptRoot
. (Join-Path $PSScriptRoot 'msvc-env.ps1')

if (-not $SkipBuild -and $ExecutablePath) {
    throw '-ExecutablePath requires -SkipBuild because the selected binary must already exist'
}
if (-not $SkipBuild) {
    Push-Location $repoRoot
    try {
        cargo build -p nfidb
        if ($LASTEXITCODE -ne 0) { throw 'debug host build failed' }
    }
    finally {
        Pop-Location
    }
}
$hostExecutable = if ($ExecutablePath) {
    [IO.Path]::GetFullPath($ExecutablePath)
} else {
    Join-Path $repoRoot 'target\debug\nfidb.exe'
}
if (-not (Test-Path -LiteralPath $hostExecutable -PathType Leaf)) {
    throw "host executable does not exist: $hostExecutable"
}

$tempBase = [IO.Path]::GetFullPath([IO.Path]::GetTempPath())
$testRoot = Join-Path $tempBase ("nfidb-transfer-smoke-" + [guid]::NewGuid().ToString('N'))
$resolvedRoot = [IO.Path]::GetFullPath($testRoot)
if (-not $resolvedRoot.StartsWith($tempBase, [StringComparison]::OrdinalIgnoreCase)) {
    throw "refusing to use a test directory outside $tempBase"
}

$inbox = Join-Path $resolvedRoot 'inbox'
$sessionInfo = Join-Path $resolvedRoot 'session.json'
$outgoingPath = Join-Path $resolvedRoot 'from-windows.bin'
$outgoingPathTwo = Join-Path $resolvedRoot 'from-windows-2.bin'
$fullDownload = Join-Path $resolvedRoot 'downloaded.bin'
$fullDownloadTwo = Join-Path $resolvedRoot 'downloaded-2.bin'
$rangeDownload = Join-Path $resolvedRoot 'range.bin'
$hostProcess = $null
$stopwatch = [Diagnostics.Stopwatch]::StartNew()

try {
    New-Item -ItemType Directory -Force -Path $inbox | Out-Null
    $outgoingBytes = [byte[]]::new(5 * 1024 * 1024 + 137)
    for ($index = 0; $index -lt $outgoingBytes.Length; $index++) {
        $outgoingBytes[$index] = ($index * 31 + 17) % 251
    }
    [IO.File]::WriteAllBytes($outgoingPath, $outgoingBytes)
    $outgoingBytesTwo = [byte[]]::new(2 * 1024 * 1024 + 83)
    for ($index = 0; $index -lt $outgoingBytesTwo.Length; $index++) {
        $outgoingBytesTwo[$index] = ($index * 23 + 29) % 247
    }
    [IO.File]::WriteAllBytes($outgoingPathTwo, $outgoingBytesTwo)

    $hostProcess = Start-Process -FilePath $hostExecutable -ArgumentList @(
        '--headless', '--capture=none', '--input-sink=log', '--no-mdns', '--run-seconds=90',
        "--session-info=$sessionInfo", "--file-inbox=$inbox", "--queue-file=$outgoingPath", "--queue-file=$outgoingPathTwo"
    ) -PassThru -WindowStyle Hidden

    $deadline = [DateTime]::UtcNow.AddSeconds(20)
    while (-not (Test-Path -LiteralPath $sessionInfo)) {
        if ($hostProcess.HasExited) { throw "headless host exited with code $($hostProcess.ExitCode)" }
        if ([DateTime]::UtcNow -gt $deadline) { throw 'timed out waiting for headless session metadata' }
        Start-Sleep -Milliseconds 100
    }
    $session = Get-Content -Raw -LiteralPath $sessionInfo | ConvertFrom-Json
    $origin = $session.url.TrimEnd('/')
    $web = [Microsoft.PowerShell.Commands.WebRequestSession]::new()
    $commonHeaders = @{ Origin = $origin }
    $null = Invoke-RestMethod -Uri "$origin/api/pair" -Method Post -WebSession $web -Headers $commonHeaders -ContentType 'application/json' -Body (@{ pin = $session.pin } | ConvertTo-Json -Compress)

    try {
        $unauthorized = [Microsoft.PowerShell.Commands.WebRequestSession]::new()
        $null = Invoke-WebRequest -Uri "$origin/api/files" -WebSession $unauthorized
        throw 'unauthenticated file listing unexpectedly succeeded'
    }
    catch {
        if ($_.Exception.Response.StatusCode.value__ -ne 401) { throw }
    }

    $uploadBytes = [byte[]]::new(3 * 1024 * 1024 + 509)
    for ($index = 0; $index -lt $uploadBytes.Length; $index++) {
        $uploadBytes[$index] = ($index * 19 + 3) % 253
    }
    $uploadId = [guid]::NewGuid().ToString()
    $uploadRequest = @{
        upload_id = $uploadId; name = 'from-ipad.bin'; mime = 'application/octet-stream'; size = $uploadBytes.Length
    } | ConvertTo-Json -Compress
    $ticket = Invoke-RestMethod -Uri "$origin/api/files/uploads" -Method Post -WebSession $web -Headers $commonHeaders -ContentType 'application/json' -Body $uploadRequest
    $retriedTicket = Invoke-RestMethod -Uri "$origin/api/files/uploads" -Method Post -WebSession $web -Headers $commonHeaders -ContentType 'application/json' -Body $uploadRequest
    if ($retriedTicket.upload_id -ne $ticket.upload_id) { throw 'idempotent upload creation returned a different ticket' }

    $offset = 0
    $uploadStarted = [Diagnostics.Stopwatch]::StartNew()
    while ($offset -lt $uploadBytes.Length) {
        $count = [Math]::Min([int]$ticket.chunk_size_bytes, $uploadBytes.Length - $offset)
        $chunk = [byte[]]::new($count)
        [Array]::Copy($uploadBytes, $offset, $chunk, 0, $count)
        $checksum = [Convert]::ToHexString([Security.Cryptography.SHA256]::HashData($chunk)).ToLowerInvariant()
        $headers = @{ Origin = $origin; 'x-nfidb-offset' = $offset.ToString(); 'x-nfidb-chunk-sha256' = $checksum }
        $progress = Invoke-RestMethod -Uri "$origin/api/files/uploads/$($ticket.upload_id)" -Method Put -WebSession $web -Headers $headers -ContentType 'application/octet-stream' -Body $chunk
        $offset = [int64]$progress.uploaded_bytes
    }
    $complete = Invoke-RestMethod -Uri "$origin/api/files/uploads/$($ticket.upload_id)/complete" -Method Post -WebSession $web -Headers $commonHeaders
    $retriedComplete = Invoke-RestMethod -Uri "$origin/api/files/uploads/$($ticket.upload_id)/complete" -Method Post -WebSession $web -Headers $commonHeaders
    if ($retriedComplete.sha256 -ne $complete.sha256 -or $retriedComplete.name -ne $complete.name) {
        throw 'idempotent upload completion returned a different result'
    }
    $uploadStarted.Stop()
    $inboxPath = Join-Path $inbox $complete.name
    if (-not (Test-Path -LiteralPath $inboxPath)) { throw 'completed upload did not appear in the Inbox' }
    $uploadedHash = (Get-FileHash -Algorithm SHA256 -LiteralPath $inboxPath).Hash.ToLowerInvariant()
    $sourceUploadHash = [Convert]::ToHexString([Security.Cryptography.SHA256]::HashData($uploadBytes)).ToLowerInvariant()
    if ($uploadedHash -ne $sourceUploadHash -or $complete.sha256 -ne $sourceUploadHash) {
        throw 'uploaded file checksum mismatch'
    }
    if ((Get-ChildItem -LiteralPath $inbox -File | Measure-Object).Count -ne 1) {
        throw 'repeated upload completion created a duplicate Inbox file'
    }

    $cancelTicket = Invoke-RestMethod -Uri "$origin/api/files/uploads" -Method Post -WebSession $web -Headers $commonHeaders -ContentType 'application/json' -Body (@{
        name = 'cancel-me.bin'; mime = 'application/octet-stream'; size = 4096
    } | ConvertTo-Json -Compress)
    $cancelChunk = [byte[]]::new(1024)
    $cancelHash = [Convert]::ToHexString([Security.Cryptography.SHA256]::HashData($cancelChunk)).ToLowerInvariant()
    $null = Invoke-RestMethod -Uri "$origin/api/files/uploads/$($cancelTicket.upload_id)" -Method Put -WebSession $web -Headers @{
        Origin = $origin; 'x-nfidb-offset' = '0'; 'x-nfidb-chunk-sha256' = $cancelHash
    } -ContentType 'application/octet-stream' -Body $cancelChunk
    $null = Invoke-WebRequest -Uri "$origin/api/files/uploads/$($cancelTicket.upload_id)" -Method Delete -WebSession $web -Headers $commonHeaders

    $listing = Invoke-RestMethod -Uri "$origin/api/files" -WebSession $web
    $outgoing = $listing.outbox | Where-Object name -eq 'from-windows.bin' | Select-Object -First 1
    $outgoingTwo = $listing.outbox | Where-Object name -eq 'from-windows-2.bin' | Select-Object -First 1
    if (-not $outgoing) { throw 'headless outbound file was not listed' }
    if (-not $outgoingTwo) { throw 'second headless outbound file was not listed' }
    $null = Invoke-WebRequest -Uri "$origin/api/files/outbox/$($outgoing.id)/download?remove=1" -WebSession $web -Headers @{ Range = 'bytes=1024-2047' } -OutFile $rangeDownload
    $rangeBytes = [IO.File]::ReadAllBytes($rangeDownload)
    if ($rangeBytes.Length -ne 1024) { throw "range response was $($rangeBytes.Length) bytes instead of 1024" }
    for ($index = 0; $index -lt $rangeBytes.Length; $index++) {
        if ($rangeBytes[$index] -ne $outgoingBytes[$index + 1024]) { throw "range response differed at byte $index" }
    }
    $afterRangeListing = Invoke-RestMethod -Uri "$origin/api/files" -WebSession $web
    if (-not ($afterRangeListing.outbox | Where-Object id -eq $outgoing.id)) {
        throw 'partial download incorrectly cleared its outbound queue item'
    }

    $downloadStarted = [Diagnostics.Stopwatch]::StartNew()
    $null = Invoke-WebRequest -Uri "$origin/api/files/outbox/$($outgoing.id)/download?remove=1" -WebSession $web -Headers @{ Range = 'bytes=0-' } -OutFile $fullDownload
    $null = Invoke-WebRequest -Uri "$origin/api/files/outbox/$($outgoingTwo.id)/download?remove=1" -WebSession $web -Headers @{ Range = 'bytes=0-' } -OutFile $fullDownloadTwo
    $downloadStarted.Stop()
    $downloadHash = (Get-FileHash -Algorithm SHA256 -LiteralPath $fullDownload).Hash
    $sourceDownloadHash = (Get-FileHash -Algorithm SHA256 -LiteralPath $outgoingPath).Hash
    if ($downloadHash -ne $sourceDownloadHash) {
        $downloadLength = (Get-Item -LiteralPath $fullDownload).Length
        throw "full ranged-capable download checksum mismatch: downloaded=$downloadLength/$downloadHash source=$($outgoingBytes.Length)/$sourceDownloadHash"
    }
    $downloadHashTwo = (Get-FileHash -Algorithm SHA256 -LiteralPath $fullDownloadTwo).Hash
    $sourceDownloadHashTwo = (Get-FileHash -Algorithm SHA256 -LiteralPath $outgoingPathTwo).Hash
    if ($downloadHashTwo -ne $sourceDownloadHashTwo) {
        $downloadLengthTwo = (Get-Item -LiteralPath $fullDownloadTwo).Length
        throw "second full download checksum mismatch: downloaded=$downloadLengthTwo/$downloadHashTwo source=$($outgoingBytesTwo.Length)/$sourceDownloadHashTwo"
    }

    $finalListing = Invoke-RestMethod -Uri "$origin/api/files" -WebSession $web
    if ($finalListing.outbox | Where-Object { $_.id -eq $outgoing.id -or $_.id -eq $outgoingTwo.id }) {
        throw 'completed downloads remained in the outbound queue despite auto-clear'
    }
    $stopwatch.Stop()
    $report = [ordered]@{
        passed = $true
        executable = $hostExecutable
        upload_bytes = $uploadBytes.Length
        upload_sha256 = $sourceUploadHash
        upload_seconds = [Math]::Round($uploadStarted.Elapsed.TotalSeconds, 3)
        upload_mbps = [Math]::Round($uploadBytes.Length * 8 / [Math]::Max($uploadStarted.Elapsed.TotalSeconds, 0.001) / 1000000, 3)
        download_files = 2
        download_bytes = $outgoingBytes.Length + $outgoingBytesTwo.Length
        download_seconds = [Math]::Round($downloadStarted.Elapsed.TotalSeconds, 3)
        download_mbps = [Math]::Round(($outgoingBytes.Length + $outgoingBytesTwo.Length) * 8 / [Math]::Max($downloadStarted.Elapsed.TotalSeconds, 0.001) / 1000000, 3)
        range_bytes = $rangeBytes.Length
        outbox_items_after_auto_clear = $finalListing.outbox.Count
        server_stats = $finalListing.stats
        total_seconds = [Math]::Round($stopwatch.Elapsed.TotalSeconds, 3)
    }
    $reportPath = Join-Path $repoRoot 'build\file-transfer-smoke.json'
    New-Item -ItemType Directory -Force -Path (Split-Path -Parent $reportPath) | Out-Null
    $report | ConvertTo-Json -Depth 8 | Set-Content -LiteralPath $reportPath -Encoding utf8
    $report | ConvertTo-Json -Depth 8
}
finally {
    if ($hostProcess -and -not $hostProcess.HasExited) {
        Stop-Process -Id $hostProcess.Id -Force
        $hostProcess.WaitForExit()
    }
    if (Test-Path -LiteralPath $resolvedRoot) {
        $verifiedRoot = [IO.Path]::GetFullPath($resolvedRoot)
        if (-not $verifiedRoot.StartsWith($tempBase, [StringComparison]::OrdinalIgnoreCase)) {
            throw "refusing to remove a test directory outside $tempBase"
        }
        Remove-Item -Recurse -Force -LiteralPath $verifiedRoot
    }
}
