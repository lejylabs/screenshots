# Owned Windows production drop acceptance

This fixture drives the application's actual installed native receiver process,
Rust COM target and GPUI editor with synthetic file, image and loopback URL data.
The provider, custom marshaler, outer Job controller and PowerShell runner come
from the reviewed revision-6 packet. Root subsequently corrected registration/
ALLOW readiness with per-registration epochs and an exact current receiver stamp.
A real reclaim registration wake must be rejected before ALLOW in a finite
handshake. `provenance.json` records their
identities. There are 105 authored cases with explicit owned input-desktop opt-in,
93 without. An authored case count is not an executed test result.

The runner refuses local and self-hosted machines. Direct native cases use a
generated private station/desktop. GPUI cases use a generated private non-input
desktop on WinSta0, retaining GPUI's interactive-station guard; they never switch
the input desktop. Genuine OLE cases additionally require an explicitly owned
disposable hosted input desktop and independent restoration. No user
screen, file, account or general clipboard is acquired. The test executable is
instrumented and is never a shipping artifact or a performance baseline.

Use the repository-pinned Rust toolchain and native prerequisites, then build:

```powershell
$env:RUSTFLAGS = '--cfg screenshots_native_drop_acceptance --check-cfg=cfg(screenshots_native_drop_acceptance)'
cargo build --locked --profile release-fast -p screenshots -j 2
if ($LASTEXITCODE -ne 0) {exit $LASTEXITCODE}
& verification/windows-production-native-fixture/run.ps1 `
  -Application (Resolve-Path target/release-fast/screenshots.exe).Path `
  -OutputDirectory (Join-Path $PWD 'target/production-drop-results') `
  -OwnedHostedCiInputDesktop
```

The manually dispatched `windows-production-drop.yml` workflow prepares the
pinned native engines and separate instrumented cache. Its 180-minute job limit
covers compilation and the complete suite; product operation deadlines and PLAN
budgets are unchanged. It retains raw case reports and binary/source identities,
including failures. A timeout or unrelated process failure cannot pass a negative
control. Capacity refusal checks every one of 256 real source attempts.

The earlier root instrumentation passed GNU workspace/all-target type checking,
and the native C++ sources compiled/linked with MSVC. The actual e541 hosted run
44174375175 failed: normal providers returned NONE, GPUI windows were headless,
and stall waits aborted before their final evidence. The runtime correction
separates ordinary operation timeouts from wait failures, retains the original
source-phase helper identity across receiver replacement, and records numeric
native operation stages/HRESULTs to resolve the remaining provider failure.
PowerShell records independent restoration/release evidence without replacing
the primary failure. This correction still requires native compilation and
execution. Full COM/OLE/editor/fault acceptance remains open.
Physical input, mixed DPI, hardware presentation and joint main/helper resource
acceptance remain separate from this owned synthetic suite.
