# Live scrolling capture

The [macOS native completion](macos-capture-completion.md) adds a separate
owned-helper runner with retained, explicitly bound public AX window/scrollbar
objects. It reuses this assembler/result contract; the X11/Windows evidence
below does not count as macOS permission, focus, pixel or native delivery
acceptance. No global wheel fallback is used.

Author: NewtTheWolf
Date: 2026-11-15
Status: implemented for Windows and local X11; core/CLI tests and private-Xvfb
native scrolling checks passed. Native Windows and other app acceptance remain
open; macOS/native Wayland have no live adapter yet.

## Outcome and scope

Add a finite explicit scrolling capture that controls a supplied native window,
keeps captured pixels in memory and publishes one stitched PNG. This replaces
neither the existing exact `stitch-scroll` file utility nor its byte semantics.
The new command actually sends scroll events, captures each resulting viewport,
recognizes repeated end-of-scroll frames, matches shifted overlap with a bounded
pixel tolerance, and removes explicitly supplied fixed header/footer bands.
It does not inspect browser DOMs, install extensions or initialize GPUI.

```
screenshots scroll-capture WINDOW OUTPUT.png --region X Y WIDTH HEIGHT
  [--steps N] [--wheel N] [--settle-ms N] [--min-overlap N]
  [--delay-ms N] [--tolerance N] [--trim-top N] [--trim-bottom N]
```

WINDOW is an exact positive decimal native window ID, at most 20 bytes. The
region is in window-local physical pixels; all four fields are required.
The selected external window must already be foreground/focused, visible and
unminimized when capture starts. `--delay-ms` accepts 0–10000 (default 0), allowing
the caller to focus the selected application after starting the command from a
terminal. Cancellation remains active during that wait, which counts toward the
120 second deadline. The command never activates an app or moves the pointer.
Before every wheel and capture, recheck focus, process identity, outer geometry
and window existence. Abort without publishing when they change. Coordinates
must remain in bounds. Windows uses a per-monitor-aware thread context and the
selected window must remain on one unrotated 100%-scale display; native mixed-DPI
scrolling is not claimed. Linux requires local X11/XWayland, scale 1 and no
Wayland session. macOS and native Wayland fail explicitly before input unless a
native adapter is actually implemented and independently tested.

Defaults and strict bounds: steps=32 (1–128 scroll actions), wheel=3 (1–10
notches), settle-ms=200 (20–2000), min-overlap=32 (1–1024 and less than the
trimmed height), tolerance=0 (0–16), trim bands=0. Duplicate/unknown options,
invalid paths/numbers and an existing output (including dangling symlinks) fail
before capture or input. The output extension must be PNG, case-insensitive.
The output path is at most 4096 native encoded bytes. Input arguments never
pass through a shell. A window frame is at most 32 MiB RGBA; the requested
viewport at most 16 MiB and 4096 pixels per side. Refuse unsupported native
coordinates before requesting pixels.

The admitted total wait (`delay-ms + steps * settle-ms`) must not exceed 100 seconds,
leaving operation-deadline room for captures, matching and output. Native kernel
or filesystem stalls still have the explicit limits below.

Trimming removes the specified top and bottom rows from every viewport before
matching and output, leaving at least two content rows. This handles a known
fixed header/footer without guessing whether repeated application content is
chrome. Trimmed bands are not retained in the final image; whole-window border
semantics do not change. Automatic detection, horizontal scrolling and
imperfect perspective/scale correction need separate contracts.

## Native input and session behavior

Linux opens one Xlib connection on this headless thread. Send ButtonPress and
ButtonRelease button 5 events to the exact target window with coordinates at
the region center. Verify target process through `_NET_WM_PID`, map state,
geometry, and focus target/descendant using bounded parent traversal. A window
without process identity cannot be used. Targeted synthetic wheel delivery is
not accepted by every X11 client; if no content changes the result reports that
fact, rather than pretending an entire document was captured. No XTest/global
mouse injection, pointer motion or focus stealing is used. X11 errors must be
trapped and returned rather than terminating the process. The adapter's
thread-local use and error-handler lifetime must not overlap other Xlib work.

