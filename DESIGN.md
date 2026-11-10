# screenshots design system

A native GPUI workbench. The image is the work; the interface should help it move
from capture to annotation to an explicit output. Keep the existing core, workers,
bounded previews, cancellation and privacy guarantees.

## Default selection

Oldschool is the startup and settings fallback. Light table and Safelight are
optional appearances. The historical serialized `default` value remains the
light palette, now labelled Light table, so explicit preferences do not change.

## Visual idea

The capture workspace is a digital light table: paper-like surfaces, graphite
text and petrol controls. A four-corner viewfinder marks the region-capture area.
Safelight translates a photographic darkroom into warm paper text and deep
red-brown surfaces with coral controls. These are custom palettes for this tool,
not imported preset themes. Oldschool combines compact task navigation, charcoal surfaces and colorful function icons.

## Structure and hierarchy

- A stable sidebar holds the primary capture command, Recent capture, Library,
  Capture sources, Uploads and Settings. Capture menu holds less frequent modes.
- One workspace per destination. Hidden pages do not render or fetch.
- Recent capture: one main region command, followed by a compact command list.
  A finished image replaces the empty state with its preview and output actions.
- Library: search, filters, selection actions, bounded image rows and pagination.
- Settings: Appearance, Storage, then optional diagnostics.
- Editor: output actions, grouped annotation tools, then contextual controls.

Oldschool uses the same commands and destinations in a denser task menu, with a
shortcut-based empty workspace. Light table and Safelight retain the larger capture
workspace. Keep command behavior and keyboard shortcuts consistent across themes. Do not add marketing sections, fake
metrics, browser frames, ornamental artwork, or empty destinations.

## Native tokens

`crates/app-ui/src/theme.rs` is the single source for palettes, geometry and UI
font selection. `Palette` is copied into render/paint closures. Active appearance
lives in GPUI app state, not a process-wide mutable static. Theme changes invalidate
windows once. No CSS, webview, theme framework or remotely loaded font is needed.

| Theme | Character | Geometry | UI fonts |
| --- | --- | --- | --- |
| Light table | Bright light-table surfaces, graphite text, deep petrol actions | 8px controls / 12px panels | Segoe UI / system / Cantarell |
| Oldschool | Compact desktop: charcoal, blue selection, colorful function icons | Square, compact rows and 28px tool buttons | Segoe UI / DejaVu Sans |
| Safelight | Red-brown surfaces, warm paper text, coral actions | 4px controls / 8px panels | Segoe UI / system / Cantarell |

Annotation palettes, masks, source pixels and export colors never follow these
tokens. Changing appearance must not change the document.

## Typography and space

Use native readable body text, medium-weight commands and semibold section
headings. Roman headings, no uppercase section kickers. Follow existing GPUI
spacing helpers on a 4px scale. Short command labels stay on one line. Source
filenames may ellipsize; content and status can wrap.

## Interaction

- Primary capture/output actions have filled accent controls with their own
  high-contrast foreground token. Secondary commands stay quieter.
- Selection has both a border and a visible check/state, with accessibility state.
- Keyboard focus appears immediately. No permanent animation or polling timer.
- Theme selection applies optimistically, disables competing settings writes,
  saves on the existing worker executor and rolls back if saving fails.
- Preserve independent save/copy/upload outcomes; do not imply success early.
- No automatic capture, clipboard writes or uploads on navigation/theme changes.

## Window sizes

The shell opens at 1060×760 and supports 680×480. Major workspace sections do
not shrink below their measured content. The sidebar and main workspace scroll
independently. Theme choices wrap; output/selection controls wrap as complete
controls. The editor retains its existing 760×400 standalone minimum and bounded
scrolling toolbar. This desktop app does not claim mobile support.

## Review

Native adaptation of the frontend-design / Hallmark principles: hierarchy,
consistent semantic tokens, real states, high contrast and restrained decoration.
Web-only CSS/export/mobile rules do not apply to this GPUI implementation.
