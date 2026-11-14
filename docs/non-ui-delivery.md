# Non-UI delivery plan

Author: NewtTheWolf
Date: 2026-11-14–15
Status: implementation in progress; no platform or parity acceptance is implied.

## Request and constraints

Implement the outstanding functionality collected in PLAN.md and
docs/gap-analysis.md, without redesigning the UI. Use a feature branch,
Conventional Commits, incremental pushes, fresh EGM reviewers and end-to-end
tests. The user is away and cannot provide passwords, approvals or test input.
Use existing credentials only. Shut down the workstation after saving the final
work and reporting its state. Never confuse an unavailable native test with a
passing test or an implemented helper with a complete product feature.

The baseline is 0c4a455. Preserve the performance contract in PLAN.md: native
events, no resident polling, one admitted capture, bounded pixels and queues,
worker-side blocking operations, explicit outputs and uploads, no source-image
disk round trip, and unchanged user files on cancellation. Do not alter layout,
theme, toolbar or editor interaction. New headless commands must bypass GPUI
initialization and must not change the normal resident launch path.

## EGM execution

The Elephant maintains this plan and resolves findings. Goldfish reviewers use
fresh conversation contexts, read their installed EGM personas and standards,
and inspect the plan and referenced source. Record real reports and unresolved
findings. The user's unattended implementation instruction overrides interactive
confirmation checkpoints; it does not authorize claims without evidence.
Use the review-loop skill for implementation-readiness and codebase-detail
reviews. Review completed changes again before the final push. Do not claim
two-round convergence unless two consecutive completed rounds are clean.

## Work packages and acceptance

### 1. Reproducible build, CI and distribution

Inspect Cargo.toml, Cargo.lock, .cargo/config.toml, apps/desktop and
.github/workflows/rust.yml. Install and pin Rust 1.100.0 if published; do not
lower the manifest requirement to fit a local compiler. Match CI to the pin,
run formatting, Clippy and tests, and add macOS compilation. Produce explicit
portable archives, checksums, license notices and documented Linux launcher
installation. Packaging must stage files before publication, refuse accidental
overwrites and never change system installation during tests. Record unsigned
Windows and macOS signing limitations. Validate an actual Linux release artifact
with isolated config/data/cache directories. Do not publish a release or add
unrequested service credentials.

### 2. Storage safety and recovery

Inspect crates/app-core/src/history.rs, capture.rs, export.rs and settings.rs.
Choose bounded explicit recovery operations rather than an idle janitor. Preserve
original files and metadata references. Use SQLite's online backup for explicit
backups, not a bare copy of a live WAL database. Validate backups before restore;
never replace an open user database or silently erase newer data. Expose read-only
integrity checking and bounded orphan-part inspection through the headless CLI.
Do not delete unknown .part files or follow symlinks. Fault fixtures must prove
rollback, no overwrite, cancellation and recovery after interruption. Existing
schema migrations and indexed bounded history remain compatible.

### 3. Headless commands and explicit workflows

Add CLI routing in apps/desktop before tracing/GPUI startup. Reuse app-core
load/export/capture/history/upload functions; avoid another backend or runtime.
Commands include help/version, image conversion/resizing/basic transforms,
display/window enumeration and explicit still capture, history queries, storage
inspection/backup, and explicit uploads as supported by the existing APIs.
Inputs and output dimensions must be bounded before allocations. A supplied
output path never overwrites existing files. Uploaded files require an explicit
upload command; conversion never writes history, changes the clipboard or starts
network requests. Normal --show and diagnostic launch arguments keep their
existing behavior. Shell invocation of untrusted file names is forbidden.
Subprocess E2E fixtures run the real executable in an isolated temporary root,
check exact output pixels/format/dimensions and unchanged source bytes, and test
invalid arguments, existing destinations and output-free failure. Native capture
tests require explicit opt-in because they read real desktop pixels.

### 4. Remaining capture functionality

Track cursor/frame/shadow defaults, precise keyboard/snapping selection,
cross-monitor/stitching/multiple regions, interval/scheduled captures, HDR and
color correctness, topology events, Wayland capture/shortcuts and macOS signed
permission flows. Selection UI changes remain deferred by the user's UI exclusion.
Each backend change needs real session-specific capture/cancel/permission/DPI
acceptance; synthetic fixtures cannot establish it. No eager capture streams.

