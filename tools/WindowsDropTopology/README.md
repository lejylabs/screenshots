# Held Windows receiver topology proof, revision 8

This is source-only verification work, not production integration. The author has
not compiled or executed this revision. Root must review the complete source and
run it only in its disposable GitHub-hosted Windows VM. All earlier frozen source
and actual CI failure artifacts remain unchanged. No probe has run on the user's
computer, read the user's clipboard, or accessed a printer/account/secret.

## Actual evidence and remaining question

Revision 3 run 44051463077 passes six direct COM cases, including normal medium
release, query/GetData/release five-second reaps and a successful 128 MiB source
HGLOBAL that fails receiver GetData with E_OUTOFMEMORY under the installed
96 MiB private-commit growth ceiling. Its three private DoDragDrop cases never
enter the target. Raw Windows CRLF source SHA-256 is
f0eab7b43aaf3a7a17cb632f2a7230e9a78fc5316eb09334e815be6313f4eaa0;
normalizing only CRLF to LF yields the frozen local b5151b8759367e87e235b82c5f457fa2aadbb51c241399d65d8e8af239574c9e.

Revisions 4 and 5 also pass all six direct cases, but fail the real OLE cases.
Revision 4 establishes GetCursorPos error 1459 on the noninteractive private
station and a valid (16,16) cursor/owned target on the explicitly selected CI
input desktop. Its source handles posted moves but native feedback occurs only
once. Revision 5's first cursor set is a no-op to the initial (16,16), then its
source STA's WM_TIMER does not dispatch inside DoDragDrop. Both revisions restore
the six CI desktops/cursors successfully; neither establishes OLE routing.

Revision 6 run 44059075939, job 133955895430, artifact 14316838348 again passes
six direct cases. Its independent mover makes roughly 247 actual cursor moves;
WindowFromPoint consistently identifies the owned ScreenshotsPrivateDropProbe
and correct owner PID, and source enumeration returns S_OK. Yet every CI OLE
case still has no QueryContinueDrag, GiveFeedback or target entry. All six
external desktop/cursor guards succeed. The ordinary source must therefore be
started from a real initiating gesture before making any routing inference.

Revision 7 initiates a real left-button event on its owned source window and
calls DoDragDrop from that window's WM_LBUTTONDOWN handler. It preserves every
previous outcome and adds source input, QueryInterface, enumeration Next and GUI
capture-stage diagnostics. A failure remains a failure; no direct-proxy result
is a substitute for actual OLE target discovery.

Revision 7 run 44067178252, job 133983753282, artifact 14319993444 passes
six direct cases but every CI case exits at controller SendInput(left-up) with
ERROR_ACCESS_DENIED (5). That final require suppressed the source-stage JSON;
these results establish no OLE behavior. Every external desktop/cursor restore
succeeds. The owned desktop access mask omitted DESKTOP_JOURNALPLAYBACK (0x20),
which is required for journal playback and needed by SendInput on that desktop.

Revision 8 adds only that right to creation of the owned CI desktop and the
external releaser's input-desktop handle. It changes no ACL, logon privilege,
security policy or ordinary private-desktop access. Native input is still
explicitly gated to the disposable hosted VM.

Every SendInput now independently checks both the current input desktop and its
calling thread's attached desktop against the exact generated owned desktop
name. A mismatch records a not-owned error and sends nothing; no original CI
input desktop is used as fallback. The mover remains pinned to its source-owned
desktop. Source and controller input errors have separate fields, and the first
native failure is retained with an Interlocked claim so a cleanup failure cannot
overwrite the original cause. A failed controller release is recorded and causes
test failure, while the controller still emits its entire bounded diagnostic
JSON and attempts normal restoration. The existing independent release/restore
guards remain active. Actual API rights/enforcement must still be demonstrated
by the next native run; no successful gesture or OLE routing is claimed here.

## Explicitly different execution scopes

