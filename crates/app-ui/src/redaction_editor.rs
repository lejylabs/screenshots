use crate::caption::{CaptionFonts, CaptionInput, TextAsset};
use crate::{CloseOverlay, SpikeShell, capture_button};
use app_core::{ArrowAnnotation, CapturedImage, FrameAnnotation, HighlightAnnotation, PixelRegion};
use gpui::{
    App, Context, FocusHandle, IntoElement, KeyBinding, Render, Window, actions, div, img,
    prelude::*, px, rgb, rgba,
};
use std::sync::Arc;

fn pen_point(position: [f32; 2], scale: f32, image: [u32; 2]) -> [u32; 2] {
    std::array::from_fn(|axis| {
        (position[axis] / scale)
            .floor()
            .clamp(0.0, (image[axis] - 1) as f32) as u32
    })
}
fn line_from_drag(
    start: [f32; 2],
    end: [f32; 2],
    viewport: [f32; 2],
    image: [u32; 2],
    color: u32,
    width: u8,
    snap: bool,
) -> Option<app_core::PenAnnotation> {
    let endpoints =
        ArrowAnnotation::from_drag_constrained(start, end, viewport, image, color, snap)?;
    let mut line = app_core::PenAnnotation::from_trace(&[endpoints.start, endpoints.end], color)?;
    line.width = width;
    Some(line)
}

fn pen_path(
    pen: &app_core::PenAnnotation,
    origin: gpui::Point<gpui::Pixels>,
    scale: f32,
) -> Option<gpui::Path<gpui::Pixels>> {
    let mut positions = pen.positions();
    let first = positions.next()?;
    let map = |point: [f32; 2]| {
        gpui::point(
            origin.x + px(point[0] * scale),
            origin.y + px(point[1] * scale),
        )
    };
    let mut builder = gpui::PathBuilder::stroke(px(f32::from(pen.width) * scale)).with_style(
        gpui::PathStyle::Stroke(
            gpui::StrokeOptions::default()
                .with_line_width(f32::from(pen.width) * scale)
                .with_miter_limit(1.0),
        ),
    );
    builder.move_to(map(first));
    for point in positions {
        builder.line_to(map(point));
    }
    builder.build().ok()
}

#[derive(Clone, Copy)]
pub(super) enum OutputMode {
    CopyOnly,
    SaveOnly,
    SaveAndCopy,
}
impl OutputMode {
    pub fn copies(self) -> bool {
        matches!(self, Self::CopyOnly | Self::SaveAndCopy)
    }
    pub fn saves(self) -> bool {
        matches!(self, Self::SaveOnly | Self::SaveAndCopy)
    }
}

fn clamp_pan(pan: [f32; 2], image: [f32; 2], viewport: [f32; 2]) -> [f32; 2] {
    std::array::from_fn(|axis| {
        let limit = ((image[axis] - viewport[axis].max(1.0)) / 2.0).max(0.0);
        if pan[axis].is_finite() {
            pan[axis].clamp(-limit, limit)
        } else {
            0.0
        }
    })
}

actions!(
    redaction_editor,
    [
        UndoRedaction,
        RedoRedaction,
        DeleteAnnotation,
        DuplicateAnnotation
    ]
);

pub(super) fn bind_keys(cx: &mut App) {
    crate::caption::bind_keys(cx);
    cx.bind_keys([
        KeyBinding::new(
            if cfg!(target_os = "macos") {
                "cmd-z"
            } else {
                "ctrl-z"
            },
            UndoRedaction,
            Some("RedactionEditor"),
        ),
        KeyBinding::new(
            if cfg!(target_os = "macos") {
                "cmd-shift-z"
            } else {
                "ctrl-shift-z"
            },
            RedoRedaction,
            Some("RedactionEditor"),
        ),
        KeyBinding::new("ctrl-y", RedoRedaction, Some("RedactionEditor")),
        KeyBinding::new("delete", DeleteAnnotation, Some("RedactionEditor")),
        KeyBinding::new("backspace", DeleteAnnotation, Some("RedactionEditor")),
        KeyBinding::new(
            if cfg!(target_os = "macos") {
                "cmd-d"
            } else {
                "ctrl-d"
            },
            DuplicateAnnotation,
            Some("RedactionEditor"),
        ),
    ]);
}

