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
