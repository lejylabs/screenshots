# Unix resident window and graphics ownership

Integrated source, 2026-11-18. Optimized Linux metadata checks, all 17 focused
resident lifecycle regressions and the private X11 graphics-owner probe pass.
Windows GNU metadata checking also passes; this is not Windows runtime evidence.
The owned actual-main/helper 300-second idle check records zero CPU ticks, context-switch and I/O deltas. Its private-memory target still fails (~221 MiB versus50MiB); reference-hardware/GPU budgets remain open.

When the singleton-owned main app has a real tray host, Close removes its native
control and overlay windows. The existing ResidentController global owns the
same CaptureWorkspace after those windows disappear, and explicit Quit mode
keeps GPUI running. The existing singleton, Show listener, tray, shortcuts,
clipboard owner and recoverable capture/save/editor state retain their owners.
An editor or editor-close job prevents control retirement. No-tray and unscoped
Close keep the existing provisional Quit path.

Before retiring, the control explicitly removes its thumbnail atlas entry. A
failure keeps the render handle and accessible window. The existing retirement
routine clears window-local forms, history previews and thumbnail state and
removes native windows in a deferred foreground operation. Unsaved source pixels
and failed-save recovery batches stay in the controller; they are live consumers
and must count toward the active-image budget. No source is encoded, saved,
uploaded or discarded to lower an idle number.

A single boolean coalesces Show during retirement or active capture/preview.
Retirement completion applies it after native windows are removed. Explicit
Escape/CloseOverlay cancellation drains it after closing the frozen preview.
Generic close_overlays also runs while setting up a preview and beginning
crop/new-capture work; that cleanup must not drain Show or focus a control over
a new preview.

Capture result completion runs after the final preview/editor/output state is
installed. Cancellation, permission/backend failure, empty results and failed
fullscreen installation fulfill a queued Show even if restoring an existing
control already presents it. The flag is then cleared, so a later real Close does
not replay it. Picker, image-input, edit-render and crop terminal paths use the
same final-state completion rule because they share capture admission.
Show during provisional Quit can create the existing status control without
admitting another capture. The replacement control uses the same tray-aware
Close route. A stale close callback must still resolve its old window handle
before it can affect the controller. Native tray loss uses the same coalesced
reopen path. macOS Dock and Finder reopen find the retained controller even while
there are no windows.

## Native graphics owners

Linux keeps the current X11/Wayland event and clipboard connection. Destroying
the final native window already destroys its renderer; the client then takes the
existing lazy shared WgpuContext out of its Option and drops it outside the client
state borrow. With any native window still alive, that shared context remains.
The next native window initializes the same slot normally. Both native clients
copy their existing compositor GPU hint for each window instead of consuming it;
recreation therefore keeps the compositor's adapter preference on hybrid systems.
Keeping the display connection is necessary for clipboard paste and descriptor-only Wayland: its
inherited established connection cannot safely be reconstructed from an absent
WAYLAND_DISPLAY or replaced with another authority.

macOS keeps the existing Dock and native application ownership. Its platform
holds a Weak reference to the existing Metal InstanceBufferPool instead of a
permanent strong Arc. Native renderers and GPU completion blocks retain strong
owners. Concurrent windows share the current live pool. Only after the final
real consumer releases it can the pool and its buffers retire; a later window
creates the normal empty pool on demand. No Apple crate/API change is needed.
The published GPUI Mac window teardown remains responsible for releasing its
renderer/view ownership. Toggling WindowingRequest::Headless would only change
macOS activation policy and would not release this pool, so this implementation
preserves Dock reopen instead.

Neither clearing an Option nor expiring a Weak establishes actual memory usage.
wgpu/Metal, the allocator, dynamic libraries, compiler/driver caches and pending
native work may retain memory. Their measured costs remain part of acceptance.

## Resource cost and compatibility

The controller reuses an existing global owner and adds one coalesced Show flag.
Linux changes ownership of the existing lazy GPU context; macOS replaces one
strong Arc with one Weak of the same pointer-sized representation and moves the
existing empty-pool construction to window demand. There is no new thread,
process, timer, executor, runtime, queue, cache, image representation, connection,
dependency or disk round trip. The existing native connection metadata,
clipboard representations, fonts and service helper remain real idle costs.

No storage schema or serialized setting changes. Rollback restores the previous
window/GPU lifetime without migrating user data. Window geometry/design and
controls remain the existing implementation. Window-local unsaved form inputs
follow the existing resident retirement behavior; capture pixels and failed
output recovery remain owned independently.

## Integrated verification and remaining gates

The owned authenticated Xvfb probe passed eight reopen cycles on the same native
connection. With two windows it retained the shared graphics owner after closing
one and retired it after closing the last; adapter vendor/device/name/backend
identity remained stable on recreation. The optimized probe executable SHA256 was
`deb678ceb68c39347f5d1b4fe96f2e87658af86a89ad04507bdc87a5e46b747d`.
The runner isolated D-Bus, HOME/config/runtime and retained exact child ownership
through exit/reap before deleting its runtime directory. An initial test-harness
Quit issued before the native loop started was lost; a test-only deferred Quit
fixed that harness failure. It was not evidence of a production GPU deadlock.

The 17 lifecycle regressions include rendered Escape/Close-overlays cancellation
with an existing hidden control and a queued Show. The two fresh final source
reviews are clean. These checks establish ownership and functional behavior;
dropping the graphics slot does not establish a private-memory saving. The
previous whole-process resource failures remain unresolved until remeasurement.

Use the exact integrated source and optimized binaries. Root integration owns
Cargo and native execution. Do not run these native probes on the user's session.

