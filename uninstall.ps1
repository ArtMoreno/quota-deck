<#
.SYNOPSIS
Restore plugin-owned configuration, then unlink herdr-agent-quota-win.

.DESCRIPTION
The Windows counterpart of uninstall.sh. The restore action runs, and is waited
for, before unlinking: Herdr owns the plugin state directory holding the
Claude/Agy statusLine backups, and `herdr plugin action invoke` returns before
the action has finished. Unlinking early would strand a statusLine entry
pointing at a plugin that is gone.

A full uninstall also drops the saved sidebar-layout, row-gap, quota-percent,
fields, brand-colors, brand-glyphs, agent-order and low-quota-alert prefs, and
hands Herdr's agent panel back its own ordering.

.PARAMETER Agent
Remove only the agents you name and stay installed for the rest.

.EXAMPLE
.\uninstall.ps1

.EXAMPLE
.\uninstall.ps1 -Agent grok
#>
[CmdletBinding()]
param(
    [string[]] $Agent
)

$ErrorActionPreference = 'Stop'
Set-StrictMode -Version Latest

$root = Split-Path -Parent $MyInvocation.MyCommand.Path
. (Join-Path $root 'scripts\HerdrAction.ps1')

function Die {
    param([string] $Message)
    Write-Error "error: $Message"
    exit 1
}

if (-not (Get-Command herdr -ErrorAction SilentlyContinue)) {
    Die 'Herdr is not installed or not on PATH'
}

$pluginId = $script:HerdrActionPluginId
$allowedAgents = @('all', 'claude', 'codex', 'grok', 'agy', 'opencode', 'pi', 'omp', 'hermes')
$selectedAgents = @()
if ($Agent) {
    $selectedAgents = @(
        $Agent | ForEach-Object { $_ -split ',' } | ForEach-Object { $_.Trim().ToLowerInvariant() }
    )
}
foreach ($name in $selectedAgents) {
    if (-not $name -or $name -notin $allowedAgents) { Die "unknown agent '$name'" }
}
if ($selectedAgents -contains 'all' -and $selectedAgents.Count -ne 1) {
    Die "'all' cannot be combined with another agent"
}
$selection = $selectedAgents -join ','
$fullUninstall = -not $selection -or (($selection -split ',') -contains 'all')

$listOutput = & herdr plugin list --plugin $pluginId --json 2>$null
if ($LASTEXITCODE -ne 0 -or -not $listOutput) { Die 'cannot inspect Herdr plugin state' }
try {
    $plugins = @(($listOutput -join "`n" | ConvertFrom-Json -ErrorAction Stop).result.plugins)
} catch {
    Die 'Herdr returned invalid plugin state JSON'
}
$plugin = $plugins | Where-Object { $_.plugin_id -eq $pluginId } | Select-Object -First 1
if (-not $plugin) {
    Write-Host "$pluginId is not linked; no configuration was changed."
    exit 0
}
$restoreDisabled = -not [bool]$plugin.enabled

# Herdr runs the uninstall action with a fixed command line in the server's own
# environment, so setting a variable around `herdr plugin action invoke` is
# silently ignored — and an ignored selection means removing everything instead
# of one agent. A partial selection therefore travels through a one-shot file
# in the plugin config directory; full uninstalls remove any stale one first.
$uninstallAgentsPref = $null

function Clear-UninstallAgentsPref {
    if ($uninstallAgentsPref -and (Test-Path $uninstallAgentsPref)) {
        Remove-Item -Force $uninstallAgentsPref
    }
}

try {
    # An earlier interrupted uninstall may have disabled the plugin. Enable it
    # long enough for Herdr to provide the state directory to the restore action.
    if ($restoreDisabled) {
        & herdr plugin enable $pluginId *> $null
        if ($LASTEXITCODE -ne 0) { Die 'cannot temporarily enable the plugin for restoration' }
    }

    $configDir = ((& herdr plugin config-dir $pluginId) -join '').Trim()
    if ($LASTEXITCODE -ne 0 -or -not $configDir) {
        Die 'cannot resolve plugin config directory'
    }
    New-Item -ItemType Directory -Force -Path $configDir | Out-Null
    $uninstallAgentsPref = Join-Path $configDir 'uninstall-agents'
    Clear-UninstallAgentsPref
    if (-not $fullUninstall) {
        [System.IO.File]::WriteAllText($uninstallAgentsPref, "$selection`n", (New-Object System.Text.UTF8Encoding $false))
    }

    Write-Host '-> restoring plugin-owned configuration'
    # Waiting matters twice here: the selection file must stay in place until
    # the action has read it, and unlinking before the restore finishes can
    # strand a statusLine entry pointing at a plugin that is gone.
    if (-not (Invoke-HerdrActionAndWait -Action 'uninstall')) {
        Die 'restore action failed; nothing was unlinked'
    }
    if ($fullUninstall) { $restoreDisabled = $false }
} finally {
    Clear-UninstallAgentsPref
    if ($restoreDisabled) {
        & herdr plugin disable $pluginId *> $null
        if ($LASTEXITCODE -ne 0) { Write-Warning 'could not restore the plugin disabled state' }
    }
}

# Removing one agent is not uninstalling the plugin; the rest still need it.
if (-not $fullUninstall) {
    Write-Host "Removed $selection. The plugin stays linked for the other agents."
    exit 0
}

Write-Host '-> disabling and unlinking the Herdr plugin'
& herdr plugin disable $pluginId *> $null
$unlinkOutput = & herdr plugin unlink $pluginId 2>&1
if ($LASTEXITCODE -ne 0) { Die "herdr plugin unlink failed: $unlinkOutput" }
Write-Host 'Uninstalled and restored.'
