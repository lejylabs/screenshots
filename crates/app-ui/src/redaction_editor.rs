use crate::{CloseOverlay, SpikeShell, capture_button};
use app_core::{ArrowAnnotation, CapturedImage, FrameAnnotation, PixelRegion};
use gpui::{
    App, Context, FocusHandle, IntoElement, KeyBinding, Render, Window, actions, div, img,
    prelude::*, px, rgb, rgba,
};
use std::sync::Arc;

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

actions!(redaction_editor, [UndoRedaction, RedoRedaction]);

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
    ]);
}

#[derive(Clone, Default)]
struct EditState {
    regions: Vec<PixelRegion>,
    frames: Vec<FrameAnnotation>,
    arrows: Vec<ArrowAnnotation>,
    ellipses: Vec<FrameAnnotation>,
    crop: Option<PixelRegion>,
}
#[derive(Default)]
struct RectEdits {
    regions: Vec<PixelRegion>,
    frames: Vec<FrameAnnotation>,
    arrows: Vec<ArrowAnnotation>,
    ellipses: Vec<FrameAnnotation>,
    crop: Option<PixelRegion>,
    past: Vec<EditState>,
    undone: Vec<EditState>,
}
impl RectEdits {
    fn snapshot(&self) -> EditState {
        EditState {
            regions: self.regions.clone(),
            frames: self.frames.clone(),
            arrows: self.arrows.clone(),
            ellipses: self.ellipses.clone(),
            crop: self.crop,
        }
    }
    fn checkpoint(&mut self) {
        if self.past.len() == 100 {
            self.past.remove(0);
        }
        self.past.push(self.snapshot());
        self.undone.clear();
    }
    fn restore(&mut self, state: EditState) {
        self.regions = state.regions;
        self.frames = state.frames;
        self.arrows = state.arrows;
        self.ellipses = state.ellipses;
        self.crop = state.crop;
    }
    fn add(&mut self, region: PixelRegion) {
        if self.regions.len() + self.frames.len() + self.arrows.len() + self.ellipses.len() < 100 {
            self.checkpoint();
            self.regions.push(region);
        }
    }
    fn add_frame(&mut self, frame: FrameAnnotation) {
        if self.regions.len() + self.frames.len() + self.arrows.len() + self.ellipses.len() < 100 {
            self.checkpoint();
            self.frames.push(frame);
        }
    }
    fn add_arrow(&mut self, arrow: ArrowAnnotation) {
        if self.regions.len() + self.frames.len() + self.arrows.len() + self.ellipses.len() < 100 {
            self.checkpoint();
            self.arrows.push(arrow);
        }
    }
    fn add_ellipse(&mut self, ellipse: FrameAnnotation) {
        if self.regions.len() + self.frames.len() + self.arrows.len() + self.ellipses.len() < 100 {
            self.checkpoint();
            self.ellipses.push(ellipse);
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
        self.crop = None;
        self.past.clear();
        self.undone.clear();
    }
}
#[derive(Clone, Copy, PartialEq, Eq)]
enum Tool {
    Redact,
    Crop,
    Frame,
    Arrow,
    Ellipse,
}

pub(super) struct RedactionEditor {
    pub source: CapturedImage,
    pub render_image: Arc<gpui::RenderImage>,
    focus: FocusHandle,
    owner: gpui::WeakEntity<SpikeShell>,
    direct_capture: bool,
    edits: RectEdits,
    drag: Option<[f32; 2]>,
    end: [f32; 2],
    layout: Option<[f32; 2]>,
    tool: Tool,
    frame_color: u32,
    zoom: Option<f32>,
    pan: [f32; 2],
}

impl RedactionEditor {
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
            edits: RectEdits::default(),
            drag: None,
            end: [0.0; 2],
            layout: None,
            tool: Tool::Redact,
            frame_color: 0xef4444,
            zoom: None,
            pan: [0.0; 2],
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
        let toolbar_height = if matches!(self.tool, Tool::Frame | Tool::Arrow | Tool::Ellipse) {
            220.0
        } else {
            172.0
        } + if layout[0] < 1100.0 { 44.0 } else { 0.0 };
        let available = [
            f32::from(viewport.width) - 32.0,
            f32::from(viewport.height) - toolbar_height - 16.0,
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
            toolbar_height + (available[1].max(1.0) - size[1]) / 2.0 + self.pan[1],
        ];
        let mut preview = self.edits.regions.clone();
        let mut frame_preview = self.edits.frames.clone();
        let mut arrow_preview = self.edits.arrows.clone();
        let mut ellipse_preview = self.edits.ellipses.clone();
        let mut crop_preview = self.edits.crop;
        if let Some(start) = self.drag {
            if self.tool == Tool::Arrow {
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
            .on_action(cx.listener(|this, _: &CloseOverlay, _, cx| this.cancel(cx)))
            .on_action(cx.listener(|this, _: &UndoRedaction, _, cx| {
                this.edits.undo();
                this.drag = None;
                cx.notify();
            }))
            .on_action(cx.listener(|this, _: &RedoRedaction, _, cx| {
                this.edits.redo();
                this.drag = None;
                cx.notify();
            }))
            .on_mouse_move(cx.listener(|this, event: &gpui::MouseMoveEvent, _, cx| {
                if this.drag.is_some() && !event.dragging() {
                    this.drag = None;
                    cx.notify();
                }
            }))
            .child(
                div()
                    .absolute()
                    .top_0()
                    .left_0()
                    .right_0()
                    .p_3()
                    .flex()
                    .flex_col()
                    .gap_2()
                    .child(if self.direct_capture {
                        "New capture · No original PNG saved · Save and copy exports your edits"
                    } else {
                        "Edit a copy · Existing original files and history entries are kept"
                    })
                    .child(div().text_sm().child(format!(
                        "{} · {} / 100 annotations · Export: {} × {}",
                        match self.tool {
                            Tool::Crop => "Drag to crop; shaded area is excluded",
                            Tool::Frame => "Drag to highlight with a 3-pixel frame",
                            Tool::Arrow => "Drag from arrow tail to tip",
                            Tool::Ellipse => "Drag to highlight with an ellipse",
                            Tool::Redact => "Drag to redact",
                        },
                        self.edits.regions.len()
                            + self.edits.frames.len()
                            + self.edits.arrows.len()
                            + self.edits.ellipses.len(),
                        crop_preview.map_or(dimensions.0, |area| area.width),
                        crop_preview.map_or(dimensions.1, |area| area.height)
                    )))
                    .child(
                        div()
                            .flex()
                            .flex_wrap()
                            .gap_2()
                            .child(capture_button(
                                "tool-redact",
                                if self.tool == Tool::Redact {
                                    "Redact ✓"
                                } else {
                                    "Redact"
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
                                    "Frame ✓"
                                } else {
                                    "Frame"
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
                                    "Arrow ✓"
                                } else {
                                    "Arrow"
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
                                    "Ellipse ✓"
                                } else {
                                    "Ellipse"
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
                                    "Crop ✓"
                                } else {
                                    "Crop"
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
                                    && self.edits.ellipses.is_empty()
                                    && self.edits.crop.is_none()
                                    && self.edits.undone.is_empty()
                                    && self.edits.past.is_empty(),
                                cx.listener(|this, _, _, cx| {
                                    this.edits.reset();
                                    this.drag = None;
                                    cx.notify();
                                }),
                            ))
                            .child(capture_button(
                                "redaction-export",
                                "Save and copy",
                                (self.edits.regions.is_empty()
                                    && self.edits.frames.is_empty()
                                    && self.edits.arrows.is_empty()
                                    && self.edits.ellipses.is_empty()
                                    && self.edits.crop.is_none())
                                    || self.drag.is_some(),
                                cx.listener(|this, _, _, cx| {
                                    let owner = this.owner.clone();
                                    let source = this.source.clone();
                                    let regions = this.edits.regions.clone();
                                    let crop = this.edits.crop;
                                    let frames = this.edits.frames.clone();
                                    let arrows = this.edits.arrows.clone();
                                    let ellipses = this.edits.ellipses.clone();
                                    cx.defer(move |cx| {
                                        owner
                                            .update(cx, |this, cx| {
                                                this.export_redaction(
                                                    source, regions, crop, frames, arrows,
                                                    ellipses, cx,
                                                )
                                            })
                                            .ok();
                                    });
                                }),
                            ))
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
                    .when(
                        matches!(self.tool, Tool::Frame | Tool::Arrow | Tool::Ellipse),
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
                                                if self.frame_color == color {
                                                    selected
                                                } else {
                                                    label
                                                },
                                                false,
                                                cx.listener(move |this, _, _, cx| {
                                                    this.frame_color = color;
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
            )
            .child(
                div()
                    .absolute()
                    .left(px(16.0))
                    .right(px(16.0))
                    .top(px(toolbar_height))
                    .bottom(px(16.0))
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
                            .left(px(origin[0] - 16.0))
                            .top(px(origin[1] - toolbar_height))
                            .w(px(size[0]))
                            .h(px(size[1]))
                            .overflow_hidden()
                            .on_mouse_down(
                                gpui::MouseButton::Left,
                                cx.listener(move |this, event: &gpui::MouseDownEvent, _, cx| {
                                    if this.tool == Tool::Crop
                                        || this.edits.regions.len()
                                            + this.edits.frames.len()
                                            + this.edits.arrows.len()
                                            + this.edits.ellipses.len()
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
                                    if this.drag.is_some() {
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