### 5. Remaining editing and outputs

Track arbitrary effect order with explicit opaque-redaction safety, richer
annotation objects, canvas/padding/shadow/watermarks, upscaling, annotated-document
transforms, richer typography and output preferences, printing/file drag-out.
UI-only choices remain deferred. Headless image operations may cover a bounded
subset, but do not mark an editor workflow complete from a CLI operation.

### 6. Sharing and library breadth

Track complete custom-uploader compatibility, service/OAuth/S3/SFTP integrations,
additional upload inputs/batches, explicit after-upload actions, Linux Secret
Service/macOS Keychain, library date/sort/cross-page actions, rename/move/retention,
missing-file reconciliation and upload-link search. Reuse validators and keep
secrets out of JSON, arguments, logs and history. New network actions remain
explicit; no watched folders or background retries by default.

### 7. Major capabilities and platform acceptance

Scrolling capture, OCR/QR, video/GIF/audio and workflow/actions each require a
separate bounded active-work design, cancellation and real end-to-end tests.
Pin-to-screen is a window/UI feature and remains deferred under the UI exclusion.
Recording needs an explicit codec/license choice; external tools must not be
silently installed or exposed as supported merely because a binary exists.
Autostart, notifications and platform associations require native integration.
Hardware acceptance includes mixed DPI/negative origins, reconnect, sleep/resume,
device loss, keyboard/IME/accessibility, actual presentation and clipboard paste.
Only the current Linux session is locally available. Windows/macOS checks from
CI are compilation evidence until their native runtime tests actually run.

## Verification and integration

### First implementation tranche: exact public contracts

This tranche delivers packages 1–3 and native secret-store adapters from package
6. Packages 4–7 remain requested and open; they are not considered delivered by
this plan or by completing the first tranche. In particular, no claim of complete
ShareX scope or complete E2E acceptance follows from CLI helpers. The initial
Goldfish round found the full backlog not implementation-ready; later feature
designs and native evidence are still required, not silently waived.

File ownership: root creates apps/desktop/src/cli.rs and
apps/desktop/tests/cli.rs, modifies apps/desktop/src/main.rs and Cargo.toml,
crates/app-core/src/lib.rs, workspace Cargo.toml/Cargo.lock and this document.
Release implementer creates rust-toolchain.toml, scripts/package.py,
scripts/test-package.py, packaging assets and docs/packaging.md, and modifies
.github/workflows/rust.yml. Storage implementer creates
crates/app-core/src/storage.rs and docs/storage-recovery.md; root exposes its
exports and enables rusqlite's existing backup feature. Credential implementer
modifies upload_credentials.rs and upload-authentication.md; additional native
dependencies require a recorded resource explanation. No UI files are assigned.

CLI grammar is documented by `screenshots --help`. Dispatch returns
Result<bool>: true for a completed headless action, false only for existing
desktop --show/resource-probe/exit-after launch arguments or no arguments.
Unknown commands exit 1 before GPUI; help/version exit 0. Headless actions print
one JSON value to stdout (help/version use plain text); errors go to stderr and
exit 1. Paths use native OsString and resolve relative to the invoking directory.
Upload paths are made absolute before calling the existing uploader. No CLI
command records history or copies clipboard data implicitly. `history DATABASE`
explicitly queries an existing application database and may apply existing schema
migrations; `storage check DATABASE` instead opens read-only and never migrates.
History output is at most 20 entries with has_more/next_before, not an unbounded
dump. Search keeps the existing 512-byte/16-word prefix-search contract.

`image convert INPUT OUTPUT` accepts width, height, rotation 90/180/270,
horizontal/vertical flips and JPEG quality. Reject duplicate/unknown options.
Apply EXIF orientation on decode, then rotation, horizontal flip, vertical flip,
then export resize. Reuse load_editor_image, ExportSize and save_image_as.
Reject zero/oversized dimensions, upscaling and transparent JPEG; infer output
format from extension. Preserve exact source bytes. Reuse upload_png with a
validated explicit profile, its 60-second deadline and no automatic history.
Subprocess loopback tests validate actual PNG bytes and returned JSON, including
server failure. Native capture is opt-in; no native capture acceptance is inferred
from tests of image conversion or argument parsing.

