# Non-UI delivery: current checklist

Date: 2026-11-17
Scope: the user's complete major-feature list, with final GPUI redesign and new
UI controls excluded. The workstation stays on. The full backlog is **open**.

Current dependency gate (2026-11-17): the active application graph now uses
Jiff 0.2.43/jiff-core 0.1.5; default `cargo deny check advisories` passes again
without ignores or version spoofing. The earlier Chrono maintenance failure
RUSTSEC-2026-0543 is retained as historical evidence. All current timestamp/
calendar/clock consumers and the unused rusqlite conversion feature are migrated.
Fresh source rounds 7/8 are clean, optimized Linux all-target checking, Clippy
and formatting pass, and the full optimized workspace passes **1,098 tests,
zero failures, 73 ignored, 61 outer suites**. An independent published-Chrono
oracle agrees on 80,004 parse/format/order/duration cases (13,830 admitted).
The owned same-process timezone probe passes; exact old source fails its
negative control. Windows GNU metadata, five exact retained Date-Picker interaction fixtures and
31 scheduler tests also pass. Native runtime and source-specific resource
observations remain separate checks.
[Maintenance details](dependency-maintenance.md).

Finite Linux portal recording/audio and manual-scroll CLI/Task routes are
integrated. Portable/preflight, installed SPA and real private PipeWire/codec/
freshness cases pass, including five native managed-color formats and SDR
fallback. A synthetic 4K run measures p95 46 ms capture / 91.31 ms CLI; it is not
real-compositor or reference-hardware acceptance. [Exact scope](wayland-finite-capture.md).

Native macOS/Windows HDR acquisition and a declared SDR rendition are now
integrated in development source. Optimized Linux and Windows GNU metadata,
89 affected capture controls (four opt-ins ignored) and Clippy pass. Native SDK/
frame/runtime/hardware/resource gates and the larger Windows tiled extension
remain separate. [HDR contract](hdr-capture.md).

