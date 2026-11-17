# Held private Windows receiver topology proof

This prerequisite is source only and has not been compiled or run by the author.
It changes no registered source file or workflow. Root must copy it into an
auxiliary native CI snapshot, review it, and run it before choosing a production
receiver architecture.

In an x64 MSVC developer PowerShell:

```powershell
pwsh -NoProfile -File .\run_probe.ps1 -OutputDirectory .\results
```

Exact compiler arguments are in `run_probe.ps1`: MSVC C++20, `/EHsc /W4 /O2`,
`_WIN32_WINNT=0x0A00`, and `ole32.lib user32.lib psapi.lib rpcrt4.lib`. There is no
third-party dependency, Cargo build or application launch. Output is one JSON
object per case plus `results/report.json`, source/executable SHA-256 and stderr.

## Evidence boundaries

`normal` and the direct fault cases call an independently marshaled IDropTarget
proxy from a synthetic source STA. They test the COM apartment and containment
boundary, including cross-process RegisterDragDrop against the controller's HWND.
They do **not** establish that OLE discovers that same registered endpoint.

`ole-normal`, `ole-query-stall` and `baseline-ole-query-stall` actually call
DoDragDrop, relying on a finite timer to post mouse-move messages to the private
source's own thread. They never move the system cursor or inject system input.
If Windows refuses non-input-desktop OLE, these cases must fail as an unresolved
prerequisite; never substitute the direct-proxy result or switch to the user's
input desktop. The fixture's deterministic IDropSource drops after observing
the target's first actual callback. No physical mouse/keyboard state is needed.

All private windows have fixed 128×128 geometry at the posted test coordinate;
there is no full-desktop/large backing surface or real desktop capture.

The baseline target owns its own HWND on its foreground STA and invokes an owned
synthetic IDataObject directly there. That deliberate failure control avoids
relying on which messages Windows might pump during an outgoing remote COM wait.
The separate source process drives the target/OLE call; baseline QueryGetData's
recorded PID/TID must be the HWND's foreground PID/TID. It does not represent a
measurement of every ordinary external source's COM wait behavior. The contained
target runs in a different process from the HWND-owning foreground STA. A posted
ping to the owned HWND during blocked QueryGetData must miss 50 ms in baseline
and complete within 50 ms in contained mode. This is a synthetic failure control,
not a latency distribution, physical-input result or GPUI benchmark.

Success requires real receiver/source PID/TID agreement, a normal data copy only
after ReleaseStgMedium returns, a single custom source-medium release, helper
memory-limit success/failure probes, and reaped source/helper handles. RPC
client PID is reported separately and asserted when Windows supplies it: it must
identify the receiver as the QueryGetData caller. A zero value is explicitly
reported as unavailable and does not manufacture caller-PID evidence. The source still executes
its implementation on its own STA; that is expected and does not put work on
the HWND-owning foreground thread.

`query-stall`, `get-stall` and `release-stall` block in the owned synthetic
source. `over-memory` returns a 128 MiB HGLOBAL, testing receiver unmarshaling
against its 96 MiB growth allowance before the 64 KiB returned-medium check.
That case additionally requires exactly one source GetData, a successfully
created 128 MiB source medium and an actual receiver memory-error HRESULT.
A successful GetData followed only by post-acquisition extent refusal does not
pass this memory proof. Query/GetData/cleanup HRESULTs, offered native bytes,
owned process exit codes and fixed failure-stage/Win32-error fields remain in
the JSON, including unsupported RegisterDragDrop results.
All can fail without publishing a copied payload; the externally enforced
five-second deadline covers query, GetData, native media and all cleanup. The
helper's baseline+96 MiB committed-memory ceiling is installed and allocation
probed before OleInitialize or registration. The outer kill-on-close Job has two
child slots and a 512 MiB per-child ceiling; children are placed atomically with
PROC_THREAD_ATTRIBUTE_JOB_LIST and inherit only a mapping and three events.

## Private resources and teardown

The controller creates a unique noninteractive window station and desktop.
It never calls SwitchDesktop, SetCursorPos, SendInput, clipboard APIs, capture,
printer, credential, account or network APIs. Its windows are shown only inside
that unselected private station. A shared mapping contains fixed numeric
instrumentation and at most 16 KiB of a trusted, owned target OBJREF. Synthetic
path bytes exist only in an owned HGLOBAL and are never opened or logged.

There are at most controller + receiver + source processes, one fixed mapping,
three private events and operation-scoped COM/native objects. The controller
waits on events/process handles while pumping its own private message queue. It
kills the owned Job and waits on the exact source/receiver process handles on
every normal result, then destroys its private HWND/station/desktop. A 15-second
external PowerShell limit also catches a stuck controller, closes its Job by
terminating that owned process and reports failure. It never kills a PID found
by name or enumeration. The case list takes at most roughly 60 seconds normally;
each individual native wait remains five seconds or less except the independent
15-second outer crash/hang guard.

No result from this prerequisite establishes all-format compatibility,
untrusted custom COM marshalers, Explorer/browser interoperability, main-app
integration, mixed DPI, application cancellation, production idle/process cost,
or complete native containment. Those remain subsequent gates.
