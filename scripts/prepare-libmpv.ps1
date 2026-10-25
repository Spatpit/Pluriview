[CmdletBinding()]
param()

$ErrorActionPreference = "Stop"

$workspace = (Resolve-Path (Join-Path $PSScriptRoot "..")).Path
$vendorDirectory = Join-Path $workspace "vendor"
$runtimePath = Join-Path $vendorDirectory "libmpv-2.dll"
$archivePath = Join-Path $vendorDirectory "mpv-dev-x86_64.7z"
$downloadUrl = "https://downloads.sourceforge.net/project/mpv-player-windows/libmpv/mpv-dev-x86_64-v3-20261018-git-01e2533f31.7z"
$expectedSha256 = "49533E80D681672E42AB38FCCFDB9DB0CC9FC423F86D34F5F9634EBA71109912"
$expectedRuntimeSha256 = "5C5C9D094859BDC60B8B27B2D832DB8FA8C2E487EFBD35F5AEA5AFE6960DAD39"

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
    # SourceForge serves a browser download page unless the client looks like a
    # plain download tool.
    Invoke-WebRequest -Uri $downloadUrl -OutFile $archivePath -UserAgent "Wget"

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
