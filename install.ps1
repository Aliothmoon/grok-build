#!/usr/bin/env pwsh
# igrok installer (Windows).
# Downloads the latest igrok.exe from GitHub Releases into ~\.igrok\bin and
# adds it to the user PATH. No admin rights required; no other dependencies.
#
# Install:  irm https://raw.githubusercontent.com/Aliothmoon/grok-build/dev/install.ps1 | iex
$ErrorActionPreference = 'Stop'

$Repo = 'Aliothmoon/grok-build'
$Arch = switch ($env:PROCESSOR_ARCHITECTURE) {
    'AMD64' { 'x86_64' }
    'ARM64' { 'aarch64' }
    default { throw "Unsupported architecture: $env:PROCESSOR_ARCHITECTURE" }
}
$Pattern = "igrok-*-windows-$Arch.exe"

$BinDir = Join-Path $env:USERPROFILE '.igrok\bin'
New-Item -ItemType Directory -Force -Path $BinDir | Out-Null

Write-Host "Resolving latest release from $Repo ..."
$Release = Invoke-RestMethod -Uri "https://api.github.com/repos/$Repo/releases/latest" -Headers @{ 'User-Agent' = 'igrok-installer' }
$Asset = $Release.assets | Where-Object { $_.name -like $Pattern } | Select-Object -First 1
if (-not $Asset) {
    throw "No release asset matching '$Pattern' (found: $($Release.assets.name -join ', ')). This platform may not be published yet."
}

$Dest = Join-Path $BinDir 'igrok.exe'
Write-Host "Downloading $($Asset.name) [$([math]::Round($Asset.size / 1MB, 1)) MB] ..."
Invoke-WebRequest -Uri $Asset.browser_download_url -OutFile $Dest -UseBasicParsing

# Add ~\.igrok\bin to the user PATH if missing (takes effect in new terminals).
$UserPath = [Environment]::GetEnvironmentVariable('Path', 'User')
if ($UserPath -notlike "*$BinDir*") {
    [Environment]::SetEnvironmentVariable('Path', "$UserPath;$BinDir", 'User')
    Write-Host "Added $BinDir to your user PATH (open a new terminal to pick it up)."
}

Write-Host ""
Write-Host "Installed: $Dest"
& $Dest --version
Write-Host ""
Write-Host "Next steps:"
Write-Host "  - Provider API keys via env vars, e.g. ANTHROPIC_API_KEY / OPENAI_API_KEY"
Write-Host "  - Or `igrok login` for grok models"
Write-Host "  - Self-update: igrok update  (pulls from $Repo releases)"
