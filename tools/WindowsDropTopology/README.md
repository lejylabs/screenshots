# Held first-unmarshal proof, revision 9

This revision is uncompiled/unexecuted by its author and is held for root's
review and auxiliary Windows CI. Copy all four files, including
`custom_marshal.hpp`. It changes no production application source. Native input
is permitted only in the existing explicitly gated disposable GitHub-hosted VM,
never on the user's computer or a self-hosted runner.

## Proven base and new question

Actual revision 8 run 44075604618, job 134012674293, artifact 14324198695 passes
all six direct COM and all six real CI OLE cases. Its three non-input cases
remain explicit negative controls. Cross-process RegisterDragDrop against the
separate HWND owner actually routes OLE callbacks to the helper STA. Normal
transfer releases the synthetic medium once; query/GetData/release stalls are
killed and reaped after the requested five-second deadline while the HWND owner
responds to a posted ping. The foreground failure control misses that ping.
A successful 128 MiB source HGLOBAL returns receiver E_OUTOFMEMORY under its
installed/probed +96 MiB private-commit limit, without copy publication. All
controller button releases and external desktop/cursor restores succeed.

The actual revision 8 CRLF source hash is
`0d24c62d9783e3cdd84b8340e92817919ab041224c75454583ead7bd697966a0`;
normalizing only CRLF to LF matches local
`30be918f2f82d7ae1391c199aa4d0c93995db9e6da0af3a4fbc744ef63811a54`.
The artifact ZIP digest is
`adbcd1df4eca5987dd940b07903c3d82ad0580a2d4e3e62c8da1d39fba8ce2fc`.
These are synthetic process/OLE controls, not full app or hardware acceptance.

Callback location alone does not establish where the IDataObject parameter was
first unmarshaled. It also does not establish a usable watchdog trigger in a
parked receiver: starting a deadline inside DragEnter is too late for argument
unmarshaling. Revision 9 tests both explicitly.

## Controlled custom source

CustomSource implements IDataObject and IMarshal. Its outer custom packet adds
one fixed four-byte marker, then delegates a nested standard OBJREF for an owned
normal Source. There is no image, file opening, arbitrary blob growth, downloaded
DLL or registry write. The same executable registers an in-process class factory
in each fixture process with CoRegisterClassObject(CLSCTX_INPROC_SERVER,
REGCLS_MULTIPLEUSE), then revokes it on normal cleanup. Microsoft documents this
registration combination. The ordinary source/target cases remain unchanged in
their advertised single CF_HDROP format and input gesture.

The factory records the first non-source PID/TID and dispatch-guard sequence
before allocating its unmarshaler. The unmarshaler separately records its first
non-source PID/TID and guard sequence before delegating the standard packet.
Both must be the helper STA and lie inside the same externally armed interval.
Unmarshaler final Release/ReleaseMarshalData outside that interval is counted
and fails the boundary proof. Main-foreground execution fails the proof rather
than being described as acceptable marshaling.

The controlled code returns an error if it observes first unmarshal in a process
other than the receiver, instead of deliberately hanging the observer. In the
receiver, `unmarshal-stall` actually blocks before target entry, and
`unmarshal-memory` tries an actual 128 MiB commit before target entry. The latter
must fail under the preinstalled +96 MiB limit. The original source GetData does
not run in those two cases. No source error or callback PID substitutes for this
first-boundary evidence.

## Guard before native dispatch

The receiver waits event-driven while idle. When its native queue wakes, it
announces a new sequence using an inherited auto-reset event and waits for the
controller's armed acknowledgement. Only after the matching acknowledgement
does it mark its dispatch interval active and call PeekMessage/DispatchMessage.
PeekMessage itself can dispatch sent messages, so the guard is established before
that call, rather than merely before DispatchMessage. Each batch handles at most
32 queued messages, then clears the guard and signals completion.

The fixture controller's absolute five-second deadline is already active before
the source starts. Its event/message wait acknowledges dispatch sequences without
resetting or extending that deadline. This verifies an arm-before-dispatch
handshake with a deadline independent of the receiver thread. A production
parked controller must supply an equivalent per-interval deadline; the fixture's
whole-case deadline is deliberately stronger and does not include human delay.

If the factory/unmarshal runs before the guard, on another thread/apartment or
in the HWND owner's process, this revision fails and the receiving architecture
needs another boundary. Do not relabel the already passing ordinary callback
cases as complete containment. A source-provided marshaler in the external
source process is outside the application's process-memory budget; this proof
concerns code executing inside the receiving application/helper.

## Cases and assertions

The six original direct cases and six original real CI OLE cases remain required.
Add three direct and three real OLE custom cases:

