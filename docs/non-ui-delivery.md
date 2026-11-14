# Non-UI delivery plan

Author: NewtTheWolf
Date: 2026-11-14
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
- Implementation, review and test results will be appended as completed.
