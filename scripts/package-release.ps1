<#
.SYNOPSIS
    Packages one release archive for pmpx (Windows).

.DESCRIPTION
        scripts/package-release.ps1 -Target <triple> [-OutDir dist]

    writes

        dist/pmpx-<target>.zip

    The archive holds exactly two entries, both at the top level: the binary
    (`pmpx.exe`) and `LICENSE`. The layout is an interface, not a preference --
    `pmpx self update` opens the archive for its own target and picks the binary out
    of it by name, and MIT requires the licence notice to travel with a copy of the
    binary anyway.

    The name is an interface too: `pmpx-<target>` with `.zip` on Windows and
    `.tar.gz` on Unix is what `self update` and the install script build their URLs
    from.

    This script is what CI runs, so a release can be reproduced locally with one
    command.

.PARAMETER Target
    The target triple to build for, such as x86_64-pc-windows-msvc.

.PARAMETER OutDir
    Where the archive goes. Defaults to `dist`.
#>
[CmdletBinding()]
param(
    [Parameter(Mandatory = $true)]
    [string]$Target,

    [string]$OutDir = 'dist'
)

$ErrorActionPreference = 'Stop'

$root = Split-Path -Parent $PSScriptRoot

# `--locked`: a release is built from the committed lockfile, so the same tag yields the
# same dependency set no matter who builds it.
cargo build --release --locked --manifest-path "$root/Cargo.toml" --target $Target
if ($LASTEXITCODE -ne 0) {
    throw "cargo build failed (exit code $LASTEXITCODE)"
}

$binary = Join-Path $root "target/$Target/release/pmpx.exe"
if (-not (Test-Path $binary)) {
    throw "no executable at $binary"
}

New-Item -ItemType Directory -Force -Path $OutDir | Out-Null
$archive = Join-Path $OutDir "pmpx-$Target.zip"

# Staged in a temporary directory, then compressed with a wildcard so the entries sit at
# the archive root instead of inside a directory named after the staging path.
$staging = Join-Path ([System.IO.Path]::GetTempPath()) "pmpx-package-$([guid]::NewGuid())"
New-Item -ItemType Directory -Force -Path $staging | Out-Null
try {
    Copy-Item $binary (Join-Path $staging 'pmpx.exe')
    Copy-Item (Join-Path $root 'LICENSE') (Join-Path $staging 'LICENSE')

    Compress-Archive -Path (Join-Path $staging '*') -DestinationPath $archive -Force
}
finally {
    Remove-Item $staging -Recurse -Force -ErrorAction SilentlyContinue
}

Write-Output $archive
