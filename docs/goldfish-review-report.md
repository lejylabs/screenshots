# Non-UI delivery: EGM review record

Date: 2026-11-14–15. Elephant: root implementation agent. Goldfish reviewers used
fresh conversation turns and the installed EGM codebase-details or
implementation-readiness personas plus their supplied standards. Instruction-file
exclusion is not independently established. All reviews below were actual
read-only subagent runs, not simulated roles. Reviewers did not compile or run
tests; test evidence is recorded separately in non-ui-delivery.md.

## Completed reviewers and resolutions

| Reviewer | Result | Resolution |
| --- | --- | --- |
| goldfish_readiness | Completed; initial plan not implementation-ready | Exact first-tranche grammar, file ownership, storage bounds and packaging contracts added. Remaining large capability families remain open, not accepted as delivered. |
| goldfish_codebase | Completed; CLI/storage/cancellation gaps | Read-only maintenance separated from migrating history; explicit no-history upload semantics and process-interruption limits documented; executable loopback/WAL fixtures added. |
| goldfish_storage_round2 | Completed; non-UTF-8 serialization Major | Candidate paths use explicitly labeled lossy display strings; native API retains exact paths. Added subprocess regression. Post-sync cancellation check added before publication. |
| goldfish_cli_release | Completed; retired macOS runner and test isolation Majors | Switched to active macos-15 ARM; all process fixtures isolate HOME/USERPROFILE/XDG/APPDATA/LOCALAPPDATA; help and directory behavior corrected. |
| goldfish_credentials | Completed; macOS no-prompt Blocker | Explicit Data Protection Keychain plus no-auth-UI policy; unsupported signed identity fails closed. Windows case-insensitive references and native payload/CLI contracts documented. Native runtime acceptance remains open. |
| goldfish_image_qr | Completed; WebP allocation and QR truncation Blockers | Immutable bounded WebP preflight/EXIF handling added. Reject eight-grid saturation rather than returning eight of nine codes. Added malformed WebP, nine-code and cross-platform isolation fixtures; exact image math/defaults documented. |
| goldfish_ocr | Completed; signal lifetime and descendant tests Majors | Process-lifetime conditional shutdown preserves signal termination during blocked final stdout; engine cancellation still cleans up its process group. Added descendant PID, SIGINT/SIGTERM, held-pipe deadline and blocked-stdout fixtures; result format specified. |
| goldfish_webp | Completed; valid animated unknown-chunk Major | Admit bounded trailing unknown ANMF chunks, reject extra known reconstruction chunks; valid JUNK-suffix animation fixture added. No further security blocker reported by source inspection. |
| final_goldfish_codebase / final_goldfish_safety | Completed; writable Windows sync handle Major and evidence/test-name issues | Open the verified backup read/write for sync; record actual commands/results and distinguish CLI backup from the core held-WAL fixture. |
| final_code_round2 / final_ready_round2 | Both completed; no findings | First clean source-review round for implemented slices. |
| final_code_round3 / final_ready_round3 | Both completed; no findings | Second consecutive clean source-review round for implemented slices. Architecture coverage was clarified to the three actual CI targets. |

Applied fixes are not permanently settled: a later reviewer may reopen a failed
fix. No requested reviewer above was counted as clean after failing or timing out.
The implemented slices reached two consecutive clean source-review rounds. No
assertion of full-plan convergence or full implementation-readiness is made. The original full backlog still contains unimplemented work and native
acceptance gates; finishing these reviewed slices does not close those findings.

## Independent checks and release limits

The root runs the real executable tests, optimized workspace suites, native
synthetic-window probe, optional installed-engine OCR check and archive smoke
checks. Record completed commands/results in the delivery plan, including any
failures and fixes. Source reviews do not establish native execution.

Dependency inspection discovered RUSTSEC-2026-0459 in bundled SQLite; the
rusqlite/libsqlite3-sys/migration family is updated together to the patched
SQLite 3.54.1 family. The current default cargo-deny advisory policy still fails
on nine unmaintained transitive packages: instant, paste, proc-macro-error,
rustls-pemfile, rustybuzz, ttf-parser, zbus_names, zvariant and zvariant_derive.
No ignore list or lowered advisory policy was added. The separate GitHub glib
alert requires upstream-family review; it is not represented as closed here.