1. `cargo test --release -p app-ui unix_native_services::tests -- --test-threads=1`
   covers repeated native-control removal/reopen with shared unsaved source
   pixels; Show coalescing through retirement; successful capture-to-preview and
   rendered Escape cancellation; native cancel/permission/backend failure followed
   by later Close staying windowless; empty results and failed fullscreen install;
   image-input cancellation/failure; provisional Quit with no previous control;
   editor protection; reopened no-tray Close with an active save; and the existing
   rollback, atlas and save-failure controls. Terminal-result fixtures invoke the
   actual production result handler with synthetic data and start no native capture.
2. `cargo test --release -p gpui-pre-linux resident_graphics_tests --no-run`
   builds the opt-in native GPU-slot probe. Run its exact test binary only inside
   an owned private Sway or authenticated Xvfb session with an isolated D-Bus,
   HOME/config/runtime and no user clipboard/input. Its exact test is
   `linux::platform::resident_graphics_tests::private_last_window_releases_graphics_and_reopens_on_same_connection`.
   It checks actual GPU-context presence for two native windows, retention after
   closing one, retirement after closing the last, and eight reopen cycles on the
   same display connection. It also verifies preserved compositor vendor/device
   hints and the actually selected adapter's vendor/device/name/backend identity
   across context recreation. A private software compositor may have no DRM hint;
   in that fixture only, it pins the already-selected adapter through a test-only
   setter so the consumed-hint regression is still exercised. The guard requires a 0700 root named
   `screenshots-private-graphics-*`, `SCREENSHOTS_PRIVATE_GRAPHICS_ROOT`,
   `SCREENSHOTS_PRIVATE_GRAPHICS_BACKEND=x11|wayland`, matching HOME/config/runtime
   subdirectories and a private D-Bus address. Use named Wayland and a separate
   inherited-descriptor-only process; use only private Xauthority for X11.
3. On a native macOS build after root activates its exact GPUI Mac vendor source:
   `cargo test --release -p gpui-pre-macos resident_graphics_tests -- --test-threads=1`.
   The pure ownership test exercises the same cache helper used by open_window:
   concurrent consumers share a pool, pending-completion ownership prevents early
   retirement, last-consumer release expires the platform Weak, and reopen
   creates a new pool. It does not submit actual GPU commands or establish native
   renderer destruction. Exercise real concurrent control/editor windows,
   close/reopen, Dock/Finder Show, pending Metal command completion and shutdown
   in the private native Mac lifecycle harness as well.
4. Run the existing optimized Unix singleton, native-service, private Wayland
   named/descriptor and authenticated X11 suites, then current workspace tests,
   all-target Clippy, formatting and Windows metadata. Native focus/activation
   policy remains compositor-dependent and must be recorded separately from
   protocol success.

For actual process measurements, compare the old and new optimized applications
under the same owned private session, synthetic workload and clipboard ownership.
Use the normal singleton main app with its real native helper and private tray
host; a headless diagnostic binary or standalone GPU-slot probe omits real costs.
Record executable/source hashes and process start times. Observe visible shell,
then close-to-tray, then 30-second idle, then a 300-second idle window, then Show.
Use the existing explicit family sampler, for example:

```
python3 target/verification/unix-native-services-held/tests/measure_linux_services.py --pid OWNED_PRIMARY_PID --seconds 300
```

That existing sampler reports joint parent/helper private resident mappings,
PSS/RSS, CPU, threads and FDs; it does not measure swap, all-thread context
switches, GPU memory or presentation. Capture `/proc/PID/smaps_rollup`, per-thread
`/proc/PID/task/TID/status` and process I/O at both endpoints for every stable
owned family member too. Report Private_Clean/Dirty/Hugetlb, RSS, PSS, Swap and
SwapPss separately, without trimming or interpreting swapped-out bytes as a
budget pass. Compare the joint idle private budget of 50 MiB and owned GPU budget
of 16 MiB against PLAN, and keep driver/shared-library costs visible. Repeat 100
warm reopen samples and 500 synthetic close/reopen/cancel cycles; use physical
reference-hardware timing and GPU tools for the actual presentation/resource
acceptance, including a native macOS footprint and Metal allocation observation.

Private software-compositor results expose real source regressions and memory
costs but do not establish physical display/input latency or the documented
reference-hardware budgets. A repeatable failed budget requires further source
remediation; it must not be closed by the logical release assertions here.

## Actual main/helper observation and allocation diagnosis — 2026-11-18

The optimized normal app and its same-executable native-services child passed eight real WM Close/secondary Show cycles, remained windowless through30seconds warmup plus300seconds idle, reopened and quit through the actual tray route. All owned processes reaped before private runtime removal. Observed300.0058seconds: jointCPU0ticks (0.0% ofonecore), context-switch deltas0, all I/O deltas0. Scheduler wakeup counters were unavailable and remain null, not inferredzero.

Jointprivate mappings ended232,255,488bytes in the first successful run and231,153,664bytes in the repeat with explicit mapping diagnostics. Both fail50MiB. The signed privately extracted Mesa26.2.5 Lavapipe workload is software-rendered development evidence, not the integrated-GPU reference machine. No working-set trimming, allocator trim or image disposal was used.

A separate explicitly instrumented heaptrack run is diagnostic only: tracked live allocations peak93,019,417bytes and fall to5,564,218bytes throughout the windowless plateau. The graphics-pipeline allocation domain falls tozero; file-backed compiler/driver mappings and resident allocator capacity still dominate the observed footprint. This does not prove a continuing live pipeline leak or a50MiBpass. Removing one pipeline that has no supported draw consumer is separately reviewed; its individual savings require same-source A/B measurement.

The actual startup run found a reentrant controller update in listener registration. Both native launch paths now finish extracting the controller entity before registering the listener; the successful full observation exercises this corrected ordering.
