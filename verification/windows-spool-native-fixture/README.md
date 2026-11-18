# Owned Windows spool/driver fixture

This is a private, explicitly instrumented downstream acceptance fixture. It
does not extend the product's supported port policy. Run only on a disposable,
already elevated GitHub-hosted Windows VM, with a developer command prompt and
the repository's locked Rust/native dependencies already available. It never
installs a driver, enables a feature, prompts for elevation, reboots, changes
the default printer, restarts the spool service, selects an existing queue or
contacts a network print endpoint.

Prerequisites are a running spool service, a valid Microsoft-signed installed
`Microsoft XPS Document Writer v4` driver, PrintManagement/CIM, .NET Framework
WPF/ReachFramework and an existing host default printer whose name is read only
for unchanged-state verification. The default queue is never opened or used.
Missing prerequisites fail. Feature provisioning, if needed, is a separate
explicit disposable-host operation by the CI owner; the fixture has no install,
download, untrusted-driver or prompt-to-file fallback.

The source guard `screenshots_native_print_acceptance` is declared in the
workspace lint configuration and is never enabled by default. Both
`SCREENSHOTS_NATIVE_PRINT_QUEUE` and `SCREENSHOTS_NATIVE_PRINT_PORT` must exist
at compile time. Their literals are compiled into app-core. Runtime environment
variables cannot change this pair or enable the guard. The helper admits that
one exact queue/path pair in the instrumented build, while retaining all other
native printer/DEVMODE/rendering logic. The normal shipping binary must reject
the same newly owned Local Port queue without creating a spool job.

Use an ordinary non-print-instrumented optimized application as
`$ShippingApplication` below. Preserve its exact build command and hash. Then,
from an elevated hosted Windows developer PowerShell (the commands below are
instructions, not evidence they have run):

```powershell
$FixtureScript = (Resolve-Path verification/windows-spool-native-fixture/run.ps1).Path
$Fixture = & "$env:SystemRoot\System32\WindowsPowerShell\v1.0\powershell.exe" -NoLogo -NoProfile -NonInteractive -STA -File $FixtureScript -Stage Prepare | ConvertFrom-Json
if ($LASTEXITCODE -ne 0) {throw 'Private fixture preparation failed'}
$env:SCREENSHOTS_NATIVE_PRINT_QUEUE = $Fixture.queue
$env:SCREENSHOTS_NATIVE_PRINT_PORT = $Fixture.port
$env:RUSTFLAGS = '--cfg screenshots_native_print_acceptance --check-cfg=cfg(screenshots_native_print_acceptance)'
$env:CARGO_TARGET_DIR = (Join-Path $PWD 'target/windows-spool-instrumented')
cargo build --locked --profile release-fast -p screenshots
if ($LASTEXITCODE -ne 0) {throw 'Explicit instrumented build failed'}
$InstrumentedApplication = (Resolve-Path target/windows-spool-instrumented/release-fast/screenshots.exe).Path
$Controller = Join-Path $Fixture.directory 'controller.exe'
cl.exe /nologo /std:c++20 /EHsc /W4 /O2 /D_WIN32_WINNT=0x0A00 verification/windows-spool-native-fixture/controller.cpp "/Fe:$Controller" "/Fo:$($Fixture.directory)\controller.obj" winspool.lib user32.lib gdi32.lib advapi32.lib ole32.lib
if ($LASTEXITCODE -ne 0) {throw 'Owned native supervisor build failed'}
& $Controller supervise $FixtureScript $Fixture.directory $ShippingApplication $InstrumentedApplication > (Join-Path $Fixture.directory 'controller-proof.json')
if ($LASTEXITCODE -ne 0) {throw 'Owned native spool/driver gate failed'}
```

Do not reuse the compiled instrumented application for another GUID/path. A
normal product/package build must clear this guard and must not be built or
published from the instrumented target directory. Restore the invoking shell's
prior `RUSTFLAGS`/`CARGO_TARGET_DIR` and remove both compile-time variables after
the isolated build; no runtime application flag is introduced.

The native supervisor first runs a read-only PowerShell Preflight in its Job.
A prerequisite/collision failure acquires no queue/port authority and grants no
cleanup permission. Only after Preflight actually exits and its family reaps,
the supervisor creates the exact port through Localmon XcvData(AddPort), then
the exact private queue through AddPrinterW. API success acquires explicit
port/queue flags and the returned queue handle remains held through execution
and cleanup. PowerShell never creates/adopts a queue or port. Preflight and
Execute validate their actual native-controller parent PID and executable path.

The native supervisor creates each fixture PowerShell process suspended
in an atomically assigned kill-on-close Job and a private noninput station/
desktop. Up to eight processes and one GiB per process are permitted, covering
the inner PowerShell, exact CLI launcher, real controller/helper and native
WPF work. No driver/spool-service process is claimed to belong to that Job.
The real CLI retains its own 60-second supervision. Each invocation has a
70-second outer native wait, Preflight has a 60-second wait, and Execute has a
five-minute wait. Native acquisition and cleanup remain synchronous API calls.
Kernel/driver/spool cleanup APIs are synchronous and have no fabricated hard
timeout guarantee. The supervisor reaps its Job, then owns GUID-restricted
spool cleanup independently of the killed application family.

