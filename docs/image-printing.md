# Explicit image printing

This backend adds no print dialog, editor button, startup enumeration, default
printer lookup, saved destination, or automatic printing.

Linux/macOS with an existing non-temporary queue on an explicit local CUPS server:

```text
screenshots print-image INPUT --printer QUEUE --paper a4 --server 127.0.0.1:631
screenshots print-image INPUT --printer QUEUE --paper letter --server 127.0.0.1:631
```

Windows with an installed GDI printer queue:

```text
screenshots print-image INPUT --printer "Exact printer queue name" --paper a4
```

Both the destination and paper are required. Unix also requires an exact numeric loopback IPv4 CUPS
server and port. Queue instances, arbitrary print options, shell fragments,
printer discovery, multiple copies, and job cancellation commands are absent.
CUPS queue names use letters, digits, underscore, dot and hyphen; Windows queue
names may contain spaces and Unicode. Direct remote CUPS servers, hostnames and IPv6 endpoints are rejected. A local
CUPS queue can still target a configured network printer. The command never installs tools
or creates or modifies printer queues/settings.

The existing managed loader admits PNG/JPEG/WebP and supported truecolor BMP or
uncompressed TIFF, with its existing encoded 32 MiB, decoded 256 MiB, color,
orientation, malformed-input and regular-file rules. Decoding happens in the
isolated operation helper. Only final SDR sRGB pixels reach the spooler; source
metadata, source paths, history and original encoded image bytes are omitted.
The source is opened for reading and never saved or replaced. Printing uploads
those pixels to the explicitly selected spool destination, which can retain
them under its own policy.

One portrait page and one copy are requested. The image is centered, enlarged
or reduced to fit while preserving its physical aspect ratio. Unix streams
PostScript for A4 or Letter with a quarter-inch page border. Windows intersects
that border with the device's imageable area and accounts for independent X/Y
DPI and physical page offsets. Hardware margins, printer color conversion and
calibration still apply. Transparency is composited over white using rounded
8-bit sRGB channel arithmetic; there is no implicit black or transparent paper.
This is ordinary SDR image printing, not a calibrated proofing pipeline.

Unix uses a bounded IPP/1.1 client over an exact numeric loopback IPv4 TCP
address. It first sends Get-Printer-Attributes for `/printers/QUEUE` and requires
an exact `printer-name`, `printer-is-temporary=false`, accepting-jobs, PostScript
support and Print-Job support. A missing or ambiguous queue fails before sending
pixels. It then streams one Print-Job directly to that same address/resource,
with fixed title `Screenshot`, media, portrait, one-copy, simplex, one-page,
no-banner, one-up and no-additional-scaling attributes. Attribute fidelity is
required; only an unqualified IPP success with a positive job ID is acknowledged.
A substituted/ignored-attribute response leaves the outcome unknown, since the
spooler may already own a job. Queue configuration may change between query and
submission; the selected spool service remains the trusted destination authority.

No CUPS client executable or library is invoked. This deliberately avoids `lp`'s
native missing-queue network discovery and temporary-queue creation fallback.
There is no enumeration, DNS, proxy, redirected/authenticated/TLS upgrade,
credential lookup, default destination, queue creation, Cancel-Job or retry path.
Response URIs are never followed. Local plaintext IPP is the only transport;
direct remote/hostname/IPv6/TLS-only servers and authenticated queues are unsupported.
An existing local queue can forward to its configured network printer under the
spooler's own policy; this is not a remote TLS guarantee. Queues must expose the
CUPS `printer-is-temporary` attribute and accept the fixed print settings.

Requests use HTTP/1.1: bounded query Content-Length and chunked PostScript
streaming avoid a full spool buffer or a second pixel-encoding pass. Responses
require HTTP 200, `application/ipp`, one Content-Length of at most 64 KiB, at most
8 KiB/64 HTTP headers, at most 128 IPP attributes, IPP 1.1, the exact request ID
and a complete terminator. Chunked responses and other response forms fail closed.
Connections use a five-second connect timeout and ten-second individual I/O waits;
the controller still owns the overall 60-second deadline and cancellation.

