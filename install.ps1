# ============================================================================
# harness-multitool installer (Windows)
# ============================================================================
# Builds the harness from source and puts it on your PATH. No releases exist yet,
# so a Rust toolchain is needed.
#
#   .\install.ps1 -Local            from this checkout
#   irm <raw url>/install.ps1 | iex   from a temporary clone
#
# Speech to text is not installed here: it is chosen and downloaded in the
# harness's own settings, section Speech to text.
# ============================================================================
param(
    [switch]$Yes,
    [switch]$Local,
    [switch]$NoClaudeCode,
    [switch]$Uninstall,
    [switch]$Help
)
$ErrorActionPreference = 'Stop'

$RepoUrl = 'https://github.com/codefather-labs/harness-multitool.git'
$DataHome = Join-Path $env:LOCALAPPDATA 'harness-multitool'
$BinDir = Join-Path $DataHome 'bin'

function Log-Info($m)  { Write-Host "[INFO] $m" -ForegroundColor Blue }
function Log-Ok($m)    { Write-Host "  [OK] $m" -ForegroundColor Green }
function Log-Warn($m)  { Write-Host "[WARN] $m" -ForegroundColor Yellow }
function Log-Error($m) { Write-Host "[ERROR] $m" -ForegroundColor Red }

if ($Help) {
    Write-Host @"
harness-multitool installer

USAGE:
  .\install.ps1 [OPTIONS]

OPTIONS:
  -Yes            Skip confirmation prompts
  -Local          Build this checkout instead of cloning
  -NoClaudeCode   Without Claude Code as an adapter
  -Uninstall      Remove the binary and its PATH entry, and keep your data
  -Help           Show this help

WHAT GETS INSTALLED:
  $BinDir\multitool.exe, with that folder on your user PATH
"@
    exit 0
}

function Remove-FromPath {
    $path = [Environment]::GetEnvironmentVariable('Path', 'User')
    if ($path) {
        $kept = ($path -split ';' | Where-Object { $_ -and $_ -ne $BinDir }) -join ';'
        [Environment]::SetEnvironmentVariable('Path', $kept, 'User')
    }
}

if ($Uninstall) {
    $exe = Join-Path $BinDir 'multitool.exe'
    if (Test-Path $exe) { Remove-Item $exe -Force; Log-Ok "removed $exe" } else { Log-Info "nothing installed at $exe" }
    Remove-FromPath
    Log-Ok 'removed it from your user PATH'
    Log-Info 'kept your data, delete it yourself if you want it gone:'
    Log-Info "  $env:LOCALAPPDATA\harness-multitool (sessions, journals, speech models)"
    Log-Info "  $env:APPDATA\harness-multitool (settings, may hold a bot token)"
    exit 0
}

if (-not (Get-Command cargo -ErrorAction SilentlyContinue)) {
    Log-Error 'no Rust toolchain: install it from https://rustup.rs and run this again'
    exit 1
}
$missing = @()
if (-not (Get-Command git -ErrorAction SilentlyContinue)) { $missing += 'git' }
if ($missing.Count -gt 0) {
    Log-Warn ('needed to build, and missing: ' + ($missing -join ', '))
    $ids = @{ git = 'Git.Git' }
    $commands = $missing | ForEach-Object { "winget install --id $($ids[$_]) -e" }
    Log-Info ('this would run: ' + ($commands -join '; '))
    $go = $Yes
    if (-not $go) { $go = (Read-Host 'Run it? [y/N]') -match '^(y|yes)$' }
    if (-not $go) {
        Log-Error 'not installed. Install them yourself and run this again'
        exit 1
    }
    foreach ($command in $commands) { Invoke-Expression $command }
}

$clone = $null
if ($Local) {
    $src = $PSScriptRoot
    if (-not (Test-Path (Join-Path $src 'Cargo.toml'))) { Log-Error '-Local: no Cargo.toml beside this script'; exit 1 }
} else {
    $clone = Join-Path ([IO.Path]::GetTempPath()) ("harness-multitool-" + [Guid]::NewGuid())
    Log-Info "cloning $RepoUrl"
    git clone --depth 1 -q $RepoUrl $clone
    $src = $clone
}

try {
    $features = @()
    if (-not $NoClaudeCode) { $features += 'multitool/claude-code' }
    $cargoArgs = @('build', '--release', '-p', 'multitool')
    if ($features.Count -gt 0) { $cargoArgs += @('--features', ($features -join ',')) }
    Log-Info ('building: cargo ' + ($cargoArgs -join ' '))
    Push-Location $src
    try {
        & cargo @cargoArgs
        if ($LASTEXITCODE -ne 0) { Log-Error 'the build failed'; exit 1 }
    } finally { Pop-Location }

    New-Item -ItemType Directory -Force -Path $BinDir | Out-Null
    $exe = Join-Path $BinDir 'multitool.exe'
    Copy-Item (Join-Path $src 'target\release\multitool.exe') $exe -Force
    Log-Ok "installed $exe"

    $path = [Environment]::GetEnvironmentVariable('Path', 'User')
    if (($path -split ';') -notcontains $BinDir) {
        [Environment]::SetEnvironmentVariable('Path', (($path, $BinDir | Where-Object { $_ }) -join ';'), 'User')
        Log-Ok "added $BinDir to your user PATH: open a new terminal to use it"
    }
    $version = & $exe --version
    if ($LASTEXITCODE -ne 0) { Log-Error 'the installed binary does not run'; exit 1 }
    Log-Ok "it runs: $version"
} finally {
    if ($clone) { Remove-Item $clone -Recurse -Force -ErrorAction SilentlyContinue }
}

Write-Host ''
Write-Host 'harness-multitool installed.' -ForegroundColor White
Write-Host ''
Write-Host '  multitool                  open it; s opens the settings'
Write-Host '  Settings, Provider         choose an adapter and sign in'
Write-Host '  Settings, Telegram         a bot token, to reach your sessions from a chat'
Write-Host '  Settings, Speech to text   choose and download an engine'
Write-Host '  .\install.ps1 -Uninstall   remove it, keeping your data'
Write-Host ''
