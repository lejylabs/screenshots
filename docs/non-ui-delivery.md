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
one JSON value to stdout (help/version use plain text), except `upload-images`
which flushes NDJSON progress/results and a final summary. Errors go to stderr and
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

### Scope still open after the first tranche (historical snapshot)

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

### Image collection utilities

Create `apps/desktop/src/image_collection.rs`,
`apps/desktop/tests/image_collection.rs` and `docs/image-collection.md`; wire
`apps/desktop/src/main.rs`/`cli.rs` only. Add explicit horizontal/vertical image
combination (2–16 images), same-dimension pixel comparison and exact grid
splitting (at most 64 tiles). Reuse the bounded decoder, native path handling and
atomic image exporter. Combined source and result pixels each stay within 256 MiB;
split retains one source and one tile, never all decoded tile copies. Failed
multi-file split preserves earlier completed tiles and never overwrites existing
files. These are file utilities, not live scrolling or native screen selection.

Image collection verification: eight actual-executable tests passed for combine,
raw-pixel comparison, alpha, split reassembly, limits and native path handling.
The recorder gained Linux parent-death handling; nine recording tests (including
real synthetic FFmpeg and controller-SIGKILL behavior) pass. The complete
post-collection workspace run is recorded after it finishes. No desktop pixels
or real service credentials were used in these tests.

Latest post-collection optimized workspace verification passed 274 regular tests
with 46 explicit opt-in tests excluded; all doc-test phases passed. Recording
focused tests also passed the real FFmpeg fixture separately. Existing native
platform gates and remote CI remain distinct.

GIF recorder follow-up: extend `apps/desktop/src/record.rs`, its executable tests
and `docs/recording.md` using `.gif` output selection. GIF has stricter limits:
1–30 nominal seconds, 1–15 fps, at most 1280×720 and 450 frames. Use the builtin
GIF encoder with a fixed RGB332 palette, without palette-generation history or
an added frame buffer. Verify GIF header/codec/container/dimensions/frame count
and centisecond-quantized frame timing before publication. Existing FFV1 output,
resource/process controls and no-bundle licensing decision remain unchanged.

Image inspection follow-up: create `apps/desktop/src/image_info.rs`,
`apps/desktop/tests/image_info.rs` and `docs/image-info.md`; route `image info`
and `image pixel` through the existing headless dispatch. Decode with existing
format/size/EXIF rules; return only dimensions, observed encoded size, decoded
RGBA size or the selected raw RGBA sample. No hidden clipboard writes or color
management claim. Invalid coordinates fail, and source pixels are released
before writing the bounded JSON result.

The GIF-focused real-engine run passed all 13 recording fixtures, including
centisecond timing and odd dimensions. Image information/color sampling passed
five executable fixtures. The latest full optimized workspace run passed 282
regular tests with 47 opt-in tests excluded; all doctests passed. The subsequent
small parent-death setup-race fix passed the full 13 recording fixtures again.

### Explicit generic-file sharing

Create `apps/desktop/src/upload_file.rs`, `apps/desktop/tests/upload_file.rs` and
`docs/file-uploads.md`; extend `crates/app-core/src/upload.rs` with a separate
generic-file entrypoint and expose it through `lib.rs` and CLI. Keep PNG upload
validation unchanged. Generic input is one existing absolute regular file,
streamed through the existing single upload job with a bounded 64 MiB extent.
Use application/octet-stream and generic file.bin multipart naming; preserve
HTTPS/credential/redirect/timeout rules. No directory traversal, archive
extraction, filename disclosure or JSON/base64 generic upload in this slice.
Real process tests use local loopback and exact arbitrary binary bytes.

Generic-file upload verification: four loopback executable tests passed (binary
and empty PUT, multipart octet-stream filename/privacy, oversize/directory/JSON
rejection, named-pipe preflight); Clippy passed with existing warnings. Output
write/flush failures explicitly report that the uploaded remote file persists.
The new entrypoint shares the original PNG transport and does not relax PNG
signature checks in existing commands.

After generic-file sharing, the final local optimized workspace run passed 286
regular tests with 47 opt-in cases ignored and all doctest phases passing.
Clippy and formatting passed. Current source/CI state is on the feature branch;
CI jobs remain separate native evidence and are not marked successful while
pending. PC shutdown remains revoked.

