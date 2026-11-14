//! Native design tokens. A theme change invalidates windows once; idle does no work.
//! Hallmark · workbench · modern-minimal · native GPUI tokens, no web layer.
use app_core::UiTheme;
use gpui::component::{Theme as KitTheme, ThemeColor, ThemeMode, highlighter::HighlightTheme};
use gpui::{App, Global, Hsla, px, rgb, rgba};
use std::sync::Arc;

use gpui_kit as gpui;
impl Global for ActiveTheme {}
#[derive(Default)]
struct ActiveTheme(UiTheme);

#[derive(Clone, Copy)]
#[non_exhaustive]
pub struct Palette {
    pub classic: bool,
    /// Classic appearance: Zed-like workbench chrome (33px titlebar, tabs, status, density).
    pub workbench: bool,
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

pub const APPEARANCES: [UiTheme; 3] = [UiTheme::Default, UiTheme::Oldschool, UiTheme::Classic];

pub fn appearance_index(theme: UiTheme) -> usize {
    match theme {
        UiTheme::Default => 0,
        UiTheme::Oldschool => 1,
        UiTheme::Classic => 2,
    }
}

pub fn appearance_at(index: usize) -> UiTheme {
    APPEARANCES.get(index).copied().unwrap_or_default()
}

fn color(hex: u32) -> Hsla {
    rgb(hex).into()
}

fn sync_kit(cx: &mut App) {
    let selected = selected(cx);
    let p = palette(selected);
    let font = font(cx);
    // Reload Kit Default Dark first so Classic chrome tokens do not leak into Dark/Oldschool.
    KitTheme::change(ThemeMode::Dark, None, cx);
    KitTheme::update(cx, |theme| {
        theme.font_family = font.into();
        theme.font_size = px(if p.workbench { 14. } else { 16. });
        theme.radius = px(p.radius);
        theme.radius_lg = px(p.panel_radius);
        apply_product_colors(&mut theme.colors, p);
        if selected == UiTheme::Classic {
            apply_classic_chrome(theme, p);
        } else {
            // Kit::change reloads colors, not window_radius / highlight.
            theme.window_radius = px(0.);
            theme.highlight_theme = HighlightTheme::default_dark();
        }
    });
}

fn apply_product_colors(c: &mut ThemeColor, p: Palette) {
    c.background = color(p.background);
    c.foreground = color(p.text);
    c.border = color(p.border);
    c.accent = color(p.selection);
    c.accent_foreground = color(p.text);
    c.muted = color(p.control);
    c.muted_foreground = color(p.muted);
    c.button = color(p.control);
    c.button_foreground = color(p.text);
    c.button_hover = color(p.hover);
    c.button_active = color(p.pressed);
    c.primary = color(p.primary);
    c.primary_foreground = color(p.primary_text);
    c.primary_hover = color(p.primary_hover);
    c.primary_active = color(p.primary_pressed);
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
    c.group_box = color(p.surface);
    c.group_box_foreground = c.foreground;
    c.popover = color(p.surface);
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
    c.ring = color(p.focus);
    c.caret = color(p.accent);
    c.selection = color(p.selection);
    c.title_bar = color(p.surface);
    c.title_bar_border = color(p.border);
}

/// One Dark chrome tokens from zed-industries/zed `assets/themes/one/one.json`.
fn apply_classic_chrome(theme: &mut KitTheme, p: Palette) {
    theme.radius_lg = px(p.panel_radius);
    theme.window_radius = px(10.);
    theme.shadow = true;
    theme.focus_ring = true;
    // Keep Kit Default Dark syntax/status tokens, then overlay One Dark editor chrome.
    let mut highlight = HighlightTheme::default_dark().as_ref().clone();
    highlight.name = "Classic".into();
    highlight.appearance = ThemeMode::Dark;
    highlight.style.editor_background = Some(color(0x282c33));
    highlight.style.editor_foreground = Some(color(0xacb2be));
    highlight.style.editor_active_line = Some(rgba(0x2f343ebf).into());
    highlight.style.editor_line_number = Some(color(0x4e5a5f));
    highlight.style.editor_active_line_number = Some(color(0xd0d4da));
    highlight.style.editor_gutter_background = Some(color(0x282c33));
    highlight.style.editor_invisible = Some(color(0x4e5a5f));
    theme.highlight_theme = Arc::new(highlight);
    apply_classic_kit_colors(&mut theme.colors, p);
}

fn apply_classic_kit_colors(c: &mut ThemeColor, p: Palette) {
    let text = color(p.text);
    let muted = color(p.muted);
    let border = color(p.border);
    let panel = color(p.surface);
    let editor = color(p.background);
    let hover = color(p.hover);
    let selected = color(p.selection);
    let accent = color(p.accent);
    let title = color(0x3b414d);
    let border_variant = color(0x363c46);
    let danger = color(0xd07277);
    let danger_hover = color(0xea858b);
    let danger_active = color(0xa7545a);
    let warning = color(0xdec184);
    let warning_hover = color(0xffd885);
    let warning_active = color(0xb8985b);
    let success = color(0xa1c181);
    let success_hover = color(0xaad581);
    let success_active = color(0x6d8f59);
    let info = accent;
    let info_hover = color(p.primary_hover);
    let info_active = color(p.primary_pressed);
    let on_fill = color(p.primary_text);
    c.accordion = panel;
    c.popover = panel;
    c.popover_foreground = text;
    c.group_box = panel;
    c.group_box_foreground = text;
    c.muted = color(p.control);
    c.muted_foreground = muted;
    c.ring = accent;
    c.caret = accent;
    c.accent = selected;
    c.accent_foreground = text;
    c.title_bar = title;
    c.title_bar_border = border;
    c.status_bar = title;
    c.status_bar_border = border;
    c.window_border = border;
    c.tab_bar = panel;
    c.tab_bar_segmented = panel;
    c.tab = panel;
    c.tab_foreground = muted;
    c.tab_active = editor;
    c.tab_active_foreground = text;
    c.sidebar = panel;
    c.sidebar_foreground = text;
    c.sidebar_border = border_variant;
    c.sidebar_accent = selected;
    c.sidebar_accent_foreground = text;
    c.sidebar_primary = accent;
    c.sidebar_primary_foreground = on_fill;
    c.list = editor;
    c.list_even = editor;
    c.list_head = panel;
    c.list_hover = hover;
    c.list_active = rgba(0x74ade83d).into();
    c.list_active_border = accent;
    c.table = editor;
    c.table_even = editor;
    c.table_head = panel;
    c.table_head_foreground = muted;
    c.table_foot = panel;
    c.table_foot_foreground = muted;
    c.table_hover = hover;
    c.table_active = c.list_active;
    c.table_active_border = accent;
    c.table_row_border = border_variant;
    c.input = border_variant;
    c.selection = rgba(p.selection_fill).into();
    c.drop_target = rgba(0x83899480).into();
    c.drag_border = accent;
    c.overlay = rgba(0x00000066).into();
    c.link = accent;
    c.link_hover = info_hover;
    c.link_active = info_active;
    c.description_list_label = panel;
    c.description_list_label_foreground = text;
    c.progress_bar = accent;
    c.skeleton = hover;
    c.slider_bar = accent;
    c.slider_thumb = text;
    c.switch = color(p.control);
    c.switch_thumb = text;
    c.scrollbar = rgba(0x282c3300).into();
    c.scrollbar_thumb = rgba(0xc8ccd44c).into();
    c.scrollbar_thumb_hover = hover;
    c.danger = danger;
    c.danger_foreground = on_fill;
    c.danger_hover = danger_hover;
    c.danger_active = danger_active;
    c.button_danger = danger;
    c.button_danger_foreground = on_fill;
    c.button_danger_hover = danger_hover;
    c.button_danger_active = danger_active;
    c.warning = warning;
    c.warning_foreground = on_fill;
    c.warning_hover = warning_hover;
    c.warning_active = warning_active;
    c.button_warning = warning;
    c.button_warning_foreground = on_fill;
    c.button_warning_hover = warning_hover;
    c.button_warning_active = warning_active;
    c.success = success;
    c.success_foreground = on_fill;
    c.success_hover = success_hover;
    c.success_active = success_active;
    c.button_success = success;
    c.button_success_foreground = on_fill;
    c.button_success_hover = success_hover;
    c.button_success_active = success_active;
    c.info = info;
    c.info_foreground = on_fill;
    c.info_hover = info_hover;
    c.info_active = info_active;
    c.button_info = info;
    c.button_info_foreground = on_fill;
    c.button_info_hover = info_hover;
    c.button_info_active = info_active;
    c.chart_1 = accent;
    c.chart_2 = success;
    c.chart_3 = warning;
    c.chart_4 = danger;
    c.chart_5 = color(0xb477cf);
    c.chart_bullish = color(0x27a657);
    c.chart_bearish = color(0xe06c76);
    c.chart_grid = rgba(0x464b5780).into();
    c.red = danger;
    c.red_light = danger_hover;
    c.green = success;
    c.green_light = success_hover;
    c.blue = accent;
    c.blue_light = info_hover;
    c.yellow = warning;
    c.yellow_light = warning_hover;
    c.magenta = color(0xb477cf);
    c.magenta_light = color(0xd398eb);
    c.cyan = color(0x6eb4bf);
    c.cyan_light = color(0x85c1ff);
}

pub fn palette(theme: UiTheme) -> Palette {
    match theme {
        UiTheme::Default => Palette {
            classic: false,
            workbench: false,
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
            workbench: false,
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
        // One Dark UI tokens from zed-industries/zed assets/themes/one/one.json.
        UiTheme::Classic => Palette {
            classic: false,
            workbench: true,
            background: 0x282c33,
            surface: 0x2f343e,
            control: 0x2e343e,
            hover: 0x363c46,
            pressed: 0x454a56,
            border: 0x464b57,
            text: 0xdce0e5,
            muted: 0xa9afbc,
            icon: 0xdce0e5,
            accent: 0x74ade8,
            primary: 0x74ade8,
            primary_hover: 0x85c1ff,
            primary_pressed: 0x5b9ad6,
            primary_text: 0x282c33,
            focus: 0x74ade8,
            selection: 0x293b5b,
            selection_fill: 0x74ade83d,
            radius: 4.,
            panel_radius: 4.,
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
        UiTheme::Classic => "Classic",
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
    fn classic_palette_matches_one_dark_tokens() {
        let p = palette(UiTheme::Classic);
        assert!(!p.classic);
        assert!(p.workbench);
        assert!(!palette(UiTheme::Default).workbench);
        assert!(!palette(UiTheme::Oldschool).workbench);
        assert_eq!(p.background, 0x282c33);
        assert_eq!(p.surface, 0x2f343e);
        assert_eq!(p.control, 0x2e343e);
        assert_eq!(p.hover, 0x363c46);
        assert_eq!(p.pressed, 0x454a56);
        assert_eq!(p.border, 0x464b57);
        assert_eq!(p.text, 0xdce0e5);
        assert_eq!(p.muted, 0xa9afbc);
        assert_eq!(p.accent, 0x74ade8);
        assert_eq!(p.primary, 0x74ade8);
        assert_eq!(p.focus, 0x74ade8);
        assert_eq!(p.selection, 0x293b5b);
        assert_eq!(p.selection_fill, 0x74ade83d);
        assert_eq!(p.radius, 4.);
        assert_eq!(p.panel_radius, 4.);
        assert_eq!(appearance_index(UiTheme::Classic), 2);
        assert_eq!(appearance_at(2), UiTheme::Classic);
        assert_eq!(appearance_at(99), UiTheme::Default);
        assert_eq!(name(UiTheme::Classic), "Classic");
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
        let p = palette(UiTheme::Classic);
        for bg in [p.background, p.surface, p.control] {
            for fg in [p.text, p.muted] {
                assert!(contrast(fg, bg) >= 4.5, "Classic: {fg:x} on {bg:x}");
            }
        }
        for bg in [p.background, p.surface, p.control, p.hover, p.selection] {
            assert!(contrast(p.text, bg) >= 4.5, "Classic: text on {bg:x}");
            assert!(contrast(p.focus, bg) >= 3., "Classic: focus on {bg:x}");
            assert!(contrast(p.icon, bg) >= 3., "Classic: icon on {bg:x}");
        }
        for bg in [p.primary, p.primary_hover, p.primary_pressed] {
            assert!(
                contrast(p.primary_text, bg) >= 4.5,
                "Classic: primary action {bg:x}"
            );
        }
    }
}