#[derive(Clone, Default)]
struct EditState {
    regions: Vec<PixelRegion>,
    frames: Vec<FrameAnnotation>,
    highlights: Vec<HighlightAnnotation>,
    pens: Vec<app_core::PenAnnotation>,
    arrows: Vec<ArrowAnnotation>,
    ellipses: Vec<FrameAnnotation>,
    pixelations: Vec<PixelRegion>,
    blurs: Vec<PixelRegion>,
    texts: Vec<Caption>,
    order: Vec<Selected>,
    crop: Option<PixelRegion>,
    last_number: u16,
}
#[derive(Default)]
struct RectEdits {
    current: EditState,
    past: Vec<EditState>,
    undone: Vec<EditState>,
}
impl std::ops::Deref for RectEdits {
    type Target = EditState;
    fn deref(&self) -> &EditState {
        &self.current
    }
}
impl std::ops::DerefMut for RectEdits {
    fn deref_mut(&mut self) -> &mut EditState {
        &mut self.current
    }
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Selected {
    Pen(usize),
    Highlight(usize),
    Text(usize),
    Blur(usize),
    Pixelation(usize),
    Redaction(usize),
    Frame(usize),
    Arrow(usize),
    Ellipse(usize),
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum ResizeHandle {
    Corner([bool; 2]),
    ArrowStart,
    ArrowEnd,
    StrokeStart,
    StrokeEnd,
}
impl EditState {
    fn filled(&self, selected: Selected) -> Option<bool> {
        match selected {
            Selected::Frame(i) => self.frames.get(i).map(|shape| shape.filled),
            Selected::Ellipse(i) => self.ellipses.get(i).map(|shape| shape.filled),
            _ => None,
        }
    }
    fn text_mask_bytes(&self) -> usize {
        self.texts
            .iter()
            .map(|text| text.asset.stamp.mask.len())
            .sum()
    }
    fn duplicate_target(&self, selected: Selected) -> Result<Selected, &'static str> {
        self.bounds(selected)
            .ok_or("Select an existing mark to duplicate.")?;
        if self.order.len() + self.regions.len() + self.pixelations.len() + self.blurs.len() >= 100
        {
            return Err("The document already has 100 marks. Remove a mark before duplicating.");
        }
        match selected {
            Selected::Pen(i)
                if self.pen_points() + self.pens[i].points.len()
                    > app_core::MAX_DOCUMENT_PEN_POINTS =>
            {
                Err("Pen point limit reached. Remove a stroke before duplicating.")
            }
            Selected::Text(i)
                if self.text_mask_bytes() + self.texts[i].asset.stamp.mask.len()
                    > 4 * 1024 * 1024 =>
            {
                Err("Text export limit reached. Remove a label before duplicating.")
            }
            Selected::Pen(_) => Ok(Selected::Pen(self.pens.len())),
            Selected::Text(_) => Ok(Selected::Text(self.texts.len())),
            Selected::Frame(_) => Ok(Selected::Frame(self.frames.len())),
            Selected::Arrow(_) => Ok(Selected::Arrow(self.arrows.len())),
            Selected::Ellipse(_) => Ok(Selected::Ellipse(self.ellipses.len())),
            Selected::Highlight(_) => Ok(Selected::Highlight(self.highlights.len())),
            Selected::Redaction(_) => Ok(Selected::Redaction(self.regions.len())),
            Selected::Blur(_) => Ok(Selected::Blur(self.blurs.len())),
            Selected::Pixelation(_) => Ok(Selected::Pixelation(self.pixelations.len())),
        }
    }
    fn stroke_width(&self, selected: Selected) -> Option<u8> {
        match selected {
            Selected::Pen(i) => self.pens.get(i).map(|mark| mark.width),
            Selected::Frame(i) => self.frames.get(i).map(|mark| mark.width),
            Selected::Ellipse(i) => self.ellipses.get(i).map(|mark| mark.width),
            Selected::Arrow(i) => self.arrows.get(i).map(|mark| mark.width),
            _ => None,
        }
    }
    fn pen_points(&self) -> usize {
        self.pens.iter().map(|pen| pen.points.len()).sum()
    }
    fn layer_target(&self, selected: Selected, forward: bool) -> Option<(usize, usize)> {
        let index = self.order.iter().position(|item| *item == selected)?;
        let target = if forward {
            index.checked_add(1)?
        } else {
            index.checked_sub(1)?
        };
        (target < self.order.len()).then_some((index, target))
    }
    fn ordered_annotations(&self) -> Vec<app_core::DrawAnnotation> {
        self.order
            .iter()
            .map(|selected| match *selected {
                Selected::Pen(i) => app_core::DrawAnnotation::Pen(self.pens[i].clone()),
                Selected::Highlight(i) => app_core::DrawAnnotation::Highlight(self.highlights[i]),
                Selected::Frame(i) => app_core::DrawAnnotation::Frame(self.frames[i]),
                Selected::Arrow(i) => app_core::DrawAnnotation::Arrow(self.arrows[i]),
                Selected::Ellipse(i) => app_core::DrawAnnotation::Ellipse(self.ellipses[i]),
                Selected::Text(i) => {
                    let mut stamp = self.texts[i].asset.stamp.clone();
                    stamp.area = self.texts[i].area;
                    app_core::DrawAnnotation::Text(stamp)
                }
                _ => unreachable!("source effects and redactions have fixed layers"),
            })
            .collect()
    }
    fn color(&self, selected: Selected) -> Option<u32> {
        match selected {
            Selected::Pen(i) => self.pens.get(i).map(|pen| pen.color),
            Selected::Highlight(i) => self.highlights.get(i).map(|mark| mark.color),
            Selected::Frame(i) => self.frames.get(i).map(|mark| mark.color),
            Selected::Arrow(i) => self.arrows.get(i).map(|mark| mark.color),
            Selected::Ellipse(i) => self.ellipses.get(i).map(|mark| mark.color),
            Selected::Redaction(_)
            | Selected::Pixelation(_)
            | Selected::Blur(_)
            | Selected::Text(_) => None,
        }
    }
    fn handle_at(
        &self,
        selected: Selected,
        point: [f32; 2],
        tolerance: f32,
    ) -> Option<ResizeHandle> {
        self.handles(selected)
            .into_iter()
            .map(|(handle, position)| {
                (
                    handle,
                    (position[0] - point[0]).hypot(position[1] - point[1]),
                )
            })
            .filter(|(_, distance)| *distance <= tolerance)
            .min_by(|a, b| a.1.total_cmp(&b.1))
            .map(|(handle, _)| handle)
    }
    fn handles(&self, selected: Selected) -> Vec<(ResizeHandle, [f32; 2])> {
        if let Selected::Pen(i) = selected {
            if let Some(pen) = self.pens.get(i).filter(|pen| pen.points.len() == 2) {
                return [ResizeHandle::StrokeStart, ResizeHandle::StrokeEnd]
                    .into_iter()
                    .zip(pen.positions())
                    .collect();
            }
        }
        if let Selected::Arrow(i) = selected {
            return self.arrows.get(i).map_or_else(Vec::new, |arrow| {
                vec![
                    (
                        ResizeHandle::ArrowStart,
                        arrow.start.map(|v| v as f32 + 0.5),
                    ),
                    (ResizeHandle::ArrowEnd, arrow.end.map(|v| v as f32 + 0.5)),
                ]
            });
        }
        self.bounds(selected).map_or_else(Vec::new, |area| {
            [[false, false], [true, false], [true, true], [false, true]]
                .map(|corner| {
                    (
                        ResizeHandle::Corner(corner),
                        [
                            (area.x + if corner[0] { area.width } else { 0 }) as f32,
                            (area.y + if corner[1] { area.height } else { 0 }) as f32,
                        ],
                    )
                })
                .to_vec()
        })
    }
    fn resize(
        &mut self,
        selected: Selected,
        handle: ResizeHandle,
        delta: [f32; 2],
        image: [u32; 2],
    ) -> bool {
        if image.contains(&0) || delta.iter().any(|v| !v.is_finite()) {
            return false;
        }
        if let Selected::Pen(i) = selected {
            if matches!(handle, ResizeHandle::StrokeStart | ResizeHandle::StrokeEnd) {
                let Some(pen) = self.pens.get(i).filter(|pen| pen.points.len() == 2) else {
                    return false;
                };
                let mut endpoints = [[0u32; 2]; 2];
                for (output, point) in endpoints.iter_mut().zip(pen.positions()) {
                    *output = std::array::from_fn(|axis| {
                        (point[axis] - 0.5)
                            .round()
                            .clamp(0.0, (image[axis] - 1) as f32) as u32
                    });
                }
                let moving = if handle == ResizeHandle::StrokeStart {
                    0
                } else {
                    1
                };
                let next = std::array::from_fn(|axis| {
                    i64::from(endpoints[moving][axis])
                        .saturating_add(delta[axis].round() as i64)
                        .clamp(0, i64::from(image[axis]) - 1) as u32
                });
                if next == endpoints[moving] || next == endpoints[1 - moving] {
                    return false;
                }
                endpoints[moving] = next;
                let Some(mut updated) = app_core::PenAnnotation::from_trace(&endpoints, pen.color)
                else {
                    return false;
                };
                updated.width = pen.width;
                self.pens[i] = updated;
                return true;
            }
        }
        if let Selected::Arrow(i) = selected {
            let Some(arrow) = self.arrows.get_mut(i) else {
                return false;
            };
            let (point, opposite) = match handle {
                ResizeHandle::ArrowStart => (&mut arrow.start, arrow.end),
                ResizeHandle::ArrowEnd => (&mut arrow.end, arrow.start),
                _ => return false,
            };
            let next = std::array::from_fn(|axis| {
                i64::from(point[axis])
                    .saturating_add(delta[axis].round() as i64)
                    .clamp(0, i64::from(image[axis]) - 1) as u32
            });
            if next == opposite || next == *point {
                return false;
            }
            *point = next;
            return true;
        }
        let ResizeHandle::Corner(corner) = handle else {
            return false;
        };
        let Some(area) = self.bounds(selected) else {
            return false;
        };
        let mut next = area;
        for axis in 0..2 {
            let (origin, extent) = if axis == 0 {
                (area.x, area.width)
            } else {
                (area.y, area.height)
            };
            let fixed = if corner[axis] {
                origin
            } else {
                origin + extent
            };
            let moving = if corner[axis] {
                origin + extent
            } else {
                origin
            };
            let target = i64::from(moving) + delta[axis].round() as i64;
            let target = if corner[axis] {
                target.clamp(i64::from(fixed) + 1, i64::from(image[axis]))
            } else {
                target.clamp(0, i64::from(fixed) - 1)
            } as u32;
            let (start, length) = (fixed.min(target), fixed.abs_diff(target));
            if axis == 0 {
                next.x = start;
                next.width = length;
            } else {
                next.y = start;
                next.height = length;
            }
        }
        if next == area {
            return false;
        }
        match selected {
            Selected::Redaction(i) => self.regions[i] = next,
            Selected::Pixelation(i) => self.pixelations[i] = next,
            Selected::Blur(i) => self.blurs[i] = next,
            Selected::Text(i) => self.texts[i].area = next,
            Selected::Frame(i) => self.frames[i].area = next,
            Selected::Highlight(i) => self.highlights[i].area = next,
            Selected::Pen(i) => self.pens[i].area = next,
            Selected::Ellipse(i) => self.ellipses[i].area = next,
            Selected::Arrow(_) => unreachable!(),
        }
        true
    }
    fn transform(
        &mut self,
        selected: Selected,
        handle: Option<ResizeHandle>,
        delta: [f32; 2],
        image: [u32; 2],
    ) -> bool {
        match handle {
            Some(handle) => self.resize(selected, handle, delta, image),
            None => self.translate(selected, delta, image),
        }
    }
    fn bounds(&self, selected: Selected) -> Option<PixelRegion> {
        match selected {
            Selected::Redaction(i) => self.regions.get(i).copied(),
            Selected::Pixelation(i) => self.pixelations.get(i).copied(),
            Selected::Blur(i) => self.blurs.get(i).copied(),
            Selected::Text(i) => self.texts.get(i).map(|text| text.area),
            Selected::Frame(i) => self.frames.get(i).map(|item| item.area),
            Selected::Highlight(i) => self.highlights.get(i).map(|item| item.area),
            Selected::Pen(i) => self.pens.get(i).map(|pen| pen.area),
            Selected::Ellipse(i) => self.ellipses.get(i).map(|item| item.area),
            Selected::Arrow(i) => self.arrows.get(i).map(|item| PixelRegion {
                x: item.start[0].min(item.end[0]),
                y: item.start[1].min(item.end[1]),
                width: item.start[0].abs_diff(item.end[0]) + 1,
                height: item.start[1].abs_diff(item.end[1]) + 1,
            }),
        }
    }
    fn pick(&self, point: [f32; 2], tolerance: f32) -> Option<Selected> {
        // Match the existing paint order, checking the visible foreground first.
        let candidates = (0..self.regions.len())
            .rev()
            .map(Selected::Redaction)
            .chain(self.order.iter().rev().copied())
            .chain((0..self.pixelations.len()).rev().map(Selected::Pixelation))
            .chain((0..self.blurs.len()).rev().map(Selected::Blur));
        candidates.into_iter().find(|selected| {
            let area = self.bounds(*selected).unwrap();
            let x = point[0] - area.x as f32;
            let y = point[1] - area.y as f32;
            let w = area.width as f32;
            let h = area.height as f32;
            let margin = tolerance
                + match *selected {
                    Selected::Pen(index) => f32::from(self.pens[index].width) / 2.0,
                    Selected::Arrow(index) => f32::from(self.arrows[index].width) * 7.0 / 3.0,
                    _ => 0.0,
                };
            if x < -margin || y < -margin || x > w + margin || y > h + margin {
                return false;
            }
            match *selected {
                Selected::Pen(i) => {
                    self.pens[i].distance(point) <= f32::from(self.pens[i].width) / 2.0 + tolerance
                }
                Selected::Redaction(_)
                | Selected::Pixelation(_)
                | Selected::Blur(_)
                | Selected::Text(_)
                | Selected::Highlight(_) => x >= 0.0 && y >= 0.0 && x < w && y < h,
                Selected::Frame(i) => {
                    if self.frames[i].filled {
                        x >= 0.0 && y >= 0.0 && x < w && y < h
                    } else {
                        x.min(w - x).min(y.min(h - y))
                            <= f32::from(self.frames[i].width) + tolerance
                    }
                }
                Selected::Ellipse(i) => {
                    let width = f32::from(self.ellipses[i].width);
                    let rx = w / 2.0;
                    let ry = h / 2.0;
                    let outer = ((x - rx) / (rx + tolerance)).powi(2)
                        + ((y - ry) / (ry + tolerance)).powi(2);
                    let inner = if !self.ellipses[i].filled
                        && rx > width + tolerance
                        && ry > width + tolerance
                    {
                        ((x - rx) / (rx - width - tolerance)).powi(2)
                            + ((y - ry) / (ry - width - tolerance)).powi(2)
                    } else {
                        2.0
                    };
                    outer <= 1.0 && inner >= 1.0
                }
                Selected::Arrow(i) => self.arrows[i].hit_test(point, tolerance),
            }
        })
    }
    fn translate(&mut self, selected: Selected, delta: [f32; 2], image: [u32; 2]) -> bool {
        let Some(area) = self.bounds(selected) else {
            return false;
        };
        let delta: [i64; 2] = std::array::from_fn(|axis| {
            let (origin, extent) = if axis == 0 {
                (area.x, area.width)
            } else {
                (area.y, area.height)
            };
            (delta[axis].round() as i64).clamp(
                -i64::from(origin),
                i64::from(image[axis]) - i64::from(origin) - i64::from(extent),
            )
        });
        if delta == [0, 0] {
            return false;
        }
        let shift = |value: u32, axis: usize| (i64::from(value) + delta[axis]) as u32;
        match selected {
            Selected::Arrow(i) => {
                let arrow = &mut self.arrows[i];
                for point in [&mut arrow.start, &mut arrow.end] {
                    *point = [shift(point[0], 0), shift(point[1], 1)];
                }
            }
            _ => {
                let area = match selected {
                    Selected::Redaction(i) => &mut self.regions[i],
                    Selected::Pixelation(i) => &mut self.pixelations[i],
                    Selected::Blur(i) => &mut self.blurs[i],
                    Selected::Text(i) => &mut self.texts[i].area,
                    Selected::Frame(i) => &mut self.frames[i].area,
                    Selected::Highlight(i) => &mut self.highlights[i].area,
                    Selected::Pen(i) => &mut self.pens[i].area,
                    Selected::Ellipse(i) => &mut self.ellipses[i].area,
                    _ => unreachable!(),
                };
                area.x = shift(area.x, 0);
                area.y = shift(area.y, 1);
            }
        }
        true
    }
}
impl RectEdits {
    fn duplicate(&mut self, selected: Selected, image: [u32; 2]) -> Result<Selected, &'static str> {
        let target = self.duplicate_target(selected)?;
        let area = self.bounds(selected).unwrap();
        if area
            .x
            .checked_add(area.width)
            .is_none_or(|right| right > image[0])
            || area
                .y
                .checked_add(area.height)
                .is_none_or(|bottom| bottom > image[1])
        {
            return Err("The selected mark is outside the image.");
        }
        let delta = std::array::from_fn(|axis| {
            let (origin, extent) = if axis == 0 {
                (area.x, area.width)
            } else {
                (area.y, area.height)
            };
            let available = image[axis] - origin - extent;
            if available >= 10 {
                10.0
            } else if origin >= 10 {
                -10.0
            } else {
                available as f32
            }
        });
        self.checkpoint();
        match selected {
            Selected::Pen(i) => {
                let copy = self.pens[i].clone();
                self.pens.push(copy);
            }
            Selected::Text(i) => {
                let copy = self.texts[i].clone();
                self.texts.push(copy);
            }
            Selected::Frame(i) => {
                let copy = self.frames[i];
                self.frames.push(copy);
            }
            Selected::Arrow(i) => {
                let copy = self.arrows[i];
                self.arrows.push(copy);
            }
            Selected::Ellipse(i) => {
                let copy = self.ellipses[i];
                self.ellipses.push(copy);
            }
            Selected::Highlight(i) => {
                let copy = self.highlights[i];
                self.highlights.push(copy);
            }
            Selected::Redaction(i) => {
                let copy = self.regions[i];
                self.regions.push(copy);
            }
            Selected::Blur(i) => {
                let copy = self.blurs[i];
                self.blurs.push(copy);
            }
            Selected::Pixelation(i) => {
                let copy = self.pixelations[i];
                self.pixelations.push(copy);
            }
        }
        if self.order.contains(&selected) {
            self.order.push(target);
        }
        self.current.translate(target, delta, image);
        Ok(target)
    }
    fn add_pen(&mut self, pen: app_core::PenAnnotation) -> bool {
        if self.regions.len()
            + self.frames.len()
            + self.highlights.len()
            + self.pens.len()
            + self.arrows.len()
            + self.ellipses.len()
            + self.pixelations.len()
            + self.blurs.len()
            + self.texts.len()
            < 100
            && self.pen_points() + pen.points.len() <= app_core::MAX_DOCUMENT_PEN_POINTS
        {
            self.checkpoint();
            let index = self.pens.len();
            self.order.push(Selected::Pen(index));
            self.pens.push(pen);
            true
        } else {
            false
        }
    }
    fn add_highlight(&mut self, highlight: HighlightAnnotation) {
        if self.regions.len()
            + self.frames.len()
            + self.highlights.len()
            + self.pens.len()
            + self.arrows.len()
            + self.ellipses.len()
            + self.pixelations.len()
            + self.blurs.len()
            + self.texts.len()
            < 100
        {
            self.checkpoint();
            let index = self.highlights.len();
            self.order.push(Selected::Highlight(index));
            self.highlights.push(highlight);
        }
    }
    fn shift_layer(&mut self, selected: Selected, forward: bool) {
        if let Some((from, to)) = self.layer_target(selected, forward) {
            self.checkpoint();
            self.order.swap(from, to);
        }
    }
    fn set_highlight_opacity(&mut self, selected: Selected, opacity: u8) {
        let Selected::Highlight(index) = selected else {
            return;
        };
        if opacity == 0
            || self
                .highlights
                .get(index)
                .is_none_or(|mark| mark.opacity == opacity)
        {
            return;
        }
        self.checkpoint();
        self.highlights[index].opacity = opacity;
    }
    fn set_filled(&mut self, selected: Selected, filled: bool) {
        if self
            .filled(selected)
            .is_none_or(|previous| previous == filled)
        {
            return;
        }
        self.checkpoint();
        match selected {
            Selected::Frame(i) => self.frames[i].filled = filled,
            Selected::Ellipse(i) => self.ellipses[i].filled = filled,
            _ => unreachable!(),
        }
    }
    fn set_stroke_width(&mut self, selected: Selected, width: u8) {
        if !(1..=12).contains(&width)
            || self
                .stroke_width(selected)
                .is_none_or(|previous| previous == width)
        {
            return;
        }
        self.checkpoint();
        match selected {
            Selected::Pen(i) => self.pens[i].width = width,
            Selected::Frame(i) => self.frames[i].width = width,
            Selected::Ellipse(i) => self.ellipses[i].width = width,
            Selected::Arrow(i) => self.arrows[i].width = width,
            _ => unreachable!(),
        }
    }
    fn set_text(&mut self, text: Caption, index: Option<usize>) -> bool {
        let replaced_bytes = index
            .and_then(|i| self.texts.get(i))
            .map_or(0, |text| text.asset.stamp.mask.len());
        if self.text_mask_bytes() - replaced_bytes + text.asset.stamp.mask.len() > 4 * 1024 * 1024 {
            return false;
        }
        if let Some(index) = index {
            if index >= self.texts.len() {
                return false;
            }
            self.checkpoint();
            self.texts[index] = text;
        } else if self.regions.len()
            + self.frames.len()
            + self.highlights.len()
            + self.pens.len()
            + self.arrows.len()
            + self.ellipses.len()
            + self.pixelations.len()
            + self.blurs.len()
            + self.texts.len()
            < 100
        {
            self.checkpoint();
            let index = self.texts.len();
            self.order.push(Selected::Text(index));
            self.texts.push(text);
        } else {
            return false;
        }
        true
    }
    fn add_number(&mut self, caption: Caption) -> bool {
        let Some(number) = self.last_number.checked_add(1) else {
            return false;
        };
        if self.order.len() + self.regions.len() + self.pixelations.len() + self.blurs.len() >= 100
        {
            return false;
        }
        if !self.set_text(caption, None) {
            return false;
        }
        self.last_number = number;
        true
    }
    fn restart_numbers(&mut self) {
        if self.last_number != 0 {
            self.checkpoint();
            self.last_number = 0;
        }
    }
    fn set_color(&mut self, selected: Selected, color: u32) {
        let Some(previous) = self.color(selected) else {
            return;
        };
        if previous == color || color > 0xffffff {
            return;
        }
        self.checkpoint();
        match selected {
            Selected::Frame(i) => self.frames[i].color = color,
            Selected::Highlight(i) => self.highlights[i].color = color,
            Selected::Pen(i) => self.pens[i].color = color,
            Selected::Arrow(i) => self.arrows[i].color = color,
            Selected::Ellipse(i) => self.ellipses[i].color = color,
            Selected::Redaction(_)
            | Selected::Pixelation(_)
            | Selected::Blur(_)
            | Selected::Text(_) => unreachable!(),
        }
    }
    fn snapshot(&self) -> EditState {
        self.current.clone()
    }
    fn checkpoint(&mut self) {
        if self.past.len() == 100 {
            self.past.remove(0);
        }
        self.past.push(self.snapshot());
        self.undone.clear();
    }
    fn restore(&mut self, state: EditState) {
        self.current = state;
    }
    fn transform_selected(
        &mut self,
        selected: Selected,
        handle: Option<ResizeHandle>,
        delta: [f32; 2],
        image: [u32; 2],
    ) {
        let mut next = self.snapshot();
        if next.transform(selected, handle, delta, image) {
            self.checkpoint();
            self.current = next;
        }
    }
    fn delete_selected(&mut self, selected: Selected) {
        if self.bounds(selected).is_none() {
            return;
        }
        self.checkpoint();
        self.order.retain(|item| *item != selected);
        for item in &mut self.order {
            *item = match (*item, selected) {
                (Selected::Pen(i), Selected::Pen(removed)) if i > removed => Selected::Pen(i - 1),
                (Selected::Highlight(i), Selected::Highlight(removed)) if i > removed => {
                    Selected::Highlight(i - 1)
                }
                (Selected::Frame(i), Selected::Frame(removed)) if i > removed => {
                    Selected::Frame(i - 1)
                }
                (Selected::Arrow(i), Selected::Arrow(removed)) if i > removed => {
                    Selected::Arrow(i - 1)
                }
                (Selected::Ellipse(i), Selected::Ellipse(removed)) if i > removed => {
                    Selected::Ellipse(i - 1)
                }
                (Selected::Text(i), Selected::Text(removed)) if i > removed => {
                    Selected::Text(i - 1)
                }
                _ => *item,
            };
        }
        match selected {
            Selected::Redaction(i) => {
                self.regions.remove(i);
            }
            Selected::Pixelation(i) => {
                self.pixelations.remove(i);
            }
            Selected::Blur(i) => {
                self.blurs.remove(i);
            }
            Selected::Text(i) => {
                self.texts.remove(i);
            }
            Selected::Frame(i) => {
                self.frames.remove(i);
            }
            Selected::Highlight(i) => {
                self.highlights.remove(i);
            }
            Selected::Pen(i) => {
                self.pens.remove(i);
            }
            Selected::Arrow(i) => {
                self.arrows.remove(i);
            }
            Selected::Ellipse(i) => {
                self.ellipses.remove(i);
            }
        }
    }
    fn add(&mut self, region: PixelRegion) {
        if self.regions.len()
            + self.frames.len()
            + self.highlights.len()
            + self.pens.len()
            + self.arrows.len()
            + self.ellipses.len()
            + self.pixelations.len()
            + self.blurs.len()
            + self.texts.len()
            < 100
        {
            self.checkpoint();
            self.regions.push(region);
        }
    }
    fn add_frame(&mut self, frame: FrameAnnotation) {
        if self.regions.len()
            + self.frames.len()
            + self.highlights.len()
            + self.pens.len()
            + self.arrows.len()
            + self.ellipses.len()
            + self.pixelations.len()
            + self.blurs.len()
            + self.texts.len()
            < 100
        {
            self.checkpoint();
            let index = self.frames.len();
            self.order.push(Selected::Frame(index));
            self.frames.push(frame);
        }
    }
    fn add_arrow(&mut self, arrow: ArrowAnnotation) {
        if self.regions.len()
            + self.frames.len()
            + self.highlights.len()
            + self.pens.len()
            + self.arrows.len()
            + self.ellipses.len()
            + self.pixelations.len()
            + self.blurs.len()
            + self.texts.len()
            < 100
        {
            self.checkpoint();
            let index = self.arrows.len();
            self.order.push(Selected::Arrow(index));
            self.arrows.push(arrow);
        }
    }
    fn add_ellipse(&mut self, ellipse: FrameAnnotation) {
        if self.regions.len()
            + self.frames.len()
            + self.highlights.len()
            + self.pens.len()
            + self.arrows.len()
            + self.ellipses.len()
            + self.pixelations.len()
            + self.blurs.len()
            + self.texts.len()
            < 100
        {
            self.checkpoint();
            let index = self.ellipses.len();
            self.order.push(Selected::Ellipse(index));
            self.ellipses.push(ellipse);
        }
    }
    fn add_pixelation(&mut self, area: PixelRegion) {
        if self.regions.len()
            + self.frames.len()
            + self.highlights.len()
            + self.pens.len()
            + self.arrows.len()
            + self.ellipses.len()
            + self.pixelations.len()
            + self.blurs.len()
            + self.texts.len()
            < 100
        {
            self.checkpoint();
            self.pixelations.push(area);
        }
    }
    fn add_blur(&mut self, area: PixelRegion) {
        if self.regions.len()
            + self.frames.len()
            + self.highlights.len()
            + self.pens.len()
            + self.arrows.len()
            + self.ellipses.len()
            + self.pixelations.len()
            + self.blurs.len()
            + self.texts.len()
            < 100
        {
            self.checkpoint();
            self.blurs.push(area);
        }
    }
    fn set_crop(&mut self, crop: Option<PixelRegion>) {
        if self.crop != crop {
            self.checkpoint();
            self.crop = crop;
        }
    }
    fn undo(&mut self) {
        if let Some(state) = self.past.pop() {
            self.undone.push(self.snapshot());
            self.restore(state);
        }
    }
    fn redo(&mut self) {
        if let Some(state) = self.undone.pop() {
            self.past.push(self.snapshot());
            self.restore(state);
        }
    }
    fn reset(&mut self) {
        self.regions.clear();
        self.frames.clear();
        self.highlights.clear();
        self.pens.clear();
        self.arrows.clear();
        self.ellipses.clear();
        self.pixelations.clear();
        self.blurs.clear();
        self.texts.clear();
        self.order.clear();
        self.crop = None;
        self.last_number = 0;
        self.past.clear();
        self.undone.clear();
    }
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Tool {
    Line,
    Spotlight,
    Number,
    Pen,
    Highlight,
    Text,
    Blur,
    Pixelate,
    Select,
    Redact,
    Crop,
    Frame,
    Arrow,
    Ellipse,
}
impl Tool {
    fn shortcut(key: &str) -> Option<Self> {
        match key {
            "l" => Some(Self::Line),
            "v" => Some(Self::Select),
            "x" => Some(Self::Redact),
            "c" => Some(Self::Crop),
            "r" => Some(Self::Frame),
            "a" => Some(Self::Arrow),
            "e" => Some(Self::Ellipse),
            "p" => Some(Self::Pixelate),
            "b" => Some(Self::Blur),
            "t" => Some(Self::Text),
            "n" => Some(Self::Number),
            "h" => Some(Self::Highlight),
            "s" => Some(Self::Spotlight),
            "d" => Some(Self::Pen),
            _ => None,
        }
    }
}
fn nudge_delta(key: &str, shift: bool) -> Option<[f32; 2]> {
    let step = if shift { 10.0 } else { 1.0 };
    match key {
        "left" => Some([-step, 0.0]),
        "right" => Some([step, 0.0]),
        "up" => Some([0.0, -step]),
        "down" => Some([0.0, step]),
        _ => None,
    }
}

pub(super) struct RedactionEditor {
    pub source: CapturedImage,
    pub render_image: Arc<gpui::RenderImage>,
    focus: FocusHandle,
    owner: gpui::WeakEntity<SpikeShell>,
    direct_capture: bool,
    toolbar_bottom: bool,
    edits: RectEdits,
    drag: Option<[f32; 2]>,
    end: [f32; 2],
    layout: Option<[f32; 2]>,
    tool: Tool,
    selected: Option<Selected>,
    resize_handle: Option<ResizeHandle>,
    frame_color: u32,
    highlight_opacity: u8,
    spotlight_color: u32,
    spotlight_opacity: u8,
    stroke_width: u8,
    shape_filled: bool,
    zoom: Option<f32>,
    pan: [f32; 2],
    pixelation: Option<(Arc<app_core::PixelationGrid>, Arc<gpui::RenderImage>)>,
    effects_cancel: Arc<std::sync::atomic::AtomicBool>,
    pixelation_loading: bool,
    pixelation_error: Option<String>,
    blur: Option<Arc<gpui::RenderImage>>,
    blur_loading: bool,
    blur_error: Option<String>,
    pending_effect: Option<Tool>,
    text_draft: Option<TextDraft>,
    text_assets: Vec<Arc<TextAsset>>,
    text_size: u32,
    text_bold: bool,
    text_italic: bool,
    text_loading: bool,
    text_error: Option<String>,
    text_job: u64,
    text_cancel: Option<Arc<std::sync::atomic::AtomicBool>>,
    pen_trace: Vec<[u32; 2]>,
    pen_limit_hit: bool,
    pen_capacity: usize,
}
#[derive(Clone)]
struct Caption {
    area: PixelRegion,
    asset: Arc<TextAsset>,
}
struct TextDraft {
    position: [u32; 2],
    editing: Option<usize>,
    input: gpui::Entity<CaptionInput>,
}
impl Drop for RedactionEditor {
    fn drop(&mut self) {
        self.effects_cancel
            .store(true, std::sync::atomic::Ordering::Relaxed);
    }
}

impl RedactionEditor {
    fn cancel_pen(&mut self, cx: &mut Context<Self>) {
        self.drag = None;
        self.pen_trace.clear();
        self.pen_limit_hit = false;
        cx.notify();
    }
    fn sample_pen(&mut self, point: [u32; 2], force: bool) -> bool {
        if let Some(last) = self.pen_trace.last() {
            let dx = u64::from(last[0].abs_diff(point[0]));
            let dy = u64::from(last[1].abs_diff(point[1]));
            if *last == point || (!force && dx * dx + dy * dy < 4) {
                return false;
            }
        }
        if self.pen_trace.len() >= self.pen_capacity {
            let changed = !self.pen_limit_hit;
            self.pen_limit_hit = true;
            return changed;
        }
        self.pen_trace.push(point);
        true
    }
    fn begin_pen(
        &mut self,
        position: [f32; 2],
        scale: f32,
        image: [u32; 2],
        cx: &mut Context<Self>,
    ) {
        self.pen_trace.clear();
        self.pen_limit_hit = false;
        self.pen_capacity = app_core::MAX_PEN_POINTS
            .min(app_core::MAX_DOCUMENT_PEN_POINTS.saturating_sub(self.edits.pen_points()));
        if self.pen_capacity == 0
            || self.edits.order.len()
                + self.edits.regions.len()
                + self.edits.blurs.len()
                + self.edits.pixelations.len()
                >= 100
        {
            self.pen_limit_hit = true;
            cx.notify();
            return;
        }
        self.drag = Some(position);
        self.end = position;
        self.sample_pen(pen_point(position, scale, image), true);
        cx.notify();
    }
    fn finish_pen(
        &mut self,
        position: [f32; 2],
        scale: f32,
        image: [u32; 2],
        cx: &mut Context<Self>,
    ) {
        if self.tool != Tool::Pen || self.drag.is_none() {
            return;
        }
        self.sample_pen(pen_point(position, scale, image), true);
        self.drag = None;
        if let Some(mut pen) =
            app_core::PenAnnotation::from_trace(&self.pen_trace, self.frame_color)
        {
            pen.width = self.stroke_width;
            let index = self.edits.pens.len();
            if self.edits.add_pen(pen) {
                self.selected = Some(Selected::Pen(index));
            }
        }
        self.pen_trace.clear();
        cx.notify();
    }
    fn move_layer(&mut self, forward: bool, cx: &mut Context<Self>) {
        if self.tool != Tool::Select
            || self.drag.is_some()
            || self.text_draft.is_some()
            || self.text_loading
        {
            return;
        }
        if let Some(selected) = self.selected {
            self.edits.shift_layer(selected, forward);
            cx.notify();
        }
    }
    fn begin_text(
        &mut self,
        position: [u32; 2],
        editing: Option<usize>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.text_draft.is_some() || self.text_loading {
            return;
        }
        if editing.is_none()
            && self.edits.regions.len()
                + self.edits.frames.len()
                + self.edits.highlights.len()
                + self.edits.pens.len()
                + self.edits.arrows.len()
                + self.edits.ellipses.len()
                + self.edits.pixelations.len()
                + self.edits.blurs.len()
                + self.edits.texts.len()
                >= 100
        {
            self.text_error = Some("The document already has 100 marks".into());
            cx.notify();
            return;
        }
        let content = editing
            .and_then(|index| self.edits.texts.get(index))
            .map(|text| {
                self.text_size = text.asset.font_size;
                self.text_bold = text.asset.bold;
                self.text_italic = text.asset.italic;
                self.frame_color = text.asset.stamp.color;
                text.asset.text.clone()
            })
            .unwrap_or_default();
        let owner = window.window_handle().downcast::<Self>().unwrap();
        let input = cx.new(|cx| CaptionInput::new(content, owner, cx));
        let focus = input.read(cx).focus.clone();
        window.focus(&focus, cx);
        self.text_draft = Some(TextDraft {
            position,
            editing,
            input,
        });
        self.drag = None;
        self.text_error = None;
        self.tool = Tool::Text;
        cx.notify();
    }
    pub(super) fn cancel_text(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(cancelled) = &self.text_cancel {
            cancelled.store(true, std::sync::atomic::Ordering::Relaxed);
        }
        self.text_job = self.text_job.wrapping_add(1);
        self.text_draft = None;
        self.text_error = None;
        window.focus(&self.focus, cx);
        cx.notify();
    }
    pub(super) fn commit_text(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.text_loading || self.pixelation_loading || self.blur_loading {
            return;
        }
        let Some(draft) = &self.text_draft else {
            return;
        };
        let (text, position, editing) = (
            draft.input.read(cx).content.clone(),
            draft.position,
            draft.editing,
        );
        self.prepare_text(text, position, editing, window, cx);
    }
    fn begin_number(&mut self, position: [u32; 2], window: &mut Window, cx: &mut Context<Self>) {
        if self.text_draft.is_some() || self.text_loading {
            return;
        }
        if self.pixelation_loading || self.blur_loading {
            self.text_error =
                Some("Wait for the current preview preparation, then click again.".into());
            cx.notify();
            return;
        }
        let Some(number) = self.edits.last_number.checked_add(1) else {
            self.text_error = Some("Number limit reached. Choose Restart numbering.".into());
            cx.notify();
            return;
        };
        if self.edits.order.len()
            + self.edits.regions.len()
            + self.edits.pixelations.len()
            + self.edits.blurs.len()
            >= 100
        {
            self.text_error = Some("The document already has 100 marks".into());
            cx.notify();
            return;
        }
        self.tool = Tool::Number;
        self.drag = None;
        self.prepare_text(format!("{number}."), position, None, window, cx);
    }
    fn prepare_text(
        &mut self,
        text: String,
        position: [u32; 2],
        editing: Option<usize>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(fonts) = cx.try_global::<CaptionFonts>().map(|fonts| fonts.0.clone()) else {
            self.text_error = Some("Native text renderer is unavailable".into());
            cx.notify();
            return;
        };
        let number = self.tool == Tool::Number && self.text_draft.is_none();
        let (font_size, color) = (self.text_size, self.frame_color);
        let (bold, italic) = (self.text_bold, self.text_italic);
        let cancelled = self.effects_cancel.clone();
        let text_cancel = Arc::new(std::sync::atomic::AtomicBool::new(false));
        self.text_cancel = Some(text_cancel.clone());
        self.text_job = self.text_job.wrapping_add(1);
        let job = self.text_job;
        let owner = window.window_handle().downcast::<Self>().unwrap();
        if let Some(draft) = &self.text_draft {
            draft.input.update(cx, |input, cx| {
                input.enabled = false;
                cx.notify();
            });
        }
        self.text_loading = true;
        self.text_error = None;
        cx.notify();
        cx.spawn(async move|_,cx|{
            let result=cx.background_spawn(async move{crate::caption::rasterize(fonts.as_ref(),text,font_size,color,bold,italic,||cancelled.load(std::sync::atomic::Ordering::Relaxed)||text_cancel.load(std::sync::atomic::Ordering::Relaxed))}).await;
            owner.update(cx,|this,window,cx|{
                this.text_loading=false;
                this.text_cancel=None;
                if this.effects_cancel.load(std::sync::atomic::Ordering::Relaxed){return;}
                if this.text_job!=job {this.prepare_pending_effect(cx);cx.notify();return;}
                if let Some(draft)=&this.text_draft {draft.input.update(cx,|input,cx|{input.enabled=true;cx.notify();});}
                match result {
                    Ok(asset)=>{
                        this.text_assets.retain(|cached| {
                            if Arc::strong_count(cached)==1 {window.drop_image(cached.preview.clone()).ok();false}else{true}
                        });
                        if this.text_assets.len()>=200 || this.text_assets.iter().map(|asset|asset.bytes()).sum::<usize>()+asset.bytes()>4*1024*1024 {
                            this.text_error=Some("Text memory limit reached. Use shorter labels or Reset to clear this document.".into());
                        } else if editing.is_some_and(|index|index>=this.edits.texts.len()) {
                            this.text_error=Some("Selected label changed. Cancel and select it again.".into());
                        } else if asset.stamp.width>this.source.width || asset.stamp.height>this.source.height {
                            this.text_error=Some("Text does not fit this image. Choose a smaller size or shorter text.".into());
                        } else {
                            let natural=asset.stamp.area;
                            let width=natural.width.min(this.source.width);let height=natural.height.min(this.source.height);
                            let area=PixelRegion{x:position[0].min(this.source.width-width),y:position[1].min(this.source.height-height),width,height};
                            let asset=Arc::new(asset);
                            let selected=editing.unwrap_or(this.edits.texts.len());
                            if number {
                                if !this.edits.add_number(Caption{area,asset:asset.clone()}) {
                                    this.text_error=Some("The document cannot accept another number".into());
                                    this.prepare_pending_effect(cx);cx.notify();return;
                                }
                            } else if !this.edits.set_text(Caption{area,asset:asset.clone()},editing) {
                                this.text_error=Some("The document mark or text export limit is reached. Remove a mark or use shorter text.".into());
                                this.prepare_pending_effect(cx);cx.notify();return;
                            }
                            this.text_assets.push(asset);this.text_draft=None;this.selected=Some(Selected::Text(selected));if this.tool==Tool::Text {this.tool=Tool::Select;}window.focus(&this.focus,cx);
                        }
                    }
                    Err(error)=>this.text_error=Some(error),
                }
                this.prepare_pending_effect(cx);cx.notify();
            }).ok();
        }).detach();
    }
    #[cfg(test)]
    pub(super) fn source_effects_probe(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Result<bool, String> {
        if let Some(error) = self.pixelation_error.as_ref().or(self.blur_error.as_ref()) {
            return Err(error.clone());
        }
        if self.pixelation.is_none()
            && !self.pixelation_loading
            && self.blur.is_none()
            && !self.blur_loading
        {
            self.tool = Tool::Pixelate;
            self.edits.add_pixelation(PixelRegion {
                x: 0,
                y: 0,
                width: self.source.width.min(128),
                height: self.source.height.min(128),
            });
            self.prepare_pixelation(cx);
            self.tool = Tool::Blur;
            self.edits.add_blur(PixelRegion {
                x: 0,
                y: 0,
                width: self.source.width.min(64),
                height: self.source.height.min(64),
            });
            self.prepare_blur(cx);
            if self.blur_loading || self.pending_effect != Some(Tool::Blur) {
                return Err("Source effect preparations ran concurrently".into());
            }
        }
        if let Some(error) = &self.text_error {
            return Err(error.clone());
        }
        if self.pixelation.is_some()
            && self.blur.is_some()
            && self.edits.texts.is_empty()
            && !self.text_loading
            && self.text_draft.is_none()
        {
            self.begin_text([10, 10], None, window, cx);
            let input = self.text_draft.as_ref().unwrap().input.clone();
            input.update(cx, |input, cx| {
                gpui::EntityInputHandler::replace_text_in_range(
                    input,
                    None,
                    "Hello Grüße العربية\r\n\r\n中文 😀",
                    window,
                    cx,
                )
            });
        }
        if !self.text_loading
            && self.text_draft.is_none()
            && self
                .edits
                .texts
                .first()
                .is_some_and(|text| text.asset.text != "😀")
        {
            self.begin_text([10, 10], Some(0), window, cx);
            if self.text_bold || self.text_italic {
                return Err("Editing a regular label did not restore its font style".into());
            }
            self.text_bold = true;
            self.text_italic = true;
            let input = self.text_draft.as_ref().unwrap().input.clone();
            input.update(cx, |input, cx| {
                gpui::EntityInputHandler::replace_text_in_range(input, None, "😀", window, cx)
            });
        }
        if !self.text_loading
            && self
                .text_draft
                .as_ref()
                .is_some_and(|draft| draft.input.read(cx).has_current_layout())
        {
            let input = self.text_draft.as_ref().unwrap().input.clone();
            if input.read(cx).content.contains('\n') {
                input.update(cx, |input, cx| input.verify_multiline_input(window, cx))?;
            }
            self.commit_text(window, cx);
        }
        if !self.text_loading
            && self.edits.pens.is_empty()
            && self
                .edits
                .texts
                .first()
                .is_some_and(|text| text.asset.text == "😀")
        {
            if !self.edits.texts[0]
                .asset
                .stamp
                .mask
                .iter()
                .any(|alpha| *alpha > 0)
            {
                return Err("Native monochrome emoji mask is empty".into());
            }
            self.edits.undo();
            if self.edits.texts[0].asset.bold || self.edits.texts[0].asset.italic {
                return Err("Text style undo did not restore the regular asset".into());
            }
            if self.edits.texts[0].asset.text != "Hello Grüße العربية\n\n中文 😀" {
                return Err("Text edit did not preserve undo".into());
            }
            self.edits.redo();
            if !self.edits.texts[0].asset.bold || !self.edits.texts[0].asset.italic {
                return Err("Text style redo did not restore the bold italic asset".into());
            }
            let area = PixelRegion {
                x: 0,
                y: 0,
                width: self.source.width.min(64),
                height: self.source.height.min(64),
            };
            self.edits.add_frame(FrameAnnotation {
                filled: false,
                width: 3,
                area,
                color: 0x22c55e,
            });
            self.edits.add_ellipse(FrameAnnotation {
                filled: false,
                width: 3,
                area,
                color: 0xef4444,
            });
            self.edits.shift_layer(Selected::Text(0), true);
            for selected in [Selected::Frame(0), Selected::Ellipse(0)] {
                self.edits.set_filled(selected, true);
                self.edits.undo();
                if self.edits.filled(selected) != Some(false) {
                    return Err("Shape fill undo failed".into());
                }
                self.edits.redo();
                if self.edits.filled(selected) != Some(true) {
                    return Err("Shape fill redo failed".into());
                }
            }
            self.edits.shift_layer(Selected::Text(0), true);
            if self.edits.order.last() != Some(&Selected::Text(0)) {
                return Err("Text did not move above the vector layers".into());
            }
            self.edits.add_highlight(HighlightAnnotation {
                outside: false,
                opacity: 85,
                area,
                color: 0xfacc15,
            });
            self.edits.shift_layer(Selected::Highlight(0), false);
            self.edits
                .set_highlight_opacity(Selected::Highlight(0), 128);
            self.edits.undo();
            if self.edits.highlights[0].opacity != 85 {
                return Err("Highlight opacity undo failed".into());
            }
            self.edits.redo();
            if self.edits.highlights[0].opacity != 128 {
                return Err("Highlight opacity redo failed".into());
            }
            if self.edits.order.last() != Some(&Selected::Text(0)) {
                return Err("Highlight did not move below text".into());
            }
            self.edits.add_arrow(ArrowAnnotation {
                start: [0, 0],
                end: [
                    self.source.width.min(128) - 1,
                    self.source.height.min(64) - 1,
                ],
                color: 0x22c55e,
                width: 3,
            });
            self.edits.add_highlight(HighlightAnnotation {
                area,
                color: 0,
                opacity: 191,
                outside: true,
            });
            self.edits
                .set_highlight_opacity(Selected::Highlight(1), 128);
            self.edits.undo();
            if !self.edits.highlights[1].outside || self.edits.highlights[1].opacity != 191 {
                return Err("Spotlight opacity undo lost its outside fill".into());
            }
            self.edits.redo();
            for selected in [Selected::Frame(0), Selected::Ellipse(0), Selected::Arrow(0)] {
                self.edits.set_stroke_width(selected, 12);
                self.edits.undo();
                if self.edits.stroke_width(selected) != Some(3) {
                    return Err("Shape width undo failed".into());
                }
                self.edits.redo();
                if self.edits.stroke_width(selected) != Some(12) {
                    return Err("Shape width redo failed".into());
                }
            }
            self.tool = Tool::Pen;
            self.stroke_width = 12;
            let dimensions = [self.source.width, self.source.height];
            self.begin_pen([16.0, 16.0], 1.0, dimensions, cx);
            self.sample_pen([32, 40], false);
            self.finish_pen([48.0, 16.0], 1.0, dimensions, cx);
            if self
                .edits
                .pens
                .first()
                .is_none_or(|pen| pen.points.len() != 3 || pen.width != 12)
            {
                return Err("Pen gesture was not committed once".into());
            }
            self.edits.set_stroke_width(Selected::Pen(0), 6);
            self.edits.undo();
            if self.edits.pens[0].width != 12 {
                return Err("Pen width undo failed".into());
            }
            self.edits.redo();
            if self.edits.pens[0].width != 6 {
                return Err("Pen width redo failed".into());
            }
            self.begin_pen([16.0, 16.0], 1.0, dimensions, cx);
            for index in 0..1100 {
                self.sample_pen([20 + (index % 32) * 2, 20 + (index / 32 % 32) * 2], false);
            }
            if self.pen_trace.len() != app_core::MAX_PEN_POINTS || !self.pen_limit_hit {
                return Err("Pen sampling exceeded its limit or omitted the limit status".into());
            }
            self.cancel_pen(cx);
            if self.edits.pens.len() != 1 {
                return Err("Canceled pen gesture changed the document".into());
            }
            let line = line_from_drag(
                [64.0, 64.0],
                [120.0, 68.0],
                [dimensions[0] as f32, dimensions[1] as f32],
                dimensions,
                0x22c55e,
                6,
                true,
            )
            .ok_or("Line geometry unavailable")?;
            if !self.edits.add_pen(line) {
                return Err("Line admission failed".into());
            }
            self.edits.undo();
            self.edits.redo();
            if self.edits.pens.len() != 2
                || self.edits.pens[1].points.len() != 2
                || self.edits.pens[1].width != 6
            {
                return Err("Line metadata undo failed".into());
            }
            self.edits.transform_selected(
                Selected::Pen(1),
                Some(ResizeHandle::StrokeEnd),
                [10.0, 20.0],
                dimensions,
            );
            self.edits.undo();
            self.edits.redo();
            if self.edits.pens[1].positions().collect::<Vec<_>>()
                != vec![[64.5, 64.5], [130.5, 84.5]]
            {
                return Err("Line endpoint undo failed".into());
            }
            self.selected = Some(Selected::Pen(1));
            self.tool = Tool::Select;
            cx.notify();
        }
        let ready = self.pixelation.is_some()
            && self.blur.is_some()
            && self
                .edits
                .texts
                .first()
                .is_some_and(|text| text.asset.text == "😀")
            && !self.text_loading;
        if !ready {
            return Ok(false);
        }
        match self.edits.last_number {
            0 => {
                self.text_bold = false;
                self.text_italic = true;
                self.begin_number([100, 100], window, cx);
                Ok(false)
            }
            1 => {
                if self
                    .edits
                    .texts
                    .last()
                    .is_none_or(|text| text.asset.bold || !text.asset.italic)
                {
                    return Err("First number did not keep its italic style".into());
                }
                if self
                    .edits
                    .texts
                    .last()
                    .is_none_or(|text| text.asset.text != "1.")
                {
                    return Err("First step number was not prepared".into());
                }
                self.edits.undo();
                if self.edits.last_number != 0 || self.edits.texts.len() != 1 {
                    return Err("Number undo did not restore the sequence".into());
                }
                self.edits.redo();
                if self.edits.last_number != 1 || self.edits.texts.len() != 2 {
                    return Err("Number redo did not restore the sequence".into());
                }
                self.text_bold = true;
                self.text_italic = false;
                self.begin_number([self.source.width, self.source.height], window, cx);
                Ok(false)
            }
            2 => {
                let label = self.edits.texts.last().unwrap();
                if !label.asset.bold || label.asset.italic {
                    return Err("Second number did not keep its bold style".into());
                }
                if label.asset.text != "2."
                    || label.area.x + label.area.width != self.source.width
                    || label.area.y + label.area.height != self.source.height
                    || !label.asset.stamp.mask.iter().any(|alpha| *alpha > 0)
                {
                    return Err("Number placement did not clamp at the image edge".into());
                }
                self.edits.add(PixelRegion {
                    x: 1,
                    y: 1,
                    width: 4,
                    height: 4,
                });
                self.tool = Tool::Select;
                for selected in [
                    Selected::Text(2),
                    Selected::Pen(0),
                    Selected::Frame(0),
                    Selected::Arrow(0),
                    Selected::Ellipse(0),
                    Selected::Highlight(0),
                    Selected::Highlight(1),
                    Selected::Blur(0),
                    Selected::Pixelation(0),
                    Selected::Redaction(0),
                ] {
                    self.selected = Some(selected);
                    let history = self.edits.past.len();
                    self.duplicate_selection(cx);
                    let copy = self.selected.ok_or("Duplicate did not select a copy")?;
                    if copy == selected || self.edits.past.len() != history + 1 {
                        return Err("Duplicate did not create one undo step".into());
                    }
                    self.edits.undo();
                    if self.edits.bounds(copy).is_some() || self.edits.bounds(selected).is_none() {
                        return Err("Duplicate undo changed the original".into());
                    }
                    self.edits.redo();
                    if self.edits.bounds(copy).is_none() || self.edits.last_number != 2 {
                        return Err(
                            "Duplicate redo changed the number sequence or lost a mark".into()
                        );
                    }
                }
                cx.notify();
                Ok(true)
            }
            _ => Err("Unexpected numbering state".into()),
        }
    }
    pub(super) fn release_images(&self, window: &mut Window) {
        self.effects_cancel
            .store(true, std::sync::atomic::Ordering::Relaxed);
        window.drop_image(self.render_image.clone()).ok();
        if let Some((_, image)) = &self.pixelation {
            window.drop_image(image.clone()).ok();
        }
        if let Some(image) = &self.blur {
            window.drop_image(image.clone()).ok();
        }
        for asset in &self.text_assets {
            window.drop_image(asset.preview.clone()).ok();
        }
    }
    fn prepare_pending_effect(&mut self, cx: &mut Context<Self>) {
        if self
            .effects_cancel
            .load(std::sync::atomic::Ordering::Relaxed)
        {
            self.pending_effect = None;
            return;
        }
        match self.pending_effect.take() {
            Some(Tool::Blur) if self.tool == Tool::Blur => self.prepare_blur(cx),
            Some(Tool::Pixelate) if self.tool == Tool::Pixelate => self.prepare_pixelation(cx),
            _ => {}
        }
    }
    fn prepare_blur(&mut self, cx: &mut Context<Self>) {
        if self.blur.is_some() || self.blur_loading {
            return;
        }
        if self.pixelation_loading || self.text_loading {
            self.pending_effect = Some(Tool::Blur);
            return;
        }
        self.blur_loading = true;
        self.blur_error = None;
        let source = self.source.pixels.clone();
        let cancelled = self.effects_cancel.clone();
        cx.spawn(async move |this, cx| {
            let result = cx
                .background_spawn(async move {
                    app_core::blur_preview_bgra(&source, || {
                        cancelled.load(std::sync::atomic::Ordering::Relaxed)
                    })
                    .map(|preview| {
                        Arc::new(gpui::RenderImage::new(vec![image::Frame::new(preview)]))
                    })
                    .map_err(|error| error.to_string())
                })
                .await;
            this.update(cx, |this, cx| {
                this.blur_loading = false;
                match result {
                    Ok(preview) => this.blur = Some(preview),
                    Err(error) => this.blur_error = Some(error),
                }
                this.prepare_pending_effect(cx);
                cx.notify();
            })
            .ok();
        })
        .detach();
        cx.notify();
    }
    fn prepare_pixelation(&mut self, cx: &mut Context<Self>) {
        if self.pixelation.is_some() || self.pixelation_loading {
            return;
        }
        if self.blur_loading || self.text_loading {
            self.pending_effect = Some(Tool::Pixelate);
            return;
        }
        self.pixelation_loading = true;
        self.pixelation_error = None;
        let source = self.source.pixels.clone();
        let cancelled = self.effects_cancel.clone();
        cx.spawn(async move |this, cx| {
            let result = cx
                .background_spawn(async move {
                    let is_cancelled = || cancelled.load(std::sync::atomic::Ordering::Relaxed);
                    let grid = app_core::PixelationGrid::prepare(&source, is_cancelled)
                        .map_err(|e| e.to_string())?;
                    let preview = grid.preview_bgra(is_cancelled).map_err(|e| e.to_string())?;
                    Ok::<_, String>((
                        Arc::new(grid),
                        Arc::new(gpui::RenderImage::new(vec![image::Frame::new(preview)])),
                    ))
                })
                .await;
            this.update(cx, |this, cx| {
                this.pixelation_loading = false;
                match result {
                    Ok(prepared) => this.pixelation = Some(prepared),
                    Err(error) => this.pixelation_error = Some(error),
                }
                this.prepare_pending_effect(cx);
                cx.notify();
            })
            .ok();
        })
        .detach();
        cx.notify();
    }
    fn finish_selection_drag(
        &mut self,
        position: [f32; 2],
        scale: f32,
        image: [u32; 2],
        cx: &mut Context<Self>,
    ) {
        if self.tool != Tool::Select {
            return;
        }
        if let (Some(start), Some(selected)) = (self.drag.take(), self.selected) {
            self.edits.transform_selected(
                selected,
                self.resize_handle,
                [
                    (position[0] - start[0]) / scale,
                    (position[1] - start[1]) / scale,
                ],
                image,
            );
            cx.notify();
        }
    }
    fn duplicate_selection(&mut self, cx: &mut Context<Self>) {
        if self.tool != Tool::Select
            || self.drag.is_some()
            || self.text_draft.is_some()
            || self.text_loading
        {
            return;
        }
        if let Some(selected) = self.selected {
            if let Ok(copy) = self
                .edits
                .duplicate(selected, [self.source.width, self.source.height])
            {
                self.selected = Some(copy);
                self.resize_handle = None;
                cx.notify();
            }
        }
    }
    fn delete_selection(&mut self, cx: &mut Context<Self>) {
        if self.text_draft.is_some() || self.text_loading {
            return;
        }
        if self.tool == Tool::Select {
            if let Some(selected) = self.selected.take() {
                self.edits.delete_selected(selected);
                self.drag = None;
                cx.notify();
            }
        }
    }
    fn export(&self, mode: OutputMode, cx: &mut Context<Self>) {
        if self.drag.is_some()
            || self.text_draft.is_some()
            || self.text_loading
            || (self.edits.regions.is_empty()
                && self.edits.frames.is_empty()
                && self.edits.highlights.is_empty()
                && self.edits.pens.is_empty()
                && self.edits.arrows.is_empty()
                && self.edits.ellipses.is_empty()
                && self.edits.pixelations.is_empty()
                && self.edits.blurs.is_empty()
                && self.edits.texts.is_empty()
                && self.edits.crop.is_none()
                && !self.direct_capture)
        {
            return;
        }
        let owner = self.owner.clone();
        let source = self.source.clone();
        let regions = self.edits.regions.clone();
        let crop = self.edits.crop.or_else(|| {
            self.direct_capture.then_some(PixelRegion {
                x: 0,
                y: 0,
                width: self.source.width,
                height: self.source.height,
            })
        });
        let annotations = self.edits.ordered_annotations();
        let pixelations = self.edits.pixelations.clone();
        let blurs = self.edits.blurs.clone();
        let pixelation = self.pixelation.as_ref().map(|(grid, _)| grid.clone());
        cx.defer(move |cx| {
            owner
                .update(cx, |this, cx| {
                    this.export_redaction(
                        source,
                        regions,
                        crop,
                        annotations,
                        pixelations,
                        pixelation,
                        blurs,
                        mode,
                        cx,
                    )
                })
                .ok();
        });
    }
    fn set_zoom(&mut self, zoom: Option<f32>, current: f32, cx: &mut Context<Self>) {
        let old = self.zoom.unwrap_or(current);
        self.pan = if let Some(next) = zoom {
            self.pan.map(|offset| offset * next / old)
        } else {
            [0.0; 2]
        };
        self.zoom = zoom;
        self.drag = None;
        cx.notify();
    }
    pub fn new(
        source: CapturedImage,
        render_image: Arc<gpui::RenderImage>,
        owner: gpui::WeakEntity<SpikeShell>,
        direct_capture: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let focus = cx.focus_handle();
        window.focus(&focus, cx);
        Self {
            source,
            render_image,
            focus,
            owner,
            direct_capture,
            toolbar_bottom: false,
            edits: RectEdits::default(),
            drag: None,
            end: [0.0; 2],
            layout: None,
            tool: if direct_capture {
                Tool::Crop
            } else {
                Tool::Redact
            },
            frame_color: 0xef4444,
            highlight_opacity: 85,
            spotlight_color: 0,
            spotlight_opacity: 191,
            stroke_width: 3,
            shape_filled: false,
            selected: None,
            resize_handle: None,
            zoom: None,
            pan: [0.0; 2],
            pixelation: None,
            effects_cancel: Arc::new(std::sync::atomic::AtomicBool::new(false)),
            pixelation_loading: false,
            pixelation_error: None,
            blur: None,
            blur_loading: false,
            blur_error: None,
            pending_effect: None,
            text_draft: None,
            text_assets: Vec::new(),
            text_size: 24,
            text_bold: false,
            text_italic: false,
            text_loading: false,
            text_error: None,
            text_job: 0,
            text_cancel: None,
            pen_trace: Vec::new(),
            pen_limit_hit: false,
            pen_capacity: app_core::MAX_PEN_POINTS,
        }
    }

    fn cancel(&self, cx: &mut Context<Self>) {
        let owner = self.owner.clone();
        cx.defer(move |cx| {
            owner.update(cx, |this, cx| this.close_editor(cx)).ok();
        });
    }
}

impl Render for RedactionEditor {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let viewport = window.viewport_size();
        let layout = [f32::from(viewport.width), f32::from(viewport.height)];
        if self.layout != Some(layout) {
            self.drag = None;
            self.layout = Some(layout);
        }
        let show_palette = matches!(
            self.tool,
            Tool::Select
                | Tool::Frame
                | Tool::Arrow
                | Tool::Ellipse
                | Tool::Text
                | Tool::Number
                | Tool::Highlight
                | Tool::Spotlight
                | Tool::Pen
                | Tool::Line
        );
        let spotlight_style = self.tool == Tool::Spotlight
            || (self.tool == Tool::Select
                && self.selected.is_some_and(|selected| match selected {
                    Selected::Highlight(i) => {
                        self.edits.highlights.get(i).is_some_and(|h| h.outside)
                    }
                    _ => false,
                }));
        let palette_color = if self.tool == Tool::Select {
            self.selected.and_then(|mark| self.edits.color(mark))
        } else if self.tool == Tool::Spotlight {
            Some(self.spotlight_color)
        } else {
            Some(self.frame_color)
        };
        let palette_filled = if matches!(self.tool, Tool::Frame | Tool::Ellipse) {
            Some(self.shape_filled)
        } else if self.tool == Tool::Select {
            self.selected
                .and_then(|selected| self.edits.filled(selected))
        } else {
            None
        };
        let draws_stroke = matches!(
            self.tool,
            Tool::Pen | Tool::Line | Tool::Frame | Tool::Ellipse | Tool::Arrow
        );
        let palette_stroke_width = if draws_stroke {
            Some(self.stroke_width)
        } else {
            self.selected
                .and_then(|selected| self.edits.stroke_width(selected))
        };
        let show_stroke_width =
            draws_stroke || (self.tool == Tool::Select && palette_stroke_width.is_some());
        let show_opacity = matches!(self.tool, Tool::Highlight | Tool::Spotlight | Tool::Select)
            && !show_stroke_width;
        let palette_opacity = if self.tool == Tool::Highlight {
            Some(self.highlight_opacity)
        } else if self.tool == Tool::Spotlight {
            Some(self.spotlight_opacity)
        } else {
            self.selected.and_then(|selected| match selected {
                Selected::Highlight(index) => self.edits.highlights.get(index).map(|h| h.opacity),
                _ => None,
            })
        };
        let duplicate_error = if self.tool == Tool::Select {
            self.selected
                .and_then(|selected| self.edits.duplicate_target(selected).err())
        } else {
            None
        };
        let toolbar_height = if show_palette { 220.0 } else { 172.0 }
            + if layout[0] < 900.0 {
                88.0
            } else if layout[0] < 1400.0 {
                44.0
            } else {
                0.0
            }
            + if self.tool == Tool::Number {
                88.0
            } else if self.tool == Tool::Text {
                44.0
            } else {
                0.0
            }
            + self.text_draft.as_ref().map_or(0.0, |draft| {
                80.0 + 24.0 * (draft.input.read(cx).line_count() - 1) as f32
            })
            + if self.text_error.is_some() { 24.0 } else { 0.0 }
            + if duplicate_error.is_some() { 24.0 } else { 0.0 };
        let inset = if self.direct_capture { 0.0 } else { 16.0 };
        let workspace_top = if self.direct_capture {
            0.0
        } else {
            toolbar_height
        };
        let available = [
            f32::from(viewport.width) - inset * 2.0,
            f32::from(viewport.height) - workspace_top - inset,
        ];
        let dimensions = self.source.pixels.dimensions();
        let fit_scale = (available[0].max(1.0) / dimensions.0 as f32)
            .min(available[1].max(1.0) / dimensions.1 as f32);
        let scale = self.zoom.unwrap_or(fit_scale);
        let size = [dimensions.0 as f32 * scale, dimensions.1 as f32 * scale];
        self.pan = clamp_pan(self.pan, size, available);
        let dpi = window.scale_factor();
        let minimum = fit_scale.min(0.05 / dpi);
        let maximum = 8.0 / dpi;
        let origin = [
            (f32::from(viewport.width) - size[0]) / 2.0 + self.pan[0],
            workspace_top + (available[1].max(1.0) - size[1]) / 2.0 + self.pan[1],
        ];
        let mut displayed = self.edits.snapshot();
        if self.tool == Tool::Select {
            if let (Some(start), Some(selected)) = (self.drag, self.selected) {
                displayed.transform(
                    selected,
                    self.resize_handle,
                    [
                        (self.end[0] - start[0]) / scale,
                        (self.end[1] - start[1]) / scale,
                    ],
                    [dimensions.0, dimensions.1],
                );
            }
        }
        let selected_bounds = if self.tool == Tool::Select {
            self.selected
                .and_then(|selected| displayed.bounds(selected))
        } else {
            None
        };
        let resize_handles = if self.tool == Tool::Select {
            self.selected
                .map_or_else(Vec::new, |selected| displayed.handles(selected))
        } else {
            Vec::new()
        };
        let mut preview = displayed.regions;
        let mut paint_order = displayed.order;
        let mut frame_preview = displayed.frames;
        let mut highlight_preview = displayed.highlights;
        let mut pen_preview = displayed.pens;
        let mut arrow_preview = displayed.arrows;
        let mut ellipse_preview = displayed.ellipses;
        let mut pixelation_preview = displayed.pixelations;
        let pixelation_render = self.pixelation.as_ref().map(|(_, render)| render.clone());
        let mut blur_preview = displayed.blurs;
        let text_preview = displayed.texts;
        let blur_render = self.blur.clone();
        let mut crop_preview = self.edits.crop;
        if let Some(start) = self.drag {
            if self.tool == Tool::Select {
                // Transforms are previewed in metadata above and committed on release.
            } else if self.tool == Tool::Pen {
                if let Some(mut pen) =
                    app_core::PenAnnotation::from_trace(&self.pen_trace, self.frame_color)
                {
                    pen.width = self.stroke_width;
                    paint_order.push(Selected::Pen(pen_preview.len()));
                    pen_preview.push(pen);
                }
            } else if self.tool == Tool::Line {
                if let Some(line) = line_from_drag(
                    start,
                    self.end,
                    size,
                    [dimensions.0, dimensions.1],
                    self.frame_color,
                    self.stroke_width,
                    window.modifiers().shift,
                ) {
                    paint_order.push(Selected::Pen(pen_preview.len()));
                    pen_preview.push(line);
                }
            } else if self.tool == Tool::Arrow {
                if let Some(mut arrow) = ArrowAnnotation::from_drag_constrained(
                    start,
                    self.end,
                    size,
                    [dimensions.0, dimensions.1],
                    self.frame_color,
                    window.modifiers().shift,
                ) {
                    arrow.width = self.stroke_width;
                    paint_order.push(Selected::Arrow(arrow_preview.len()));
                    arrow_preview.push(arrow);
                }
            } else if let Some(region) = PixelRegion::from_drag_constrained(
                start,
                self.end,
                size,
                [dimensions.0, dimensions.1],
                window.modifiers().shift,
            ) {
                if self.tool == Tool::Crop {
                    crop_preview = Some(region);
                } else if self.tool == Tool::Pixelate {
                    if self.pixelation.is_some() {
                        pixelation_preview.push(region);
                    }
                } else if self.tool == Tool::Blur {
                    if self.blur.is_some() {
                        blur_preview.push(region);
                    }
                } else if self.tool == Tool::Frame {
                    paint_order.push(Selected::Frame(frame_preview.len()));
                    frame_preview.push(FrameAnnotation {
                        filled: self.shape_filled,
                        width: self.stroke_width,
                        area: region,
                        color: self.frame_color,
                    });
                } else if matches!(self.tool, Tool::Highlight | Tool::Spotlight) {
                    paint_order.push(Selected::Highlight(highlight_preview.len()));
                    highlight_preview.push(HighlightAnnotation {
                        outside: self.tool == Tool::Spotlight,
                        opacity: if self.tool == Tool::Spotlight {
                            self.spotlight_opacity
                        } else {
                            self.highlight_opacity
                        },
                        area: region,
                        color: if self.tool == Tool::Spotlight {
                            self.spotlight_color
                        } else {
                            self.frame_color
                        },
                    });
                } else {
                    if self.tool == Tool::Ellipse {
                        paint_order.push(Selected::Ellipse(ellipse_preview.len()));
                        ellipse_preview.push(FrameAnnotation {
                            filled: self.shape_filled,
                            width: self.stroke_width,
                            area: region,
                            color: self.frame_color,
                        });
                    } else {
                        preview.push(region);
                    }
                }
            }
        }
        div()
            .relative()
            .size_full()
            .bg(rgb(0x111318))
            .text_color(rgb(0xf1f5f9))
            .track_focus(&self.focus)
            .key_context("RedactionEditor")
            .on_key_down(cx.listener(|this, event: &gpui::KeyDownEvent, _, cx| {
                if this.text_draft.is_some() || this.text_loading {return;}
                let modifiers = event.keystroke.modifiers;
                if modifiers.control || modifiers.alt || modifiers.platform || modifiers.function || event.prefer_character_input { return; }
                let key = event.keystroke.key.as_str();
                if !modifiers.shift {
                    if let Some(tool) = Tool::shortcut(key) {
                        this.tool = tool;
                        this.pen_trace.clear();this.pen_limit_hit=false;
                        if tool == Tool::Pixelate { this.prepare_pixelation(cx); }
                        if tool == Tool::Blur { this.prepare_blur(cx); }
                        this.drag = None;
                        this.resize_handle = None;
                        cx.stop_propagation(); cx.notify(); return;
                    }
                }
                if this.tool == Tool::Select && this.drag.is_none() {
                    if let (Some(selected),Some(delta)) = (this.selected,nudge_delta(key,modifiers.shift)) {
                        this.edits.transform_selected(selected,None,delta,[this.source.width,this.source.height]);
                        cx.stop_propagation(); cx.notify();
                    }
                }
            }))
            .on_action(cx.listener(|this, _: &CloseOverlay, window, cx| {if this.text_draft.is_some(){this.cancel_text(window,cx);}else if this.tool==Tool::Pen && this.drag.is_some(){this.cancel_pen(cx);}else{this.cancel(cx);}}))
            .on_action(cx.listener(|this, _: &DeleteAnnotation, _, cx| this.delete_selection(cx)))
            .on_action(cx.listener(|this, _: &DuplicateAnnotation, _, cx| this.duplicate_selection(cx)))
            .on_modifiers_changed(cx.listener(|this, _, _, cx| {
                if this.drag.is_some() && !matches!(this.tool, Tool::Select | Tool::Pen) {
                    cx.notify();
                }
            }))
            .on_action(cx.listener(|this, _: &UndoRedaction, _, cx| {
                if this.text_draft.is_some() || this.text_loading {return;}
                this.edits.undo();
                this.selected = None;
                this.drag = None;
                cx.notify();
            }))
            .on_action(cx.listener(|this, _: &RedoRedaction, _, cx| {
                if this.text_draft.is_some() || this.text_loading {return;}
                this.edits.redo();
                this.selected = None;
                this.drag = None;
                cx.notify();
            }))
            .on_mouse_move(cx.listener(|this, event: &gpui::MouseMoveEvent, _, cx| {
                if this.drag.is_some() && !event.dragging() {
                    this.drag = None;
                    this.pen_trace.clear();
                    this.pen_limit_hit=false;
                    cx.notify();
                }
            }))
            .on_mouse_move(cx.listener(move |this, event: &gpui::MouseMoveEvent, _, cx| {
                if this.tool == Tool::Select && this.drag.is_some() && event.dragging() {
                    this.end = [f32::from(event.position.x)-origin[0],f32::from(event.position.y)-origin[1]];
                    cx.notify();
                }
                if this.tool==Tool::Pen && this.drag.is_some() && event.dragging(){
                    let position=[f32::from(event.position.x)-origin[0],f32::from(event.position.y)-origin[1]];
                    if this.sample_pen(pen_point(position,scale,[dimensions.0,dimensions.1]),false){cx.notify();}
                }
            }))
            .on_mouse_up(gpui::MouseButton::Left, cx.listener(move |this, event: &gpui::MouseUpEvent, _, cx| {
                this.finish_pen([f32::from(event.position.x)-origin[0],f32::from(event.position.y)-origin[1]],scale,[dimensions.0,dimensions.1],cx);
                this.finish_selection_drag([f32::from(event.position.x)-origin[0],f32::from(event.position.y)-origin[1]],scale,[dimensions.0,dimensions.1],cx);
            }))
            .on_mouse_up_out(gpui::MouseButton::Left,cx.listener(move|this,event:&gpui::MouseUpEvent,_,cx|this.finish_pen([f32::from(event.position.x)-origin[0],f32::from(event.position.y)-origin[1]],scale,[dimensions.0,dimensions.1],cx)))
            .child(gpui::deferred(
                div()
                    .id("editor-toolbar")
                    .absolute()
                    .when(!self.toolbar_bottom, |view| view.top_0())
                    .when(self.toolbar_bottom, |view| view.bottom_0())
                    .bg(rgb(0x111318))
                    .occlude()
                    .max_h_full()
                    .overflow_y_scroll()
                    .left_0()
                    .right_0()
                    .p_3()
                    .flex()
                    .flex_col()
                    .gap_2()
                    .child(if self.direct_capture {
                        "New capture · No original PNG saved · Choose Copy, Save, or both"
                    } else {
                        "Edit a copy · Existing original files and history entries are kept"
                    })
                    .child(div().text_sm().child(format!(
                        "{} · {} / 100 annotations · Export: {} × {}",
                        match self.tool {
                            Tool::Spotlight=>"Drag a focus area; hold Shift for a square. Use Redact for sensitive content.",
                            Tool::Number=>if self.text_loading {"Preparing number..."}else{"Click to place the next step number; Select to move or edit it"},
                            Tool::Pen=>if self.pen_limit_hit {"Pen limit reached. Release or remove marks to continue."}else{"Draw freehand at the chosen width; release to finish"},
                            Tool::Highlight=>"Drag a highlight; hold Shift for a square",
                            Tool::Text => if self.text_loading {"Preparing text..."} else {"Up to 4 lines / 512 bytes; Shift+Enter adds a line, Enter applies, Escape cancels"},
                            Tool::Pixelate => if self.pixelation_loading || self.pending_effect==Some(Tool::Pixelate) { "Preparing pixelation preview..." } else if self.pixelation_error.is_some() { "Pixelation failed. Select Pixelate to retry." } else { "Drag to pixelate source pixels; use Redact for sensitive information" },
                            Tool::Blur => if self.blur_loading || self.pending_effect==Some(Tool::Blur) { "Preparing blur preview..." } else if self.blur_error.is_some() { "Blur failed. Select Blur to retry." } else { "Drag to blur source pixels; use Redact for sensitive information" },
                            Tool::Select =>
                                "Drag a mark to move; handles resize; arrow keys move 1px (Shift: 10px); Delete removes it",
                            Tool::Crop => "Drag to crop; hold Shift for a square",
                            Tool::Frame => "Drag a frame; hold Shift for a square",
                            Tool::Arrow => "Drag tail to tip; hold Shift to snap to 45 degrees",
                            Tool::Line => if self.pen_limit_hit {"Line limit reached. Remove marks to continue."}else{"Drag a straight line; hold Shift to snap to 45 degrees"},
                            Tool::Ellipse => "Drag an ellipse; hold Shift for a circle",
                            Tool::Redact => "Drag to redact; hold Shift for a square",
                        },
                        self.edits.regions.len()
                            + self.edits.frames.len() + self.edits.highlights.len() + self.edits.pens.len()
                            + self.edits.arrows.len()
                            + self.edits.ellipses.len() + self.edits.pixelations.len() + self.edits.blurs.len() + self.edits.texts.len(),
                        crop_preview.map_or(dimensions.0, |area| area.width),
                        crop_preview.map_or(dimensions.1, |area| area.height)
                    )))
                    .child(
                        div()
                            .flex()
                            .flex_wrap()
                            .gap_2()
                            .child(capture_button("tool-pen",if self.tool==Tool::Pen {"Pen (D) active"}else{"Pen (D)"},false,cx.listener(|this,_,_,cx|{this.tool=Tool::Pen;this.drag=None;this.pen_trace.clear();this.pen_limit_hit=false;cx.notify();})))
                            .child(capture_button("tool-line",if self.tool==Tool::Line {"Line (L) active"}else{"Line (L)"},false,cx.listener(|this,_,_,cx|{this.tool=Tool::Line;this.drag=None;this.pen_trace.clear();this.pen_limit_hit=false;cx.notify();})))
                            .child(capture_button("tool-highlight",if self.tool==Tool::Highlight {"Highlight (H) active"}else{"Highlight (H)"},false,cx.listener(|this,_,_,cx|{this.tool=Tool::Highlight;this.drag=None;cx.notify();})))
                            .child(capture_button("tool-spotlight",if self.tool==Tool::Spotlight {"Spotlight (S) active"}else{"Spotlight (S)"},false,cx.listener(|this,_,_,cx|{this.tool=Tool::Spotlight;this.drag=None;cx.notify();})))
                            .child(capture_button("layer-forward","Bring forward",self.tool!=Tool::Select||self.drag.is_some()||self.text_draft.is_some()||self.text_loading||self.selected.and_then(|selected|self.edits.layer_target(selected,true)).is_none(),cx.listener(|this,_,_,cx|this.move_layer(true,cx))))
                            .child(capture_button("layer-backward","Send backward",self.tool!=Tool::Select||self.drag.is_some()||self.text_draft.is_some()||self.text_loading||self.selected.and_then(|selected|self.edits.layer_target(selected,false)).is_none(),cx.listener(|this,_,_,cx|this.move_layer(false,cx))))
                            .child(capture_button("tool-text",if self.tool==Tool::Text {"Text (T) active"}else{"Text (T)"},false,cx.listener(|this,_,_,cx|{this.tool=Tool::Text;this.drag=None;cx.notify();})))
                            .child(capture_button("tool-number",if self.tool==Tool::Number {"Number (N) active"}else{"Number (N)"},false,cx.listener(|this,_,_,cx|{this.tool=Tool::Number;this.drag=None;cx.notify();})))
                            .child(capture_button("edit-text","Edit text",!matches!(self.selected,Some(Selected::Text(_)))||self.text_draft.is_some()||self.text_loading,cx.listener(|this,_,window,cx|{if let Some(Selected::Text(index))=this.selected {if let Some(text)=this.edits.texts.get(index){this.begin_text([text.area.x,text.area.y],Some(index),window,cx);}}})))
                            .child(capture_button(
                                "tool-blur", if self.tool == Tool::Blur { "Blur (B) active" } else { "Blur (B)" },false,
                                cx.listener(|this,_,_,cx| { this.tool=Tool::Blur; this.drag=None; this.prepare_blur(cx); cx.notify(); }),
                            ))
                            .child(capture_button(
                                "tool-pixelate", if self.tool == Tool::Pixelate { "Pixelate (P) active" } else { "Pixelate (P)" }, false,
                                cx.listener(|this,_,_,cx| { this.tool=Tool::Pixelate; this.drag=None; this.prepare_pixelation(cx); cx.notify(); }),
                            ))
                            .child(capture_button(
                                "tool-select",
                                if self.tool == Tool::Select {
                                    "Select (V) (active)"
                                } else {
                                    "Select (V)"
                                },
                                false,
                                cx.listener(|this, _, _, cx| {
                                    this.tool = Tool::Select;
                                    this.drag = None;
                                    cx.notify();
                                }),
                            ))
                            .child(capture_button(
                                "delete-annotation",
                                "Delete",
                                self.tool != Tool::Select || self.selected.is_none(),
                                cx.listener(|this, _, _, cx| this.delete_selection(cx)),
                            ))
                            .child(capture_button(
                                "duplicate-annotation",
                                "Duplicate",
                                self.tool != Tool::Select || self.selected.is_none() || self.drag.is_some() || self.text_draft.is_some() || self.text_loading || duplicate_error.is_some(),
                                cx.listener(|this, _, _, cx| this.duplicate_selection(cx)),
                            ))
                            .child(capture_button(
                                "tool-redact",
                                if self.tool == Tool::Redact {
                                    "Redact (X) ✓"
                                } else {
                                    "Redact (X)"
                                },
                                false,
                                cx.listener(|this, _, _, cx| {
                                    this.tool = Tool::Redact;
                                    this.drag = None;
                                    cx.notify();
                                }),
                            ))
                            .child(capture_button(
                                "tool-frame",
                                if self.tool == Tool::Frame {
                                    "Frame (R) ✓"
                                } else {
                                    "Frame (R)"
                                },
                                false,
                                cx.listener(|this, _, _, cx| {
                                    this.tool = Tool::Frame;
                                    this.drag = None;
                                    cx.notify();
                                }),
                            ))
                            .child(capture_button(
                                "tool-arrow",
                                if self.tool == Tool::Arrow {
                                    "Arrow (A) ✓"
                                } else {
                                    "Arrow (A)"
                                },
                                false,
                                cx.listener(|this, _, _, cx| {
                                    this.tool = Tool::Arrow;
                                    this.drag = None;
                                    cx.notify();
                                }),
                            ))
                            .child(capture_button(
                                "tool-ellipse",
                                if self.tool == Tool::Ellipse {
                                    "Ellipse (E) ✓"
                                } else {
                                    "Ellipse (E)"
                                },
                                false,
                                cx.listener(|this, _, _, cx| {
                                    this.tool = Tool::Ellipse;
                                    this.drag = None;
                                    cx.notify();
                                }),
                            ))
                            .child(capture_button(
                                "tool-crop",
                                if self.tool == Tool::Crop {
                                    "Crop (C) ✓"
                                } else {
                                    "Crop (C)"
                                },
                                false,
                                cx.listener(|this, _, _, cx| {
                                    this.tool = Tool::Crop;
                                    this.drag = None;
                                    cx.notify();
                                }),
                            ))
                            .child(capture_button(
                                "crop-full-image",
                                "Full image",
                                self.edits.crop.is_none(),
                                cx.listener(|this, _, _, cx| {
                                    this.edits.set_crop(None);
                                    this.drag = None;
                                    cx.notify();
                                }),
                            ))
                            .child(capture_button(
                                "redaction-undo",
                                "Undo",
                                self.edits.past.is_empty() || self.text_draft.is_some() || self.text_loading,
                                cx.listener(|this, _, _, cx| {
                                    this.edits.undo();
                                    this.selected = None;
                                    this.drag = None;
                                    cx.notify();
                                }),
                            ))
                            .child(capture_button(
                                "redaction-redo",
                                "Redo",
                                self.edits.undone.is_empty() || self.text_draft.is_some() || self.text_loading,
                                cx.listener(|this, _, _, cx| {
                                    this.edits.redo();
                                    this.selected = None;
                                    this.drag = None;
                                    cx.notify();
                                }),
                            ))
                            .child(capture_button(
                                "redaction-reset",
                                "Reset",
                                self.text_draft.is_some() || self.text_loading || (self.edits.regions.is_empty()
                                    && self.edits.frames.is_empty() && self.edits.highlights.is_empty() && self.edits.pens.is_empty()
                                    && self.edits.arrows.is_empty()
                                    && self.edits.ellipses.is_empty() && self.edits.pixelations.is_empty() && self.edits.blurs.is_empty() && self.edits.texts.is_empty()
                                    && self.edits.crop.is_none()
                                    && self.edits.undone.is_empty()
                                    && self.edits.past.is_empty()),
                                cx.listener(|this, _, window, cx| {
                                    this.edits.reset();
                                    this.pen_trace.clear();this.pen_limit_hit=false;
                                    for asset in this.text_assets.drain(..){window.drop_image(asset.preview.clone()).ok();}
                                    this.selected = None;
                                    this.drag = None;
                                    cx.notify();
                                }),
                            ))
                            .children(
                                [
                                    ("export-copy-only", "Copy only", OutputMode::CopyOnly),
                                    ("export-save-only", "Save only", OutputMode::SaveOnly),
                                    ("export-save-copy", "Save and copy", OutputMode::SaveAndCopy),
                                ]
                                .into_iter()
                                .map(|(id, label, mode)| {
                                    capture_button(
                                        id,
                                        label,
                                        (self.edits.regions.is_empty()
                                            && self.edits.frames.is_empty() && self.edits.highlights.is_empty() && self.edits.pens.is_empty()
                                            && self.edits.arrows.is_empty()
                                            && self.edits.ellipses.is_empty() && self.edits.pixelations.is_empty() && self.edits.blurs.is_empty() && self.edits.texts.is_empty()
                                            && self.edits.crop.is_none()
                                            && !self.direct_capture)
                                            || self.drag.is_some() || self.text_draft.is_some() || self.text_loading,
                                        cx.listener(move |this, _, _, cx| this.export(mode, cx)),
                                    )
                                }),
                            )
                            .child(capture_button(
                                "redaction-cancel",
                                "Cancel",
                                false,
                                cx.listener(|this, _, _, cx| this.cancel(cx)),
                            )),
                    )
                    .child(
                        div()
                            .flex()
                            .gap_2()
                            .items_center()
                            .child(capture_button(
                                "zoom-out",
                                "−",
                                scale <= minimum,
                                cx.listener(move |this, _, _, cx| {
                                    this.set_zoom(
                                        Some(
                                            (this.zoom.unwrap_or(scale) / 1.25)
                                                .clamp(minimum, maximum),
                                        ),
                                        scale,
                                        cx,
                                    )
                                }),
                            ))
                            .child(capture_button(
                                "zoom-in",
                                "+",
                                scale >= maximum,
                                cx.listener(move |this, _, _, cx| {
                                    this.set_zoom(
                                        Some(
                                            (this.zoom.unwrap_or(scale) * 1.25)
                                                .clamp(minimum, maximum),
                                        ),
                                        scale,
                                        cx,
                                    )
                                }),
                            ))
                            .child(capture_button(
                                "zoom-actual",
                                "100%",
                                false,
                                cx.listener(move |this, _, _, cx| {
                                    this.set_zoom(Some(1.0 / dpi), scale, cx)
                                }),
                            ))
                            .child(capture_button(
                                "zoom-fit",
                                "Fit",
                                false,
                                cx.listener(move |this, _, _, cx| this.set_zoom(None, scale, cx)),
                            ))
                            .child(div().text_sm().child(format!(
                                "{:.0}% · Wheel to pan · Shift+wheel horizontally",
                                scale * dpi * 100.0
                            ))),
                    )
                    .when(self.direct_capture, |view| {
                        view.child(capture_button(
                            "move-overlay-toolbar",
                            "Move toolbar",
                            false,
                            cx.listener(|this, _, _, cx| {
                                this.toolbar_bottom = !this.toolbar_bottom;
                                this.drag = None;
                                cx.notify();
                            }),
                        ))
                    })
                    .children(self.text_error.clone().map(|error|div().text_sm().child(error)))
                    .children(duplicate_error.map(|error|div().text_sm().child(error)))
                    .children(self.text_draft.as_ref().map(|draft|draft.input.clone()))
                    .when(self.text_draft.is_some(),|view|view.child(div().flex().gap_2()
                        .child(capture_button("apply-text","Apply text",self.text_loading||self.pixelation_loading||self.blur_loading,cx.listener(|this,_,window,cx|this.commit_text(window,cx))))
                        .child(capture_button("cancel-text","Cancel text",false,cx.listener(|this,_,window,cx|this.cancel_text(window,cx))))))
                    .when(self.tool==Tool::Number,|view|view.child(div().flex().gap_2().child(format!("Next: {}",self.edits.last_number.checked_add(1).map(|n|n.to_string()).unwrap_or_else(||"limit reached".into()))).child(capture_button("restart-numbers","Restart numbering",self.edits.last_number==0||self.text_loading||self.text_draft.is_some(),cx.listener(|this,_,_,cx|{this.edits.restart_numbers();this.text_error=None;cx.notify();})))))
                    .when(matches!(self.tool,Tool::Text|Tool::Number),|view|view.child(div().flex().flex_wrap().gap_2().child(capture_button("text-bold",if self.text_bold {"Bold active"}else{"Bold"},self.text_loading,cx.listener(|this,_,_,cx|{this.text_bold=!this.text_bold;cx.notify();}))).child(capture_button("text-italic",if self.text_italic {"Italic active"}else{"Italic"},self.text_loading,cx.listener(|this,_,_,cx|{this.text_italic=!this.text_italic;cx.notify();}))).children([16,24,36].into_iter().map(|size|capture_button(match size{16=>"text-size-16",24=>"text-size-24",_=>"text-size-36"},match size{16=>"16 px",24=>"24 px",_=>"36 px"},self.text_loading||self.text_size==size,cx.listener(move|this,_,_,cx|{this.text_size=size;cx.notify();}))))))
                    .when(
                        show_palette,
                        |toolbar| {
                            toolbar.child(
                                div().flex().flex_wrap().gap_2().children(
                                    [
                                        ("frame-red", "Red", "Red ✓", 0xef4444),
                                        ("frame-yellow", "Yellow", "Yellow ✓", 0xfacc15),
                                        ("frame-green", "Green", "Green ✓", 0x22c55e),
                                        ("frame-black", "Black", "Black ✓", 0),
                                    ]
                                    .into_iter()
                                    .map(
                                        |(id, label, selected, color)| {
                                            capture_button(
                                                id,
                                                if palette_color == Some(color) {
                                                    selected
                                                } else {
                                                    label
                                                },
                                                palette_color.is_none() || self.text_loading,
                                                cx.listener(move |this, _, _, cx| {
                                                    if spotlight_style { this.spotlight_color = color; } else { this.frame_color = color; }
                                                    if this.tool == Tool::Select {
                                                        if let Some(selected) = this.selected {
                                                            this.edits.set_color(selected,color);
                                                        }
                                                    }
                                                    this.drag = None;
                                                    cx.notify();
                                                }),
                                            )
                                        },
                                    ),
                                ).when(show_opacity, |row| row
                                    .child(div().child("Opacity"))
                                    .children([
                                        ("highlight-25", "25%", "25% ✓", 64),
                                        ("highlight-33", "33%", "33% ✓", 85),
                                        ("highlight-50", "50%", "50% ✓", 128),
                                        ("highlight-75", "75%", "75% ✓", 191),
                                        ("highlight-100", "100%", "100% ✓", 255),
                                    ].into_iter().map(|(id,label,active,opacity)| capture_button(
                                        id,
                                        if palette_opacity == Some(opacity) { active } else { label },
                                        palette_opacity.is_none() || self.text_loading || self.text_draft.is_some() || self.drag.is_some(),
                                        cx.listener(move |this,_,_,cx| {
                                            if spotlight_style { this.spotlight_opacity = opacity; } else { this.highlight_opacity = opacity; }
                                            if this.tool == Tool::Select {
                                                if let Some(selected) = this.selected {
                                                    this.edits.set_highlight_opacity(selected, opacity);
                                                }
                                            }
                                            cx.notify();
                                        }),
                                    )))).when(show_stroke_width, |row| row
                                    .child(div().child("Width"))
                                    .children([
                                        ("stroke-width-1", "1 px", "1 px ✓", 1),
                                        ("stroke-width-3", "3 px", "3 px ✓", 3),
                                        ("stroke-width-6", "6 px", "6 px ✓", 6),
                                        ("stroke-width-12", "12 px", "12 px ✓", 12),
                                    ].into_iter().map(|(id,label,active,width)| capture_button(
                                        id,
                                        if palette_stroke_width == Some(width) { active } else { label },
                                        palette_stroke_width.is_none() || palette_filled == Some(true) || self.text_loading || self.text_draft.is_some() || self.drag.is_some(),
                                        cx.listener(move |this,_,_,cx| {
                                            this.stroke_width = width;
                                            if this.tool == Tool::Select {
                                                if let Some(selected) = this.selected {
                                                    this.edits.set_stroke_width(selected, width);
                                                }
                                            }
                                            cx.notify();
                                        }),
                                    )))).when(palette_filled.is_some(), |row| row
                                    .children([
                                        ("shape-outline", "Outline", "Outline ✓", false),
                                        ("shape-filled", "Filled", "Filled ✓", true),
                                    ].into_iter().map(|(id,label,active,filled)| capture_button(
                                        id,
                                        if palette_filled == Some(filled) { active } else { label },
                                        self.text_loading || self.text_draft.is_some() || self.drag.is_some(),
                                        cx.listener(move |this,_,_,cx| {
                                            this.shape_filled = filled;
                                            if this.tool == Tool::Select {
                                                if let Some(selected) = this.selected { this.edits.set_filled(selected, filled); }
                                            }
                                            cx.notify();
                                        }),
                                    )))),
                            )
                        },
                    ),
            ))
            .child(
                div()
                    .absolute()
                    .left(px(inset))
                    .right(px(inset))
                    .top(px(workspace_top))
                    .bottom(px(inset))
                    .overflow_hidden()
                    .on_scroll(cx.listener(|this, event: &gpui::ScrollEvent, _, cx| {
                        if this.drag.is_none() {
                            let delta = event.delta.pixel_delta(px(24.0));
                            if event.modifiers.shift {
                                this.pan[0] += f32::from(delta.y) + f32::from(delta.x);
                            } else {
                                this.pan[0] += f32::from(delta.x);
                                this.pan[1] += f32::from(delta.y);
                            }
                            cx.notify();
                        }
                    }))
                    .child(
                        div()
                            .absolute()
                            .left(px(origin[0] - inset))
                            .top(px(origin[1] - workspace_top))
                            .w(px(size[0]))
                            .h(px(size[1]))
                            .overflow_hidden()
                            .on_mouse_down(
                                gpui::MouseButton::Left,
                                cx.listener(move |this, event: &gpui::MouseDownEvent, window, cx| {
                                    if this.text_draft.is_some() || this.text_loading {return;}
                                    if this.tool==Tool::Pen {this.begin_pen([f32::from(event.position.x)-origin[0],f32::from(event.position.y)-origin[1]],scale,[dimensions.0,dimensions.1],cx);return;}
                                    if this.tool==Tool::Line {this.pen_limit_hit=false;}
                                    if this.tool==Tool::Number {let position=[((f32::from(event.position.x)-origin[0])/scale).floor().max(0.0) as u32,((f32::from(event.position.y)-origin[1])/scale).floor().max(0.0) as u32];this.begin_number(position,window,cx);return;}
                                    if this.tool==Tool::Text {let position=[((f32::from(event.position.x)-origin[0])/scale).floor().max(0.0) as u32,((f32::from(event.position.y)-origin[1])/scale).floor().max(0.0) as u32];this.begin_text(position,None,window,cx);return;}
                                    if this.tool == Tool::Pixelate && this.pixelation.is_none() { return; }
                                    if this.tool == Tool::Blur && this.blur.is_none() { return; }
                                    if this.tool == Tool::Select {
                                        let position = [
                                            f32::from(event.position.x) - origin[0],
                                            f32::from(event.position.y) - origin[1],
                                        ];
                                        this.resize_handle = this.selected.and_then(|selected| this.edits.handle_at(selected, position.map(|v| v / scale), 7.0 / scale));
                                        if this.resize_handle.is_none() {
                                            this.selected = this.edits.pick(position.map(|v| v / scale), 5.0 / scale);
                                        }
                                        this.drag = this.selected.map(|_| position);
                                        this.end = position;
                                        cx.notify();
                                        return;
                                    }
                                    if this.tool == Tool::Crop
                                        || this.edits.regions.len()
                                            + this.edits.frames.len() + this.edits.highlights.len() + this.edits.pens.len()
                                            + this.edits.arrows.len()
                                            + this.edits.ellipses.len() + this.edits.pixelations.len() + this.edits.blurs.len() + this.edits.texts.len()
                                            < 100
                                    {
                                        let position = [
                                            f32::from(event.position.x) - origin[0],
                                            f32::from(event.position.y) - origin[1],
                                        ];
                                        this.drag = Some(position);
                                        this.end = position;
                                        cx.notify();
                                    }
                                }),
                            )
                            .on_mouse_move(cx.listener(
                                move |this, event: &gpui::MouseMoveEvent, _, cx| {
                                    if this.tool != Tool::Select && this.tool!=Tool::Pen && this.drag.is_some() {
                                        if event.dragging() {
                                            this.end = [
                                                f32::from(event.position.x) - origin[0],
                                                f32::from(event.position.y) - origin[1],
                                            ];
                                        } else {
                                            this.drag = None;
                                        }
                                        cx.notify();
                                    }
                                },
                            ))
                            .on_mouse_up(
                                gpui::MouseButton::Left,
                                cx.listener(move |this, event: &gpui::MouseUpEvent, _, cx| {
                                    if this.tool == Tool::Select || this.tool==Tool::Pen { return; }
                                    if let Some(start) = this.drag.take() {
                                        let end = [
                                            f32::from(event.position.x) - origin[0],
                                            f32::from(event.position.y) - origin[1],
                                        ];
                                        if this.tool == Tool::Line {
                                            if let Some(line)=line_from_drag(start,end,size,[dimensions.0,dimensions.1],this.frame_color,this.stroke_width,event.modifiers.shift) {this.pen_limit_hit=!this.edits.add_pen(line);}
                                        } else if this.tool == Tool::Arrow {
                                            if let Some(mut arrow) = ArrowAnnotation::from_drag_constrained(
                                                start,
                                                end,
                                                size,
                                                [dimensions.0, dimensions.1],
                                                this.frame_color,
                                                event.modifiers.shift,
                                            ) {
                                                arrow.width = this.stroke_width;
                                                this.edits.add_arrow(arrow);
                                            }
                                        } else if let Some(region) = PixelRegion::from_drag_constrained(
                                            start,
                                            end,
                                            size,
                                            [dimensions.0, dimensions.1],
                                            event.modifiers.shift,
                                        ) {
                                            if this.tool == Tool::Crop {
                                                this.edits.set_crop(Some(region));
                                            } else if this.tool == Tool::Pixelate {
                                                if this.pixelation.is_some() { this.edits.add_pixelation(region); }
                                            } else if this.tool == Tool::Blur {
                                                if this.blur.is_some() { this.edits.add_blur(region); }
                                            } else if this.tool == Tool::Frame {
                                                this.edits.add_frame(FrameAnnotation {
    filled: this.shape_filled,
    width: this.stroke_width,
                                                    area: region,
                                                    color: this.frame_color,
                                                });
                                            } else if matches!(this.tool, Tool::Highlight | Tool::Spotlight) {
                                                this.edits.add_highlight(HighlightAnnotation {
                                                    outside: this.tool == Tool::Spotlight,
                                                    area: region,
                                                    color: if this.tool == Tool::Spotlight { this.spotlight_color } else { this.frame_color },
                                                    opacity: if this.tool == Tool::Spotlight { this.spotlight_opacity } else { this.highlight_opacity },
                                                });
                                            } else {
                                                if this.tool == Tool::Ellipse {
                                                    this.edits.add_ellipse(FrameAnnotation {
    filled: this.shape_filled,
    width: this.stroke_width,
                                                        area: region,
                                                        color: this.frame_color,
                                                    });
                                                } else {
                                                    this.edits.add(region);
                                                }
                                            }
                                        }
                                        cx.notify();
                                    }
                                }),
                            )
                            .child(
                                img(self.render_image.clone())
                                    .absolute()
                                    .top_0()
                                    .left_0()
                                    .size_full(),
                            )
                            .child(
                                gpui::canvas(|_,_,_| (),move |bounds,_,window,_| {
                                    for (render,areas) in [(&blur_render,&blur_preview),(&pixelation_render,&pixelation_preview)] {
                                      if let Some(render) = render {
                                        for area in areas {
                                            let clip = gpui::Bounds { origin: bounds.origin + gpui::point(px(area.x as f32*scale),px(area.y as f32*scale)), size:gpui::size(px(area.width as f32*scale),px(area.height as f32*scale)) };
                                            window.paint_quad(gpui::fill(clip,rgb(0x111318)));
                                            window.with_content_mask(Some(gpui::ContentMask { bounds:clip }), |window| {
                                                window.paint_image(bounds,bounds,Default::default(),render.clone(),0,false).ok();
                                            });
                                        }
                                      }
                                    }
                                }).absolute().top_0().left_0().size_full()
                            )
                            .child(gpui::canvas(|_,_,_| (),move|bounds,_,window,_| {
                                let map=|point:[f32;2]|gpui::point(bounds.origin.x+px(point[0]*scale),bounds.origin.y+px(point[1]*scale));
                                let rect=|area:PixelRegion|gpui::Bounds{origin:map([area.x as f32,area.y as f32]),size:gpui::size(px(area.width as f32*scale),px(area.height as f32*scale))};
                                for item in &paint_order {
                                    match *item {
                                        Selected::Pen(i)=>{let pen=&pen_preview[i];if pen.points.len()==1{let p=pen.positions().next().unwrap();let width=f32::from(pen.width);window.paint_quad(gpui::fill(gpui::Bounds{origin:map([p[0]-width/2.0,p[1]-width/2.0]),size:gpui::size(px(width*scale),px(width*scale))},rgb(pen.color)));}else if let Some(path)=pen_path(pen,bounds.origin,scale){window.paint_path(path,rgb(pen.color));}}
                                        Selected::Highlight(i)=>{let highlight=highlight_preview[i];for area in highlight.segments(PixelRegion{x:0,y:0,width:dimensions.0,height:dimensions.1}){if area.width>0 && area.height>0 {window.paint_quad(gpui::fill(rect(area),rgba((highlight.color<<8)|u32::from(highlight.opacity))));}}}
                                        Selected::Frame(i)=>{let frame=frame_preview[i];for area in frame.segments(){if area.width>0 && area.height>0 {window.paint_quad(gpui::fill(rect(area),rgb(frame.color)));}}}
                                        Selected::Arrow(i)=>{let arrow=arrow_preview[i];let vertices=arrow.vertices();let mut path=gpui::PathBuilder::fill();path.move_to(map(vertices[0]));for point in &vertices[1..]{path.line_to(map(*point));}path.line_to(map(vertices[0]));if let Ok(path)=path.build(){window.paint_path(path,rgb(arrow.color));}}
                                        Selected::Ellipse(i)=>{let ellipse=ellipse_preview[i];let mut path=gpui::PathBuilder::fill();for contour in ellipse.ellipse_contours(){path.move_to(map(contour[0]));for point in &contour[1..]{path.line_to(map(*point));}path.line_to(map(contour[0]));}if let Ok(path)=path.build(){window.paint_path(path,rgb(ellipse.color));}}
                                        Selected::Text(i)=>{let text=&text_preview[i];let area=rect(text.area);window.paint_image(area,area,Default::default(),text.asset.preview.clone(),0,false).ok();}
                                        _=>{}
                                    }
                                }
                            }).absolute().top_0().left_0().size_full())
                            .children(preview.into_iter().map(move |region| {
                                div()
                                    .absolute()
                                    .left(px(region.x as f32 * scale))
                                    .top(px(region.y as f32 * scale))
                                    .w(px(region.width as f32 * scale))
                                    .h(px(region.height as f32 * scale))
                                    .bg(rgb(0x000000))
                            }))
                            .children(
                                crop_preview
                                    .into_iter()
                                    .flat_map(move |area| {
                                        [
                                            PixelRegion {
                                                x: 0,
                                                y: 0,
                                                width: dimensions.0,
                                                height: area.y,
                                            },
                                            PixelRegion {
                                                x: 0,
                                                y: area.y + area.height,
                                                width: dimensions.0,
                                                height: dimensions.1 - area.y - area.height,
                                            },
                                            PixelRegion {
                                                x: 0,
                                                y: area.y,
                                                width: area.x,
                                                height: area.height,
                                            },
                                            PixelRegion {
                                                x: area.x + area.width,
                                                y: area.y,
                                                width: dimensions.0 - area.x - area.width,
                                                height: area.height,
                                            },
                                        ]
                                    })
                                    .filter(|area| area.width > 0 && area.height > 0)
                                    .map(move |area| {
                                        div()
                                            .absolute()
                                            .left(px(area.x as f32 * scale))
                                            .top(px(area.y as f32 * scale))
                                            .w(px(area.width as f32 * scale))
                                            .h(px(area.height as f32 * scale))
                                            .bg(rgba(0x000000aa))
                                    }),
                            )
                            .children(selected_bounds.map(move |area| {
                                div()
                                    .absolute()
                                    .left(px(area.x as f32 * scale))
                                    .top(px(area.y as f32 * scale))
                                    .w(px((area.width as f32 * scale).max(2.0)))
                                    .h(px((area.height as f32 * scale).max(2.0)))
                                    .border_1()
                                    .border_color(rgb(0x22d3ee))
                            }))
                            .children(resize_handles.into_iter().map(move |(_, position)| {
                                div().absolute()
                                    .left(px(position[0]*scale-4.0))
                                    .top(px(position[1]*scale-4.0))
                                    .w(px(8.0)).h(px(8.0))
                                    .bg(rgb(0x22d3ee))
                                    .border_1().border_color(rgb(0x111318))
                            }))
                            .children(crop_preview.map(move |area| {
                                div()
                                    .absolute()
                                    .left(px(area.x as f32 * scale))
                                    .top(px(area.y as f32 * scale))
                                    .w(px(area.width as f32 * scale))
                                    .h(px(area.height as f32 * scale))
                                    .border_2()
                                    .border_color(rgb(0x60a5fa))
                            })),
                    ),
            )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    #[ignore = "short optimized endpoint metadata comparison; no windows, images or clipboard"]
    fn line_endpoint_preview_cost() {
        for line in [false, true] {
            let mut edits = RectEdits::default();
            let arrow = ArrowAnnotation::from_drag(
                [10.0, 20.0],
                [70.0, 20.0],
                [100.0, 100.0],
                [100, 100],
                0,
            )
            .unwrap();
            edits.add_arrow(arrow);
            edits.add_pen(
                app_core::PenAnnotation::from_trace(&[arrow.start, arrow.end], 0).unwrap(),
            );
            let selected = if line {
                Selected::Pen(0)
            } else {
                Selected::Arrow(0)
            };
            let handle = if line {
                ResizeHandle::StrokeEnd
            } else {
                ResizeHandle::ArrowEnd
            };
            let mut samples = Vec::new();
            for _ in 0..100 {
                let started = std::time::Instant::now();
                let mut preview = edits.current.clone();
                assert!(preview.resize(selected, handle, [10.0, 20.0], [100, 100]));
                std::hint::black_box(preview.handles(selected));
                samples.push(started.elapsed());
            }
            samples.sort_unstable();
            eprintln!(
                "Endpoint line={line}: metadata preview/handles p95={:?}",
                samples[94]
            );
        }
    }