Windows measures `DWMWA_EXTENDED_FRAME_BOUNDS`, matching the pinned XCap capture
origin and size rather than `GetWindowRect`'s invisible resize margins. It sends
WM_MOUSEWHEEL directly to the target HWND with the region center in screen
coordinates and negative WHEEL_DELTA multiples. Use
SendMessageTimeoutW with SMTO_ABORTIFHUNG/SMTO_BLOCK and a 250 ms timeout, no
PostMessage queue growth. Require GetForegroundWindow target and stable process/
geometry checks on every step. A window-specific message can be ignored by an
app; unchanged content is handled as end/no-scroll rather than success of a
whole-document capture. No general input automation API is exposed.

A finite deadline of at most 120 seconds covers application-side capture,
settling and matching. Install operation-lifetime SIGINT/SIGTERM handling on
Unix and Ctrl+C on Windows using existing mechanisms if available; terminate
without output on cancellation. Settling waits are operation-scoped and check
cancellation in at most 20 ms chunks. Native XCap/Xlib/filesystem calls can
still stall in the kernel; the elapsed deadline is checked before/after them
and does not claim hard interruption. No new watchdog or orphaned timeout
thread is allowed.

## Matching and output assembly

Create a reusable core `ScrollAssembler` with validated options and a streaming
`push` method. Keep the immediately previous trimmed viewport and one growing
output vector, not the whole frame collection. First image is copied into the
output once. Each subsequent image must have identical dimensions and exact
RGBA backing. Compare repeated complete frames before searching for overlap.
Two consecutive unchanged captures after separate scroll actions establish an
end condition. A transient unchanged frame does not end capture immediately;
a later changed frame resumes matching. The caller never appends an unchanged
viewport. Reaching the action cap returns a bounded image with explicit
`stop_reason: "step_limit"`, `complete: false`; it does not claim document end.
A detected repeated end gives `stop_reason: "unchanged"`, `complete: true` only
in the operational sense of no observed change, not proof the document has no
lazy-loaded content. Zero changed frames returns `stop_reason: "no_scroll"`
and complete=false, with the initial viewport and truthful capture counts.

Search only positive vertical advances with overlap from min-overlap through
height-1. Compare deterministic sparse row/column samples to reject candidates
cheaply, then fully verify every surviving candidate's overlapping RGBA bytes.
A candidate passes only when each channel's absolute difference is at most the
requested tolerance. Alpha and hidden RGB are included; never allow a tolerant
match to remove an opaque redaction or synthesize source pixels. Default exact
matching is unchanged by the existence of tolerance. Exactly one full candidate
must pass; no-match or ambiguity is a failure, not a guessed seam. Preserve the
first occurrence of overlap bytes and append only the new suffix from the next
viewport. Do not average/blend pixels across the seam or silently repair motion.

Bound all candidate work explicitly to 512 MiB of channel comparisons per job,
including sparse and full comparisons, and at most 128 append operations.
The growing output must satisfy `MAX_CAPTURE_BYTES` (256 MiB), checked before
reserve/append. Allocation failure is a returned error. Maximum 16 MiB per
viewport, two active viewports and growing-output/reallocation cost are recorded
against PLAN's active-work formula. Capturing/cropping can additionally retain
one native full-window frame of up to 32 MiB before the 16 MiB viewport is
extracted; it is released when that capture returns. Vector growth can retain
the old and new allocation during reallocation. On completion consume the vector as one
RGBA image and use the existing atomic PNG writer. Reject overflow, changed
width/height, invalid geometry, matching ambiguity/work exhaustion, disk errors
and output collisions without publishing a final partial file.

## Result, privacy and resource ownership

Success flushes one JSON object with format/width/height, captured_frames,
scroll_actions, appended_frames, vertical advances, tolerance, trim_top,
trim_bottom, stop_reason and complete. It does not include app titles, paths,
window/process IDs or content. A stdout failure after publication explicitly
says the image remains saved. No upload, clipboard write, history, settings,
background service or retained capture buffer follows. Cancel/failure drops all
pixels and removes only the atomic writer's known temporary file. A crash can
leave an unpublished `.part`; existing storage inspection remains explicit.
The target app's scroll position is a deliberate effect and is not rolled back;
failed cleanup does not send compensating scroll events to a possibly changed
window. Document this beside invocation examples.

