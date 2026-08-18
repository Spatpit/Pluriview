[CmdletBinding()]
param()

$ErrorActionPreference = "Stop"

$workspace = (Resolve-Path (Join-Path $PSScriptRoot "..")).Path
$vendorDirectory = Join-Path $workspace "vendor"
$runtimePath = Join-Path $vendorDirectory "libmpv-2.dll"
$archivePath = Join-Path $vendorDirectory "mpv-dev-x86_64.7z"
$downloadUrl = "https://github.com/shinchiro/mpv-winbuild-cmake/releases/download/20260610/mpv-dev-x86_64-v3-20260610-git-304426c.7z"
$expectedSha256 = "D4B3D6DF9FDB33D5591C4ECE7D0CC24D2F7822B298F6A1528595E0CCFF7424A6"
$expectedRuntimeSha256 = "ADE5CAC46CFC397A3D5CD356A968CDA7ACF0DEBFFB705A16509DAFDF93029F5E"

if (Test-Path -LiteralPath $runtimePath -PathType Leaf) {
    $actualRuntimeSha256 = (Get-FileHash -Algorithm SHA256 -LiteralPath $runtimePath).Hash
    if ($actualRuntimeSha256 -ne $expectedRuntimeSha256) {
        throw "Cached libmpv checksum mismatch. Expected $expectedRuntimeSha256, got $actualRuntimeSha256."
    }
    Write-Host "libmpv is already available: $runtimePath"
    exit 0
}

New-Item -ItemType Directory -Path $vendorDirectory -Force | Out-Null

try {
    Write-Host "Downloading the pinned libmpv runtime..."
    Invoke-WebRequest -Uri $downloadUrl -OutFile $archivePath

    $actualSha256 = (Get-FileHash -Algorithm SHA256 -LiteralPath $archivePath).Hash
    if ($actualSha256 -ne $expectedSha256) {
        throw "libmpv archive checksum mismatch. Expected $expectedSha256, got $actualSha256."
    }

    & tar -xf $archivePath -C $vendorDirectory "libmpv-2.dll"
    if ($LASTEXITCODE -ne 0 -or -not (Test-Path -LiteralPath $runtimePath -PathType Leaf)) {
        throw "Could not extract libmpv-2.dll from the downloaded archive."
    }

    $actualRuntimeSha256 = (Get-FileHash -Algorithm SHA256 -LiteralPath $runtimePath).Hash
    if ($actualRuntimeSha256 -ne $expectedRuntimeSha256) {
        throw "Extracted libmpv checksum mismatch. Expected $expectedRuntimeSha256, got $actualRuntimeSha256."
    }

    Write-Host "Prepared libmpv runtime: $runtimePath"
} finally {
    if (Test-Path -LiteralPath $archivePath -PathType Leaf) {
        Remove-Item -LiteralPath $archivePath -Force
    }
}
