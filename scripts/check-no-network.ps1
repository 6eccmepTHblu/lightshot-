# Fails when the release executable links or names a Windows networking library.
param([string] $Executable = (Join-Path $PSScriptRoot '..\target\release\rustshot.exe'))

$ErrorActionPreference = 'Stop'

$bannedLibraries = @(
    'dnsapi', 'iphlpapi', 'mswsock', 'ncrypt', 'rasapi32', 'secur32', 'urlmon',
    'webio', 'websocket', 'winhttp', 'wininet', 'wsock32', 'ws2_32'
)

if (!(Test-Path -LiteralPath $Executable -PathType Leaf)) {
    throw "Executable not found: $Executable"
}

$vswhere = Join-Path ${env:ProgramFiles(x86)} 'Microsoft Visual Studio\Installer\vswhere.exe'
$installation = & $vswhere -latest -products * `
    -requires Microsoft.VisualStudio.Component.VC.Tools.x86.x64 -property installationPath
$toolset = Get-ChildItem (Join-Path $installation 'VC\Tools\MSVC') |
    Sort-Object Name -Descending | Select-Object -First 1
$dumpbin = Join-Path $toolset.FullName 'bin\Hostx64\x64\dumpbin.exe'

$linked = & $dumpbin /nologo /dependents $Executable |
    Where-Object { $_ -match '\.dll\s*$' } | ForEach-Object { $_.Trim().ToLowerInvariant() }
if (!$linked) {
    throw 'dumpbin reported no linked libraries, so the check proved nothing'
}

$bytes = [System.IO.File]::ReadAllBytes($Executable)
$ascii = [System.Text.Encoding]::ASCII.GetString($bytes).ToLowerInvariant()
$utf16 = [System.Text.Encoding]::Unicode.GetString($bytes).ToLowerInvariant()

$problems = @()
foreach ($library in $bannedLibraries) {
    if ($linked -contains "$library.dll") {
        $problems += "links $library.dll"
    }
    # Catches LoadLibrary by name, which never appears in the import table.
    if ($ascii.Contains("$library.dll") -or $utf16.Contains("$library.dll")) {
        $problems += "names $library.dll"
    }
}

if ($problems) {
    throw "Network library check failed:`n$(($problems | Sort-Object -Unique) -join "`n")"
}
Write-Output "No network libraries in $Executable ($($linked.Count) linked DLLs checked)"
