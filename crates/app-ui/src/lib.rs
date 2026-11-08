//! GPUI views and window shell. No capture or database implementation.

use gpui::{
    App, Bounds, Context, IntoElement, Render, TitlebarOptions, Window, WindowBounds, WindowOptions,
    div, prelude::*, px, rgb, size,
};

pub struct AppShell;

impl Render for AppShell {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        div()
            .flex()
            .size_full()
            .items_center()
            .justify_center()
            .bg(rgb(0x1a1a1a))
            .text_color(rgb(0xf5f5f5))
            .child("screenshots")
    }
}

pub fn run() {
    tracing::info!(version = app_core::version(), "starting screenshots");

    gpui_platform::application().run(|cx: &mut App| {
        let bounds = Bounds::centered(None, size(px(960.0), px(640.0)), cx);
        cx.open_window(
            WindowOptions {
                window_bounds: Some(WindowBounds::Windowed(bounds)),
                titlebar: Some(TitlebarOptions {
                    title: Some("screenshots".into()),
                    ..Default::default()
                }),
                ..Default::default()
            },
            |_, cx| cx.new(|_| AppShell),
        )
        .expect("failed to open window");
        cx.activate(true);
    });
}
