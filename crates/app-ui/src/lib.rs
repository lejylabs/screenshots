//! Phase 0 spike: prove GPUI can host a capture overlay on Windows.
//!
//! A transparent, frameless, always-on-top surface per display is the
//! region-selection path. If this window cannot cover a monitor, stop
//! and revisit the UI toolkit before Phase 1.

use gpui::{
    App, Bounds, Context, DisplayId, FocusHandle, Focusable, IntoElement, KeyBinding, Pixels,
    TitlebarOptions, Window, WindowBackgroundAppearance, WindowBounds, WindowHandle, WindowKind,
    WindowOptions, actions, div, point, prelude::*, px, rgb, rgba, size,
};

actions!(spike, [OpenProbeOverlays, OpenFullscreenOverlays, CloseOverlay, Quit]);

#[derive(Clone, Copy, Debug)]
enum OverlayMode {
    Probe,
    Fullscreen,
}

pub struct SpikeShell {
    focus_handle: FocusHandle,
    overlays: Vec<WindowHandle<Overlay>>,
}

impl SpikeShell {
    fn new(cx: &mut Context<Self>) -> Self {
        Self {
            focus_handle: cx.focus_handle(),
            overlays: Vec::new(),
        }
    }

    fn live_overlay_count(&self, cx: &App) -> usize {
        self.overlays.iter().filter(|handle| handle.read(cx).is_ok()).count()
    }

    fn close_overlays(&mut self, cx: &mut Context<Self>) {
        tracing::info!(count = self.overlays.len(), "closing overlays");
        for handle in self.overlays.drain(..) {
            match handle.update(cx, |_, window, _| window.remove_window()) {
                Ok(()) => {}
                Err(error) => tracing::warn!(?error, "overlay handle already gone"),
            }
        }
        cx.notify();
    }

    fn open_overlays(&mut self, mode: OverlayMode, cx: &mut Context<Self>) {
        self.close_overlays(cx);

        let screens: Vec<(DisplayId, Bounds<Pixels>)> = cx
            .displays()
            .iter()
            .map(|display| (display.id(), display.bounds()))
            .collect();

        for (display_id, display_bounds) in screens {
            tracing::info!(
                display_id = u64::from(display_id),
                mode = ?mode,
                origin_x = %display_bounds.origin.x,
                origin_y = %display_bounds.origin.y,
                width = %display_bounds.size.width,
                height = %display_bounds.size.height,
                "opening overlay"
            );

            match cx.open_window(
                overlay_options(mode, display_id, display_bounds),
                |window, cx| {
                    tracing::info!(
                        display_id = u64::from(display_id),
                        mode = ?mode,
                        scale = window.scale_factor(),
                        actual_origin_x = %window.bounds().origin.x,
                        actual_origin_y = %window.bounds().origin.y,
                        actual_width = %window.bounds().size.width,
                        actual_height = %window.bounds().size.height,
                        "overlay window ready"
                    );
                    window.on_window_should_close(cx, move |_, _| {
                        tracing::warn!(
                            display_id = u64::from(display_id),
                            mode = ?mode,
                            "native WM_CLOSE on overlay; refusing so the spike stays open"
                        );
                        false
                    });
                    cx.new(|cx| Overlay::new(display_id, display_bounds, mode, cx))
                },
            ) {
                Ok(handle) => self.overlays.push(handle),
                Err(error) => tracing::error!(?error, "failed to open overlay window"),
            }
        }

        cx.notify();
    }
}

impl Focusable for SpikeShell {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl Render for SpikeShell {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let displays: Vec<(u64, Bounds<Pixels>)> = cx
            .displays()
            .iter()
            .map(|display| (u64::from(display.id()), display.bounds()))
            .collect();
        let live_count = self.live_overlay_count(cx);
        let handle_count = self.overlays.len();

        div()
            .id("spike-shell")
            .track_focus(&self.focus_handle)
            .key_context("SpikeShell")
            .flex()
            .flex_col()
            .size_full()
            .p_6()
            .gap_4()
            .bg(rgb(0x1a1a1a))
            .text_color(rgb(0xf5f5f5))
            .on_action(cx.listener(|this, _: &OpenProbeOverlays, _, cx| {
                this.open_overlays(OverlayMode::Probe, cx);
            }))
            .on_action(cx.listener(|this, _: &OpenFullscreenOverlays, _, cx| {
                this.open_overlays(OverlayMode::Fullscreen, cx);
            }))
            .on_action(cx.listener(|this, _: &CloseOverlay, _, cx| {
                tracing::info!("CloseOverlay on spike shell");
                this.close_overlays(cx);
            }))
            .on_action(|_: &Quit, _, cx| cx.quit())
            .child("Phase 0 — overlay spike")
            .child(
                div()
                    .text_sm()
                    .text_color(rgb(0xa3a3a3))
                    .child("Probe: small red PopUp at the top-left of each display. Fullscreen: blue tint covering the display. Escape or Close overlays to dismiss."),
            )
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap_1()
                    .text_sm()
                    .children(displays.into_iter().map(|(id, bounds)| {
                        div().child(format!(
                            "display {id}  origin=({}, {})  size={}×{}",
                            bounds.origin.x,
                            bounds.origin.y,
                            bounds.size.width,
                            bounds.size.height
                        ))
                    })),
            )
            .child(
                div()
                    .text_sm()
                    .text_color(rgb(0xa3a3a3))
                    .child(format!(
                        "{live_count} live overlay(s), {handle_count} handle(s)"
                    )),
            )
            .child(
                div()
                    .flex()
                    .gap_2()
                    .child(
                        div()
                            .id("open-probe")
                            .px_3()
                            .py_2()
                            .rounded_md()
                            .bg(rgb(0x2563eb))
                            .cursor_pointer()
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.open_overlays(OverlayMode::Probe, cx);
                            }))
                            .child("Open small overlay"),
                    )
                    .child(
                        div()
                            .id("open-fullscreen")
                            .px_3()
                            .py_2()
                            .rounded_md()
                            .bg(rgb(0x2563eb))
                            .cursor_pointer()
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.open_overlays(OverlayMode::Fullscreen, cx);
                            }))
                            .child("Open fullscreen overlay"),
                    )
                    .child(
                        div()
                            .id("close-overlays")
                            .px_3()
                            .py_2()
                            .rounded_md()
                            .bg(rgb(0x3f3f46))
                            .cursor_pointer()
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.close_overlays(cx);
                            }))
                            .child("Close overlays"),
                    ),
            )
    }
}

