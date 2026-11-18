# Unix main-application desktop services

Integrated development source, 2026-11-17. The optimized full workspace now
passes **1,141 tests, zero failures, 73 ignored, 61 outer suites**, excluding
child-harness summaries. Optimized all-target Clippy, formatting, Windows GNU
all-target metadata and default advisory checks pass. The final six-mode private
Xvfb/D-Bus fixture also passes with reservation-before-publication. Actual fresh
applied-source reviews 11/12 are clean. Earlier protocol, X11 ordering and
TestWindow failures remain archived. Formatting and a redundant Arc-clone move
follow the full-suite checkpoint and are covered by final Clippy/metadata/native
checks. No native Mac/full-GUI/physical-input/resource acceptance is implied;
tests used owned services and never captured the user desktop or clipboard.

## Admission and native ownership

Normal primary startup acquires the existing user/session singleton before platform startup. GPUI consumes Linux startup environment first. Once the existing window and Show listener are installed, a deferred callback starts native services. File --open, resource probes, forwarded secondary launches and WindowedOnly fallback cannot construct them. The parent retains the singleton guard until native teardown and process reaping finish.

macOS uses the existing Cocoa/GPUI loop and locked Carbon/NSStatusItem implementations. Native registration, reservation, commit/rollback and destruction remain on the main thread. No second Carbon loop, media-key tap or Accessibility request is added for normal saved keys. A bounded atomic input gate pauses at transaction admission and retires queued hotkey and menu captures on every failure, commit and rollback. Show/Quit survive settings changes; canceled Quit attempts retire older Quit commands too.

Linux uses one supervised native helper process. The existing 256 KiB-requested-stack parent worker becomes its parked IPC supervisor; it does not retain a second native event thread or add an idle runtime. The helper's own main thread owns libdbus, X11 grabs, portal sessions and StatusNotifierItem/DBusMenu. Child entry runs before UI, logging, singleton acquisition, capture executors or optional runtimes. Installing the spawn callback stores one function pointer and starts nothing.

The helper reuses the existing inherited-channel machinery: private bounded socket queues, parent PID/UID validation, a fresh nonce, close-on-exec protection, parent-death handling and a waitable process-group leader. Desktop metadata uses an explicit SOCK_SEQPACKET mode whose constructor is compiled only on Linux; existing capture-worker callers, including the Mac constructor, retain SOCK_STREAM. Every desktop packet carries its magic and nonce and admits at most 64 KiB of typed JSON. There are no pixels, profile paths, arbitrary commands or upload credentials in this protocol.

A helper EOF, malformed packet, wrong nonce or deadline disables native availability and stops the incarnation. There is no automatic respawn loop. The parent sends Shutdown, permits a 250 ms graceful interval, then kills and reaps its owned family before releasing ownership. A 5-second watchdog covers initial D-Bus connection/authentication and another 5 seconds covers native setup. Configuration requests have an overall 8-second X11 or 185-second Wayland deadline, including portal decision time. Idle IPC waits use no deadline or timer. Process creation, uninterruptible kernel/D-state waits and failed process-family inspection are not claimed interruptible; incomplete cleanup retains its owner and keeps Quit waiting.

This isolation is necessary: the pinned libdbus source performs a blocking Unix connect before setting nonblocking mode. Its default Hello reply timeout does not bound that connect, and its public Rust API cannot attach cancellation first. The implementation does not time out and abandon a blocked reader thread.

## Native shortcut backends

Linux selects the backend from preserved NativeCaptureContext.wayland_session, including original inherited Wayland authority. The X11 display comes from GPUI's preserved environment. An original Wayland session never receives global XWayland grabs, even if its visible window falls back to X11.

X11 uses pinned x11rb checked GrabKey/UngrabKey requests through a single-owner nonblocking Stream. Socket connect, handshake, extension and mapping replies, grabs, flushes and cleanup waits observe cancellation/control and admitted deadlines: three seconds for initialization, two seconds per subsequent operation. Terminal cancellation, timeout or transport error closes the sole owned socket immediately. No persistent deadline remains idle. Incoming headers are checked before x11rb allocation: at most 256 KiB per packet, 1 MiB and 1,024 packets per operation. Xauthority reads admit one 64 KiB regular file, check cancellation between steps and never log its contents; kernel filesystem syscall stalls remain an explicit OS limit.

The backend discovers Alt/Super and lock mappings, requires detectable autorepeat and handles all admitted X screens. Absent/ambiguous base-keymap chords, collapsed logical modifiers and duplicate physical (keycode, resolved mask) reservations are rejected before any grab. Alias rejection therefore cannot ungrab an existing working chord during rollback. Keyboard-map changes make the old registration unavailable and require restart. The separate X11 task host reuses this Stream and registrar, with its native stop source installed before connection initialization.

