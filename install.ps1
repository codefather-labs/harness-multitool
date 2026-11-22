# ============================================================================
# harness-multitool installer (Windows)
# ============================================================================
# Installs the harness, puts it on your PATH, runs the daemon as a task at
# your logon, and sets Claude Code up for the harness (CON-162), as install.sh
# does on Linux and macOS.
#
#   irm https://harness-multitool.dev/install.ps1 | iex
#   irm <raw url>/install.ps1 | iex   the prebuilt multitool of the latest
#                                   GitHub Release, its SHA-256 checked; no
#                                   Rust (CON-195)
#   .\install.ps1 -Version 0.1.0-dev+f5bd7a9   that release instead
#   .\install.ps1 -FromSource       build a temporary clone of main
#   .\install.ps1 -Local            build this checkout (a Rust toolchain is needed)
#   .\install.ps1 -Local -NoBuild   install the build beside it: the unpacked
#                                   multitool-<version>-x86_64-pc-windows-msvc.zip
#                                   of a GitHub Release (CON-194)
#
# Speech to text is not installed here: it is chosen and downloaded in the
# harness's own settings, section Speech to text.
# ============================================================================
param(
    [switch]$Yes,
    [switch]$Local,
    [switch]$NoBuild,
    [switch]$FromSource,
    [string]$Version = '',
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
  -Version V      Install release V instead of the latest
  -FromSource     Build a clone of main instead of installing a release
  -Local          Build this checkout instead of installing a release
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

# A legacy claudebase / claude-code-sdlc setup, found and, on the user's word, deleted (CON-198), as
# install.sh does: its hooks and rules, a CLAUDE.md that names it, its settings, MCP server, plugin
# and marketplace, ~/.claudebase, and its lines in the PowerShell profile.
function Find-Claudebase {
    $found = @()
    $hooks = Join-Path $ClaudeDir 'hooks'
    if (Test-Path $hooks) { $found += Get-ChildItem $hooks -File | Where-Object { $_.Name -match '^(claudebase-|sdlc-)' } | ForEach-Object { $_.FullName } }
    $rules = Join-Path $ClaudeDir 'rules'
    $own = 'knowledge-base.md', 'knowledge-base-tool.md', 'subagent-onboarding.md', 'session-changelog.md'
    $sdlc = 'scratchpad.md', 'error-recovery.md', 'git.md', 'tool-limitations.md'
    $marked = ($found.Count -gt 0) -or [bool]($own | Where-Object { Test-Path (Join-Path $rules $_) }) -or (Test-Path (Join-Path $UserHome '.claudebase'))
    foreach ($n in $own) { $p = Join-Path $rules $n; if ((Test-Path $p) -and (Select-String -Path $p -Pattern 'claudebase|claude-code-sdlc' -Quiet)) { $found += $p } }
    if ($marked) { foreach ($n in $sdlc) { $p = Join-Path $rules $n; if (Test-Path $p) { $found += $p } } }
    $md = Join-Path $ClaudeDir 'CLAUDE.md'
    if ((Test-Path $md) -and (Select-String -Path $md -Pattern 'claudebase|claude-code-sdlc' -Quiet)) { $found += $md }
    foreach ($d in (Join-Path $ClaudeDir 'plugins/claudebase'), (Join-Path $ClaudeDir 'plugins/marketplaces/claudebase-dev'), (Join-Path $UserHome '.claudebase')) { if (Test-Path $d) { $found += $d } }
    if (Test-Path $ClaudeDir) { $found += Get-ChildItem $ClaudeDir -Directory | Where-Object { $_.Name -like 'claudebase-backup-*' } | ForEach-Object { $_.FullName } }
    # The agents and commands of the claude-code-sdlc pipeline, and the on-demand agents it generated.
    $pipelineAgents = 'architect', 'ba-analyst', 'build-runner', 'changelog-writer', 'code-reviewer', 'consolidator', 'corporate-code-style-reviewer', 'doc-updater', 'e2e-runner', 'planner', 'prd-writer', 'qa-engineer', 'qa-planner', 'red-team', 'refactor-cleaner', 'reflection', 'release-engineer', 'resource-architect', 'role-planner', 'security-auditor', 'test-writer', 'verifier'
    $pipelineCommands = 'bootstrap-feature', 'consolidate', 'context-refresh', 'develop-feature', 'implement-slice', 'knowledge-ingest', 'merge-ready', 'qa-cycle', 'reflect', 'release', 'update-claudebase'
    $sdlc = 'claudebase|claude-code-sdlc|sdlc|scratchpad|role-planner'
    $agents = Join-Path $ClaudeDir 'agents'
    if (Test-Path $agents) {
        foreach ($f in Get-ChildItem $agents -File -Filter '*.md') {
            $ondemand = $f.Name -like 'ondemand-*' -and (Select-String -Path $f.FullName -Pattern 'scope: on-demand' -Quiet)
            if ($ondemand -or (($pipelineAgents -contains $f.BaseName) -and (Select-String -Path $f.FullName -Pattern $sdlc -Quiet))) { $found += $f.FullName }
        }
    }
    $commands = Join-Path $ClaudeDir 'commands'
    if (Test-Path $commands) {
        foreach ($f in Get-ChildItem $commands -File -Filter '*.md') {
            if (($pipelineCommands -contains $f.BaseName) -and (Select-String -Path $f.FullName -Pattern $sdlc -Quiet)) { $found += $f.FullName }
        }
    }
    $found
}

function Remove-Claudebase {
    $found = @(Find-Claudebase)
    $settings = Join-Path $ClaudeDir 'settings.json'
    $claudeJson = Join-Path $UserHome '.claude.json'
    $legacyJson = @()
    if ((Test-Path $settings) -and ((Get-Content $settings -Raw) -match 'claudebase-|sdlc-|claudebase-dev')) { $legacyJson += $settings }
    if ((Test-Path $claudeJson) -and ((Get-Content $claudeJson -Raw) -match '"claudebase"')) { $legacyJson += $claudeJson }
    $profileHas = $PROFILE -and (Test-Path $PROFILE) -and (Select-String -Path $PROFILE -Pattern 'claudebase' -Quiet)
    if ($found.Count -eq 0 -and $legacyJson.Count -eq 0 -and -not $profileHas) { return }
    Log-Warn "a legacy claudebase setup is here; its rules and hooks contradict the harness's:"
    $lines = @($found) + @($legacyJson | ForEach-Object { "its entries in $_" })
    if ($profileHas) { $lines += "its lines in $PROFILE" }
    $lines | ForEach-Object { Write-Host "         $_" }
    $go = $Yes
    if (-not $go) { $go = (Read-Host 'Delete it, all of it (no backup)? [y/N]') -match '^(y|yes)$' }
    if (-not $go) { Log-Warn "kept: its instructions will contradict the harness's until it is gone"; return }
    foreach ($p in $found) { Remove-Item $p -Recurse -Force }
    if ($legacyJson -contains $settings) {
        $s = Get-Content $settings -Raw | ConvertFrom-Json
        if ($s.hooks) {
            foreach ($ev in @($s.hooks.PSObject.Properties.Name)) {
                $groups = @($s.hooks.$ev | ForEach-Object { $_.hooks = @($_.hooks | Where-Object { (Split-Path -Leaf (($_.command -split ' ')[0])) -notmatch '^(claudebase-|sdlc-)' }); $_ } | Where-Object { $_.hooks.Count -gt 0 })
                if ($groups.Count -gt 0) { $s.hooks.$ev = $groups } else { $s.hooks.PSObject.Properties.Remove($ev) }
            }
        }
        if ($s.extraKnownMarketplaces) { $s.extraKnownMarketplaces.PSObject.Properties.Remove('claudebase-dev') }
        $s | ConvertTo-Json -Depth 32 | Set-Content $settings -Encoding UTF8
    }
    if ($legacyJson -contains $claudeJson) {
        $c = Get-Content $claudeJson -Raw | ConvertFrom-Json
        if ($c.mcpServers) { $c.mcpServers.PSObject.Properties.Remove('claudebase') }
        if ($c.projects) { foreach ($pr in $c.projects.PSObject.Properties) { if ($pr.Value.mcpServers) { $pr.Value.mcpServers.PSObject.Properties.Remove('claudebase') } } }
        $c | ConvertTo-Json -Depth 64 | Set-Content $claudeJson -Encoding UTF8
    }
    if ($profileHas) { (Get-Content $PROFILE | Where-Object { $_ -notmatch 'claudebase' }) | Set-Content $PROFILE }
    Log-Ok 'deleted the legacy claudebase setup'
}

function Install-AgentSetup($src) {
    Remove-Claudebase
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

# The prebuilt multitool of a GitHub Release (CON-195): its zip and checksum, checked, then the
# zip's own install.ps1 -Local -NoBuild with the switches given. Nothing is installed when a
# download or a check fails.
function Install-Release {
    $releases = if ($env:MULTITOOL_RELEASES_URL) { $env:MULTITOOL_RELEASES_URL } else { 'https://github.com/codefather-labs/harness-multitool/releases' }
    [Net.ServicePointManager]::SecurityProtocol = [Net.ServicePointManager]::SecurityProtocol -bor [Net.SecurityProtocolType]::Tls12
    $arch = $env:PROCESSOR_ARCHITECTURE
    if ($arch -ne 'AMD64') { Log-Error "no prebuilt multitool for ${arch}: build it with -FromSource"; exit 1 }
    $target = 'x86_64-pc-windows-msvc'
    $v = $Version
    if (-not $v) {
        # The latest release, from where /releases/latest leads: no API, no rate limit.
        $request = [System.Net.HttpWebRequest]::Create("$releases/latest")
        $request.AllowAutoRedirect = $false
        $request.Method = 'HEAD'
        try { $response = $request.GetResponse() } catch [System.Net.WebException] { $response = $_.Exception.Response }
        $location = if ($response) { $response.Headers['Location'] } else { $null }
        if ($response) { $response.Close() }
        if (-not $location -or $location -notmatch '/tag/v(.+)$') { Log-Error "cannot find the latest release at $releases/latest"; exit 1 }
        $v = [Uri]::UnescapeDataString($Matches[1])
    }
    $v = $v.TrimStart('v')
    if ($v -notmatch '^[0-9A-Za-z.+_-]+$') { Log-Error "not a version: $v"; exit 1 }
    $name = "multitool-$v-$target.zip"
    $at = "$releases/download/v$($v.Replace('+', '%2B'))/$($name.Replace('+', '%2B'))"
    $tmp = Join-Path ([IO.Path]::GetTempPath()) ("multitool-install-" + [Guid]::NewGuid())
    New-Item -ItemType Directory -Force -Path $tmp | Out-Null
    try {
        Log-Info "downloading multitool $v for $target"
        $zip = Join-Path $tmp $name
        try {
            Invoke-WebRequest -UseBasicParsing -Uri $at -OutFile $zip
            Invoke-WebRequest -UseBasicParsing -Uri "$at.sha256" -OutFile "$zip.sha256"
        } catch { Log-Error "no download of $name at ${at}: $_"; exit 1 }
        $want = ((Get-Content "$zip.sha256" -Raw).Trim() -split '\s+')[0]
        $have = (Get-FileHash $zip -Algorithm SHA256).Hash
        if (-not $want -or $want -ne $have.ToLower() -and $want -ne $have) { Log-Error "the checksum of $name does not match: nothing installed"; exit 1 }
        Log-Ok 'checksum ok'
        $bundle = Join-Path $tmp 'bundle'
        Expand-Archive -Path $zip -DestinationPath $bundle
        $script = Join-Path $bundle 'install.ps1'
        if (-not (Test-Path $script) -or -not (Test-Path (Join-Path $bundle 'target\release\multitool.exe'))) { Log-Error 'the download is not a multitool bundle: nothing installed'; exit 1 }
        if ($NoClaudeCode) { Log-Warn '-NoClaudeCode only changes a build: the release has Claude Code in it (use -FromSource)' }
        $pass = @{ Local = $true; NoBuild = $true; Yes = $Yes; NoService = $NoService; NoAgentSetup = $NoAgentSetup }
        & $script @pass
        $code = $LASTEXITCODE
    } finally {
        Remove-Item $tmp -Recurse -Force -ErrorAction SilentlyContinue
    }
    exit $code
}

if (-not $Local -and -not $FromSource -and $env:MULTITOOL_INSTALL_AGENT_ONLY -ne '1') {
    Install-Release
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
