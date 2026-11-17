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

The runner refuses local and self-hosted machines. Ordinary cases use a generated
private station/desktop; genuine OLE cases additionally require an explicitly
owned disposable hosted input desktop and independent restoration. No user
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

The current root instrumentation passes actual GNU workspace/all-target type
checking. The earlier revision-6 native C++ sources compile/link with MSVC and its runner
parses on Windows. The later readiness correction still needs native compilation
and runtime. Full COM/OLE/editor/fault execution remains pending.
Physical input, mixed DPI, hardware presentation and joint main/helper resource
acceptance remain separate from this owned synthetic suite.
