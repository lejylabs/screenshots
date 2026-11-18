# Native window policy slice

This source implements conditional Linux X11 decorated-frame inclusion and native
alpha preservation. It leaves Linux compositor shadows, Windows native
shadow/alpha preservation and macOS foreign-window client-only capture
unsupported. It adds no UI controls. Source implementation, compilation, private
fixture results and actual native desktop/resource acceptance are separate
claims. Integrated optimized Linux checks and the owned Composite fixture pass:
twelve CLI invocations, 24,456 exact RGBA pixels and one explicitly discovered
and executed post-readback identity/geometry/cancellation case. Real desktop,
compositor and resource acceptance remain open.

## Source contract

Modify `crates/app-core/Cargo.toml` only to enable Composite on the already locked
x11rb dependency. Render/Shape are already enabled by XFixes. Modify
`capture_native_linux.rs` to allow the new option combinations and delegate
explicit inclusion/preservation to new `capture_native_linux_window.rs`.
Modify `capture_native.rs` to carry Linux admission and the existing cancellation
callback. Modify `capture.rs` to freeze one Linux window admission per request,
charge the entire frame extent and verify it before capture. Capture entry
points, tasks, the existing native worker protocol, exports and controls retain
their public shapes. No vendor patch is needed.

`Include` or `Preserve` selects the new route. Omitted policies, client/native
frame capture and opaque-only legacy capture retain their previous path.
Native option validation still rejects shadow inclusion before delay/capture.
New explicit policies fail early for Wayland sessions; Linux's existing
nonempty-DISPLAY capability gate still lists/captures X11/XWayland windows only.
No native Wayland coordinates or window list is inferred.

The new route admits only a viewable rectangular zero-border InputOutput leaf
client. Frame inclusion requires exactly four CARDINAL `_NET_FRAME_EXTENTS`,
a client directly parented by a root-child frame (or a root-child client with
zero extents), exact physical geometry agreement and no other frame children.
Naming the child as a Composite pixmap must fail with Match; otherwise it is
independently redirected and its content may be absent from the parent's backing
store, so frame capture fails. Other protocol errors propagate. This intentionally
restricted hierarchy avoids adding a second compositor or silently missing
separately redirected children. Nonzero native borders and shaped windows fail.

Capture uses only `CompositeNameWindowPixmap` on an already redirected drawable,
then `GetImage` of that pixmap. It never redirects user windows, reads root,
grabs the server, creates an overlay, resamples, guesses frame padding or
changes another client's window state. Lack of existing backing storage fails
with an explicit constraint. RGB8/ARGB8 visual, depth, channel masks, 32-bit
packing/scanlines and server image byte order are verified through Render and
core setup. XRGB alpha is 255 because the verified visual has no alpha component.
ARGB channels must be premultiplied; decoding unpremultiplies to straight RGBA,
using integer rounding and zero RGB for zero alpha. Malformed alpha is rejected.
Unsupported formats fail rather than inferring RGB/HDR/color conversions.

The frozen admission includes client/frame IDs and parent relationship, client
PID property, positions, dimensions, visual, depth and optional exact extents.
Viewability, unshaped geometry and hierarchy restrictions are rechecked before
readback, after readback and before publication. A changed observation fails the
entire batch without publishing an image. PID properties and endpoint geometry
are the existing identity model; XRes process authentication, generation tracking
and atomic geometry/pixel/compositor snapshots are not implemented here.
The source charge is `frame_width * frame_height * 4` for included frames,
rather than the smaller client charge. The existing aggregate 256 MiB ceiling
and one active native operation remain. Uninterruptible calls stay within the
existing isolated process lifecycle when that executor is installed. The
callback checks cancellation around native stages and on every decode/flatten
row; typed cancellation is preserved in Linux delivery. Native pixmap references
and connections drop on success and failure; pixels remain in RAM.

## Resource cost

The new path holds one on-demand X11 connection, one server pixmap reference,
a bounded observation and one native packed reply plus one straight RGBA output.
Each pixel buffer is at most the admitted source bytes (256 MiB aggregate per
job). Naming existing storage keeps it alive until release; it does not request
an additional raster copy. The X server's existing backing storage and its
allocator/driver cost are additional OS costs. Native protocol metadata parsing
is owned by x11rb; this is not a new independent hard process-RSS guarantee.
No resident runtime, stream, timer, cache, thread, queue, image texture, disk
handoff or dependency package is added. Shared native-worker result transport
and previous-frame ownership costs retain their existing contract. Full memory,
latency, 500-cycle and reference-hardware acceptance must still be measured.

## Finite owned-fixture gate

Create `scripts/test-native-window-policy.py`. Given an already built optimized
application, core test executable and Xvfb path, it starts one authenticated,
TCP-disabled private X server under a private directory, clears Wayland/session
bus access and redirects user state. It creates every window and pixel itself:
a minimal reparenting WM model with declared extents, an opaque sibling covering
that frame, and a leaf ARGB32 surface with known premultiplied values. Production
capture reads only these fixtures. FFmpeg independently decodes exact output
RGBA; the covering sibling catches any root sampling. No user input, clipboard,
settings, hotkeys, uploads or native application windows are touched.

