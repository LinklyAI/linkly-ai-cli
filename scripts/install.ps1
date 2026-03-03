# Linkly CLI installer for Windows
# Usage: irm https://updater.linkly.ai/cli/install.ps1 | iex
$ErrorActionPreference = 'Stop'

$LatestUrl = "https://updater.linkly.ai/cli/latest.json"
$DefaultInstallDir = Join-Path $env:LOCALAPPDATA "linkly\bin"
$InstallDir = if ($env:LINKLY_INSTALL_DIR) { $env:LINKLY_INSTALL_DIR } else { $DefaultInstallDir }
$BinaryName = "linkly.exe"
$PlatformKey = "windows-x86_64"

function Write-Info($msg) { Write-Host "[info] $msg" -ForegroundColor Cyan }
function Write-Ok($msg) { Write-Host "[ok] $msg" -ForegroundColor Green }
function Write-Err($msg) { Write-Host "[error] $msg" -ForegroundColor Red; exit 1 }

$TempDir = $null

try {
    Write-Info "Installing Linkly CLI..."
    Write-Info "Platform: $PlatformKey"

    # Fetch latest.json
    Write-Info "Fetching latest version info..."
    $latest = Invoke-RestMethod -Uri $LatestUrl

    # Extract download URL
    $downloadUrl = $latest.assets.$PlatformKey
    if (-not $downloadUrl) {
        Write-Err "No download URL found for platform: $PlatformKey"
    }
    Write-Info "Downloading from: $downloadUrl"

    # Create temp directory
    $TempDir = Join-Path ([System.IO.Path]::GetTempPath()) ("linkly-install-" + [System.IO.Path]::GetRandomFileName())
    New-Item -ItemType Directory -Path $TempDir -Force | Out-Null

    # Download zip
    $zipPath = Join-Path $TempDir "linkly.zip"
    Invoke-WebRequest -Uri $downloadUrl -OutFile $zipPath -UseBasicParsing

    # Extract
    Expand-Archive -Path $zipPath -DestinationPath $TempDir -Force

    # Install binary
    if (-not (Test-Path $InstallDir)) {
        New-Item -ItemType Directory -Path $InstallDir -Force | Out-Null
    }
    Copy-Item -Path (Join-Path $TempDir $BinaryName) -Destination (Join-Path $InstallDir $BinaryName) -Force

    # Add to user PATH (idempotent)
    $userPath = [Environment]::GetEnvironmentVariable("Path", "User")
    if ($userPath -notlike "*$InstallDir*") {
        $newPath = "$InstallDir;$userPath"
        [Environment]::SetEnvironmentVariable("Path", $newPath, "User")
        Write-Info "Added $InstallDir to user PATH"
    }

    # Update current session PATH
    if ($env:Path -notlike "*$InstallDir*") {
        $env:Path = "$InstallDir;$env:Path"
    }

    Write-Ok "Linkly CLI installed to $InstallDir\$BinaryName"
    Write-Host ""
    Write-Info "Run 'linkly --help' to get started."
    Write-Info "You may need to restart your terminal for PATH changes to take effect."
}
catch {
    Write-Err "Installation failed: $_"
}
finally {
    if ($TempDir -and (Test-Path $TempDir)) {
        Remove-Item -Path $TempDir -Recurse -Force -ErrorAction SilentlyContinue
    }
}
