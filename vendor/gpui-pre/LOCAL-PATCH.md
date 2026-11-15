# Maintained SVG/font dependencies

Based on published gpui-pre 0.3.12. Source and Apache-2.0 license retained.
Update resvg/usvg 0.46 to 0.50, which uses maintained Harfrust/Skrifa rather
than abandoned rustybuzz. Replace the sole direct ttf-parser glyph-presence
query with Skrifa's cmap lookup, retaining missing-glyph rejection. No GUI
layout, event loop, graphics backend or eager runtime is changed.
Review SVG text, embedded icons and emoji fallback and measure resources;
dependency resolution alone is not verification.
