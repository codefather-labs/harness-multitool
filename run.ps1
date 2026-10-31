# Run harness-multitool from this checkout with its environment set (Windows PowerShell).
# The future installer sets the same variables globally; until then this script does it.
#
#   .\run.ps1                          # preview, starting at the splash
#   .\run.ps1 -Graphics sixel          # force a picture protocol: auto|sixel|kitty|iterm2|off
#   .\run.ps1 preview main-session     # any multitool arguments after the options
param(
    [ValidateSet('auto', 'sixel', 'kitty', 'iterm2', 'off')]
    [string]$Graphics,
    [switch]$DebugBuild,
    [Parameter(ValueFromRemainingArguments = $true)]
    [string[]]$Rest
)
$ErrorActionPreference = 'Stop'
$here = Split-Path -Parent $MyInvocation.MyCommand.Path

# --- environment ---------------------------------------------------------------------------
# How pictures (logo, icons, big text) are drawn. auto asks graphics-capable terminals.
if ($Graphics) { $env:MULTITOOL_GRAPHICS = $Graphics }
elseif (-not $env:MULTITOOL_GRAPHICS) { $env:MULTITOOL_GRAPHICS = 'auto' }
# 24-bit colour: the themes are RGB.
if (-not $env:COLORTERM) { $env:COLORTERM = 'truecolor' }
# ---------------------------------------------------------------------------------------------

$buildProfile = if ($DebugBuild) { 'debug' } else { 'release' }
$manifest = Join-Path $here 'Cargo.toml'
if ($buildProfile -eq 'release') { cargo build --quiet --release --manifest-path $manifest }
else { cargo build --quiet --manifest-path $manifest }
if ($LASTEXITCODE -ne 0) { exit $LASTEXITCODE }

$bin = Join-Path $here (Join-Path 'target' (Join-Path $buildProfile 'multitool.exe'))
if (-not $Rest -or $Rest.Count -eq 0) { $Rest = @('preview', 'splash') }
& $bin @Rest
exit $LASTEXITCODE
