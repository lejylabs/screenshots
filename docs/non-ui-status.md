# Non-UI delivery: current checklist

Date: 2026-11-15
Scope: the user's complete major-feature list, with final GPUI redesign and new
UI controls excluded. The workstation stays on. The full backlog is **open**.

Snapshot: branch `feat/non-ui-delivery`, committed implementation HEAD `17ee7ae`,
including GIF implementation `485386e`, TIFF input `bc37362` and link-result
formatting/QR `17ee7ae`. Thumbnail cancellation and admission changes are in
progress in the working tree. “Delivered subset” means an existing CLI/core
path, not completion of
the corresponding product family or native acceptance. Evidence belongs to the
named build and fixture. No new tests, builds, network calls or captures were
performed to prepare this checklist.

## Immediate verification state

- [x] Final committed GIF tree: 487 regular optimized workspace tests,
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
- [x] TIFF final complete optimized workspace run passed 499 regular tests,
  including all seven TIFF core fixtures, with 51 opt-in cases excluded and
  all doctests passing. Implementation commit `bc37362` is pushed.
  [TIFF contract](tiff-input.md), [current evidence](non-ui-delivery.md#bounded-truecolor-tiff-input-in-progress-2026-11-15).
- [x] Link-result formatting/QR: final rerun passed 511 regular optimized tests,
  51 opt-in cases excluded, all doctests passed; final Clippy, formatting and
  Windows GNU test compilation passed. The release passed independent
  CommonMark/HTML href and label checks, QR decoding and package smoke.
  The earlier dense-QR scanner failure is disclosed in the
  [delivery evidence](non-ui-delivery.md); it was not counted as a pass.
- [ ] Thumbnail cancellation is in progress: stop obsolete reduction work,
  retain the active job's admission until completion, and make both Quit paths
  wait for that completion. This repairs control flow without changing UI
  design or adding controls. Tests remain pending at this snapshot.
  [Contract](image-cancellation.md).
- [ ] Read native CI for exact head `17ee7ae` before claiming it passed. The
  earlier `bc37362` run was queued at its recorded check; its current state was
  not queried for this documentation update. [Recorded TIFF run](https://github.com/lejylabs/screenshots/actions/runs/43731771823).

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
| Privacy and library | Pixel-only edited exports/uploads, opt-in PNG metadata stripping, bounded PNG/JPEG/WebP structural inventory; capture/upload date/order/text queries, missing-reference audit, import/relink/metadata removal/export, and Linux journaled same-filesystem PNG moves with explicit finish/cancel recovery. Existing UI has visible-page thumbnails and page-local actions. [Metadata](image-metadata.md), [PNG inventory](png-metadata.md), [sanitized export](sanitized-export.md), [queries](history-queries.md), [audit](history-audit.md), [management](history-management.md), [moves](history-file-moves.md). |
| OCR, QR and workflows | File QR generation/scanning, optional installed-Tesseract file OCR on Unix/Windows, CLI region OCR/QR using in-memory pixels under strict session/scale limits, exact supplied-image scroll assembly, and bounded nine-operation offline recipes. [QR](qr.md), [OCR](ocr.md), [Windows OCR](windows-ocr.md), [region analysis](region-analysis.md), [scroll assembly](scroll-stitch.md), [recipes](workflows.md). |
| Recording | Optional installed FFmpeg/ffprobe: finite Unix X11 or Windows gdigrab video/GIF, synthetic mode, cursor show/hide, output verification and bounded process/staging cleanup. This is neither audio nor a full recording workflow. [Unix recorder](recording.md), [Windows recorder](recording-windows.md). |
| Desktop, storage, release | Explicit Linux/Windows/macOS autostart management; Windows resident/tray/Show-only single-instance lifecycle; native secret adapters and CLI setup; SQLite read-only check, WAL-aware backup to a new path and bounded unfinished-file inspection; pinned Rust/lockfile, native CI matrix, portable archives/checksums/notices and manual update instructions. [Autostart](autostart.md), [Windows](autostart-windows.md), [macOS](autostart-macos.md), [credentials](upload-authentication.md), [storage](storage-recovery.md), [packaging](packaging.md). |

## Remaining implementation

These are actionable gaps, not unavailable-test excuses. Implement bounded core,
CLI or native services without adding the excluded UI controls. UI-only workflow
completion is listed separately.

- [ ] **Capture controls:** configurable still-image cursor composition,
  window-frame/shadow/transparency policies, arbitrary command-to-hotkey mapping,
  and persisted region-to-monitor identity plus per-region hotkey/task profile.
  Existing recording cursor flags and six named capture hotkeys do not supply
  these. Add general after-capture/after-upload chains and sharing profiles
  beyond the fixed `capture-task` sequence.
- [ ] **Timing, topology and color:** persisted/repeating schedules, missed-run
  handling and any explicitly selected wake integration; window/region sequences;
  calibrated mixed-DPI/rotation desktop composition; dynamic native topology
  notifications; HDR/wide-gamut metadata, conversion and truthful capability
  reporting. One live process waiting for a future time is not a scheduler.
- [ ] **Editor data model and image operations:** transform existing annotations,
  consistent arbitrary effect/drawing order while preserving opaque-redaction
  safety, richer font/color/style and reusable tool/effect/export presets;
  speech bubbles, inserted image/logo/clipboard/emoji objects, magnification,
  clone/eraser and annotation rotation. Add missing bounded shadow/background
  removal or richer composition algorithms as separate core work. Existing CLI
  canvas/upscale/watermark and flattened recipes do not implement these objects.
- [ ] **Import/output integration:** Open With/file associations, explicit URL
  import and raw-image drops, non-Windows image paste, print and native file
  drag-out. Preserve PNG-only history/default distinctions when extending
  formats/preferences. Broader BMP/TIFF variants, general animation editing,
  and color-managed import remain outside the narrow codecs already delivered.
- [ ] **Sharing:** remaining `.sxcu` methods/body/template/parser compatibility,
  bounded SFTP/FTP and OAuth/provider integrations, S3 multipart/STS/refresh or
  remote browsing where required, text/folder/clipboard/URL upload workflows,
  explicit configurable link copying/custom templates/shortening and integrated
  QR follow-up. The delivered `link` postprocessor supplies fixed plain/Markdown/
  HTML formats and explicit QR output from single-result JSON; it does not
  integrate actions into upload commands. Batch/nested input remains unsupported.
  See [result processing](upload-results.md).
  Unsupported `ErrorMessage` templates currently fail explicitly; they are not
  implemented. [Uploader boundary](file-uploads.md), [S3 limits](s3-uploads.md).
- [ ] **Library and storage:** cross-page selection/actions; bounded next-page
  prefetch/persistent thumbnail storage; automatic or explicit missing-reference
  reconciliation beyond audit/relink; Windows/macOS moves, cross-filesystem
  transfer and robust rename; explicit physical-file deletion/retention with
  recoverable file/DB ordering; richer substring/OCR search. Upload-link search
  already exists in CLI queries; integrated Library link search does not.
  Existing thumbnail cancellation/admission/Quit repair is the active bounded
  lifecycle slice above, not new prefetch or thumbnail UI.
- [ ] **Major capabilities and utilities:** actual live scrolling control,
  fixed-header handling and imperfect-match reporting; audio capture and richer
  recording devices/codecs/timeline/trim/transcode workflows; general external
  actions/task graph and authenticated typed task IPC beyond Show; native screen
  color picker, ruler/measurement, clipboard viewer, URL/system/remote-storage
  tools. Existing combine/compare/split/hash, file pixel sampling and recipes
  remain useful completed subsets.
- [ ] **Desktop and resilience:** actionable native notifications, file/context
  associations, full install/upgrade/uninstall integration and signed macOS
  bundles; authenticated update delivery and rollback behavior. Add macOS
  x86_64/Linux ARM build/package coverage required by
  the user's list. Finish cross-file/database crash recovery and unavailable/full
  volume behavior, bounded isolation for uninterruptible native calls and
  cooperative cancellation where image jobs still lack it. Do not add retries,
  polling or silent deletion to hide these failures.
- [ ] **Dependency maintenance:** resolve the remaining font/SVG and D-Bus
  dependency families with appropriate rendering/IPC regression coverage; keep
  advisory policy and supported platforms intact. Recheck the exact final graph
  rather than copying an earlier audit count.

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
  and covers Linux x86_64, Windows x86_64 MSVC and macOS ARM64. Portable archives
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
  `zbus_names`, `zvariant`, `zvariant_derive`. Those five names remain in the
  inspected Cargo.lock. This is historical audit evidence plus graph inspection,
  not a newly run audit. The glib defect has a source backport; its version-based
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
