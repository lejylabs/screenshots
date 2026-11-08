use crate::redaction_editor::{RedactionEditor, RedoRedaction, UndoRedaction};
use gpui::{
    App, Bounds, ClipboardItem, ContentMask, Context, ElementInputHandler, EntityInputHandler,
    FocusHandle, Font, FontRun, Global, KeyBinding, KeyDownEvent, Keystroke, MouseButton,
    MouseDownEvent, Pixels, PlatformTextSystem, Point, Render, RenderGlyphParams, RenderImage,
    ShapedLine, SharedString, TextAlign, TextRun, UTF16Selection, Window, WindowHandle, actions,
    canvas, div, fill, point, prelude::*, px, rgb, rgba, size,
};
use std::{ops::Range, sync::Arc};
use unicode_segmentation::UnicodeSegmentation;
actions!(caption, [InputBackspace, InputDelete]);
pub(super) fn bind_keys(cx: &mut App) {
    cx.bind_keys([
        KeyBinding::new("backspace", InputBackspace, Some("CaptionInput")),
        KeyBinding::new("delete", InputDelete, Some("CaptionInput")),
    ]);
}

pub(super) struct CaptionFonts(pub Arc<dyn PlatformTextSystem>);
impl Global for CaptionFonts {}

pub(super) struct TextAsset {
    pub text: String,
    pub font_size: u32,
    pub stamp: app_core::TextAnnotation,
    pub preview: Arc<RenderImage>,
}
impl TextAsset {
    pub fn bytes(&self) -> usize {
        self.stamp.mask.len() * 5
    }
}

pub(super) fn rasterize(
    fonts: &dyn PlatformTextSystem,
    text: String,
    font_size: u32,
    color: u32,
    cancelled: impl Fn() -> bool,
) -> Result<TextAsset, String> {
    #[cfg(test)]
    let started = std::time::Instant::now();
    if text.trim().is_empty()
        || text.len() > 512
        || text.chars().any(char::is_control)
        || ![16, 24, 36].contains(&font_size)
        || color > 0xffffff
    {
        return Err("Use a single text line of up to 512 bytes".into());
    }
    let font_id = fonts.font_id(&Font::default()).map_err(|e| e.to_string())?;
    let layout = fonts.layout_line(
        &text,
        px(font_size as f32),
        &[FontRun {
            len: text.len(),
            font_id,
        }],
    );

    let mut glyphs = Vec::new();
    let (mut left, mut top, mut right, mut bottom) = (
        0,
        0,
        f32::from(layout.width).ceil() as i32,
        (f32::from(layout.ascent) + f32::from(layout.descent)).ceil() as i32,
    );
    for run in &layout.runs {
        for glyph in &run.glyphs {
            if cancelled() {
                return Err("Text preparation canceled".into());
            }
            let params = RenderGlyphParams {
                font_id: run.font_id,
                glyph_id: glyph.id,
                font_size: px(font_size as f32),
                subpixel_variant: Default::default(),
                scale_factor: 1.0,
                // Color glyphs use the UI's shared D3D context; workers only rasterize coverage.
                is_emoji: false,
                subpixel_rendering: false,
                dilation: 0,
            };
            let bounds = fonts
                .glyph_raster_bounds(&params)
                .map_err(|e| e.to_string())?;
            if bounds.size.width.0 == 0 || bounds.size.height.0 == 0 {
                continue;
            }
            if bounds.size.width.0 < 0
                || bounds.size.width.0 > 1024
                || bounds.size.height.0 < 0
                || bounds.size.height.0 > 128
            {
                return Err("Text glyph exceeds the label size limit".into());
            }
            let x = f32::from(glyph.position.x).floor() as i32 + bounds.origin.x.0;
            let y = (f32::from(layout.ascent) + f32::from(glyph.position.y)).floor() as i32
                + bounds.origin.y.0;
            left = left.min(x);
            top = top.min(y);
            right = right.max(x + bounds.size.width.0);
            bottom = bottom.max(y + bounds.size.height.0);
            glyphs.push((params, bounds, x, y));
        }
    }
    let (width, height) = (right - left, bottom - top);
    if width <= 0 || width > 1024 || height <= 0 || height > 128 || glyphs.is_empty() {
        return Err(
            "Text is too wide or has no visible glyphs; shorten it or choose a smaller size".into(),
        );
    }
    let mut mask = vec![0u8; (width * height) as usize];
    for (params, bounds, x, y) in glyphs {
        if cancelled() {
            return Err("Text preparation canceled".into());
        }
        let (size, bytes) = fonts
            .rasterize_glyph(&params, bounds)
            .map_err(|e| e.to_string())?;
        if size != bounds.size {
            return Err("Text glyph dimensions changed".into());
        }
        let count = (size.width.0 * size.height.0) as usize;
        let channels = bytes.len() / count;
        if bytes.len() != count * channels || ![1, 3, 4].contains(&channels) {
            return Err("Unsupported text raster format".into());
        }
        for row in 0..size.height.0 {
            for column in 0..size.width.0 {
                let index = (row * size.width.0 + column) as usize * channels;
                let alpha = match channels {
                    1 => u32::from(bytes[index]),
                    4 => u32::from(bytes[index + 3]),
                    _ => {
                        bytes[index..index + 3]
                            .iter()
                            .map(|v| u32::from(*v))
                            .sum::<u32>()
                            / 3
                    }
                };
                let destination = &mut mask[((y - top + row) * width + x - left + column) as usize];
                *destination =
                    (alpha + u32::from(*destination) * (255 - alpha) / 255).min(255) as u8;
            }
        }
    }
    if !mask.iter().any(|alpha| *alpha > 0) {
        return Err("Text has no visible glyphs".into());
    }
    let mut bgra = image::RgbaImage::new(width as u32, height as u32);
    for (pixel, alpha) in bgra.pixels_mut().zip(&mask) {
        pixel.0 = [color as u8, (color >> 8) as u8, (color >> 16) as u8, *alpha];
    }
    #[cfg(test)]
    eprintln!(
        "Native caption shaping/raster: {:?}; mask={} bytes",
        started.elapsed(),
        mask.len()
    );
    Ok(TextAsset {
        text,
        font_size,
        stamp: app_core::TextAnnotation {
            area: app_core::PixelRegion {
                x: 0,
                y: 0,
                width: width as u32,
                height: height as u32,
            },
            color,
            width: width as u32,
            height: height as u32,
            mask: mask.into(),
        },
        preview: Arc::new(RenderImage::new(vec![image::Frame::new(bgra)])),
    })
}