    #[test]
    fn line_endpoints_edit_independently_with_preview_and_one_undo() {
        let mut edits = RectEdits::default();
        assert!(
            edits.add_pen(
                line_from_drag(
                    [10.0, 20.0],
                    [70.0, 20.0],
                    [100.0, 100.0],
                    [100, 100],
                    0xff0000,
                    6,
                    false
                )
                .unwrap()
            )
        );
        let selected = Selected::Pen(0);
        let asset = edits.pens[0].points.clone();
        let order = edits.order.clone();
        assert_eq!(
            edits.handles(selected),
            vec![
                (ResizeHandle::StrokeStart, [10.5, 20.5]),
                (ResizeHandle::StrokeEnd, [70.5, 20.5])
            ]
        );
        assert_eq!(
            edits.handle_at(selected, [70.5, 20.5], 1.0),
            Some(ResizeHandle::StrokeEnd)
        );
        let mut preview = edits.current.clone();
        assert!(preview.resize(selected, ResizeHandle::StrokeEnd, [10.0, 20.0], [100, 100]));
        assert_eq!(
            preview.pens[0].positions().collect::<Vec<_>>(),
            vec![[10.5, 20.5], [80.5, 40.5]]
        );
        assert!(Arc::ptr_eq(&asset, &edits.pens[0].points));
        let previous = edits.past.len();
        edits.transform_selected(
            selected,
            Some(ResizeHandle::StrokeEnd),
            [10.0, 20.0],
            [100, 100],
        );
        assert_eq!(edits.past.len(), previous + 1);
        assert_eq!(edits.order, order);
        assert_eq!((edits.pens[0].color, edits.pens[0].width), (0xff0000, 6));
        assert_eq!(
            edits.pens[0].positions().collect::<Vec<_>>(),
            preview.pens[0].positions().collect::<Vec<_>>()
        );
        edits.undo();
        assert!(Arc::ptr_eq(&asset, &edits.pens[0].points));
        edits.redo();
        let committed = edits.pens[0].points.clone();
        let before = edits.past.len();
        edits.transform_selected(
            selected,
            Some(ResizeHandle::StrokeEnd),
            [-70.0, -20.0],
            [100, 100],
        );
        assert_eq!(edits.past.len(), before);
        assert!(Arc::ptr_eq(&committed, &edits.pens[0].points));
        edits.transform_selected(
            selected,
            Some(ResizeHandle::StrokeStart),
            [-f32::MAX, f32::MAX],
            [100, 100],
        );
        assert_eq!(
            edits.pens[0].positions().collect::<Vec<_>>(),
            vec![[0.5, 99.5], [80.5, 40.5]]
        );
        assert!(!edits.current.resize(
            selected,
            ResizeHandle::StrokeEnd,
            [f32::NAN, 0.0],
            [100, 100]
        ));
        assert!(
            !edits
                .current
                .resize(selected, ResizeHandle::StrokeEnd, [1.0, 0.0], [0, 100])
        );
        let mut freehand = RectEdits::default();
        freehand.add_pen(
            app_core::PenAnnotation::from_trace(&[[10, 10], [20, 30], [40, 20]], 0).unwrap(),
        );
        assert_eq!(freehand.handles(Selected::Pen(0)).len(), 4);
    }

