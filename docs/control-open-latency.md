# Windows control-window latency probe

2026-11-08. Optimized test binary, real resident controller/tray/hotkeys, metadata loaded, control window opened from detached graphics. One first attachment and 100 reopen/detach cycles in one process. This is a development-PC diagnostic, not reference-hardware acceptance or a cold-process launch benchmark.

| Command dispatch to callback | First attachment (n=1) | Reopen p50 | p95 | p99 | Max |
| --- | --- | --- | --- | --- | --- |
| First GPUI frame callback | 86.45 ms | 40.69 ms | 50.42 ms | 51.54 ms | 53.45 ms |
| Following frame callback | 105.20 ms | 43.55 ms | 52.86 ms | 54.96 ms | 55.87 ms |

Raw samples and test-executable SHA256 are in [control-open-latency.json](control-open-latency.json). Percentiles use nearest rank over the 100 reopen samples, excluding first attachment. The timer starts before `show_control`, so graphics attachment and deferred window creation are included; enumeration completes before timing. Teardown occurs between samples. No captures, physical input injection or clipboard changes occur in this probe.

Despite its documentation saying “after rendering”, the pinned GPUI implementation invokes `on_next_frame` callbacks before that frame's draw/present section. The first callback therefore measures a frame opportunity, not a completed render. The second callback follows the preceding frame's draw/present submission. Neither proves physical scanout, visual correctness, accessibility readiness or input-to-photon latency. The <=150 ms usable-window target is not marked passed from these numbers. The probe deliberately requests one extra frame; no instrumentation or probe timers compile into the ordinary application.

## Teardown race found and fixed

The initial 101-cycle run produced invalid-HWND errors despite the functional test returning success. The controller now waits for GPUI's asynchronous native destruction before requesting headless mode, with a two-second operation-scoped worker deadline. No idle timer was added.

A separate race remained in GPUI Windows: its VSync thread could call RedrawWindow on a destroyed HWND until a posted close notification removed it from the tracked list. The locally pinned Windows backend synchronously removes tracking during WM_DESTROY using the existing validated close handler and read/write lock. See [local patch](../vendor/gpui-windows-gpui-unofficial/LOCAL-PATCH.md). The vendored source adds about 0.53 MiB to the repository, with no additional runtime service or dependency version.

The final 101-cycle measurement and five consecutive real capture/copy/save/reopen/cancel regressions passed with ERROR logs counted as failures. Ordinary core/UI tests and the release application build also pass. Measurements across preliminary builds are not a controlled performance comparison; this report preserves the final run, including its slower samples.

A subsequent 30.06-second release-app idle check after opening/closing the UI measured 42.62 MiB peak Private Bytes, falling to 8.09 MiB in the final sample, with no additional measured CPU ticks. See [idle recheck](control-lifecycle-idle.json) for exact counts and the application executable hash. This short recheck does not replace the five-minute/reference-hardware gate or GPU-memory measurements. Vendored source hashes were compared with the original registry package: only `src/events.rs` differs.

Run explicitly with no other screenshots instance:

```powershell
cargo test --release -p app-ui resident_control_render_latency -- --ignored --nocapture --test-threads=1
```

The probe opens/closes real windows 101 times and writes `target/performance/control-open-latency.json`. Capture/overlay p95, physical presentation, cold launch, mixed DPI and the 500-cycle memory gate remain open.

## Navigation foundation follow-up

2026-11-09. The control window now renders only the selected Captures/Uploads/Settings section, with a fixed navigation header and shared neutral/primary buttons. A separate native regression opens all sections, verifies cancellation/release of hidden history state and closes back to detached graphics without ERROR logs. The initial sandboxed attempt reported denied Windows cursor access; the desktop-enabled run passed.

The same optimized 101-cycle reopen probe observed a first attachment of 56.71 ms to the first callback (59.72 ms to the second), then 41.18 ms p95 to the first and 43.86 ms p95 to the second callback over 100 reopen samples. The earlier second-callback p95 above was 52.86 ms. These separate development runs do not isolate a speedup or establish physical input/presentation acceptance. Current raw samples and test executable hash are preserved in [navigation follow-up samples](control-navigation-latency.json); the original report remains unchanged. No new process-memory/GPU/idle or manual keyboard/screen-reader/DPI acceptance is claimed.