Windows/macOS runtime, signed identity/permission flows, real credential stores,
mixed-DPI hardware, physical presentation and the full reference-hardware/soak
budgets remain unverified. The current Linux/XWayland native probe uses only
synthetic pixels and this application's own windows, never desktop content.

Final local executable, native synthetic-window, installed OCR engine and actual
archive checks passed as recorded in non-ui-delivery.md. Native GitHub CI remains
a separate remote gate; pending jobs are not acceptance evidence. A subsequent
ignored trusted-fixture WebP timing test adds measurement evidence without
changing production behavior.

## Continuation review (2026-11-15)

The user explicitly revoked shutdown; leave the workstation powered on. Actual
prior CI finished: Linux passed, Windows packaging failed because Python decoded
UTF-8 metadata as cp1252, and macOS live-WAL backup rejected `/var` symlink
ancestry through SQLite NOFOLLOW. Fixes preserve native no-replace and final-file
symlink protection; five packaging regressions and the optimized core suite pass
locally. Native CI rerun remains the cross-platform verification step.

`continuation_plan_review` completed read-only: missing exact file allowlists and
ignored per-frame topology contradictions were Major findings. Exact paths are
now listed, and native stitched capture requires each frame's verified geometry
to match the admitted display. Rotation and pure-vs-native scale contracts are
explicit; tests cover missing and mismatched geometry.

`workflow_review` completed read-only: no implementation blocker found. Contract
Majors concerned no-op recipes re-encoding, exact JPEG/extension/error behavior,
and physical-coordinate assumptions. These are documented. Seven real workflow
process tests pass, as does the new history-query process fixture. Source-review
completion does not replace native capture or platform runtime tests.

The same `workflow_review` follow-up covered interval capture and image batches.
It found documentation mismatches about recreated output directories and native
allocation limits, plus missing final-summary failure context and PUT target
reuse. Documentation now states the exporter behavior/native limits and repeated
PUT preconditions; final summary failure explicitly preserves the remote success
count. This same-agent follow-up is not counted as a fresh convergence round.

Recording design was reviewed by the existing readiness reviewer (same-context
follow-up, not a fresh convergence claim). The main finding was that requested
seconds were only a frame-count contract. Documentation now distinguishes
nominal frames from wall-clock/timestamp duration. Verification uses average
frame-rate metadata rather than Matroska's derived r_frame_rate, which can differ
from requested FPS. Real synthetic recording found that mismatch before commit.
Resource-limit tests also revealed child core dumps on SIGXFSZ; child RLIMIT_CORE
is now zero, test dumps were removed, and the tests passed again without dumps.

Recording follow-up review identified that Linux piped core collectors may
ignore RLIMIT_CORE. The documentation now limits the no-core claim to ordinary
file dumps and records system crash collection as outside the child guarantee.
This machine uses file-based `core`; no promise is made for other system policy.

Image collection same-reviewer inspection found no concrete defects in argument
limits, raw-alpha preservation, output bounds, row-major splitting or partial
publication status. Eight executable collection tests passed. Recording's
Linux parent-death setup now kills the direct engine; a SIGKILL-controller
regression passed. Nine recording process tests including actual synthetic
FFmpeg output passed after those changes. System crash-collector and deliberate
escaped-descendant limits remain explicitly documented.

GIF/image-info same-context review found no data-path issue but identified a
parent-death setup race. The expected controller PID is now captured before
spawning and checked before and after Linux PR_SET_PDEATHSIG. Thirteen focused
recording fixtures including actual GIF/video output passed after the fix.

Generic file-upload same-context review identified ambiguous stdout failure
after confirmed remote success and an overstated wall-clock deadline. Checked
write/flush now states remote persistence, and docs preserve the uninterruptible
filesystem/runtime-shutdown limitation. Four loopback process tests, including
a named pipe rejected before opening, passed after the metadata preflight was
included in the rebuilt binary. Existing image-upload validation is unchanged.

Scroll-image assembly same-context review found no concrete defect in KMP
candidate discovery, exact-byte seam validation, ambiguity rejection, bounded
source/output/verification work, or no-replace publication. Six executable
fixtures and two algorithm fixtures (including exhaustive small-sequence oracle
and hash-collision checks) passed. No native scrolling was performed.

History-reference-management same-context review found no new implementation
defect. Wording was narrowed to the actual optimistic path/date identity check;
import/create and remove/relink/no-create semantics are documented. Six CLI
fixtures and eight core history regressions passed without modifying images.

