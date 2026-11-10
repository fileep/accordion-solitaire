<#
.SYNOPSIS
For most people `acc run` does what this script does, and more (it also pauses a laptop's run while the battery is
low): see docs/LONG_RUNS.md. This script is for seeing a long `acc batch` through with a binary of your choosing.

Sees a long `acc batch` through: starts it, or carries on with it if its checkpoint is there, and carries on again
whenever it stops for a reason that a new process may get past. Run it from the repository's directory, as the run's
paths are relative to where it started. Press Ctrl+C once to pause the run; this script stops with it.

.EXAMPLE
powershell -ExecutionPolicy Bypass -File tools\long-run.ps1 -Acc .\target\release\acc.exe -From 0 -Count 1000000000

.EXAMPLE
# Eight hours, then the computer is yours. The same command carries on from there.
powershell -ExecutionPolicy Bypass -File tools\long-run.ps1 -Acc .\target\release\acc.exe -From 0 -Count 1000000000 -StopAfterHours 8

.EXAMPLE
# Refuse to start without the GPU, rather than go on at the CPU's speed.
powershell -ExecutionPolicy Bypass -File tools\long-run.ps1 -Acc .\target\release\acc.exe -From 0 -Count 1000000000 -Gpu on
#>
param(
    # The acc.exe to run. For a run of days, give it a copy that has been seen to start, not the one in target\ that
    # the next build replaces: Smart App Control can block a rebuilt binary, which would stop a run that wants to carry on.
    [Parameter(Mandatory)][string]$Acc,
    [Parameter(Mandatory)][uint64]$From,
    [Parameter(Mandatory)][uint64]$Count,
    # `acc batch --gpu` when the run starts: auto uses an NVIDIA card if it can and the CPU alone if not; on refuses
    # to start without the card, which suits a run sized for it; off never uses it.
    [ValidateSet('auto', 'on', 'off')][string]$Gpu = 'auto',
    # Pause after this many hours of each session (each new process counts from its own start).
    [double]$StopAfterHours = 0,
    # Pause once this many of the run's deals are done.
    [uint64]$Until = 0,
    # Tries in a row, making no progress, before giving up.
    [int]$MaxFailures = 6,
    [int]$WaitSeconds = 60,
    # Anything else goes to `acc batch` when the run starts, not when it carries on: its settings are in the checkpoint.
    [Parameter(ValueFromRemainingArguments)][string[]]$StartArgs = @()
)

# acc keeps the computer from going to sleep while it runs; this holds that between its sessions as well. It changes no
# setting, and ends with this script.
try {
    Add-Type -Namespace LongRun -Name Power -MemberDefinition '[DllImport("kernel32.dll")] public static extern uint SetThreadExecutionState(uint flags);'
    [void][LongRun.Power]::SetThreadExecutionState([uint32]"0x80000001")
} catch {
    Write-Host "note: couldn't ask Windows to stay awake between sessions: $_"
}

$checkpoint = "checkpoints/batch-$From-$Count.json"
$sessionArgs = @()
if ($StopAfterHours -gt 0) { $sessionArgs += @('--stop-after', "$StopAfterHours") }
if ($Until -gt 0) { $sessionArgs += @('--until', "$Until") }

function Done-So-Far {
    if (-not (Test-Path $checkpoint)) { return [uint64]0 }
    try { return [uint64](Get-Content $checkpoint -Raw | ConvertFrom-Json).standing.done } catch { return [uint64]0 }
}

$failures = 0
$reached = Done-So-Far
while ($true) {
    if (Test-Path $checkpoint) {
        $acting = @('batch', '--resume', $checkpoint) + $sessionArgs
    } else {
        $acting = @('batch', '--from', "$From", '--count', "$Count", '--gpu', $Gpu) + $StartArgs + $sessionArgs
    }
    Write-Host "$(Get-Date -Format s) acc $($acting -join ' ')"
    try {
        & $Acc @acting
        $code = $LASTEXITCODE
    } catch {
        # It didn't start at all: Smart App Control, most likely. Trying again gets the same answer, by its hash.
        Write-Host "$(Get-Date -Format s) acc.exe wouldn't start: $_"
        Write-Host 'A rebuilt binary can be blocked; use one that has been seen to start. Nothing is lost.'
        exit 1
    }
    $now = Done-So-Far
    switch ($code) {
        0 {
            if (Test-Path $checkpoint) {
                Write-Host "$(Get-Date -Format s) paused at $now deals. The same command carries on."
            } else {
                Write-Host "$(Get-Date -Format s) finished: the report is beside where the checkpoint was."
            }
            exit 0
        }
        74 {
            Write-Host "$(Get-Date -Format s) out of room on the disk. Make room; the same command carries on."
            exit 74
        }
        130 {
            Write-Host "$(Get-Date -Format s) stopped by hand. The same command carries on."
            exit 130
        }
        default {
            # 75: the GPU failed, and a new process usually gets it back. Anything else is a crash: the last checkpoint
            # stands. Either way it is worth trying again, unless that keeps getting nowhere.
            if ($now -gt $reached) { $failures = 0 }
            $reached = $now
            $failures += 1
            Write-Host "$(Get-Date -Format s) exit code $code at $now deals, try $failures of $MaxFailures without progress"
            if ($failures -ge $MaxFailures) {
                Write-Host "$(Get-Date -Format s) giving up: it isn't getting anywhere. Nothing is lost; see the .events.log."
                exit $code
            }
            Start-Sleep -Seconds $WaitSeconds
        }
    }
}