The Windows path resolves its four Winspool functions with a System32-only
`LoadLibraryExW`/`GetProcAddress` table inside the explicit helper, avoiding new
direct Winspool calls/imports in the ordinary startup path. That table remains
loaded until all owned printer handles are closed. Windows opens only the
explicitly named queue, reads at most 64 KiB of port data
and at most 64 KiB of driver DEVMODE data, and requests/validates portrait paper,
100% device scale, simplex, uncollated one-copy output without a property sheet.
Known native hardware port families (`IP_`, `WSD-`, `USB`, `LPT`, `COM`, `TS`) are
accepted. File paths, multiport names, FILE:, PORTPROMPT: and other unfamiliar
ports are rejected, including the standard interactive PDF/XPS output ports.
Unknown vendor port families require a separately reviewed extension. Third-party
printer drivers remain trusted native code, whose memory, hangs, color behavior,
and UI behavior require native acceptance testing. No explicit GDI dialog API is
called; `DOCINFO.lpszOutput` is null and no output filename is supplied.

Successful JSON means only the platform spooler accepted the job:

```json
{"status":"accepted_by_spooler","physical_delivery":"unverified","copies":1,"pages":1}
```

It never asserts physical delivery. Failures before native submission are
reported as such. Once submission starts, timeout, cancellation, an incomplete
pixel stream, native error, missing/invalid helper acknowledgement, or result
output failure can leave the outcome unknown. The command does not retry or
claim a remote rollback. Check the explicit spool destination before choosing
to print again. Ctrl+C/termination requests stop the owned helper family; paper
or remote jobs already accepted can remain. A failed public stdout write reports
that acceptance occurred and tells the caller not to retry automatically.

## Lifecycle and resource cost

There is one admitted print per process, no backlog, no resident worker, and no
new runtime or package dependency. The CLI's blocking controller never starts
GPUI; the pixel helpers and GDI APIs are documented as isolated-worker-only
operations. A future UI integration must reuse this supervised worker boundary
off GPUI's foreground executor, carry cancellation/session IDs, and discard stale
results. This change does not implement such UI integration.

The controller reuses `capture_worker_transport.rs` for bounded inherited IPC,
nonce/owner validation and Linux group parent-death handling, and
`capture_worker_unix.rs` for reserved process-group identity and bounded cleanup.
The helper sends only an eight-byte acknowledgement; native diagnostics are
discarded so source names/printer responses cannot leak through logs. The
operation lasts at most the controller's 60-second native wait plus a 250 ms
cleanup attempt, except process creation itself and underlying kernel calls are
not promised to obey a hard deadline. An uninterruptible cleanup retains one
owned pending child and rejects another submission; there is no recurring
cleanup timer, replacement thread, or overlapping abandoned worker.

Unix retains the managed decoded image plus a maximum 64 KiB ASCII pixel staging
buffer, a bounded request smaller than 4 KiB, at most 64 KiB response bytes,
8 KiB HTTP header and 8 KiB read buffer, and at most 128 borrowed IPP attribute
records. Two sequential loopback TCP connections use OS socket buffers; none
survives the operation. Streamed PostScript is bounded by seven bytes/pixel plus
4 KiB and is never stored by the application. No PNG/disk round trip, pixel
snapshot, GPU texture or application image temp file is added. The sole helper
has a 1 GiB virtual-address limit on Linux and core dumps disabled. On macOS,
the authenticated fresh worker reuses the native-image budget: it measures its
own VM baseline, verifies small-success/over-limit-failure under a 96 MiB growth
ceiling, then allows at most 1 GiB VM growth for decode/printing. This is a growth
allowance, not a claim of 1 GiB total VM or process-private memory. It is installed
after exec, before image decoding or print access. Lowering the forked GUI/test
process to a fixed 1 GiB VM ceiling before exec caused EINVAL in native ARM CI;
that failed run is preserved. Unsupported enforcement still fails closed.
Linux parent death terminates that group. macOS reuses
the native kqueue lifeline, which adds one 128 KiB-stack operation-owned waiting
thread. Windows converts RGBA to opaque BGRA in the same owned allocation and
uses the existing suspended/atomic-job child launcher with one permitted helper,
kill-on-close and a 1 GiB process commit ceiling. Driver/spool-service copies and
printer-server storage are additional OS/server resource costs. These ceilings
are admission/containment choices, not measured performance results.

