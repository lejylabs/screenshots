param(
    [Parameter(Mandatory=$true)][string]$Application,
    [string]$OutputDirectory = (Join-Path $PSScriptRoot 'results'),
    [switch]$OwnedHostedCiInputDesktop
)
$ErrorActionPreference = 'Stop'
if ($env:GITHUB_ACTIONS -cne 'true' -or $env:RUNNER_OS -cne 'Windows' -or $env:RUNNER_ENVIRONMENT -cne 'github-hosted') { throw 'Only an owned disposable GitHub-hosted Windows VM may run this fixture' }
$Application = (Resolve-Path $Application).Path
New-Item -ItemType Directory -Force $OutputDirectory | Out-Null
$OutputDirectory = (Resolve-Path $OutputDirectory).Path
$Native = Join-Path $PSScriptRoot 'native'
$Source = Join-Path $OutputDirectory 'production_source.exe'
$Bridge = Join-Path $OutputDirectory 'production_bridge.dll'
$Controller = Join-Path $OutputDirectory 'production_controller.exe'
foreach ($Build in @(@('production_source.cpp', $Source, $false), @('production_bridge.cpp', $Bridge, $true), @('production_controller.cpp', $Controller, $false))) {
    $Arguments = @('/nologo','/std:c++20','/EHsc','/W4','/O2','/D_WIN32_WINNT=0x0A00',(Join-Path $Native $Build[0]),"/Fe:$($Build[1])","/Fo:$OutputDirectory\$($Build[0]).obj",'ole32.lib','oleaut32.lib','user32.lib','psapi.lib','rpcrt4.lib','shell32.lib','uuid.lib')
    if ($Build[2]) { $Arguments += '/LD' }
    & cl.exe @Arguments
    if ($LASTEXITCODE -ne 0) { throw "Owned native fixture compilation failed: $($Build[0])" }
}
$env:SCREENSHOTS_OWNED_PRODUCTION_ACCEPTANCE = '1'
$env:SCREENSHOTS_DROP_ACCEPTANCE_DLL = $Bridge
$env:SCREENSHOTS_DROP_ACCEPTANCE_SOURCE = $Source
Remove-Item Env:SCREENSHOTS_DROP_ACCEPTANCE_CONTROL -ErrorAction SilentlyContinue
$Cases = @(
    'protocol-register-remove-stop','protocol-bad-auth','protocol-withheld-startup-permit',
    'protocol-job-kill-reap','protocol-broken-command-pipe','protocol-broken-response-pipe','protocol-truncated-body','protocol-bad-magic',
    'protocol-reserved-bits','protocol-overflow-length','protocol-invalid-kind',
    'protocol-skipped-sequence','protocol-parent-death',
    'normal','custom-normal','query-stall','get-stall','release-stall','over-memory',
    'custom-unmarshal-stall','custom-unmarshal-memory','source-death','startup-stall','register-stall','revoke-stall','uninitialize-stall',
    'query-reenter','get-reenter','medium-reenter',
    'helper-factory-stall','helper-enter-stall','helper-drop-stall','helper-release-stall',
    'helper-enter-reenter','helper-drop-reenter','helper-release-reenter',
    'error-release-stall','error-release-reenter',
    'cancel-pending-body','a-close-b-live','same-hwnd-reclaim','withheld-body-256','hwnd-reuse',
    'gpui-file','gpui-image','gpui-url','gpui-busy','gpui-cancel','gpui-hide-show',
    'ordered-enter-submit','ordered-replacement-enter-submit','ordered-failure-publish'
)
if ($OwnedHostedCiInputDesktop) {
    $env:SCREENSHOTS_OWNED_HOSTED_CI_INPUT = '1'
    $Cases += @('ci-ole-normal','ci-ole-query-stall','ci-ole-get-stall','ci-ole-release-stall','ci-ole-over-memory','ci-custom-ole-normal','ci-custom-ole-unmarshal-stall','ci-custom-ole-unmarshal-memory','gpui-ole-image','ci-control-source-error','ci-control-source-none','ci-control-duplicate-acquire')
    Add-Type -TypeDefinition @'
using System;
using System.Runtime.InteropServices;
public static class ProductionDesktopRestore {
    [StructLayout(LayoutKind.Sequential)] public struct Point { public int X; public int Y; }
    [DllImport("user32.dll", SetLastError=true)] public static extern IntPtr OpenInputDesktop(uint flags, bool inherit, uint access);
    [DllImport("user32.dll", SetLastError=true)] public static extern bool SwitchDesktop(IntPtr desktop);
    [DllImport("user32.dll", SetLastError=true)] public static extern bool CloseDesktop(IntPtr desktop);
    [DllImport("user32.dll", SetLastError=true)] public static extern bool GetCursorPos(out Point point);
    [DllImport("user32.dll", SetLastError=true)] public static extern bool SetCursorPos(int x, int y);
    [DllImport("user32.dll")] public static extern short GetAsyncKeyState(int key);
    [DllImport("kernel32.dll")] private static extern uint GetCurrentThreadId();
    [DllImport("user32.dll")] private static extern IntPtr GetThreadDesktop(uint thread);
    [DllImport("user32.dll",CharSet=CharSet.Unicode,SetLastError=true)] private static extern bool GetUserObjectInformation(IntPtr handle,int index,System.Text.StringBuilder text,uint bytes,out uint actual);
    public static bool ThreadIsOnDesktop(IntPtr desktop) {
        var expected=new System.Text.StringBuilder(256);var current=new System.Text.StringBuilder(256);uint actual;
        return GetUserObjectInformation(desktop,2,expected,512,out actual) && GetUserObjectInformation(GetThreadDesktop(GetCurrentThreadId()),2,current,512,out actual) && String.Equals(expected.ToString(),current.ToString(),StringComparison.Ordinal);
    }
}
'@
} else { Remove-Item Env:SCREENSHOTS_OWNED_HOSTED_CI_INPUT -ErrorAction SilentlyContinue }
# Every controlled injection has a same-binary omission test. These run only on
# the private non-input station; an unrelated app failure is not accepted.
$NoOpCases = @('startup-stall','register-stall','revoke-stall','uninitialize-stall',
    'query-stall','get-stall','release-stall','custom-unmarshal-stall','custom-unmarshal-memory','over-memory',
    'query-reenter','get-reenter','medium-reenter','helper-factory-stall','helper-enter-stall','helper-drop-stall','helper-release-stall',
    'helper-enter-reenter','helper-drop-reenter','helper-release-reenter','error-release-stall','error-release-reenter')