Signals keep the OS process-termination behavior: Ctrl+C terminates an active
headless process. Synchronous native capture/export cannot promise mid-call
cancellation. No new permanent signal thread is added. Interruption can leave an
unpublished UUID .part file; inspection reports candidates and never deletes them.
An interrupted upload may already exist remotely; no automatic retry or deletion
is attempted. Test signal termination of a stalled synthetic loopback upload and
prove unchanged source files/no application history. Hard native-call timeouts,
cooperative image cancellation and crash reconciliation remain open acceptance
work; this is an explicit limitation, not an achieved cancellation gate.

Storage API: check_history_database returns schema_version and quick_check;
backup_history_database takes source, fresh destination and cancellation callback;
inspect_part_files takes one directory and a cancellation callback. Source
connections open read-only without create/migrate/WAL-setting. Validate known
schema/version; SQLite progress/deadline bounds inspection. Backups use online
SQLite steps, deadline and cancellation, validate before no-replace publication,
and retain the original. Restore is only backup to a new destination; replacing
an active database is unsupported. Part inspection visits at most 10,000 entries,
returns at most 100 strict UUID-pattern candidates and a truncation flag, never
recurses/follows symlinks/deletes or declares a candidate abandoned. Native
publication reuses capture::publish_output; no fallback overwrites a file.

CI packages Linux x86_64 and macOS arm64 as tar.gz and Windows x86_64 as ZIP on
their native runners. Linux aarch64, macOS x86_64 and other architectures remain
unverified: the packaging helper requires a native host, and no additional
architecture's artifact or acceptance is claimed. Archive contents include the
executable, project/dependency licenses, README and platform launcher where
relevant. Run packaging fixture tests and extracted-binary --help/--version with
no display and isolated storage; full install/signing acceptance remains open.
Rust 1.100.0 was confirmed available and installed; CI must use that exact pin.

First review disposition: fix concrete CLI/storage/file-map/packaging omissions
using the contracts above. Accept no claim of implementation-readiness for the
remaining capability inventories. Keep those open rather than choosing codec,
provider or backend contracts without implementing and verifying them.

Run optimized focused tests for changed core operations, real executable CLI
E2E tests, existing synthetic native UI probe if available without interacting
with user data, formatting and Clippy. Record compiler/profile, commands and
results. Keep performance observations separate from budgets; do not rerun the
entire hardware benchmark for documentation or packaging changes.

Each implementer owns disjoint files. Only the root agent commits and pushes;
stage explicit paths, inspect staged diffs and retain Conventional Commit
boundaries. No force pushes. Verify the remote head after push. Record tests,
fresh review findings and every still-open work package here before shutdown.

## Progress and evidence

- Branch created: feat/non-ui-delivery.
- Initial inspection: clean working tree; origin/main matches baseline.
- Build mismatch found: manifest requires Rust 1.100; CI installs 1.99.
- Project Markdown inventory: 16 files outside generated target output.
- Completed implementation, review and test evidence is recorded below; remaining scope stays open.

### Completed verification (Linux development workstation)

- Rust 1.100.0 is installed and pinned; all implementation commits use the feature
  branch, with incremental Conventional Commits and pushes.
- Optimized `cargo test --workspace --profile release-fast --locked -j 2` passed:
  113 core, 84 UI/component, 2 CLI-module, 8 CLI-process, 5 image-process, 7 OCR-process
  and 5 QR-process tests (224 total); 42 hardware/diagnostic tests were ignored.
  OCR coverage includes the real 30-second deadline, process-group interruption
  and blocked final stdout. Follow-up checks after final review repairs are
  recorded below; passing this run does not prove native Windows/macOS behavior.
- Native X11/XWayland synthetic-window test passed:
  `native_probe::native_kit_workspace_and_editor`, 3.14 seconds. Only this
  application's windows and generated pixels were used; source bytes, output
  behavior and isolated settings were checked. This is not Linux capture/portal,
  physical input, mixed-DPI or screen-reader acceptance.
- The real executable Tesseract fixture passed against installed Tesseract 5.5.4
  with installed German language data (`deu`), 0.17 seconds. Default `eng` data is
  absent locally and fails truthfully; no language package was installed.
