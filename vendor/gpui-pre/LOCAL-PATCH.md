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
