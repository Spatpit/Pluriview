[CmdletBinding()]
param(
    [switch]$ExecutableOnly
)

$ErrorActionPreference = "Stop"

# The workstation harness uses ExecutableOnly; it must never enter release ZIPs.
if (-not $ExecutableOnly -and $env:PLURIVIEW_LOCAL_PERFORMANCE -eq "1") {
    throw "Release packaging requires PLURIVIEW_LOCAL_PERFORMANCE to be unset. Local performance builds are executable-only."
}

$workspace = (Resolve-Path (Join-Path $PSScriptRoot "..")).Path
$sysroot = (& rustc --print sysroot).Trim()
$rustupMarker = "$([IO.Path]::DirectorySeparatorChar).rustup$([IO.Path]::DirectorySeparatorChar)"
$rustupIndex = $sysroot.IndexOf($rustupMarker, [StringComparison]::OrdinalIgnoreCase)

if ($rustupIndex -lt 0) {
    throw "Could not derive the Rust toolchain directory from rustc --print sysroot."
}

$profileRoot = $sysroot.Substring(0, $rustupIndex)
$rustupHome = Join-Path $profileRoot ".rustup"
$cargoHome = if ($env:CARGO_HOME) {
    [IO.Path]::GetFullPath($env:CARGO_HOME)
} else {
    Join-Path $profileRoot ".cargo"
}

$separator = [char]0x1f
$previousRustFlags = $env:CARGO_ENCODED_RUSTFLAGS
$rustFlags = @()

if ($previousRustFlags) {
    $rustFlags += $previousRustFlags -split [string]$separator
}

$rustFlags += @(
    "--remap-path-prefix=$workspace=<workspace>",
    "--remap-path-prefix=$cargoHome=<cargo-home>",
    "--remap-path-prefix=$rustupHome=<rustup-home>"
)

$credentialPatterns = @(
    "-----BEGIN (?:RSA |EC |OPENSSH |DSA )?PRIVATE KEY-----",
    "(?:AKIA|ASIA)[A-Z0-9]{16}",
    "gh[pousr]_[A-Za-z0-9]{30,}",
    "github_pat_[A-Za-z0-9_]{20,}",
    "sk-(?:proj-|svcacct-)?[A-Za-z0-9_-]{20,}",
    "AIza[0-9A-Za-z_-]{35}",
    "xox[baprs]-[0-9A-Za-z-]{10,}",
    "[sr]k_live_[A-Za-z0-9]{16,}",
    "eyJ[A-Za-z0-9_-]{10,}\.[A-Za-z0-9_-]{10,}\.[A-Za-z0-9_-]{10,}"
)

function Assert-PrivacySafeBinary {
    param(
        [Parameter(Mandatory)] [string] $Path,
        [Parameter(Mandatory)] [string[]] $ForbiddenText,
        [Parameter(Mandatory)] [string[]] $CredentialPatterns
    )

    $bytes = [IO.File]::ReadAllBytes($Path)
    # Windows PowerShell 5.1 has no Encoding::Latin1; ISO-8859-1 is the same 1:1 mapping.
    $ascii = [Text.Encoding]::GetEncoding(28591).GetString($bytes)
    $utf16 = [Text.Encoding]::Unicode.GetString($bytes)

    foreach ($value in $ForbiddenText) {
        if ([string]::IsNullOrEmpty($value)) {
            continue
        }
        if ($ascii.IndexOf($value, [StringComparison]::OrdinalIgnoreCase) -ge 0 -or
            $utf16.IndexOf($value, [StringComparison]::OrdinalIgnoreCase) -ge 0) {
            throw "Release privacy check failed: $(Split-Path -Leaf $Path) contains a local or internal path."
        }
    }

    foreach ($pattern in $CredentialPatterns) {
        if ([regex]::IsMatch($ascii, $pattern) -or [regex]::IsMatch($utf16, $pattern)) {
            throw "Release privacy check failed: $(Split-Path -Leaf $Path) contains a credential-like string."
        }
    }
}

