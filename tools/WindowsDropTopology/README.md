# Held Windows receiver topology proof, revision 6

This is verification source only. It has not been compiled or executed by the
subagent. The frozen previous native files and all earlier actual CI artifacts
remain unchanged. Root must review and copy this entire revision into its
auxiliary Windows CI snapshot. There are no production source changes.

## Actual evidence so far

Revision 3's actual native Windows run is 44051463077, job 133930070373, artifact
14314066479. All six direct COM cases pass, including a 128 MiB source HGLOBAL
that succeeds in the source and returns E_OUTOFMEMORY in the 96 MiB growth-limited
receiver. Query, GetData and release stalls are reaped after five seconds. Its
three noninteractive DoDragDrop cases never entered the target. Their single
source QueryGetData was OLE preflight, so those cases establish no OLE routing.
The actual CRLF artifact source SHA-256 is f0eab7b43aaf3a7a17cb632f2a7230e9a78fc5316eb09334e815be6313f4eaa0;
normalizing only CRLF to LF gives b5151b8759367e87e235b82c5f457fa2aadbb51c241399d65d8e8af239574c9e.

## New diagnostics and source model

Actual revision 4 run 44054603220, job 133940606080, artifact 14314743711 passed
all six direct cases again. All six CI input-desktop cases still failed before
target entry even though GetCursorPos succeeded at (16,16), WindowFromPoint
identified the owned target, and registration succeeded. Their source loops
processed roughly 300 posted-move timer ticks but issued drag feedback only once.
All six external desktop/cursor restoration guards succeeded. The old private
cases record GetCursorPos error 1459, independently establishing that limitation.

Revision 5 attempted to change movement in the explicitly gated hosted CI mode: its
finite timer alternates real SetCursorPos positions (16,16) and (32,32), both
inside the owned 128x128 target, while DoDragDrop owns native mouse capture.
Actual revision 5 run 44055717794, job 133944440564, artifact 14315697484 again
passed six direct cases, while every CI OLE case made only one movement attempt,
never called QueryContinueDrag/GiveFeedback, and recorded one enumerator call.
The first attempted position was the unchanged initial (16,16), so it generated
no movement. The STA WM_TIMER callback then did not run inside DoDragDrop. All
six external restoration guards succeeded. Those failures remain preserved.

Revision 6 uses one operation-owned source-process mover thread only in the
explicit hosted CI mode. Before creating any User32 object on that new thread,
it attaches the source STA's owned private desktop. A kernel waitable timer wakes
it every 20 ms only during the gesture; it waits for both the call-start marker
and an actual source enumeration/preflight/continue callback from DoDragDrop
before its first real movement, (32,32), then (16,16). It waits on its
local stop event, the controller stop event and its timer. Normal DoDragDrop
completion signals stop and joins the exact thread handle before source cleanup.
Blocked cases remain inside the same externally killed/reaped source Job. It
carries no COM object, IDataObject pointer, UI owner or frame across threads.
The call-start marker and callback counters use Interlocked reads/writes for
coordination between the source STA and its mover.

The thread requests a 256 KiB stack and owns one stop event, one waitable timer
and its join handle. These are fixture-only, bounded by the five-second source
process deadline, with no thread or timer left in the application. Its PID/TID,
start/stop markers, real move count and the actual window PID/class under the
cursor are recorded and asserted. Source enumeration's return HRESULT is also
recorded separately from its call count. Ordinary noninteractive cases keep the
old posted-move diagnostic and create no mover thread. No mouse button/keyboard
injection is used.
The ordinary noninteractive cases and all previous artifacts remain unchanged.

Only a target's explicit QueryGetData/GetData acquisition stages trigger fault
injection or its ready/ping signal. OLE preflight queries, gets and medium
releases have separate counters. A fixed 16-entry numeric FORMATETC trace records
format, aspect, tymed, lindex, stage and PID/TID without payload contents.
The source advertises exactly CF_HDROP through SHCreateStdEnumFmtEtc. Its private
source HWND receives finite posted WM_MOUSEMOVE messages. Source loop/timer,
GetCursorPos result/error, GetMessagePos and WindowFromPoint diagnostics help
distinguish missing input-desktop authority from registration/COM routing.

Windows documents that GetCursorPos requires the input desktop, while DoDragDrop
selects its target from the cursor's location. Noninteractive private desktop
failures are preserved as diagnostic results. They are never substituted for a
passing OLE source-to-receiver proof.

## Two explicitly different environments

Without opt-in, every case creates its own unselected noninteractive window
station and desktop. No SwitchDesktop, SetCursorPos, SendInput, clipboard,
printer, account, credential, network or capture call runs in this mode. All
windows are fixed 128×128 or 64×64, with no full-desktop backing surface.

The real OLE proof additionally requires a disposable, dedicated, GitHub-hosted
Windows VM. It is NEVER a mode for the developer's desktop, a self-hosted runner,
or another user's session. It requires all of:

