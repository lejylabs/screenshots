# Non-UI delivery plan

Author: NewtTheWolf
Date: 2026-11-14–15
Status: implementation in progress; no platform or parity acceptance is implied.

## Request and constraints

Implement the outstanding functionality collected in PLAN.md and
docs/gap-analysis.md, without redesigning the UI. Use a feature branch,
Conventional Commits, incremental pushes, fresh EGM reviewers and end-to-end
tests. The user is away and cannot provide passwords, approvals or test input.
Use existing credentials only. The user revoked shutdown on 2026-11-15:
leave the workstation powered on while continuing implementation. Never confuse an unavailable native test with a
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

## Continuation (2026-11-15)

The user requested continued implementation and explicitly revoked shutdown.
`shutdown -c` completed; no scheduled systemd shutdown remains. The previous
source CI completed: Linux passed, Windows failed during packaging, and macOS
failed a storage test. Investigate those actual results before treating the
three-platform delivery as complete.

### Next execution slices

1. Fix Windows packaging UTF-8 decoding explicitly and macOS SQLite no-follow
   behavior for canonical system temporary directories. Preserve final-component
   symlink rejection and live-WAL semantics. Re-run exact failing fixtures and CI.
2. Add a separate read-only history-query API and CLI command: captures/uploads,
   ID ascending/descending keyset pages, UTC date range and bounded text search.
   Do not change existing UI queries or silently create/migrate databases.
3. Add explicit versioned image workflow recipes: at most sixteen in-memory
   crop/rotate/flip/resize/canvas/redaction operations, then one no-overwrite
   output. Recipes cannot invoke processes, read extra files, upload, change
   clipboard/history, or execute code. Exact schemas and fixtures are recorded
   in workflows.md before acceptance.
4. Add explicit stitched-desktop output from one admitted capture group. Combine
   frozen RGBA images on their physical-pixel desktop bounds; reject overlapping
   or contradictory geometry rather than silently producing a wrong mixed-DPI
   image. Transparent gaps stay explicit. Admission checks the source sum and
   final bounding-box bytes before allocating. Synthetic multi-monitor fixtures
   prove negative-origin placement and failure behavior; native topology and
   mixed-DPI acceptance remain separate.

5. Add a finite explicit still-capture sequence, exposed as
   `capture-sequence display ID DIRECTORY --count N --interval-ms MS`.
   Count is 1–1000 and the post-save wait is 100–3,600,000 ms. The directory
   must already exist and be empty; filenames use one invocation UUID and index.
   One image is captured, saved and released before waiting for the next; no
   catch-up backlog, history, clipboard, upload or resident timer. Cancellation
   retains completed files and may leave the ordinary interrupted-export part.
   Create `apps/desktop/src/capture_sequence.rs`,
   `apps/desktop/tests/capture_sequence.rs` and `docs/capture-sequence.md`; modify
   `apps/desktop/src/main.rs`, `apps/desktop/src/cli.rs`,
   `apps/desktop/Cargo.toml` and `Cargo.lock`
   only for module dispatch and reuse of the existing uuid dependency. Native
   capture acceptance is an explicit opt-in test, never part of routine tests.

These slices extend the non-UI surface. No screen recording, native scrolling,
or interactive selection capability is claimed until implemented and tested.
The workstation must remain on; shutdown is no longer authorized.

File assignments for this tranche (repository-relative):

- Modify `scripts/package.py`, `scripts/test-package.py`,
  `scripts/test-package-smoke.py`, `crates/app-core/src/storage.rs`,
  `docs/packaging.md` and `docs/storage-recovery.md` for the CI repairs.
- Create `crates/app-core/src/history_query.rs`,
  `apps/desktop/tests/history_query.rs` and `docs/history-queries.md`.
  Modify `crates/app-core/src/history.rs` to expose the existing FTS parser,
  `crates/app-core/src/lib.rs` for exports and `apps/desktop/src/cli.rs` for routing.
