# ============================================================================
# harness-multitool installer (Windows)
# ============================================================================
# Builds the harness from source, puts it on your PATH, runs the daemon as a
# task at your logon, and sets Claude Code up for the harness (CON-162), as
# install.sh does on Linux and macOS.
#
#   .\install.ps1 -Local            build this checkout (a Rust toolchain is needed)
#   .\install.ps1 -Local -NoBuild   install the build beside it: the unpacked
#                                   multitool-<version>-x86_64-pc-windows-msvc.zip
#                                   of a GitHub Release (CON-194)
#   irm <raw url>/install.ps1 | iex   build a temporary clone
#
# Speech to text is not installed here: it is chosen and downloaded in the
# harness's own settings, section Speech to text.
# ============================================================================
param(
    [switch]$Yes,
    [switch]$Local,
    [switch]$NoBuild,
    [switch]$NoClaudeCode,
    [switch]$NoService,
    [switch]$NoAgentSetup,
    [switch]$Uninstall,
    [switch]$Help
)
$ErrorActionPreference = 'Stop'

$RepoUrl = 'https://github.com/codefather-labs/harness-multitool.git'
$OnWindows = ($env:OS -eq 'Windows_NT')
$LocalHome = if ($env:LOCALAPPDATA) { $env:LOCALAPPDATA } else { Join-Path $HOME '.local/share' }
$RoamingHome = if ($env:APPDATA) { $env:APPDATA } else { Join-Path $HOME '.config' }
$UserHome = if ($env:USERPROFILE) { $env:USERPROFILE } else { $HOME }
$DataHome = Join-Path $LocalHome 'harness-multitool'
$BinDir = Join-Path $DataHome 'bin'
$Exe = Join-Path $BinDir 'multitool.exe'
# The harness's own data (sessions, journals, the daemon): the roaming AppData, as the dirs crate gives it.
$AppData = Join-Path $RoamingHome 'harness-multitool'
$TaskName = 'multitool-daemon'
$ClaudeDir = Join-Path $UserHome '.claude'
$HookName = 'harness-selfcheck-reminder.ps1'
$HookPath = Join-Path (Join-Path $ClaudeDir 'hooks') $HookName
$HookCommand = "powershell -NoProfile -ExecutionPolicy Bypass -File `"$HookPath`""
$Manifest = Join-Path $DataHome 'installed-claude-files'

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
  -NoBuild        With -Local: install target\release\multitool.exe beside
                  this script, as a release's zip has it, without building
  -NoClaudeCode   Without Claude Code as an adapter
  -NoService      Do not run the daemon as a task at your logon
  -NoAgentSetup   Do not install Claude Code's rules and hook
  -Uninstall      Remove the binary, its PATH entry, the task and the agent
                  setup; keep your data and the backups
  -Help           Show this help

WHAT GETS INSTALLED:
  $Exe, with that folder on your user PATH
  the daemon as the Scheduled Task $TaskName, run at your logon
  $ClaudeDir\rules\project-journal.md and cognitive-self-check.md,
  the hook $HookPath, and its UserPromptSubmit entry in
  $ClaudeDir\settings.json; a file of yours that differs is kept beside it
  as <name>.before-multitool-<time> first
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

function Stamp { Get-Date -Format 'yyyyMMddHHmmss' }

# ---------------------------------------------------------------------------
# The daemon as a task at the user's logon (CON-162)
# ---------------------------------------------------------------------------
function Stop-RunningDaemon {
    if ((Test-Path $Exe) -and (& $Exe daemon --status 2>$null) -and $LASTEXITCODE -eq 0) {
        Log-Info 'stopping the daemon that runs now, so the task runs the only one'
        & $Exe daemon stop *> $null
    }
}

function Install-Service {
    if (-not $OnWindows) { Log-Warn 'not Windows: no Scheduled Task here'; return }
    Stop-RunningDaemon
    $action = New-ScheduledTaskAction -Execute $Exe -Argument 'daemon'
    $trigger = New-ScheduledTaskTrigger -AtLogOn -User $env:USERNAME
    $settings = New-ScheduledTaskSettingsSet -AllowStartIfOnBatteries -DontStopIfGoingOnBatteries -StartWhenAvailable -ExecutionTimeLimit ([TimeSpan]::Zero) -RestartCount 3 -RestartInterval (New-TimeSpan -Minutes 1) -Hidden
    $principal = New-ScheduledTaskPrincipal -UserId $env:USERNAME -LogonType Interactive -RunLevel Limited
    Register-ScheduledTask -TaskName $TaskName -Action $action -Trigger $trigger -Settings $settings -Principal $principal -Description 'harness-multitool daemon' -Force | Out-Null
    Start-ScheduledTask -TaskName $TaskName
    Log-Ok "the daemon runs as the Scheduled Task $TaskName, at your logon"
}

function Start-InBackground {
    Log-Warn 'no task: the daemon is started in the background, and does not come back after you log out'
    if (-not ((& $Exe daemon --status 2>$null) -and $LASTEXITCODE -eq 0)) {
        Start-Process -FilePath $Exe -ArgumentList 'daemon' -WindowStyle Hidden
    }
}

function Uninstall-Service {
    if (-not $OnWindows) { return }
    if (Get-ScheduledTask -TaskName $TaskName -ErrorAction SilentlyContinue) {
        Stop-ScheduledTask -TaskName $TaskName -ErrorAction SilentlyContinue
        if (Test-Path $Exe) { & $Exe daemon stop *> $null }
        Unregister-ScheduledTask -TaskName $TaskName -Confirm:$false
        Log-Ok "removed the Scheduled Task $TaskName"
    }
}

# ---------------------------------------------------------------------------
# Claude Code's agent setup (CON-162): the harness's rules and self-check hook
# ---------------------------------------------------------------------------
# A file of the user's that differs is kept beside it before it is replaced.
function Put-File($from, $to) {
    New-Item -ItemType Directory -Force -Path (Split-Path $to) | Out-Null
    if ((Test-Path $to) -and ((Get-FileHash $from).Hash -ne (Get-FileHash $to).Hash)) {
        Copy-Item $to "$to.before-multitool-$(Stamp)"
        Log-Info "backed up $to"
    }
    if (-not (Test-Path $to) -or (Get-FileHash $from).Hash -ne (Get-FileHash $to).Hash) { Copy-Item $from $to -Force }
    New-Item -ItemType Directory -Force -Path (Split-Path $Manifest) | Out-Null
    $listed = if (Test-Path $Manifest) { Get-Content $Manifest } else { @() }
    if ($listed -notcontains $to) { Add-Content -Path $Manifest -Value $to }
    Log-Ok $to
}

# Adds the hook's UserPromptSubmit entry to settings.json, or takes it out: the rest is left as it is.
# Answers whether it changed the file.
function Set-SettingsHook([string]$action) {
    $path = Join-Path $ClaudeDir 'settings.json'
    if ($action -eq 'remove' -and -not (Test-Path $path)) { return $false }
    $settings = if (Test-Path $path) { Get-Content $path -Raw | ConvertFrom-Json } else { [pscustomobject]@{} }
    if (-not $settings.PSObject.Properties['hooks']) { $settings | Add-Member -NotePropertyName hooks -NotePropertyValue ([pscustomobject]@{}) }
    $groups = @()
    if ($settings.hooks.PSObject.Properties['UserPromptSubmit']) { $groups = @($settings.hooks.UserPromptSubmit) }
    $present = @($groups | ForEach-Object { $_.hooks } | Where-Object { $_.command -eq $HookCommand }).Count -gt 0
    if ($action -eq 'add' -and -not $present) {
        $groups += [pscustomobject]@{ hooks = @([pscustomobject]@{ type = 'command'; command = $HookCommand }) }
    } elseif ($action -eq 'remove' -and $present) {
        $groups = @($groups | ForEach-Object {
            $_.hooks = @($_.hooks | Where-Object { $_.command -ne $HookCommand }); $_
        } | Where-Object { @($_.hooks).Count -gt 0 })
    } else {
        return $false
    }
    if ($groups.Count -gt 0) {
        $settings.hooks | Add-Member -NotePropertyName UserPromptSubmit -NotePropertyValue $groups -Force
    } elseif ($settings.hooks.PSObject.Properties['UserPromptSubmit']) {
        $settings.hooks.PSObject.Properties.Remove('UserPromptSubmit')
    }
    if (Test-Path $path) { Copy-Item $path "$path.before-multitool-$(Stamp)" }
    New-Item -ItemType Directory -Force -Path $ClaudeDir | Out-Null
    $tmp = "$path.tmp"
    [IO.File]::WriteAllText($tmp, ($settings | ConvertTo-Json -Depth 32) + "`n")
    Move-Item $tmp $path -Force
    return $true
}

