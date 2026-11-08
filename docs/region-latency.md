# Windows region-selection latency

2026-11-08. Release-mode diagnostic on the development PC, one 2560x1440 display at scale 1.0. The real resident app services register Ctrl+Shift+R; the test injects that shortcut with Windows SendInput, receives it through the native event bridge, captures the desktop in memory, creates fullscreen frozen-image overlays and waits for their second GPUI frame callback followed by worker-side DwmFlush.

## Final native-hotkey run

One initial attachment plus 100 subsequent capture/cancel cycles, detaching graphics between every cycle. The initial sample is excluded from warm percentiles. Nearest-rank percentiles:

| Injected hotkey to compositor proxy | Time |
| --- | --- |
| First attachment, single sample | 99.51 ms |
| Warm p50 | 69.39 ms |
| Warm p95 | 73.25 ms |
| Warm p99 | 75.55 ms |
| Warm maximum | 76.98 ms |

[Raw events, hardware context, stage statistics and test-executable hash](region-latency.json). The hardware context is inherited from the preceding measurement on the same PC; this is not the modest reference-hardware gate. Earlier direct-command probes measured 86.84 and 86.08 ms p95. Those probes bypassed native key delivery and ran under different warmup conditions; their differences do not demonstrate a code optimization. The final report preserves the native-hotkey run.

## Where time goes

Durations between consecutive stage timestamps, excluding the initial sample:

| Stage | p50 | p95 |
| --- | --- | --- |
| SendInput setup/native event delivery to UI handler | 0.51 ms | 0.71 ms |
| Renderer attachment after hotkey handling | 25.28 ms | 27.54 ms |
| Worker dispatch | 0.06 ms | 0.08 ms |
| Native hiding / compositor synchronization | 1.62 ms | 4.21 ms |
| Capture batch, including source preparation/admission | 18.04 ms | 18.83 ms |
| RGBA to BGRA preparation | 1.91 ms | 2.04 ms |
| Worker completion to overlay setup | 8.69 ms | 9.19 ms |
| Overlay setup to second frame callback | 9.53 ms | 10.23 ms |
| Callback observation, worker dispatch and DwmFlush | 3.37 ms | 4.61 ms |

Stage percentiles are computed independently and must not be added to reconstruct end-to-end p95. The dominant measured stages are renderer attachment and native capture. No cache, background stream or permanent renderer was added for this result. The diagnostic provides no evidence requiring those resource costs.

## Correctness and scope

All 101 cycles verified the frozen snapshot and render image were present, the active overlays covered the snapshot dimensions at the current scale and aligned with the display origin, and cancellation routed through the rendered overlay's action listener. Every cancellation returned to detached graphics with no GPUI windows. Clipboard sequence remained unchanged and the unique capture directory was never created. ERROR logs fail the test. A preliminary injection attempt safely aborted when modifier/R input was already held; it is not a latency sample.

The test rejects unavailable region-hotkey registration before injecting input, checks modifier/R state, and sends key-release events after partial injection failure. Its timestamps, locks and diagnostic polling compile only in tests. Ordinary app startup has no added timers, tracing buffers or frame requests. The existing geometry callback is reused to observe frame progression.

GPUI's frame callback precedes that frame's draw/present work. The second callback therefore follows the preceding frame's render/present submission. DwmFlush is a compositor synchronization proxy, not proof of physical scanout or visual correctness. The bounds and action checks do not replace physical mouse dragging, Escape, accessibility or input-to-photon measurement. Diagnostic observation adds up to a polling interval and worker scheduling to the endpoint.

The 1440p proxy is below the plan's 150 ms p95 target, but the full visible-and-usable acceptance gate remains open. No 4K, mixed-DPI, negative-origin/multi-monitor, physical-display, cold-process or reference-hardware result is claimed. Copy/save timing is outside this cancel-only scenario.

## Reproduce

Close other screenshots instances and leave keyboard and clipboard untouched during the approximately ten-second run. This opens real fullscreen selection overlays and injects the registered shortcut 101 times; screenshots remain in RAM and are discarded on cancel.

```powershell
cargo test --release -p app-ui resident_region_latency -- --ignored --nocapture --test-threads=1
```

Raw output is `target/performance/region-latency.json`. Build and run the test through the workspace so the pinned Windows GPUI teardown correction is applied.