- normal transfer with actual custom MarshalInterface, helper factory/unmarshal
  PID/TID, matching armed/active sequences, ordinary GetData and medium release;
- first-unmarshal stall before target entry, no target query/data/copy, responsive
  HWND-owner ping, externally killed and reaped processes;
- first-unmarshal allocation refusal before target entry, no target query/data/
  copy, actual failed 128 MiB commit and responsive owner.

Normal custom cases additionally retain all existing data, cleanup and genuine
DoDragDrop assertions. Custom failure cases report `enter_pid=0` truthfully and
have a separate positive `first_unmarshal_boundary_verified` field. They may not
pretend target entry happened. RPC client PID remains asserted only when Windows
provides it. All new first-factory/unmarshal/release/sequence fields are retained
in the JSON even when an assertion fails.

## Existing owned input and cleanup safeguards

The script requires -OwnedHostedCiInputDesktop, the private native flag/scenario,
GITHUB_ACTIONS=true, RUNNER_OS=Windows, RUNNER_ENVIRONMENT=github-hosted and the
explicit SCREENSHOTS_OWNED_HOSTED_CI_INPUT gate. These prevent accidental host
execution; they are not a cryptographic attestation. The reviewed hosted workflow
and actual GitHub run identity establish the execution environment.

The controller creates a unique private WinSta0 desktop with journal-playback
access, switches only the owned CI VM, and owns fixed source/target windows. A
source-process mover attaches the exact source desktop, presses only on the
verified source HWND at (272,272), then moves to (32,32)/(16,16) only after an
actual source callback from DoDragDrop. DoDragDrop starts in the source window's
real WM_LBUTTONDOWN handler. No COM pointer crosses that mover thread.

The mover releases and joins normally. The controller always attempts left-up
after exact source/helper process reaping and before restoration. Every SendInput
checks both current input and attached thread desktop against the exact owned
generated name; an ownership change sends nothing. First failure is preserved,
source/controller input errors remain separate, and controller errors retain
bounded JSON. No ACL, global policy or logon privilege is changed.

The independent PowerShell finally guard runs the same executable in the gated
--release-owned-ci-input mode before switching back. That fresh process verifies
the exact current owned desktop name, attaches only it, releases the button, and
has a two-second exact-process timeout. If normal controller restoration already
occurred, it reports a skip and injects nothing. Desktop/cursor restoration is
attempted even on release errors. The original button must be up before a case.
There is no user clipboard, browser, printer, credential or capture operation.

## Build, identity and resources

In the owned hosted Windows VM's x64 MSVC developer PowerShell:

```powershell
pwsh -NoProfile -File .\run_probe.ps1 -OutputDirectory .\results -OwnedHostedCiInputDesktop
```

The script records source, custom-header and executable SHA-256, including separate
raw CRLF/native and normalized-LF source/header identities. Per-case stdout/stderr
and report.json are preserved. It builds only this C++ fixture, never Cargo or the
screenshots product. Source examples/default non-input cases remain diagnostic;
no local/native host execution is authorized by this handoff.

There are controller + receiver + source processes during a case. The existing
Job still has two child slots, +96 MiB receiver private-commit growth verified
before OLE, 512 MiB outer per-child commit, a five-second acquisition/cleanup
deadline and exact process-handle reaps. The private mapping adds only fixed
numeric fields and three guard handles; the inherited whitelist is now one
mapping plus six events. There is no unbounded trace/queue. The CI mover remains
one 256 KiB-stack thread with operation-only timer/stop/join handles. The extra
factory/unmarshaler objects are small, operation-scoped native objects. Idle
receiver waiting adds no polling timer or runtime. This is fixture resource cost,
not a measurement of a production parked helper or aggregate app memory.

The outside-guard release counter covers this controlled unmarshaler's relevant
methods. It does not establish arbitrary installed DLL behavior, OS/kernel/shared
mapping memory, Explorer/browser interoperability, mixed DPI/accessibility or a
security sandbox. Native Shell drag-image-helper ownership, queued-result leases,
stale-window/app cancellation and actual raw/URL editor integration remain further
required work. The held Rust payload adapter is still unregistered/uncompiled.

Native basis: [REGCLS combinations](https://learn.microsoft.com/en-us/windows/win32/api/combaseapi/ne-combaseapi-regcls),
[CoUnmarshalInterface](https://learn.microsoft.com/en-us/windows/win32/api/combaseapi/nf-combaseapi-counmarshalinterface),
[IMarshal](https://learn.microsoft.com/en-us/windows/win32/api/objidl/nn-objidl-imarshal).