pub(super) struct CaptionInput {
    pub enabled: bool,
    pub focus: FocusHandle,
    pub content: String,
    selection: Range<usize>,
    reversed: bool,
    marked: Option<Range<usize>>,
    past: Vec<String>,
    redo: Vec<String>,
    layout: Option<ShapedLine>,
    bounds: Option<Bounds<Pixels>>,
    offset: Pixels,
    owner: WindowHandle<RedactionEditor>,
}
fn utf8(text: &str, offset: usize) -> usize {
    let mut length = 0;
    for (byte, ch) in text.char_indices() {
        if length + ch.len_utf16() > offset {
            return byte;
        }
        length += ch.len_utf16();
    }
    text.len()
}
fn utf16(text: &str, offset: usize) -> usize {
    text[..offset].encode_utf16().count()
}
impl CaptionInput {
    #[cfg(test)]
    pub(super) fn has_current_layout(&self) -> bool {
        self.layout
            .as_ref()
            .is_some_and(|line| line.text.as_ref() == self.content.as_str())
    }
    pub fn new(
        content: String,
        owner: WindowHandle<RedactionEditor>,
        cx: &mut Context<Self>,
    ) -> Self {
        let end = content.len();
        Self {
            enabled: true,
            focus: cx.focus_handle(),
            content,
            selection: 0..end,
            reversed: false,
            marked: None,
            past: Vec::new(),
            redo: Vec::new(),
            layout: None,
            bounds: None,
            offset: px(0.0),
            owner,
        }
    }
    fn range(&self, range: Range<usize>) -> Range<usize> {
        let start = utf8(&self.content, range.start);
        start..utf8(&self.content, range.end).max(start)
    }
    fn changed(&self, window: &mut Window, cx: &mut Context<Self>) {
        window.notify_text_input_changed(&self.focus);
        cx.notify();
    }
    fn cursor(&self) -> usize {
        if self.reversed {
            self.selection.start
        } else {
            self.selection.end
        }
    }
    fn select_to(&mut self, position: usize, extend: bool) {
        let anchor = if extend {
            if self.reversed {
                self.selection.end
            } else {
                self.selection.start
            }
        } else {
            position
        };
        self.selection = anchor.min(position)..anchor.max(position);
        self.reversed = position < anchor;
    }
    fn replace(&mut self, range: Range<usize>, text: &str) -> bool {
        if !self.enabled {
            return false;
        }
        if text.chars().any(char::is_control) {
            return false;
        }
        if self.content.len() - range.len() + text.len() > 512 {
            return false;
        }
        if self.past.len() == 100 {
            self.past.remove(0);
        }
        self.past.push(self.content.clone());
        self.redo.clear();
        self.content.replace_range(range.clone(), text);
        let end = range.start + text.len();
        self.selection = end..end;
        self.reversed = false;
        true
    }
    fn undo(&mut self, redo: bool, window: &mut Window, cx: &mut Context<Self>) {
        if !self.enabled {
            return;
        }
        let state = if redo {
            self.redo.pop()
        } else {
            self.past.pop()
        };
        if let Some(state) = state {
            let previous = std::mem::replace(&mut self.content, state);
            if redo {
                self.past.push(previous);
            } else {
                self.redo.push(previous);
            }
            self.selection = self.content.len()..self.content.len();
            self.reversed = false;
            self.marked = None;
            self.changed(window, cx);
        }
    }
    fn key(&mut self, event: &KeyDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        let key = event.keystroke.key.as_str();
        let modifiers = event.keystroke.modifiers;
        if !self.enabled && key != "escape" {
            cx.stop_propagation();
            return;
        }
        if key == "enter" && self.marked.is_none() {
            let owner = self.owner;
            cx.defer(move |cx| {
                owner
                    .update(cx, |editor, window, cx| editor.commit_text(window, cx))
                    .ok();
            });
        } else if key == "escape" {
            let owner = self.owner;
            cx.defer(move |cx| {
                owner
                    .update(cx, |editor, window, cx| editor.cancel_text(window, cx))
                    .ok();
            });
        } else if (modifiers.control || modifiers.platform) && !modifiers.alt {
            match key {
                "a" => {
                    self.selection = 0..self.content.len();
                    self.reversed = false;
                }
                "v" => {
                    if let Some(text) = cx.read_from_clipboard().and_then(|item| item.text()) {
                        self.replace_text_in_range(None, &text, window, cx);
                    }
                }
                "c" | "x" => {
                    if !self.selection.is_empty() {
                        cx.write_to_clipboard(ClipboardItem::new_string(
                            self.content[self.selection.clone()].into(),
                        ));
                        if key == "x" {
                            self.replace_text_in_range(None, "", window, cx);
                        }
                    }
                }
                _ => return,
            }
        } else if !modifiers.control && !modifiers.platform && !modifiers.alt {
            match key {
                "backspace" | "delete" => {
                    let mut range = self.selection.clone();
                    if range.is_empty() {
                        if key == "backspace" {
                            range.start = self.content[..range.start]
                                .grapheme_indices(true)
                                .last()
                                .map_or(0, |(i, _)| i);
                        } else {
                            range.end = self.content[range.end..]
                                .graphemes(true)
                                .next()
                                .map_or(range.end, |ch| range.end + ch.len());
                        }
                    }
                    if !range.is_empty() {
                        self.replace(range, "");
                        self.marked = None;
                    }
                }
                "left" | "right" | "home" | "end" => {
                    let cursor = self.cursor();
                    let position = match key {
                        "home" => 0,
                        "end" => self.content.len(),
                        "left" if !modifiers.shift && !self.selection.is_empty() => {
                            self.selection.start
                        }
                        "right" if !modifiers.shift && !self.selection.is_empty() => {
                            self.selection.end
                        }
                        "left" => self.content[..cursor]
                            .grapheme_indices(true)
                            .last()
                            .map_or(0, |(i, _)| i),
                        _ => self.content[cursor..]
                            .graphemes(true)
                            .next()
                            .map_or(self.content.len(), |ch| cursor + ch.len()),
                    };
                    self.select_to(position, modifiers.shift);
                    self.marked = None;
                }
                _ => return,
            }
        } else {
            return;
        }
        cx.stop_propagation();
        self.changed(window, cx);
    }
}
impl EntityInputHandler for CaptionInput {
    fn accepts_text_input(&self, _: &mut Window, _: &mut Context<Self>) -> bool {
        self.enabled
    }
    fn text_for_range(
        &mut self,
        range: Range<usize>,
        actual: &mut Option<Range<usize>>,
        _: &mut Window,
        _: &mut Context<Self>,
    ) -> Option<String> {
        let range = self.range(range);
        *actual = Some(utf16(&self.content, range.start)..utf16(&self.content, range.end));
        Some(self.content[range].into())
    }
    fn selected_text_range(
        &mut self,
        _: bool,
        _: &mut Window,
        _: &mut Context<Self>,
    ) -> Option<UTF16Selection> {
        Some(UTF16Selection {
            range: utf16(&self.content, self.selection.start)
                ..utf16(&self.content, self.selection.end),
            reversed: self.reversed,
        })
    }
    fn marked_text_range(&self, _: &mut Window, _: &mut Context<Self>) -> Option<Range<usize>> {
        self.marked
            .as_ref()
            .map(|range| utf16(&self.content, range.start)..utf16(&self.content, range.end))
    }
    fn unmark_text(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.marked = None;
        self.changed(window, cx);
    }
    fn set_selected_text_range(
        &mut self,
        range: Range<usize>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.selection = self.range(range);
        self.reversed = false;
        self.changed(window, cx);
    }
    fn text_length_utf16(&mut self, _: &mut Window, _: &mut Context<Self>) -> Option<usize> {
        Some(self.content.encode_utf16().count())
    }
    fn replace_text_in_range(
        &mut self,
        range: Option<Range<usize>>,
        text: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let range = range
            .map(|range| self.range(range))
            .or(self.marked.clone())
            .unwrap_or(self.selection.clone());
        if self.replace(range, text) {
            self.marked = None;
            self.changed(window, cx);
        }
    }
    fn replace_and_mark_text_in_range(
        &mut self,
        range: Option<Range<usize>>,
        text: &str,
        selection: Option<Range<usize>>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let range = range
            .map(|range| self.range(range))
            .or(self.marked.clone())
            .unwrap_or(self.selection.clone());
        let start = range.start;
        if self.replace(range, text) {
            self.marked = (!text.is_empty()).then_some(start..start + text.len());
            if let Some(selection) = selection {
                let base = utf16(&self.content, start);
                let limit = text.encode_utf16().count();
                self.selection =
                    self.range(base + selection.start.min(limit)..base + selection.end.min(limit));
                self.reversed = false;
            }
            self.changed(window, cx);
        }
    }
    fn bounds_for_range(
        &mut self,
        range: Range<usize>,
        bounds: Bounds<Pixels>,
        _: &mut Window,
        _: &mut Context<Self>,
    ) -> Option<Bounds<Pixels>> {
        let range = self.range(range);
        let layout = self.layout.as_ref()?;
        if layout.text.as_ref() != self.content.as_str() {
            return None;
        }
        Some(Bounds::from_corners(
            point(
                bounds.left() - self.offset + layout.x_for_index(range.start),
                bounds.top(),
            ),
            point(
                bounds.left() - self.offset + layout.x_for_index(range.end),
                bounds.bottom(),
            ),
        ))
    }
    fn character_index_for_point(
        &mut self,
        position: Point<Pixels>,
        _: &mut Window,
        _: &mut Context<Self>,
    ) -> Option<usize> {
        let bounds = self.bounds?;
        if self.layout.as_ref()?.text.as_ref() != self.content.as_str() {
            return None;
        }
        let index = self
            .layout
            .as_ref()?
            .closest_index_for_x(position.x - bounds.left() + self.offset);
        Some(utf16(&self.content, index.min(self.content.len())))
    }
}
impl Render for CaptionInput {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let entity = cx.entity();
        let focus = self.focus.clone();
        let text = self.content.clone();
        let cursor = self.cursor();
        let marked = self.marked.clone();
        div()
            .h(px(32.0))
            .w_full()
            .overflow_hidden()
            .bg(rgb(0x252830))
            .track_focus(&self.focus)
            .key_context("CaptionInput")
            .on_key_down(cx.listener(Self::key))
            .on_action(cx.listener(|this, _: &UndoRedaction, window, cx| {
                this.undo(false, window, cx);
                cx.stop_propagation();
            }))
            .on_action(cx.listener(|this, _: &RedoRedaction, window, cx| {
                this.undo(true, window, cx);
                cx.stop_propagation();
            }))
            .on_action(cx.listener(|this, _: &InputBackspace, window, cx| {
                let event = KeyDownEvent {
                    keystroke: Keystroke::parse("backspace").unwrap(),
                    is_held: false,
                    prefer_character_input: false,
                };
                this.key(&event, window, cx);
                cx.stop_propagation();
            }))
            .on_action(cx.listener(|this, _: &InputDelete, window, cx| {
                let event = KeyDownEvent {
                    keystroke: Keystroke::parse("delete").unwrap(),
                    is_held: false,
                    prefer_character_input: false,
                };
                this.key(&event, window, cx);
                cx.stop_propagation();
            }))
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|this, event: &MouseDownEvent, window, cx| {
                    if let Some(bounds) = this.bounds {
                        if let Some(layout) = &this.layout {
                            if layout.text.as_ref() != this.content.as_str() {
                                return;
                            }
                            let index = layout
                                .closest_index_for_x(event.position.x - bounds.left() + this.offset)
                                .min(this.content.len());
                            this.select_to(index, event.modifiers.shift);
                        }
                    }
                    window.focus(&this.focus, cx);
                    this.changed(window, cx);
                }),
            )
            .child(
                canvas(
                    move |bounds, window, _| {
                        let display: SharedString = if text.is_empty() {
                            "Type a label; Enter applies it".into()
                        } else {
                            text.clone().into()
                        };
                        let run = TextRun {
                            len: display.len(),
                            font: Font::default(),
                            color: rgb(0xf1f5f9).into(),
                            background_color: None,
                            underline: None,
                            strikethrough: None,
                        };
                        let line = window
                            .text_system()
                            .shape_line(display, px(16.0), &[run], None);
                        let offset =
                            (line.x_for_index(cursor) - bounds.size.width + px(8.0)).max(px(0.0));
                        (line, offset)
                    },
                    move |bounds, (line, offset), window, cx| {
                        window.handle_input(
                            &focus,
                            ElementInputHandler::new(bounds, entity.clone()),
                            cx,
                        );
                        let selection = entity.read(cx).selection.clone();
                        window.with_content_mask(Some(ContentMask { bounds }), |window| {
                            if !selection.is_empty() {
                                window.paint_quad(fill(
                                    Bounds::from_corners(
                                        point(
                                            bounds.left() - offset
                                                + line.x_for_index(selection.start),
                                            bounds.top(),
                                        ),
                                        point(
                                            bounds.left() - offset
                                                + line.x_for_index(selection.end),
                                            bounds.bottom(),
                                        ),
                                    ),
                                    rgba(0x22d3ee55),
                                ));
                            }
                            line.paint(
                                point(bounds.left() - offset, bounds.top()),
                                bounds.size.height,
                                TextAlign::Left,
                                None,
                                window,
                                cx,
                            )
                            .ok();
                            if selection.is_empty() {
                                window.paint_quad(fill(
                                    Bounds::new(
                                        point(
                                            bounds.left() - offset
                                                + line.x_for_index(selection.end),
                                            bounds.top(),
                                        ),
                                        size(px(1.0), bounds.size.height),
                                    ),
                                    rgb(0x22d3ee),
                                ));
                            }
                            if let Some(marked) = &marked {
                                let x = bounds.left() - offset + line.x_for_index(marked.start);
                                let end = bounds.left() - offset + line.x_for_index(marked.end);
                                window.paint_quad(fill(
                                    Bounds::new(
                                        point(x, bounds.bottom() - px(2.0)),
                                        size(end - x, px(1.0)),
                                    ),
                                    rgb(0x22d3ee),
                                ));
                            }
                        });
                        entity.update(cx, |this, _| {
                            this.layout = Some(line);
                            this.bounds = Some(bounds);
                            this.offset = offset;
                        });
                    },
                )
                .size_full(),
            )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn utf16_ranges_keep_unicode_and_grapheme_boundaries_valid() {
        let text = "A😀e\u{301}日";
        for (byte, _) in text.char_indices() {
            assert_eq!(utf8(text, utf16(text, byte)), byte);
        }
        assert_eq!(utf8(text, 2), 1);
        assert_eq!(utf8(text, usize::MAX), text.len());
        assert_eq!(
            text.graphemes(true).collect::<Vec<_>>(),
            vec!["A", "😀", "e\u{301}", "日"]
        );
        assert_eq!(utf16(text, text.len()), 6);
    }
}
