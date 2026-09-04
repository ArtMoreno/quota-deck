# Invoke a Herdr plugin action and wait for it to finish.
#
# `herdr plugin action invoke` starts the action and returns its log id before
# the action completes. Both installer scripts need the matching log to report
# success before they continue: Uninstall.ps1 unlinks the plugin next, and
# unlinking while the restore action is still running can leave a statusLine
# entry pointing at a plugin that is no longer there.
#
# Dot-sourced by install.ps1 and uninstall.ps1; not a standalone script.
# The PowerShell counterpart of scripts/herdr-action.sh — keep the two in step.

$script:HerdrActionPluginId = 'herdr-agent-quota-win'

# Configuration writes touch a handful of small files. A minute is far beyond
# any legitimate run and still bounds a hung action.
if (-not $env:HERDR_ACTION_TIMEOUT_SECONDS) {
    $script:HerdrActionTimeoutSeconds = 60
} else {
    $script:HerdrActionTimeoutSeconds = [int]$env:HERDR_ACTION_TIMEOUT_SECONDS
}

function Get-HerdrActionStatus {
    <#
      Status of one log entry, or $null while Herdr has not listed it yet.
    #>
    param([Parameter(Mandatory)][string] $LogId)

    $output = & herdr plugin log list --plugin $script:HerdrActionPluginId --limit 50 2>$null
    if ($LASTEXITCODE -ne 0 -or -not $output) {
        throw 'cannot list Herdr plugin action logs'
    }
    $response = ($output -join "`n") | ConvertFrom-Json -ErrorAction Stop
    $logs = @($response.result.logs)
    $entry = $logs | Where-Object { $_.log_id -eq $LogId } | Select-Object -First 1
    if (-not $entry) { return $null }
    if (-not $entry.status) { throw "Herdr log $LogId has no status" }
    return [string] $entry.status
}

function Invoke-HerdrActionAndWait {
    <#
      Returns $true when the action succeeded.

      Missing, malformed, failed, or timed-out action evidence fails closed.
    #>
    param([Parameter(Mandatory)][string] $Action)

    $output = & herdr plugin action invoke "$($script:HerdrActionPluginId).$Action"
    if ($LASTEXITCODE -ne 0) { return $false }

    try {
        $response = (($output -join "`n") | ConvertFrom-Json -ErrorAction Stop)
        $logId = if ($response.result.PSObject.Properties['log']) {
            [string] $response.result.log.log_id
        } else {
            [string] $response.result.log_id
        }
        if (-not $logId) { throw 'missing log_id' }
    } catch {
        Write-Error "plugin action $Action returned invalid JSON or no log_id"
        return $false
    }

    $waited = 0
    while ($waited -lt $script:HerdrActionTimeoutSeconds) {
        try {
            $state = Get-HerdrActionStatus -LogId $logId
        } catch {
            Write-Error "cannot verify plugin action ${Action}: $($_.Exception.Message)"
            return $false
        }
        switch ($state) {
            'succeeded' { return $true }
            'running'   { }
            $null       { }
            default {
                Write-Error "plugin action $Action $state"
                Write-Host "inspect it with: herdr plugin log list --plugin $($script:HerdrActionPluginId)"
                return $false
            }
        }
        Start-Sleep -Seconds 1
        $waited++
    }

    Write-Error "plugin action $Action did not finish within $($script:HerdrActionTimeoutSeconds)s"
    return $false
}