function Install-AgentSetup($src) {
    $assets = Join-Path $src 'assets/claude'
    if (-not (Test-Path $assets)) { Log-Warn "no assets/claude in the source: Claude Code's agent setup skipped"; return }
    Put-File (Join-Path $assets 'rules/project-journal.md') (Join-Path $ClaudeDir 'rules/project-journal.md')
    Put-File (Join-Path $assets 'rules/cognitive-self-check.md') (Join-Path $ClaudeDir 'rules/cognitive-self-check.md')
    Put-File (Join-Path $assets "hooks/$HookName") $HookPath
    if (Set-SettingsHook 'add') { Log-Ok "$ClaudeDir\settings.json: the self-check hook added" } else { Log-Ok "$ClaudeDir\settings.json: the self-check hook is there" }
}

function Uninstall-AgentSetup {
    if (Set-SettingsHook 'remove') { Log-Ok "took the self-check hook out of $ClaudeDir\settings.json" }
    if (-not (Test-Path $Manifest)) { return }
    foreach ($file in Get-Content $Manifest) {
        if ($file -and (Test-Path $file)) { Remove-Item $file -Force; Log-Ok "removed $file" }
    }
    Remove-Item $Manifest -Force
    Log-Info 'kept the backups beside them (*.before-multitool-*): restore one by renaming it'
}