Before the application cases, native negative controls attempt queue and port
collision acquisition and verify zero destructive cleanup calls and unchanged
owned queue/port identity. An actual parked child in the Job proves that the
same cleanup gate returns pending with zero mutations and retained authority;
only the exact control child is then terminated and actually reaped.

Only the generated GUID queue is paused/resumed or purged. Its driver, Local
Port monitor, exact output path and nonshared/local attributes are checked.
Pausing before each invocation makes the exact single spool job independently
observable and rules out another application submission before delivery. The
fixture then resumes the owned queue, requires it to empty and the output to
close for exclusive reading, bounds the package, renders its image-page subset
through WPF, checks one requested portrait Letter or A4 page, an independently
centered physical fit and opaque/half/transparent quadrants, and deletes the output.

Modern MXDW may produce OpenXPS by default. The oracle handles either XPS
namespace using explicit Canvas/Path/ImageBrush construction, not XamlReader
or host-wide format-policy edits. It permits a deliberately narrow generated
raster-page subset. Unknown markup, missing parts, interleaved representations
that do not resolve directly, color-conversion markup, unsupported image units,
extra pages, text or external resources fail rather than being silently
ignored. Extend that subset only after retaining and inspecting an owned
native failing artifact against the primary format/API contract; do not weaken
the physical/color assertions to force a passing result.

Real file-port sharing denial supplies a native delivery fault: the CLI may
truthfully acknowledge spool acceptance while the sole owned job reports an
error/blocked/offline state and no output. Broken public stdout supplies the
ambiguous command-result case after spool acceptance, with exactly one job
and an independently verified page. The fixture asserts no duplicate
application submission. The spool service may retry delivery of its same job;
this is not misreported as an application retry or physical output.

ImageBrush elements must have the page namespace and no unsupported child or
property markup. Nested brush-transform and foreign-namespace mutations must
fail at their precise rejection boundary, in addition to the golden/fit/color/
page-count oracle controls. The parser never ignores a rendering property.

The supervisor performs no spool/port/pixel cleanup until a fresh Job query
proves actual family emptiness. A pending family parks with all authority
retained before any destructive call. It checks the acquired queue's retained
creation handle and owner/protected DACL, and each acquired port's actual Local
Port monitor before deletion.
It never opens/adopts a preexisting collision for cleanup. It then purges only
its acquired queue, waits for no jobs, deletes that queue
and its exact Local Port, then deletes the generated source/sink pixels. Logs
and sanitized JSON remain under the private ACL. Cleanup failure keeps the
supervisor alive, holding its Job and exact authority, prints `OWNED CLEANUP
PENDING` and fails the gate; the CI owner must destroy the disposable host.
Do not kill it and continue using that host or claim cleanup completed. Never
solve a fixture failure with a global spooler purge/restart or default change.

Only a zero controller exit, `passed:true` in both proof JSONs, all native
ownership controls, all strict brush mutation controls, all four
instrumented cases and `normal_policy_refused:true` establish the proposed
gate. Source/build hashes, actual OS/runner identity and retained native logs
must accompany it. This would establish real downstream Winspool/driver/GDI/
spool/output behavior under a disclosed admission seam; it would not establish
unmodified shipping file-port acceptance, calibrated hardware output,
reference-hardware performance, blocked-driver fault coverage, driver
compatibility beyond MXDW or any physical delivery.


## Current host result — blocked before product execution

The actual Windows2022 host has the exact installed Microsoft-signed XPSv4
driver and running spooler. Original native preflight now passes through the
same private-desktop/Job boundary. Original acquisition subsequently returns
AddPrinterW1726 while Windows records spoolsv/localspl access violation
0xc0000005; event and error evidence was downloaded and hash-verified.
Product printing, ownership controls after acquisition and pixel output were
not executed by these probes. They establish a blocked host gate, not a pass.

The original uncertain-acquisition retention remains in force; no destructive
queue/port/pixel cleanup is authorized from that ambiguous RPC failure. The
three-minute ownership probe is bounded by its disposable CI host. No queue or
port cleanup success is fabricated. No service restart, driver install, ACL
change or generic-driver substitution is used as a workaround.

The full run44355258320 is separately pending with hard cap20:08:29UTC on
2026-11-18 and remains untouched. Detailed source hashes, verified preflight and
crash artifact identities, event versions/offset and scope are retained in
`target/verification/windows-spool-blocked-host-final/evidence-index.json`.


A final explicit API-precondition probe confirms winprint/RAW support and uses
only the controller's already-assigned SeLoadDriverPrivilege:0→2 enabled and
verified→0 restored and verified, scoped around AddPrinter. The same1726 and
spoolsv/localspl access violation persist. The reviewed/exercised fixture guard
is integrated; it grants no privilege or policy and preserves the exact ACL,
parameters, uncertainty, ownership and cleanup rules. Verified artifact14463708225
(run44381333600) and source/event hashes are referenced by the final evidence
index. This closes the enabled-caller verification gap, not product or pixel
acceptance; no causal claim about the earlier disabled state is made.
