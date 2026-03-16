#Requires -Version 5.1
# Install godot-lsp-bridge — fetches the latest Windows release binary and
# installs it to $HOME\.cargo\bin (if present) or $env:LOCALAPPDATA\Programs\godot-lsp-bridge\bin.
[CmdletBinding()]
param()

$ErrorActionPreference = 'Stop'

$Repo    = 'Pixel-Directive-LLC/godot-lsp-bridge'
$BinName = 'godot-lsp-bridge'
$Target  = 'x86_64-pc-windows-msvc'

# ── Resolve latest version ────────────────────────────────────────────────────
$Release = Invoke-RestMethod "https://api.github.com/repos/$Repo/releases/latest"
$Version = $Release.tag_name -replace '^v', ''

if (-not $Version) {
    Write-Error 'Could not determine latest release version.'
    exit 1
}

# ── Choose install directory ──────────────────────────────────────────────────
$CargoBin = Join-Path $env:USERPROFILE '.cargo\bin'
if (Test-Path $CargoBin) {
    $InstallDir = $CargoBin
} else {
    $InstallDir = Join-Path $env:LOCALAPPDATA "Programs\$BinName\bin"
    New-Item -ItemType Directory -Force -Path $InstallDir | Out-Null
}

# ── Download and install ──────────────────────────────────────────────────────
$Archive = "$BinName-$Target.zip"
$Url     = "https://github.com/$Repo/releases/download/v$Version/$Archive"
$Dest    = Join-Path $InstallDir "$BinName.exe"

Write-Host "Installing $BinName v$Version ($Target)"
Write-Host "  source: $Url"
Write-Host "  dest:   $Dest"
Write-Host ""

$Tmp = Join-Path ([System.IO.Path]::GetTempPath()) ([System.IO.Path]::GetRandomFileName())
New-Item -ItemType Directory -Force -Path $Tmp | Out-Null

try {
    $ZipPath = Join-Path $Tmp $Archive
    Invoke-WebRequest -Uri $Url -OutFile $ZipPath -UseBasicParsing
    Expand-Archive -Path $ZipPath -DestinationPath $Tmp -Force
    Copy-Item -Path (Join-Path $Tmp "$BinName.exe") -Destination $Dest -Force
} finally {
    Remove-Item -Recurse -Force $Tmp -ErrorAction SilentlyContinue
}

Write-Host "Done. $BinName v$Version installed."

# ── PATH hint ─────────────────────────────────────────────────────────────────
$UserPath = [System.Environment]::GetEnvironmentVariable('PATH', 'User')
$PathDirs = $UserPath -split ';' | Where-Object { $_ -ne '' }
if ($InstallDir -notin $PathDirs) {
    Write-Host ""
    Write-Host "Note: $InstallDir is not on your PATH."
    Write-Host "Run the following to add it for your user:"
    Write-Host ""
    Write-Host "  [System.Environment]::SetEnvironmentVariable('PATH', `"`$env:PATH;$InstallDir`", 'User')"
}
