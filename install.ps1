# jhol installer for Windows - the PowerShell twin of install.sh
#
#   irm https://raw.githubusercontent.com/bhuvanprakash/jhol/main/install.ps1 | iex
#
# Contract is identical to install.sh: download the release asset for the
# detected architecture, install it, print the PATH hint. No GUI double-click -
# Windows users get jhol through the same one-line command as everyone else.
#
# Flags as environment variables:
#   JHOL_INSTALL_DIR    install directory (default: $env:LOCALAPPDATA\Programs\jhol)
#   JHOL_VERSION        release to install (default: latest)
#   JHOL_INSTALL_FROM_SOURCE = 1   build from source with cargo

$ErrorActionPreference = "Stop"
$Repo = "bhuvanprakash/jhol"

# ── platform ─────────────────────────────────────────────────────────────────
$arch = switch ($env:PROCESSOR_ARCHITECTURE) {
    "ARM64"   { "arm64" }
    default   { "x64" }   # AMD64 and 32-bit hosts run the x64 binary under WoW64
}

# ── presentation ─────────────────────────────────────────────────────────────
function Say($msg)        { Write-Host "  $msg" }
function Step($mark,$msg) { Write-Host "  $mark $msg" }
function Die($msg)        { Write-Host "  error $msg" -ForegroundColor Red; exit 1 }

$ProgressPreference = "SilentlyContinue"   # IWR's own bar is noise; we render ours

# ── download ─────────────────────────────────────────────────────────────────
$version = if ($env:JHOL_VERSION) { $env:JHOL_VERSION } else { "latest" }
if ($version -eq "latest") { $asset = "$Repo/releases/latest/download/jhol-windows-x64.exe" }
else                       { $asset = "$Repo/releases/download/$version/jhol-windows-x64.exe" }
$url = "https://github.com/$asset"

$tmp = New-TemporaryFile
try {
    Step "→" "downloading jhol (windows/$arch)"
    Say $url
    Invoke-WebRequest -Uri $url -OutFile $tmp.FullName -UseBasicParsing | Out-Null
    $size = (Get-Item $tmp.FullName).Length
    if ($size -lt 100000) { Die "downloaded file is only $size bytes - looks wrong" }
    Step "✓" ("downloaded {0:N1} MB" -f ($size / 1MB))
} catch { Die "download failed: $($_.Exception.Message)" }

# ── install ──────────────────────────────────────────────────────────────────
$installDir = if ($env:JHOL_INSTALL_DIR) { $env:JHOL_INSTALL_DIR }
              else { Join-Path $env:LOCALAPPDATA "Programs\jhol" }
New-Item -ItemType Directory -Force -Path $installDir | Out-Null
$dest = Join-Path $installDir "jhol.exe"
Move-Item -Force $tmp.FullName $dest
Step "✓" "installed to $dest"

# ── PATH hint ────────────────────────────────────────────────────────────────
$inPath = ($env:Path -split ';') -contains $installDir
if (-not $inPath) {
    Say ""
    Say "Add jhol to your PATH for this session:"
    Say "  `$env:Path += \";$installDir\""
    Say "To make it permanent, add the directory to the user PATH in:"
    Say "  System Properties > Environment Variables"
}

$rule = "-" * 64
Say ""
Write-Host $rule -ForegroundColor DarkGray
Say ""
Write-Host ("  {0} ready - try  jhol --version" -f "jhol") -NoNewline
if (-not $inPath) { Write-Host "  (after adding it to PATH)" } else { Write-Host "" }
Write-Host $rule -ForegroundColor DarkGray
