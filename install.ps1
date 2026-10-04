<#
.SYNOPSIS
    Installs pmpx from a GitHub release (Windows).

.DESCRIPTION
        irm https://raw.githubusercontent.com/pmpx-rs/pmpx/main/install.ps1 | iex

    Environment:

      PMPX_INSTALL_DIR   where the binary goes          (default: %USERPROFILE%\.pmpx\bin)
      PMPX_VERSION       install this version           (default: the newest release)
      PMPX_BASE_URL      download from somewhere else   (default: the GitHub release page;
                         a mirror, and what the tests point at a local server)

    It writes exactly one file and touches nothing else: no PATH, no profile script, no
    package manager. If the directory it writes to is not on PATH, it says so and stops.

    The download is verified against the release's SHA256SUMS before anything is unpacked.
    So is the version it reports afterwards, which catches a mirror serving a stale archive
    under a newer tag.

    It ends with `throw` rather than `exit` on purpose: piped into `iex`, `exit` would close
    the shell the user is sitting in.
#>
[CmdletBinding()]
param()

$ErrorActionPreference = 'Stop'
Set-StrictMode -Version Latest

$Repo = 'pmpx-rs/pmpx'
$Binary = 'pmpx.exe'

# ---------------------------------------------------------------------------
# Which release asset is this machine?
# ---------------------------------------------------------------------------

function Get-Target {
    # PROCESSOR_ARCHITEW6432 is set when a 32-bit host is running the 64-bit PowerShell.
    $arch = if ($env:PROCESSOR_ARCHITEW6432) { $env:PROCESSOR_ARCHITEW6432 } else { $env:PROCESSOR_ARCHITECTURE }

    switch ($arch) {
        'AMD64' { 'x86_64-pc-windows-msvc' }
        default { throw "no prebuilt pmpx for Windows $arch; use 'cargo install pmpx'" }
    }
}

# ---------------------------------------------------------------------------
# Talking to the release page
# ---------------------------------------------------------------------------

# `WebClient` instead of `Invoke-WebRequest`: the cmdlet's behaviour around basic parsing
# differs between Windows PowerShell and PowerShell 7, and a download needs none of it.
function Save-Url {
    param([string]$Url, [string]$Destination)

    $client = New-Object System.Net.WebClient
    try {
        $client.DownloadFile($Url, $Destination)
    }
    finally {
        $client.Dispose()
    }
}

function Get-LatestVersion {
    $headers = @{ 'User-Agent' = 'pmpx-installer' }
    $release = Invoke-RestMethod -Uri "https://api.github.com/repos/$Repo/releases/latest" -Headers $headers
    $release.tag_name
}

# The hash listed for one file. GNU sha256sum writes `hash  name` in text mode and
# `hash *name` in binary mode, so both spellings are accepted -- the same rule the Rust side
# follows when it reads the same file.
function Get-ExpectedHash {
    param([string]$SumsPath, [string]$FileName)

    foreach ($line in Get-Content -Path $SumsPath) {
        $parts = -split $line
        if ($parts.Count -lt 2) { continue }
        if ($parts[1].TrimStart('*') -eq $FileName) { return $parts[0] }
    }

    return $null
}

# ---------------------------------------------------------------------------

$Target = Get-Target

$Version = if ($env:PMPX_VERSION) { $env:PMPX_VERSION } else { Get-LatestVersion }
if (-not $Version.StartsWith('v')) { $Version = "v$Version" }

$Base = if ($env:PMPX_BASE_URL) { $env:PMPX_BASE_URL } else { "https://github.com/$Repo/releases/download" }
$Archive = "pmpx-$Target.zip"
$Dir = if ($env:PMPX_INSTALL_DIR) { $env:PMPX_INSTALL_DIR } else { Join-Path $env:USERPROFILE '.pmpx\bin' }

$tmp = Join-Path ([System.IO.Path]::GetTempPath()) "pmpx-install-$([guid]::NewGuid())"
New-Item -ItemType Directory -Force -Path $tmp | Out-Null

try {
    Write-Host "Downloading $Base/$Version/$Archive"
    $archivePath = Join-Path $tmp $Archive
    Save-Url -Url "$Base/$Version/$Archive" -Destination $archivePath
    $sumsPath = Join-Path $tmp 'SHA256SUMS'
    Save-Url -Url "$Base/$Version/SHA256SUMS" -Destination $sumsPath

    $expected = Get-ExpectedHash -SumsPath $sumsPath -FileName $Archive
    if (-not $expected) { throw "SHA256SUMS does not list $Archive" }

    $actual = (Get-FileHash -Algorithm SHA256 -Path $archivePath).Hash.ToLower()
    if ($expected.ToLower() -ne $actual) {
        throw "checksum mismatch for $Archive`n  expected $expected`n  got      $actual`nNothing was installed."
    }

    Expand-Archive -Path $archivePath -DestinationPath $tmp -Force

    $unpacked = Join-Path $tmp $Binary
    if (-not (Test-Path $unpacked)) { throw "$Archive does not contain $Binary" }

    # Everything that can be checked is checked *before* anything lands in the install
    # directory: a mirror serving a stale archive under a newer tag should leave nothing
    # behind, not a working install of the wrong version.
    $downloaded = & $unpacked --version
    if ($LASTEXITCODE -ne 0) { throw "the downloaded $Binary does not run" }

    $number = ($downloaded -split ' ')[-1]
    if ($number -ne $Version.TrimStart('v')) {
        throw "$Version contains $number; nothing was installed"
    }

    New-Item -ItemType Directory -Force -Path $Dir | Out-Null
    $installed = Join-Path $Dir $Binary
    Copy-Item $unpacked $installed -Force

    $reported = & $installed --version
    if ($LASTEXITCODE -ne 0) { throw "$installed was installed but does not run" }

    Write-Host "$reported installed to $installed"

    $onPath = ($env:Path -split ';') -contains $Dir
    if (-not $onPath) {
        Write-Host ''
        Write-Host "$Dir is not on your PATH. Add it for this session with:"
        Write-Host "  `$env:Path = `"$Dir;`$env:Path`""
    }

    # Two pmpx on PATH is a confusing thing to debug, and the one that runs is whichever
    # comes first -- which is probably not the one just installed.
    $other = Get-Command pmpx -ErrorAction SilentlyContinue
    if ($other -and $other.Source -ne $installed) {
        Write-Host ''
        Write-Host "Note: another pmpx is on your PATH at $($other.Source); that one runs first."
    }
}
finally {
    Remove-Item -Path $tmp -Recurse -Force -ErrorAction SilentlyContinue
}
