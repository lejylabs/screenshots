use crate::redaction_editor::{RedactionEditor, RedoRedaction, UndoRedaction};
use gpui::{
    App, Bounds, ClipboardItem, ContentMask, Context, ElementInputHandler, EntityInputHandler,
    FocusHandle, Font, FontRun, FontStyle, FontWeight, Global, KeyBinding, KeyDownEvent, Keystroke,
    MouseButton, MouseDownEvent, Pixels, PlatformTextSystem, Point, Render, RenderGlyphParams,
    RenderImage, ShapedLine, SharedString, TextAlign, TextRun, UTF16Selection, Window,
    WindowHandle, actions, canvas, div, fill, point, prelude::*, px, rgb, rgba, size,
};
use std::{ops::Range, sync::Arc};
use unicode_segmentation::UnicodeSegmentation;

const MAX_LINES: usize = 4;
const LINE_HEIGHT: f32 = 24.0;

fn line_range(text: &str, row: usize) -> Range<usize> {
    text.split('\n')
        .scan(0, |start, line| {
            let range = *start..*start + line.len();
            *start = range.end + 1;
            Some(range)
        })
        .nth(row)
        .unwrap_or(text.len()..text.len())
}
fn row_for_index(text: &str, index: usize) -> usize {
    text[..index].bytes().filter(|byte| *byte == b'\n').count()
}
fn normalize_input(text: &str) -> Option<std::borrow::Cow<'_, str>> {
    if text.len() > 512 + MAX_LINES - 1 {
        return None;
    }
    Some(if text.contains('\r') {
        std::borrow::Cow::Owned(text.replace("\r\n", "\n").replace('\r', "\n"))
    } else {
        std::borrow::Cow::Borrowed(text)
    })
}
fn normalized_utf16_offset(text: &str, offset: usize) -> usize {
    let prefix = &text[..utf8(text, offset)];
    prefix.encode_utf16().count()
        - prefix
            .as_bytes()
            .windows(2)
            .filter(|pair| *pair == b"\r\n")
            .count()
}
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
    pub bold: bool,
    pub italic: bool,
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
    bold: bool,
    italic: bool,
    cancelled: impl Fn() -> bool,
) -> Result<TextAsset, String> {
    #[cfg(test)]
    let started = std::time::Instant::now();
    if text.trim().is_empty()
        || text.len() > 512
        || text.bytes().filter(|byte| *byte == b'\n').count() >= MAX_LINES
        || text.chars().any(|ch| ch.is_control() && ch != '\n')
        || ![16, 24, 36].contains(&font_size)
        || color > 0xffffff
    {
        return Err("Use up to four lines and 512 UTF-8 bytes".into());
    }
    let font_id = fonts
        .font_id(&Font {
            weight: if bold {
                FontWeight::BOLD
            } else {
                FontWeight::NORMAL
            },
            style: if italic {
                FontStyle::Italic
            } else {
                FontStyle::Normal
            },
            ..Font::default()
        })
        .map_err(|e| e.to_string())?;
    let mut glyphs = Vec::new();
    let (mut left, mut top, mut right, mut bottom) = (0, 0, 0, 0);
    let mut line_y = 0;
    for line in text.split('\n') {
        if cancelled() {
            return Err("Text preparation canceled".into());
        }
        let display = if line.is_empty() { " " } else { line };
        let layout = fonts.layout_line(
            display,
            px(font_size as f32),
            &[FontRun {
                len: display.len(),
                font_id,
            }],
        );
        let line_height = (f32::from(layout.ascent) + f32::from(layout.descent))
            .ceil()
            .max(1.0) as i32;
        right = right.max(f32::from(layout.width).ceil() as i32);
        bottom = bottom.max(line_y + line_height);
        if bottom - top > 128 {
            return Err("Text is too tall. Use fewer lines or a smaller size.".into());
        }
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
                let y = line_y
                    + (f32::from(layout.ascent) + f32::from(glyph.position.y)).floor() as i32
                    + bounds.origin.y.0;
                left = left.min(x);
                top = top.min(y);
                right = right.max(x + bounds.size.width.0);
                bottom = bottom.max(y + bounds.size.height.0);
                glyphs.push((params, bounds, x, y));
            }
        }
        line_y += line_height;
    }
    let (width, height) = (right - left, bottom - top);
    if width <= 0 || width > 1024 || height <= 0 || height > 128 || glyphs.is_empty() {
        return Err(
            "Text is too wide/tall or has no visible glyphs; shorten it or choose a smaller size"
                .into(),
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
        bold,
        italic,
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

enum InputOwner {
    Caption(WindowHandle<RedactionEditor>),
    History(gpui::WeakEntity<crate::SpikeShell>),
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
    layout: Vec<ShapedLine>,
    layout_text: String,
    rows: usize,
    bounds: Option<Bounds<Pixels>>,
    offset: Pixels,
    owner: InputOwner,
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
    pub(super) fn verify_search_limits(&mut self) -> Result<(), String> {
        if !self.replace(0..0, "Grüße 😀")
            || self.replace(0..0, "\n")
            || self.replace(0..0, &"x".repeat(513))
        {
            return Err("Search input lost its Unicode/single-line/byte limits".into());
        }
        for _ in 0..110 {
            self.replace(0..self.content.len(), "search");
        }
        if self.past.len() != 100 || self.content != "search" {
            return Err("Search input undo is not bounded".into());
        }
        self.enabled = false;
        if self.replace(0..0, "blocked") {
            return Err("Disabled search input accepted text".into());
        }
        self.enabled = true;
        self.replace(0..self.content.len(), "");
        Ok(())
    }

    #[cfg(test)]
    pub(super) fn verify_multiline_input(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Result<(), String> {
        let original = self.content.clone();
        let last_row = self.line_count() - 1;
        let last = line_range(&self.content, last_row);
        let bounds = self.bounds.ok_or("Multiline input was not painted")?;
        let index = self
            .character_index_for_point(
                point(
                    bounds.left(),
                    bounds.top() + px(last_row as f32 * LINE_HEIGHT + 1.0),
                ),
                window,
                cx,
            )
            .ok_or("Multiline point lookup failed")?;
        if index != utf16(&self.content, last.start) {
            return Err("Pointer mapping used the wrong text row".into());
        }
        let caret = self
            .bounds_for_range(index..index, bounds, window, cx)
            .ok_or("Multiline caret bounds missing")?;
        if caret.top() != bounds.top() + px(last_row as f32 * LINE_HEIGHT) {
            return Err("IME caret bounds used the wrong row".into());
        }
        let event = |key| KeyDownEvent {
            keystroke: Keystroke::parse(key).unwrap(),
            is_held: false,
            prefer_character_input: false,
        };
        self.select_to(last.end, false);
        self.key(&event("home"), window, cx);
        if self.cursor() != last.start {
            return Err("Home did not move to the current line start".into());
        }
        self.key(&event("up"), window, cx);
        if row_for_index(&self.content, self.cursor()) != last_row - 1 {
            return Err("Up did not navigate to the previous row".into());
        }
        self.key(&event("down"), window, cx);
        if row_for_index(&self.content, self.cursor()) != last_row {
            return Err("Down did not navigate to the next row".into());
        }
        self.key(&event("end"), window, cx);
        self.key(&event("shift-enter"), window, cx);
        if self.line_count() != last_row + 2 {
            return Err("Shift+Enter did not insert a line break".into());
        }
        self.undo(false, window, cx);
        if self.content != original {
            return Err("Input undo did not restore the multiline text".into());
        }
        self.replace_text_in_range(Some(0..usize::MAX), "A\r\n😀\r\nB\r\nC", window, cx);
        let full = self.content.clone();
        let history = self.past.len();
        self.replace_text_in_range(None, "\n", window, cx);
        if self.content != full || self.past.len() != history || self.line_count() != MAX_LINES {
            return Err("Input did not preserve its four-line admission limit".into());
        }
        self.undo(false, window, cx);
        self.replace_and_mark_text_in_range(Some(0..usize::MAX), "A\r\n😀", Some(3..5), window, cx);
        let selection = self
            .selected_text_range(false, window, cx)
            .ok_or("Composition selection missing")?;
        if self.content != "A\n😀" || selection.range != (2..4) {
            return Err("CRLF composition offsets were not normalized".into());
        }
        self.undo(false, window, cx);
        if self.content != original || self.marked.is_some() {
            return Err("Input probe did not restore its text/composition".into());
        }
        Ok(())
    }
    #[cfg(test)]
    pub(super) fn has_current_layout(&self) -> bool {
        !self.layout.is_empty() && self.layout_text == self.content
    }
    pub(super) fn line_count(&self) -> usize {
        self.content.bytes().filter(|byte| *byte == b'\n').count() + 1
    }
    pub fn new(
        content: String,
        owner: WindowHandle<RedactionEditor>,
        cx: &mut Context<Self>,
    ) -> Self {
        Self::with_owner(content, InputOwner::Caption(owner), cx)
    }
    pub fn search(owner: gpui::WeakEntity<crate::SpikeShell>, cx: &mut Context<Self>) -> Self {
        Self::with_owner(String::new(), InputOwner::History(owner), cx)
    }
    fn with_owner(content: String, owner: InputOwner, cx: &mut Context<Self>) -> Self {
        let end = content.len();
        let rows = content.bytes().filter(|byte| *byte == b'\n').count() + 1;
        Self {
            enabled: true,
            focus: cx.focus_handle(),
            content,
            selection: 0..end,
            reversed: false,
            marked: None,
            past: Vec::new(),
            redo: Vec::new(),
            layout: Vec::new(),
            layout_text: String::new(),
            rows,
            bounds: None,
            offset: px(0.0),
            owner,
        }
    }
    fn range(&self, range: Range<usize>) -> Range<usize> {
        let start = utf8(&self.content, range.start);
        start..utf8(&self.content, range.end).max(start)
    }
    fn changed(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let rows = self.line_count();
        let resized = rows != self.rows;
        if resized {
            self.rows = rows;
        }
        if let InputOwner::Caption(owner) = self.owner {
            if resized {
                cx.defer(move |cx| {
                    owner.update(cx, |_, _, cx| cx.notify()).ok();
                });
            }
        } else if let InputOwner::History(owner) = &self.owner {
            let owner = owner.clone();
            let input = cx.entity_id();
            let content = self.content.clone();
            cx.defer(move |cx| {
                owner
                    .update(cx, |shell, cx| {
                        if shell
                            .history_search_input
                            .as_ref()
                            .is_some_and(|field| field.entity_id() == input)
                        {
                            shell.set_history_query(content, cx);
                        }
                    })
                    .ok();
            });
        }
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
    fn index_at(&self, position: Point<Pixels>) -> Option<usize> {
        let bounds = self.bounds?;
        if self.layout.is_empty() || self.layout_text != self.content {
            return None;
        }
        let row = ((f32::from(position.y - bounds.top()) / LINE_HEIGHT)
            .floor()
            .max(0.0) as usize)
            .min(self.layout.len() - 1);
        let range = line_range(&self.content, row);
        Some(
            range.start
                + self.layout[row]
                    .closest_index_for_x(position.x - bounds.left() + self.offset)
                    .min(range.len()),
        )
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
        if text.chars().any(|ch| ch.is_control() && ch != '\n') {
            return false;
        }
        if self.content.len() - range.len() + text.len() > 512 {
            return false;
        }
        let breaks = |text: &str| text.bytes().filter(|byte| *byte == b'\n').count();
        let max_lines = if matches!(self.owner, InputOwner::History(_)) {
            1
        } else {
            MAX_LINES
        };
        if breaks(&self.content) - breaks(&self.content[range.clone()]) + breaks(text) >= max_lines
        {
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
            if let InputOwner::Caption(owner) = self.owner {
                if modifiers.shift {
                    self.replace(self.selection.clone(), "\n");
                } else {
                    cx.defer(move |cx| {
                        owner
                            .update(cx, |editor, window, cx| editor.commit_text(window, cx))
                            .ok();
                    });
                }
            }
        } else if key == "escape" {
            if let InputOwner::Caption(owner) = self.owner {
                cx.defer(move |cx| {
                    owner
                        .update(cx, |editor, window, cx| editor.cancel_text(window, cx))
                        .ok();
                });
            } else {
                self.replace(0..self.content.len(), "");
                self.marked = None;
            }
        } else if (modifiers.control || modifiers.platform) && !modifiers.alt {
            match key {
                "home" | "end" => {
                    self.select_to(
                        if key == "home" { 0 } else { self.content.len() },
                        modifiers.shift,
                    );
                    self.marked = None;
                }
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
                "left" | "right" | "home" | "end" | "up" | "down" => {
                    let cursor = self.cursor();
                    let row = row_for_index(&self.content, cursor);
                    let current = line_range(&self.content, row);
                    let position = match key {
                        "home" => current.start,
                        "end" => current.end,
                        "up" | "down" => {
                            let next_row = if key == "up" {
                                row.saturating_sub(1)
                            } else {
                                (row + 1).min(self.line_count() - 1)
                            };
                            let next = line_range(&self.content, next_row);
                            if self.layout_text == self.content
                                && self.layout.len() == self.line_count()
                            {
                                let x = self.layout[row].x_for_index(cursor - current.start);
                                next.start
                                    + self.layout[next_row].closest_index_for_x(x).min(next.len())
                            } else {
                                let column =
                                    self.content[current.start..cursor].graphemes(true).count();
                                next.start
                                    + self.content[next.clone()]
                                        .grapheme_indices(true)
                                        .nth(column)
                                        .map_or(next.len(), |(index, _)| index)
                            }
                        }
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
        let Some(text) = normalize_input(text) else {
            return;
        };
        let range = range
            .map(|range| self.range(range))
            .or(self.marked.clone())
            .unwrap_or(self.selection.clone());
        if self.replace(range, &text) {
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
        let original = text;
        let Some(text) = normalize_input(original) else {
            return;
        };
        let selection = selection.map(|range| {
            normalized_utf16_offset(original, range.start)
                ..normalized_utf16_offset(original, range.end)
        });
        let range = range
            .map(|range| self.range(range))
            .or(self.marked.clone())
            .unwrap_or(self.selection.clone());
        let start = range.start;
        if self.replace(range, &text) {
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
        if self.layout.is_empty() || self.layout_text != self.content {
            return None;
        }
        let row = row_for_index(&self.content, range.start);
        let end_row = row_for_index(&self.content, range.end);
        let top = bounds.top() + px(row as f32 * LINE_HEIGHT);
        if row != end_row {
            return Some(Bounds::new(
                point(bounds.left(), top),
                size(
                    bounds.size.width,
                    px((end_row - row + 1) as f32 * LINE_HEIGHT),
                ),
            ));
        }
        let line = line_range(&self.content, row);
        let start =
            bounds.left() - self.offset + self.layout[row].x_for_index(range.start - line.start);
        let end =
            bounds.left() - self.offset + self.layout[row].x_for_index(range.end - line.start);
        Some(Bounds::from_corners(
            point(start.min(end), top),
            point(start.max(end), top + px(LINE_HEIGHT)),
        ))
    }
    fn character_index_for_point(
        &mut self,
        position: Point<Pixels>,
        _: &mut Window,
        _: &mut Context<Self>,
    ) -> Option<usize> {
        self.index_at(position)
            .map(|index| utf16(&self.content, index))
    }
}
impl Render for CaptionInput {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let entity = cx.entity();
        let focus = self.focus.clone();
        let text = self.content.clone();
        let layout_text = text.clone();
        let cursor = self.cursor();
        let marked = self.marked.clone();
        div()
            .h(px(self.line_count() as f32 * LINE_HEIGHT + 8.0))
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
                    if let Some(index) = this.index_at(event.position) {
                        this.select_to(index, event.modifiers.shift);
                    }
                    window.focus(&this.focus, cx);
                    this.changed(window, cx);
                }),
            )
            .child(
                canvas(
                    move |bounds, window, _| {
                        let lines = text
                            .split('\n')
                            .map(|part| {
                                let display: SharedString = if text.is_empty() {
                                    "Type a label; Shift+Enter adds a line".into()
                                } else {
                                    part.to_owned().into()
                                };
                                let run = TextRun {
                                    len: display.len(),
                                    font: Font::default(),
                                    color: rgb(0xf1f5f9).into(),
                                    background_color: None,
                                    underline: None,
                                    strikethrough: None,
                                };
                                window
                                    .text_system()
                                    .shape_line(display, px(16.0), &[run], None)
                            })
                            .collect::<Vec<_>>();
                        let row = row_for_index(&text, cursor);
                        let range = line_range(&text, row);
                        let offset = (lines[row].x_for_index(cursor - range.start)
                            - bounds.size.width
                            + px(8.0))
                        .max(px(0.0));
                        (lines, offset)
                    },
                    move |bounds, (lines, offset), window, cx| {
                        let text_bounds = Bounds::new(
                            point(bounds.left(), bounds.top() + px(4.0)),
                            size(bounds.size.width, px(lines.len() as f32 * LINE_HEIGHT)),
                        );
                        window.handle_input(
                            &focus,
                            ElementInputHandler::new(text_bounds, entity.clone()),
                            cx,
                        );
                        let selection = entity.read(cx).selection.clone();
                        window.with_content_mask(Some(ContentMask { bounds }), |window| {
                            for (row, line) in lines.iter().enumerate() {
                                let range = line_range(&layout_text, row);
                                let top = text_bounds.top() + px(row as f32 * LINE_HEIGHT);
                                let x_for = |index: usize| {
                                    text_bounds.left() - offset
                                        + line.x_for_index(
                                            index.saturating_sub(range.start).min(range.len()),
                                        )
                                };
                                if !selection.is_empty()
                                    && selection.start <= range.end
                                    && selection.end > range.start
                                {
                                    let start = x_for(selection.start.max(range.start));
                                    let mut end = x_for(selection.end.min(range.end));
                                    if selection.end > range.end && range.end < layout_text.len() {
                                        end = end.max(
                                            text_bounds.left() - offset + line.width + px(4.0),
                                        );
                                    }
                                    window.paint_quad(fill(
                                        Bounds::from_corners(
                                            point(start.min(end), top),
                                            point(start.max(end), top + px(LINE_HEIGHT)),
                                        ),
                                        rgba(0x22d3ee55),
                                    ));
                                }
                                line.paint(
                                    point(text_bounds.left() - offset, top),
                                    px(LINE_HEIGHT),
                                    TextAlign::Left,
                                    None,
                                    window,
                                    cx,
                                )
                                .ok();
                                if selection.is_empty()
                                    && row == row_for_index(&layout_text, cursor)
                                {
                                    window.paint_quad(fill(
                                        Bounds::new(
                                            point(x_for(cursor), top),
                                            size(px(1.0), px(LINE_HEIGHT)),
                                        ),
                                        rgb(0x22d3ee),
                                    ));
                                }
                                if let Some(marked) = &marked {
                                    if marked.start < range.end && marked.end > range.start {
                                        let x = x_for(marked.start.max(range.start));
                                        let end = x_for(marked.end.min(range.end));
                                        window.paint_quad(fill(
                                            Bounds::new(
                                                point(x.min(end), top + px(LINE_HEIGHT - 2.0)),
                                                size((end - x).abs(), px(1.0)),
                                            ),
                                            rgb(0x22d3ee),
                                        ));
                                    }
                                }
                            }
                        });
                        entity.update(cx, |this, _| {
                            this.layout = lines;
                            this.layout_text = layout_text;
                            this.bounds = Some(text_bounds);
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
    fn multiline_ranges_and_crlf_mapping_keep_utf16_positions_valid() {
        let text = "A😀\n\n日e\u{301}\n";
        assert_eq!(line_range(text, 0), 0..5);
        assert_eq!(line_range(text, 1), 6..6);
        assert_eq!(line_range(text, 2), 7..13);
        assert_eq!(line_range(text, 3), 14..14);
        for (index, row) in [(0, 0), (5, 0), (6, 1), (7, 2), (13, 2), (14, 3)] {
            assert_eq!(row_for_index(text, index), row);
        }
        assert_eq!(normalize_input("😀\r\nA\rB\n").unwrap(), "😀\nA\nB\n");
        let raw = "😀\r\nA\rB\n";
        for (raw_offset, normalized) in [
            (0, 0),
            (1, 0),
            (2, 2),
            (3, 3),
            (4, 3),
            (5, 4),
            (6, 5),
            (7, 6),
            (8, 7),
        ] {
            assert_eq!(normalized_utf16_offset(raw, raw_offset), normalized);
        }
        assert!(normalize_input(&"x".repeat(516)).is_none());
        for (byte, _) in text.char_indices() {
            assert_eq!(utf8(text, utf16(text, byte)), byte);
        }
    }
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
