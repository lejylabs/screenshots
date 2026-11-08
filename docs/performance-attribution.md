# Windows resource attribution and image-lifetime changes

2026-11-08. Follow-up to `performance-baseline.md`; exploratory results, not production acceptance.

## Changes implemented

- Normal-output thumbnails use at most 400x240 BGRA (375 KiB), not another full-resolution desktop buffer. Region overlays retain full resolution while selecting.
- Dismissed overlay and replaced thumbnail images are explicitly removed from the GPUI image atlas. Backing atlas pages/driver allocations may still be retained; no GPU-memory release claim is made without measuring them.
- Once output saves successfully, the retained original RGBA buffer is released. Only the saved path and small thumbnail remain. Explicit recopy decodes that saved file on the worker with decoder allocation and final RGBA-size limits of 256 MiB; the capture/selection/initial-copy pipeline does not round-trip through disk. Save failures retain unsaved pixels.
- Diagnostic `headless`/`empty` resource modes isolate GPUI initialization from application services; ordinary launch behavior is unchanged. The sampler records threads/handles and waits one visible second before hiding windowed modes, then warms for five seconds.

## Final repeated measurements

Same release executable for all three sequential runs, SHA256 `2D85942AF9A3888BAA1F98CFCE3D2C1EB371517D26C64ABF8EAF435168953127`. Same development PC as the baseline, Ryzen 9800X3D / Radeon RX 9070 XT / Windows 10.0.26300.0. Each sample is approximately 30 seconds; no capture is taken during sampling. Exact values and UTC timestamps are in `performance-attribution.json`.

| Mode | Private Bytes peak | Working set peak | Mean CPU, one core | Threads peak | Handles peak |
| --- | --- | --- | --- | --- | --- |
| GPUI, no GPU/windows, no app services | 6.59 MiB | 16.24 MiB | 0 measured CPU ticks | 6 | 206 |
| GPUI empty window, hidden, no app services | 51.18 MiB | 41.68 MiB | 0.260% | 27 | 869 |
| Normal app, hidden control/tray resident | 57.16 MiB | 45.27 MiB | 0.364% | 27 | 879 |

The windowed/GPU path accounts for a large observed difference relative to the no-window diagnostic. App services/UI add a smaller difference in this setup. This does not separate driver allocations from text/rendering caches or thread stacks; exact heap attribution still requires allocation traces. Pinned GPUI Windows source attaches DirectX and starts a VSyncProvider when connecting windowing, and only detaches when no windows are open. That motivates a resident-state/lazy-window design, not an immediate toolkit replacement.

The normal app remains above the 50 MiB private-memory and 0.1%-of-one-core CPU targets in this short run. Do not claim a measured optimization from 91.71 MiB in the earlier baseline to 57.16 MiB here: first-frame/hide timing was made more consistent, driver/cache warmup differs, and the initial sample is not an equivalent controlled trial. Thumbnail/source-release changes affect post-capture retention, not these no-capture idle samples. CPU figures are especially provisional at this sample duration and timer granularity. Zero measured CPU ticks is not a promise of zero CPU activity.

## Verification and next work

Release build and ordinary core/UI tests pass. A deterministic test verifies thumbnail size, BGRA conversion and untouched source pixels. Explicit GPUI tests verify that the original source Arc is released after save, recopy still works, repeated overlay cancellation preserves clipboard/files, and Quit waits for a complete PNG. Each GUI test was run separately; copy tests use small clipboard fixtures.

Follow-up: [Windows resident implementation and measurements](performance-resident.md) implements the independent controller and lazy graphics lifecycle. Warm latency, post-capture/500-cycle private+GPU memory measurements, explicit idle wakeup/energy traces, native topology handling, and reference-hardware acceptance remain open. No budget has been relaxed.
