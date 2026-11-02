# Fails when the release executable may resolve its imports from its own directory.
param([string] $Executable = (Join-Path $PSScriptRoot '..\target\release\rustshot.exe'))

$ErrorActionPreference = 'Stop'

if (!(Test-Path -LiteralPath $Executable -PathType Leaf)) {
    throw "Executable not found: $Executable"
}

$vswhere = Join-Path ${env:ProgramFiles(x86)} 'Microsoft Visual Studio\Installer\vswhere.exe'
$installation = & $vswhere -latest -products * `
    -requires Microsoft.VisualStudio.Component.VC.Tools.x86.x64 -property installationPath
$toolset = Get-ChildItem (Join-Path $installation 'VC\Tools\MSVC') |
    Sort-Object Name -Descending | Select-Object -First 1
$dumpbin = Join-Path $toolset.FullName 'bin\Hostx64\x64\dumpbin.exe'

$line = & $dumpbin /nologo /loadconfig $Executable | Select-String 'Dependent Load Flag'
if (!$line) {
    throw 'dumpbin reported no Dependent Load Flag, so the check proved nothing'
}
if ($line.Line.Trim() -notmatch '^0*800\s') {
    throw "Imports are not restricted to System32: $($line.Line.Trim())"
}
Write-Output "Imports restricted to System32 in $Executable"