## Executed local verification (2026-11-17)

Fresh independent source reviews 2/3 are clean after review 1 identified CUPS
client discovery/provisioning fallback. The integrated direct IPP backend avoids
that fallback. Actual optimized runs pass three core pixel/layout/writer cases
and six desktop parser/IPP/process cases. They cover explicit destinations,
exact existing-queue admission, fixed attributes, bounded HTTP/IPP responses,
worker rejection/invalid acknowledgement, timeout, cancellation, group cleanup
and readmission. An initial filter selected zero desktop cases; the separate
correct module filter ran all six, and the zero-case block is not counted.

The real optimized CLI passed the owned virtual-IPP end-to-end fixture: missing
queue produces one query and no submission; accepted final RGB includes white
alpha flattening, one page/copy and no source metadata; original bytes stay
unchanged; a second explicitly requested job with broken public stdout produces
no automatic duplicate. No user queue or physical printer was read or used.
Linux all-target optimized metadata, Windows GNU all-target metadata, optimized
workspace/all-target Clippy, formatting and diff checks pass. Windows checking
first found unsigned SDK constants passed to a signed GetDeviceCaps index; their
explicit fixed-index conversion was corrected and the passing rerun retained.
The pre-restart unit run had no result and is preserved as interrupted.

Executable SHA256:
`c595658cf9ad7878073fb79c21a33935fe6a2d98509720bd1333cf6248c692b0`.
Exact source/merge identities, commands, initial failures and passing logs are in
`target/verification/native-print-integration/`. Native Windows SDK and owned OLE/print-process fixtures subsequently passed
on the exact `fac0328` source in run 44053816811; that is not driver output.
ARM macOS passed the pure print tests but failed the mock startup with EINVAL
before IPP; Intel macOS was cancelled. The post-exec memory-limit correction
awaits a fresh native run. Native GDI output/failure injection, macOS actual
lifetime/IPP and reference-hardware resource acceptance
remain unverified. Metadata checking and private Unix success do not accept them.

## Required native CI and acceptance

1. Use the integrated source with the repository's locked Rust toolchain and
   packages for optimized Linux/macOS/Windows builds. Compile the public core,
   CLI registration and both platform branches. Run only the focused core and
   owned-helper test cases first. The native tests must not inherit a default
   queue, CUPS server, client options or credentials.
2. On Unix CI, run `scripts/verify-image-print-virtual-ipp.py` with the explicit
   optimized executable. It listens only on an ephemeral 127.0.0.1 socket,
   accepts CUPS requests for its owned synthetic destination, stores one page
   only in test memory, and has no printer/queue provisioning or forwarding.
   Its missing-queue case must receive exactly one named query and no submission,
   enumeration or queue-provisioning request. It requires no CUPS installation.
