# Linux X11 and XWayland window capture

Linux window listing and capture use the pinned XCap 0.9.10 X11 path when `DISPLAY` is
present, valid Unicode and not empty or whitespace-only. This includes X11
clients running through XWayland inside a Wayland session. Native Wayland
windows are not listed or captured by this path. A Wayland session with no
`DISPLAY` reports window listing and capture as unsupported.

X11/XWayland **window** capability remains distinct from display/region capture
and Automatic clipboard admission. Retained Wayland authority restricts those
other routes even after GPUI consumes `WAYLAND_SOCKET`; an available XWayland
window does not authorize capturing the whole Wayland desktop. Native Wayland
windows remain unsupported. See [startup authority](linux-wayland-startup.md).

The capability flag checks the environment without opening a connection.
An inaccessible X server, missing authentication or unsupported window-manager
properties still causes the requested operation to fail. A set `DISPLAY` is
not a connectivity or permission guarantee. Display and region capture retain
their existing behavior; this change does not implement or validate a native
Wayland portal backend, shortcuts, overlays or resident integration.

XCap's Linux window implementation reads the X11 window manager's
`_NET_CLIENT_LIST_STACKING` property, limited to its first 1,024 window IDs on
the X screen currently containing the pointer. Its window capture uses `GetImage`
against the selected X11 drawable. It does not select a whole display or invoke
a Wayland screenshot portal for window capture. The linked upstream 0.9.8 implementation below records the original review
rationale; current workspace builds use the pinned local 0.9.10 patch. See
[window enumeration implementation](https://docs.rs/crate/xcap/0.9.8/source/src/linux/impl_window.rs)
and [window capture dispatch](https://docs.rs/crate/xcap/0.9.8/source/src/linux/capture.rs).
Window managers must expose the expected EWMH properties. Unavailable or
protected pixels and covered-window behavior depend on the X server and
compositor; this path does not promise capture of every application.

Linux window captures leave `CapturedImage.display_id` empty and
`display_geometry` unset: the monitor association is unknown, and `window_id`
identifies the captured source. XCap 0.9.8's monitor association uses an incorrect
coordinate for vertical overlap, mixes monitor/window coordinate units and can
overflow before image admission. This path bypasses that association entirely;
it does not guess a display from those values. Ordinary window editing and
export use the captured window pixels. Remembered display regions are not
created from window captures.

Requests keep the existing process-ID, minimized-window and missing-window
checks. Capture groups admit at most 256 MiB of decoded RGBA pixels before
native capture, check dimensions again before each capture and validate the
returned dimensions against the budget. XCap allocates its native reply and
image before returning, so the existing limitation around resize races and
native allocation bounds remains. Enumeration and capture remain synchronous
worker operations; callers must keep them off GPUI's foreground executor.

There is no connection probe during idle, background stream, polling service,
new dependency, worker, cache or retained image. Capability evaluation reads one
small environment value; explicit listing/capture uses the existing on-demand
XCap calls and in-memory image ownership.

## Verification

On 2026-11-15, optimized tests passed on CachyOS Linux under KDE Wayland with
XWayland `DISPLAY=:0`. The native fixture created one managed 96×64 X11 window
in a short-lived Python/libX11 child. It checked that this window was listed,
captured only its ID into RAM, checked the unknown display association, and
verified all 6,144 pixels were exactly
RGBA `[18, 52, 86, 255]`. It also checked rejection after changing only that
fixture's PID to the test process, after setting its minimized state, and after
destroying it. The fixture did not write root-window properties or other clients' state.
No screenshot file, output directory or clipboard content was created.

The final fixture completed in 0.08 seconds including its own startup, enumeration,
capture, guard checks and teardown. This single small-window observation is not
a capture latency distribution, physical presentation measurement or resource
acceptance result. The regular unit test also checks missing, empty,
whitespace-only and non-Unicode `DISPLAY` values without changing process
environment variables.

Run the deterministic capability test with:

```sh
cargo test -p app-core --profile release-fast --locked -j 2 \
  linux_window_capabilities_require_a_nonempty_display
```

The explicit native test requires an accessible X11/XWayland desktop, Python 3,
libX11 and an EWMH window manager. Run it with an outer process timeout because
native X11 calls and fixture pipes are blocking. GNU `timeout` also terminates
the command's process group if a native call hangs:

```sh
timeout --kill-after=5s 30s cargo test -p app-core \
  --profile release-fast --locked -j 2 captures_only_its_synthetic_x11_window \
  -- --ignored --exact capture::tests::captures_only_its_synthetic_x11_window
```

Pure X11 sessions, other window managers, mixed DPI/monitors, unusual visual
depths, covered/protected clients, X-server disconnects, resize/close races,
native Wayland windows and full CPU/memory/input-latency acceptance remain
unverified by this fixture. Native X11 operations still have no hard in-process
deadline.