Without opt-in, every native case uses a unique unselected noninteractive window
station and desktop. This mode performs no SwitchDesktop, SetCursorPos or
SendInput. Its posted-move negative controls remain in the result report.

The actual gesture mode requires a disposable dedicated GitHub-hosted Windows VM,
never the user's computer, another user's session or a self-hosted runner. Both
script and native code gate it with:

- explicit PowerShell -OwnedHostedCiInputDesktop;
- native --owned-ci-input-desktop and a ci- scenario;
- GITHUB_ACTIONS=true, RUNNER_OS=Windows, RUNNER_ENVIRONMENT=github-hosted;
- SCREENSHOTS_OWNED_HOSTED_CI_INPUT=1 set after the script verifies those flags.

These environment checks prevent accidental host execution; they are not a
cryptographic host attestation. The reviewed auxiliary workflow's windows-latest
runner and actual GitHub run identity establish where the test was launched.

The CI controller creates a uniquely named temporary WinSta0 desktop, switches
that VM's input to it, and uses only its owned source/target windows. The source
window is 64x64 at (256,256); the target is 128x128 at (0,0). No full-desktop
surface, screenshot, real browser, clipboard, network request or credential call
is involved. Only one synthetic left-button down/up and cursor movement are
used; there is no keyboard injection. The original left button must be up before
the case begins.

## Source gesture and thread ownership

The source STA creates its data object and native window, retaining all COM
pointers on that STA. One CI-only native mover thread requests a 256 KiB stack.
Before any User32 object is created on that new thread, it attaches the exact
source STA's owned desktop. Its context contains only a stop handle, desktop
handle and source HWND, never an IDataObject/COM pointer, frame or UI reference.
A kernel waitable timer wakes it every 20 ms only during this operation.

After source-window readiness, the mover goes to (272,272), verifies that the
window under that point is the exact owned source HWND, then sends one left-down.
The source STA's ordinary GetMessage/DispatchMessage loop receives WM_LBUTTONDOWN
and starts DoDragDrop in that handler. The mover waits for the atomic call-start
marker and an actual source enumeration/preflight/continue callback from inside
DoDragDrop. Its first target move is then (32,32), followed by (16,16), both within
the owned target. It never posts fake negative-coordinate moves in this mode.

Normal DoDragDrop completion signals the source-local stop event and joins the
exact mover thread handle before releasing COM objects or destroying the source
window. The mover sends left-up before exit. The controller's stop event also
wakes it on cancellation, and the existing five-second owned Job contains every
thread on a stall. Source/STA coordination uses Interlocked operations. A short
process-local SRW lock serializes only the fixed QueryInterface GUID diagnostic
copy; no foreign COM/native callback is made while that lock is held.

The source implements a one-element IEnumFORMATETC directly: exactly CF_HDROP,
HGLOBAL, CONTENT, lindex -1, with bounded Next/Skip/Reset/Clone behavior and
explicit Next call/result/fetched counters. This removes an opaque enumerator
from the diagnostic path. Fixed 16-entry FORMATETC traces separate OLE preflight
from the target's explicit QueryGetData/GetData stages. Fault injection and ping
signals occur only in the target acquisition stages, never during preflight.

## Three button/desktop cleanup boundaries

1. The source mover sends left-up and joins on normal completion. If any source
   callback stalls or exits, all its threads remain inside the externally killed
   and reaped Job.
2. After reaping exact receiver/source process handles, the controller always
   sends left-up on its still-selected owned CI desktop before restoring the
   original desktop or cursor. This covers source errors and forced source kills.
3. An independent PowerShell finally guard holds the original input desktop and
   cursor before launch. On normal exit, exception or the 15-second controller
   kill, it launches the same executable in --release-owned-ci-input CONTROLLER_ID
   mode before switching back. That fresh native helper verifies all CI gates
   and that the current input desktop name exactly matches the generated desktop
   for that owned controller PID. It attaches only that desktop and sends left-up.
   If the controller already restored the original desktop, the helper reports a
   skip and injects nothing. It never sends input to an unrelated desktop.

