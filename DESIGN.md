# screenshots design system

A native GPUI workbench with two appearances. The image is the work; the
interface helps it move from capture to annotation to explicit output.

## Appearances

| Appearance | Character | Geometry | UI fonts |
| --- | --- | --- | --- |
| Dark (default) | Neutral charcoal, grayscale controls, restrained borders | 6px controls / 8px panels | Segoe UI / system / Cantarell |
| Oldschool | Compact task menu, blue selection, colorful function icons | Square controls, 28px editor tools | Segoe UI / DejaVu Sans |

Dark is the startup and settings fallback. Retired or unknown saved values fall
back to Dark; an explicit Oldschool choice remains Oldschool. Both options share
the same capture, history, editor and output behavior. UI appearance never changes
annotation colors, masks or exported pixels.

## Structure

- Sidebar: capture command, Capture menu, Recent capture, Library, Capture
  sources, Uploads and Settings.
- Dark uses a simple empty state and a compact capture command list. No slogan,
  ornamental viewfinder, gradients or animated decoration.
- Oldschool keeps dense task navigation and the shortcut-based empty workspace.
- Library: search, filters, selection actions, bounded rows and pagination.
- Settings: Appearance, Storage, optional diagnostics.
- Editor: explicit output choices, grouped tools, contextual controls.

## Tokens and interaction

`crates/app-ui/src/theme.rs` owns native palette, geometry and font tokens. Active
appearance is one GPUI global enum; changes invalidate windows once. No webview,
font downloads, dependency, permanent timer, cache or polling loop is added.
Settings save on the existing worker executor and roll back on failure.

Use readable native type, a 4px spacing scale, short command labels, immediate
keyboard focus and visible selected state. Keep independent copy/save/upload
outcomes truthful. Navigation and theme changes do not trigger capture, clipboard
writes or uploads. Preserve existing cancellation, paging and pixel ownership.

## Window sizes

The shell opens at 1060×760 and supports 680×480. Content keeps its natural height
and scrolls, while complete controls wrap. The standalone editor retains its
760×400 minimum and bounded scrolling toolbar. This desktop app does not claim
mobile support.