Wayland uses GlobalShortcuts. A bounded worker-side StartServiceByName activates an installed idle portal before resolving and pinning its unique owner. Method destinations, responses and session signals use that owner and the exact request/session path. Bus owner-change rules strictly require the actual bus-daemon sender. A staged session is not an action route until the settings save commits it; rollback closes it. Partial grants, unknown/duplicate IDs, invalid descriptions and unexpected handles are rejected.

The protocol accepts preferred triggers and returns human trigger descriptions. It exposes neither a canonical assigned accelerator nor atomic physical-chord reservation/rollback. The application therefore saves preferences, reports the compositor's actual descriptions, and stages application session routes without asserting unsupported physical-key equality. It never substitutes application defaults. See the [official GlobalShortcuts interface](https://flatpak.github.io/xdg-desktop-portal/docs/doc-org.freedesktop.portal.GlobalShortcuts.html).

During reservation, persistence and remapping, capture inputs are dropped rather than retained as commands. Portal callbacks retain only coalesced held/released state while paused. Input resumes after a quiet drain; generations reject already delivered captures from an older configuration. State, owner/watcher loss, Show and Quit continue through IPC during consent. A delayed old-session press cannot become a capture after commit.

The separate Wayland task host uses a task-only mode of the same supervised helper. It sends at most 16 validated IDs/preferences and receives activations, with no task/profile file access or execution in the helper. Connection/auth is bounded, portal consent has the overall deadline, and CLI cancellation kills/reaps the helper. Existing task-host job admission and output behavior remain in the parent.

## Tray, controls and storage lifecycle

Linux exports the existing eight commands through StatusNotifierItem/DBusMenu and a 32x32 in-memory camera icon. It starts no GTK/AppIndicator loop. Registration requires a real host; watcher methods and host signals are pinned to the resolved unique watcher. Owner-watch rules filter the named watcher/portal, and a directed forged signal cannot impersonate the bus daemon. Native host loss revokes availability even during consent. Status snapshots coalesce independently of the bounded action queue, so losing an action cannot retain stale availability.

The existing six shortcut inputs and Save/Restore controls are reused on Unix. No new product controls, themes, editor design or menu actions are added. Settings load and persistence stay on existing background workers. Failed settings load cannot register fallback defaults. Save and Import reserve, persist, then commit or roll back. Cleanup failures report their actual active state and prevent further changes.

Quit is provisional while recoverable output saves, editor closure or other acknowledged work are pending. Native services and singleton Show admission stay owned; capture input is paused and Show exposes the existing busy window. Save failure cancels Quit, retires queued input and resumes the same services/listener. Only the final irreversible exit stops Show admission. Native reservation/consent can still be canceled before persistence when no recoverable work remains; acknowledged storage and native finish results are awaited. Native availability loss cancels an in-flight native capture before restoring the control window, preserving output-free cancellation. No-tray Close follows the existing Quit lifecycle; an actual available tray permits destroying the cleared native control and retaining its controller. Before forgetting its last render, that window explicitly removes the thumbnail from its sprite atlas; a release error retains the handle and keeps the window accessible. Linux releases its existing lazy shared GPU context when the last native window releases its renderer, while retaining the native event and clipboard connection. macOS retains only a weak reference to the shared Metal instance-buffer pool, so actual renderers and in-flight GPU completions determine its lifetime. Reopen creates the existing controls on demand; a single flag coalesces Show through retirement. No existing editor or recoverable source pixel is discarded. See [the source and verification contract](unix-resident-graphics.md).

Primary, region, edit, repeat and countdown paths reuse existing capture admission and output behavior. Unix active-window/display actions use existing native focused-capture calls; unsupported Wayland targets report unavailable and use no replacement source. The original capture context remains attached to the parent backend. The helper never reads screenshot pixels.

## Resource contract

The PLAN budgets are unchanged. In particular, the 50 MiB idle private-memory budget is joint across the primary and its native helper; it is not raised to 100 MiB and neither process may be omitted. Private, PSS and RSS, shared-library pages, kernel socket queues and driver/GPU allocations must be reported separately.

Source-level costs and limits, not measurements:

- One parent supervisor thread with a requested 256 KiB stack and one native helper process with its OS main stack. The helper adds no event, upload, capture or GTK runtime thread. Input/quit retirement adds bounded atomic counters/flags, not another queue, worker, descriptor or timer. macOS adds no worker/process.
- Parent controller wake socket pair and one IPC endpoint; helper IPC endpoint, internal native wake pair, duplicated X11 control descriptor, one private D-Bus channel and one X11 socket in an X11 session. OS/runtime process handles and library buffers require measurement.
- One active and one queued configuration request, at most 32 capture/task events and one coalesced latest status snapshot. Metadata packets are bounded to 64 KiB; retained statuses/errors to 4 KiB each.
- At most 12 main-app native X11 registrations including active/staged/failed-cleanup keys. The shared task-host registrar admits 16. At most 32 screens and eight lock combinations: 3,072 main-app or 4,096 task-host passive grabs in the admitted worst case.
- One active and one staged main-app portal session. Six main-app descriptions are bounded to 256 bytes each and paths to 512 bytes. The task-only mode admits 16 existing task IDs.
- A 4 KiB icon representation per serialization plus native/serialization copies; no screenshot source, texture, image undo, PNG round trip or background capture stream.