3. On Windows CI, run the pure port validation/pixel tests, then explicitly
   compile `apps/desktop/tests/fixtures/print_image/fake_worker_windows.rs` to an
   owned mock executable. Set its absolute path in the test-only
   `SCREENSHOTS_PRINT_TEST_HELPER` variable and explicitly run the ignored
   `windows_owned_job_acceptance_rejection_timeout_and_cancel` test. It fails
   if that variable is absent and never routes to a real printer. Exercise pre-resume job
   attachment, inherited-handle identity, memory/process limits, acknowledgement
   failure, cancellation and uninterruptible cleanup. Exercise the GDI calls
   through a fake native API or an exclusively owned, non-forwarding virtual
   print driver; never select a user's queue, built-in prompt-to-file queue, or
   physical printer in unattended CI. Explicitly run the ignored core tests
   `windows_owned_gdi_pixels_geometry_and_cleanup` and
   `windows_owned_gdi_failures_latch_and_cleanup` with `--test-threads=1` in an
   optimized Windows test build. Their owned memory DC/DIB exercises the exact
   shipping geometry, alpha/BGRA conversion, SetStretchBltMode/SetBrushOrgEx and
   StretchDIBits body with native GDI and validates output pixels and DC/DIB
   cleanup. Device capabilities and successful document callbacks are controlled
   test inputs; StartDocW on the memory DC is also exercised as a real native
   refusal. These cases establish no Winspool, printer driver, successful native
   StartDoc/EndDoc, worker-to-GDI or spooler acceptance. That complete native
   print-output gate stays open until an exclusively owned, non-forwarding
   virtual-driver fixture exists and is run. The Unix and owned process-mock
   fixtures do not establish it either.
4. Inject missing queue/service/driver, malformed images, unsupported paper, invalid
   device geometry, stream break, StartDoc/EndDoc failure, stdout failure and
   timeout after accepted submission. Confirm no automatic duplicate/remote
   cancellation and no sensitive native diagnostics.
5. Measure optimized helper/controller peak private/resident memory and active
   CPU for synthetic 1440p/4K and maximum admitted images; report driver/server
   resources separately. Check return to idle and zero new startup activity,
   macOS sandbox/local IPP compatibility, actual hardware margins/mixed DPI and
   third-party driver behavior before making production or calibrated-output
   claims. No reference-hardware performance or physical delivery claim follows
   from the held static parse checks.

## Native primary references

