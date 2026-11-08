use crate::{CloseOverlay, SpikeShell, capture_button};
use app_core::{CapturedImage, PixelRegion};
use gpui::{
    App, Context, FocusHandle, IntoElement, KeyBinding, Render, Window, actions, div, img,
    prelude::*, px, rgb, rgba,
};
use std::sync::Arc;

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
    crop: Option<PixelRegion>,
}
#[derive(Default)]
struct RectEdits {
    regions: Vec<PixelRegion>,
    crop: Option<PixelRegion>,
    past: Vec<EditState>,
    undone: Vec<EditState>,
}
impl RectEdits {
    fn snapshot(&self) -> EditState {
        EditState {
            regions: self.regions.clone(),
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
        self.crop = state.crop;
    }
    fn add(&mut self, region: PixelRegion) {
        if self.regions.len() < 100 {
            self.checkpoint();
            self.regions.push(region);
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
        self.crop = None;
        self.past.clear();
        self.undone.clear();
    }
}
pub(super) struct RedactionEditor {
    pub source: CapturedImage,
    pub render_image: Arc<gpui::RenderImage>,
    focus: FocusHandle,
    owner: gpui::WeakEntity<SpikeShell>,
    edits: RectEdits,
    drag: Option<[f32; 2]>,
    end: [f32; 2],
    layout: Option<[f32; 2]>,
    crop_tool: bool,
}

impl RedactionEditor {
    pub fn new(
        source: CapturedImage,
        render_image: Arc<gpui::RenderImage>,
        owner: gpui::WeakEntity<SpikeShell>,
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
            edits: RectEdits::default(),
            drag: None,
            end: [0.0; 2],
            layout: None,
            crop_tool: false,
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
        let available = [
            f32::from(viewport.width) - 32.0,
            f32::from(viewport.height) - 144.0,
        ];
        let dimensions = self.source.pixels.dimensions();
        let scale = (available[0].max(1.0) / dimensions.0 as f32)
            .min(available[1].max(1.0) / dimensions.1 as f32);
        let size = [dimensions.0 as f32 * scale, dimensions.1 as f32 * scale];
        let origin = [
            (f32::from(viewport.width) - size[0]) / 2.0,
            128.0 + (available[1].max(1.0) - size[1]) / 2.0,
        ];
        let mut preview = self.edits.regions.clone();
        let mut crop_preview = self.edits.crop;
        if let Some(start) = self.drag {
            if let Some(region) =
                PixelRegion::from_drag(start, self.end, size, [dimensions.0, dimensions.1])
            {
                if self.crop_tool {
                    crop_preview = Some(region);
                } else {
                    preview.push(region);
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
                    .child("Edit a copy · Existing original files and history entries are kept")
                    .child(div().text_sm().child(format!(
                        "{} · {} redactions · Export: {} × {}",
                        if self.crop_tool {
                            "Drag to crop; shaded area is excluded"
                        } else {
                            "Drag to redact"
                        },
                        self.edits.regions.len(),
                        crop_preview.map_or(dimensions.0, |area| area.width),
                        crop_preview.map_or(dimensions.1, |area| area.height)
                    )))
                    .child(
                        div()
                            .flex()
                            .gap_2()
                            .child(capture_button(
                                "tool-redact",
                                if self.crop_tool {
                                    "Redact"
                                } else {
                                    "Redact ✓"
                                },
                                false,
                                cx.listener(|this, _, _, cx| {
                                    this.crop_tool = false;
                                    this.drag = None;
                                    cx.notify();
                                }),
                            ))
                            .child(capture_button(
                                "tool-crop",
                                if self.crop_tool { "Crop ✓" } else { "Crop" },
                                false,
                                cx.listener(|this, _, _, cx| {
                                    this.crop_tool = true;
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
                                (self.edits.regions.is_empty() && self.edits.crop.is_none())
                                    || self.drag.is_some(),
                                cx.listener(|this, _, _, cx| {
                                    let owner = this.owner.clone();
                                    let source = this.source.clone();
                                    let regions = this.edits.regions.clone();
                                    let crop = this.edits.crop;
                                    cx.defer(move |cx| {
                                        owner
                                            .update(cx, |this, cx| {
                                                this.export_redaction(source, regions, crop, cx)
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
                    ),
            )
            .child(
                div()
                    .absolute()
                    .left(px(origin[0]))
                    .top(px(origin[1]))
                    .w(px(size[0]))
                    .h(px(size[1]))
                    .overflow_hidden()
                    .on_mouse_down(
                        gpui::MouseButton::Left,
                        cx.listener(move |this, event: &gpui::MouseDownEvent, _, cx| {
                            if this.crop_tool || this.edits.regions.len() < 100 {
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
                    .on_mouse_move(
                        cx.listener(move |this, event: &gpui::MouseMoveEvent, _, cx| {
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
                        }),
                    )
                    .on_mouse_up(
                        gpui::MouseButton::Left,
                        cx.listener(move |this, event: &gpui::MouseUpEvent, _, cx| {
                            if let Some(start) = this.drag.take() {
                                let end = [
                                    f32::from(event.position.x) - origin[0],
                                    f32::from(event.position.y) - origin[1],
                                ];
                                if let Some(region) = PixelRegion::from_drag(
                                    start,
                                    end,
                                    size,
                                    [dimensions.0, dimensions.1],
                                ) {
                                    if this.crop_tool {
                                        this.edits.set_crop(Some(region));
                                    } else {
                                        this.edits.add(region);
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
            )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
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
