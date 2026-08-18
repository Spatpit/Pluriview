[CmdletBinding()]
param()

$ErrorActionPreference = "Stop"

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

Push-Location $workspace
try {
    $env:CARGO_ENCODED_RUSTFLAGS = $rustFlags -join $separator
    & cargo build --release --locked
    if ($LASTEXITCODE -ne 0) {
        throw "cargo build --release --locked failed with exit code $LASTEXITCODE."
    }

    $executable = Join-Path $workspace "target\release\pluriview.exe"
    $bytes = [IO.File]::ReadAllBytes($executable)
    # Windows PowerShell 5.1 has no Encoding::Latin1; ISO-8859-1 is the same 1:1 mapping.
    $ascii = [Text.Encoding]::GetEncoding(28591).GetString($bytes)
    $utf16 = [Text.Encoding]::Unicode.GetString($bytes)

    $forbiddenText = @(
        $workspace,
        $profileRoot,
        $cargoHome,
        $rustupHome,
        "CLAUDE_CONTEXT.md",
        "docs\superpowers",
        "docs/superpowers"
    )

    foreach ($value in $forbiddenText) {
        if ([string]::IsNullOrEmpty($value)) {
            continue
        }
        if ($ascii.IndexOf($value, [StringComparison]::OrdinalIgnoreCase) -ge 0 -or
            $utf16.IndexOf($value, [StringComparison]::OrdinalIgnoreCase) -ge 0) {
            throw "Release privacy check failed: the executable contains a local or internal path."
        }
    }

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

    foreach ($pattern in $credentialPatterns) {
        if ([regex]::IsMatch($ascii, $pattern) -or [regex]::IsMatch($utf16, $pattern)) {
            throw "Release privacy check failed: the executable contains a credential-like string."
        }
    }

    $hash = (Get-FileHash -Algorithm SHA256 -LiteralPath $executable).Hash
    $distDirectory = Join-Path $workspace "dist"
    $distExecutable = Join-Path $distDirectory "pluriview.exe"
    $libmpvSource = Join-Path $workspace "vendor\libmpv-2.dll"
    $distLibmpv = Join-Path $distDirectory "libmpv-2.dll"
    if (-not (Test-Path -LiteralPath $libmpvSource -PathType Leaf)) {
        throw "The libmpv runtime is missing. Run .\scripts\prepare-libmpv.ps1, then build again."
    }
    New-Item -ItemType Directory -Path $distDirectory -Force | Out-Null
    Copy-Item -LiteralPath $executable -Destination $distExecutable -Force
    Copy-Item -LiteralPath $libmpvSource -Destination $distLibmpv -Force

    $libmpvHash = (Get-FileHash -Algorithm SHA256 -LiteralPath $distLibmpv).Hash

    Write-Host "Privacy-safe release executable: $executable"
    Write-Host "Persistent release executable: $distExecutable"
    Write-Host "Executable SHA-256: $hash"
    Write-Host "libmpv SHA-256: $libmpvHash"
    Write-Warning "Publish pluriview.exe together with libmpv-2.dll. Do not publish pluriview.pdb; debug symbols can contain local source paths."
} finally {
    if ($null -eq $previousRustFlags) {
        Remove-Item Env:CARGO_ENCODED_RUSTFLAGS -ErrorAction SilentlyContinue
    } else {
        $env:CARGO_ENCODED_RUSTFLAGS = $previousRustFlags
    }
    Pop-Location
}