### Exact scroll-image assembly

Create `apps/desktop/src/scroll_stitch.rs`,
`apps/desktop/tests/scroll_stitch.rs` and `docs/scroll-stitch.md`; wire main/CLI.
`stitch-scroll OUTPUT MIN_OVERLAP INPUT INPUT...` accepts 2–16 already captured
images with equal widths and unique exact row overlaps. It must reject absent
or ambiguous seams instead of inventing content. All sources fit the existing
256 MiB combined decoded limit, output fits 256 MiB, and exact seam verification
has a separate 256 MiB byte-work cap. Detect all seams before allocating/output.
No live scrolling, input injection, network or history is performed. Native
scroll capture remains open rather than being claimed by an assembly helper.

Exact scroll-image assembly passed six executable fixtures and two algorithm
fixtures, including a raw-overlap oracle and collision verification. Clippy
passed. The output joins supplied images only; the unimplemented native scroll
controller and application-specific scrolling behavior remain open.

Post-scroll-assembly full optimized workspace verification passed 294 regular
tests with 47 opt-in cases excluded; all doctest phases passed. PC remains on.

### Explicit reference management

Added `history-manage import`, `remove` and `relink` through existing atomic
metadata operations. Remove/relink cannot create a missing database and reject
final database symlinks; lookup is bounded and read-only. No image file is
deleted, moved or rewritten. Six actual-executable fixtures and eight existing
core history regressions passed. Clippy passed after a test-only ownership fix.
Details and partial-import/optimistic-update limits: history-management.md.

Final local full-workspace verification after metadata management passed 300
regular tests with 47 opt-in checks excluded, including all doctest phases.
The latest native CI run is still pending/running separately; no platform
acceptance is inferred from these Linux results. The workstation stays on.

## Reconciled checklist follow-up

The supplied checklist targets `main @ 523bc3c`. Current feature work already
adds CLI-only file/batch uploads, image utilities, exact scroll-image assembly,
history filters/reference management, bounded recipes and optional Unix video/GIF.
These do not complete their corresponding editor/native workflows. Preserve
that distinction rather than checking off whole feature families.

Next concrete changes: create `apps/desktop/src/hash_file.rs`,
`apps/desktop/tests/hash_file.rs` and `docs/file-hashes.md` for bounded SHA256
hash/check, and add explicit sanitized PNG history export through the existing
core exporter plus `history-manage export DATABASE DIRECTORY POLICY ID...`.
`POLICY` is `original` or `strip-metadata`; no implicit change to GUI/export
defaults. Stripped export re-encodes pixels only, preserving source files and
history. At most20 IDs, one decoded image at a time, existing atomic no-overwrite
publication and partial-failure reporting. No new idle activity.

### Checklist reconciliation (2026-11-15, next tranche)

The user supplied a main-523bc3c checklist. A fresh EGM codebase reviewer compared
that baseline to 500ee38 and identified CLI-only progress separately from UI and
native acceptance. The current gap-analysis introduction now supersedes old
historical table claims. The central output contract explicitly exempts the
NDJSON upload batch from the one-JSON-result rule.

New concrete work adds explicit hash checking, missing-reference inspection and
metadata-policy exports. Hash and history-audit commands do not mutate user
files or application state; metadata-stripped export is opt-in and does not
change GUI defaults. The shared loader and byte-copy exporter now reject
nonregular inputs, including FIFO paths, before blocking reads; Unix nonblocking
open closes the special-file replacement window. These changes are being
verified separately from the earlier 300-test baseline.

### Latest checklist tranche verification

- Optimized full workspace: 318 passed, 47 opt-in checks ignored, all doctests
  passed. Hash executable fixtures: 7; missing-reference audit: 5; history manage
  fixtures: 7. Core export tests pass for source preservation, metadata stripping,
  no overwrite and cancellation before publication. Clippy passed with existing
  warnings; formatting and whitespace checks passed.
- Synthetic 4K export comparison: original-byte export median 2.10 ms/RSS 14.63 MiB;
  stripped-pixel export median 13.30 ms/RSS 47.20 MiB. This is extra decoding work,
  not a hardware acceptance claim.