if ($Uninstall) {
    Uninstall-Service
    Uninstall-AgentSetup
    if (Test-Path $Exe) { Remove-Item $Exe -Force; Log-Ok "removed $Exe" } else { Log-Info "nothing installed at $Exe" }
    if ($OnWindows) { Remove-FromPath; Log-Ok 'removed it from your user PATH' }
    Log-Info 'kept your data, delete it yourself if you want it gone:'
    Log-Info "  $AppData (sessions, journals, speech models, the daemon)"
    Log-Info "  $UserHome\.harness (profiles: settings and sign-ins; may hold a bot token)"
    exit 0
}

# The agent setup alone, for checking it off Windows (journal#1209): nothing is built or installed.
if ($env:MULTITOOL_INSTALL_AGENT_ONLY -eq '1') {
    Install-AgentSetup $PSScriptRoot
    exit 0
}

if ($NoBuild -and -not ($Local -and (Test-Path (Join-Path $PSScriptRoot 'target\release\multitool.exe')))) {
    Log-Error '-NoBuild needs -Local and a build in target\release\multitool.exe beside this script'
    exit 1
}
if (-not $NoBuild -and -not (Get-Command cargo -ErrorAction SilentlyContinue)) {
    Log-Error 'no Rust toolchain: install it from https://rustup.rs and run this again'
    exit 1
}
$missing = @()
if (-not $NoBuild -and -not (Get-Command git -ErrorAction SilentlyContinue)) { $missing += 'git' }
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

