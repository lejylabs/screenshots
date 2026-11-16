# Non-UI delivery: current checklist

Date: 2026-11-16
Scope: the user's complete major-feature list, with final GPUI redesign and new
UI controls excluded. The workstation stays on. The full backlog is **open**.

Snapshot: branch `feat/non-ui-delivery`, committed implementation HEAD `61986a1`.
Committed follow-ups include native defaults/templates and process isolation,
Library lifecycle/reservations, bounded image cancellation, modern SXCU execution,
shared encoded-input decoding and complete private-runtime/native packaging.
Atomic Windows worker startup, durable schedules, real in-memory Wayland
capture and explicit URL image input are also committed. The editor document
backend, remaining macOS native paths and clipboard/import/output integration
are in progress.
Delivered backend paths do not close excluded UI or real native acceptance gates.

The earlier integrated snapshot passed **786 optimized tests**, with 62
explicit opt-ins excluded across 57 outer suites. Clippy passed configured denied
lints, Windows GNU dev app/tests metadata passed, and default advisories passed
without ignores. Earlier whole-suite totals remain historical snapshots.
Focused proof includes 26 SXCU/process tests, 134 helper assertions, 94 actual
HTTPS requests, private native capture/portal/scheduling fixtures, and actual
ABI-bearing extracted archive/.deb checks; scopes and skipped gates are separate
in each contract. The host-dependent UI fixtures are corrected without changing
production capability gating. Current editor/macOS/clipboard/DPI source has focused
checks and independent review in its contracts; it has not yet passed a new
complete integrated run or been committed as an accepted release.

CI `8fdbc78` found unformatted snapshot code and a nanosecond-clock temp-name
collision on macOS. `3882c35` fixes both; exact-head native CI remains a separate
gate. Newer local source still needs its reviewed commit and native matrix.
Later native CI corrected BSD loopback socket modes, scheduler lock teardown,
Windows path-spelling deduplication and portable test disconnect handling.
Current partial native SDK/private-process evidence remains distinct from screen,
permission, input, hardware and release-signature acceptance.
Default active-graph advisory checking passes without ignores. The workstation
remains powered on; no shutdown will be performed.

## Immediate verification state