- Create `apps/desktop/src/workflow.rs`, `apps/desktop/tests/workflow.rs` and
  `docs/workflows.md`. Modify `apps/desktop/src/main.rs` for module registration,
  `apps/desktop/src/cli.rs` for routing, `apps/desktop/src/image_tools.rs` for
  shared image helpers and `apps/desktop/Cargo.toml`/`Cargo.lock` for serde.
- Create `crates/app-core/src/desktop_composite.rs` and
  `docs/desktop-composite.md`. Modify `crates/app-core/src/lib.rs` and
  `apps/desktop/src/cli.rs` to expose and route the explicit capture operation.

Existing UI views, capture backends and schema migrations remain unmodified by
these assignments.

History CLI contract: `screenshots history-query captures|uploads DATABASE`
with optional `--order newest|oldest`, `--cursor ID`, `--from YYYY-MM-DD`,
`--through YYYY-MM-DD`, `--search TEXT`, `--source all|captures|imports`.
Reject repeated/unknown flags. Upload queries reject non-all source filters.
Output contains `entries`, `has_more`, `next_cursor`; paths are validated UTF-8,
upload URLs remain inert strings. Entry fields match each public core row type.
The command never performs migration and uses the same explicit error/status
contract as other headless commands. Tests invoke the real binary against
synthetic databases, with no image files or existing user history.

6. Add `screenshots upload-images PROFILE IMAGE...`, an explicit sequential batch
   for 1–20 PNG/JPEG/WebP files. Create `apps/desktop/src/upload_batch.rs`,
   `apps/desktop/tests/upload_batch.rs` and `docs/upload-batch.md`; modify the
   existing desktop main/CLI only for dispatch. Preflight profile/count/regular
   files and encoded extent before any upload, then decode/encode/upload one
   input at a time. Each confirmed result is flushed as NDJSON, so earlier links
   remain visible if a later upload fails. No implicit history, clipboard,
   retries, remote deletion or disk image intermediate. This uses the existing
   uploader/credential safeguards and does not add provider or arbitrary-file
   support. Duplicate inputs explicitly mean repeated uploads.

### Continuation evidence

- Both actual prior CI causes were corrected, with five packaging regressions
  passing and a linked-ancestor live-WAL storage regression added. The Linux
  optimized core run passed 116 tests at that checkpoint. Native rerun results
  remain distinct from the local synthetic regression.
- New HistoryQuery tests passed 7 cases including 10,000 records and cooperative
  cancellation; real executable history query passed 1, image workflow passed 7,
  upload-images passed 5 local-loopback cases. No real remote service or desktop
  capture was used. Later full-workspace evidence follows below.
- The explicit history timing fixture passed: new ID query p95 0.111 ms, existing
  ID query 0.080 ms, FTS 0.215 ms and full no-match upload scan 2.708 ms. These are warm
  development database observations; unchanged UI query paths and the unindexed
  scan deadline remain documented in history-queries.md.
- Every new functional slice and each CI repair was committed and pushed with
  a Conventional Commit. Original unfinished capability families stay open.

- Final continuation workspace run passed all 257 regular tests (45 opt-in tests
  excluded), including the five upload-batch tests; all doctest phases passed.
  Workspace Clippy passed with existing warnings, formatting and whitespace
  checks passed. The earlier overlapping-build doctest artifact failure was
  reproduced as infrastructure churn, not suppressed: the final stable run
  completed successfully. Native capture sequence/stitched acceptance remains
  unrun, because these commands would read the actual desktop.
- Source head CI is running at
  https://github.com/lejylabs/screenshots/actions/runs/43698698480 ; no native
  result is claimed until that run completes. The PC stays on.

### Recording design decision (2026-11-15)

Add an optional Unix file recorder using separately installed `ffmpeg` and
`ffprobe`; do not bundle, download, auto-update or link their binaries/libraries.
The selected on-disk format is Matroska with FFV1 lossless video, no audio,
1–60 nominal frame-count seconds, 1–30 fps and at most 1920×1080 pixels. The X11 input is explicit
and accepts a local display only; `test` input records only FFmpeg's generated
test pattern for E2E validation. Wayland and Windows/macOS native screen capture
remain separate work. This is a bounded recording slice, not complete recording
or GIF/audio support.

