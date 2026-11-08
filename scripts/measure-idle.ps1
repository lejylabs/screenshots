param([ValidateRange(10, 600)][int]$Seconds = 30, [ValidateSet("normal", "empty", "headless")][string]$Mode = "normal")
$ErrorActionPreference = 'Stop'
$workspace = [IO.Path]::GetFullPath((Join-Path $PSScriptRoot '..'))
$binary = Join-Path $workspace 'target/release/screenshots.exe'
if (-not (Test-Path -LiteralPath $binary)) { throw 'Build cargo build --release -p screenshots first.' }
if (Get-Process -Name screenshots -ErrorAction SilentlyContinue) { throw 'Close the running screenshots instance first.' }
$output = Join-Path $workspace 'target/performance'
New-Item -ItemType Directory -Force -Path $output | Out-Null
Add-Type @'
using System;
using System.Text;
using System.Runtime.InteropServices;
public static class IdleProbe {
    delegate bool EnumProc(IntPtr window, IntPtr data);
    [DllImport("user32.dll")] static extern bool EnumWindows(EnumProc callback, IntPtr data);
    [DllImport("user32.dll")] static extern uint GetWindowThreadProcessId(IntPtr window, out uint process);
    [DllImport("user32.dll", CharSet=CharSet.Unicode)] static extern int GetWindowText(IntPtr window, StringBuilder text, int count);
    [DllImport("user32.dll")] public static extern bool ShowWindowAsync(IntPtr window, int command);
    [DllImport("user32.dll")] public static extern bool IsWindowVisible(IntPtr window);
    [DllImport("user32.dll")] public static extern bool PostMessage(IntPtr window, uint message, IntPtr w, IntPtr l);
    public static IntPtr FindControl(uint process) {
        IntPtr found = IntPtr.Zero;
        EnumWindows((window, data) => {
            uint owner; GetWindowThreadProcessId(window, out owner);
            var title = new StringBuilder(256); GetWindowText(window, title, title.Capacity);
            if (owner == process && title.ToString().StartsWith("screenshots")) { found = window; return false; }
            return true;
        }, IntPtr.Zero);
        return found;
    }
}
'@
$launch = @{ }
if ($Mode -ne 'normal') { $launch.ArgumentList = @("--resource-probe=$Mode", "--probe-seconds=$($Seconds + 15)") }
$child = Start-Process @launch -FilePath $binary -WorkingDirectory $workspace -WindowStyle Hidden -PassThru -RedirectStandardOutput (Join-Path $output "idle-$Mode.stdout.log") -RedirectStandardError (Join-Path $output "idle-$Mode.stderr.log")
$window = [IntPtr]::Zero
try {
    $startup = [Diagnostics.Stopwatch]::StartNew()
    while ($Mode -ne "headless" -and $window -eq [IntPtr]::Zero) {
        if ($child.HasExited -or $startup.Elapsed.TotalSeconds -gt 15) { throw 'Release app did not open its control window.' }
        $window = [IdleProbe]::FindControl($child.Id)
        Start-Sleep -Milliseconds 50
    }
    if ($Mode -ne "headless") { Start-Sleep -Seconds 1 }
    if ($Mode -ne "headless" -and -not [IdleProbe]::ShowWindowAsync($window, 0)) { throw 'Could not request hidden control window.' }
    Start-Sleep -Seconds 5
    if ($Mode -ne "headless" -and [IdleProbe]::IsWindowVisible($window)) { throw 'Control window is still visible.' }
    $child.Refresh()
    $cpuBefore = $child.TotalProcessorTime.TotalSeconds
    $timer = [Diagnostics.Stopwatch]::StartNew()
    $samples = @()
    for ($index = 0; $index -lt $Seconds; $index++) {
        Start-Sleep -Seconds 1
        $child.Refresh()
        if ($child.HasExited) { throw 'App exited during measurement.' }
        if ($Mode -ne "headless" -and [IdleProbe]::IsWindowVisible($window)) { throw 'Control window became visible; run invalid.' }
        $samples += [pscustomobject]@{ elapsedSeconds = $timer.Elapsed.TotalSeconds; threads = $child.Threads.Count; handles = $child.HandleCount; privateBytes = $child.PrivateMemorySize64; workingSetBytes = $child.WorkingSet64; cpuSeconds = $child.TotalProcessorTime.TotalSeconds }
    }
    $elapsed = $timer.Elapsed.TotalSeconds
    $cpu = $child.TotalProcessorTime.TotalSeconds - $cpuBefore
    if ($samples.Count -ne $Seconds -or $elapsed -lt $Seconds) { throw 'Invalid sample interval.' }
    $report = [pscustomobject]@{
        capturedAtUtc = [DateTime]::UtcNow.ToString('o'); scenario = "release resource probe: $Mode"; warmupSeconds = 5; visibleSettleSeconds = $(if ($Mode -eq "headless") { 0 } else { 1 })
        durationSeconds = $elapsed; cpuSeconds = $cpu; cpuPercentOneLogicalCore = 100 * $cpu / $elapsed
        privateBytesMax = ($samples | Measure-Object privateBytes -Maximum).Maximum
        workingSetBytesMax = ($samples | Measure-Object workingSetBytes -Maximum).Maximum
        threadsMax = ($samples | Measure-Object threads -Maximum).Maximum
        handlesMax = ($samples | Measure-Object handles -Maximum).Maximum
        executableSha256 = (Get-FileHash -LiteralPath $binary -Algorithm SHA256).Hash
        logicalProcessors = [Environment]::ProcessorCount
        cpuModel = (Get-CimInstance Win32_Processor | Select-Object -First 1 -ExpandProperty Name)
        osVersion = [Environment]::OSVersion.VersionString
        sourceCommit = (git -C $workspace rev-parse HEAD)
        sourceDirty = [bool](git -C $workspace status --porcelain)
        limitations = 'Exploratory single run; not the five-minute reference-hardware gate. No GPU memory, wakeup, energy, cold-launch or post-capture measurement.'
        samples = $samples
    }
    $report | ConvertTo-Json -Depth 4 | Set-Content -LiteralPath (Join-Path $output "idle-$Mode.json") -Encoding UTF8
    $report | Select-Object durationSeconds,cpuPercentOneLogicalCore,privateBytesMax,workingSetBytesMax | Format-List
} finally {
    if ($Mode -eq 'headless' -and -not $child.HasExited) { if (-not $child.WaitForExit(20000)) { Write-Warning 'Headless probe did not exit on its deadline.' } }
    if (-not $child.HasExited -and $window -ne [IntPtr]::Zero) {
        [IdleProbe]::PostMessage($window, 0x0010, [IntPtr]::Zero, [IntPtr]::Zero) | Out-Null
        if (-not $child.WaitForExit(10000)) { Write-Warning 'Measured process did not quit; close it manually. It was not force-killed.' }
    }
    $child.Dispose()
}