- Actual previous CI logs show test-binary compilation was still running when
  the run was superseded; no test execution had begun. Do not call this a runtime
  deadlock. Native application builds passed on Windows/macOS/Linux in that run,
  but tests/package gates were canceled. Deterministic CI tests now use the
  existing optimized release-fast profile; the packaged binary remains thin-LTO
  release and is smoke-tested after extraction.

### Bounded PNG metadata inventory

Create `apps/desktop/src/png_metadata.rs`, its executable fixtures and
`docs/png-metadata.md`, and route `image metadata INPUT` through the CLI. Inspect
PNG structure/CRC and chunk types/lengths, never print ancillary payloads or
decompress metadata. Input is one regular file at most 32 MiB with at most 4096
chunks. Report dimensions/bit depth/color type and metadata/animation presence.
This is PNG inventory, not a general EXIF/ICC editor or full decoder validation.
Reuse the existing transitive CRC dependency and do no disk or clipboard writes.

PNG inventory verification: five real-executable fixtures and two parser/input
unit fixtures passed, as did workspace Clippy. A fresh EGM reviewer found no
functional blocker; exact PNG/APNG acceptance rules and CLI output semantics
were expanded in the documentation. Payload values are never emitted.

Latest PNG-inventory full-workspace verification: 325 regular tests passed,
47 opt-in tests excluded, all doctest phases passed. Clippy, formatting and
whitespace checks passed. Five packaging tests passed; all 39 project Markdown
files had valid local links. Release archive smoke and real release-binary
hash/export/missing-reference checks passed with unchanged user state. Native
CI remains a separate outstanding gate; no runtime acceptance is inferred.

### Windows OCR continuation

Implement optional installed-Tesseract OCR for Windows without changing the UI
or Unix path. Create `apps/desktop/src/ocr_windows.rs`,
`apps/desktop/tests/ocr_windows.rs` and `docs/windows-ocr.md`; modify shared OCR
entrypoint cfgs, desktop main and target-specific windows-sys features only.
The child starts suspended, enters a kill-on-close Job Object, then its one
initial thread is resumed after ownership validation. Job-assignment or resume
failure must terminate the still-suspended child; no uncontained engine starts.
Three bounded pipe workers retain existing output/input caps and 30-second
operation deadline. Job termination precedes pipe joins; close kills descendants
on controller exit. Real Windows process and console behavior needs native CI
evidence and separate user-session acceptance. No auto-install or credential use.

Windows OCR implementation is now present. An independent codebase-detail
review found no unresolved blocker/major after fixing shared documentation and
making the absent-engine fixture independent of Windows executable search paths.
The entire desktop app and tests passed GNU Windows cross-check; the isolated
optimized lifecycle binary passed three tests under Wine (one internal fixture
ignored), including descendant pipe cleanup on timeout. This is emulated evidence,
not native Windows or real-engine acceptance. The small parent-death window before
job assignment remains documented; no engine code runs before assignment.

### One-shot scheduled capture

The existing finite `capture-sequence` command now accepts `--start-at` with a
known RFC 3339 offset, nanosecond precision and a future horizon of at most seven
days. It sleeps once against a monotonic deadline before constructing the native
backend, then rechecks the destination. Immediate invocation/output stays
compatible. No service, persisted schedule, machine wake-up or idle timer is added.
Eleven optimized unit tests and seven executable tests passed, including a real
two-second wait/revalidation and termination during a future wait; native
capture remained excluded. The separate codebase-detail review found no actionable
finding. See [the exact contract](capture-sequence.md).

### Additional opt-in verification

Previously ignored loopback upload tests exposed three test bugs: whole multipart
requests containing binary PNG data were decoded as UTF-8. Assertions now inspect
header/text byte ranges. The six focused wire tests passed after correction;
production upload code is unchanged. CI now explicitly runs the loopback-only
response, body, header, cancellation and probe fixtures, keeping real credential
and native capture checks out of that automatic step.

