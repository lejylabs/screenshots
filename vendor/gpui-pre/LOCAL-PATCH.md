# Maintained SVG/font dependencies

Based on published gpui-pre 0.3.12. Source and Apache-2.0 license retained.
Update resvg/usvg 0.46 to 0.50, which uses maintained Harfrust/Skrifa rather
than abandoned rustybuzz. Replace the sole direct ttf-parser glyph-presence
query with Skrifa's cmap lookup, retaining missing-glyph rejection. The dependency migration retains GUI layout and graphics backend behavior.
Review SVG text, embedded icons and emoji fallback and measure resources;
dependency resolution alone is not verification.

## Clipboard/drop platform seams

The local platform/window/event interfaces additionally expose focused encoded
clipboard input, bounded pending native drop payloads, cancellation and test
hooks used by existing Paste/Drop actions. These are functional input changes;
no new controls, visual design or eager runtime is added. Native role/lease and
private-context checks are described in `docs/clipboard-import-output.md`.

## Standalone test fixtures

The published package omits the workspace fonts and `util_macros::perf` used by
its unit tests. Test-only font includes now use local test-assets containing the
exact IBM Plex Sans and Lilex files and original OFL texts from pinned Zed revision
`d2a4fb6ded7c470be25d1541281da3882495b65b`; Git blob identities were verified
against that upstream revision. They are not loaded into production. The three
style assertion functions use standard `#[test]` instead of the unavailable
workspace performance attribute, preserving their complete assertions.

## Chrono maintenance migration (2026-11-17)

Replace the unmaintained Chrono dependency with Jiff 0.2.43, default features
disabled (`std` plus browser `js` clock support). Calendar/date/time public Rust
types use Jiff civil types, and scheduler clocks use Jiff Timestamp. UI layout,
controls, events and the existing month-grid algorithms stay intact. GPUI's
on-demand local conversion uses native OS timezone/DST rules and a fixed-offset
Jiff value; it owns no timezone database/cache, refresh timer, worker or runtime.
The existing Apache-2.0 source/license and earlier local patches remain intact.

Activation, optimized regression tests, five native-target metadata checks and
default advisory-policy verification belong to the root integration gate; this
held source packet alone establishes none of those passing results.
