[CmdletBinding()]
param([string]$Fxc)
$ErrorActionPreference = 'Stop'
$workspace = (Resolve-Path (Join-Path $PSScriptRoot '..')).Path
if (-not $Fxc) {
    $sdkBin = Join-Path ${env:ProgramFiles(x86)} 'Windows Kits\10\bin'
    $Fxc = Get-ChildItem -LiteralPath $sdkBin -Directory |
        Where-Object Name -Match '^10\.\d+\.\d+\.\d+$' |
        Sort-Object { [version]$_.Name } -Descending |
        ForEach-Object { Join-Path $_.FullName 'x64\fxc.exe' } |
        Where-Object { Test-Path -LiteralPath $_ -PathType Leaf } | Select-Object -First 1
}
if (-not $Fxc) { throw 'Install the Windows SDK shader compiler or pass -Fxc.' }
Push-Location $workspace
try {
    # Relative source names and stripped reflection/debug data keep build-machine
    # paths out of the embedded bytecode. Normal app builds need no compiler DLL.
    & $Fxc /nologo /T cs_5_0 /E main /O3 /Ges /Qstrip_debug /Qstrip_reflect /Fo src/capture/shaders/area_average.cso src/capture/shaders/area_average.hlsl
    if ($LASTEXITCODE -ne 0) { throw 'Capture shader compilation failed.' }
} finally { Pop-Location }
