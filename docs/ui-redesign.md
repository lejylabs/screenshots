# Native UI redesign on feat/newt

The project is a Rust/GPUI desktop tool with a substantial capture/editor core.
Its existing shared palette was static; capture modes lived behind a menu, and
page hierarchy, focus and compact layouts needed refinement. The implementation
keeps native windows and existing capture, history, cancellation, export and
upload boundaries. No additional dependency or service was introduced.

## Design and behavior

- Default is a custom digital light-table palette: bright surfaces, graphite
  text, deep petrol commands and a four-corner viewfinder for region capture.
- Oldschool uses a compact native layout: charcoal surfaces, blue selection, colorful
  function icons and a compact task sidebar. It replaces the large empty capture
  panel with shortcut help and uses 28px editor tool buttons.
- Safelight is a custom darkroom palette: deep red-brown, warm paper text and
  coral controls. UI appearance never changes annotation or export colors.
- Recent capture, Library, Capture sources, Uploads and Settings are directly
  reachable. Recent capture has one primary command and a compact capture list.
- Search has comfortable padding, a native text-input role and a focus-only
  caret. Source rows now have keyboard focus and accessible names.
- Theme choices include visible checks and selected accessibility state. A
  setting change applies immediately, saves off the foreground executor and
  restores the old theme on failure. Folder changes preserve appearance.
- Returning from the editor restores control-window keyboard focus. Ctrl+N,
  Ctrl+1/2/3 and Ctrl+, navigate the workbench (Command on macOS).
- The shell opens at 1060x760, with a 680x480 floor. Major content sections retain
  measured height and scroll; theme choices wrap. Standalone editor retains its
  existing 760x400 minimum.

Tokens and interaction principles live in [DESIGN.md](../DESIGN.md). The native
implementation uses `theme.rs` rather than adding a CSS/webview theme layer.

![Oldschool workspace](design/oldschool.png)

## Verification

`cargo +1.99.0 check --workspace`, the release build and all 91 enabled workspace
tests pass. Twenty-four opt-in tests remain ignored. Tests cover all theme
serialization, legacy/missing/unknown values, contrast for text/actions/focus,
and existing capture/editor/history behavior. Clippy completes with existing
warnings; this change does not claim a warning-free repository.

Real release windows were inspected on Linux X11 via XWayland, using isolated
XDG directories and a synthetic image under `target/ui-preview`. All three
appearances persisted; restart restored the selection; a permission-denied save
restored the previous appearance and reported the failure. Search, empty results,
Escape reset, bounded preview, editor entry/cancel, restored shortcuts and compact
layouts were exercised. A synthetic frame annotation exported through Save only
created a new 1280x720 PNG while preserving the original hash. The final Safelight
editor was inspected as well. No real desktop or user image was captured, and no
upload was performed.

The native picker import was not completed in automation; UI checks used an
isolated fixture database, alongside the existing passing import unit tests.
Linux window enumeration remains unsupported by the existing backend in this
session. Windows/macOS interaction, mixed DPI and screen-reader acceptance remain
open; the Linux visual checks do not establish those.

## Resource cost and short comparison

One GPUI global stores a small enum; render closures copy a fixed palette. The
viewfinder paints four native vector corner strokes. Theme choices add three
small control trees only while Settings is open. There is no new image cache,
texture, worker pool, dependency, permanent timer, polling loop or idle task.
Settings use the existing worker executor. The first explicit appearance change
may create the configured capture directory if it does not yet exist.

An exploratory release comparison against the original `f4357f2` used the same
1060x760 empty visible workspace and one five-second idle sample per build.
Observed private mappings were 187.61 MiB before and 130.83 MiB after. CPU was
0.2% and 0.4% of one core respectively (10 vs 20 ms of process CPU time): the
short CPU samples are inconclusive. Memory observations are not a causal speedup
claim. These are not the five-minute/reference-machine acceptance checks in
PLAN.md; GPU allocations, cold/input latency and percentile regressions were not
measured. The existing resource targets remain open.

Machine-readable evidence and binary hashes are in
[ui-redesign-verification.json](ui-redesign-verification.json). Native screenshots
are local review artifacts under `target/ui-preview`, intentionally outside Git.

## Compact Oldschool follow-up

Oldschool now has its own compact native layout: a 180px task sidebar, 13px
commands, colorful function icons, blue row selection, square controls and a
shortcut-based empty workspace. The editor uses 28px tool buttons. Default and
Safelight retain their existing workspace. Keyboard page actions explicitly
restore the root focus context before replacing a focused page control, so
successive shortcuts keep working.

The release build and all 91 enabled workspace tests pass after this follow-up.
Native control windows were inspected at 1060x760 and 680x480, and the editor at
1000x760 and 760x500. Theme persistence, return to Default and restart restoration
were exercised. Three keyboard-navigation cycles were checked by comparing
rendered page headers, including successful return to Settings. Only app-owned
windows and the isolated synthetic image were used; no capture/upload or output
was triggered in these follow-up checks. The earlier idle measurements predate
this follow-up and are not a performance claim for the classic layout.

## Default appearance

Oldschool is now the default for fresh installs, missing theme fields and unknown
future theme names. Explicitly saved preferences remain unchanged. The original
light appearance is labelled Light table and retains its historical `default`
storage value. Theme choices show Oldschool first and describe it as the default.
The settings regression covers fresh, missing and unknown theme selections as
well as round-trips of all three explicit preferences.