## Sidebar layout follow-up

2026-11-09. The subsequent main-window layout replaces header tabs with a fixed 224-logical-pixel sidebar and separate screenshot/history/source/upload/settings workspaces. Its native navigation regression passes cancellation/cleanup checks and saves only the empty app HWND for visual inspection, without user pixels or clipboard writes. The optimized 101-cycle reopen probe observed 59.31 ms to the first callback on initial attachment; over 100 reopen samples, first-callback p95 was 44.75 ms and second-callback p95 was 48.03 ms (maximum 71.68 ms). The preceding header-navigation run's second-callback p95 was 43.86 ms. These separate runs do not establish the cause of their difference or a performance improvement. [Sidebar raw samples and executable hash](control-sidebar-latency.json) are preserved separately. Physical presentation, manual input/DPI/accessibility and full CPU/memory/idle acceptance remain open.

## Workspace layout follow-up

2026-11-09. The main window now uses a 216-logical-pixel sidebar, primary New screenshot action, collapsible capture options, and Recent/Library tabs under Screenshots. File actions expand per library entry; at most one entry is expanded. The opt-in native regression checks navigation/cancellation, renderer detach and wide/compact synthetic layouts at 1000×720 and 680×480. The editor probe checks its separated output/tools rows at 1000×760 and 760×500. Both pass with desktop access; sandbox-only runs report Windows cursor access denied and are not counted as passing native checks. The ordinary optimized UI suite passes 34 tests, with 20 opt-in diagnostics skipped.

The optimized 101-cycle reopen probe observed 54.87 ms to the first callback on initial attachment. Over 100 reopen samples, first-callback p95 was 42.97 ms; second-callback p95 was 45.39 ms (p50 41.55 ms, p99 46.56 ms, maximum 47.04 ms). The preceding sidebar run reported 48.03 ms second-callback p95. Separate development runs and a changed initial window size do not isolate a speedup; these callback proxies do not establish physical presentation/input acceptance. [Workspace samples](control-workspace-latency.json) are preserved separately. Full process/GPU-memory, five-minute idle, mixed DPI and manual keyboard/accessibility acceptance were not rerun. Incremental retained UI state is one boolean and one optional row ID; no new worker, runtime, timer, dependency or image cache is introduced. History keeps its existing bounded page and explicit preview behavior.

## Visual refinement follow-up

2026-11-09. Neutral tabs/filters/navigation and quiet secondary controls now reserve the primary accent for capture/output. Library rows put filename/metadata next to Edit/Copy/selection; clicking the filename toggles the existing bounded preview. More contains file, upload, relink and metadata-removal actions. Destination setup and transfer are separate panels. The sidebar is 200 logical pixels, reduced to 184 below an 820-pixel viewport; compact workspaces use 16-pixel padding. Native release inspection covers empty/library/menu/Uploads/Settings and editor layouts, including the 680×480 main window and 760×500 editor. Navigation cancellation, stale menu events and graphics detach still pass. No image buffer, queue, runtime, worker, polling, dependency or retained controller field is added; new icon paths and layout nodes are transient on invalidation. The normal optimized UI suite passed 34 tests; 20 opt-in diagnostics remain skipped except the explicit native probes.

The final optimized 101-cycle reopen run observed 60.87 ms to the first callback on initial attachment. Over 100 reopen samples, first-callback p95 was 43.88 ms and second-callback p95 was 46.50 ms (p50 39.03 ms, p99 49.48 ms, max 54.51 ms). The previous workspace run reported 45.39 ms second-callback p95; this small difference between separate runs is inconclusive. [Refinement samples and executable hash](control-refinement-latency.json) are preserved separately. These are development callback proxies, not physical presentation/input acceptance. Full idle/private/GPU memory, mixed DPI and manual assistive-technology acceptance were not rerun.