    #[test]
    #[ignore = "short optimized line/pen geometry comparison; no images, windows or clipboard"]
    fn line_preview_geometry_cost() {
        for line_tool in [false, true] {
            let mut samples = Vec::new();
            for _ in 0..100 {
                let started = std::time::Instant::now();
                let pen = if line_tool {
                    line_from_drag(
                        [80.0, 80.0],
                        [400.0, 200.0],
                        [3840.0, 2160.0],
                        [3840, 2160],
                        0xff0000,
                        3,
                        false,
                    )
                    .unwrap()
                } else {
                    app_core::PenAnnotation::from_trace(&[[80, 80], [400, 200]], 0xff0000).unwrap()
                };
                std::hint::black_box(pen_path(&pen, gpui::point(px(0.0), px(0.0)), 1.0).unwrap());
                samples.push(started.elapsed());
            }
            samples.sort_unstable();
            eprintln!(
                "Line tool={line_tool}: geometry/path p95={:?}; two shared source points, no image allocation",
                samples[94]
            );
        }
    }

    #[test]
    fn straight_lines_snap_export_and_share_selection_undo() {
        assert_eq!(Tool::shortcut("l"), Some(Tool::Line));
        let line = line_from_drag(
            [10.0, 20.0],
            [70.0, 23.0],
            [100.0, 100.0],
            [100, 100],
            0xff0000,
            3,
            true,
        )
        .unwrap();
        assert_eq!(
            line.positions().collect::<Vec<_>>(),
            vec![[10.5, 20.5], [70.5, 20.5]]
        );
        for (start, end) in [
            ([80.0, 90.0], [1.0, 4.0]),
            ([-20.0, 40.0], [150.0, 44.0]),
            ([22.0, 20.0], [25.0, 80.0]),
        ] {
            let snapped =
                line_from_drag(start, end, [100.0, 100.0], [100, 100], 0, 3, true).unwrap();
            let positions = snapped.positions().collect::<Vec<_>>();
            let dx = (positions[0][0] - positions[1][0]).abs();
            let dy = (positions[0][1] - positions[1][1]).abs();
            assert!(dx == 0.0 || dy == 0.0 || dx == dy);
            assert!(
                positions
                    .iter()
                    .flatten()
                    .all(|point| *point >= 0.0 && *point < 100.0)
            );
        }
        assert!(
            line_from_drag(
                [1.0, 1.0],
                [1.0, 1.0],
                [100.0, 100.0],
                [100, 100],
                0,
                3,
                false
            )
            .is_none()
        );
        let source = CapturedImage {
            display_id: "test".into(),
            window_id: None,
            width: 100,
            height: 100,
            png_path: std::path::PathBuf::from("unused.png"),
            pixels: Arc::new(image::RgbaImage::from_pixel(
                100,
                100,
                image::Rgba([255, 255, 255, 255]),
            )),
            elapsed_ms: 0,
            source_format: app_core::PixelFormat::Rgba8UnormSrgb,
        };
        let annotations = [app_core::DrawAnnotation::Pen(line.clone())];
        let output =
            app_core::render_ordered_snapshot(&source, &[], None, &annotations, &[], None, &[])
                .unwrap();
        assert_eq!(output.pixels.get_pixel(40, 20).0, [255, 0, 0, 255]);
        assert_eq!(output.pixels.get_pixel(68, 17).0, [255, 255, 255, 255]);
        let sanitized = app_core::render_ordered_snapshot(
            &source,
            &[PixelRegion {
                x: 35,
                y: 15,
                width: 10,
                height: 10,
            }],
            None,
            &annotations,
            &[],
            None,
            &[],
        )
        .unwrap();
        assert_eq!(sanitized.pixels.get_pixel(40, 20).0, [0, 0, 0, 255]);
        assert_eq!(source.pixels.get_pixel(40, 20).0, [255, 255, 255, 255]);
        let mut edits = RectEdits::default();
        let points = line.points.clone();
        assert!(edits.add_pen(line));
        let selected = Selected::Pen(0);
        assert_eq!(edits.pick([40.0, 20.0], 0.5), Some(selected));
        edits.set_color(selected, 0);
        edits.undo();
        assert_eq!(edits.color(selected), Some(0xff0000));
        edits.set_stroke_width(selected, 12);
        edits.undo();
        assert_eq!(edits.stroke_width(selected), Some(3));
        let copy = edits.duplicate(selected, [100, 100]).unwrap();
        assert_eq!(edits.pen_points(), 4);
        assert!(Arc::ptr_eq(&points, &edits.pens[1].points));
        edits.delete_selected(copy);
        edits.undo();
        assert_eq!(edits.pens.len(), 2);
        for _ in 2..100 {
            let line = edits.pens[0].clone();
            assert!(edits.add_pen(line));
        }
        assert!(!edits.add_pen(edits.pens[0].clone()));
    }
    #[test]
    fn text_styles_follow_assets_through_undo_duplication_and_transforms() {
        let area = PixelRegion {
            x: 10,
            y: 10,
            width: 20,
            height: 10,
        };
        let regular = caption_fixture("Style", area);
        let styled = Caption {
            area,
            asset: Arc::new(TextAsset {
                bold: true,
                italic: true,
                text: regular.asset.text.clone(),
                font_size: regular.asset.font_size,
                stamp: regular.asset.stamp.clone(),
                preview: regular.asset.preview.clone(),
            }),
        };
        let mut edits = RectEdits::default();
        assert!(edits.set_text(regular.clone(), None));
        assert!(edits.set_text(styled.clone(), Some(0)));
        edits.undo();
        assert!(!edits.texts[0].asset.bold && !edits.texts[0].asset.italic);
        assert!(Arc::ptr_eq(&edits.texts[0].asset, &regular.asset));
        edits.redo();
        assert!(edits.texts[0].asset.bold && edits.texts[0].asset.italic);
        assert!(Arc::ptr_eq(&edits.texts[0].asset, &styled.asset));
        let copy = edits.duplicate(Selected::Text(0), [100, 100]).unwrap();
        let Selected::Text(index) = copy else {
            panic!("Styled text changed annotation type");
        };
        assert!(Arc::ptr_eq(&edits.texts[index].asset, &styled.asset));
        edits.transform_selected(
            copy,
            Some(ResizeHandle::Corner([true, true])),
            [10.0, 5.0],
            [100, 100],
        );
        assert!(edits.texts[index].asset.bold && edits.texts[index].asset.italic);
        assert!(Arc::ptr_eq(
            &edits.texts[index].asset.stamp.mask,
            &styled.asset.stamp.mask
        ));
        assert_eq!(edits.texts[index].asset.bytes(), regular.asset.bytes());
    }
    #[test]
    fn filled_shapes_share_selection_style_duplication_and_undo() {
        let shape = FrameAnnotation {
            area: PixelRegion {
                x: 10,
                y: 10,
                width: 80,
                height: 80,
            },
            color: 0x22c55e,
            width: 6,
            filled: false,
        };
        for selected in [Selected::Frame(0), Selected::Ellipse(0)] {
            let mut edits = RectEdits::default();
            match selected {
                Selected::Frame(_) => edits.add_frame(shape),
                _ => edits.add_ellipse(shape),
            }
            let history = edits.past.len();
            edits.set_filled(selected, false);
            edits.set_filled(Selected::Frame(99), true);
            edits.set_filled(Selected::Highlight(0), true);
            assert_eq!(edits.past.len(), history);
            assert_eq!(edits.pick([50.0, 50.0], 0.0), None);
            edits.set_filled(selected, true);
            assert_eq!(edits.past.len(), history + 1);
            assert_eq!(edits.pick([50.0, 50.0], 0.0), Some(selected));
            if matches!(selected, Selected::Ellipse(_)) {
                assert_eq!(edits.pick([11.0, 11.0], 0.0), None);
            }
            assert_eq!(edits.stroke_width(selected), Some(6));
            assert_eq!(edits.color(selected), Some(0x22c55e));
            assert_eq!(edits.order, [selected]);
            edits.undo();
            assert_eq!(edits.filled(selected), Some(false));
            edits.redo();
            assert_eq!(edits.filled(selected), Some(true));
            edits.transform_selected(
                selected,
                Some(ResizeHandle::Corner([true, true])),
                [5.0, 7.0],
                [120, 120],
            );
            edits.set_color(selected, 0);
            let copy = edits.duplicate(selected, [120, 120]).unwrap();
            assert_eq!(edits.filled(copy), Some(true));
            assert_eq!(edits.color(copy), Some(0));
            assert_eq!(edits.stroke_width(copy), Some(6));
            edits.delete_selected(copy);
            edits.undo();
            assert_eq!(edits.filled(copy), Some(true));
            edits.set_filled(copy, false);
            assert_eq!(edits.stroke_width(copy), Some(6));
            assert_eq!(edits.filled(selected), Some(true));
        }
    }
    #[test]
    fn spotlight_keeps_focus_selection_style_order_and_shared_admission() {
        let mut edits = RectEdits::default();
        let area = PixelRegion {
            x: 10,
            y: 10,
            width: 20,
            height: 20,
        };
        let mark = HighlightAnnotation {
            area,
            color: 0,
            opacity: 191,
            outside: true,
        };
        edits.add_highlight(mark);
        let selected = Selected::Highlight(0);
        assert_eq!(Tool::shortcut("s"), Some(Tool::Spotlight));
        assert_eq!(edits.pick([20.0, 20.0], 0.0), Some(selected));
        assert_eq!(edits.pick([0.0, 0.0], 0.0), None);
        edits.set_highlight_opacity(selected, 128);
        edits.undo();
        assert_eq!(edits.highlights[0], mark);
        edits.redo();
        edits.set_color(selected, 0x22c55e);
        assert!(edits.highlights[0].outside);
        assert_eq!(edits.highlights[0].opacity, 128);
        assert_eq!(edits.color(selected), Some(0x22c55e));
        edits.transform_selected(
            selected,
            Some(ResizeHandle::Corner([true, true])),
            [10.0, 5.0],
            [100, 100],
        );
        assert_eq!(
            edits.bounds(selected),
            Some(PixelRegion {
                width: 30,
                height: 25,
                ..area
            })
        );
        assert!(edits.highlights[0].outside);
        edits.undo();
        assert_eq!(edits.bounds(selected), Some(area));
        let copy = edits.duplicate(selected, [100, 100]).unwrap();
        let Selected::Highlight(index) = copy else {
            panic!("Spotlight changed annotation type");
        };
        assert!(edits.highlights[index].outside);
        assert_eq!(edits.highlights[index].opacity, 128);
        assert_eq!(edits.order.last(), Some(&copy));
        edits.shift_layer(copy, false);
        assert_eq!(edits.order.last(), Some(&selected));
        edits.delete_selected(copy);
        edits.undo();
        assert!(edits.highlights[index].outside);
        for _ in 0..100 {
            edits.add_highlight(mark);
        }
        assert_eq!(edits.highlights.len(), 100);
        assert_eq!(edits.ordered_annotations().len(), 100);
        assert!(edits.past.len() <= 100);
        edits.reset();
        assert!(edits.highlights.is_empty());
    }
    fn caption_fixture(text: &str, area: PixelRegion) -> Caption {
        Caption {
            area,
            asset: Arc::new(TextAsset {
                bold: false,
                italic: false,
                text: text.into(),
                font_size: 24,
                stamp: app_core::TextAnnotation {
                    area,
                    color: 0xef4444,
                    width: area.width,
                    height: area.height,
                    mask: vec![255; (area.width * area.height) as usize].into(),
                },
                preview: Arc::new(gpui::RenderImage::new(vec![image::Frame::new(
                    image::RgbaImage::new(area.width, area.height),
                )])),
            }),
        }
    }
    #[test]
    fn duplicate_all_marks_shares_assets_preserves_style_and_undoes_once() {
        let area = PixelRegion {
            x: 10,
            y: 10,
            width: 20,
            height: 20,
        };
        for selected in [
            Selected::Text(0),
            Selected::Pen(0),
            Selected::Frame(0),
            Selected::Arrow(0),
            Selected::Ellipse(0),
            Selected::Highlight(0),
            Selected::Blur(0),
            Selected::Pixelation(0),
            Selected::Redaction(0),
        ] {
            let mut edits = RectEdits::default();
            let caption = caption_fixture("2.", area);
            let mut pen =
                app_core::PenAnnotation::from_trace(&[[10, 10], [29, 29]], 0x22c55e).unwrap();
            pen.width = 6;
            let points = pen.points.clone();
            let shape = FrameAnnotation {
                filled: false,
                area,
                color: 0x22c55e,
                width: 6,
            };
            match selected {
                Selected::Text(_) => {
                    assert!(edits.set_text(caption.clone(), None));
                }
                Selected::Pen(_) => {
                    assert!(edits.add_pen(pen));
                }
                Selected::Frame(_) => edits.add_frame(shape),
                Selected::Arrow(_) => edits.add_arrow(ArrowAnnotation {
                    start: [29, 29],
                    end: [10, 10],
                    color: 0x22c55e,
                    width: 6,
                }),
                Selected::Ellipse(_) => edits.add_ellipse(shape),
                Selected::Highlight(_) => edits.add_highlight(HighlightAnnotation {
                    outside: false,
                    area,
                    color: 0x22c55e,
                    opacity: 128,
                }),
                Selected::Blur(_) => edits.add_blur(area),
                Selected::Pixelation(_) => edits.add_pixelation(area),
                Selected::Redaction(_) => edits.add(area),
            }
            edits.last_number = 2;
            let history = edits.past.len();
            let order = edits.order.clone();
            let color = edits.color(selected);
            let width = edits.stroke_width(selected);
            let copy = edits.duplicate(selected, [100, 100]).unwrap();
            assert_eq!(edits.past.len(), history + 1);
            assert_ne!(copy, selected);
            assert_eq!(edits.bounds(selected), Some(area));
            assert_eq!(
                edits.bounds(copy),
                Some(PixelRegion {
                    x: 20,
                    y: 20,
                    ..area
                })
            );
            assert_eq!(edits.color(copy), color);
            assert_eq!(edits.stroke_width(copy), width);
            assert_eq!(edits.last_number, 2);
            if order.contains(&selected) {
                assert_eq!(edits.order.last(), Some(&copy));
            } else {
                assert_eq!(edits.order, order);
            }
            if let Selected::Text(index) = copy {
                assert!(Arc::ptr_eq(&edits.texts[index].asset, &caption.asset));
            }
            if let Selected::Pen(index) = copy {
                assert!(Arc::ptr_eq(&edits.pens[index].points, &points));
            }
            if let Selected::Arrow(index) = copy {
                assert_eq!(edits.arrows[index].start, [39, 39]);
                assert_eq!(edits.arrows[index].end, [20, 20]);
            }
            if let Selected::Highlight(index) = copy {
                assert_eq!(edits.highlights[index].opacity, 128);
            }
            edits.undo();
            assert_eq!(edits.bounds(copy), None);
            assert_eq!(edits.bounds(selected), Some(area));
            assert_eq!(edits.order, order);
            edits.redo();
            assert!(edits.bounds(copy).is_some());
            edits.delete_selected(copy);
            edits.undo();
            assert!(edits.bounds(copy).is_some());
        }
    }
    #[test]
    fn duplicate_clamps_at_image_edges_and_rejects_full_or_invalid_documents() {
        let mut edits = RectEdits::default();
        let area = PixelRegion {
            x: 90,
            y: 90,
            width: 10,
            height: 10,
        };
        edits.add(area);
        let copy = edits.duplicate(Selected::Redaction(0), [100, 100]).unwrap();
        assert_eq!(
            edits.bounds(copy),
            Some(PixelRegion {
                x: 80,
                y: 80,
                ..area
            })
        );
        let history = edits.past.len();
        assert!(edits.duplicate(Selected::Redaction(9), [100, 100]).is_err());
        assert!(edits.duplicate(Selected::Redaction(0), [50, 50]).is_err());
        assert_eq!(edits.past.len(), history);
        for _ in 0..98 {
            edits.add_blur(area);
        }
        assert!(edits.duplicate(Selected::Redaction(0), [100, 100]).is_err());
        assert_eq!(edits.past.len(), 100);
        assert_eq!(edits.regions.len(), 2);
        edits.reset();
        let full = PixelRegion {
            x: 0,
            y: 0,
            width: 100,
            height: 100,
        };
        edits.add(full);
        let copy = edits.duplicate(Selected::Redaction(0), [100, 100]).unwrap();
        assert_eq!(edits.bounds(copy), Some(full));
    }
    #[test]
    fn duplication_keeps_pen_and_text_documents_within_export_limits() {
        let mut edits = RectEdits::default();
        let trace = (0..app_core::MAX_PEN_POINTS)
            .map(|x| [x as u32, 0])
            .collect::<Vec<_>>();
        let pen = app_core::PenAnnotation::from_trace(&trace, 0).unwrap();
        let points = pen.points.clone();
        assert!(edits.add_pen(pen));
        for _ in 0..7 {
            edits.duplicate(Selected::Pen(0), [2048, 1024]).unwrap();
        }
        assert_eq!(edits.pen_points(), app_core::MAX_DOCUMENT_PEN_POINTS);
        let history = edits.past.len();
        assert!(edits.duplicate(Selected::Pen(0), [2048, 1024]).is_err());
        assert_eq!(edits.past.len(), history);
        assert!(
            edits
                .pens
                .iter()
                .all(|pen| Arc::ptr_eq(&pen.points, &points))
        );
        edits.undo();
        assert!(edits.duplicate(Selected::Pen(0), [2048, 1024]).is_ok());
        assert!(edits.undone.is_empty());
        edits.reset();
        let large = caption_fixture(
            "Large",
            PixelRegion {
                x: 0,
                y: 0,
                width: 1024,
                height: 128,
            },
        );
        let small = caption_fixture(
            "Small",
            PixelRegion {
                x: 0,
                y: 0,
                width: 16,
                height: 16,
            },
        );
        assert!(edits.set_text(large.clone(), None));
        for _ in 0..31 {
            edits.duplicate(Selected::Text(0), [2048, 1024]).unwrap();
        }
        assert_eq!(edits.text_mask_bytes(), 4 * 1024 * 1024);
        assert!(
            edits
                .texts
                .iter()
                .all(|text| Arc::ptr_eq(&text.asset, &large.asset))
        );
        let history = edits.past.len();
        edits.last_number = 7;
        assert!(edits.duplicate(Selected::Text(0), [2048, 1024]).is_err());
        assert!(!edits.set_text(small.clone(), None));
        assert!(!edits.add_number(small.clone()));
        assert_eq!(edits.last_number, 7);
        assert_eq!(edits.past.len(), history);
        assert!(edits.set_text(large.clone(), Some(0)));
        assert!(edits.set_text(small.clone(), Some(0)));
        assert!(edits.set_text(small, None));
        let history = edits.past.len();
        assert!(!edits.set_text(large, Some(0)));
        assert_eq!(edits.past.len(), history);
        assert_eq!(edits.texts[0].asset.text, "Small");
    }
    #[test]
    #[ignore = "short optimized duplication timing; no desktop, files or clipboard"]
    fn duplication_metadata_cost() {
        for count in [1, 99] {
            let mut edits = RectEdits::default();
            for _ in 0..count {
                edits.add_frame(FrameAnnotation {
                    filled: false,
                    area: PixelRegion {
                        x: 10,
                        y: 10,
                        width: 20,
                        height: 20,
                    },
                    color: 0xef4444,
                    width: 6,
                });
            }
            let mut samples = Vec::with_capacity(2000);
            for _ in 0..2000 {
                let start = std::time::Instant::now();
                std::hint::black_box(edits.duplicate(Selected::Frame(0), [3840, 2160]).unwrap());
                samples.push(start.elapsed().as_nanos());
                edits.undo();
            }
            assert!(edits.past.len() <= 100);
            assert_eq!(edits.frames.len(), count);
            samples.sort_unstable();
            eprintln!(
                "Duplicate {count} marks: metadata command p95={} ns; shared pixels/assets, no image job",
                samples[1900]
            );
        }
    }
    #[test]
    fn shape_width_selection_and_undo_keep_geometry_color_and_order() {
        let area = PixelRegion {
            x: 10,
            y: 10,
            width: 80,
            height: 80,
        };
        for (selected, point) in [
            (Selected::Frame(0), [19.0, 50.0]),
            (Selected::Ellipse(0), [20.0, 50.0]),
            (Selected::Arrow(0), [150.0, 85.0]),
        ] {
            let mut edits = RectEdits::default();
            let shape = FrameAnnotation {
                filled: false,
                area,
                color: 0xef4444,
                width: 3,
            };
            match selected {
                Selected::Frame(_) => edits.add_frame(shape),
                Selected::Ellipse(_) => edits.add_ellipse(shape),
                Selected::Arrow(_) => edits.add_arrow(ArrowAnnotation {
                    start: [10, 64],
                    end: [220, 64],
                    color: 0xef4444,
                    width: 3,
                }),
                _ => unreachable!(),
            }
            let original_bounds = edits.bounds(selected);
            assert_eq!(edits.pick(point, 0.0), None);
            let history = edits.past.len();
            for (target, width) in [
                (selected, 3),
                (selected, 0),
                (selected, 13),
                (Selected::Arrow(9), 6),
                (Selected::Highlight(0), 6),
            ] {
                edits.set_stroke_width(target, width);
            }
            assert_eq!(edits.past.len(), history);
            edits.set_stroke_width(selected, 12);
            assert_eq!(edits.stroke_width(selected), Some(12));
            assert_eq!(edits.pick(point, 0.0), Some(selected));
            assert_eq!(edits.bounds(selected), original_bounds);
            assert_eq!(edits.color(selected), Some(0xef4444));
            assert_eq!(edits.order, [selected]);
            edits.undo();
            assert_eq!(edits.stroke_width(selected), Some(3));
            edits.redo();
            assert_eq!(edits.stroke_width(selected), Some(12));
            edits.transform_selected(selected, None, [2.0, 3.0], [256, 128]);
            assert_eq!(edits.stroke_width(selected), Some(12));
            let handle = if matches!(selected, Selected::Arrow(_)) {
                ResizeHandle::ArrowEnd
            } else {
                ResizeHandle::Corner([true, true])
            };
            edits.transform_selected(selected, Some(handle), [4.0, 5.0], [256, 128]);
            assert_eq!(edits.stroke_width(selected), Some(12));
            edits.set_color(selected, 0x22c55e);
            assert_eq!(edits.stroke_width(selected), Some(12));
            edits.delete_selected(selected);
            edits.undo();
            assert_eq!(edits.stroke_width(selected), Some(12));
        }
    }
    #[test]
    fn numbering_undo_restart_delete_and_admission_keep_sequence_and_shared_assets() {
        let area = PixelRegion {
            x: 1,
            y: 1,
            width: 1,
            height: 1,
        };
        let caption = |text: &str| Caption {
            area,
            asset: Arc::new(TextAsset {
                bold: false,
                italic: false,
                text: text.into(),
                font_size: 24,
                stamp: app_core::TextAnnotation {
                    area,
                    color: 0xef4444,
                    width: 1,
                    height: 1,
                    mask: vec![255].into(),
                },
                preview: Arc::new(gpui::RenderImage::new(vec![image::Frame::new(
                    image::RgbaImage::new(1, 1),
                )])),
            }),
        };
        let mut edits = RectEdits::default();
        assert_eq!(Tool::shortcut("n"), Some(Tool::Number));
        edits.restart_numbers();
        assert!(edits.past.is_empty());
        assert!(edits.add_number(caption("1.")));
        assert!(edits.add_number(caption("2.")));
        let asset = edits.texts[1].asset.clone();
        edits.undo();
        assert_eq!(edits.last_number, 1);
        assert_eq!(edits.texts.len(), 1);
        edits.redo();
        assert_eq!(edits.last_number, 2);
        assert!(Arc::ptr_eq(&edits.texts[1].asset, &asset));
        edits.delete_selected(Selected::Text(1));
        assert_eq!(edits.last_number, 2);
        edits.undo();
        edits.set_text(caption("Edited number"), Some(1));
        assert_eq!(edits.last_number, 2);
        edits.restart_numbers();
        assert_eq!(edits.last_number, 0);
        assert_eq!(edits.texts.len(), 2);
        edits.undo();
        assert_eq!(edits.last_number, 2);
        edits.redo();
        assert!(edits.add_number(caption("1.")));
        assert_eq!(edits.last_number, 1);
        assert_eq!(edits.order.last(), Some(&Selected::Text(2)));
        edits.add_blur(area);
        while edits.order.len() + edits.blurs.len() < 100 {
            assert!(edits.add_number(caption("number")));
        }
        let number = edits.last_number;
        let past = edits.past.len();
        assert!(!edits.add_number(caption("full")));
        assert_eq!(edits.last_number, number);
        assert_eq!(edits.past.len(), past);
        edits.reset();
        assert_eq!(edits.last_number, 0);
        edits.last_number = u16::MAX;
        assert!(!edits.add_number(caption("overflow")));
        assert!(edits.texts.is_empty());
        edits.restart_numbers();
        assert!(edits.add_number(caption("1.")));
    }
    #[test]
    fn stroke_width_selection_and_undo_share_points_and_reject_invalid_changes() {
        let mut edits = RectEdits::default();
        let pen = app_core::PenAnnotation::from_trace(&[[10, 20], [40, 20]], 0xef4444).unwrap();
        let area = pen.area;
        let points = pen.points.clone();
        edits.add_pen(pen);
        let selected = Selected::Pen(0);
        let past = edits.past.len();
        for (target, width) in [
            (selected, 3),
            (selected, 0),
            (selected, 13),
            (Selected::Pen(9), 6),
            (Selected::Highlight(0), 6),
        ] {
            edits.set_stroke_width(target, width);
        }
        assert_eq!(edits.past.len(), past);
        assert_eq!(edits.pick([20.5, 25.5], 0.0), None);
        edits.set_stroke_width(selected, 12);
        assert_eq!(edits.pick([20.5, 25.5], 0.0), Some(selected));
        assert_eq!(edits.pick([20.5, 27.5], 0.0), None);
        assert_eq!(edits.pens[0].area, area);
        assert_eq!(edits.pens[0].color, 0xef4444);
        assert_eq!(edits.order, [selected]);
        assert!(Arc::ptr_eq(&edits.pens[0].points, &points));
        edits.undo();
        assert_eq!(edits.pens[0].width, 3);
        edits.redo();
        assert_eq!(edits.pens[0].width, 12);
        assert!(Arc::ptr_eq(&edits.pens[0].points, &points));
        let mut dot = app_core::PenAnnotation::from_trace(&[[50, 50]], 0).unwrap();
        dot.width = 12;
        edits.add_pen(dot);
        assert_eq!(edits.pick([55.5, 55.5], 0.0), Some(Selected::Pen(1)));
        assert_eq!(edits.pick([57.5, 55.5], 0.0), None);
    }
    #[test]
    fn highlight_opacity_changes_only_metadata_and_supports_undo() {
        let mut edits = RectEdits::default();
        let mark = HighlightAnnotation {
            outside: false,
            area: PixelRegion {
                x: 10,
                y: 10,
                width: 20,
                height: 20,
            },
            color: 0xfacc15,
            opacity: 85,
        };
        edits.add_highlight(mark);
        let selected = Selected::Highlight(0);
        let past = edits.past.len();
        for (target, opacity) in [
            (selected, 85),
            (selected, 0),
            (Selected::Highlight(9), 128),
            (Selected::Frame(0), 128),
        ] {
            edits.set_highlight_opacity(target, opacity);
        }
        assert_eq!(edits.past.len(), past);
        let order = edits.order.clone();
        edits.set_highlight_opacity(selected, 128);
        assert_eq!(
            edits.highlights[0],
            HighlightAnnotation {
                outside: false,
                opacity: 128,
                ..mark
            }
        );
        assert_eq!(edits.order, order);
        edits.undo();
        assert_eq!(edits.highlights[0], mark);
        edits.redo();
        assert_eq!(edits.highlights[0].opacity, 128);
        edits.set_highlight_opacity(selected, 255);
        assert_eq!(edits.highlights[0].opacity, 255);
    }
    #[test]
    fn pen_selection_transform_color_and_undo_share_bounded_points() {
        let mut edits = RectEdits::default();
        let pen =
            app_core::PenAnnotation::from_trace(&[[10, 10], [30, 10], [30, 30]], 0xef4444).unwrap();
        let points = pen.points.clone();
        let area = pen.area;
        assert!(edits.add_pen(pen));
        let selected = Selected::Pen(0);
        assert_eq!(edits.pick([20.0, 10.5], 0.5), Some(selected));
        assert_eq!(edits.pick([15.0, 25.0], 0.5), None);
        edits.set_color(selected, 0x22c55e);
        assert_eq!(edits.color(selected), Some(0x22c55e));
        edits.undo();
        assert_eq!(edits.color(selected), Some(0xef4444));
        edits.transform_selected(selected, None, [5.0, 5.0], [100, 100]);
        edits.transform_selected(
            selected,
            Some(ResizeHandle::Corner([true, true])),
            [5.0, 5.0],
            [100, 100],
        );
        assert!(Arc::ptr_eq(&points, &edits.pens[0].points));
        edits.undo();
        edits.undo();
        assert_eq!(edits.bounds(selected), Some(area));
        edits.delete_selected(selected);
        assert!(edits.pens.is_empty());
        edits.undo();
        assert!(Arc::ptr_eq(&points, &edits.pens[0].points));
        assert_eq!(Tool::shortcut("d"), Some(Tool::Pen));
        assert_eq!(pen_point([-20.0, 1000.0], 2.0, [100, 50]), [0, 49]);
        let trace = (0..1024).map(|i| [i, 1]).collect::<Vec<_>>();
        let large = app_core::PenAnnotation::from_trace(&trace, 0).unwrap();
        edits.reset();
        for _ in 0..8 {
            assert!(edits.add_pen(large.clone()));
        }
        assert!(!edits.add_pen(large));
        assert_eq!(edits.pen_points(), 8192);
        edits.reset();
        assert!(edits.pens.is_empty());
    }
    #[test]
    #[ignore = "short optimized pen preview tessellation; no desktop or clipboard"]
    fn pen_preview_geometry_cost() {
        let trace = (0..1024)
            .map(|i| [i * 2, 100 + (i % 2) * 3])
            .collect::<Vec<_>>();
        let mut pen = app_core::PenAnnotation::from_trace(&trace, 0).unwrap();
        for (count, width) in [(1, 3), (1, 12), (8, 3), (8, 12)] {
            pen.width = width;
            let mut samples = Vec::new();
            for _ in 0..50 {
                let start = std::time::Instant::now();
                for _ in 0..count {
                    std::hint::black_box(
                        pen_path(&pen, gpui::point(px(0.0), px(0.0)), 1.0).unwrap(),
                    );
                }
                samples.push(start.elapsed());
            }
            samples.sort();
            eprintln!(
                "Pen preview: {} points, width={width}, tessellation p95={:?}",
                count * 1024,
                samples[47]
            );
        }
    }
    #[test]
    fn highlights_share_selection_color_resize_order_and_bounded_undo() {
        let mut edits = RectEdits::default();
        let area = PixelRegion {
            x: 10,
            y: 10,
            width: 20,
            height: 20,
        };
        edits.add_highlight(HighlightAnnotation {
            outside: false,
            opacity: 85,
            area,
            color: 0xfacc15,
        });
        let selected = Selected::Highlight(0);
        assert_eq!(edits.pick([20.0, 20.0], 1.0), Some(selected));
        assert_eq!(Tool::shortcut("h"), Some(Tool::Highlight));
        edits.set_color(selected, 0x22c55e);
        assert_eq!(edits.color(selected), Some(0x22c55e));
        edits.undo();
        assert_eq!(edits.color(selected), Some(0xfacc15));
        edits.transform_selected(
            selected,
            Some(ResizeHandle::Corner([true, true])),
            [10.0, 10.0],
            [100, 100],
        );
        assert_eq!(edits.bounds(selected).unwrap().width, 30);
        edits.undo();
        assert_eq!(edits.bounds(selected), Some(area));
        edits.add_frame(FrameAnnotation {
            filled: false,
            width: 3,
            area,
            color: 0xef4444,
        });
        edits.shift_layer(selected, true);
        assert_eq!(edits.order.last(), Some(&selected));
        edits.delete_selected(selected);
        assert!(edits.highlights.is_empty());
        edits.undo();
        assert_eq!(edits.order.last(), Some(&selected));
        edits.add_highlight(HighlightAnnotation {
            outside: false,
            opacity: 85,
            area,
            color: 0xfacc15,
        });
        edits.delete_selected(selected);
        assert!(edits.order.contains(&Selected::Highlight(0)));
        assert!(!edits.order.contains(&Selected::Highlight(1)));
        assert_eq!(edits.ordered_annotations().len(), 2);
        for _ in 0..100 {
            edits.add_highlight(HighlightAnnotation {
                outside: false,
                opacity: 85,
                area,
                color: 0xfacc15,
            });
        }
        assert_eq!(edits.highlights.len(), 99);
        edits.add_blur(area);
        assert!(edits.blurs.is_empty());
        edits.reset();
        assert!(edits.highlights.is_empty());
        assert!(edits.order.is_empty());
    }
    #[test]
    fn annotation_order_tracks_creation_reorder_delete_and_undo() {
        let mut edits = RectEdits::default();
        let area = PixelRegion {
            x: 10,
            y: 10,
            width: 40,
            height: 40,
        };
        let shape = FrameAnnotation {
            filled: false,
            width: 3,
            area,
            color: 0xff0000,
        };
        edits.add_ellipse(shape);
        edits.add_frame(shape);
        edits.add_frame(shape);
        assert_eq!(
            edits.order,
            vec![Selected::Ellipse(0), Selected::Frame(0), Selected::Frame(1)]
        );
        assert_eq!(edits.pick([10.0, 30.0], 1.0), Some(Selected::Frame(1)));
        edits.shift_layer(Selected::Ellipse(0), true);
        edits.shift_layer(Selected::Ellipse(0), true);
        assert_eq!(edits.pick([10.0, 30.0], 1.0), Some(Selected::Ellipse(0)));
        let before = edits.past.len();
        edits.shift_layer(Selected::Ellipse(0), true);
        assert_eq!(edits.past.len(), before);
        edits.delete_selected(Selected::Frame(0));
        assert_eq!(edits.order, vec![Selected::Frame(0), Selected::Ellipse(0)]);
        assert_eq!(edits.ordered_annotations().len(), 2);
        edits.undo();
        assert_eq!(
            edits.order,
            vec![Selected::Frame(0), Selected::Frame(1), Selected::Ellipse(0)]
        );
        edits.undo();
        assert_eq!(
            edits.order,
            vec![Selected::Frame(0), Selected::Ellipse(0), Selected::Frame(1)]
        );
        edits.redo();
        assert_eq!(
            edits.order,
            vec![Selected::Frame(0), Selected::Frame(1), Selected::Ellipse(0)]
        );
        edits.add(area);
        assert_eq!(edits.pick([10.0, 30.0], 1.0), Some(Selected::Redaction(0)));
        assert!(edits.layer_target(Selected::Redaction(0), false).is_none());
        edits.add_blur(area);
        edits.add_pixelation(area);
        assert!(edits.layer_target(Selected::Blur(0), true).is_none());
        assert!(edits.layer_target(Selected::Pixelation(0), true).is_none());
        edits.delete_selected(Selected::Redaction(0));
        assert_eq!(edits.order.len(), 3);
        edits.reset();
        assert!(edits.order.is_empty());
    }
    #[test]
    fn text_selection_transform_and_undo_share_the_prepared_mask() {
        let area = PixelRegion {
            x: 1,
            y: 1,
            width: 2,
            height: 2,
        };
        let asset = Arc::new(TextAsset {
            bold: false,
            italic: false,
            text: "label".into(),
            font_size: 24,
            stamp: app_core::TextAnnotation {
                area,
                color: 0xef4444,
                width: 2,
                height: 2,
                mask: vec![0, 255, 128, 255].into(),
            },
            preview: Arc::new(gpui::RenderImage::new(vec![image::Frame::new(
                image::RgbaImage::new(2, 2),
            )])),
        });
        let mut edits = RectEdits::default();
        edits.set_text(
            Caption {
                area,
                asset: asset.clone(),
            },
            None,
        );
        let selected = Selected::Text(0);
        assert_eq!(edits.pick([2.0, 2.0], 1.0), Some(selected));
        assert_eq!(Tool::shortcut("t"), Some(Tool::Text));
        edits.transform_selected(
            selected,
            Some(ResizeHandle::Corner([true, true])),
            [3.0, 3.0],
            [20, 20],
        );
        assert_eq!(edits.bounds(selected).unwrap().width, 5);
        assert!(Arc::ptr_eq(
            &edits.texts[0].asset.stamp.mask,
            &asset.stamp.mask
        ));
        edits.undo();
        assert_eq!(edits.bounds(selected), Some(area));
        edits.transform_selected(selected, None, [5.0, 5.0], [20, 20]);
        assert_eq!(edits.bounds(selected).unwrap().x, 6);
        edits.delete_selected(selected);
        assert!(edits.texts.is_empty());
        edits.undo();
        assert!(Arc::ptr_eq(&edits.texts[0].asset, &asset));
        for _ in 0..100 {
            edits.set_text(
                Caption {
                    area,
                    asset: asset.clone(),
                },
                None,
            );
        }
        assert_eq!(edits.texts.len(), 100);
        edits.add_blur(area);
        assert!(edits.blurs.is_empty());
        edits.reset();
        assert!(edits.texts.is_empty());
    }
    #[test]
    fn blur_regions_share_selection_transform_delete_and_undo_limits() {
        let mut edits = RectEdits::default();
        let area = PixelRegion {
            x: 10,
            y: 10,
            width: 20,
            height: 20,
        };
        edits.add_blur(area);
        let selected = Selected::Blur(0);
        assert_eq!(edits.pick([15.0, 15.0], 1.0), Some(selected));
        assert_eq!(Tool::shortcut("b"), Some(Tool::Blur));
        edits.transform_selected(
            selected,
            Some(ResizeHandle::Corner([true, true])),
            [10.0, 10.0],
            [100, 100],
        );
        assert_eq!(edits.bounds(selected).unwrap().width, 30);
        edits.undo();
        assert_eq!(edits.bounds(selected), Some(area));
        edits.transform_selected(selected, None, [5.0, 5.0], [100, 100]);
        assert_eq!(edits.bounds(selected).unwrap().x, 15);
        edits.delete_selected(selected);
        assert!(edits.blurs.is_empty());
        edits.undo();
        assert_eq!(edits.blurs.len(), 1);
        for _ in 0..100 {
            edits.add_blur(area);
        }
        assert_eq!(edits.blurs.len(), 100);
        edits.add_pixelation(area);
        assert!(edits.pixelations.is_empty());
        edits.reset();
        assert!(edits.blurs.is_empty());
    }
    #[test]
    fn pixelation_uses_shared_selection_resize_and_undo_limits() {
        let mut edits = RectEdits::default();
        let area = PixelRegion {
            x: 10,
            y: 10,
            width: 20,
            height: 20,
        };
        edits.add_pixelation(area);
        let selected = Selected::Pixelation(0);
        assert_eq!(edits.pick([15.0, 15.0], 1.0), Some(selected));
        assert_eq!(Tool::shortcut("p"), Some(Tool::Pixelate));
        edits.transform_selected(
            selected,
            Some(ResizeHandle::Corner([true, true])),
            [10.0, 10.0],
            [100, 100],
        );
        assert_eq!(edits.bounds(selected).unwrap().width, 30);
        edits.undo();
        assert_eq!(edits.bounds(selected), Some(area));
        edits.transform_selected(selected, None, [5.0, 5.0], [100, 100]);
        assert_eq!(edits.bounds(selected).unwrap().x, 15);
        edits.delete_selected(selected);
        assert!(edits.pixelations.is_empty());
        edits.undo();
        assert_eq!(edits.pixelations.len(), 1);
        for _ in 0..100 {
            edits.add_pixelation(area);
        }
        assert_eq!(edits.pixelations.len(), 100);
        edits.add(area);
        assert!(edits.regions.is_empty());
        edits.reset();
        assert!(edits.pixelations.is_empty());
    }
    #[test]
    fn selected_colors_are_undoable_and_never_change_redactions() {
        let mut edits = RectEdits::default();
        let area = PixelRegion {
            x: 10,
            y: 20,
            width: 30,
            height: 40,
        };
        let mark = FrameAnnotation {
            filled: false,
            width: 3,
            area,
            color: 0xef4444,
        };
        edits.add(area);
        edits.add_frame(mark);
        edits.add_ellipse(mark);
        edits.add_arrow(ArrowAnnotation {
            width: 3,
            start: [10, 20],
            end: [39, 59],
            color: mark.color,
        });
        for selected in [Selected::Frame(0), Selected::Arrow(0), Selected::Ellipse(0)] {
            let before = edits.past.len();
            edits.set_color(selected, 0x22c55e);
            assert_eq!(edits.color(selected), Some(0x22c55e));
            assert_eq!(edits.past.len(), before + 1);
            assert_eq!(edits.bounds(selected), Some(area));
            edits.set_color(selected, 0x22c55e);
            edits.set_color(selected, 0x1000000);
            assert_eq!(edits.past.len(), before + 1);
            edits.undo();
            assert_eq!(edits.color(selected), Some(mark.color));
            edits.redo();
            assert_eq!(edits.color(selected), Some(0x22c55e));
        }
        let before = edits.past.len();
        edits.set_color(Selected::Redaction(0), 0x22c55e);
        edits.set_color(Selected::Frame(100), 0x22c55e);
        assert_eq!(edits.past.len(), before);
        assert_eq!(edits.regions, vec![area]);
        assert_eq!(edits.color(Selected::Redaction(0)), None);
    }
    #[test]
    fn shortcuts_and_nudges_use_source_pixels_and_keep_bounded_undo() {
        for (key, tool) in [
            ("v", Tool::Select),
            ("x", Tool::Redact),
            ("c", Tool::Crop),
            ("r", Tool::Frame),
            ("a", Tool::Arrow),
            ("l", Tool::Line),
            ("e", Tool::Ellipse),
        ] {
            assert_eq!(Tool::shortcut(key), Some(tool));
        }
        assert_eq!(Tool::shortcut("z"), None);
        assert_eq!(nudge_delta("z", false), None);
        let mut edits = RectEdits::default();
        edits.add(PixelRegion {
            x: 1,
            y: 1,
            width: 2,
            height: 2,
        });
        let selected = Selected::Redaction(0);
        for (key, shift) in [
            ("left", false),
            ("up", false),
            ("right", true),
            ("down", true),
        ] {
            edits.transform_selected(selected, None, nudge_delta(key, shift).unwrap(), [20, 20]);
        }
        assert_eq!(
            edits.bounds(selected),
            Some(PixelRegion {
                x: 10,
                y: 10,
                width: 2,
                height: 2
            })
        );
        edits.undo();
        assert_eq!(edits.regions[0].y, 0);
        edits.redo();
        assert_eq!(edits.regions[0].y, 10);
        for _ in 0..120 {
            edits.transform_selected(
                selected,
                None,
                nudge_delta("left", false).unwrap(),
                [20, 20],
            );
            edits.transform_selected(
                selected,
                None,
                nudge_delta("right", false).unwrap(),
                [20, 20],
            );
        }
        assert_eq!(edits.past.len(), 100);
        edits.transform_selected(selected, None, [-500.0, -500.0], [20, 20]);
        let before = edits.past.len();
        edits.transform_selected(selected, None, nudge_delta("left", true).unwrap(), [20, 20]);
        assert_eq!(edits.regions[0].x, 0);
        assert_eq!(edits.past.len(), before);
    }
    #[test]
    fn resize_corners_preserve_opposite_anchor_and_single_undo_step() {
        let original = PixelRegion {
            x: 10,
            y: 20,
            width: 30,
            height: 40,
        };
        let frame = FrameAnnotation {
            filled: false,
            width: 3,
            area: original,
            color: 0x22c55e,
        };
        for selected in [
            Selected::Redaction(0),
            Selected::Frame(0),
            Selected::Ellipse(0),
        ] {
            for corner in [[false, false], [true, false], [true, true], [false, true]] {
                let mut edits = RectEdits::default();
                edits.add(original);
                edits.add_frame(frame);
                edits.add_ellipse(frame);
                let handle = ResizeHandle::Corner(corner);
                let before = edits.past.len();
                let delta = [
                    if corner[0] { 10.0 } else { -5.0 },
                    if corner[1] { 10.0 } else { -5.0 },
                ];
                let mut preview = edits.snapshot();
                assert!(preview.resize(selected, handle, delta, [100, 100]));
                assert_eq!(edits.bounds(selected), Some(original));
                edits.transform_selected(selected, Some(handle), delta, [100, 100]);
                assert_eq!(edits.bounds(selected), preview.bounds(selected));
                assert_eq!(edits.past.len(), before + 1);
                assert_eq!(
                    edits.bounds(selected),
                    Some(PixelRegion {
                        x: if corner[0] { 10 } else { 5 },
                        y: if corner[1] { 20 } else { 15 },
                        width: if corner[0] { 40 } else { 35 },
                        height: if corner[1] { 50 } else { 45 },
                    })
                );
                edits.undo();
                assert_eq!(edits.bounds(selected), Some(original));
                edits.redo();
                assert_eq!(edits.bounds(selected), preview.bounds(selected));
                assert_eq!(edits.frames[0].color, frame.color);
            }
        }
    }
    #[test]
    fn resize_clamps_edges_and_rejects_collapsed_arrow() {
        let mut edits = RectEdits::default();
        edits.add(PixelRegion {
            x: 10,
            y: 20,
            width: 30,
            height: 40,
        });
        let selected = Selected::Redaction(0);
        assert_eq!(
            edits.handle_at(selected, [40.0, 60.0], 1.0),
            Some(ResizeHandle::Corner([true, true]))
        );
        assert_eq!(edits.handle_at(selected, [25.0, 40.0], 1.0), None);
        edits.transform_selected(
            selected,
            Some(ResizeHandle::Corner([true, true])),
            [500.0, 500.0],
            [100, 100],
        );
        assert_eq!(
            edits.bounds(selected),
            Some(PixelRegion {
                x: 10,
                y: 20,
                width: 90,
                height: 80
            })
        );
        edits.transform_selected(
            selected,
            Some(ResizeHandle::Corner([false, false])),
            [500.0, 500.0],
            [100, 100],
        );
        assert_eq!(
            edits.bounds(selected),
            Some(PixelRegion {
                x: 99,
                y: 99,
                width: 1,
                height: 1
            })
        );
        assert_eq!(
            edits.handle_at(selected, [100.0, 100.0], 7.0),
            Some(ResizeHandle::Corner([true, true]))
        );
        edits.add_arrow(ArrowAnnotation {
            width: 3,
            start: [10, 20],
            end: [40, 60],
            color: 0xff0000,
        });
        let arrow = Selected::Arrow(0);
        assert_eq!(edits.handles(arrow).len(), 2);
        assert_eq!(
            edits.handle_at(arrow, [10.5, 20.5], 1.0),
            Some(ResizeHandle::ArrowStart)
        );
        let before = edits.past.len();
        edits.transform_selected(
            arrow,
            Some(ResizeHandle::ArrowStart),
            [30.0, 40.0],
            [100, 100],
        );
        assert_eq!(edits.past.len(), before);
        edits.transform_selected(
            arrow,
            Some(ResizeHandle::ArrowEnd),
            [-500.0, 500.0],
            [100, 100],
        );
        assert_eq!(edits.arrows[0].start, [10, 20]);
        assert_eq!(edits.arrows[0].end, [0, 99]);
        edits.undo();
        assert_eq!(edits.arrows[0].end, [40, 60]);
        edits.redo();
        assert_eq!(edits.arrows[0].end, [0, 99]);
    }
    #[test]
    fn selection_moves_and_deletes_all_mark_types_with_bounded_undo() {
        let area = PixelRegion {
            x: 10,
            y: 20,
            width: 30,
            height: 40,
        };
        let shape = FrameAnnotation {
            filled: false,
            width: 3,
            area,
            color: 0xff0000,
        };
        let mut edits = RectEdits::default();
        edits.add(area);
        edits.add_frame(shape);
        edits.add_ellipse(shape);
        edits.add_arrow(ArrowAnnotation {
            width: 3,
            start: [10, 20],
            end: [39, 59],
            color: 0xff0000,
        });
        for selected in [
            Selected::Redaction(0),
            Selected::Frame(0),
            Selected::Ellipse(0),
            Selected::Arrow(0),
        ] {
            let before = edits.bounds(selected).unwrap();
            let history = edits.past.len();
            edits.transform_selected(selected, None, [500.0, -500.0], [100, 100]);
            assert_eq!(
                edits.bounds(selected).unwrap(),
                PixelRegion {
                    x: 70,
                    y: 0,
                    ..before
                }
            );
            assert_eq!(edits.past.len(), history + 1);
            edits.undo();
            assert_eq!(edits.bounds(selected), Some(before));
            edits.redo();
            edits.delete_selected(selected);
            assert!(edits.bounds(selected).is_none());
            edits.undo();
            assert!(edits.bounds(selected).is_some());
            edits.undo();
            assert_eq!(edits.bounds(selected), Some(before));
        }
        let history = edits.past.len();
        edits.transform_selected(Selected::Redaction(0), None, [0.1, 0.1], [100, 100]);
        assert_eq!(edits.past.len(), history);
        assert_eq!(edits.arrows[0].end, [39, 59]);
        assert_eq!(edits.frames[0].color, 0xff0000);
    }
    #[test]
    fn picking_respects_visible_layers_and_hollow_shapes() {
        let mut edits = RectEdits::default();
        let area = PixelRegion {
            x: 10,
            y: 10,
            width: 40,
            height: 40,
        };
        edits.add_frame(FrameAnnotation {
            filled: false,
            width: 3,
            area,
            color: 0xff0000,
        });
        assert_eq!(edits.pick([30.0, 30.0], 1.0), None);
        assert_eq!(edits.pick([10.0, 30.0], 1.0), Some(Selected::Frame(0)));
        edits.add_ellipse(FrameAnnotation {
            filled: false,
            width: 3,
            area,
            color: 0xff0000,
        });
        assert_eq!(edits.pick([30.0, 30.0], 1.0), None);
        assert_eq!(edits.pick([30.0, 10.0], 1.0), Some(Selected::Ellipse(0)));
        edits.add_arrow(ArrowAnnotation {
            width: 3,
            start: [10, 30],
            end: [49, 30],
            color: 0xff0000,
        });
        assert_eq!(edits.pick([30.0, 30.0], 1.0), Some(Selected::Arrow(0)));
        edits.add(area);
        edits.add(area);
        assert_eq!(edits.pick([30.0, 30.0], 1.0), Some(Selected::Redaction(1)));
        assert_eq!(edits.pick([0.0, 0.0], 1.0), None);
    }
    #[test]
    #[ignore = "short optimized command timing; no desktop or clipboard"]
    fn keyboard_and_color_metadata_cost() {
        for command in ["nudge", "recolor", "width", "fill"] {
            let mut edits = RectEdits::default();
            for _ in 0..100 {
                edits.add_frame(FrameAnnotation {
                    filled: false,
                    width: 3,
                    area: PixelRegion {
                        x: 10,
                        y: 10,
                        width: 30,
                        height: 30,
                    },
                    color: 0xef4444,
                });
            }
            let selected = Selected::Frame(99);
            let mut samples = Vec::with_capacity(2000);
            for index in 0..2000 {
                let start = std::time::Instant::now();
                if command == "recolor" {
                    edits.set_color(selected, if index % 2 == 0 { 0x22c55e } else { 0xef4444 });
                } else if command == "width" {
                    edits.set_stroke_width(selected, if index % 2 == 0 { 12 } else { 3 });
                } else if command == "fill" {
                    edits.set_filled(selected, index % 2 == 0);
                } else {
                    edits.transform_selected(
                        selected,
                        None,
                        nudge_delta(if index % 2 == 0 { "right" } else { "left" }, false).unwrap(),
                        [3840, 2160],
                    );
                }
                std::hint::black_box(&edits);
                samples.push(start.elapsed().as_nanos());
            }
            assert_eq!(edits.past.len(), 100);
            samples.sort_unstable();
            eprintln!(
                "100 marks/100 undo states: command={command}, metadata command p95={} ns",
                samples[1900]
            );
        }
    }
    #[test]
    #[ignore = "short optimized metadata interaction timing; no desktop or clipboard"]
    fn selection_metadata_cost() {
        for count in [1, 100] {
            let mut edits = RectEdits::default();
            for _ in 0..count {
                edits.add(PixelRegion {
                    x: 10,
                    y: 10,
                    width: 30,
                    height: 30,
                });
            }
            for resize in [false, true] {
                let mut samples = Vec::with_capacity(2000);
                for _ in 0..2000 {
                    let start = std::time::Instant::now();
                    let selected = edits.pick(std::hint::black_box([20.0, 20.0]), 5.0).unwrap();
                    let mut preview = edits.snapshot();
                    let handle = resize.then(|| {
                        edits
                            .handle_at(selected, std::hint::black_box([40.0, 40.0]), 7.0)
                            .unwrap()
                    });
                    preview.transform(selected, handle, [2.0, 3.0], [3840, 2160]);
                    std::hint::black_box(preview.handles(selected));
                    std::hint::black_box(preview);
                    samples.push(start.elapsed().as_nanos());
                }
                samples.sort_unstable();
                eprintln!(
                    "selection {count} marks, resize={resize}: metadata pick/preview/handles p95={} ns; source pixel allocations=0",
                    samples[1900]
                );
            }
        }
    }
    #[test]
    fn ellipses_share_undo_and_annotation_admission() {
        let ellipse = FrameAnnotation {
            filled: false,
            width: 3,
            area: PixelRegion {
                x: 1,
                y: 1,
                width: 20,
                height: 12,
            },
            color: 0x22c55e,
        };
        let mut edits = RectEdits::default();
        edits.add_ellipse(ellipse);
        edits.undo();
        assert!(edits.ellipses.is_empty());
        edits.redo();
        assert_eq!(edits.ellipses, vec![ellipse]);
        for _ in 0..100 {
            edits.add_ellipse(ellipse);
        }
        assert_eq!(edits.ellipses.len(), 100);
        edits.add_frame(ellipse);
        assert!(edits.frames.is_empty());
        edits.reset();
        assert!(edits.ellipses.is_empty());
    }
    #[test]
    fn zoom_pan_bounds_and_source_coordinates_are_stable() {
        assert_eq!(
            clamp_pan([500.0, -500.0], [400.0, 200.0], [200.0, 100.0]),
            [100.0, -50.0]
        );
        assert_eq!(
            clamp_pan([50.0, -50.0], [100.0, 50.0], [200.0, 100.0]),
            [0.0, 0.0]
        );
        assert_eq!(
            clamp_pan([f32::NAN, 0.0], [400.0, 200.0], [200.0, 100.0]),
            [0.0, 0.0]
        );
        // Window positions are mapped back through the zoomed/panned image origin.
        let origin = [-200.0, -230.0];
        let start = [-160.0, -170.0];
        let end = [-80.0, -90.0];
        let region = PixelRegion::from_drag(
            [start[0] - origin[0], start[1] - origin[1]],
            [end[0] - origin[0], end[1] - origin[1]],
            [800.0, 400.0],
            [400, 200],
        )
        .unwrap();
        assert_eq!(
            region,
            PixelRegion {
                x: 20,
                y: 30,
                width: 40,
                height: 40
            }
        );
    }
    #[test]
    fn arrows_share_undo_and_combined_annotation_limit() {
        let arrow = ArrowAnnotation {
            width: 3,
            start: [4, 5],
            end: [40, 5],
            color: 0x22c55e,
        };
        let mut edits = RectEdits::default();
        edits.add_arrow(arrow);
        edits.undo();
        assert!(edits.arrows.is_empty());
        edits.redo();
        assert_eq!(edits.arrows, vec![arrow]);
        for _ in 0..101 {
            edits.add_arrow(arrow);
        }
        assert_eq!(edits.arrows.len(), 100);
        edits.add(PixelRegion {
            x: 1,
            y: 1,
            width: 2,
            height: 2,
        });
        assert!(edits.regions.is_empty());
        edits.reset();
        assert!(edits.arrows.is_empty());
    }
    #[test]
    fn frames_keep_color_through_undo_redo_and_share_annotation_limit() {
        let region = PixelRegion {
            x: 1,
            y: 2,
            width: 10,
            height: 8,
        };
        let frame = FrameAnnotation {
            filled: false,
            width: 3,
            area: region,
            color: 0xfacc15,
        };
        let mut edits = RectEdits::default();
        edits.add_frame(frame);
        edits.add(region);
        edits.undo();
        edits.undo();
        assert!(edits.frames.is_empty());
        edits.redo();
        assert_eq!(edits.frames, vec![frame]);
        edits.redo();
        assert_eq!(edits.regions, vec![region]);
        for _ in 0..100 {
            edits.add_frame(frame);
        }
        assert_eq!(edits.regions.len() + edits.frames.len(), 100);
        edits.reset();
        assert!(edits.frames.is_empty());
    }
    #[test]
    fn crop_and_redaction_share_bounded_undo_history() {
        let a = PixelRegion {
            x: 1,
            y: 1,
            width: 4,
            height: 3,
        };
        let b = PixelRegion {
            x: 2,
            width: 2,
            ..a
        };
        let mut edits = RectEdits::default();
        edits.set_crop(Some(a));
        edits.add(b);
        edits.set_crop(Some(b));
        edits.undo();
        assert_eq!(edits.crop, Some(a));
        assert_eq!(edits.regions, vec![b]);
        edits.undo();
        assert!(edits.regions.is_empty());
        assert_eq!(edits.crop, Some(a));
        edits.redo();
        assert_eq!(edits.regions, vec![b]);
        edits.set_crop(None);
        assert!(edits.undone.is_empty());
        for _ in 0..200 {
            edits.set_crop(Some(a));
            edits.set_crop(None);
        }
        assert_eq!(edits.past.len(), 100);
        edits.reset();
        assert!(edits.crop.is_none());
        assert!(edits.regions.is_empty());
    }
    #[test]
    fn redaction_undo_redo_branches_and_stays_bounded() {
        let a = PixelRegion {
            x: 1,
            y: 1,
            width: 2,
            height: 2,
        };
        let b = PixelRegion { x: 4, ..a };
        let c = PixelRegion { x: 8, ..a };
        let mut edits = RectEdits::default();
        edits.add(a);
        edits.add(b);
        edits.undo();
        assert_eq!(edits.regions, vec![a]);
        edits.redo();
        assert_eq!(edits.regions, vec![a, b]);
        edits.undo();
        edits.add(c);
        edits.redo();
        assert_eq!(edits.regions, vec![a, c]);
        assert!(edits.undone.is_empty());
        edits.reset();
        for _ in 0..101 {
            edits.add(a);
        }
        assert_eq!(edits.regions.len(), 100);
        for _ in 0..101 {
            edits.undo();
        }
        assert!(edits.regions.is_empty());
        assert_eq!(edits.undone.len(), 100);
        edits.reset();
        edits.redo();
        assert!(edits.regions.is_empty());
    }
}