## User-checklist follow-up review

`checklist_reconcile` completed a fresh read-only comparison from main 523bc3c
to feature 500ee38. It found stale “missing” statements for CLI subsets and a
one-JSON-output rule conflicting with upload-batch NDJSON. The historical scope
heading and central contract are corrected; gap-analysis now has a dated
reconciliation without marking whole UI/native families complete.

`export_hash_review` independently reviewed the new hash and sanitized export.
Its cooperative-deadline, special-file-open and resource/format-disclosure
findings were fixed: both export paths reject nonregular files, the stripped
path retains a 60-second per-file check, and docs describe RGBA8/still conversion,
transient decode memory and partial-report semantics. No remaining Blocker/Major
was reported after source reinspection; tests were run by the root, not by the
reviewer. `checklist_delivery_review` is a separate fresh final review.

The latest independent checklist delivery review found no implementation defect.
Its documentation findings (original-copy byte/signature limits, filesystem
publication requirements and query lookahead) are now explicit. Both reviewers
ran read-only; the root's full 318-test run and export/hash/audit fixtures provide
the separate execution evidence.

Canceled native CI logs show compilation/linking, not a demonstrated runtime
hang. Test binaries now use the existing optimized release-fast profile to avoid
per-test thin-LTO cost; shipped artifacts still use release with thin LTO.

`png_inventory_review` independently inspected the bounded PNG inventory parser
and found no functional blocker. Its documentation findings were addressed with
exact palette/APNG/bit-depth acceptance and one-input/JSON/exit-status semantics.
Five subprocess tests and two unit tests passed; no image pixels or metadata
values are printed by the command.

## Windows OCR and scheduled-capture review

`windows_ocr_review` read the Codebase Details persona and Re-Implementation
Standard, repository rules, native API documentation and both OCR launch paths.
No production-code blocker/major remained. It caught stale Unix-only wording,
incorrect job-handle-lifetime wording, and a missing-engine test that assumed
PATH was Windows' only executable search location. Those were corrected; a
unique absolute absent path now checks the failure separately from CLI lookup.
The later same-context inspection of scheduled capture found no actionable
finding in timestamp admission, monotonic waiting, output or frame lifetimes.
That second work-package inspection is not a new fresh-context convergence round.

`ocr_schedule_readiness` separately read the Implementation Readiness persona
and standard, both contracts, implementation/tests and CI wiring in a fresh
context. It returned no unresolved actionable finding. Root execution evidence
includes GNU Windows full-app/test cross-check and the three Wine process tests;
neither reviewer ran native Windows tests. These are completed reviews of two
scoped slices, not full-backlog or native-platform acceptance.


The direct-ShareX bridge received a same-context codebase review: no actionable
blocker/major in extension selection, existing HTTPS/secret validation, bounded
profile reads or JSON-only credential commands. The optional ShareX name is
discarded and no profile is persisted. This is a follow-up inspection, not another
fresh-context convergence round. Root/implementer execution separately passed the
synthetic parser tests and eight local TLS uploads.


The dependency follow-up matched the glib VariantStrIter defect to upstream
commit b5a4071e439bef2b5eea76c3aa25e5ae84839e34. Root vendored exact glib 0.18.5
and verified that only the two pointer-mutability lines differ in source. Eleven
upstream iterator tests passed optimized in an isolated copy (unneeded benchmark/
compile-test dev dependencies removed only from that test copy); a workspace
regression also passed forward/reverse Unicode/empty-string iteration. This
fixes the identified code defect without inventing a safe package version. The
remote version-based glib alert and nine unmaintained-package deny errors remain
explicitly unresolved; no advisory ignore was introduced.


The same reviewer compared the gpui-pre-reqwest PEM migration to published
0.12.15 and rustls-pemfile 2.2.0: no actionable difference in certificate/CRL
filtering, errors, identity key selection, or unknown/ECH section handling. It
confirmed required alloc/std feature wiring and API availability in pki-types
1.9.0. The capture-sequence format follow-up also had no actionable finding.
These are same-context follow-ups, not fresh rounds; executable evidence is
recorded separately.

