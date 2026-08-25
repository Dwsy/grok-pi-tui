# grok-pi installer (Windows PowerShell)
#
# One-line install (latest):
#   irm https://github.com/Dwsy/grok-pi/releases/latest/download/install.ps1 | iex
#
# Pin a version:
#   $env:GROK_PI_VERSION='v0.0.1'; irm https://github.com/Dwsy/grok-pi/releases/download/v0.0.1/install.ps1 | iex
#
# Env overrides:
#   $env:GROK_PI_VERSION = 'v0.0.1' | 'latest'
#   $env:GROK_PI_INSTALL_DIR = "$env:LOCALAPPDATA\grok-pi\bin"
#   $env:GROK_PI_REPO = 'Dwsy/grok-pi'
#   $env:GROK_PI_SKIP_PI_HINT = '1'
#   $env:GROK_PI_FORCE = '1'
#
# Supported release assets:
#   grok-pi-windows-x86_64.zip
#   grok-pi-windows-aarch64.zip
$ErrorActionPreference = 'Stop'

function Write-Info([string]$Message) {
    Write-Host $Message
}

function Fail([string]$Message) {
    throw $Message
}

function Get-OsArchitectureName {
    $arch = [System.Runtime.InteropServices.RuntimeInformation]::OSArchitecture
    switch ($arch) {
        ([System.Runtime.InteropServices.Architecture]::X64) { return 'x86_64' }
        ([System.Runtime.InteropServices.Architecture]::Arm64) { return 'aarch64' }
        default { Fail "Unsupported Windows architecture: $arch (need x64 or arm64)." }
    }
}

function Resolve-AssetName([string]$ArchName) {
    return "grok-pi-windows-$ArchName.zip"
}

function Resolve-DownloadUrl([string]$Repository, [string]$Version, [string]$Asset) {
    if ($Version -eq 'latest') {
        return "https://github.com/$Repository/releases/latest/download/$Asset"
    }
    if ($Version -match '^v\d') {
        return "https://github.com/$Repository/releases/download/$Version/$Asset"
    }
    if ($Version -match '^\d') {
        return "https://github.com/$Repository/releases/download/v$Version/$Asset"
    }
    Fail "GROK_PI_VERSION must be 'latest', 'vX.Y.Z', or 'X.Y.Z' (got: $Version)."
}