Push-Location $workspace
try {
    $env:CARGO_ENCODED_RUSTFLAGS = $rustFlags -join $separator
    & cargo build --release --locked
    if ($LASTEXITCODE -ne 0) {
        throw "cargo build --release --locked failed with exit code $LASTEXITCODE."
    }

    $executable = Join-Path $workspace "target\release\pluriview.exe"
    $forbiddenText = @(
        $workspace,
        $profileRoot,
        $cargoHome,
        $rustupHome,
        "CLAUDE_CONTEXT.md",
        "docs\superpowers",
        "docs/superpowers"
    )
    Assert-PrivacySafeBinary -Path $executable -ForbiddenText $forbiddenText -CredentialPatterns $credentialPatterns

    $hash = (Get-FileHash -Algorithm SHA256 -LiteralPath $executable).Hash
    if ($ExecutableOnly) {
        Write-Host "Privacy-safe release executable: $executable"
        Write-Host "Executable SHA-256: $hash"
        Write-Warning "Do not publish pluriview.pdb; debug symbols can contain local source paths."
        return
    }

    $distDirectory = Join-Path $workspace "dist"
    $distExecutable = Join-Path $distDirectory "pluriview.exe"
    $distLibDirectory = Join-Path $distDirectory "lib"
    $libmpvSource = Join-Path $workspace "vendor\libmpv-2.dll"
    $distLibmpv = Join-Path $distLibDirectory "libmpv-2.dll"
    $eglSource = Join-Path $workspace "vendor\libEGL.dll"
    $glesSource = Join-Path $workspace "vendor\libGLESv2.dll"
    $distEgl = Join-Path $distLibDirectory "libEGL.dll"
    $distGles = Join-Path $distLibDirectory "libGLESv2.dll"
    $licensePath = Join-Path $workspace "LICENSE"
    $noticesPath = Join-Path $workspace "THIRD_PARTY_NOTICES.md"
    $angleLicensesPath = Join-Path $workspace "assets\third_party\angle\ANGLE_LICENSES.txt"
    $apacheLicensePath = Join-Path $workspace "assets\third_party\angle\APACHE-2.0.txt"
    $distLicense = Join-Path $distLibDirectory "LICENSE.txt"
    $distNotices = Join-Path $distLibDirectory "THIRD_PARTY_NOTICES.txt"
    $distAngleLicenses = Join-Path $distLibDirectory "ANGLE_LICENSES.txt"
    $distApacheLicense = Join-Path $distLibDirectory "APACHE-2.0.txt"
    if (-not (Test-Path -LiteralPath $libmpvSource -PathType Leaf)) {
        throw "The libmpv runtime is missing. Run .\scripts\prepare-libmpv.ps1, then build again."
    }
    if (-not (Test-Path -LiteralPath $eglSource -PathType Leaf) -or
        -not (Test-Path -LiteralPath $glesSource -PathType Leaf)) {
        throw "The ANGLE runtime is missing. Run .\scripts\prepare-angle.ps1, then build again."
    }
    Assert-PrivacySafeBinary -Path $eglSource -ForbiddenText $forbiddenText -CredentialPatterns $credentialPatterns
    Assert-PrivacySafeBinary -Path $glesSource -ForbiddenText $forbiddenText -CredentialPatterns $credentialPatterns
    New-Item -ItemType Directory -Path $distLibDirectory -Force | Out-Null
    Copy-Item -LiteralPath $executable -Destination $distExecutable -Force
    Copy-Item -LiteralPath $libmpvSource -Destination $distLibmpv -Force
    Copy-Item -LiteralPath $eglSource -Destination $distEgl -Force
    Copy-Item -LiteralPath $glesSource -Destination $distGles -Force
    Copy-Item -LiteralPath $licensePath -Destination $distLicense -Force
    Copy-Item -LiteralPath $noticesPath -Destination $distNotices -Force
    Copy-Item -LiteralPath $angleLicensesPath -Destination $distAngleLicenses -Force
    Copy-Item -LiteralPath $apacheLicensePath -Destination $distApacheLicense -Force

    # Remove files created by the previous flat package layout. These exact
    # generated paths are safe to delete and keeping them would make upgrades
    # look as though both layouts were required.
    foreach ($leafName in @(
        "libmpv-2.dll",
        "libEGL.dll",
        "libGLESv2.dll",
        "LICENSE",
        "THIRD_PARTY_NOTICES.md",
        "ANGLE_LICENSES.txt",
        "APACHE-2.0.txt"
    )) {
        $legacyPath = Join-Path $distDirectory $leafName
        if (Test-Path -LiteralPath $legacyPath -PathType Leaf) {
            Remove-Item -LiteralPath $legacyPath -Force
        }
    }

    $libmpvHash = (Get-FileHash -Algorithm SHA256 -LiteralPath $distLibmpv).Hash
    $eglHash = (Get-FileHash -Algorithm SHA256 -LiteralPath $distEgl).Hash
    $glesHash = (Get-FileHash -Algorithm SHA256 -LiteralPath $distGles).Hash
    $expectedLibmpvHash = "ADE5CAC46CFC397A3D5CD356A968CDA7ACF0DEBFFB705A16509DAFDF93029F5E"
    $expectedEglHash = "FFC4565AD5839AC9FE6719D399B7E90CF5C7984E609D7876DE1D9FA9BF9EDAF3"
    $expectedGlesHash = "CB302095BACCBEC3BD30184A837B342E3260970C9934558762A5415FB85D12BA"
    if ($eglHash -ne $expectedEglHash -or $glesHash -ne $expectedGlesHash) {
        throw "The ANGLE runtime does not match the pinned standalone build. Run .\scripts\prepare-angle.ps1, then build again."
    }
    if ($libmpvHash -ne $expectedLibmpvHash) {
        throw "The libmpv runtime does not match the pinned build. Run .\scripts\prepare-libmpv.ps1, then build again."
    }

    $metadataJson = & cargo metadata --locked --no-deps --format-version 1
    if ($LASTEXITCODE -ne 0) {
        throw "cargo metadata failed with exit code $LASTEXITCODE."
    }
    $metadata = $metadataJson | ConvertFrom-Json
    $releaseVersion = ($metadata.packages | Where-Object { $_.name -eq "pluriview" } | Select-Object -First 1).version
    if ([string]::IsNullOrWhiteSpace($releaseVersion)) {
        throw "Could not determine the Pluriview package version."
    }

    $fullArchive = Join-Path $distDirectory "Pluriview-v$releaseVersion-windows-x64-full.zip"
    # Remove this version's obsolete Lite output when rebuilding an existing dist.
    $legacyLiteArchive = Join-Path $distDirectory "Pluriview-v$releaseVersion-windows-x64-lite.zip"
    $checksumManifest = Join-Path $distDirectory "SHA256SUMS.txt"
    $fullStageDirectory = Join-Path $distDirectory ".full-package-stage"

    foreach ($path in @($fullArchive, $legacyLiteArchive, $checksumManifest)) {
        if (Test-Path -LiteralPath $path) {
            Remove-Item -LiteralPath $path -Force
        }
    }

    foreach ($stageDirectory in @($fullStageDirectory)) {
        if (Test-Path -LiteralPath $stageDirectory) {
            $resolvedStage = (Resolve-Path -LiteralPath $stageDirectory).Path
            $expectedStage = [IO.Path]::GetFullPath($stageDirectory)
            if (-not $resolvedStage.Equals($expectedStage, [StringComparison]::OrdinalIgnoreCase) -or
                -not $resolvedStage.StartsWith(
                    [IO.Path]::GetFullPath($distDirectory) + [IO.Path]::DirectorySeparatorChar,
                    [StringComparison]::OrdinalIgnoreCase
                )) {
                throw "Refusing to clean unexpected package staging directory: $resolvedStage"
            }
            Remove-Item -LiteralPath $resolvedStage -Recurse -Force
        }
    }

    try {
        $fullStageLib = Join-Path $fullStageDirectory "lib"
        New-Item -ItemType Directory -Path $fullStageLib -Force | Out-Null

        Copy-Item -LiteralPath $distExecutable -Destination $fullStageDirectory
        foreach ($runtimeFile in @(
            $distEgl,
            $distGles,
            $distLicense,
            $distNotices,
            $distAngleLicenses,
            $distApacheLicense
        )) {
            Copy-Item -LiteralPath $runtimeFile -Destination $fullStageLib
        }
        Copy-Item -LiteralPath $distLibmpv -Destination $fullStageLib

        Compress-Archive -LiteralPath @(
            (Join-Path $fullStageDirectory "pluriview.exe"),
            $fullStageLib
        ) -DestinationPath $fullArchive -CompressionLevel Optimal
    } finally {
        foreach ($stageDirectory in @($fullStageDirectory)) {
            if (Test-Path -LiteralPath $stageDirectory) {
                $resolvedStage = (Resolve-Path -LiteralPath $stageDirectory).Path
                $expectedStage = [IO.Path]::GetFullPath($stageDirectory)
                if ($resolvedStage.Equals($expectedStage, [StringComparison]::OrdinalIgnoreCase) -and
                    $resolvedStage.StartsWith(
                        [IO.Path]::GetFullPath($distDirectory) + [IO.Path]::DirectorySeparatorChar,
                        [StringComparison]::OrdinalIgnoreCase
                    )) {
                    Remove-Item -LiteralPath $resolvedStage -Recurse -Force
                }
            }
        }
    }

    $publishedAssets = @($fullArchive)
    $checksumLines = foreach ($asset in $publishedAssets) {
        $assetHash = (Get-FileHash -Algorithm SHA256 -LiteralPath $asset).Hash.ToLowerInvariant()
        "$assetHash  $(Split-Path -Leaf $asset)"
    }
    $utf8NoBom = New-Object Text.UTF8Encoding($false)
    [IO.File]::WriteAllLines($checksumManifest, [string[]]$checksumLines, $utf8NoBom)

    Write-Host "Privacy-safe release executable: $executable"
    Write-Host "Persistent release executable: $distExecutable"
    Write-Host "Executable SHA-256: $hash"
    Write-Host "libmpv SHA-256: $libmpvHash"
    Write-Host "libEGL SHA-256: $eglHash"
    Write-Host "libGLESv2 SHA-256: $glesHash"
    Write-Host "Full release: $fullArchive"
    Write-Host "Checksums: $checksumManifest"
    Write-Warning "Publish the Full zip archive and SHA256SUMS.txt. Do not publish pluriview.pdb; debug symbols can contain local source paths."
} finally {
    if ($null -eq $previousRustFlags) {
        Remove-Item Env:CARGO_ENCODED_RUSTFLAGS -ErrorAction SilentlyContinue
    } else {
        $env:CARGO_ENCODED_RUSTFLAGS = $previousRustFlags
    }
    Pop-Location
}
