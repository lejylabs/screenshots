# Exploratory Windows release idle baseline

Recorded 2026-11-08, before further memory optimization. This is measured evidence, not a release gate pass.

## Setup

- `cargo build --release -p screenshots`, default optimized release profile.
- Ryzen 7 9800X3D, 16 logical processors; AMD Radeon RX 9070 XT selected by GPUI.
- Windows build 10.0.26300.0; single attached 2560×1440 display at scale 1.0 from this session.
- New app process; control window created then hidden natively; tray/hotkeys active. No capture or editor document, five-second warmup, one-second samples.
- 30.0496669 s sample interval; process exited normally afterward. No errors in stderr.
- Binary SHA256: `3D96A04A2C34437718B1DD8DE095A83DF92644A9FA0D3AB5A7A5EE119E180E06`.
- Source HEAD `453240167ef13bb3d1d0b1f29eb471280055c56f` plus local modifications. The binary hash identifies the tested artifact; HEAD alone does not reconstruct it.

## Results

| Metric | Observation | Plan target |
| --- | --- | --- |
| Process CPU time | 0.0625 s over 30.0496669 s | Reported independently of core count |
| Mean CPU, one-core normalization | 0.207989% | <=0.1% over five minutes |
| Maximum Private Bytes | 96,165,888 bytes = 91.71 MiB | <=50 MiB with no image consumers |
| Maximum working set | 103,796,736 bytes = 98.99 MiB | Report separately; not equivalent to Private Bytes |

Private Bytes were nearly flat through the run (about 91.66–91.71 MiB). That does not establish absence of leaks after capture/editing. The observed private-memory level is above the planned baseline; removing receiver polling has not established the desired resource budget.

## Limits and next diagnosis

This high-end development PC is not the modest reference machine. One short run is not the five-minute acceptance protocol. CPU timer quantization is significant when only 62.5ms of CPU time accumulates; repeat a longer run before attributing small CPU differences. Power mode, GPU memory, wakeups, presentation rate and energy were not captured. Initial window creation can retain renderer resources; the result is not a tray-only/lazy-window experiment.

Next, attribute private allocations and thread stacks, compare startup with/without eagerly created control-window rendering resources, and measure post-capture return-to-idle behavior. GPUI/driver overhead is a hypothesis to profile, not a proven cause. Do not relax the budget or force-trim the working set to disguise committed memory.

Reproduce with `scripts/measure-idle.ps1 -Seconds 30` or `-Seconds 300`, after building release and closing other instances. Raw samples/logs are local ignored artifacts under `target/performance/`; subsequent runs overwrite those local files. The committed numbers above retain this run's summary.