Linux window follow-up found a concrete XCap current_monitor defect: incorrect
vertical coordinate, mixed coordinate units and overflow before image admission.
Linux window capture now bypasses that optional association and records unknown
display metadata, retaining authoritative window_id and exact drawable pixels.
The reviewer checked current CLI/window-editor consumers for compatibility.
Its remaining enumeration limits are explicit. Root/implementer native fixtures
passed against a self-owned XWayland window; the final source passed 349 regular
workspace tests and Clippy. This does not establish native Wayland capture.


Web-time and workflow follow-up source review found no production defect. The
21 upstream source changes are only instant:: to web_time:: substitutions;
native std::time identity is preserved. Blur/pixelate use current flattened
pixels, validated rectangles, existing alpha algorithms and bounded step-local
scratch. Reviewer caught an outdated example result step count, now corrected.
The FIFO regression runs in a killable child so a blocking regression is bounded.
These are same-context reviews; no new fresh-context convergence is claimed.


The macro diagnostic-helper follow-up found no actionable issue: all source
changes are namespace substitutions, exact helper3.1.1 configuration matches Syn2,
and stable abort/panic propagation and cleanup semantics match the original.
Nightly auto-detection is deliberately not claimed. Positive and six negative
compile fixtures were executed separately by the implementer.


Pastey follow-up review found no actionable issue: the two aliases leave codec,
CPU dispatch and assembly sources unchanged, and consumers use supported simple
concatenation without the replacement's differing case-transform forms. The
existing MSRV remains satisfied. Independent before/after AVIF, color-macro and
NASM fixtures passed; no ARM/native UI coverage is inferred.


Linux autostart same-context review found the need for truthful post-mutation
stdout failure reporting; checked write/flush now states completion explicitly.
Exact ownership, no-replace creation and quoting were reviewed against the XDG
specification. Scope is only its managed user entry; system-wide startup and the
final hostile unlink race are documented. Six isolated tests and desktop-file
validation passed; no real autostart entry was changed.


Windows autostart same-context review found no implementation blocker after
canonical-prefix CreateProcess coverage was added. The temporary registry
fixture remains outside the real Run key. macOS file-only autostart review
confirmed XML/ownership semantics and recommended directory fsync; that change
passed the three focused tests. Wine/Linux evidence does not establish native
Windows or macOS login acceptance.


The explicit capture-task upload extension was reviewed for final-pixel ordering
and partial-success reporting. Review caught a duplicate PNG encode for local
PNG plus upload; one bounded encoded buffer now feeds atomic local publication
and upload, with a no-overwrite regression. The native synthetic-X11 CLI chain
passed crop/redact/save/local upload with exact saved/transferred bytes and
independently decoded pixels. No user screen or remote provider was tested.


Windows recording review identified that setting a DACL alone does not protect
FAT/exFAT storage. The resolved path checks FILE_PERSISTENT_ACLS on the actual
directory and verifies a protected exact current-user DACL before any pixels are
written. Five Wine process/ACL/stream fixtures and Unix regression tests passed;
real Windows FFmpeg/gdigrab remains unrun. A parent-enforced output cap discards
overflow before writing, and Job Object shutdown precedes worker joins.


Lossy WebP review found and fixed retained-original memory during downscale and
missing nested libwebp license/patent distribution. Callback state, synchronous
FFI lifetime, panic containment, byte cap and atomic cleanup were reviewed;
packaging now requires the exact nested notices and tests their archive bytes.
Capture task/sequence quality integration was reviewed without code findings;
docs distinguish native encoder scratch from admitted image bytes.

## Further unattended slices (2026-11-15)

The following were reviews by the existing reviewer in its continuing context,
not new independent Goldfish contexts or new two-round convergence claims.

- Region OCR/QR review required explicit argument/result contracts, the inherited
  full-display metadata ceiling and exact returned backing-size checks. All were
  added. Separate backend inspection found the current Wayland full-file path
  and uncalibrated scaling; the new commands reject those cases before capture.
- Recording cursor review confirmed input-option ordering and unchanged omitted
  defaults/JSON. Private Xvfb tests independently decoded the exact pointer patch;
  this does not establish Windows pointer rendering or physical display timing.
- Multi-region review required before/after selected display geometry, actual
  backing/identity checks before any saves and full-display X11 wire limits.
  Review then found a final symlink bypass via trailing `/` or `/.`; path-component
  normalization and targeted regression tests closed it. Partial-save errors
  preserve confirmed earlier files and disclose ambiguous current publication.