$Cases += @('control-failed-as-leave-source-death','control-failed-as-leave-unmarshal')
$Cases += @('control-late-submit','control-noop-barrier','gpui-control-noop-busy-ack','gpui-control-noop-cancel-ack','control-wrong-terminal','control-early-failure-ack')
$Cases += @('control-source-error','control-source-none','control-duplicate-acquire','control-duplicate-release','control-busy-copy','control-busy-acquire','control-custom-source-error','gpui-control-source-error','gpui-control-source-none','gpui-control-duplicate-acquire','gpui-control-duplicate-release')
$Cases += @($NoOpCases | ForEach-Object {'noop-'+$_})
$Matrix = Get-Content -Raw (Join-Path $PSScriptRoot 'assertion-matrix.json') | ConvertFrom-Json
if ($Matrix.injections.Count -ne 22) {throw 'Incomplete required injection matrix'}
$SourceRules=Get-Content -Raw (Join-Path $PSScriptRoot 'source-oracle-rules.json') | ConvertFrom-Json
$Results = @()
foreach ($Case in $Cases) {
    $Output = Join-Path $OutputDirectory "$Case.stdout.jsonl"
    $ErrorLog = Join-Path $OutputDirectory "$Case.stderr.log"
    $ControllerLog = Join-Path $OutputDirectory "$Case.controller.json"
    $Identity = Join-Path $OutputDirectory "$Case.identity.json"
    $ControllerError = Join-Path $OutputDirectory "$Case.controller.stderr.log"
    foreach ($Path in @($Output,$ErrorLog,$ControllerLog,$ControllerError,$Identity)) { if (Test-Path $Path) { throw "Refusing to overwrite evidence: $Path" } }
    $ControllerProof = $null; $Process = $null; $Restore = [IntPtr]::Zero; $Cursor = $null; $Restored = $null
    try {
        if ($Case.StartsWith('ci-') -or $Case.StartsWith('gpui-ole-')) {
            $Restore = [ProductionDesktopRestore]::OpenInputDesktop(0, $false, 0x0181)
            $Cursor = [ProductionDesktopRestore+Point]::new()
            if ($Restore -eq [IntPtr]::Zero -or -not [ProductionDesktopRestore]::ThreadIsOnDesktop($Restore) -or -not [ProductionDesktopRestore]::GetCursorPos([ref]$Cursor)) {throw 'Cannot establish independent owned CI restoration guard'}
            if (([ProductionDesktopRestore]::GetAsyncKeyState(1) -band 0x8000) -ne 0) {throw 'Original owned VM left button must be up'}
        }
        # Exact argv are fixed case names and resolved generated paths. Each quoted path
        # rejects an embedded quote before passing Windows' native command line.
        foreach ($Path in @($Application,$Output,$ErrorLog,$Identity)) {if ($Path.Contains('"')) {throw 'Invalid owned fixture path'}}
        $Arguments = @('"'+$Application+'"',$Case,'"'+$Output+'"','"'+$ErrorLog+'"','"'+$Identity+'"')
        $Process = Start-Process -FilePath $Controller -ArgumentList $Arguments -PassThru -NoNewWindow -RedirectStandardOutput $ControllerLog -RedirectStandardError $ControllerError
        if (-not $Process.WaitForExit(270000)) {$Process.Kill(); $Process.WaitForExit(); throw "Outer controller watchdog: $Case"}
        $Process.Refresh()
        $ControllerProof = Get-Content -Raw $ControllerLog | ConvertFrom-Json
        $Proof = @(Get-Content $Output | Where-Object {$_} | ForEach-Object {$_ | ConvertFrom-Json})
        $Negative = $Case.StartsWith('noop-')
        $Canonical = if ($Negative) {$Case.Substring(5)} else {$Case}
        if ($Case -eq 'control-failed-as-leave-unmarshal') {$Canonical='custom-unmarshal-memory'}
        if ($Case -in @('ordered-failure-publish','control-early-failure-ack')) {$Canonical='helper-drop-stall'}
        $Expected = @($Matrix.injections | Where-Object {$_.runtime_cases -contains $Canonical})
        if ($Expected.Count -gt 1 -or ($Negative -and $Expected.Count -ne 1)) {throw 'Ambiguous/missing injection scenario mapping'}
        $Matching = if ($Expected.Count) {@($Proof | Where-Object {$_.injection_id -eq $Expected[0].id -and $_.visits -gt 0 -and $_.guard -eq 1 -and $_.sequence -gt 0 -and $_.sequence -eq $_.armed_sequence -and $_.observed_helper_pid -eq $_.helper_pid -and $_.observed_helper_tid -eq $_.helper_tid})} else {@()}
        $OmissionProof = @($Matching | Where-Object {$_.no_op_control -eq $true -and $_.fired -eq 0 -and $_.mandatory_injection_evidence -eq $false})
        $PositiveProof = @($Matching | Where-Object {$_.no_op_control -eq $false -and $_.fired -gt 0 -and $_.mandatory_injection_evidence -eq $true})
        $TeardownInitial = @($Proof | Where-Object {$_.initial_delivery_acknowledged -eq $true})
        $TeardownFinal = @($Proof | Where-Object {$_.post_close_teardown -eq $true -and $_.exact_receiver_reaped -eq $true})
        $TeardownPassed = $Expected.Count -eq 0 -or -not $Expected[0].teardown_after_initial_submit -or ($TeardownInitial.Count -eq 1 -and $TeardownFinal.Count -eq 1 -and ($Negative -or ($TeardownFinal[0].receiver_exit -eq 70 -and $TeardownFinal[0].ordered_after_close -eq $true -and $TeardownFinal[0].mandatory_injection_evidence -eq $true)))
        $NegativePassed = -not $Negative -or ($ControllerProof.negative_control_rejected -eq $true -and $ControllerProof.application_exit -eq 1 -and -not $ControllerProof.outer_timeout -and $ControllerProof.outer_job_empty -and $OmissionProof.Count -eq 1)
        $PositivePassed = $Negative -or $Expected.Count -eq 0 -or $PositiveProof.Count -eq 1
        $OracleControl=$Case -in @('control-failed-as-leave-source-death','control-failed-as-leave-unmarshal','control-late-submit','control-noop-barrier','gpui-control-noop-busy-ack','gpui-control-noop-cancel-ack','control-wrong-terminal','control-early-failure-ack')
        $SourceRule=@($SourceRules.rules|Where-Object {$_.case -eq $Case})
        $SourceControl=$SourceRule.Count -eq 1
        $OracleControl=$OracleControl -or $SourceControl
        $OracleProof=$true
        if ($Case -eq 'control-late-submit') {$OracleProof=@($Proof|Where-Object {$_.completion_barrier -eq $true -and $_.barrier_acknowledged -eq $true -and $_.enters -gt 0 -and $_.submits -gt 0 -and $_.reader_submits -gt 0 -and $_.rejection_verified -eq $false}).Count -eq 1 -and @($Proof|Where-Object {$_.drop_returned -eq 1 -and $_.drop_effect -eq 1 -and $_.rejection_effect_verified -eq $false}).Count -eq 1}
        if ($Case -eq 'control-noop-barrier') {$OracleProof=@($Proof|Where-Object {$_.completion_barrier -eq $true -and $_.barrier_acknowledged -eq $false -and $_.rejection_verified -eq $false}).Count -eq 1}
        if ($Case -eq 'gpui-control-noop-busy-ack') {$OracleProof=@($Proof|Where-Object {$_.consumer_path -eq 'busy-admission' -and $_.consumer_visited -eq $true -and $_.consumer_token -gt 0 -and $_.consumer_generation -gt 0 -and $_.consumer_acknowledged -eq $false}).Count -eq 1}
        if ($Case -eq 'gpui-control-noop-cancel-ack') {$OracleProof=@($Proof|Where-Object {$_.consumer_path -eq 'cancel-discard' -and $_.consumer_visited -eq $true -and $_.consumer_token -gt 0 -and $_.consumer_generation -gt 0 -and $_.consumer_acknowledged -eq $false}).Count -eq 1}
        if ($Case -in @('control-failed-as-leave-source-death','control-failed-as-leave-unmarshal')) {$OracleProof=@($Proof|Where-Object {$_.failure_terminal_mutated -eq $true -and $_.expected_kind -eq 14 -and $_.actual_kind -eq 12}).Count -ge 1 -and @($Proof|Where-Object {$_.wrong_terminal_rejected -eq $true -and $_.expected_kind -eq 14 -and $_.actual_kind -eq 12}).Count -eq 1}
        if ($Case -eq 'control-wrong-terminal') {$OracleProof=@($Proof|Where-Object {$_.wrong_terminal_rejected -eq $true -and $_.expected_kind -eq 13 -and $_.actual_kind -eq 12}).Count -eq 1}
        if ($Case -eq 'control-early-failure-ack') {$OracleProof=@($Proof|Where-Object {$_.reader_reaped_before_failure_publication -eq $true -and $_.retained_enter_before_transition -eq $true -and $_.completion_pending_before_transition -eq $false -and $_.early_ack_rejected -eq $true}).Count -eq 1}
        $SourcePassed=$true
        if ($SourceControl) {
            $Rule=$SourceRule[0]
            $Rows=@($Proof|Where-Object {$_.source_final_oracle -eq $true -and $_.expected_outcome -eq $Rule.outcome -and $_.mutation_id -eq $Rule.mutation_id -and $_.mutation_applied -gt 0 -and $_.mutation_context_verified -eq $true -and $_.source_oracle_passed -eq $false -and $_.phase_bad -eq 0 -and $_.source_natural_exit -eq $true -and $_.source_exit -eq 0 -and $_.source_cleanup_done -eq 1})
            $Match=$Rows.Count -eq 1
            if ($Match) {
                $Row=$Rows[0]
                switch ($Rule.reason) {
                    'wrong-hresult' {$Match=$Row.result_verified -eq $false -and $Row.payload_counts_verified -eq $true -and $Row.drop_returned -eq 1 -and ($Rule.ole -and $Row.drop_hresult -ne 262400 -or -not $Rule.ole -and $Row.drop_hresult -lt 0) -and $Row.submits -eq 1}
                    'wrong-effect' {$Match=$Row.result_verified -eq $false -and $Row.payload_counts_verified -eq $true -and $Row.drop_returned -eq 1 -and $Row.drop_effect -eq 0 -and $Row.submits -eq 1}
                    'duplicate-acquire' {$Match=$Row.result_verified -eq $true -and $Row.payload_counts_verified -eq $false -and $Row.helper_gets -eq 2 -and $Row.helper_get_success -eq 2 -and $Row.helper_releases -eq 2 -and $Row.helper_release_ends -eq 2 -and $Row.submits -eq 1}
                    'duplicate-release' {$Match=$Row.result_verified -eq $true -and $Row.payload_counts_verified -eq $false -and $Row.helper_gets -eq 1 -and $Row.helper_get_success -eq 1 -and $Row.helper_releases -eq 2 -and $Row.helper_release_ends -eq 2 -and $Row.duplicate_release_calls -eq 2 -and $Row.submits -eq 1}
                    'capacity-copy' {$Match=$Row.result_verified -eq $false -and $Row.payload_counts_verified -eq $true -and $Row.enter_returned -eq 1 -and $Row.enter_hresult -eq 0 -and $Row.enter_effect -eq 1 -and $Row.helper_gets -eq 0 -and $Row.submits -eq 0}
                    'capacity-acquire' {$Match=$Row.result_verified -eq $true -and $Row.payload_counts_verified -eq $false -and $Row.enter_returned -eq 1 -and $Row.enter_effect -eq 0 -and $Row.drop_returned -eq 0 -and $Row.helper_gets -eq 1 -and $Row.helper_releases -eq 1 -and $Row.submits -eq 0}
                    default {$Match=$false}
                }
            }
            if ($Rule.source_return_adapter) {$Match=$Match -and $Row.raw_ole_returned -eq 1 -and $Row.raw_ole_hresult -eq 262400 -and $Row.raw_ole_effect -eq 1 -and $Row.source_adapter_applied -gt 0 -and $Row.mutation_actor -eq 1}
            else {$Match=$Match -and $Row.mutation_guard_verified -eq $true -and $Row.mutation_actor -eq 0}
            if ($Rule.requires_editor) {$Match=$Match -and @($Proof|Where-Object {$_.gpui_editor_verified_before_source_oracle_failure -eq $true -and $_.exact_pixels_alpha -eq $true -and $_.editor_closed -eq $true}).Count -eq 1}
            $OracleProof=$Match
            $SourcePassed=$Match
        } elseif (-not $Negative) {
            $SourceRows=@($Proof|Where-Object {$_.source_final_oracle -eq $true})
            if ($SourceRows.Count) {$SourcePassed=@($SourceRows|Where-Object {$_.source_oracle_passed -ne $true}).Count -eq 0}
        }
        $OrderingPassed=$true
        if ($Case -in @('ordered-enter-submit','ordered-replacement-enter-submit')) {$OrderingPassed=@($Proof|Where-Object {$_.real_enter_consumed_before_drop -eq $true -and $_.generation -gt 0}).Count -eq 1 -and @($Proof|Where-Object {$_.actual_drop_after_consumed_enter -eq $true -and $_.returned_drop_effect -eq 1 -and $_.source_reaped -eq $true -and $_.ordered_source_passed -eq $true}).Count -eq 1 -and @($Proof|Where-Object {$_.ordered_delivery -eq $true -and $_.expected_kind -eq 13 -and $_.actual_kind -eq 13 -and $_.current -eq $true}).Count -ge 1}
        if ($Case -eq 'ordered-failure-publish') {$OrderingPassed=@($Proof|Where-Object {$_.reader_reaped_before_failure_publication -eq $true -and $_.retained_enter_before_transition -eq $true -and $_.completion_pending_before_transition -eq $true -and $_.early_ack_rejected -eq $false}).Count -eq 1 -and @($Proof|Where-Object {$_.failure_publication_completed -eq $true -and $_.failure_version -gt 0}).Count -eq 1 -and @($Proof|Where-Object {$_.terminal_rejection_consumed -eq $true -and $_.terminal_kind -eq 14}).Count -eq 1}
        if ($Case -eq 'ordered-failure-publish') {$OrderingPassed=$OrderingPassed -and @($Proof|Where-Object {$_.actual_enter_published_before_fault -eq $true}).Count -eq 1}
        if ($Case -eq 'ordered-failure-publish' -and $OrderingPassed) {$Version=@($Proof|Where-Object {$_.failure_publication_completed -eq $true})[0].failure_version;$Terminal=@($Proof|Where-Object {$_.terminal_rejection_consumed -eq $true})[0];$OrderingPassed=$Terminal.terminal_version -ge $Version}

        if ($Case -eq 'control-early-failure-ack') {$OracleProof=$OracleProof -and @($Proof|Where-Object {$_.actual_enter_published_before_fault -eq $true}).Count -eq 1}
        $OraclePassed=-not $OracleControl -or ($OracleProof -and $ControllerProof.application_exit -eq 1 -and $ControllerProof.negative_control_rejected -eq $true -and -not $ControllerProof.outer_timeout -and $ControllerProof.outer_job_empty)
        if ($Case -eq 'withheld-body-256') {$Capacity=@($Proof|Where-Object {$_.capacity_refusal_iteration -gt 0});$SourcePassed=$SourcePassed -and $Capacity.Count -eq 256 -and @($Capacity|Where-Object {$_.source_result_and_counts_verified -ne $true}).Count -eq 0 -and @($Capacity|Select-Object -ExpandProperty capacity_refusal_iteration -Unique).Count -eq 256 -and ($Capacity|Measure-Object -Property capacity_refusal_iteration -Maximum).Maximum -eq 256}
        if (-not $OracleControl -and -not $Negative) {$SourcePassed=$SourcePassed -and @($Proof|Where-Object {$_.source_final_oracle -eq $true -and $_.ole_prefetch_get -eq $true -and ($_.outside_created -le 0 -or $_.outside_released -ne $_.outside_created)}).Count -eq 0}
        $ConsumerPassed=$true
        if ($Case -eq 'gpui-busy') {$ConsumerPassed=@($Proof|Where-Object {$_.consumer_path -eq 'busy-admission' -and $_.consumer_acknowledged -eq $true}).Count -eq 1}
        if ($Case -eq 'gpui-cancel') {$Held=@($Proof|Where-Object {$_.consumer_path -eq 'body-held' -and $_.consumer_acknowledged -eq $true});$Discard=@($Proof|Where-Object {$_.consumer_path -eq 'cancel-discard' -and $_.consumer_acknowledged -eq $true});$ConsumerPassed=$Held.Count -eq 1 -and $Discard.Count -eq 1 -and $Held[0].consumer_token -eq $Discard[0].consumer_token -and $Held[0].consumer_generation -eq $Discard[0].consumer_generation}
        $CompletionPassed=$true
        if ($Case -eq 'same-hwnd-reclaim') {$ConsumerPassed=$ConsumerPassed -and @($Proof|Where-Object {$_.registration_wake_rejected_before_allow -eq $true -and $_.epoch -gt 1 -and $_.receiver_pid -gt 0}).Count -eq 1 -and @($Proof|Where-Object {$_.current_receiver_admitted -eq $true -and $_.epoch -gt 1 -and $_.receiver_pid -eq $_.allow_pid -and $_.allow_tid -gt 0}).Count -eq 1}
        if (-not $OracleControl -and -not $Negative -and @($Proof|Where-Object {$_.completion_barrier -eq $true}).Count) {$CompletionPassed=@($Proof|Where-Object {$_.completion_barrier -eq $true -and $_.barrier_acknowledged -eq $true -and $_.submits -eq 0 -and $_.reader_submits -eq 0 -and $_.rejection_verified -eq $true}).Count -eq 1 -and @($Proof|Where-Object {$_.terminal_rejection_consumed -eq $true -and $_.terminal_kind -in @(12,14)}).Count -eq 1}
        if ($Case -in @('source-death','custom-unmarshal-memory')) {$CompletionPassed=$CompletionPassed -and @($Proof|Where-Object {$_.terminal_rejection_consumed -eq $true -and $_.terminal_kind -eq 14}).Count -eq 1 -and @($Proof|Where-Object {$_.ordered_delivery -eq $true -and $_.expected_kind -eq 14 -and $_.actual_kind -eq 14 -and $_.current -eq $true}).Count -eq 1}
        $Results += [pscustomobject]@{case=$Case; exit_code=$Process.ExitCode; controller=$ControllerProof; proof=$Proof; negative_control=$Negative; omitted_injection_rejected=$NegativePassed; positive_injection_proved=$PositivePassed; teardown_proved=$TeardownPassed; oracle_control_rejected=$OraclePassed; source_oracle_proved=$SourcePassed; ordering_proved=$OrderingPassed; consumer_proved=$ConsumerPassed; completion_proved=$CompletionPassed; external_desktop_restored=$null}
    } finally {
        if ($Process -and -not $Process.HasExited) {$Process.Kill(); $Process.WaitForExit()}
        if ($Restore -ne [IntPtr]::Zero) {
            $ReleaseError = $null
            try {
                # The actual app PID is recorded by its unique private desktop name.
                # Find only the exact current owned generated name, never guess a host PID.
                # Native release helper accepts app PID; runner obtains it from the
                # controller proof emitted after reaping, before attempting restoration.
                $OwnedIdentity = if (Test-Path $Identity) {Get-Content -Raw $Identity | ConvertFrom-Json} else {$null}
                if ($OwnedIdentity -and $OwnedIdentity.desktop_owner_pid) {
                    $Release = Start-Process -FilePath $Source -ArgumentList @('--release-owned-ci-input',[string]$OwnedIdentity.desktop_owner_pid) -PassThru -NoNewWindow -RedirectStandardOutput (Join-Path $OutputDirectory "$Case.release.json") -RedirectStandardError (Join-Path $OutputDirectory "$Case.release.stderr.log")
                    if (-not $Release.WaitForExit(2000)) {$Release.Kill(); $Release.WaitForExit(); throw 'Owned CI release helper timed out'}
                    if ($Release.ExitCode -ne 0) {throw 'Owned CI release helper failed'}
                } else {throw 'Missing exact owned app PID for external button release'}
            } catch {$ReleaseError = $_} finally {
                $Restored = [ProductionDesktopRestore]::SwitchDesktop($Restore)
                $Restored = $Restored -and [ProductionDesktopRestore]::ThreadIsOnDesktop($Restore) -and [ProductionDesktopRestore]::SetCursorPos($Cursor.X,$Cursor.Y)
                [ProductionDesktopRestore]::CloseDesktop($Restore) | Out-Null
            }
            if (-not $Restored) {throw 'Owned CI desktop/cursor restoration failed'}
            if ($ReleaseError) {throw $ReleaseError}
            if ($Results.Count -gt 0 -and $Results[-1].case -eq $Case) {$Results[-1].external_desktop_restored=$true}
        }
    }
}
$Files = Get-ChildItem -File -Recurse $Native
$Sources = @($Files | ForEach-Object {
    $Normalized = [Text.Encoding]::UTF8.GetBytes([IO.File]::ReadAllText($_.FullName).Replace("`r`n","`n"))
    [pscustomobject]@{name=$_.Name; sha256=(Get-FileHash -Algorithm SHA256 $_.FullName).Hash.ToLowerInvariant(); lf_sha256=[Convert]::ToHexString([Security.Cryptography.SHA256]::HashData($Normalized)).ToLowerInvariant()}
})
$Report = [ordered]@{status='executed-owned-ci'; fixture_revision=6; source_oracle_rules_sha256=(Get-FileHash -Algorithm SHA256 (Join-Path $PSScriptRoot 'source-oracle-rules.json')).Hash.ToLowerInvariant(); authored_case_inventory=$Cases.Count; executed_case_results=$Results.Count; assertion_matrix_sha256=(Get-FileHash -Algorithm SHA256 (Join-Path $PSScriptRoot 'assertion-matrix.json')).Hash.ToLowerInvariant(); github_run_id=$env:GITHUB_RUN_ID; github_run_attempt=$env:GITHUB_RUN_ATTEMPT; github_sha=$env:GITHUB_SHA; rustflags=$env:RUSTFLAGS; application_sha256=(Get-FileHash -Algorithm SHA256 $Application).Hash.ToLowerInvariant(); bridge_sha256=(Get-FileHash -Algorithm SHA256 $Bridge).Hash.ToLowerInvariant(); source_executable_sha256=(Get-FileHash -Algorithm SHA256 $Source).Hash.ToLowerInvariant(); controller_sha256=(Get-FileHash -Algorithm SHA256 $Controller).Hash.ToLowerInvariant(); native_sources=$Sources; windows=[Environment]::OSVersion.VersionString; owned_hosted_ci_input_opt_in=[bool]$OwnedHostedCiInputDesktop; cases=$Results; physical_hardware_acceptance=$false}
$Report | ConvertTo-Json -Depth 12 | Set-Content -Encoding utf8 (Join-Path $OutputDirectory 'report.json')
$Failures = @($Results | Where-Object {$_.exit_code -ne 0 -or -not $_.controller.passed -or -not $_.controller.outer_job_empty -or -not $_.omitted_injection_rejected -or -not $_.positive_injection_proved -or -not $_.teardown_proved -or -not $_.oracle_control_rejected -or -not $_.consumer_proved -or -not $_.completion_proved -or -not $_.ordering_proved -or -not $_.source_oracle_proved})
if ($Failures.Count) {throw 'Actual production acceptance failed; preserve all evidence and inspect report.json'}
