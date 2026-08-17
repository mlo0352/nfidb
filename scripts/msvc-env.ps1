$ErrorActionPreference = 'Stop'

if ($env:VSCMD_VER) {
    return
}

$vsWhere = Join-Path ${env:ProgramFiles(x86)} 'Microsoft Visual Studio\Installer\vswhere.exe'
if (-not (Test-Path -LiteralPath $vsWhere)) {
    throw 'Visual Studio Installer was not found. Install Visual Studio 2022 Build Tools with Desktop development with C++.'
}

$installPath = & $vsWhere -latest -products * -requires Microsoft.VisualStudio.Component.VC.Tools.x86.x64 -property installationPath
if (-not $installPath) {
    throw 'The Visual Studio C++ x64 toolchain was not found. Add the Desktop development with C++ workload.'
}

$vsDevCmd = Join-Path $installPath 'Common7\Tools\VsDevCmd.bat'
$setupCommand = "call `"$vsDevCmd`" -arch=x64 -host_arch=x64 >nul && set"
$environment = & cmd.exe /d /s /c $setupCommand
if ($LASTEXITCODE -ne 0) {
    throw 'Visual Studio developer environment setup failed.'
}
foreach ($line in $environment) {
    if ($line -match '^([^=]+)=(.*)$') {
        Set-Item -Path "Env:$($Matches[1])" -Value $Matches[2]
    }
}

$cargoBin = Join-Path $env:USERPROFILE '.cargo\bin'
if ((Test-Path -LiteralPath (Join-Path $cargoBin 'cargo.exe')) -and (($env:Path -split ';') -notcontains $cargoBin)) {
    $env:Path = "$cargoBin;$env:Path"
}