$status = 0
try {
    # Claude Code as an adapter is in the default build (ADR 0020); -NoClaudeCode leaves it out.
    if ($NoBuild) {
        Log-Info 'installing the build beside this script (-NoBuild)'
    } else {
        $cargoArgs = @('build', '--release', '-p', 'multitool')
        if ($NoClaudeCode) { $cargoArgs += '--no-default-features' }
        Log-Info ('building: cargo ' + ($cargoArgs -join ' '))
        Push-Location $src
        try {
            & cargo @cargoArgs
            if ($LASTEXITCODE -ne 0) { Log-Error 'the build failed'; exit 1 }
        } finally { Pop-Location }
    }

    New-Item -ItemType Directory -Force -Path $BinDir | Out-Null
    # A running daemon keeps its file open: it stops first, and the task starts the new one.
    if (-not $NoService) { Stop-RunningDaemon }
    Copy-Item (Join-Path $src 'target\release\multitool.exe') $Exe -Force
    Log-Ok "installed $Exe"

    $path = [Environment]::GetEnvironmentVariable('Path', 'User')
    if (($path -split ';') -notcontains $BinDir) {
        [Environment]::SetEnvironmentVariable('Path', (($path, $BinDir | Where-Object { $_ }) -join ';'), 'User')
        Log-Ok "added $BinDir to your user PATH: open a new terminal to use it"
    }
    # Another multitool found first on PATH would be the one a new terminal runs.
    $first = Get-Command multitool -ErrorAction SilentlyContinue | Select-Object -First 1
    if ($first -and $first.Source -ne $Exe) {
        Log-Warn "another multitool comes first on your PATH: $($first.Source). Remove it, or run $Exe"
    }
    $version = & $Exe --version
    if ($LASTEXITCODE -ne 0) { Log-Error 'the installed binary does not run'; exit 1 }
    Log-Ok "it runs: $version"

    if ($NoService) { Start-InBackground } else { try { Install-Service } catch { Log-Error "the task was not registered: $_"; $status = 1; Start-InBackground } }
    if (-not $NoAgentSetup) { Install-AgentSetup $src }

    # What works, before saying it does (CON-162).
    Write-Host ''
    Log-Info 'checking the install'
    function Check($what, [scriptblock]$test) {
        $ok = $false
        try { $ok = [bool](& $test) } catch { $ok = $false }
        if ($ok) { Log-Ok $what } else { Log-Error "${what}: no"; $script:status = 1 }
    }
    Check 'multitool runs' { & $Exe --version *> $null; $LASTEXITCODE -eq 0 }
    $answered = $false
    foreach ($i in 1..30) { & $Exe daemon --status *> $null; if ($LASTEXITCODE -eq 0) { $answered = $true; break }; Start-Sleep -Milliseconds 500 }
    Check 'the daemon answers' { $answered }
    if (-not $NoService -and $OnWindows) { Check "the task $TaskName is registered" { [bool](Get-ScheduledTask -TaskName $TaskName -ErrorAction SilentlyContinue) } }
    if (-not $NoAgentSetup -and (Test-Path (Join-Path $src 'assets/claude'))) {
        Check "the rules are in $ClaudeDir\rules" { (Test-Path (Join-Path $ClaudeDir 'rules/project-journal.md')) -and (Test-Path (Join-Path $ClaudeDir 'rules/cognitive-self-check.md')) }
        Check 'the self-check hook runs' { (powershell -NoProfile -ExecutionPolicy Bypass -File $HookPath | ConvertFrom-Json).hookSpecificOutput.hookEventName -eq 'UserPromptSubmit' }
        Check 'settings.json calls it' { (Get-Content (Join-Path $ClaudeDir 'settings.json') -Raw).Contains($HookPath.Replace('\', '\\')) }
    }
} finally {
    if ($clone) { Remove-Item $clone -Recurse -Force -ErrorAction SilentlyContinue }
}

Write-Host ''
if ($status -eq 0) { Write-Host 'harness-multitool installed.' -ForegroundColor White } else { Write-Host 'harness-multitool installed, with the problems above.' -ForegroundColor White }
Write-Host ''
Write-Host '  multitool                  open it; s opens the settings'
Write-Host '  Settings, Provider         choose an adapter and sign in'
Write-Host '  Settings, Telegram         a bot token, to reach your sessions from a chat'
Write-Host '  Settings, Speech to text   choose and download an engine'
Write-Host '  .\install.ps1 -Uninstall   remove it, keeping your data'
Write-Host ''
exit $status
