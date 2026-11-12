//! Native design tokens. A theme change invalidates windows once; idle does no work.
//! Hallmark · workbench · modern-minimal · native GPUI tokens, no web layer.
use app_core::UiTheme;
use gpui::component::{Theme as KitTheme, ThemeMode};
use gpui::{App, Global, px, rgb};

use gpui_kit as gpui;
impl Global for ActiveTheme {}
#[derive(Default)]
struct ActiveTheme(UiTheme);

#[derive(Clone, Copy)]
#[non_exhaustive]
pub struct Palette {
    pub classic: bool,
    pub background: u32,
    pub surface: u32,
    pub control: u32,
    pub hover: u32,
    pub pressed: u32,
    pub border: u32,
    pub text: u32,
    pub muted: u32,
    pub icon: u32,
    pub accent: u32,
    pub primary: u32,
    pub primary_hover: u32,
    pub primary_pressed: u32,
    pub primary_text: u32,
    pub focus: u32,
    pub selection: u32,
    pub selection_fill: u32,
    pub radius: f32,
    pub panel_radius: f32,
}

pub fn selected(cx: &App) -> UiTheme {
    cx.try_global::<ActiveTheme>()
        .map(|theme| theme.0)
        .unwrap_or_default()
}
pub fn current(cx: &App) -> Palette {
    palette(selected(cx))
}
struct KitReady;
impl Global for KitReady {}

pub fn init(cx: &mut App) {
    if cx.has_global::<KitReady>() {
        return;
    }
    gpui::init(cx);
    gpui::component::set_locale("en");
    KitTheme::change(ThemeMode::Dark, None, cx);
    cx.set_global(KitReady);
    sync_kit(cx);
}

pub fn apply(theme: UiTheme, cx: &mut App) {
    cx.set_global(ActiveTheme(theme));
    if cx.has_global::<KitReady>() {
        sync_kit(cx);
    } else {
        cx.refresh_windows();
    }
}

fn sync_kit(cx: &mut App) {
    let p = current(cx);
    let font = font(cx);
    KitTheme::update(cx, |theme| {
        theme.font_family = font.into();
        theme.font_size = px(16.);
        theme.radius = px(p.radius);
        let c = &mut theme.colors;
        c.background = rgb(p.background).into();
        c.foreground = rgb(p.text).into();
        c.border = rgb(p.border).into();
        c.accent = rgb(p.selection).into();
        c.accent_foreground = rgb(p.text).into();
        c.muted = rgb(p.control).into();
        c.muted_foreground = rgb(p.muted).into();
        c.button = rgb(p.control).into();
        c.button_foreground = rgb(p.text).into();
        c.button_hover = rgb(p.hover).into();
        c.button_active = rgb(p.pressed).into();
        c.primary = rgb(p.primary).into();
        c.primary_foreground = rgb(p.primary_text).into();
        c.primary_hover = rgb(p.primary_hover).into();
        c.primary_active = rgb(p.primary_pressed).into();
        c.button_primary = c.primary;
        c.button_primary_foreground = c.primary_foreground;
        c.button_primary_hover = c.primary_hover;
        c.button_primary_active = c.primary_active;
        c.secondary = c.button;
        c.secondary_foreground = c.button_foreground;
        c.secondary_hover = c.button_hover;
        c.secondary_active = c.button_active;
        c.button_secondary = c.secondary;
        c.button_secondary_foreground = c.secondary_foreground;
        c.button_secondary_hover = c.secondary_hover;
        c.button_secondary_active = c.secondary_active;
        c.group_box = rgb(p.surface).into();
        c.group_box_foreground = c.foreground;
        c.popover = rgb(p.surface).into();
        c.popover_foreground = c.foreground;
        c.sidebar = c.popover;
        c.sidebar_foreground = c.foreground;
        c.sidebar_border = c.border;
        c.sidebar_accent = c.accent;
        c.sidebar_accent_foreground = c.foreground;
        c.sidebar_primary = c.primary;
        c.sidebar_primary_foreground = c.primary_foreground;
        c.list = c.background;
        c.list_active = c.accent;
        c.list_active_border = c.border;
        c.list_hover = c.button_hover;
        c.list_even = c.background;
        c.input = c.border;
        c.ring = rgb(p.focus).into();
        c.caret = rgb(p.accent).into();
        c.selection = rgb(p.selection).into();
    });
}