- FileUploader-context review kept acceptance confined to explicit `upload-file`,
  preserved image/UI import rules and rejected JSON file bodies before network
  work. Official source inspection also corrected claims about the existing
  object-valued JSON `.sxcu` extension; it is not ShareX image interoperability.

Test totals, artifact hashes and remaining native gates are recorded in the
[delivery log](non-ui-delivery.md); review findings are not test results.

## Offline GIF assembly review (2026-11-15)

The fresh-context `gif_readiness` review found a possible transparent-palette
collision and missing result/forced-termination contracts. The implementation
now checks every final palette index against the source alpha mask; the
documentation states exact JSON output and possible abandoned staging files.
An independent FFmpeg test then found opaque background disposal. Reserving an
unused transparent index on opaque frames fixed the observed defect; frames
using all 256 indices are rejected. A same-context follow-up requested explicit
255/256-color boundaries and a flush failure through the real atomic writer.
Those tests were added and passed. The follow-up is not a fresh convergence
round or a native-player acceptance claim.

## TIFF input review (2026-11-15)

The fresh-context `tiff_input_goldfish` read the implementation-readiness,
feature-spec and re-implementation standards and audited the contract against
the pinned TIFF/image source. It requested explicit classic IFD/inline/endian
layout, file ownership and execution phases; those were added. Same-context
follow-ups found no parser or integration blocker and requested the public
budget/PNG-only regression, which was added. Source reviews do not establish
runtime results or a fresh multi-round convergence claim.

## Upload-result postprocessing review (2026-11-15)

Fresh-context `upload_results_goldfish` identified two implementation hazards:
CommonMark entity decoding in link destinations, and sequence-form acceptance
by derived JSON struct decoding. The implementation escapes URL ampersands and
requires an object before decoding. The reviewer also corrected an unreachable
QR-capacity test assumption and requested alternatives/build phases. These
were resolved. Same-context follow-up found no new blocker; actual renderer
and executable results are in the delivery record.

## Thumbnail cancellation review (2026-11-15)

Fresh-context `thumbnail_cancel_goldfish` verified the pinned dimension/window
math and identified an existing admission gap: clearing a page discarded the
active thumbnail slot before its worker finished. Initial Quit checks also
omitted that slot. The slice therefore retains the slot until matching
completion and cancels/waits in both Quit paths, without changing rendering.
The reviewer required behavioral cancellation fixtures and production-equivalent
atomic flag reads plus raw samples in performance measurements. These were
added; runtime results remain separate in the delivery log.

## Saved-PNG admission review (2026-11-15)

Fresh-context `saved_png_goldfish` corrected the proposed limit fixture:
dimensions become available only after PNG decoder construction reaches IDAT,
so test admission ahead of decompression rather than ahead of a missing-IDAT
error. Exact file ownership and implementation phases were also added. The
implementation review found no blocking defect and requested a direct opened-
handle oversize regression in addition to pathname preflight/FIFO tests. This
was added. The existing header-limit and aggregate-memory caveats remain
explicit; this slice does not claim all PNG allocations are strictly bounded.


## Library lifecycle continuation (2026-11-16)

Fresh `library_lifecycle_goldfish`, `library_lifecycle_round2`,
`library_lifecycle_round3` and `library_lifecycle_round4` reviewers audited the
lifecycle implementation. These are separate contexts, not same-reviewer
follow-ups. The first three rounds found reservation/recovery, pathname-alias
and indexed-query/cancellation Majors. They caused schema versions 7–9, held-path
delete durability barriers, staging reservations, conservative alias keys/native
identity checks and unresolved-only preparation with scoped SQLite VM checks.
Round four found no Blocker or Major; its stale module-comment Nit is fixed.
This is one clean source-review round for this slice, not full-plan convergence.

The author separately ran 66 history tests, seven storage tests, ten CLI tests,
Windows metadata compilation, Clippy and the real Linux fault-interposition
fixture. The final native fixture report is
`target/verification/history-round3-native-e2e.json`. The fourth reviewer also
checked query plans independently with 30,000 already indexed rows; the partial
unresolved index avoids a full-history scan during ordinary physical operations.
Read-only source assessment does not establish native Windows/macOS durability,
power-loss or inaccessible-drive acceptance.

The historical nine-advisory failure above is superseded for the current active
production graph: default `cargo deny check advisories` passes without ignores
following the maintained native dependency migration. The default-branch glib
alert and platform acceptance remain separate gates.