The gate checks exact frame and client pixels, opaque RGB preservation, native
ARGB transparency, included-frame explicit-background flattening, EWMH listing,
invalid extents, shadow refusal, independently redirected child refusal, nested
child refusal, Wayland refusal and unavailable backing-store refusal. Failed
captures must create no output. The same fixture invokes an ignored optimized
core test in its own bounded process: it changes only the owned fixture frame's
position or client PID property after native readback and confirms refusal,
then cancels during decoded-row work and confirms typed cancellation. State is
restored by a guard; all fixture windows/server resources are reclaimed even
on failure. Two ordinary pure checks cover native-endian alpha decoding,
malformed bytes/alpha, cancellation and exact frame arithmetic.

Root integration owns compilation and execution. Example after build:

```sh
python3 scripts/test-native-window-policy.py --binary target/release-fast/screenshots --core-tests target/release-fast/deps/app_core-EXACT_TEST_EXECUTABLE --xvfb /absolute/path/to/Xvfb --report target/verification/native-window-policy-private.json
```

The runner has finite startup/native subprocess deadlines, pins the application
inode/metadata and records its SHA-256. Fixture duration includes startup,
subprocesses and independent decoding; it is not capture latency distribution
or resource-budget evidence. A failed optional tool/extension/visual is a failed
gate, not a pass with native checks skipped. Native window managers with other
hierarchies, physical desktops, Shape, other byte orders, mixed DPI and native
permission behavior still need their own acceptance cases.

## Remaining platform constraints

Linux compositor shadows usually live outside the selected window hierarchy.
The [Composite protocol](https://www.x.org/releases/current/doc/compositeproto/compositeproto.txt)
specifies hierarchy storage limited by the redirected top window's geometry.
It does not identify an external compositor's separate shadow pixels. The new
source therefore cannot promise shadow inclusion; root sampling or guessed
padding would leak unrelated windows. The protocol also defines pixmap lifetime,
visibility and independent redirection used by the native guards.
[Render](https://keithp.com/~keithp/render/protocol.html) defines visual-to-format
mapping, RGB/alpha masks, opaque no-alpha formats and premultiplied composition.
The [EWMH frame-extents contract](https://specifications.freedesktop.org/wm/latest/ar01s05.html)
supplies the four decoration extents; source checks additionally prove the
observed parent geometry rather than trusting the property alone.

Windows still uses the reviewed GDI adapter. Microsoft's
[BI_RGB bitmap contract](https://learn.microsoft.com/en-us/windows/win32/wmdm/-bitmapinfoheader)
reserves the high byte of 32-bit RGB; it supplies no reliable native alpha.
[GetWindowRect](https://learn.microsoft.com/en-us/windows/win32/api/winuser/nf-winuser-getwindowrect)
and physical DWM visible bounds provide native frame/client geometry, not
ownership-isolated compositor shadow pixels with verified alpha. Current code
therefore keeps alpha preservation and shadow inclusion unsupported. A future
owned-window WGC/compositor implementation needs separate per-pixel alpha,
shadow extent, capture-exclusion, identity, deadline and byte-admission evidence;
this document does not assert that every future Windows SDK route is impossible.
Relabeling GDI's reserved byte or sampling surrounding desktop is not support.

macOS already has the native RGBA/shadow paths described in
[macOS capture completion](macos-capture-completion.md). Its current
[CGWindowListCopyWindowInfo](https://developer.apple.com/documentation/coregraphics/cgwindowlistcopywindowinfo(_:_:))
metadata and ScreenCaptureKit source identify whole-window bounds; this adapter
has no verified foreign-window client/content rectangle. AppKit's
[contentRect(forFrameRect:styleMask:)](https://developer.apple.com/documentation/appkit/nswindow/contentrect(forframerect:stylemask:))
requires the actual window style, or an owned NSWindow instance. Guessing a
foreign titlebar height/style cannot prove client-only capture, especially with
full-size content and custom decorations. Current client-only requests remain
unsupported before capture. This is a precise constraint of the selected public
adapter, not a claim that external application cooperation can never expose
content geometry.

## Integrated owned fixture evidence — 2026-11-18

The complete private runner passes twelve real CLI invocations and compares24,456RGBA pixels, including an occluded decorated frame, verified premultiplied ARGB, opaque flattening, forbidden shadows/extent mismatch/independent-client backing/child hierarchy/Wayland and absent-backing refusals. The exact core post-readback identity/geometry/row-cancellation case is discovered and executed once. CLI SHA256 `e7c3975641a25e0152265c9037e23cb9734db1e097a9cdd77c0f9a1ebdb38d18`; core test SHA256 `ee223e845ad31631c4aeec3fb42cebda534f96b0e4ce23b40c08982ce878ce87`.

The first absent-backing fixture incorrectly assumed an ARGB depth-different window loses implicit server redirection after explicit unredirect. The corrected negative case uses the root-depth opaque frame; no production policy was weakened. Mesa shader caching is disabled in the private fixture so its no-persistent-state assertion reflects application capture, and the outer owner reaps the whole family before deleting private authority. These tiny software results do not establish physical-WM compatibility, native HDR/alpha on other systems or resource/latency budgets.
