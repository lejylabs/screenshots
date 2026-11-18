//! Metadata-only fences for a frozen native selection; no polling or pixel copies.
use gpui_kit::{Bounds, DisplayChanges, DisplayEvent, DisplayId, Pixels, Size, Window};

pub(super) fn invalidates_selection(event: DisplayEvent) -> bool {
    match event {
        DisplayEvent::Changed { changes, .. } => changes != DisplayChanges::REFRESH_INTERVAL,
        _ => true,
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) struct SelectionGeometry {
    display: Option<DisplayId>,
    bounds: Bounds<Pixels>,
    viewport: Size<Pixels>,
    scale: f32,
}
impl SelectionGeometry {
    pub(super) fn current(window: &Window) -> Self {
        Self {
            display: window.display_id(),
            bounds: window.bounds(),
            viewport: window.viewport_size(),
            scale: window.scale_factor(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use gpui_kit::{point, px, size};

    #[test]
    fn topology_invalidates_added_removed_bounds_and_unknown_changes_but_not_refresh_alone() {
        let id = DisplayId::from(7);
        assert!(invalidates_selection(DisplayEvent::Added(id)));
        assert!(invalidates_selection(DisplayEvent::Removed(id)));
        for changes in [DisplayChanges::BOUNDS, DisplayChanges::empty(), DisplayChanges::all()] {
            assert!(invalidates_selection(DisplayEvent::Changed { id, changes }));
        }
        assert!(!invalidates_selection(DisplayEvent::Changed {
            id,
            changes: DisplayChanges::REFRESH_INTERVAL,
        }));
    }

    #[test]
    fn selection_requires_original_display_origin_viewport_extent_and_scale() {
        let original = SelectionGeometry {
            display: Some(DisplayId::from(7)),
            bounds: Bounds::new(point(px(-1600.), px(20.)), size(px(1600.), px(900.))),
            viewport: size(px(1600.), px(900.)),
            scale: 1.25,
        };
        let mut changed = original;
        assert_eq!(original, changed);
        changed.viewport.width = px(1599.);
        assert_ne!(original, changed);
        changed = original;
        changed.bounds.origin.x = px(0.);
        assert_ne!(original, changed);
        changed = original;
        changed.scale = 1.5;
        assert_ne!(original, changed);
        changed = original;
        changed.display = Some(DisplayId::from(8));
        assert_ne!(original, changed);
        changed.display = None;
        assert_ne!(original, changed);
    }
}
