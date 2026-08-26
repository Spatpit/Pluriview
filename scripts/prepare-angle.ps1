[CmdletBinding()]
param()

$ErrorActionPreference = "Stop"

$workspace = (Resolve-Path (Join-Path $PSScriptRoot "..")).Path
$assetDirectory = Join-Path $workspace "assets\third_party\angle"
$vendorDirectory = Join-Path $workspace "vendor"

$runtimeFiles = @(
    [pscustomobject]@{
        Name = "libEGL.dll"
        Sha256 = "FFC4565AD5839AC9FE6719D399B7E90CF5C7984E609D7876DE1D9FA9BF9EDAF3"
    },
    [pscustomobject]@{
        Name = "libGLESv2.dll"
        Sha256 = "CB302095BACCBEC3BD30184A837B342E3260970C9934558762A5415FB85D12BA"
    }
)

function Assert-RuntimeHash {
    param(
        [Parameter(Mandatory)] [string] $Path,
        [Parameter(Mandatory)] [string] $ExpectedSha256,
        [Parameter(Mandatory)] [string] $Name
    )

    if (-not (Test-Path -LiteralPath $Path -PathType Leaf)) {
        throw "$Name is missing at $Path. Restore the pinned ANGLE asset from git."
    }

    $actualSha256 = (Get-FileHash -Algorithm SHA256 -LiteralPath $Path).Hash
    if ($actualSha256 -ne $ExpectedSha256) {
        throw "$Name checksum mismatch. Expected $ExpectedSha256, got $actualSha256."
    }
}

foreach ($runtime in $runtimeFiles) {
    $assetPath = Join-Path $assetDirectory $runtime.Name
    Assert-RuntimeHash -Path $assetPath -ExpectedSha256 $runtime.Sha256 -Name "Pinned $($runtime.Name)"
}

New-Item -ItemType Directory -Path $vendorDirectory -Force | Out-Null

foreach ($runtime in $runtimeFiles) {
    $assetPath = Join-Path $assetDirectory $runtime.Name
    $vendorPath = Join-Path $vendorDirectory $runtime.Name
    Copy-Item -LiteralPath $assetPath -Destination $vendorPath -Force
    Assert-RuntimeHash -Path $vendorPath -ExpectedSha256 $runtime.Sha256 -Name "Prepared $($runtime.Name)"
}

Write-Host "Prepared the pinned standalone ANGLE runtime in $vendorDirectory"