function Ensure-UserPathContains([string]$Dir) {
    $userPath = [Environment]::GetEnvironmentVariable('Path', 'User')
    if ([string]::IsNullOrEmpty($userPath)) {
        [Environment]::SetEnvironmentVariable('Path', $Dir, 'User')
    } elseif (($userPath -split ';' | ForEach-Object { $_.TrimEnd('\') }) -notcontains $Dir.TrimEnd('\')) {
        [Environment]::SetEnvironmentVariable('Path', "$userPath;$Dir", 'User')
    }

    $parts = @()
    if (-not [string]::IsNullOrEmpty($env:Path)) {
        $parts = $env:Path -split ';'
    }
    $normalized = $parts | ForEach-Object { $_.TrimEnd('\') }
    if ($normalized -notcontains $Dir.TrimEnd('\')) {
        $env:Path = if ([string]::IsNullOrEmpty($env:Path)) { $Dir } else { "$env:Path;$Dir" }
    }
}

function Find-PiCmd {
    $candidates = @()
    $cmd = Get-Command pi -ErrorAction SilentlyContinue
    if ($cmd) {
        # Prefer .cmd over .ps1 for CreateProcess-friendly hosts.
        if ($cmd.Source -like '*.ps1' -or $cmd.Source -like '*.cmd') {
            $dir = Split-Path -Parent $cmd.Source
            $siblingCmd = Join-Path $dir 'pi.cmd'
            if (Test-Path -LiteralPath $siblingCmd) {
                return $siblingCmd
            }
        }
        return $cmd.Source
    }

    $localPi = Join-Path $env:LOCALAPPDATA 'pi-node\current\pi.cmd'
    if (Test-Path -LiteralPath $localPi) {
        return $localPi
    }
    $npmPi = Join-Path $env:APPDATA 'npm\pi.cmd'
    if (Test-Path -LiteralPath $npmPi) {
        return $npmPi
    }
    return $null
}

function Write-PiHostHint {
    if ($env:GROK_PI_SKIP_PI_HINT -eq '1') {
        return
    }
    Write-Info ''
    $pi = Find-PiCmd
    if ($pi) {
        Write-Info "Pi host found: $pi"
        Write-Info "If bare 'grok-pi' still fails with old binaries, use:"
        Write-Info "  `$env:PI_BIN = '$pi'"
        Write-Info "  grok-pi"
        Write-Info "  # or: grok-pi --pi-bin `"$pi`""
        return
    }
    Write-Info 'Pi host not found (required: Pi >= 0.84.3).'
    Write-Info 'Install Pi (recommended):'
    Write-Info '  powershell -c "irm https://pi.dev/install.ps1 | iex"'
    Write-Info 'Or:'
    Write-Info '  npm install --global @earendil-works/pi-coding-agent'
}

# ── main ────────────────────────────────────────────────────────────────────

$repository = if ($env:GROK_PI_REPO) { $env:GROK_PI_REPO } else { 'Dwsy/grok-pi' }
$version = if ($env:GROK_PI_VERSION) { $env:GROK_PI_VERSION } else { 'latest' }
$installDir = if ($env:GROK_PI_INSTALL_DIR) {
    $env:GROK_PI_INSTALL_DIR
} else {
    Join-Path $env:LOCALAPPDATA 'grok-pi\bin'
}
$force = ($env:GROK_PI_FORCE -eq '1')

$archName = Get-OsArchitectureName
$asset = Resolve-AssetName $archName
$url = Resolve-DownloadUrl -Repository $repository -Version $version -Asset $asset
$target = Join-Path $installDir 'grok-pi.exe'

Write-Info 'grok-pi installer'
Write-Info "  repo:    $repository"
Write-Info "  version: $version"
Write-Info "  asset:   $asset"
Write-Info "  install: $target"
Write-Info ''

if ((Test-Path -LiteralPath $target) -and -not $force) {
    try {
        $existing = & $target --version 2>$null
        if ($existing) {
            Write-Info "Existing install: $existing"
            Write-Info 'Reinstall with $env:GROK_PI_FORCE=1 if needed.'
        }
    } catch {
        # ignore probe failures for older/corrupt installs
    }
}

New-Item -ItemType Directory -Force -Path $installDir | Out-Null
$tempRoot = Join-Path ([System.IO.Path]::GetTempPath()) ("grok-pi-install-" + [guid]::NewGuid().ToString('N'))
New-Item -ItemType Directory -Force -Path $tempRoot | Out-Null

try {
    $archive = Join-Path $tempRoot $asset
    Write-Info "Downloading $url ..."
    try {
        Invoke-WebRequest -Uri $url -OutFile $archive
    } catch {
        Fail "download failed for $asset ($version). Check that this platform asset exists on the release. $_"
    }

    Expand-Archive -Path $archive -DestinationPath $tempRoot -Force

    $binary = Join-Path $tempRoot 'grok-pi.exe'
    if (-not (Test-Path -LiteralPath $binary)) {
        # Some archives nest one directory — search one level.
        $found = Get-ChildItem -Path $tempRoot -Filter 'grok-pi.exe' -Recurse -ErrorAction SilentlyContinue |
            Select-Object -First 1
        if (-not $found) {
            Fail 'archive did not contain grok-pi.exe'
        }
        $binary = $found.FullName
    }

    Copy-Item -LiteralPath $binary -Destination $target -Force
} finally {
    Remove-Item -LiteralPath $tempRoot -Recurse -Force -ErrorAction SilentlyContinue
}

Ensure-UserPathContains $installDir

# Convenience copy alias for muscle memory (hard link if possible, else copy).
$alias = Join-Path $installDir 'pi-grok.exe'
try {
    if (Test-Path -LiteralPath $alias) {
        Remove-Item -LiteralPath $alias -Force -ErrorAction SilentlyContinue
    }
    New-Item -ItemType HardLink -Path $alias -Target $target -ErrorAction Stop | Out-Null
} catch {
    Copy-Item -LiteralPath $target -Destination $alias -Force
}

Write-Info ''
Write-Info "Installed $target (alias: pi-grok.exe)"
Write-Info 'PATH updated for this session and User env (new terminals inherit User PATH).'

Write-PiHostHint

Write-Info ''
Write-Info 'Run:'
Write-Info '  grok-pi'
Write-Info '  # or: pi-grok'
Write-Info '  # continue: grok-pi --continue'
Write-Info '  # custom Pi host: grok-pi --pi-bin C:\path\to\pi.cmd'
