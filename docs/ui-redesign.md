# Native UI redesign

The Rust/GPUI workspace offers **Dark** as the minimal default and **Oldschool**
as a compact alternative. Both reuse the existing capture/editor core, workers,
bounded previews, history, cancellation and explicit output paths.

## Behavior

- Dark uses neutral charcoal surfaces, grayscale controls, small corner radii
  and a simple capture empty state. Decorative viewfinder artwork and slogans
  are removed.
- Oldschool retains its compact 180px task sidebar, colorful function icons,
  blue selection, square controls and 28px editor tools.
- Settings show exactly two appearances. Missing, retired or unknown saved
  theme values use Dark; explicit Oldschool preferences survive restart.
- Theme changes apply immediately, persist off the foreground executor and
  restore the previous appearance if saving fails.
- Annotation colors and exported pixels remain independent of appearance.
- Search, keyboard page navigation and editor return preserve their existing
  focus/cancellation behavior. All app-owned UI text is English.

## Screenshots

| Dark (default) | Oldschool |
| --- | --- |
| ![Dark workspace](design/dark.png) | ![Oldschool workspace](design/oldschool.png) |

The native editor uses an isolated synthetic image.

![Native editor](design/editor.png)

## Verification and limits

Settings fixtures cover fresh configs, missing/unknown/retired values and explicit
preference round-trips. Contrast fixtures cover text, actions, icons and focus.
Local workspace tests, release build and native layout checks are recorded in
[ui-redesign-verification.json](ui-redesign-verification.json).

Native inspection uses Linux X11 via XWayland, isolated XDG directories and only
app-owned windows or a synthetic image. Windows/macOS interaction, mixed DPI,
screen readers and complete reference-hardware CPU/RAM/GPU/latency acceptance
remain open. A successful build does not establish those platform guarantees.

This change adds no dependency, worker pool, image cache, background polling or
idle timer. Appearance lives in a small GPUI global; paint closures copy a fixed
palette. See [DESIGN.md](../DESIGN.md) for the shared system and PLAN.md for the
performance contract.
