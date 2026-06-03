#Requires -Version 5.1
<#
.SYNOPSIS
    Installs the latest dbt-language-server release on Windows.
.DESCRIPTION
    Downloads the appropriate binary from GitHub releases and installs it to
    $env:LOCALAPPDATA\Programs\dbt-language-server, then offers to add that
    directory to the user's PATH.
.EXAMPLE
    irm https://raw.githubusercontent.com/j-clemons/dbt-language-server/main/install.ps1 | iex
#>
[CmdletBinding()]
param()

Set-StrictMode -Version Latest
$ErrorActionPreference = 'Stop'

$Repo    = 'j-clemons/dbt-language-server'
$ApiUrl  = "https://api.github.com/repos/$Repo/releases/latest"

# Detect architecture
$arch = switch ($env:PROCESSOR_ARCHITECTURE) {
    'AMD64' { 'amd64' }
    'ARM64' { 'arm64' }
    default {
        Write-Error "Unsupported architecture: $env:PROCESSOR_ARCHITECTURE. Only AMD64 and ARM64 are supported."
        exit 1
    }
}

$BinaryName  = "dbt-language-server-windows-$arch.exe"
$InstallDir  = Join-Path $env:LOCALAPPDATA 'Programs\dbt-language-server'
$InstallPath = Join-Path $InstallDir 'dbt-language-server.exe'

Write-Host "Installing $BinaryName to $InstallPath..."

# Fetch latest release metadata
try {
    $Release = Invoke-RestMethod -Uri $ApiUrl -UseBasicParsing
} catch {
    Write-Error "Failed to fetch release information from GitHub: $_"
    exit 1
}

# Find the matching asset
$Asset = $Release.assets | Where-Object { $_.name -eq $BinaryName } | Select-Object -First 1
if (-not $Asset) {
    Write-Error "Binary '$BinaryName' not found in the latest release."
    exit 1
}

$DownloadUrl = $Asset.browser_download_url

# Create install directory
if (-not (Test-Path $InstallDir)) {
    New-Item -ItemType Directory -Path $InstallDir | Out-Null
}

# Download the binary
$TempFile = [System.IO.Path]::GetTempFileName() + '.exe'
try {
    Invoke-WebRequest -Uri $DownloadUrl -OutFile $TempFile -UseBasicParsing
} catch {
    Write-Error "Failed to download binary: $_"
    exit 1
}

# Move to install location
Move-Item -Force -Path $TempFile -Destination $InstallPath

Write-Host "Installation complete: $InstallPath"

# Offer to add install directory to the user PATH if not already present
$UserPath = [Environment]::GetEnvironmentVariable('PATH', 'User')
if ($UserPath -split ';' -notcontains $InstallDir) {
    $AddToPath = Read-Host "Add '$InstallDir' to your user PATH? [Y/n]"
    if ($AddToPath -eq '' -or $AddToPath -match '^[Yy]') {
        $NewPath = ($UserPath.TrimEnd(';') + ';' + $InstallDir).TrimStart(';')
        [Environment]::SetEnvironmentVariable('PATH', $NewPath, 'User')
        $env:PATH = $env:PATH.TrimEnd(';') + ';' + $InstallDir
        Write-Host "PATH updated. Restart your terminal for the change to take effect."
    }
} else {
    Write-Host "'$InstallDir' is already in your PATH."
}