Full optimized workspace verification after OCR/scheduling passed 333 regular
tests with 47 opt-in tests excluded and all doctests successful. The two additional
scheduled-process tests passed separately afterward (335 regular tests in total).
The full opt-in upload filter passed 18 tests, including a unique synthetic Linux
Secret Service roundtrip and a missing-credential/no-network check. The synthetic
entry was removed; no existing account credential was accessed. Clippy, formatting,
whitespace and five packaging unit checks passed; 39 Markdown files had no broken
local links. The Linux thin-LTO release rebuild passed in 2m 30s. Its 87 MiB portable archive
passed checksum/extraction/help/version smoke checks, and the actual release
binary passed hash/PNG-inventory/conversion/scheduled-revalidation checks with
unchanged source and user state. Archive SHA256:
`aacc833434d2ec3de464790bfda0c4dff815a4f5466e9bdcadfbfa58b1fe19c3`;
binary SHA256:
`0d310fc83ed24ff3908cdca9520514236dcb8c61d7fb1241d36e2673659a1635`.
It predates the next direct-ShareX slice below; native CI remains separate.

### Direct ShareX profiles in headless uploads

Next slice: reuse the existing documented `.sxcu` importer directly from the
three explicit upload commands. Add `load_upload_profile` in desktop cli.rs;
case-insensitive `.sxcu` selects that importer, other suffixes retain native JSON.
Wire upload, upload_batch.rs and upload_file.rs; credential commands stay JSON.
The existing ImageUploader-only subset and validation remain unchanged. Share
one bounded regular-file reader in core upload.rs for the two profile formats,
reject special files before reading, close the Unix replacement-to-FIFO race with
nonblocking open, recheck the opened file and retain the 16 KiB byte limit. Keep
symlink-following compatibility and reject growing oversized files. Tests cover
both parsers/extension cases, supported loopback upload, preserved files/state,
invalid/secret-bearing profiles and special files. No new dependency, background
work or implicit persistence; this is direct profile use, not full ShareX parity.


Installed-engine regression after the direct-profile work began: the isolated
Unix Tesseract fixture passed again with synthetic printed `HELLO 123` and
installed `deu` data. No desktop, remote provider or real document was used.

### glib FFI backport

The remaining GitHub glib advisory describes VariantStrIter passing an immutable
Rust reference to C as an output pointer. Preserve the GTK3 dependency family and
vendor exactly glib 0.18.5, applying upstream commit
b5a4071e439bef2b5eea76c3aa25e5ae84839e34: make the pointer local mutable and pass
`&mut p`. Add a patch entry in workspace Cargo.toml and a provenance note in the
vendored source; run upstream iterator unit tests optimized plus normal affected
workspace checks. Do not invent a patched release version or suppress advisories.
The version-based GitHub alert remains open until the dependency family can be
upgraded. This backport adds source files only, no runtime allocation or idle work.


Direct-profile verification passed: three regular CLI parser/rejection fixtures
and one explicitly enabled Linux TLS fixture, including eight real loopback HTTPS
uploads across `.sxcu`/`.SXCU`, PNG, two-image batches and arbitrary files. Exact
payloads, returned links/NDJSON, unchanged inputs/profiles and absent user state
were checked. Existing image-batch (5), file-upload (4) and CLI upload (2) fixtures
also passed. Three new core profile-reader tests passed exact byte limits,
regular-file/symlink behavior and timeout-safe FIFO rejection. Workspace Clippy
passed before the subsequent glib source override. No external service was used.


The glib backport passed all eleven upstream iterator tests in an optimized
isolated test copy and a workspace integration regression covering borrowed
Unicode/empty strings in both directions. The copied test manifest omitted
benchmark/compile-test-only dependencies unavailable locally; shipped source and
manifest retain upstream contents. The vendored source differs only by the
upstream two-line pointer fix plus its provenance note. `cargo deny` still fails
on nine unmaintained transitive packages, and the remote glib version alert is
not marked resolved. Full post-backport verification is recorded below once done.

### Capture-sequence output formats

Next bounded extension: allow explicit `--format png|jpeg|webp` and JPEG quality
on finite sequences, reusing core image export. Omitted flags retain PNG files
and existing JSON. Validate both options before any scheduled wait or native
initialization; derive filenames from the chosen format, keep no-overwrite and
one-frame-at-a-time publication, and reject transparent JPEG through existing
policy. Synthetic core/export fixtures and CLI rejection tests cover this path;
no real screen is captured during automated checks. No new encoder, persisted
setting, worker, stream or idle timer is added.