This is a local optional-tool integration decision, not a conclusion that a
separate executable exempts distribution obligations. FFmpeg licensing depends
on its build; the installed development binary is GPL-enabled. Product packages
continue to contain no FFmpeg binaries, libraries or language/runtime downloads.
Before distributing a combined FFmpeg bundle, inspect its exact configuration,
corresponding source/license obligations and supported-platform identity. See
https://ffmpeg.org/legal.html . Our source license is unchanged; selecting FFV1
does not change the license of a user's installed FFmpeg build.

Create `apps/desktop/src/record.rs`, `apps/desktop/tests/record.rs` and
`docs/recording.md`; modify desktop main/CLI and add Unix libc as an existing
transitive dependency for child-only resource limits. One encoder/probe process
at a time, 256 MiB child file limit, private staging, bounded diagnostics and
duration+10s overall deadline. Verify exact frame count, codec, dimensions and
container before no-replace publication. No pixels/history/clipboard/network
output beyond the explicitly requested recording. Native screen tests remain
opt-in; actual synthetic FFmpeg recording E2E is run without desktop capture.

Both subprocesses share one absolute deadline (requested seconds plus ten).
X11 accepts only local `:digits[.digits]` DISPLAY names up to 64 bytes; `test`
never reads DISPLAY and uses a fixed lavfi testsrc pattern with no shell or
user-supplied filter expression. Output options force FFV1/Matroska, bounded
threads and frame count, without audio, subtitles or network protocols. Stdout
is null for the encoder; probe stdout is capped at 8 KiB, both stderr streams
at 16 KiB and discarded. Dedicated Unix process groups are killed/reaped on
timeout, interrupt, oversize or error before worker joins. Only after local
ffprobe verifies the exact expected frames/codec/geometry is the file synced
and published without replacement. Probe failure never publishes partial video.

FFmpeg's `-max_alloc 67108864` is a per-allocation bound, not a total memory
limit. The external engine's actual RSS and codec buffers must be measured
separately; no total-process budget pass is inferred from that option. The
requested frame is at most 8 MiB as RGBA and the encoder uses two threads, but
codec/reference/OS buffers are additional. Do not enable a resident recorder.

Recording implementation verification: eight optimized recording process tests
passed, including installed FFmpeg/ffprobe synthetic output. They verify exact
frames, dimensions, format/codec, failure/no-overwrite and cancellation cleanup.
Clippy and formatting pass. Native X11 desktop recording and cross-platform
recording remain unverified. Process-level synthetic memory observation is in
recording.md; no total-memory or physical timing acceptance is claimed.

The post-recording full optimized workspace run passed 265 regular tests with 46
opt-in tests ignored; all doctests passed. The real synthetic recording test was
then explicitly run as part of the recording-focused eight-test pass above.
Clippy passed with the repository's existing warnings. Native CI source run is
https://github.com/lejylabs/screenshots/actions/runs/43699260979 (running at this
checkpoint); prior intermediate runs were superseded, not counted as passes.

The updated thin-LTO release build passed (2m 14s). Its actual portable archive
passed isolated help/version smoke plus real recipe/redaction/resize and real
FFmpeg synthetic-video checks. User state and original image bytes were unchanged.
Artifact: `target/continued-packages/screenshots-0.1.0-x86_64-unknown-linux-gnu.tar.gz`.
Archive SHA256: `541d7425d2fbdbd6103a9386d0d94a664c1ce6b589ad3f1b9b8f14c6a1eb3ca0`.
Binary SHA256: `0a3b2d1bdcb10327dc2a0ce7e8b75627ffd6351cf0146c6d7312f6c3e866ff96`.
This local development artifact records dirty documentation provenance; it is
not a signed production release. The PC remains on, as explicitly requested.
