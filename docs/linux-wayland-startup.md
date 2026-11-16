# Descriptor-only Wayland startup

Status (2026-11-17): applied and registered in the desktop, GPUI and shared
capture paths. Local optimized and owned-compositor verification passes;
consecutive fresh source/document reviews 14/15 are clean on the recorded
frozen checkpoint.
Functional startup checks and source reviews remain separate from physical input,
presentation, capture and reference-hardware acceptance.

GPUI previously consumed `WAYLAND_SOCKET` while constructing its Linux platform,
then detected the default graphical environment again when starting the event
loop. That second detection could select X11 when `DISPLAY` was also present,
or start headless when the descriptor was the only display authority. A malformed
or closed inherited descriptor was also reduced to absence, allowing an X11
fallback.

The desktop entry now admits native-worker and CLI early returns before any GUI
or logger action. Its Linux/FreeBSD GUI bootstrap constructs one owned platform
before calling the logger initializer. The normal window, image-open and resource
probe paths consume that same platform. Other platforms retain their previous
construction points, including the Windows single-instance check before GPUI.
The logger guard remains alive through the GUI run and drops after platform
teardown. A constructor panic never starts the logger; a service-start panic
retires the owned platform without entering the GUI.

The implementation snapshots the original graphical environment before consuming the
inherited descriptor. Startup owns either that exact stream or its acquisition
error. A windowed startup with a present descriptor selects Wayland, or fails
closed if the descriptor/backend is unavailable. An authorized socket whose peer
has closed never selects X11. Starting explicitly headless, or using the existing
headless-only constructor without an initial request, retires the unused startup
stream as before. An inherited malformed/closed descriptor is unused in that
headless-only mode; windowed admission still refuses it. Later graphical transitions use their own requested
session rather than a cached process-wide backend guess.

The admitted connection retains a Wayland-authority boolean independently of its
actual client kind. Named Wayland display/session evidence and a present inherited
socket are restrictive authority; a Wayland client also supplies that authority.
An X11 client in a Wayland session remains an X11 client for focused Paste, while
its display/region capture jobs keep Wayland session restrictions. X11/XWayland
window listing/capture retains its DISPLAY-based capability. Focused Paste reads the
actual connected client kind. Automatic clipboard CLI requests retain their
existing raw-environment refusal before any GUI platform is constructed.

The GUI's capture backend owns the retained session authority and copies it into
each native operation. The desktop executor applies a restrictive marker to that
operation's `Command` before original-context admission and the existing private
worker environment/nonce bootstrap. A false/default context neither clears nor
overrides ambient Wayland markers. The private child receives the marker and no
Wayland descriptor. The context is not serialized into profiles, user JSON or
CLI flags. No process environment mutation, FD duplication, alternate capture
backend or automatic portal fallback is added.

Resource cost is constant metadata: one authority boolean on graphical-session
and capture/operation state, the bounded-lifetime startup environment snapshot,
and the existing short per-command session marker. The stream is moved once;
its close-on-exec flag is set before connection admission and failed ownership
setup retires it. Startup environment acquisition is the first operation inside
`LinuxPlatform::new`, preceding the font/text system and dispatcher construction
that may start native threads. The existing platform and logger are reordered;
no additional platform or service is constructed. No new dependency, runtime, thread, timer, polling, pixel buffer,
queue or cache is introduced. No performance budget or measured result changes.

The new regressions are private:

- The Linux startup API harness runs each case in a fresh empty-environment child
  and uses only child-owned Unix socket pairs. It checks descriptor-only startup,
  `DISPLAY`/named Wayland combinations, invalid/empty/closed/disconnected
  descriptors, consumption before later detection, exact FD ownership, close-on-exec
  and a disallowed Wayland mode. Valid, malformed and closed descriptor cases
  also check the shared default request used by the public headless constructor,
  exact unused FD retirement and unchanged windowed rejection. A further case takes both socket/token before
  starting one private simulated logger thread, checks that thread cannot inherit
  their environment names, and selects Wayland from owned evidence while it waits. The disconnected case exercises the native
  Wayland client against the owned closed peer. It never constructs a graphical
  platform or opens a compositor, display or clipboard.
- Core subprocess tests use a synthetic executor to check retained GUI Wayland
  authority, an X11 client in a Wayland session, ordinary X11 capability admission,
  unchanged raw CLI refusal and the exact operation context argument.
- Desktop tests send synthetic frames through the existing private nonce helper
  to verify the child's exact marker, lack of a descriptor, early Automatic
  clipboard refusal and preservation of all default-context command overrides.
- The focused-Paste test checks actual X11/Wayland/headless client choices and the
  existing native-offer route independently of session authority.
- The exact dispatch boundary used by `main` is tested with pure callbacks:
  native-worker completion/error, install error and CLI completion/error skip
  both GUI and logger admission; only the desktop result reaches the GUI callback.
- The actual GUI bootstrap order driver is tested with non-clone owned tokens:
  exactly one prepare call, identical ownership reaching Run, logger lifetime,
  constructor failure, service-start failure and Run panic teardown. These pure
  tests do not construct a native platform or claim native GUI-route execution.

The optimized vendor startup parent and its 16 fresh private child scenarios
pass. The final optimized executable passes all 19 checks in one owned private Sway
run: actual windows
for normal launch, synthetic image/editor opening and the visible probe,
malformed/closed authority refusal, desktop headless FD retirement and nine
public headless-constructor combinations. Private Sway identifies Wayland
windows by the owned PID, `xdg_shell` and no X11 window ID; application IDs may
be null. The same execution source passes the complete 1,044-test optimized workspace
(71 explicit opt-ins excluded), formatting, all-target optimized Clippy, Windows
GNU app/test metadata compilation and default advisories. Test and executable
identities are recorded from actual Cargo artifacts; no mtime/glob selection
is used. Physical focus,
DPI and reference-hardware idle/memory/presentation/latency remain unmeasured.
The pure API harness and synthetic compositor checks do not establish those gates.

## Short startup and mapping observation

Six interleaved named-Wayland synthetic probe launches compare the earlier
optimized `32398b6` image with the integrated executable. Median PID-matched
private IPC surface readiness was 209.777/209.921 ms before/after. Median
RSS was 240,752/249,384 KiB, PSS 235,099/243,690 KiB, private clean mappings
133,704/142,040 KiB and private dirty mappings 100,048/100,100 KiB. The
difference is mostly read-only mappings; this does not isolate attribution to
one source change. Three samples per image do not establish tail latency.
The bounded 0.5-second CPU samples observed no additional jiffies and cannot
prove the five-minute idle target. Both synthetic shell measurements remain
above the target; no physical input/presentation, GPU/driver, reference-hardware
or capture budget is accepted from these observations. Raw reports live in
`target/verification/formats-wayland-integration/` and
`target/verification/wayland-startup-runtime/`.

The 19-case native check also passes on the later CLI-report-correction image
`a3c64d0d6e7fcb3ccb21ff1414881c9a8c0048ec6317a9bc25ce50c4d389b49b`.
Its public headless harness was reused after exact compiler argv, source, kit
rlib and output hashes matched; no second harness compilation is claimed.
The short resource comparison above retains its earlier artifact identity.
