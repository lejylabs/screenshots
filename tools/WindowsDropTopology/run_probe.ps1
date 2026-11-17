param([string]$OutputDirectory = (Join-Path $PSScriptRoot 'results'))
$ErrorActionPreference = 'Stop'
New-Item -ItemType Directory -Force $OutputDirectory | Out-Null
$Source = Join-Path $PSScriptRoot 'receiver_topology.cpp'
$Executable = Join-Path $OutputDirectory 'receiver_topology.exe'
# Run in a VS x64 developer environment. No Cargo/native application is launched.
& cl.exe /nologo /std:c++20 /EHsc /W4 /O2 /D_WIN32_WINNT=0x0A00 $Source "/Fe:$Executable" "/Fo:$OutputDirectory\receiver_topology.obj" ole32.lib user32.lib psapi.lib rpcrt4.lib
if ($LASTEXITCODE -ne 0) { throw 'Native topology fixture compilation failed' }
$Cases = @(
    'normal', 'query-stall', 'get-stall', 'over-memory', 'release-stall',
    'baseline-query-stall',
    'ole-normal', 'ole-query-stall', 'baseline-ole-query-stall'
)
$Results = @()
foreach ($Case in $Cases) {
    $Out = Join-Path $OutputDirectory "$Case.stdout.json"
    $Err = Join-Path $OutputDirectory "$Case.stderr.log"
    $Process = Start-Process -FilePath $Executable -ArgumentList $Case -PassThru -NoNewWindow -RedirectStandardOutput $Out -RedirectStandardError $Err
    if (-not $Process.WaitForExit(15000)) {
        # The parent owns a kill-on-close Job. Killing this owned PID closes it.
        Stop-Process -Id $Process.Id -Force
        $Process.WaitForExit()
        $Results += [pscustomobject]@{case=$Case; outer_timeout=$true; exit_code=$null; proof=$null}
        continue
    }
    $Process.Refresh()
    $Proof = $null
    $Text = Get-Content -Raw $Out
    if ($Text) { $Proof = $Text | ConvertFrom-Json }
    $Results += [pscustomobject]@{case=$Case; outer_timeout=$false; exit_code=$Process.ExitCode; proof=$Proof}
}
$Report = [ordered]@{
    source_sha256=(Get-FileHash -Algorithm SHA256 $Source).Hash.ToLowerInvariant()
    executable_sha256=(Get-FileHash -Algorithm SHA256 $Executable).Hash.ToLowerInvariant()
    windows=[System.Environment]::OSVersion.VersionString
    private_station=$true
    changes_user_clipboard=$false
    switches_input_desktop=$false
    sends_global_input=$false
    cases=$Results
}
$Report | ConvertTo-Json -Depth 10 | Set-Content -Encoding utf8 (Join-Path $OutputDirectory 'report.json')
if ($Results.Where({$_.outer_timeout -or $_.exit_code -ne 0}).Count -ne 0) {
    throw 'A topology prerequisite failed; inspect report.json. This is not containment acceptance.'
}
