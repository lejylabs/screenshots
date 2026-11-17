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
merges. Native compilation, owned named-pasteboard fixtures, full gestures and
resource/hardware acceptance remain pending; source review is not runtime proof.
Exact original-file hashes and patch evidence are retained under
`target/verification/unix-native-drop-dragout-held/checkpoints/unix-native-r5/`.
