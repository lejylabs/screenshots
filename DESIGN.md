# screenshots design system

A native GPUI Kit workbench with three appearances. The image is the work; the
interface helps it move from capture to annotation to explicit output.

## Appearances

| Appearance | Character | Geometry | UI fonts |
| --- | --- | --- | --- |
| Dark (default) | Neutral charcoal, grayscale controls, restrained borders | 6px controls / 8px panels | Segoe UI / system / Cantarell |
| Oldschool | Compact task menu, blue selection, colorful function icons | Square controls, Kit small tools | Segoe UI / DejaVu Sans |
| Classic | One Dark workbench: Kit TitleBar, pane tabs, 240px dock, segmented status bar; editor `#282c33`, panels `#2f343e`, accent `#74ade8` | 4px controls / 4px panels, 33/32/24 chrome, 16px rem / 14px body | Segoe UI / system / Cantarell |

Dark is the startup and settings fallback. Retired or unknown saved values fall
back to Dark; an explicit Oldschool or Classic choice remains that appearance. All
options share the same capture, history, editor and output behavior. UI appearance
never changes annotation colors, masks or exported pixels.

## Structure

- Sidebar: capture command, Capture menu, Recent capture, Library, Capture
  sources, Uploads and Settings.
- Dark uses a simple empty state and a compact capture command list. No slogan,
  ornamental viewfinder, gradients or animated decoration.
- Oldschool keeps dense task navigation and the shortcut-based empty workspace.
- Library: search, filters, selection actions, bounded rows and pagination.
- Settings: Appearance, Storage, optional diagnostics.
- Editor: explicit output choices, grouped tools, contextual controls.

## Components, tokens and interaction

GPUI Kit 0.9.1 owns standard control behavior. Use its Sidebar, Button, Input,
Checkbox, RadioGroup, PopupMenu, Kbd and TitleBar components for application
commands, forms and the window caption. Every native window mounts the Kit Base
Root. Decorated windows use `TitleBar::window_options()` so the caption follows
the appearance. Capture overlays stay undecorated. On Windows the tray popup
and other process-owned `HMENU` menus opt into dark mode with the current
appearance; Explorer jump lists still follow the system. The application retains
explicit typed access to the content inside that Root for native lifecycle jobs.

`theme.rs` applies product palettes through `Theme::update`, keeping styled tokens
and Base projections synchronized. Dark, Oldschool and Classic remain app preferences;
annotation colors are document data. Kit initialization is deferred until a UI
window is needed, preserving resident startup readiness. Optional script, speech,
syntax and profiling runtimes are not enabled.

The custom image canvas, annotation metadata and pre-insert-bounded source-label
editor remain domain code. Label raster fonts are independent of UI appearance.
Search uses retained Kit Input state, admits at most 512 UTF-8 bytes and releases
its input/subscription when the page closes. Menus dispatch revision-scoped
commands so a stale popup cannot affect a newly loaded page.

Use native relative spacing and component sizes, readable type, visible focus
and selected state. Keep copy/save/upload outcomes independent and truthful.
Navigation does not trigger capture, clipboard output or uploads. Preserve worker
ownership, cancellation, paged data and in-memory pixels. No webview, remote asset,
JavaScript engine, extra worker pool or permanent timer is introduced.

## Window sizes

The shell opens at 1060×760 and supports 680×480. Content keeps its natural height
and scrolls, while complete controls wrap. The standalone editor retains its
760×400 minimum and bounded scrolling toolbar. This desktop app does not claim
mobile support.