- `python3 scripts/test-package.py`: 3 passed, including deterministic tar/ZIP,
  checksums, no-overwrite and extracted synthetic-executable smoke.
- `cargo build --release --locked -p screenshots -j 2` passed in 6m 48s with thin
  LTO and limited debug information. The final source-fix rebuild also passed
  (2m 09s). No bit-for-bit rebuild claim is made.
- `cargo deny --format json check advisories`: fails on 9 unmaintained transitive
  packages, listed in goldfish-review-report.md. The SQLite vulnerability found
  during the first audit is absent after the compatible 3.54.1 upgrade. No ignores
  were added. The separate GitHub glib alert remains an upstream-family review.
- Fresh review found and fixed a Windows-only backup sync-handle issue:
  FlushFileBuffers needs write access; the verified staging file is now reopened
  read/write without creation or truncation, closed before publication.

### Scope still open

The full requested backlog is not complete. UI design/interaction work is left
to the user. Recording/audio/GIF, scrolling capture, pinning, reusable workflows/
actions, broader providers/upload types and remaining library/capture/editor
families still require implementation. QR/file-OCR/image utilities are headless
slices, not corresponding editor/region workflows. Windows OCR remains unsupported
until native subprocess isolation is implemented and accepted. Platform signing,
native credential-store runtime, Wayland capture/shortcuts, macOS permissions,
mixed-DPI/topology/sleep/device-loss tests, recovery on physical storage and full
reference-hardware budgets remain open. No unavailable user input or 1Password
access was treated as approval or as a passing test.

### Final local evidence (2026-11-15)

- Final optimized workspace run passed all 224 regular tests, with 42 opt-in
  checks excluded from that run. The later ignored WebP timing fixture was
  compiled across the workspace and explicitly passed on its own; it adds one
  opt-in entry to the current test inventory. No production code changed for it.
- Workspace Clippy with the repository's configured denied lints passed. Existing
  warnings remain visible. Ownership-only test fixes and one save-worker path
  move were needed for the pinned compiler; no UI design/interaction changed.
  Formatting and whitespace checks passed.
- The actual 87 MiB Linux archive passed checksum/extracted-help/version checks
  and ten checks against its real release binary: conversion/rotation exact
  pixels, padding/background, nearest upscaling, Unicode QR roundtrip, installed
  Tesseract/deu OCR, history/integrity/backup/no-overwrite, exact-PNG loopback
  upload, credential-target computation without store access, unchanged originals
  and absent user-state writes. Only generated fixtures/local loopback were used.
- Local archive: `target/non-ui-packages/screenshots-0.1.0-x86_64-unknown-linux-gnu.tar.gz`.
  Archive SHA256: `8a47f70e6af3076c1663a533c41b8de55c1bd7c6eaaeb042ec5dfc49408def47`.
  Binary SHA256: `34aaf04de53955ffb353cce3e07114fb639dfd6596527bdea381a194c41e30a0`.
  Its build-info records explicit prebuilt input and the dirty documentation tree;
  it is a development artifact, not a signed/published production release.
- WebP worker comparison: see webp-admission.md. The checked path's 62.83 ms warm
  median versus 63.06 ms for the trusted former path is inconclusive (five samples),
  not an optimization claim or resource-gate pass.
- Both final Goldfish lenses completed two consecutive clean source-review rounds
  for the implemented slices. This is not full-plan convergence or native runtime
  acceptance. Full report: goldfish-review-report.md.
- Native GitHub CI for Linux x86_64, Windows x86_64 and macOS arm64 is running
  remotely on the source branch. No pending job is recorded as passed. The final
  documentation-only commit uses `[skip ci]` to preserve the source run instead
  of canceling and rebuilding it for prose changes.
- Shutdown is explicitly requested by the user and is the final local action
  after verifying the working tree and pushed remote head. The full backlog is
  not marked complete; its open items above are preserved for continuation.

Source CI run: https://github.com/lejylabs/screenshots/actions/runs/43652730948
(head `20452cdaba963be9162bbfacfdbced3579d7e14c`), still running at local handoff.
The previous source run was canceled by the workflow concurrency rule after the
measurement-test commit; it is not counted as passing.