struct Overlay {
    display_id: DisplayId,
    bounds: Bounds<Pixels>,
    mode: OverlayMode,
    focus_handle: FocusHandle,
}

impl Overlay {
    fn new(
        display_id: DisplayId,
        bounds: Bounds<Pixels>,
        mode: OverlayMode,
        cx: &mut Context<Self>,
    ) -> Self {
        Self {
            display_id,
            bounds,
            mode,
            focus_handle: cx.focus_handle(),
        }
    }
}

impl Drop for Overlay {
    fn drop(&mut self) {
        tracing::info!(
            display_id = u64::from(self.display_id),
            mode = ?self.mode,
            "overlay view dropped"
        );
    }
}

impl Focusable for Overlay {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl Render for Overlay {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let scale = window.scale_factor();
        let id = u64::from(self.display_id);
        let (label, fill) = match self.mode {
            OverlayMode::Probe => ("small PopUp", rgba(0xdc2626ff)),
            OverlayMode::Fullscreen => ("fullscreen PopUp", rgba(0x1d4ed8cc)),
        };

        div()
            .id("overlay")
            .track_focus(&self.focus_handle)
            .key_context("Overlay")
            .flex()
            .size_full()
            .items_start()
            .justify_start()
            .bg(fill)
            .text_color(rgb(0xffffff))
            .on_action(cx.listener(|this, _: &CloseOverlay, window, _| {
                tracing::info!(
                    display_id = u64::from(this.display_id),
                    mode = ?this.mode,
                    "CloseOverlay on overlay"
                );
                window.remove_window();
            }))
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap_2()
                    .m_6()
                    .p_6()
                    .rounded_lg()
                    .bg(rgb(0x111827))
                    .child(format!("{label}  display {id}"))
                    .child(
                        div().text_sm().child(format!(
                            "display origin=({}, {})  size={}×{}  scale={scale:.2}",
                            self.bounds.origin.x,
                            self.bounds.origin.y,
                            self.bounds.size.width,
                            self.bounds.size.height
                        )),
                    )
                    .child(
                        div()
                            .text_sm()
                            .text_color(rgb(0xa3a3a3))
                            .child("This window must stay open. Escape or Close overlays to dismiss."),
                    ),
            )
    }
}

fn overlay_options(
    mode: OverlayMode,
    display_id: DisplayId,
    display: Bounds<Pixels>,
) -> WindowOptions {
    // Match gpui's window_positioning example: no titlebar, no client
    // decorations, PopUp (WS_EX_TOOLWINDOW | WS_EX_TOPMOST), no focus steal.
    let bounds = match mode {
        OverlayMode::Probe => Bounds {
            origin: display.origin + point(px(48.0), px(48.0)),
            size: size(px(420.0), px(180.0)),
        },
        OverlayMode::Fullscreen => {
            let inset = px(16.0);
            Bounds {
                origin: display.origin + point(inset, inset),
                size: size(
                    (display.size.width - inset * 2.0).max(px(320.0)),
                    (display.size.height - inset * 2.0).max(px(240.0)),
                ),
            }
        }
    };

    WindowOptions {
        window_bounds: Some(WindowBounds::Windowed(bounds)),
        display_id: Some(display_id),
        titlebar: None,
        window_background: WindowBackgroundAppearance::Transparent,
        focus: false,
        show: true,
        kind: WindowKind::PopUp,
        is_movable: false,
        is_resizable: false,
        is_minimizable: false,
        window_min_size: None,
        window_decorations: None,
        tabbing_identifier: None,
        ..Default::default()
    }
}

pub fn run() {
    tracing::info!(version = app_core::version(), "starting overlay spike");

    gpui_platform::application().run(|cx: &mut App| {
        cx.on_action(|_: &Quit, cx| cx.quit());
        cx.bind_keys([
            KeyBinding::new("escape", CloseOverlay, None),
            KeyBinding::new("ctrl-shift-o", OpenProbeOverlays, Some("SpikeShell")),
            KeyBinding::new("ctrl-shift-f", OpenFullscreenOverlays, Some("SpikeShell")),
            KeyBinding::new("ctrl-q", Quit, None),
        ]);

        let bounds = Bounds::centered(None, size(px(720.0), px(460.0)), cx);
        cx.open_window(
            WindowOptions {
                window_bounds: Some(WindowBounds::Windowed(bounds)),
                titlebar: Some(TitlebarOptions {
                    title: Some("screenshots — overlay spike".into()),
                    ..Default::default()
                }),
                ..Default::default()
            },
            |window, cx| {
                let shell = cx.new(SpikeShell::new);
                let focus = shell.read(cx).focus_handle.clone();
                window.focus(&focus, cx);
                shell
            },
        )
        .expect("failed to open spike window");
        cx.activate(true);
    });
}