Windows incoming native file/image/URL data now uses the isolated non-agile
receiver source for all eligible windows. Linux and GNU workspace/vendor-test
metadata, 122 optimized existing consumers, three deadline controls, Clippy and
formatting pass. Actual Rust COM/OLE/fault/editor/lifecycle and joint resources
remain pending in the explicitly owned production fixture; the earlier 18-case
C++ topology result is separate. [Drop contract](clipboard-import-output.md#integrated-windows-receiver-source-2026-11-17).

Main-app Unix tray/global-shortcut services are integrated through an isolated
Linux native helper and main-thread macOS callbacks. The final optimized
workspace passes **1,141 tests, zero failures, 73 ignored, 61 outer suites**.
Fresh applied-source reviews 11/12, final Clippy/format/default advisory and
Windows GNU metadata pass; all six owned native Xvfb/D-Bus fixture modes pass.
Main-app GUI/resident resource, native Mac and physical/session gates remain
separate. [Unix services](unix-native-services.md).

The corrected modern SXCU/source/result pipeline is integrated. Fresh source
reviews 13/14 are clean; 33 focused Rust, three shared-source and 11 actual CLI
cases, 357 helper assertions and the full 137-request private HTTPS/Task/schedule/
installed-runtime fixture pass. Clippy, Windows GNU metadata and formatting pass.
The fixture uses its own bus/keyring/CA, never the user's keyring or clipboard.
[Current sharing evidence](sharex-runtime.md#corrected-parser-source-and-result-integration-2026-11-17).

Two integration regressions are corrected in `b3f0a27` and `cc38b9c`: Unix
listener EOF now follows socket retirement, and save-intent comparisons accept
semantically identical JSON layouts while retaining exact observed-byte CAS.
The optimized core run passes **523 tests, zero failures, 53 ignored**; 29 save/
recovery controls, the listener controls and 32 fresh retirement repeats pass.
All-target optimized Clippy and formatting pass. The corrected full optimized
workspace run now passes **1,094 tests, zero failures, 73 ignored, 61 outer
suites**, excluding child-harness summaries (log SHA256
`724cccda0bb6ca97c023e4bbf5bd4d9d9652f2aa1970d799ceb6d1ce7570d8e7`).
The earlier full-workspace run
failed the listener case; the subsequent core-only run exposed the JSON-layout
case. Both failed logs are retained, and neither is reported as a passing full
workspace run. Proof: `target/verification/sharex-review6-integration/`.

The renderer now skips fresh path render targets for scenes with no paths.
The old behavior fails the targeted allocation assertion; the corrected source
passes all 47 optimized package tests and an identical-workload memory probe.
The single memory pair shows no meaningful private-memory saving. Actual shell
and reference-hardware budgets remain open. [Renderer evidence](../vendor/gpui-pre-wgpu/LOCAL-PATCH.md).

Explicit isolated image printing is integrated. Three optimized core and six
desktop tests, the real CLI/private IPP end-to-end fixture, Linux/Windows GNU
metadata, optimized Clippy and formatting pass. Native ARM macOS snapshot
`b85db926` (application source `9f2e135`) now also passes fresh-process memory
limit enforcement, three core/six desktop print controls and the actual private
IPP CLI fixture. Intel job 134043931578 was canceled during later compilation and establishes
no full runtime pass; Windows native GDI output/fault
and resource gates remain open. [Printing](image-printing.md).

Chronological capture/upload history sorting is now integrated in core and CLI,
with exact fractional UTC ordering and self-contained timestamp/ID cursors.
The targeted optimized run passes 17 core and two actual CLI cases; both new
opt-in cost/allocator probes also pass. The isolated executable completes 150
timed and six separately memory-sampled requests against 10,000 capture plus
10,000 upload metadata rows with unchanged database bytes. All-target optimized
Clippy, Windows GNU metadata and formatting pass. [Query contract](history-queries.md).

The preceding Windows file-drop/window-ownership correction is committed as
`f2f03a4284e6d0318ecc1efa0363bd490c41b07b`. Its complete optimized workspace run
passed **1,065 regular tests**, zero failures, with 71 opt-ins excluded across 60
outer suites. Linux/Windows GNU all-target metadata, Windows dependency-unit
metadata, optimized Clippy and focused pure/GPUI/font/style regressions pass.
Native COM acquisition containment remains an implementation gap; Windows native
COM/resource acceptance remains open. [Drop contract](clipboard-import-output.md#windows-existing-file-drop-metadata-bounds).
The earlier format/Wayland source is committed as
`fd98138633de4a15b028b62b54c0a2a9af301ec9`; its 1,044-test/private CLI/Wayland
results remain a separate checkpoint. These totals are different source snapshots,
and are not added together or claimed as full non-UI acceptance.

Earlier pushed baseline: branch `feat/non-ui-delivery`, `247f419`, including the
reviewed persistence, logging, managed-color and portable packaging follow-ups.
Committed follow-ups include native defaults/templates and process isolation,
Library lifecycle/reservations, bounded image cancellation, modern SXCU execution,
shared encoded-input decoding and complete private-runtime/native packaging.
Atomic Windows worker startup, durable schedules, real in-memory Wayland
capture and explicit URL image input are also committed. Reviewed Windows physical capture, macOS still/scroll/recording and bounded
clipboard/Paste/Wayland-drop foundations are committed. The layered editor
document backend and physical-X11 color/ruler follow-ups are also committed;
Mac typed point/native-pixel consumers and the derived desktop grid are now
integrated; their current-source native SDK/hardware gates remain open. Other
native input/output work is still open.
Delivered backend paths do not close excluded UI or real native acceptance gates.

The prior Mac-coordinate/parser checkpoint passed **996 optimized tests**, with 69 explicit
opt-ins excluded across 59 outer suites and zero failures. This includes the
bounded logger, journaled capture saves and managed-color input with the exact
same-version moxcms correction, Mac coordinate consumers and structured parser
mutation verification. The first three source review gates converged:
logger rounds 6/7, journal rounds 5/6 and managed-color rounds 5/6. Formatting, all-target
optimized Clippy, default advisories and Windows GNU metadata checks pass. Mac
source rounds 3/4 and structured parser rounds 2/3 are also clean. Current-source
native SDK/runtime and physical acceptance remain distinct from these checks.
Earlier 868-test/786-test/Clippy/Windows GNU/advisory totals remain
historical snapshots. Source reviews and private native fixtures are recorded
separately in each contract. None establishes physical hardware, reference
performance or a production release.

Repository write access is restored. The earlier baseline is published and the
reviewed format/Wayland implementation is committed above. Exact-source native
CI remains a separate verification gate.
No password request or permission change was needed.

CI `8fdbc78` found unformatted snapshot code and a nanosecond-clock temp-name
collision on macOS. `3882c35` fixes both; exact-head native CI remains a separate
gate. Native foundations are locally committed; the current native matrix remains open.
Later native CI corrected BSD loopback socket modes, scheduler lock teardown,
Windows path-spelling deduplication and portable test disconnect handling.
Current partial native SDK/private-process evidence remains distinct from screen,
permission, input, hardware and release-signature acceptance.
Default active-graph advisory checking passes without ignores. The workstation
remains powered on; no shutdown will be performed.

## Immediate verification state

- [x] Current Mac-coordinate/parser checkpoint: 996 passed, zero failed,
  69 opt-ins excluded across 59 outer Cargo suites, with 2,273 source/provenance
  inputs unchanged during checks. Fourteen actual private CLI parser/ruler
  requests pass. A 100-sample synthetic compositor copy/resample/allocation
  report is separate from native capture, SDK, presentation and hardware gates.
  [Mac coordinates](macos-coordinate-consumers.md),
  [structured parser verification](structured-parser-fuzz.md).
- [x] Previous persistence/color/logger checkpoint: 953 passed, zero failed, 69 opt-ins
  excluded across 59 outer Cargo suites. The two new actual CLI save-recovery
  fixtures, 16 private logger fixtures/four headless requests and 14 actual
  managed-color CLI requests pass. Independent LittleCMS 2.19 expectations,
  native callback timings and a short 4K worker report are recorded separately
  from native/hardware acceptance. [Saves](capture-save-recovery.md),
  [logging](file-logging.md), [color](color-managed-input.md).
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
- [ ] Exact current-source native acceptance remains open. The Mac video-family
  completion guard is committed as `b8e4e9a98285d3485bb6bac47aee4ca7d5599ff1`.
  Its workflow-only auxiliary snapshot passed native ARM macOS, Intel macOS and
  Windows in [run 44028548810](https://github.com/lejylabs/screenshots/actions/runs/44028548810),
  including the new owned family-completion regressions on both Macs. This fixes
  the source-level parent-loss race; one green run does not identify every cause
  of the older ARM failure or prove physical capture/TCC/hardware acceptance.
  Linux/macOS ownership/reopen is committed as `1755943`; its newer native CI and
  the current Windows drop integration remain separate exact-source gates.

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
| Multi-output and timing | Calibrated Windows physical desktop composition and derived Mac desktop grids; `capture-regions` takes 1–16 crops from one snapshot per selected display; finite display sequences, explicit one-shot start time, PNG/JPEG/WebP and optional lossy WebP. `capture-task` composes display/window/region → recipe → final save → optional explicit upload. [Desktop](desktop-composite.md), [regions](capture-regions.md), [sequences](capture-sequence.md), [tasks](capture-task.md). |
| Image tools | Conversion/rotation/flips/downscale, exact nearest-neighbour upscaling, padding/background/watermark, combine/compare/split, exact row/column removal, image dimensions/pixel sampling and SHA256 hash/check. [Tools](image-tools.md), [collections](image-collection.md), [strip removal](strip-removal.md), [info](image-info.md), [hash](file-hashes.md). |
| Image formats | Shared PNG/JPEG/WebP and narrow truecolor BMP/classic uncompressed RGB8 or straight-RGBA8 TIFF input; PNG/JPEG/lossless WebP outputs, explicit lossy WebP, capture/Library format policy and byte-preserving saved formats, CLI-only BMP and straight-alpha TIFF output. GIF assembly is play-once, 2–16 images, binary alpha with unsafe/full palette rejection. [BMP input](bmp-input.md), [BMP output](bmp-export.md), [TIFF input](tiff-input.md), [TIFF output](tiff-export.md), [lossy WebP](lossy-webp.md), [GIF](gif-assembly.md). |
| Shared editing foundation | Crop, drawing/text/pen/number/highlight/spotlight, blur/pixelation/redaction, bounded metadata undo, selection/movement/resize/duplicate/drawing order, and rotation/flips before annotations. Text has multiline, discrete sizes and Bold/Italic; monochrome text emoji are not inserted emoji objects. [PLAN editor details](../PLAN.md#phase-4--image-editor). |
| Sharing | Generic bounded POST/PUT, image and arbitrary-file sequential batches, named profiles and per-task destination defaults, explicit finalized-pixel upload, supported `.sxcu` subset, bounded JSON/text/header/regex/XML results and thumbnail/deletion links. S3-compatible signed create-only single-object PUT exists, including R2-style endpoint/region configuration. Standalone `link` formats one upload-result object as plain/Markdown/HTML or saves its URL as QR; it performs no upload or clipboard action. [Image batches](upload-batch.md), [file uploads](file-uploads.md), [S3](s3-uploads.md), [link results](upload-results.md). |
| Privacy and library | Pixel-only edited exports/uploads, opt-in managed PNG/JPEG/WebP sanitization to PNG, bounded structural inventory; capture/upload date/order/text queries, missing-reference audit, import/relink/metadata removal/export, portable journaled moves/transfers and trash/restore/purge with explicit finish/cancel recovery and retention. Existing UI has visible-page thumbnails and page-local actions. [Metadata](image-metadata.md), [PNG inventory](png-metadata.md), [sanitized export](sanitized-export.md), [queries](history-queries.md), [audit](history-audit.md), [management](history-management.md), [moves](history-file-moves.md). |
| OCR, QR and workflows | File QR generation/scanning, optional installed-Tesseract file OCR on Unix/Windows, CLI region OCR/QR using in-memory pixels under strict session/scale limits, exact supplied-image scroll assembly, and bounded nine-operation offline recipes. [QR](qr.md), [OCR](ocr.md), [Windows OCR](windows-ocr.md), [region analysis](region-analysis.md), [scroll assembly](scroll-stitch.md), [recipes](workflows.md). |
| Recording | Optional installed FFmpeg/ffprobe: finite Unix X11 or Windows gdigrab video/GIF, synthetic mode, cursor show/hide, output verification and bounded process/staging cleanup. This is neither audio nor a full recording workflow. [Unix recorder](recording.md), [Windows recorder](recording-windows.md). |
| Desktop, storage, release | Explicit Linux/Windows/macOS autostart management; Windows resident/tray/Show-only lifecycle and [Linux/macOS main-app ownership/reopen](unix-app-lifecycle.md) with the existing visible-window fallback; native secret adapters and CLI setup; SQLite read-only check, WAL-aware backup to a new path and bounded unfinished-file inspection; pinned Rust/lockfile, native CI matrix, portable archives/checksums/notices and manual update instructions. [Autostart](autostart.md), [Windows](autostart-windows.md), [macOS](autostart-macos.md), [credentials](upload-authentication.md), [storage](storage-recovery.md), [packaging](packaging.md). |

## Remaining implementation

These are actionable gaps, not unavailable-test excuses. Implement bounded core,
CLI or native services without adding the excluded UI controls. UI-only workflow
completion is listed separately.

- [ ] **Capture completion:** finish remaining native macOS cursor/policy cases
  and physical topology/lifetime acceptance. Typed Mac Retina template/repeat/
  global-point consumers and derived desktop composition are implemented;
  their native SDK and hardware acceptance remains open. Finish remaining
  non-Windows desktop/platform work,
  and HDR/wide-gamut conversion/capability correctness. Existing strict refusal
  of unsupported layouts does not implement their conversion. Defaults,
  arbitrary Task hotkeys, persistent monitor templates and ordered after-capture
  actions are now implemented foundations; precision selection/new controls stay
  in the excluded UI section.
- [x] **Timing core/service:** durable/repeating schedules, explicit missed/wake
  policy, uncertain-run crash recovery and typed Task source sequences are
  committed. The service is explicit and event driven; real native timer and
  hardware-wake acceptance remains in the separate matrix.
- [x] **Editor data model and image operations backend:** affine existing marks,
  arbitrary ordered effects/drawing with opaque-redaction protection, explicit
  fonts/RGBA/text styles and presets, bubbles, image/logo/emoji assets, magnifier,
  clone/eraser and rotations are committed in the document backend and CLI.
  Existing GPUI controls still use their established renderer; new controls and
  final design remain excluded. Native presentation and performance acceptance
  remain in the separate matrix. [Contract](editor-document.md).
- [ ] **Import/output:** explicit URL CLI/Task input, bounded native clipboard
  and existing cross-platform Paste are committed. Remaining native raw image/URL
  drops and drag-out need integration. Explicit isolated image printing is now
  integrated and passes optimized pure/process and real private IPP checks; native
  Windows GDI output/fault injection, macOS runtime and resource acceptance remain
  open. [Printing contract](image-printing.md). Descriptor-only Wayland GUI startup
  and capture/Library format policy are integrated with local functional checks
  passed and source/document review completed in the committed delivery.
  Open With/file associations are implemented. Managed RGB matrix/TRC and SDR
  CICP imports are implemented with independent color oracles and truthful HDR
  refusal. Broader codec variants remain distinct work; native HDR capture/presentation is not established by import.
- [ ] **Sharing:** complete unsupported workflow-input/provider capabilities from
  their explicit contracts. Modern versioned methods/bodies/templates/parsers,
  TextUploader, URLShortener and URLSharingService destination flags, HTTP/S3/
  SFTP/FTP, public-client OAuth, image/file batches and explicit Task SXCU/Link/
  QR/clipboard actions are implemented. Very old versions remain intentionally
  refused; this is not an unimplemented modern shortener or text-upload path.
  Real accounts/native credential denial/rotation and provider-specific optional
  multipart/STS behavior remain explicit provider limits. No blanket arbitrary-
  version SXCU parity is claimed. [Runtime](sharex-runtime.md),
  [providers](sharing-completion.md), [tasks](tasks.md).
- [ ] **Library/storage:** durable capture-save intents and explicit targeted
  finish/cancel recovery now cover publication/history interruption, with failed
  batch pixels retained for retry. Finish exact native lifecycle/fault acceptance and
  any richer metadata/OCR search. Cross-page core selection/actions,
  missing-reference removal, portable move/transfer, journaled trash/restore/
  purge and explicit retention are implemented. Thumbnail prefetch/cache/filter
  controls are UI consumer work, with no speculative idle cache added.
- [ ] **Major native capabilities/utilities:** finish macOS/Wayland scrolling and
  recording adapters beyond implemented X11/Windows live scrolling and optional
  finite video/GIF/native audio/combined A/V. Native screen-color sampling,
  ruler/measurement and bounded clipboard inspection are committed tested subsets.
  Existing file pixel/hash and
  combine/compare/split tools remain completed subsets. Pin windows and new
  recording/OCR/QR interaction controls are excluded UI work.
- [ ] **Desktop/release:** atomic startup across optional Windows workers is
  committed; [non-Windows main-app ownership/reopen](unix-app-lifecycle.md)
  has passed optimized private Linux integration checks; native macOS and resource
  acceptance remain open. Finish main-app
  native fallback/focus/session acceptance beyond the integrated Linux/macOS
  tray/global-hotkey services and private six-mode functional checks,
  real signing/notarization, standard-user install/upgrade/uninstall,
  permissions/IME/accessibility, physical hotkeys/tray/clipboard and failure/
  performance matrices. User installs, native package layouts, notifications,
  associations and authenticated explicit updates/rollback exist; all five
  requested target builds are in CI. Preserve exact source/artifact provenance,
  pending cleanup and unavailable/full-volume recovery; do not hide failures.
- [ ] **Engineering requirements in PLAN:** bounded asynchronous app-data file
  logging with fixed retention and metadata-only records is implemented with
  native parked-writer and callback checks. Five-minute native idle/resource
  acceptance remain. Structured coordinate/uploader mutation smoke now covers
  16,618 seed/mutation cases, 24 boundaries and independent exact oracles; a
  coverage-guided/sanitizer campaign remains separate.
- [ ] **Dependency maintenance:** maintained font/SVG and D-Bus active families
  and focused regression coverage are implemented. Recheck the exact final graph;
  dormant upstream optional-backend lock entries and the default-branch GLib alert
  remain separate facts. [Current scope](dependency-maintenance.md).

Sources for these families are the [current gap reconciliation](gap-analysis.md#historical-reconciliation-feature-branch-2026-11-15),
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

Coverage update: the earlier hidden-path sweep inventoried **160 Markdown files**
(94 project and 66 vendor), excluding target and .git. The subsequently authored
and reviewed Unix lifecycle contract increases the current source inventory to
163 (96 project and 67 vendor) after the independently reviewed image-printing
contract and the renderer provenance/evidence note were integrated. The earlier 161-file count included the Unix lifecycle
contract; original snapshot hashes remain historical. Its snapshot hashes,
requirement/status extracts and exact per-file reading depth are recorded in the
local reconciliation reports. All 160 complete bodies have now been semantically read (94 project, 66
vendor), with exact per-file hashes and full-body read reports in
`target/verification/all-markdown-full-read-complete.json`. Later corrections
are reviewed explicit documentation deltas with new hashes; the original
snapshot hashes are not relabeled current. This is full prose review, not a
whole-code audit or implementation/native/performance acceptance. Current code checks include the PLAN performance, phase and
Definition-of-Done requirements; logging/fuzz follow-ups are implemented,
while the remaining resident/platform work above stays open. Historical evidence remains dated; this checklist does not declare
the backlog, Phase 0, native gates or ShareX parity complete.
