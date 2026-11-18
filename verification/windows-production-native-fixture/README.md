# Owned Windows production drop acceptance

This fixture drives the application's actual installed native receiver process,
Rust COM target and GPUI editor with synthetic file, image and loopback URL data.
The provider, custom marshaler, outer Job controller and PowerShell runner come
from the reviewed revision-6 packet. Root subsequently corrected registration/
ALLOW readiness with per-registration epochs and an exact current receiver stamp.
A real reclaim registration wake must be rejected before ALLOW in a finite
handshake. `provenance.json` records their
identities. There are 108 authored cases with explicit owned input-desktop opt-in,
95 without, plus the separate private-format ownership/bounds self-check. An authored case count is not an executed test result.

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
the primary failure. The subsequent fc5 hosted run44262524861 executed all105 then-authored cases:
50 failed their complete acceptance checks. Numeric diagnostics localized normal,
GPUI and OLE refusal to the real visual helper's DV_E_FORMATETC result. The new
correction accepts that one unsupported optional visual-format result after the
same operation/reentrancy checks, while preserving all real helper notifications,
Release calls and unknown-error rejection. Basic sources still refuse private
SetData/GetData. Two added cooperative source cases use the real Shell drag-image
manager and bounded private-format storage; an unknown-HRESULT control must be
rejected. The next actual5e65 hosted run44282974497 executed108 cases;52 failed
complete predicates and the private-format self-check failed. Basic sources now
returned DV_E_TYMED because fixture validation checked requested medium before
checking whether the private format existed. The follow-up restores the original
DV_E_FORMATETC for an unavailable format and preserves medium rejection for an
actually stored format. Borrowed private media are copied before giving the
native Shell store ownership. Bounded numeric private-call traces identify the
remaining cooperative manager medium refusal without widening production HRESULT
acceptance. These follow-up corrections are authored and still need native
execution. Full COM/OLE/editor/fault acceptance remains open.
Physical input, mixed DPI, hardware presentation and joint main/helper resource
acceptance remain separate from this owned synthetic suite.

The Shell helper displays optional drag images; it uses IDataObject private
formats through SetData/GetData. A basic IDataObject may legitimately return
E_NOTIMPL from SetData. Payload support is checked independently of those visual
formats. The cooperative fixture delegates private storage to SHCreateDataObject,
limited to eight registered HGLOBAL/ISTREAM formats,64KiB backing each and256KiB total; it validates
FORMATETC and tests borrowed/transferred/replaced/refused medium ownership.
fRelease=FALSE HGLOBAL media use an explicit OleDuplicateData copy and native
fRelease=TRUE; streams use an independent bounded snapshot. The original medium
remains caller-owned. A failed native store frees only its new copy or snapshot.
At most32 numeric private-call rows are retained per case, without
payloads, paths, private format names or error text. The cooperative Shell manager's
second rejected medium was later observed as TYMED_ISTREAM; the new bounded
snapshot path still requires native execution. The fixture never admits helper formats as capture/editor payloads. The source's synthetic
two-pixel drag bitmap is transferred to the real drag-image manager.

Primary contracts: [IDropTargetHelper](https://learn.microsoft.com/en-us/windows/win32/api/shobjidl_core/nn-shobjidl_core-idroptargethelper),
[IDataObject::SetData](https://learn.microsoft.com/en-us/windows/win32/api/objidl/nf-objidl-idataobject-setdata),
[SHCreateDataObject](https://learn.microsoft.com/en-us/windows/win32/api/shlobj_core/nf-shlobj_core-shcreatedataobject).

The next actual96be run44302687234 executed108 cases:104 passed all complete
predicates, with failures in the two cooperative Shell cases, same-HWND reclaim
and GPUI URL input. The HGLOBAL ownership self-check passed. Actual descriptors
identify the cooperative manager's second medium as TYMED_ISTREAM. The next held
fixture correction snapshots that private stream with64KiB capacity and exact
read/seek/cancel/ownership checks; it does not add a public stream payload format.

GPUI URL acceptance now installs the same isolated capture executor as normal
startup. Its loopback server waits on finite Winsock readiness before accept and
preserves the primary editor outcome through server/storage cleanup. That real
URL worker requires an outer fixture Job cap of4 processes for gpui-url only; the
other cases retain3. Product worker/concurrency/memory limits are unchanged.

Same-HWND reclaim still needs an actual boundary diagnosis: the first transfer
passes but the replacement never reaches the required registration/ALLOW proof.
New exact epoch/registration-stage/HRESULT and owned-process exit evidence retains
the existing rejecting handshake and7-second budget. A stale GetLastError value
is no longer reported as the cause of an admission timeout. These new corrections
and diagnostics remain authored, without a new native pass claim.