pub fn palette(theme: UiTheme) -> Palette {
    match theme {
        UiTheme::Default => Palette {
            classic: false,
            background: 0x141414,
            surface: 0x1b1b1b,
            control: 0x252525,
            hover: 0x323232,
            pressed: 0x171717,
            border: 0x3a3a3a,
            text: 0xededed,
            muted: 0xb0b0b0,
            icon: 0xc4c4c4,
            accent: 0xe0e0e0,
            primary: 0xdedede,
            primary_hover: 0xf2f2f2,
            primary_pressed: 0xbcbcbc,
            primary_text: 0x151515,
            focus: 0xfafafa,
            selection: 0x303030,
            selection_fill: 0xffffff33,
            radius: 6.,
            panel_radius: 8.,
        },
        UiTheme::Oldschool => Palette {
            classic: true,
            background: 0x282d35,
            surface: 0x282d35,
            control: 0x353c47,
            hover: 0x404b5b,
            pressed: 0x20252c,
            border: 0x535f70,
            text: 0xf0f2f5,
            muted: 0xc2cad6,
            icon: 0xdce2eb,
            accent: 0x97c4ff,
            primary: 0x365f91,
            primary_hover: 0x406fa5,
            primary_pressed: 0x294a74,
            primary_text: 0xffffff,
            focus: 0xc0dbff,
            selection: 0x334e70,
            selection_fill: 0x97c4ff44,
            radius: 0.,
            panel_radius: 0.,
        },
    }
}

impl Palette {
    pub fn icon_color(self, id: &str) -> u32 {
        if !self.classic {
            return self.icon;
        }
        match id {
            "nav-library" | "sidebar-folder" | "tool-highlight" | "tool-number" => 0xe9c86a,
            "nav-sources" | "capture-primary" | "capture-all" | "tool-pen" | "tool-ellipse" => {
                0x9bd28e
            }
            "sidebar-quit" | "tool-frame" | "tool-arrow" | "tool-line" | "tool-redact" => 0xffa69b,
            "nav-settings" | "history-more-actions" | "tool-select" => self.icon,
            _ => 0x97c4ff,
        }
    }
}

pub fn icon_color(cx: &App, id: &str) -> gpui::Hsla {
    rgb(current(cx).icon_color(id)).into()
}

pub fn name(theme: UiTheme) -> &'static str {
    match theme {
        UiTheme::Default => "Dark",
        UiTheme::Oldschool => "Oldschool",
    }
}
pub fn font(cx: &App) -> &'static str {
    match selected(cx) {
        UiTheme::Oldschool => {
            if cfg!(target_os = "windows") {
                "Segoe UI"
            } else if cfg!(target_os = "macos") {
                ".SystemUIFont"
            } else {
                "Cantarell"
            }
        }
        _ => {
            if cfg!(target_os = "windows") {
                "Segoe UI"
            } else if cfg!(target_os = "macos") {
                ".SystemUIFont"
            } else {
                "Cantarell"
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn luminance(color: u32) -> f64 {
        let channel = |shift: u32| {
            let c = ((color >> shift) & 255u32) as f64 / 255.;
            if c <= 0.04045 {
                c / 12.92
            } else {
                ((c + 0.055) / 1.055).powf(2.4)
            }
        };
        0.2126 * channel(16) + 0.7152 * channel(8) + 0.0722 * channel(0)
    }
    fn contrast(a: u32, b: u32) -> f64 {
        let (a, b) = (luminance(a), luminance(b));
        (a.max(b) + 0.05) / (a.min(b) + 0.05)
    }
    #[test]
    fn themes_keep_text_actions_and_focus_legible() {
        for theme in [UiTheme::Default, UiTheme::Oldschool] {
            let p = palette(theme);
            for bg in [p.background, p.surface, p.control, p.hover, p.selection] {
                for fg in [p.text, p.muted] {
                    assert!(contrast(fg, bg) >= 4.5, "{theme:?}: {fg:x} on {bg:x}");
                }
                assert!(contrast(p.focus, bg) >= 3., "{theme:?}: focus");
                for id in [
                    "nav-library",
                    "nav-sources",
                    "sidebar-quit",
                    "nav-settings",
                    "select-region",
                ] {
                    assert!(contrast(p.icon_color(id), bg) >= 3., "{theme:?}: icon {id}");
                }
            }
            for bg in [p.primary, p.primary_hover, p.primary_pressed] {
                assert!(
                    contrast(p.primary_text, bg) >= 4.5,
                    "{theme:?}: primary action"
                );
            }
        }
    }
}