- [x] Final committed GIF tree: 486 regular optimized workspace tests,
  51 opt-in cases excluded, all doctests passed; Clippy, Windows GNU app/test
  compilation, release FFmpeg decoding and package smoke passed.
  [Recorded results](non-ui-delivery.md#offline-gif-assembly-in-progress-2026-11-15).
- [x] TIFF input: six initial core fixtures, three executable
  fixtures and independent FFmpeg checks passed. The added seventh core fixture
  checks exact caller-budget forwarding and the PNG-only guard. Final Clippy
  and Windows GNU checks passed. The release built in 2m26s; four endian/color
  variants, 25 malformed refusals, exact alpha/hidden RGB, info/pixel and QR
  checks passed. Release binary SHA256:
  `31929f554c6646a146da591d888ab7a5c9772465b47af5b60825d2640b93925a`.
  Package smoke passed; archive SHA256:
  `7632d806df44ea02e73f1866fe3a20c887d95b3e48fafd10f8632be3eb83e6ab`.
- [x] TIFF final complete optimized workspace run passed 496 regular tests,
  including all seven TIFF core fixtures, with 51 opt-in cases excluded and
  all doctests passing. Implementation commit `bc37362` is pushed.
  [TIFF contract](tiff-input.md), [current evidence](non-ui-delivery.md#bounded-truecolor-tiff-input-in-progress-2026-11-15).
- [x] Link-result formatting/QR: final rerun passed 508 regular optimized tests,
  51 opt-in cases excluded, all doctests passed; final Clippy, formatting and
  Windows GNU test compilation passed. The release passed independent
  CommonMark/HTML href and label checks, QR decoding and package smoke.
  The earlier dense-QR scanner failure is disclosed in the
  [delivery evidence](non-ui-delivery.md); it was not counted as a pass.
- [x] Thumbnail cancellation stops obsolete reduction work,
  retains the active job's admission until completion, and makes both Quit paths
  wait for that completion. This repairs control flow without changing UI
  design or adding controls. Final regression passed 515 regular tests,
  52 opt-in cases excluded, all doctests; focused lifecycle, performance,
  Clippy, Windows compilation and release/package checks passed.
  [Contract](image-cancellation.md).
- [x] Saved-PNG admission now checks regular files and a 272 MiB readable extent,
  plus native/RGBA byte limits before decoding. Compatibility/FIFO cases and
  release/package checks passed; the complete run passed 523 regular tests and
  all doctests, with 53 opt-in cases excluded. The new timing-only opt-in test
  also passed separately. [Contract](saved-png-admission.md).
- [ ] Exact-head native CI remains an acceptance gate; read the current run
  separately from historical local evidence above.
- [x] Maintained native font/SVG and D-Bus families: active production graph
  passes default `cargo deny check advisories` without ignores. Actual private
  D-Bus AccessKit activation/actions/update/reactivation, SVG/text and Windows
  GNU compilation checks passed; native screen-reader/keyring acceptance remains.
- [x] Library lifecycle/reservations: cross-volume transfer, journaled trash,
  restore/purge, explicit retention and bounded identity indexing are committed.
  Four fresh source reviews fixed reservation, alias, recovery durability and
  query-plan findings. Round four had no Major; 66 history, seven storage and ten
  CLI tests plus real Linux fault E2E passed. Native Windows/macOS durability and
  UI bindings remain separate. [Contract](history-lifecycle.md).
- [x] New committed foundations: [Open With](open-with.md),
  [native/user installs and updates](desktop-integration.md),
  [Task execution](tasks.md), [native hotkey/IPC host](task-host.md),
  [clipboard bridge](task-clipboard.md), [audio](audio-recording.md),
  [live scrolling](scroll-capture.md) and [provider/OAuth contracts](sharing-completion.md).
  Account, platform, editor/controls and resource limits remain in those contracts.

## Delivered CLI/core subsets

Existing baseline editor and desktop features remain available; this table
separates new headless/core coverage from the remaining workflows below.

| Family | Existing bounded implementation and evidence |
| --- | --- |
| Everyday capture | Windows primary/chosen/active display, selected/active window, exact/repeat region; six configurable capture bindings; saved 1–60-second UI countdown, output profiles and per-task overrides; up to 16 named rectangles requiring explicit monitor choice. [PLAN implementation details](../PLAN.md#session-handoff--start-here-2026-11-12), [hotkeys](hotkeys.md). |
| Multi-output and timing | Windows stitched desktop CLI at 100% scale; `capture-regions` takes 1–16 crops from one snapshot per selected display; finite display sequences, explicit one-shot start time, PNG/JPEG/WebP and optional lossy WebP. `capture-task` composes display/window/region → recipe → final save → optional explicit upload. [Desktop](desktop-composite.md), [regions](capture-regions.md), [sequences](capture-sequence.md), [tasks](capture-task.md). |
| Image tools | Conversion/rotation/flips/downscale, exact nearest-neighbour upscaling, padding/background/watermark, combine/compare/split, exact row/column removal, image dimensions/pixel sampling and SHA256 hash/check. [Tools](image-tools.md), [collections](image-collection.md), [strip removal](strip-removal.md), [info](image-info.md), [hash](file-hashes.md). |
| Image formats | Shared PNG/JPEG/WebP and narrow truecolor BMP/classic uncompressed RGB8 or straight-RGBA8 TIFF input; PNG/JPEG/lossless WebP outputs, explicit lossy WebP, CLI-only BMP and straight-alpha TIFF output. GIF assembly is play-once, 2–16 images, binary alpha with unsafe/full palette rejection. [BMP input](bmp-input.md), [BMP output](bmp-export.md), [TIFF input](tiff-input.md), [TIFF output](tiff-export.md), [lossy WebP](lossy-webp.md), [GIF](gif-assembly.md). |
| Shared editing foundation | Crop, drawing/text/pen/number/highlight/spotlight, blur/pixelation/redaction, bounded metadata undo, selection/movement/resize/duplicate/drawing order, and rotation/flips before annotations. Text has multiline, discrete sizes and Bold/Italic; monochrome text emoji are not inserted emoji objects. [PLAN editor details](../PLAN.md#phase-4--image-editor). |
| Sharing | Generic bounded POST/PUT, image and arbitrary-file sequential batches, named profiles and per-task destination defaults, explicit finalized-pixel upload, supported `.sxcu` subset, bounded JSON/text/header/regex/XML results and thumbnail/deletion links. S3-compatible signed create-only single-object PUT exists, including R2-style endpoint/region configuration. Standalone `link` formats one upload-result object as plain/Markdown/HTML or saves its URL as QR; it performs no upload or clipboard action. [Image batches](upload-batch.md), [file uploads](file-uploads.md), [S3](s3-uploads.md), [link results](upload-results.md). |
| Privacy and library | Pixel-only edited exports/uploads, opt-in PNG metadata stripping, bounded PNG/JPEG/WebP structural inventory; capture/upload date/order/text queries, missing-reference audit, import/relink/metadata removal/export, portable journaled moves/transfers and trash/restore/purge with explicit finish/cancel recovery and retention. Existing UI has visible-page thumbnails and page-local actions. [Metadata](image-metadata.md), [PNG inventory](png-metadata.md), [sanitized export](sanitized-export.md), [queries](history-queries.md), [audit](history-audit.md), [management](history-management.md), [moves](history-file-moves.md). |
| OCR, QR and workflows | File QR generation/scanning, optional installed-Tesseract file OCR on Unix/Windows, CLI region OCR/QR using in-memory pixels under strict session/scale limits, exact supplied-image scroll assembly, and bounded nine-operation offline recipes. [QR](qr.md), [OCR](ocr.md), [Windows OCR](windows-ocr.md), [region analysis](region-analysis.md), [scroll assembly](scroll-stitch.md), [recipes](workflows.md). |
| Recording | Optional installed FFmpeg/ffprobe: finite Unix X11 or Windows gdigrab video/GIF, synthetic mode, cursor show/hide, output verification and bounded process/staging cleanup. This is neither audio nor a full recording workflow. [Unix recorder](recording.md), [Windows recorder](recording-windows.md). |
| Desktop, storage, release | Explicit Linux/Windows/macOS autostart management; Windows resident/tray/Show-only single-instance lifecycle; native secret adapters and CLI setup; SQLite read-only check, WAL-aware backup to a new path and bounded unfinished-file inspection; pinned Rust/lockfile, native CI matrix, portable archives/checksums/notices and manual update instructions. [Autostart](autostart.md), [Windows](autostart-windows.md), [macOS](autostart-macos.md), [credentials](upload-authentication.md), [storage](storage-recovery.md), [packaging](packaging.md). |

## Remaining implementation

These are actionable gaps, not unavailable-test excuses. Implement bounded core,
CLI or native services without adding the excluded UI controls. UI-only workflow
completion is listed separately.

- [ ] **Capture completion:** complete missing native macOS cursor/policy paths,
  calibrated mixed-DPI/rotation composition, native topology/lifetime handling,
  and HDR/wide-gamut conversion/capability correctness. Existing strict refusal
  of unsupported layouts does not implement their conversion. Defaults,
  arbitrary Task hotkeys, persistent monitor templates and ordered after-capture
  actions are now implemented foundations; precision selection/new controls stay
  in the excluded UI section.
- [x] **Timing core/service:** durable/repeating schedules, explicit missed/wake
  policy, uncertain-run crash recovery and typed Task source sequences are
  committed. The service is explicit and event driven; real native timer and
  hardware-wake acceptance remains in the separate matrix.
- [ ] **Editor data model and image operations:** transform existing annotations,
  arbitrary effects/drawing order with opaque-redaction safety, richer fonts,
  color/text styles and tool/export presets; bubbles, inserted image/logo/emoji
  objects, magnifier, clone/eraser and rotation. Backend work is in progress;
  flattened CLI canvas/upscale/watermark alone does not implement these objects.
- [ ] **Import/output:** explicit URL CLI/Task input is committed; native raw
  image/URL drops, bounded non-Windows paste, print and drag-out integration.
  Open With/file associations are implemented. Capture/Library format preference,
  broader codec variants and color-managed import remain distinct work.
- [ ] **Sharing:** reconcile very old SXCU importer/version compatibility and
  remaining workflow inputs/link templates/shortening. Modern pinned-version
  methods/bodies/templates/parsers, HTTP/S3/SFTP/FTP, public-client OAuth,
  image/file batches and explicit Task Link/QR/clipboard chains are implemented.
  Real accounts/native credential denial/rotation and provider-specific optional
  multipart/STS behavior remain explicit provider limits. No blanket arbitrary-
  version SXCU parity is claimed. [Runtime](sharex-runtime.md),
  [providers](sharing-completion.md), [tasks](tasks.md).
- [ ] **Library/storage:** finish exact native lifecycle/fault acceptance and any
  richer metadata/OCR/substring search. Cross-page core selection/actions,
  missing-reference removal, portable move/transfer, journaled trash/restore/
  purge and explicit retention are implemented. Thumbnail prefetch/cache/filter
  controls are UI consumer work, with no speculative idle cache added.
- [ ] **Major native capabilities/utilities:** finish macOS/Wayland scrolling and
  recording adapters beyond implemented X11/Windows live scrolling and optional
  finite video/GIF/native audio/combined A/V; native screen color picker,
  ruler/measurement and bounded clipboard viewer. Existing file pixel/hash and
  combine/compare/split tools remain completed subsets. Pin windows and new
  recording/OCR/QR interaction controls are excluded UI work.
- [ ] **Desktop/release:** atomic startup across optional Windows workers is
  committed; finish real signing/notarization, standard-user install/upgrade/uninstall,
  permissions/IME/accessibility, physical hotkeys/tray/clipboard and failure/
  performance matrices. User installs, native package layouts, notifications,
  associations and authenticated explicit updates/rollback exist; all five
  requested target builds are in CI. Preserve exact source/artifact provenance,
  pending cleanup and unavailable/full-volume recovery; do not hide failures.
- [x] **Dependency maintenance:** maintained font/SVG and D-Bus active families
  and focused regression coverage are implemented. Recheck the exact final graph;
  dormant upstream optional-backend lock entries and the default-branch GLib alert
  remain separate facts. [Current scope](dependency-maintenance.md).

Sources for these families are the [current gap reconciliation](gap-analysis.md#current-reconciliation-feature-branch-2026-11-15),
its detailed [capture](gap-analysis.md#1-everyday-capture-and-task-configuration),
[editor](gap-analysis.md#2-existing-images-editing-and-output),
[sharing](gap-analysis.md#3-sharing-and-uploader-compatibility),
[major capability](gap-analysis.md#4-missing-major-sharex-capabilities) and
[release](gap-analysis.md#5-library-desktop-integration-and-release-readiness)
tables, reconciled against the newer implementation records above.

## Native, account, hardware and external acceptance

- [ ] **Windows:** run the current packaged app through physical hotkeys/focus,
  tray/clipboard cross-app paste and file transfer, mixed-DPI/negative origins,
  monitor reconnect, sleep/resume, device loss, permissions, IME/accessibility and
  shutdown/recovery. Run real Windows FFmpeg/gdigrab and Tesseract workflows.
  Wine process/registry/credential fixtures and GNU compilation are partial
  evidence, not native Windows acceptance.
- [ ] **Linux:** validate pure X11 separately from XWayland; complete native
  Wayland portal capture/selection, GlobalShortcuts, clipboard and resident/
  single-instance integration. The existing synthetic X11/XWayland capture and
  private-Xvfb OCR/QR/cursor/multi-region checks do not validate Wayland.
  [Current window limits](linux-window-capture.md).
- [ ] **macOS:** validate a stable signed/notarized bundle, Screen Recording and
  Retina coordinates, Keychain entitlements/no-prompt denial, launchd/login,
  clipboard/hotkey/tray lifecycle and native file/storage behavior. Linux plist
  fixtures and ARM CI compilation cannot establish these.
- [ ] **Accounts and optional tools:** verify real HTTPS providers, AWS/R2 and
  relevant locked/denied/rotated credentials when authorized accounts exist.
  Current local TLS/signing tests use isolated trust and unique synthetic secrets;
  the Linux Secret Service create/read/update/delete roundtrip passed without
  reading existing account secrets. macOS Keychain and native Windows raw S3
  credential behavior remain deployment checks. Never source secrets from
  arguments, checked-in files, logs, or a guessed private path.
- [ ] **Performance:** retain every [PLAN budget](../PLAN.md#performance-contract--all-versions-and-features):
  five-minute idle CPU/wakeups, private/resident/GPU memory, cold/resident launch,
  physical input/presentation, 1440p/4K capture/output, return-to-idle and 500-cycle
  retention. Development timings and callback/DwmFlush proxies do not close
  these. Linux Kit's measured shell private memory exceeded its target; Windows
  historical resident improvements do not establish the current cross-platform
  baseline. [Baseline](performance-baseline.md), [attribution](performance-attribution.md),
  [resident](performance-resident.md), [control](control-open-latency.md),
  [region](region-latency.md), [Kit measurements](ui-redesign.md).
- [ ] **Final artifacts and failure matrix:** verify the exact native CI/head,
  package and target provenance, real standard-user install/update/uninstall,
  removable/network/full volumes, crash/power-loss recovery and platform-specific
  cancellation. Packaging needs local Python 3.11+; current CI pins Python 3.12
  and covers Linux x86_64/aarch64, Windows x86_64 MSVC and macOS ARM64/x86_64. Portable archives
  are unsigned development artifacts, not published signed releases.

## UI work excluded from this delivery

Do not mark these complete from a headless helper: precision magnifier,
window/control snapping and keyboard region-selection interaction; editor
canvas/upscale/annotation and richer text/tool controls; format/profile settings;
integrated Library filters, cross-page actions and search; recording/scrolling/
OCR/QR task interfaces; pin-to-screen windows and opacity/scale controls; final
GPUI redesign. Core/native support may proceed independently. Existing settings,
themes and editor controls are preserved, not redesigned.
[Design baseline](../DESIGN.md), [UI implementation](ui-redesign.md).

## How to read older status statements

- The older gap tables and PLAN slices predate CLI recording, Windows OCR,
  schedules, S3, non-PNG batches, library queries, codecs and utilities. “Missing”
  there describes that dated baseline; the dated reconciliation and source paths
  above establish current subset coverage. An old “planned” section heading can
  also contain later completed evidence in its body.
- The [review log](goldfish-review-report.md#independent-checks-and-release-limits)
  originally recorded **nine** unmaintained-package errors. Subsequent scoped
  removals in the [dependency delivery record](non-ui-delivery.md#continued-dependency-and-workflow-cleanup)
  reduced the latest recorded count to **five**: `rustybuzz`, `ttf-parser`,
  `zbus_names`, `zvariant`, `zvariant_derive`. Those dormant optional names may remain in the
  lockfile, but the subsequent active production graph migration now passes the
  default advisory audit without ignores. The older failure counts are historical,
  not a current active-graph failure. The glib defect has a source backport; its version-based
  alert is not claimed closed. [License/provenance notices](../THIRD_PARTY_NOTICES.md).
- Source review, compile checks, regular tests, explicitly run ignored fixtures,
  emulated tests and real hardware/account tests are different evidence. The
  51 excluded GIF-tree cases are not 51 passing cases, although some have separate
  named run records. CI explicitly invokes only selected loopback upload tests;
  it does not automatically run every ignored credential/hardware fixture.
- Earlier shutdown text is revoked. No shutdown, new secret access, merge or
  release publication follows from this checklist. Keep original user files and
  all existing unfinished work. Current constraints are [AGENTS](../AGENTS.md),
  the [delivery request](non-ui-delivery.md#request-and-constraints) and PLAN's
  performance/privacy contract.

Coverage: inventoried all **57** project Markdown files present before creating
this document, including hidden paths, excluding `vendor/`, `target/`, `.git/`
and generated dependencies. Reconciled status-relevant body text, not just
headers, including PLAN's detailed feature slices, architecture, phase tables,
Definition of Done and future scope; README, DESIGN, notices, every feature and
performance document, delivery history and EGM review record. Cross-checked
current CLI routing, core modules/exports, Cargo.lock, toolchain and CI target
matrix. This document condenses that evidence; it does not replace the detailed
contracts or declare the full backlog, Phase 0, native gates or ShareX parity done.
