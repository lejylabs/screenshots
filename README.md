# screenshots

Open-source desktop capture and sharing application.

## Workspace

```text
apps/desktop      GPUI binary
crates/app-ui     Windows and views
crates/app-core   Application logic, no GPUI
crates/shared     Shared types
```

## Build

Requires Rust 1.99.

```bash
cargo build -p screenshots
cargo run -p screenshots
```

The first run is the Phase 0 overlay spike: a control window plus always-on-top PopUp surfaces on every display. Open the small probe first (red card, top-left). Escape or Close overlays dismisses them.