- PowerShell switch -OwnedHostedCiInputDesktop;
- native private CLI flag --owned-ci-input-desktop and a ci- scenario;
- GITHUB_ACTIONS=true, RUNNER_OS=Windows and RUNNER_ENVIRONMENT=github-hosted;
- SCREENSHOTS_OWNED_HOSTED_CI_INPUT=1, set only after the script's environment gate.

In that isolated VM, the controller creates a uniquely named temporary desktop
on WinSta0 and switches the CI VM's input desktop to it. It sets only that owned
CI cursor inside (16,16)..(32,32), then runs actual DoDragDrop over the owned target HWND.
There is no system mouse/keyboard injection, user clipboard access, real browser,
printer, screenshot or secret lookup. The source uses the finite owned mover described above in CI mode, and its
IDropSource drops only after the real target has been entered.

The controller restores the original CI input desktop/cursor on normal cleanup.
An independent PowerShell finally guard holds the original desktop handle and
cursor before launch. It restores both after normal exit, any exception, or the
15-second outer hang kill. The exact owned process object is killed; its
kill-on-close Job closes the receiver/source descendants. No PID-by-name lookup
or unrelated process termination is used. Any restoration failure is a test
failure, with per-case stdout/stderr retained. A whole CI worker crash is outside
this guard; the VM is disposable, with no user session to recover.

## Native commands and acceptance

In an x64 MSVC developer PowerShell, ordinary private diagnostics:

```powershell
pwsh -NoProfile -File .\run_probe.ps1 -OutputDirectory .\results
```

Only in the explicitly owned GitHub-hosted Windows CI VM:

```powershell
pwsh -NoProfile -File .\run_probe.ps1 -OutputDirectory .\results -OwnedHostedCiInputDesktop
```

Exact compiler arguments are in the script: C++20, /EHsc /W4 /O2,
_WIN32_WINNT=0x0A00 and ole32.lib user32.lib psapi.lib rpcrt4.lib shell32.lib.
There is no third-party dependency or Cargo/application build. Raw source,
normalized LF source and executable SHA-256 are recorded in report.json.

The six direct COM cases remain mandatory. All six ci- OLE cases are mandatory
when input-desktop opt-in is enabled: normal, query/get/release stall, over-memory,
and a blocked foreground baseline. The earlier three noninteractive OLE cases
remain in the report; a bounded no-target failure can serve as the explicit
negative control only when the six actual CI-input OLE cases are also required.
Without the opt-in, those failures still fail the prerequisite script.

Normal success requires actual receiver/source apartment PID/TID agreement,
registered HWND owner separation, successful GetData and cleanup, one custom
medium release, no publication before release, actual DRAGDROP_S_DROP for OLE,
and reaped exact process handles. A fixed posted ping during blocked QueryGetData
must complete within 50 ms for the contained HWND owner. The baseline owns its
HWND on its foreground STA and deliberately invokes an owned synthetic source
directly on that STA, so its blocked ping is a deterministic failure control.
The baseline is explicitly labeled and is not a measurement of every ordinary
remote COM wait or a physical-input latency distribution.

The over-memory case requires exactly one target source GetData, successful
creation of the 128 MiB source medium, and an actual receiver memory-error
HRESULT. Merely rejecting the returned medium's size does not pass this proof.
Query/GetData/cleanup HRESULTs, native stages, process exit codes and fixed
failure-stage/Win32-error fields remain in the JSON, including unsupported
RegisterDragDrop outcomes. RPC client PID is asserted when supplied; zero is
reported as unavailable and never invented.

## Containment and resource scope

The receiver installs measured-private-baseline +96 MiB committed-memory growth
before OleInitialize/registration or foreign data access. A small successful and
oversized failed commit probe establishes enforcement. The external controller's
five-second deadline covers native query, GetData, media/release and cleanup.
The controller owns an outer kill-on-close Job with at most two child processes
and 512 MiB per-child commit ceiling. Children are atomically placed in that Job
and inherit only one bounded mapping and three events. There is no abandoned
thread or permanent timer. Normal cases retain at most a 64-byte source medium;
the single over-memory source case deliberately allocates 128 MiB. The private
source timer/mover exists only within the five-second gesture interval.

At most controller + receiver + source processes exist. The mapping contains
numeric instrumentation and a trusted owned target OBJREF capped at 16 KiB.
Synthetic path bytes stay in owned native memory and are never opened or logged.
The observer/controller waits on events and exact process handles while pumping
its own queue. Each case is externally limited to 15 seconds; the complete
opted-in list normally finishes within roughly 90 seconds.

These synthetic checks do not establish all-format/custom-marshaler safety,
Explorer/browser compatibility, main-app integration, DPI/accessibility,
application cancellation, parked-production-helper cost or complete acceptance.
Passing direct cases alone continues to establish no native OLE routing proof.

Microsoft references:
[GetCursorPos](https://learn.microsoft.com/en-us/windows/win32/api/winuser/nf-winuser-getcursorpos),
[DoDragDrop](https://learn.microsoft.com/en-us/windows/win32/api/ole2/nf-ole2-dodragdrop),
[SHCreateStdEnumFmtEtc](https://learn.microsoft.com/en-us/windows/win32/api/shlobj_core/nf-shlobj_core-shcreatestdenumfmtetc).