### Remove obsolete PEM-parser dependency

After the glib backport, migrate only gpui-pre-reqwest 0.12.15's three TLS PEM
parsing sites to the already-resolved rustls-pki-types PEM API. Preserve certificate
and CRL filtering and identity handling (certificate order, private-key formats,
invalid/missing sections, rejection of recognized unrelated sections). Vendor the
exact crate with a patch provenance note; remove rustls-pemfile from its manifest
and enable pki-types for the shared TLS feature. Verify public TLS parsing with
existing/synthetic fixtures and the workspace dependency graph. No TLS verification
bypass, new backend or trust-store mutation is allowed. The objective is removing
one genuinely unused/unmaintained graph node, not suppressing its advisory.

Capture-sequence format verification passed 15 optimized unit tests and eight
CLI tests; one native capture fixture remained excluded. Synthetic fixtures
verify exact default PNG bytes, lossless WebP alpha/pixels, JPEG dimensions and
quality reaching the encoder, transparency refusal, collisions and frame release.

The complete post-glib/direct-ShareX workspace run passed 348 regular tests,
48 opt-in cases excluded, all doctests passed. This run included updated source
present during build; focused format tests provide the separately reported exact
new-format coverage. Post-glib Clippy and the GNU Windows whole-app/test cross-check
passed. No additional native runtime acceptance is inferred from these checks.


PEM migration verification: nine Rustls-only and four native-default TLS tests
passed optimized in an isolated test copy. Tests cover certificate/CRL filters,
order/last-key semantics, invalid sections and retained error classes. The entire
Windows GNU application/test check passed. `rustls-pemfile` is absent from the
all-target dependency graph; `cargo deny` now reports eight unmaintained-package
errors, down from nine. No advisory suppression or TLS trust bypass was added.

Final focused-tranche check: the workspace passed again after the PEM migration
and capture format changes: 348 regular tests, 48 opt-in cases excluded, all
doctests passed. Clippy and Windows GNU application/tests check passed. Packaging
logic passed five tests; project Markdown links remained valid. The pending
release rebuild and native CI are separate from this source/test result.

The final source-tranche Linux release rebuilt successfully in 2m 29s. Archive
checksum/extraction/help/version checks passed, plus actual release-binary QR,
PNG inventory, direct uppercase ShareX profile dispatch, and all three scheduled
output-format rejection/revalidation paths with no native capture or user-state
writes. Archive SHA256:
`0c6648210b52367e82fba7e95256d85d7cd9a3cd4ea46d6683a48f2ba322b3d8`;
binary SHA256:
`5120f26aa81c64535aaa9ed01c219a2603c44db4ad58955a25b0b150094363fc`.
Native CI remains a separate pending check.

### Linux X11 window capability

Next slice: replace unconditional Linux window-disable flags with X11 availability
from a nonempty, valid DISPLAY value. Xcap's window enumeration/capture are X11
operations; this supports X11 and XWayland clients, not native Wayland windows.
Use existing worker/admission, PID/minimized checks and returned-pixel bounds.
No eager server probe, environment mutation, persistent connection or polling.
Add pure capability fixtures and a deliberately invoked native test that reads
only an explicitly supplied 96x64 synthetic external Xlib window into RAM. Test
self-PID, minimized and closed-window refusals using only that fixture's state.
Never modify desktop-wide window lists or save the captured frame. Surface actual
server/EWMH failures, and document that DISPLAY alone does not prove availability.

Linux window implementation passed the 11-test capture group and its separate
native XWayland fixture (0.08 s). The fixture verified exact 96x64 pixels, source
ID, no disk output and rejection of self/minimized/closed states. Review found
XCap's incorrect/overflow-prone monitor association; Linux now skips it entirely
and returns unknown display metadata while retaining window_id. Enumeration's
1,024-ID/current-pointer-X-screen limits are documented. Full optimized workspace
verification passed 349 regular tests with 49 opt-in cases excluded; native
fixture execution is separate. Other session/hardware gates remain open.