The external release helper has its own two-second process-handle timeout and
is killed by its exact Process object on failure. Regardless of release errors,
the outer finally still attempts original desktop/cursor restoration and reports
failure. No process is found or killed by name. Normal and fault-case reports
record source/controller button counts and external release/restoration outcomes.
A complete CI worker crash is outside these guards; the VM is disposable, with
no user session or user data to recover.

## Build, outputs and mandatory assertions

In a developer PowerShell, ordinary private diagnostics:

```powershell
pwsh -NoProfile -File .\run_probe.ps1 -OutputDirectory .\results
```

Only on the explicitly owned hosted Windows CI VM:

```powershell
pwsh -NoProfile -File .\run_probe.ps1 -OutputDirectory .\results -OwnedHostedCiInputDesktop
```

The script records exact MSVC C++20 /EHsc /W4 /O2 arguments, raw/native and
normalized-LF source SHA-256, executable SHA-256, per-case stdout/stderr and
report.json. No Cargo/application build or third-party dependency is used.
All six direct cases remain mandatory. Opted-in CI additionally requires six
actual OLE cases: normal, query/get/release stalls, over-memory and the foreground
baseline. The three old private no-target failures remain explicit bounded
negative controls only when these real CI OLE cases are also required.

Normal success requires actual receiver/source PID/TID routing, separate HWND
owner, successful GetData and cleanup, one custom medium release, publication only
after release, actual OLE DRAGDROP_S_DROP, and reaped exact process handles.
A posted ping during blocked QueryGetData must complete within 50 ms in the
contained HWND owner. The foreground failure control owns its HWND and invokes
an owned synthetic source directly on that STA; its blocked ping must fail.
This is a deterministic control, not every external source's COM wait behavior
or a physical input/presentation latency distribution.

The memory case requires exactly one target GetData, a successful 128 MiB source
allocation and an actual receiver memory-error HRESULT; returned-medium size
rejection alone cannot pass. CI assertions also require the correct mover
PID/TID, actual under-cursor owner PID/class, real left-down/source handler stage,
controller left-up and normal stop/join/source left-up. QueryInterface GUIDs,
GetGUIThreadInfo capture/active PID/class, enum completion and fixed failure/
HRESULT/process-exit fields remain diagnostic even when the case fails. RPC
caller PID is asserted only when Windows supplies it; zero means unavailable.

## Resource and evidence limits

The receiver installs/probes measured-private-baseline +96 MiB commit growth
before OLE/source access. The external five-second deadline covers acquisition,
media release and cleanup. Its kill-on-close Job has two child slots and 512 MiB
per-child commit ceiling. Child creation is atomic with the Job and inherits
only one bounded mapping and three events. Normal source data is 64 bytes; the
single memory fault intentionally creates 128 MiB in the owned source.

The fixture has controller + receiver + source during the gesture, one fixed
mapping with numeric instrumentation and <=16 KiB trusted target OBJREF, three
inherited events and the operation-only mover handles/stack. The separate button
release helper starts after controller completion/kill. No permanent timer,
abandoned application thread, source watcher, image cache or renderer is added.
Each controller is externally bounded to 15 seconds; the complete opted-in list
normally finishes within roughly 90 seconds.

No result establishes arbitrary custom-unmarshaler behavior, main-app integration,
Explorer/browser compatibility, DPI/accessibility, application cancellation,
production idle/helper startup cost or full performance acceptance. The separate
controlled custom IMarshal draft has not run and is not part of this revision.
Production hard containment and raw/URL adapter integration remain open.

Primary native access reference: [Desktop Security and Access Rights](https://learn.microsoft.com/en-us/windows/win32/winstation/desktop-security-and-access-rights).
The SendInput/journal-playback connection is also described by the archived
Microsoft staff post originally published at
https://blogs.msdn.microsoft.com/alejacma/2010/03/17/sendinput-doesnt-work-but-it-doesnt-return-an-error/ .
This source diagnosis remains subject to the owned native runtime result.