There are no measured process/private/PSS, idle CPU/wakeup, startup, physical input or GPU results for this candidate. The held diagnostic script reports the whole owned process family without reading pixels; standalone fixture figures would not establish production GUI acceptance. The resident graphics source change removes minimized-shell retention; its actual joint memory and reopen latency still require optimized measurements. Visibility-delay debt remains separate.

## Verification handoff

Focused executed checks cover registration transactions, native alias resolution, sender identity, bounded status and packets, epochs, paused key state, menu semantics, modifier filtering and consumed-control dispatch. The deterministic reservation/completion scheduling tests pass in that full run. Private socket-pair faults exercise stalled X11 handshake/replies/checked cleanup, blocked writes, cancellation, oversized headers and floods. The explicit private fixture creates a blank Xvfb server, isolated Xauthority and D-Bus service directory; it covers real activation of an installed idle portal, modifier aliases, directed forgeries, native grab rollback, delayed consent with old presses, shutdown during consent/stalled authentication, tray/menu state and process/session cleanup. All six private modes completed successfully in `private-native-g.log`: activation, stalled_bus, close_stalled_bus, x11, wayland and close_consent. The fixture uses a non-reserved Ctrl+Alt+G input, keeps the F8 grab-conflict test, verifies the real current-epoch action through services.action and bounds its receive wait. Earlier compile/commit/timeout failures remain archived separately.

Integration used genuine current/base/candidate merges, preserving Jiff and newer Windows printing features/exports. Complete the current optimized regressions, Clippy, Windows metadata and native macOS plan before claiming their gates. Fresh reviews must include the full connected controller/settings/capture/close paths; R1 did not certify unchanged large UI bodies. Repeated synthetic capture/minimize/reopen atlas checks, recoverable save-failure/busy-Show checks and deterministic queued hotkey/menu/quit gate checks are also prepared. No parser or compile result establishes native behavior or performance acceptance.

## Actual integration corrections and evidence

Original held source patch `1c764ef84189080fb5619b3813639e515d156abc09b32ee2c0b1c7f1f739be74`
received two actual clean source reviews (8/9). Compilation corrected three
D-Bus Path String-error conversions. An original optimized protocol regression
exposed Serde internally-tagged unit variants ignoring unknown fields; empty
struct variants retain exact wire bytes and now reject extras, with every
packet variant and nested status/event checked.

Actual X11 execution exposed consumed-control lost wakes and completion before
quiet native readiness. Pump reports consumed control packets before idle wait;
completion replies wait for the existing quiet drain, preserving held-key
suppression without waiting for physical release. Input is enabled before a
successful activation reply; completed rollback while quitting can reply with
input still paused. This adds one bounded deferred metadata report and no timer
or idle loop. The six-mode fixture then passed.

An additional fresh applied-source review found publication ordering: a fast
supervisor could restore the gate before the producer's later false store.
The current correction reserves a slot in the existing Tokio sync channel
before changing counters/gates, closes admission before publishing, and has no
counter/gate mutation on full/closed admission. Logical capacity remains one
queued configuration plus one active request; the sync primitive needs no Tokio
runtime/thread. A small bounded channel block replaces the previous std sync
queue; it is metadata storage and does not relax the joint memory budget.
A deterministic worker-completes-before-publisher-returns case and rejected
admission gate/epoch cases pin the ordering.

Exact initial native result, binary/source/runner hashes and verified Arch Xvfb
provenance live in `target/verification/unix-native-services-integration/`.
The private runner activates only its own mock service directory and redirects
HOME/XDG/authority; package extraction uses official SHA/signature verification
without global installation. The latest whole-workspace run and two actual fresh applied-source reviews
are successful. Exact source/artifact/check hashes are retained with the result. The resident graphics source change removes minimized-shell retention; actual
GUI/physical input, joint private/GPU memory and PLAN resource budgets remain
explicitly open until measured against this new source.

## Final local functional checkpoint

Full optimized log SHA256:
`fb9a7765209ebf8d76aaf4f623463c21e8cf3adccc728be2207fff4834726920`.
The private fixture covers activation, stalled authentication, shutdown during
stalled authentication, real X11 grabs/rollback/delivery, Wayland preferred
triggers/consent/forgery/old-input suppression and cancellation during consent.
Its own three-second event deadline reports closed/missing current-epoch input
instead of spinning on channel EOF. Official Arch Xvfb package SHA256 and
signature are verified before private extraction; all its runtime dependencies
resolve to existing system libraries. No system package installation is used.
