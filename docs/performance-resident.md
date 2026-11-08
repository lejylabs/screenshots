# Windows resident controller

2026-11-08. The Windows application now starts with its real tray, global hotkeys and capture controller, without opening GPUI windows or attaching DirectX. Primary capture/copy/save works in this state. Control windows and selection overlays attach graphics on demand. Closing the control window or dismissing the last selection without a control window hides and settles native windows before removing them and requesting headless windowing. Tray Quit still waits for output jobs.

Ordinary launch has no periodic housekeeping or diagnostic exit timer. `--show` opens the control window immediately. Measurement-only invocations explicitly supply `--exit-after`.

## Verification

The explicit `resident_capture_reopen_and_graphics_detach` test exercises real desktop capture/copy/save without graphics, reopening the control window twice, native WM_CLOSE, and region cancellation. It verifies that windows disappear and graphics detach, and that opening/closing UI and canceling selection preserve clipboard contents. The final regression run completed without logged window-handle errors. Ordinary UI tests, formatting and release compilation pass.

## Measurement scope

Release measurements use `scripts/measure-idle.ps1` on the development PC (Ryzen 9800X3D / Radeon RX 9070 XT / Windows 10.0.26300.0), five seconds of warmup and one-second samples. CPU uses process user+kernel time as a percentage of one logical core. Private Bytes and working set are separate metrics. Raw samples remain under `target/performance/`; the compact tracked report records exact counts and executable hashes.

These results do not establish reference-hardware acceptance. Zero additional measured CPU ticks does not mean literally zero CPU activity. GPU/driver memory, idle wakeups/energy, 500-cycle retention, post-capture retention and first-usable UI/overlay p95 latency remain unmeasured. Renderer attachment introduces work on the first UI action and must be timed before claiming instant responsiveness. Other platforms retain the previous windowed prototype.

## Results

Exact counts, source state and executable hashes are in [performance-resident.json](performance-resident.json).

| Actual application scenario | Sample duration | Peak Private Bytes | Final Private Bytes | Peak working set | Additional CPU time |
| --- | --- | --- | --- | --- | --- |
| Tray/hotkeys, UI never opened | 300.57 s | 6.62 MiB | 6.55 MiB | 17.43 MiB | 0 measured ticks |
| Control opened then closed before sampling | 30.07 s | 42.69 MiB | 8.14 MiB | 30.16 MiB | 0 measured ticks |

The post-UI run's first sample still held 42.69 MiB; the next sample fell to 8.24 MiB and the final samples to 8.14 MiB. Resource release is therefore delayed, not instantaneous. Both runs exited normally and produced no stderr errors. The five-minute run used the preceding build; the post-UI run includes the final guard blocking captures while graphics attach/detach is in progress. Separate executable hashes are preserved rather than claiming a same-binary comparison. The final build also passed the resident lifecycle GUI regression without logged errors.
