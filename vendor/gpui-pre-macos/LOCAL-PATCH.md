# Native drop and drag-out integration

This is the complete published `gpui-pre-macos` 0.3.12 package, Apache-2.0,
registry SHA256 `2de7d3f4ad51ac387e822495db363fcc1188d349793365731d054444c4cb06ee`.
Upstream repository: https://github.com/zed-industries/zed; recorded upstream
revision `d2a4fb6ded7c470be25d1541281da3882495b65b`. The original license and
unchanged package files are retained.

The patch routes file/image/URL pasteboard acquisition through the application's
existing isolated native worker. AppKit callbacks retain bounded source
descriptors and enforce sequence/generation/admission; stale completions are
discarded. Outgoing Files are bounded, Copy-only, and track the exact native drag
session, including close/end reentrancy. Original files are preserved.

Two fresh immutable source reviews of the Unix revision-5 packet are clean.
Root activation preserves the verified full package and uses genuine base/next
merges. The ARM macOS native SDK check passes for this source; owned named-pasteboard
fixtures, full gestures and resource/hardware acceptance remain pending.
Source review and compilation are not native transfer/runtime proof.
Exact original-file hashes and patch evidence are retained under
`target/verification/unix-native-drop-dragout-held/checkpoints/unix-native-r5/`.

The platform also retains its Metal instance-buffer pool weakly; live renderers
and their in-flight callbacks keep the strong owners. Closing the last window
can release the pool, and reopening lazily recreates it. This adds no idle worker
or timer. The two final resident-lifecycle source reviews are clean; actual macOS
window/pool lifetime execution and whole-process resource acceptance remain
separate gates. The native drag image uses the typed NSString AppKit image name.
