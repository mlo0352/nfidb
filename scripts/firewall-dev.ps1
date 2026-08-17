$ErrorActionPreference = 'Stop'
$repoRoot = Split-Path -Parent $PSScriptRoot
$exe = Join-Path $repoRoot 'target\debug\nfidb.exe'

$identity = [Security.Principal.WindowsIdentity]::GetCurrent()
$principal = [Security.Principal.WindowsPrincipal]::new($identity)
if (-not $principal.IsInRole([Security.Principal.WindowsBuiltInRole]::Administrator)) {
    throw 'Run this script from an elevated PowerShell window. It creates a Private-profile inbound rule only.'
}
if (-not (Test-Path -LiteralPath $exe)) {
    throw "Build the debug host first; not found: $exe"
}

$ruleName = 'NFiDB development host (Private)'
Get-NetFirewallRule -DisplayName $ruleName -ErrorAction SilentlyContinue | Remove-NetFirewallRule
New-NetFirewallRule -DisplayName $ruleName -Direction Inbound -Action Allow -Profile Private -Program (Resolve-Path $exe) -Protocol TCP | Out-Null
Write-Host "Created Private-profile TCP rule for $exe"
