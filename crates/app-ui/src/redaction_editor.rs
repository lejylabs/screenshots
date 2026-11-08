use crate::{CloseOverlay, SpikeShell, capture_button};
use app_core::{ArrowAnnotation, CapturedImage, FrameAnnotation, PixelRegion};
use gpui::{
    App, Context, FocusHandle, IntoElement, KeyBinding, Render, Window, actions, div, img,
    prelude::*, px, rgb, rgba,
};
use std::sync::Arc;

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
    [UndoRedaction, RedoRedaction, DeleteAnnotation]
);

pub(super) fn bind_keys(cx: &mut App) {
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
    ]);
}

#[derive(Clone, Default)]
struct EditState {
    regions: Vec<PixelRegion>,
    frames: Vec<FrameAnnotation>,
    arrows: Vec<ArrowAnnotation>,
    ellipses: Vec<FrameAnnotation>,
    pixelations: Vec<PixelRegion>,
    crop: Option<PixelRegion>,
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
}
impl EditState {
    fn color(&self, selected: Selected) -> Option<u32> {
        match selected {
            Selected::Frame(i) => self.frames.get(i).map(|mark| mark.color),
            Selected::Arrow(i) => self.arrows.get(i).map(|mark| mark.color),
            Selected::Ellipse(i) => self.ellipses.get(i).map(|mark| mark.color),
            Selected::Redaction(_) | Selected::Pixelation(_) => None,
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
                (i64::from(point[axis]) + delta[axis].round() as i64)
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
            Selected::Frame(i) => self.frames[i].area = next,
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
            Selected::Frame(i) => self.frames.get(i).map(|item| item.area),
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
            .chain((0..self.arrows.len()).rev().map(Selected::Arrow))
            .chain((0..self.ellipses.len()).rev().map(Selected::Ellipse))
            .chain((0..self.frames.len()).rev().map(Selected::Frame))
            .chain((0..self.pixelations.len()).rev().map(Selected::Pixelation));
        candidates.into_iter().find(|selected| {
            let area = self.bounds(*selected).unwrap();
            let x = point[0] - area.x as f32;
            let y = point[1] - area.y as f32;
            let w = area.width as f32;
            let h = area.height as f32;
            if x < -tolerance || y < -tolerance || x > w + tolerance || y > h + tolerance {
                return false;
            }
            match *selected {
                Selected::Redaction(_) | Selected::Pixelation(_) => {
                    x >= 0.0 && y >= 0.0 && x < w && y < h
                }
                Selected::Frame(_) => x.min(w - x).min(y.min(h - y)) <= 3.0 + tolerance,
                Selected::Ellipse(_) => {
                    let rx = w / 2.0;
                    let ry = h / 2.0;
                    let outer = ((x - rx) / (rx + tolerance)).powi(2)
                        + ((y - ry) / (ry + tolerance)).powi(2);
                    let inner = if rx > 3.0 + tolerance && ry > 3.0 + tolerance {
                        ((x - rx) / (rx - 3.0 - tolerance)).powi(2)
                            + ((y - ry) / (ry - 3.0 - tolerance)).powi(2)
                    } else {
                        2.0
                    };
                    outer <= 1.0 && inner >= 1.0
                }
                Selected::Arrow(i) => {
                    let item = self.arrows[i];
                    let a = item.start.map(|v| v as f32 + 0.5);
                    let b = item.end.map(|v| v as f32 + 0.5);
                    let d = [b[0] - a[0], b[1] - a[1]];
                    let t = (((point[0] - a[0]) * d[0] + (point[1] - a[1]) * d[1])
                        / (d[0] * d[0] + d[1] * d[1]).max(1.0))
                    .clamp(0.0, 1.0);
                    (point[0] - a[0] - t * d[0]).hypot(point[1] - a[1] - t * d[1])
                        <= 7.0 + tolerance
                }
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
                    Selected::Frame(i) => &mut self.frames[i].area,
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
            Selected::Arrow(i) => self.arrows[i].color = color,
            Selected::Ellipse(i) => self.ellipses[i].color = color,
            Selected::Redaction(_) | Selected::Pixelation(_) => unreachable!(),
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
        match selected {
            Selected::Redaction(i) => {
                self.regions.remove(i);
            }
            Selected::Pixelation(i) => {
                self.pixelations.remove(i);
            }
            Selected::Frame(i) => {
                self.frames.remove(i);
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
            + self.arrows.len()
            + self.ellipses.len()
            + self.pixelations.len()
            < 100
        {
            self.checkpoint();
            self.regions.push(region);
        }
    }
    fn add_frame(&mut self, frame: FrameAnnotation) {
        if self.regions.len()
            + self.frames.len()
            + self.arrows.len()
            + self.ellipses.len()
            + self.pixelations.len()
            < 100
        {
            self.checkpoint();
            self.frames.push(frame);
        }
    }
    fn add_arrow(&mut self, arrow: ArrowAnnotation) {
        if self.regions.len()
            + self.frames.len()
            + self.arrows.len()
            + self.ellipses.len()
            + self.pixelations.len()
            < 100
        {
            self.checkpoint();
            self.arrows.push(arrow);
        }
    }
    fn add_ellipse(&mut self, ellipse: FrameAnnotation) {
        if self.regions.len()
            + self.frames.len()
            + self.arrows.len()
            + self.ellipses.len()
            + self.pixelations.len()
            < 100
        {
            self.checkpoint();
            self.ellipses.push(ellipse);
        }
    }
    fn add_pixelation(&mut self, area: PixelRegion) {
        if self.regions.len()
            + self.frames.len()
            + self.arrows.len()
            + self.ellipses.len()
            + self.pixelations.len()
            < 100
        {
            self.checkpoint();
            self.pixelations.push(area);
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
        self.arrows.clear();
        self.ellipses.clear();
        self.pixelations.clear();
        self.crop = None;
        self.past.clear();
        self.undone.clear();
    }
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Tool {
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
            "v" => Some(Self::Select),
            "x" => Some(Self::Redact),
            "c" => Some(Self::Crop),
            "r" => Some(Self::Frame),
            "a" => Some(Self::Arrow),
            "e" => Some(Self::Ellipse),
            "p" => Some(Self::Pixelate),
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
    zoom: Option<f32>,
    pan: [f32; 2],
    pixelation: Option<(Arc<app_core::PixelationGrid>, Arc<gpui::RenderImage>)>,
    pixelation_cancel: Arc<std::sync::atomic::AtomicBool>,
    pixelation_loading: bool,
    pixelation_error: Option<String>,
}
impl Drop for RedactionEditor {
    fn drop(&mut self) {
        self.pixelation_cancel
            .store(true, std::sync::atomic::Ordering::Relaxed);
    }
}

impl RedactionEditor {
    #[cfg(test)]
    pub(super) fn pixelation_probe(&mut self, cx: &mut Context<Self>) -> Result<bool, String> {
        if let Some(error) = &self.pixelation_error {
            return Err(error.clone());
        }
        if self.pixelation.is_none() && !self.pixelation_loading {
            self.tool = Tool::Pixelate;
            self.edits.add_pixelation(PixelRegion {
                x: 0,
                y: 0,
                width: self.source.width.min(128),
                height: self.source.height.min(128),
            });
            self.prepare_pixelation(cx);
        }
        Ok(self.pixelation.is_some())
    }
    pub(super) fn release_images(&self, window: &mut Window) {
        self.pixelation_cancel
            .store(true, std::sync::atomic::Ordering::Relaxed);
        window.drop_image(self.render_image.clone()).ok();
        if let Some((_, image)) = &self.pixelation {
            window.drop_image(image.clone()).ok();
        }
    }
    fn prepare_pixelation(&mut self, cx: &mut Context<Self>) {
        if self.pixelation.is_some() || self.pixelation_loading {
            return;
        }
        self.pixelation_loading = true;
        self.pixelation_error = None;
        let source = self.source.pixels.clone();
        let cancelled = self.pixelation_cancel.clone();
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
    fn delete_selection(&mut self, cx: &mut Context<Self>) {
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
            || (self.edits.regions.is_empty()
                && self.edits.frames.is_empty()
                && self.edits.arrows.is_empty()
                && self.edits.ellipses.is_empty()
                && self.edits.pixelations.is_empty()
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
        let frames = self.edits.frames.clone();
        let arrows = self.edits.arrows.clone();
        let ellipses = self.edits.ellipses.clone();
        let pixelations = self.edits.pixelations.clone();
        let pixelation = self.pixelation.as_ref().map(|(grid, _)| grid.clone());
        cx.defer(move |cx| {
            owner
                .update(cx, |this, cx| {
                    this.export_redaction(
                        source,
                        regions,
                        crop,
                        frames,
                        arrows,
                        ellipses,
                        pixelations,
                        pixelation,
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
            selected: None,
            resize_handle: None,
            zoom: None,
            pan: [0.0; 2],
            pixelation: None,
            pixelation_cancel: Arc::new(std::sync::atomic::AtomicBool::new(false)),
            pixelation_loading: false,
            pixelation_error: None,
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
            Tool::Select | Tool::Frame | Tool::Arrow | Tool::Ellipse
        );
        let palette_color = if self.tool == Tool::Select {
            self.selected.and_then(|mark| self.edits.color(mark))
        } else {
            Some(self.frame_color)
        };
        let toolbar_height =
            if show_palette { 220.0 } else { 172.0 } + if layout[0] < 1400.0 { 44.0 } else { 0.0 };
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
        let mut frame_preview = displayed.frames;
        let mut arrow_preview = displayed.arrows;
        let mut ellipse_preview = displayed.ellipses;
        let mut pixelation_preview = displayed.pixelations;
        let pixelation_render = self.pixelation.as_ref().map(|(_, render)| render.clone());
        let mut crop_preview = self.edits.crop;
        if let Some(start) = self.drag {
            if self.tool == Tool::Select {
                // Transforms are previewed in metadata above and committed on release.
            } else if self.tool == Tool::Arrow {
                if let Some(arrow) = ArrowAnnotation::from_drag(
                    start,
                    self.end,
                    size,
                    [dimensions.0, dimensions.1],
                    self.frame_color,
                ) {
                    arrow_preview.push(arrow);
                }
            } else if let Some(region) =
                PixelRegion::from_drag(start, self.end, size, [dimensions.0, dimensions.1])
            {
                if self.tool == Tool::Crop {
                    crop_preview = Some(region);
                } else if self.tool == Tool::Pixelate {
                    if self.pixelation.is_some() {
                        pixelation_preview.push(region);
                    }
                } else if self.tool == Tool::Frame {
                    frame_preview.push(FrameAnnotation {
                        area: region,
                        color: self.frame_color,
                    });
                } else {
                    if self.tool == Tool::Ellipse {
                        ellipse_preview.push(FrameAnnotation {
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
                let modifiers = event.keystroke.modifiers;
                if modifiers.control || modifiers.alt || modifiers.platform || modifiers.function || event.prefer_character_input { return; }
                let key = event.keystroke.key.as_str();
                if !modifiers.shift {
                    if let Some(tool) = Tool::shortcut(key) {
                        this.tool = tool;
                        if tool == Tool::Pixelate { this.prepare_pixelation(cx); }
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
            .on_action(cx.listener(|this, _: &CloseOverlay, _, cx| this.cancel(cx)))
            .on_action(cx.listener(|this, _: &DeleteAnnotation, _, cx| this.delete_selection(cx)))
            .on_action(cx.listener(|this, _: &UndoRedaction, _, cx| {
                this.edits.undo();
                this.selected = None;
                this.drag = None;
                cx.notify();
            }))
            .on_action(cx.listener(|this, _: &RedoRedaction, _, cx| {
                this.edits.redo();
                this.selected = None;
                this.drag = None;
                cx.notify();
            }))
            .on_mouse_move(cx.listener(|this, event: &gpui::MouseMoveEvent, _, cx| {
                if this.drag.is_some() && !event.dragging() {
                    this.drag = None;
                    cx.notify();
                }
            }))
            .on_mouse_move(cx.listener(move |this, event: &gpui::MouseMoveEvent, _, cx| {
                if this.tool == Tool::Select && this.drag.is_some() && event.dragging() {
                    this.end = [f32::from(event.position.x)-origin[0],f32::from(event.position.y)-origin[1]];
                    cx.notify();
                }
            }))
            .on_mouse_up(gpui::MouseButton::Left, cx.listener(move |this, event: &gpui::MouseUpEvent, _, cx| {
                this.finish_selection_drag([f32::from(event.position.x)-origin[0],f32::from(event.position.y)-origin[1]],scale,[dimensions.0,dimensions.1],cx);
            }))
            .child(gpui::deferred(
                div()
                    .absolute()
                    .when(!self.toolbar_bottom, |view| view.top_0())
                    .when(self.toolbar_bottom, |view| view.bottom_0())
                    .bg(rgb(0x111318))
                    .occlude()
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
                            Tool::Pixelate => if self.pixelation_loading { "Preparing pixelation preview..." } else if self.pixelation_error.is_some() { "Pixelation failed. Select Pixelate to retry." } else { "Drag to pixelate source pixels; use Redact for sensitive information" },
                            Tool::Select =>
                                "Drag a mark to move; handles resize; arrow keys move 1px (Shift: 10px); Delete removes it",
                            Tool::Crop => "Drag to crop; shaded area is excluded",
                            Tool::Frame => "Drag to highlight with a 3-pixel frame",
                            Tool::Arrow => "Drag from arrow tail to tip",
                            Tool::Ellipse => "Drag to highlight with an ellipse",
                            Tool::Redact => "Drag to redact",
                        },
                        self.edits.regions.len()
                            + self.edits.frames.len()
                            + self.edits.arrows.len()
                            + self.edits.ellipses.len() + self.edits.pixelations.len(),
                        crop_preview.map_or(dimensions.0, |area| area.width),
                        crop_preview.map_or(dimensions.1, |area| area.height)
                    )))
                    .child(
                        div()
                            .flex()
                            .flex_wrap()
                            .gap_2()
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
                                self.edits.past.is_empty(),
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
                                self.edits.undone.is_empty(),
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
                                self.edits.regions.is_empty()
                                    && self.edits.frames.is_empty()
                                    && self.edits.arrows.is_empty()
                                    && self.edits.ellipses.is_empty() && self.edits.pixelations.is_empty()
                                    && self.edits.crop.is_none()
                                    && self.edits.undone.is_empty()
                                    && self.edits.past.is_empty(),
                                cx.listener(|this, _, _, cx| {
                                    this.edits.reset();
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
                                            && self.edits.frames.is_empty()
                                            && self.edits.arrows.is_empty()
                                            && self.edits.ellipses.is_empty() && self.edits.pixelations.is_empty()
                                            && self.edits.crop.is_none()
                                            && !self.direct_capture)
                                            || self.drag.is_some(),
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
                    .when(
                        show_palette,
                        |toolbar| {
                            toolbar.child(
                                div().flex().gap_2().children(
                                    [
                                        ("frame-red", "Red", "Red ✓", 0xef4444),
                                        ("frame-yellow", "Yellow", "Yellow ✓", 0xfacc15),
                                        ("frame-green", "Green", "Green ✓", 0x22c55e),
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
                                                palette_color.is_none(),
                                                cx.listener(move |this, _, _, cx| {
                                                    this.frame_color = color;
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
                                ),
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
                                cx.listener(move |this, event: &gpui::MouseDownEvent, _, cx| {
                                    if this.tool == Tool::Pixelate && this.pixelation.is_none() { return; }
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
                                            + this.edits.frames.len()
                                            + this.edits.arrows.len()
                                            + this.edits.ellipses.len() + this.edits.pixelations.len()
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
                                    if this.tool != Tool::Select && this.drag.is_some() {
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
                                    if this.tool == Tool::Select { return; }
                                    if let Some(start) = this.drag.take() {
                                        let end = [
                                            f32::from(event.position.x) - origin[0],
                                            f32::from(event.position.y) - origin[1],
                                        ];
                                        if this.tool == Tool::Arrow {
                                            if let Some(arrow) = ArrowAnnotation::from_drag(
                                                start,
                                                end,
                                                size,
                                                [dimensions.0, dimensions.1],
                                                this.frame_color,
                                            ) {
                                                this.edits.add_arrow(arrow);
                                            }
                                        } else if let Some(region) = PixelRegion::from_drag(
                                            start,
                                            end,
                                            size,
                                            [dimensions.0, dimensions.1],
                                        ) {
                                            if this.tool == Tool::Crop {
                                                this.edits.set_crop(Some(region));
                                            } else if this.tool == Tool::Pixelate {
                                                if this.pixelation.is_some() { this.edits.add_pixelation(region); }
                                            } else if this.tool == Tool::Frame {
                                                this.edits.add_frame(FrameAnnotation {
                                                    area: region,
                                                    color: this.frame_color,
                                                });
                                            } else {
                                                if this.tool == Tool::Ellipse {
                                                    this.edits.add_ellipse(FrameAnnotation {
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
                                    if let Some(render) = &pixelation_render {
                                        for area in &pixelation_preview {
                                            let clip = gpui::Bounds { origin: bounds.origin + gpui::point(px(area.x as f32*scale),px(area.y as f32*scale)), size:gpui::size(px(area.width as f32*scale),px(area.height as f32*scale)) };
                                            window.paint_quad(gpui::fill(clip,rgb(0x111318)));
                                            window.with_content_mask(Some(gpui::ContentMask { bounds:clip }), |window| {
                                                window.paint_image(bounds,bounds,Default::default(),render.clone(),0,false).ok();
                                            });
                                        }
                                    }
                                }).absolute().top_0().left_0().size_full()
                            )
                            .children(
                                frame_preview
                                    .into_iter()
                                    .flat_map(|frame| {
                                        frame.segments().map(|segment| (segment, frame.color))
                                    })
                                    .map(move |(region, color)| {
                                        div()
                                            .absolute()
                                            .left(px(region.x as f32 * scale))
                                            .top(px(region.y as f32 * scale))
                                            .w(px(region.width as f32 * scale))
                                            .h(px(region.height as f32 * scale))
                                            .bg(rgb(color))
                                    }),
                            )
                            .child(
                                gpui::canvas(
                                    |_, _, _| (),
                                    move |bounds, _, window, _| {
                                        for ellipse in &ellipse_preview {
                                            let mut builder = gpui::PathBuilder::fill();
                                            for contour in ellipse.ellipse_contours() {
                                                let map = |point: [f32; 2]| {
                                                    gpui::point(
                                                        bounds.origin.x + px(point[0] * scale),
                                                        bounds.origin.y + px(point[1] * scale),
                                                    )
                                                };
                                                builder.move_to(map(contour[0]));
                                                for point in &contour[1..] {
                                                    builder.line_to(map(*point));
                                                }
                                                builder.line_to(map(contour[0]));
                                            }
                                            if let Ok(path) = builder.build() {
                                                window.paint_path(path, rgb(ellipse.color));
                                            }
                                        }
                                        for arrow in &arrow_preview {
                                            let vertices = arrow.vertices();
                                            let map = |point: [f32; 2]| {
                                                gpui::point(
                                                    bounds.origin.x + px(point[0] * scale),
                                                    bounds.origin.y + px(point[1] * scale),
                                                )
                                            };
                                            let mut builder = gpui::PathBuilder::fill();
                                            builder.move_to(map(vertices[0]));
                                            for vertex in &vertices[1..] {
                                                builder.line_to(map(*vertex));
                                            }
                                            builder.line_to(map(vertices[0]));
                                            if let Ok(path) = builder.build() {
                                                window.paint_path(path, rgb(arrow.color));
                                            }
                                        }
                                    },
                                )
                                .absolute()
                                .top_0()
                                .left_0()
                                .size_full(),
                            )
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
            area,
            color: 0xef4444,
        };
        edits.add(area);
        edits.add_frame(mark);
        edits.add_ellipse(mark);
        edits.add_arrow(ArrowAnnotation {
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
            area,
            color: 0xff0000,
        };
        let mut edits = RectEdits::default();
        edits.add(area);
        edits.add_frame(shape);
        edits.add_ellipse(shape);
        edits.add_arrow(ArrowAnnotation {
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
            area,
            color: 0xff0000,
        });
        assert_eq!(edits.pick([30.0, 30.0], 1.0), None);
        assert_eq!(edits.pick([10.0, 30.0], 1.0), Some(Selected::Frame(0)));
        edits.add_ellipse(FrameAnnotation {
            area,
            color: 0xff0000,
        });
        assert_eq!(edits.pick([30.0, 30.0], 1.0), None);
        assert_eq!(edits.pick([30.0, 10.0], 1.0), Some(Selected::Ellipse(0)));
        edits.add_arrow(ArrowAnnotation {
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
        for recolor in [false, true] {
            let mut edits = RectEdits::default();
            for _ in 0..100 {
                edits.add_frame(FrameAnnotation {
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
                if recolor {
                    edits.set_color(selected, if index % 2 == 0 { 0x22c55e } else { 0xef4444 });
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
                "100 marks/100 undo states: recolor={recolor}, metadata command p95={} ns",
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