- [RFC 8010 IPP/1.1 encoding and HTTP transport](https://www.rfc-editor.org/rfc/rfc8010.html).
- [RFC 8011 Print-Job, exact attributes and response semantics](https://www.rfc-editor.org/rfc/rfc8011.html).
- [OpenPrinting named lookup discovery and temporary-queue fallback avoided here](https://github.com/OpenPrinting/cups/blob/v2.4.16/cups/dest.c).
- [OpenPrinting scheduler operation dispatch and queue lookup](https://github.com/OpenPrinting/cups/blob/v2.4.16/scheduler/ipp.c).
- [Microsoft CreateDCW](https://learn.microsoft.com/en-us/windows/win32/api/wingdi/nf-wingdi-createdcw).
- [Microsoft DocumentProperties without DM_IN_PROMPT](https://learn.microsoft.com/en-us/windows/win32/printdocs/documentproperties).
- [Microsoft DEVMODEW fields](https://learn.microsoft.com/en-us/windows/win32/api/wingdi/ns-wingdi-devmodew).
- [Microsoft GetPrinter](https://learn.microsoft.com/en-us/windows/win32/printdocs/getprinter).
- [Microsoft StartDocW blocking behavior and spool ID](https://learn.microsoft.com/en-us/windows/win32/api/wingdi/nf-wingdi-startdocw).
- [Microsoft StretchDIBits](https://learn.microsoft.com/en-us/windows/win32/api/wingdi/nf-wingdi-stretchdibits).
- [Microsoft EndDoc blocking behavior](https://learn.microsoft.com/en-us/windows/win32/api/wingdi/nf-wingdi-enddoc).
- [Microsoft System32-only LoadLibraryExW](https://learn.microsoft.com/en-us/windows/win32/api/libloaderapi/nf-libloaderapi-loadlibraryexw).
- [Microsoft GetProcAddress](https://learn.microsoft.com/en-us/windows/win32/api/libloaderapi/nf-libloaderapi-getprocaddress).

## Native ARM macOS runtime evidence (2026-11-17)

Auxiliary snapshot `b85db9261104fdac868d4d5d1d79b901445a6e65`, application
source `9f2e135bbcf3671811018faa3c028938210e7079`, passed native ARM macOS job
134043931856 in [run 44084691005](https://github.com/lejylabs/screenshots/actions/runs/44084691005).
The actual fresh-process VM enforcement probe, three core print controls, six
desktop controls including worker rejection/timeout/cancel, and the real private
IPP CLI fixture pass. The fixture verifies missing-queue refusal, exact existing
queue submission, stripped metadata, preserved source and stdout failure without
retry. It never uses a physical printer or user queue. The log is preserved at
`target/verification/native-b85db926-macos-arm.log`. This closes that source's
ARM runtime check after the earlier pre-exec EINVAL failure; Intel job 134043931578 was canceled during later optimized app-UI compilation
(the cancellation reaped Cargo/rustc); it supplies no full Intel print-runtime
pass. Windows GDI output/driver faults, calibrated physical output and reference
hardware resource acceptance remain separate gates. Later source changes are
not covered by this snapshot.

## Owned native Windows GDI evidence — 2026-11-18

Both exact optimized ignored GDI cases executed and passed in Windows job134704173055, run44276006510 (snapshot55a202af57187a63f9ebcbbc405ef4d39186a50d). The independent step requires exactly two discovered cases and retained the native output log even though the earlier unrelated drop-fixture C++ compile failed. This establishes actual owned memory-DC raster geometry/color/alpha-composition/cleanup and native StartDoc refusal, plus the separately labeled controlled document-fault branches. It does not establish real Winspool/driver submission, physical printing or resource budgets. The exact source changes are committed in `b411f73`.


## Owned real-spool fixture proposal

`verification/windows-spool-native-fixture/README.md` describes an explicitly
instrumented disposable-host fixture for the real CLI/helper, Winspool,
Microsoft-signed builtin XPS driver and owned Local Port output. Normal builds
still reject that file port; the fixture requires a same-queue normal-binary
refusal control. Only its compiled exact GUID queue/private sink pair can pass
its separate never-default-enabled source guard. It adds no product CLI flag,
queue installation, file-port support or runtime environment bypass.

The fixture is authored source, not native acceptance evidence. It must build
and run with retained source/binary identities, exact spool/output and fault
oracles and verified whole-family/queue/port/pixel cleanup before the downstream
real-driver gate can pass. Even a passing instrumented run would not establish
unmodified shipping file-port acceptance, other drivers, native hangs,
calibrated physical output or the reference-hardware resource budgets.


## Real-driver gate blocked on the current CI host — 2026-11-18

Actual MSVC compilation/linking and PowerShell parsing pass. The exact native
CreateProcess/environment/private-desktop/Job preflight independently passes
with the original ACL and signature checks, no queue/port acquisition and no
product execution (run44373977655, verified artifact14460011667).

A no-Rust probe of the original acquisition/ownership-control boundary then
observes AddPrinterW error1726. Verified Application event1000 records
`spoolsv.exe`10.0.20348.5756 faulting in `localspl.dll`10.0.20348.5866 with
access violation0xc0000005 at0x12dfa; SCM7031/7036 record OS-initiated service
recovery. The test made no service restart/install/default change. The
read-only service/crash artifact14460229686 (run44377124632) was verified at
SHA256 `9b9feedb6464acae469e9fa0143da37461f9737ec2ed89d7ef93d222f95f278c`.

This blocks real Winspool/driver qualification on that host before product or
pixel execution.1726 does not prove nonexecution of the RPC: the original
uncertain-acquisition gate retains authority and refuses destructive cleanup.
The bounded ownership probe reaches its three-minute cap; queue/port cleanup is
not claimed, and the disposable host must be discarded. No malformed source
API field was established by the targeted review, and no ACL, API parameter or
output oracle is relaxed to obtain a pass.

The existing full run44355258320 remains a separate pending observation, with
its configured hard cap at2026-11-18 20:08:29UTC; the smaller probes do not
simulate its product result. Exact source/event/error hashes and scope are in
`target/verification/windows-spool-blocked-host-final/evidence-index.json`.
The50MiB resident target still fails, and physical printing, GPU/reference
hardware, account and signing/certificate gates remain separate.
