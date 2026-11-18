# Lazy path render targets

Based on published gpui-pre-wgpu 0.3.12, Zed revision
`d2a4fb6ded7c470be25d1541281da3882495b65b`. The original Apache-2.0 license
and package files are retained. Published archive SHA256:
`dc76608d385f6d0dcbad44504b1bf669dac3b7d7ffa2885acdb7c1c358caf852`.
`UPSTREAM-SHA256.txt` records the original package inventory, including Cargo's
local extraction marker. No registry cache was edited.

Only the renderer changes: allocate intermediate path targets when the current
scene has paths. Existing cached targets remain reusable; path rendering,
backend/sample selection, resizing and invalidation retain their contracts.
There is no new runtime, thread, cache, queue, timer or idle work. The same-version
path override changes no production dependencies.

Two independent source reviews were clean. Actual optimized testing reproduced
the old quad-only allocation at the new absent-target assertion, then passed all
47 package tests with the guard, including real path/readback, overlap, resize,
reuse, invalidation, shader, atlas and text cases. The published test package
omits workspace font files and a direct naga test dependency. Verification used
an operation-local workspace/dev-manifest overlay, exact pinned upstream fonts
and original OFL texts; all production source/manifests/lock bytes were restored.
The private lock also required the existing upstream optional hdrhistogram entry;
it was not added to the production lock. Direct promoted-package tests without
that fixture preparation are not claimed to pass.

A separate identical 1024x768 five-frame readback fixture selected four path
samples. The baseline held both path targets; the fixed version held neither,
avoiding a logical 15 MiB texture request. Single fresh-process measurements were
72,144 versus 72,196 KiB private, 82,074 versus 82,143 KiB PSS and 151,504 versus
151,532 KiB RSS, with no swap. This pair shows no meaningful process-memory
saving. It is neither a visible-shell measurement nor reference-hardware/GPU
residency acceptance; the 100 MiB private/32 MiB owned-GPU targets remain open.

Optimized all-target workspace Clippy, Windows GNU metadata, formatting and diff
checks pass. Exact preparation scripts, initial fixture failures, source hashes,
logs and restoration proofs are under
`target/verification/pathless-render-targets-integration/`. Actual shell path
count/sample choice, surface/device-loss behavior and native platform/resource
acceptance remain separate checks.

## Unused surface pipeline removal — 2026-11-18

Remove the unreferenced surface render pipeline and its sole-use layout/parameter types (85 lines). PrimitiveBatch::Surfaces was already ignored; the eight supported pipeline constructors, shader interfaces, backend preference/fallback, alpha/subpixel and recovery paths remain unchanged. Two independent source reviews are clean, and47 optimized renderer tests pass using exact verified upstream font fixtures and a temporary test-only naga declaration; all current production manifests/lockfiles were restored byte-for-byte.

A strict same-other-source baseline/fixed diagnostic pair completed eight real Close/Show cycles each. Tracked allocation peak fell93,021,212→91,476,989bytes, graphics-pipeline domain peak18,188,870→16,650,203, and allocation calls fell53,222. Windowless tracked live bytes were essentially unchanged (5,546,768→5,567,760). One render pipeline and pipeline layout are avoided per pipeline rebuild; a bind-group layout is additionally avoided per new core/device recovery, not transparency-only rebuild. This is transient-work evidence, not a resident-memory saving, timing improvement, hardware result or50MiBpass. The uninstrumented joint memory target remains failed. Exact sources,47-test log and A/B traces/hashes are retained under target/verification/unix-resident-memory-analysis.