## Exact file scope and checks

Own `crates/app-core/src/scrolling.rs` for assembly/matching and its focused
fixtures; `apps/desktop/src/scroll_capture.rs` plus target-specific sibling files
for native adapters; `apps/desktop/tests/scroll_capture.rs` for executable
admission; `scripts/test-scroll-capture.py` for private Xvfb end-to-end; and this
contract. Root wires core `lib.rs`, desktop main/CLI and shared documentation.
Use existing libc/windows-sys/Xcap/image dependencies and link X11 only on Linux;
required windows-sys features are added only after notifying root. No new heavy
library, runtime or image backend.

First implement/test the pure assembler and exact/fuzzy/ambiguous/end boundaries;
then native adapters and coordinator; then executable/private-X11 verification.
Root reviews the plan and source before declaring tests passed. Synthetic tests
cover deterministic document patterns, near-color differences, fixed-band trims,
complete equality, repeated areas, movement in the wrong direction, clipped last
scroll, work/output limits, cancellation and geometry/focus/process changes.
Private Xvfb test owns an external window with PID metadata, explicit focus and
known scrolling content; responds to wheel events with deterministic redraws.
It verifies the actual executable's final pixels and scroll count with an
independent decoder, and no user state or other display is touched. Refusal tests
cover unavailable window/session, invalid options and existing destinations.
No real user app is scrolled in unattended tests. Windows cross-compilation and
pure Win-message argument tests supplement rather than replace native Windows
acceptance. Record optimized timing/RSS on synthetic reference patterns, plus
release/archive smoke and completed test commands in the delivery record.

## Completed development checks

Four optimized core fixtures passed exact multi-seam/short-last-scroll,
tolerance-preserving pixels, fixed-band trimming, repeated-end frames,
ambiguity, wrong-direction movement, cancellation, atomic-publication cancellation
and comparison-budget refusal.
Four actual CLI fixtures passed argument/size/count limits, unavailable session,
existing file/directory/dangling-symlink preservation and non-UTF-8 rejection.

The explicit `scripts/test-scroll-capture.py` check passed on a private,
authenticated Xvfb server with TCP disabled. Its own external window responded
to seven actual wheel messages; eight captured viewports produced advances
`[37,37,37,37,7]`. The four-row fixed header/footer bands were removed and the
96×227 final PNG matched every independently decoded FFmpeg RGBA byte. An
existing output remained unchanged and isolated user-state paths were absent.
The fixture creates the EWMH window/process metadata on its private server; it
does not modify root properties or send input on the user's server.
The final fixture also passed refusal after focus loss, SIGTERM cancellation
during an active settle wait, changed PID detection, a 100 ms startup delay and
truthful saved-output reporting when stdout is `/dev/full`. Cancellation is
rechecked after PNG encoding/sync immediately before atomic publication; encoding
itself is synchronous and is not claimed to stop mid-call.

```
python3 scripts/test-scroll-capture.py --binary target/release-fast/screenshots \
  --xvfb target/isolated-x11/package/usr/bin/Xvfb \
  --report target/verification/scroll-capture-private.json
```

The initial fixture failed because its minimal private server did not create
the EWMH state atoms required by XCap. Adding those fixture atoms corrected the
test setup. Mesa's optional shader cache is disabled in the fixture so its
server cannot create redirected cache state. Native app input policy, physical
presentation, mixed DPI and real document-end behavior remain separate checks.

The explicitly invoked synthetic 1920×1080 matching benchmark used two warmups
and ten measured samples with an atomic cancellation flag. Median matching was
2.656 ms and p95 3.186 ms for a 700-row advance. Three isolated complete benchmark
processes observed maximum RSS 74,704/75,204/75,168 KiB; this includes the full
reference document, both viewports, output and repeated fixture trials, not just
the production assembler. Raw measurements are in
`target/verification/scroll-matching-performance.json`. These development
observations do not establish native capture latency or reference-hardware gates.