## Cooperative image work continuation (2026-11-16)

Fresh `image_cancellation_goldfish` found aggregate-budget gaps in sparse/skinny
traversal and untyped blur/PNG-flush cancellation. The implementation now shares
one counter through traversal and blur chunks, skips empty intersections, merges
pixelation bands and normalizes latched cancellation before propagating flush
errors. `image_cancellation_round2` independently found no Blocker or Major.
Its two Nits are fixed: unchanged-size reduction checks after its copy, and an
actual partial-width narrow crop now asserts cancellation on the third callback.
This is one clean source-review round; it is not full-plan convergence.

The author ran 301 optimized core tests, 11 Task and 10 workflow cases,
Windows core compilation, Clippy and native Linux output-fault E2E. The final
Nit-focused run passed 13 raster tests (two measurements explicitly excluded).
Independent before/after pixel hashes matched; narrow blur/sparse pixelation
improved and representative wide work did not cross the documented regression
threshold. Raw evidence: `image-work-budget-comparison.json` and
`image-output-failures-work-budget.json` under `target/verification`.
Native filesystem/codec/vector internals and reference-hardware presentation
remain separate gates. Native capture and SXCU Task integration have their own
reviews; these checks do not establish their acceptance.


## SXCU execution continuation (2026-11-16)

Five fresh SXCU source reviews ran with the bundled EGM persona/standard. The
first four rounds found auth-name/Cookie classification, preflight prompt,
Task failure events, encoded/raw link and inspection, XML body and frozen
NameParser inspection defects. Author fixes added independent Rust policy
checks, bounded shared helper body classification and known-value masking;
actual transfer, Task and inspection regressions were added for each finding.

The fifth capped review found raw forms beginning with JSON/XML-looking keys
were skipped after failed syntax sniffing. The Elephant corrected ordinary
parse-failure fallthrough while preserving successful JSON/XML and hard
resource/DTD refusals. Correction checks passed 26 optimized Rust/process tests,
134 helper assertions and 94 real loopback HTTPS requests, including finalized
pixels, failure events/later-action suppression, anonymous/copy/private-runtime
and retained old-process install/upgrade behavior. The review cap was reached;
this is not a clean fresh-review convergence claim. Source record:
`target/verification/sharex-round5-goldfish.md`; runtime correction record:
`target/verification/sharex-prefix-e2e.json`.

A separate Windows startup audit found suspended-child-to-Job assignment gaps
in the interpreter and other optional process workers. Atomic native association
is active follow-up implementation work, independent of accounts and signing.
The modern pinned SXCU contract and explicitly retained older importer subset
are described in `sharex-runtime.md`; no arbitrary-version parity or full-plan
completion is claimed.


## Native capture and templates continuation (2026-11-16)

The initial fresh isolation review found Unix descendants surviving natural root
exit, handoff admission theft after enumeration released BUSY, and run-task
rereading a previously validated Task/catalog. Corrections retain the unreaped
leader until owned-group signaling finishes, prove remaining live members with
bounded native inspection and post-registration identity checks, reserve the
handoff exclusively, and execute one immutable prepared Task/template snapshot.
Fresh `native_capture_isolation_round2` found no Blocker, Major or Nit. Earlier
catalog/GDI source reviews also corrected the DWM double-crop/resizing defect;
real Windows owned-window pixels remain an explicit native CI gate.

Author verification passed 25 worker/protocol tests, 16 Task/capture-task cases,
four catalog CLI cases, Windows app/test metadata and private native E2E:
17 CLI calls, 48,259 exact pixels and three rapid native hotkey taps. Final
private report: `target/verification/capture-worker-private-review-fixes-final.json`.
This source review does not turn compilation into physical/native acceptance.

Group ownership proof initially increased local latency; getpgid-first bounded
inspection reduced its syscall work without UID assumptions. Two remediated
serial repeats measured worker p95 37.234/37.379 ms. The latter inline baseline
was 24.999 ms: +12.380 ms/+49.522%. The Elephant retains per-operation processes
after that recorded remediation to preserve cleanup/privacy and zero default
idle worker; the 250 ms 4K target is unchanged. Actual process memory, all cases
and swap qualifications remain in `native-capture-isolation.md`. These component
measurements do not establish reference-hardware or end-to-end acceptance.