### Continued dependency and workflow cleanup

Replace `instant` with already-resolved `web-time` in vendored gpui-base and
component 0.9.1. Both are exact std::time types on the supported native targets;
change only crate-qualified imports/types and the two dependency declarations.
Preserve features, application behavior and idle timers; verify native type/
time semantics and compile checks rather than claim new resource performance.
Wasm build support remains a separate check if its target/dependencies are absent.

Extend the existing bounded offline recipe with region blur/pixelate operations
using exported core rendering and PixelationGrid, applied to current flattened
pixels in step order. Keep fixed existing algorithms, one current/output image,
16-step/64-KiB recipe bounds and explicit final export. Document scratch costs,
alpha/edge behavior and redaction irreversibility; reject invalid rectangles.
Harden recipe open with the existing Unix nonblocking regular-file pattern.
No new effect engine, UI workflow, runtime, idle work or network is introduced.

The web-time migration passed 34 optimized isolated tests (UndoHistory, optional
speech timing math and native std type identity), full Linux all-target checks,
Clippy and Windows GNU all-target checks. The source diff is restricted to the
21 crate-qualified path substitutions and two manifests, plus provenance notes.
Wasm compilation was attempted but the target is not installed; it is not marked
passed. `instant` is absent from the all-target lock graph and `cargo deny` now
has seven unmaintained errors, down from eight. No ignore or new runtime was added.


Workflow effects passed ten real-executable integration fixtures plus the
killable FIFO replacement unit test. Independent alpha-aware oracles cover
ordered blur/pixelation; two different secret inputs yield identical outputs
after redaction and later effects. Source preservation, strict schema, bounds
and no-overwrite checks pass. The codebase reviewer found no production defect;
the example's step count was corrected to six. Full combined checks follow.

### Macro diagnostic dependency migration

Replace the abandoned proc-macro-error 1.x used by glib-macros 0.18.5 and
GTK3-macros 0.18.2 with the maintained proc-macro-error3 3.1.1, using its actual
namespace, default features disabled and syn2-error enabled. Vendor those exact
small macro crates, change only dependency/namespace references, and keep stable
Rust 1.100 diagnostic behavior. Do not substitute proc-macro-error2 (also
unmaintained) or enable nightly-specific diagnostics. Verify positive GLib/GTK
expansions plus malformed derive/template/closure inputs that must fail with
useful errors rather than panic. Confirm the old graph nodes disappear and
all application checks still pass. Compile-time dependency only; no runtime cost.


Macro helper verification: valid GLib Enum/Variant/Properties roundtrips and a
GTK CompositeTemplate file definition passed. Six intentionally invalid enum,
closure and template forms failed with their expected primary-span diagnostics,
without macro panics. Source review confirmed stable abort/cleanup semantics.
The older helper and Syn 1 are absent from the graph, reducing unmaintained deny
errors from seven to six. Application-wide compilation remains a separate gate.

### Token-pasting helper cleanup

Replace the abandoned `paste` macro dependency with maintained `pastey` 0.2.3
under the existing dependency key in GPUI Component and an exact vendored rav1e
0.8.1. Preserve all source, SIMD/codec/default features and generated symbols;
do not remove AVIF from the graph to hide the advisory. Keep rav1e license and
patent notices. Verify component macro output, real AVIF encoding and separately
enabled x86 assembly compilation. This is a build-time alias only; no new runtime,
resource loop or codec behavior is requested. Confirm graph/advisory removal.


The complete macro-helper integration passed 354 regular tests with 49 opt-in
cases excluded, all doctests passed, plus Clippy and Windows GNU all-target check.

Pastey verification: real AVIF baseline, patched scalar and patched NASM builds
produced identical 306-byte files (SHA256
`3e99925ee848e6fe8c89a6e5dcc7903d315365b61953c8a86ca379d80d3c90a0`).
Independent ffprobe/ffmpeg checks confirmed AV1 16x12 and identical decoded RGB.
Actual component color macros passed generated-name/value/fallback checks. Source
and feature comparison found no encoder/assembly changes. `paste` is absent
from the all-target graph; five unmaintained-package errors remain. ARM/native
UI acceptance and full combined post-alias testing are separate gates.
