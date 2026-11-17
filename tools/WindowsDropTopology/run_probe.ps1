param(
    [string]$OutputDirectory = (Join-Path $PSScriptRoot 'results'),
    [switch]$OwnedHostedCiInputDesktop
)
$ErrorActionPreference = 'Stop'
New-Item -ItemType Directory -Force $OutputDirectory | Out-Null
$Source = Join-Path $PSScriptRoot 'receiver_topology.cpp'
$Executable = Join-Path $OutputDirectory 'receiver_topology.exe'
# Run in a VS x64 developer environment. No Cargo/native application is launched.
& cl.exe /nologo /std:c++20 /EHsc /W4 /O2 /D_WIN32_WINNT=0x0A00 $Source "/Fe:$Executable" "/Fo:$OutputDirectory\receiver_topology.obj" ole32.lib user32.lib psapi.lib rpcrt4.lib shell32.lib
if ($LASTEXITCODE -ne 0) { throw 'Native topology fixture compilation failed' }
$Cases = @(
    'normal', 'query-stall', 'get-stall', 'over-memory', 'release-stall',
    'baseline-query-stall',
    'ole-normal', 'ole-query-stall', 'baseline-ole-query-stall'
)
if ($OwnedHostedCiInputDesktop) {
    if ($env:GITHUB_ACTIONS -cne 'true' -or $env:RUNNER_OS -cne 'Windows' -or $env:RUNNER_ENVIRONMENT -cne 'github-hosted') {
        throw 'Input desktop proof is permitted only by explicit opt-in on an owned hosted Windows Actions VM'
    }
    $env:SCREENSHOTS_OWNED_HOSTED_CI_INPUT = '1'
    Add-Type -TypeDefinition @'
using System;
using System.Runtime.InteropServices;
public static class OwnedCiDesktopRestore {
    [StructLayout(LayoutKind.Sequential)] public struct Point { public int X; public int Y; }
    [DllImport("user32.dll", SetLastError=true)] public static extern IntPtr OpenInputDesktop(uint flags, bool inherit, uint access);
    [DllImport("user32.dll", SetLastError=true)] public static extern bool SwitchDesktop(IntPtr desktop);
    [DllImport("user32.dll", SetLastError=true)] public static extern bool CloseDesktop(IntPtr desktop);
    [DllImport("user32.dll", SetLastError=true)] public static extern bool GetCursorPos(out Point point);
    [DllImport("user32.dll", SetLastError=true)] public static extern bool SetCursorPos(int x, int y);
    [DllImport("user32.dll")] public static extern short GetAsyncKeyState(int key);
}
'@
    $Cases += @('ci-ole-normal', 'ci-ole-query-stall', 'ci-ole-get-stall', 'ci-ole-over-memory', 'ci-ole-release-stall', 'ci-baseline-ole-query-stall')
}
$Results = @()
foreach ($Case in $Cases) {
    $Out = Join-Path $OutputDirectory "$Case.stdout.json"
    $Err = Join-Path $OutputDirectory "$Case.stderr.log"
    $RestoreDesktop = [IntPtr]::Zero
    $Cursor = $null
    $Process = $null
    try {
        $Arguments = @($Case)
        if ($Case.StartsWith('ci-')) {
            $RestoreDesktop = [OwnedCiDesktopRestore]::OpenInputDesktop(0, $false, 0x0181)
            $Cursor = [OwnedCiDesktopRestore+Point]::new()
            if ($RestoreDesktop -eq [IntPtr]::Zero -or -not [OwnedCiDesktopRestore]::GetCursorPos([ref]$Cursor)) {
                throw 'Cannot establish external owned CI desktop restoration guard'
            }
            if (([OwnedCiDesktopRestore]::GetAsyncKeyState(1) -band 0x8000) -ne 0) {
                throw 'Owned CI input proof requires the original left button to be released'
            }
            $Arguments = @('--owned-ci-input-desktop', $Case)
        }
        $Process = Start-Process -FilePath $Executable -ArgumentList $Arguments -PassThru -NoNewWindow -RedirectStandardOutput $Out -RedirectStandardError $Err
        if (-not $Process.WaitForExit(15000)) {
            # Kill the exact owned process; its kill-on-close Job reaps its children.
            $Process.Kill()
            $Process.WaitForExit()
            $Results += [pscustomobject]@{case=$Case; outer_timeout=$true; exit_code=$null; proof=$null}
            continue
        }
        $Process.Refresh()
        $Proof = $null
        $Text = Get-Content -Raw $Out
        if ($Text) { $Proof = $Text | ConvertFrom-Json }
        $Results += [pscustomobject]@{case=$Case; outer_timeout=$false; exit_code=$Process.ExitCode; proof=$Proof}
    } finally {
        if ($Process -and -not $Process.HasExited) { $Process.Kill(); $Process.WaitForExit() }
        if ($RestoreDesktop -ne [IntPtr]::Zero) {
            $ReleaseError = $null
            $ReleaseProcess = $null
            $ReleaseProof = $null
            try {
                if ($Process) {
                    $ReleaseOut = Join-Path $OutputDirectory "$Case.release.stdout.json"
                    $ReleaseErr = Join-Path $OutputDirectory "$Case.release.stderr.log"
                    # A fresh owned native thread can attach to the private input
                    # desktop even if this PowerShell thread owns native objects.
                    $ReleaseProcess = Start-Process -FilePath $Executable -ArgumentList @('--release-owned-ci-input', [string]$Process.Id) -PassThru -NoNewWindow -RedirectStandardOutput $ReleaseOut -RedirectStandardError $ReleaseErr
                    if (-not $ReleaseProcess.WaitForExit(2000)) { $ReleaseProcess.Kill(); $ReleaseProcess.WaitForExit(); throw 'External owned CI button release helper timed out' }
                    $ReleaseProcess.Refresh()
                    if ($ReleaseProcess.ExitCode -ne 0) { throw 'External owned CI button release failed' }
                    $ReleaseProof = Get-Content -Raw $ReleaseOut | ConvertFrom-Json
                    if (($ReleaseProof.released -eq $true) -eq ($ReleaseProof.owned_desktop_not_current -eq $true)) { throw 'External button release returned invalid proof shape' }
                }
            } catch { $ReleaseError = $_ } finally {
                if ($ReleaseProcess -and -not $ReleaseProcess.HasExited) { $ReleaseProcess.Kill(); $ReleaseProcess.WaitForExit() }
                # Release runs on the still-owned input desktop BEFORE switching.
                $Restored = [OwnedCiDesktopRestore]::SwitchDesktop($RestoreDesktop)
                $CursorRestored = $Restored -and [OwnedCiDesktopRestore]::SetCursorPos($Cursor.X, $Cursor.Y)
                [OwnedCiDesktopRestore]::CloseDesktop($RestoreDesktop) | Out-Null
            }
            if (-not $CursorRestored) { throw 'External owned CI desktop/cursor restoration failed' }
            if ($ReleaseError) { throw $ReleaseError }
            if ($Results.Count -gt 0 -and $Results[-1].case -eq $Case) {
                $Results[-1] | Add-Member -NotePropertyName external_ci_desktop_cursor_restored -NotePropertyValue $true
                $Results[-1] | Add-Member -NotePropertyName external_ci_button_release -NotePropertyValue $ReleaseProof
            }
        }
    }
}
$NormalizedSource = [System.Text.Encoding]::UTF8.GetBytes([System.IO.File]::ReadAllText($Source).Replace("`r`n", "`n"))
$Report = [ordered]@{
    source_sha256=(Get-FileHash -Algorithm SHA256 $Source).Hash.ToLowerInvariant()
    source_lf_sha256=[System.Convert]::ToHexString([System.Security.Cryptography.SHA256]::HashData($NormalizedSource)).ToLowerInvariant()
    executable_sha256=(Get-FileHash -Algorithm SHA256 $Executable).Hash.ToLowerInvariant()
    windows=[System.Environment]::OSVersion.VersionString
    ordinary_cases_private_station=$true
    ci_input_cases_private_desktop_on_owned_vm=[bool]$OwnedHostedCiInputDesktop
    changes_user_clipboard=$false
    owned_hosted_ci_input_opt_in=[bool]$OwnedHostedCiInputDesktop
    switches_input_desktop_only_in_owned_hosted_ci=[bool]$OwnedHostedCiInputDesktop
    synthesizes_left_button_only_on_owned_ci_desktop=[bool]$OwnedHostedCiInputDesktop
    injects_keyboard=$false
    cases=$Results
}
$Report | ConvertTo-Json -Depth 10 | Set-Content -Encoding utf8 (Join-Path $OutputDirectory 'report.json')
$Failures = $Results.Where({
    if ($OwnedHostedCiInputDesktop -and $_.case -in @('ole-normal', 'ole-query-stall', 'baseline-ole-query-stall')) {
        # An unresolved non-input desktop is a separate negative control, never
        # a substitute for any required CI-input or direct COM success.
        $_.outer_timeout -or !$_.proof -or !$_.proof.all_children_reaped -or ($_.exit_code -ne 0 -and !($_.exit_code -eq 1 -and $_.proof.enter_pid -eq 0 -and $_.proof.query_calls -eq 0))
    } else { $_.outer_timeout -or $_.exit_code -ne 0 }
})
if ($Failures.Count -ne 0) {
    throw 'A topology prerequisite failed; inspect report.json. This is not containment acceptance.'
}