## Runtime packaging and encoded-input continuation (2026-11-16)

Fresh `package_import_goldfish` found manual Linux instructions separating the
executable from its runtime and an unbounded undeclared-file scan. The fixes
retain full version folders with controlled launchers and stream directory
entries, rejecting undeclared members immediately. Fresh
`package_import_round2` found no Blocker or Major. Its packaged documentation
link Nit is fixed during staging, with an archive assertion for the actual
uppercase companion file. The shared encoded-input reader preserves opened-file
extent, format validators, orientation/alpha and 32 MiB/256 MiB admission;
optional unreadable WebP EXIF retains the documented legacy fallback.

Portable/native-layout/installer fixtures passed 11/3/21. After the source
review, the actual refreshed intermediate ELF/runtime archive and `.deb` passed
private extracted execution: help/version, profile inspection and 134 helper
assertions, with a matching actual glibc floor and unchanged isolated user state.
Evidence: `target/verification/complete-runtime-package-abi.json`. This is an
explicit prebuilt intermediate executable, not final native/ABI portability or
system installation acceptance. No full-plan convergence is claimed.


## Authorized portal and URL input (2026-11-16)

Fresh `portal_import_goldfish` found no Blocker or Major in authorized-FD
ScreenCast/PipeWire still capture, strict source/serial/buffer/color validation,
finite session/process ownership, disk-fallback removal and explicit URL input.
Its two documentation Nits are fixed: transform/crop/header refusals are stated,
and URL limits use readable unit spacing.

Private real D-Bus/PipeWire tests cover 19 CLI calls, 40 exact RGBA pixels,
11 negative modes, actual missing-frame timeout, parent loss and source removal,
Task crop and named-source refusal. Two 4K generated-source observations retain
both repeats; latest p95 is 50 ms native frame and 91.327 ms CLI including PNG,
with process/whole-fixture memory separated. This is not physical compositor or
reference-hardware acceptance. URL core/executable fixtures cover exact pixels,
streamed 32 MiB rejection before EOF, cancellation, no redirect/implicit state,
Task output and preflight before a second GET. User devices/data are untouched.

The complete current integrated optimized workspace passed 786 regular tests,
62 opt-ins excluded across 57 outer suites. Clippy passed the configured denied
lints (existing warnings remain), Windows GNU dev app/tests metadata passed, and
default active-graph advisories passed without ignores. The two host-capability
UI fixtures are now explicit test fixtures; production capability gating stays
truthful on Wayland. Evidence: `target/verification/current-integrated-checks.json`.
Editor/macOS additions, real platform/signing/account gates and the remaining
full backlog are not closed by this snapshot.


## Durable Task scheduling and worker startup (2026-11-16)

Fresh `windows_atomic_goldfish` found bare leading-dot program resolution and a
provider sequencing documentation contradiction. Both are corrected; fresh
`windows_atomic_round2` found no source Blocker/Major. Its stale evidence Nit is
resolved through a new correction report, preserving earlier source hashes.
Native process/job/handle limits and long-path behavior remain Windows CI gates;
the original Wine resource-query failure was not weakened or counted as native
success. Default-prefix probe state was precisely restored and audited.

Fresh scheduler rounds one and two found actual-source reopening, orphaned owned
macOS wake recovery and an expired Windows timer causing an idle spin. Actual
opened-object checks now reach image/raw uploads; command guards cover native
launch/resume boundaries with documented race limits. Offline mutations reconcile
only their durable owned wake. Windows timers reset/consume cancellation signals,
including simultaneous normal/alarm readiness. The checked Unix program handle
is released after spawn. Fresh `scheduler_round3_goldfish` found no Blocker/Major.

Author fixtures passed 14 scheduler, seven seal/source, one identity and 44
HTTP/S3 cases (16 opt-ins excluded), provider/Task/workflow compatibility, seven
real executable groups, private Xvfb captures and upload-before-SIGKILL recovery.
A separately pinned 300-second Linux idle observation had no additional CPU
jiffies, context switches or file I/O, with swap/physical memory reported rather
than claiming zero footprint. Root's two same-production native timer fixtures
and leading-dot oracle passed supplemental Wine; real native wake/permission/
resource behavior stays separate. The integrated 786-test proof above includes
the current source and corrected fixture gating; final exact-head native CI is
still required.
