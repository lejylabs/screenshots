//! Programmatic bridge for existing immutable annotation objects. JSON cannot fabricate it.
use super::*;
use crate::{DrawAnnotation, PixelRegion};

struct Data {
    draw: DrawAnnotation,
    source: [u32; 2],
    bounds: Rect,
    _mask: Option<Arc<MaskData>>,
    pool: Weak<Pool>,
}
#[derive(Clone)]
pub struct LegacyAnnotation(Arc<Data>);
impl std::fmt::Debug for LegacyAnnotation {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("LegacyAnnotation")
            .field("bounds", &self.0.bounds)
            .finish_non_exhaustive()
    }
}
impl PartialEq for LegacyAnnotation {
    fn eq(&self, other: &Self) -> bool {
        Arc::ptr_eq(&self.0, &other.0)
    }
}
pub(super) fn region(area: PixelRegion, source: [u32; 2]) -> Result<()> {
    ensure!(
        area.width > 0
            && area.height > 0
            && area
                .x
                .checked_add(area.width)
                .is_some_and(|end| end <= source[0])
            && area
                .y
                .checked_add(area.height)
                .is_some_and(|end| end <= source[1]),
        "Legacy editor annotation is outside its source"
    );
    Ok(())
}
fn rect(area: PixelRegion) -> Rect {
    Rect::new(
        area.x as f64,
        area.y as f64,
        area.width as f64,
        area.height as f64,
    )
}
impl LegacyAnnotation {
    pub fn new(draw: DrawAnnotation, source: [u32; 2], assets: &EditorAssets) -> Result<Self> {
        crate::validate_image_size(source[0], source[1])?;
        let mut mask = None;
        let (bounds, margin) = match &draw {
            DrawAnnotation::Frame(frame) | DrawAnnotation::Ellipse(frame) => {
                region(frame.area, source)?;
                ensure!(
                    frame.color <= 0xffffff && (1..=12).contains(&frame.width),
                    "Invalid legacy editor shape"
                );
                (rect(frame.area), 1.0)
            }
            DrawAnnotation::Arrow(arrow) => {
                ensure!(
                    arrow.color <= 0xffffff
                        && (1..=12).contains(&arrow.width)
                        && arrow.start != arrow.end
                        && [arrow.start, arrow.end]
                            .iter()
                            .all(|point| point[0] < source[0] && point[1] < source[1]),
                    "Invalid legacy editor arrow"
                );
                let vertices = arrow.vertices();
                let mut min = [f64::INFINITY; 2];
                let mut max = [f64::NEG_INFINITY; 2];
                for point in vertices {
                    for axis in 0..2 {
                        min[axis] = min[axis].min(point[axis] as f64);
                        max[axis] = max[axis].max(point[axis] as f64);
                    }
                }
                (
                    Rect::new(
                        min[0],
                        min[1],
                        (max[0] - min[0]).max(1.0),
                        (max[1] - min[1]).max(1.0),
                    ),
                    1.0,
                )
            }
            DrawAnnotation::Pen(pen) => {
                pen.validate()?;
                region(pen.area, source)?;
                (rect(pen.area), f64::from(pen.width) + 1.0)
            }
            DrawAnnotation::Text(text) => {
                text.validate()?;
                region(text.area, source)?;
                mask = Some(assets.pool.mask(text.mask.clone())?);
                (rect(text.area), 1.0)
            }
            DrawAnnotation::Highlight(highlight) => {
                region(highlight.area, source)?;
                ensure!(
                    highlight.color <= 0xffffff && highlight.opacity > 0,
                    "Invalid legacy editor highlight"
                );
                (
                    if highlight.outside {
                        Rect::new(0.0, 0.0, source[0] as f64, source[1] as f64)
                    } else {
                        rect(highlight.area)
                    },
                    0.0,
                )
            }
        };
        let left = (bounds.x - margin).floor();
        let top = (bounds.y - margin).floor();
        let right = (bounds.x + bounds.width + margin).ceil();
        let bottom = (bounds.y + bounds.height + margin).ceil();
        let bounds = Rect::new(left, top, right - left, bottom - top);
        bounds.validate()?;
        Ok(Self(Arc::new(Data {
            draw,
            source,
            bounds,
            _mask: mask,
            pool: Arc::downgrade(&assets.pool),
        })))
    }
    pub fn bounds(&self) -> Rect {
        self.0.bounds
    }
    pub fn source_size(&self) -> [u32; 2] {
        self.0.source
    }
    pub fn pen_points(&self) -> usize {
        if let DrawAnnotation::Pen(pen) = &self.0.draw {
            pen.points.len()
        } else {
            0
        }
    }
    pub fn metadata_bytes(&self) -> usize {
        std::mem::size_of::<Data>() + self.pen_points() * 8
    }
    pub(crate) fn draw(&self) -> &DrawAnnotation {
        &self.0.draw
    }
    pub(crate) fn belongs_to(&self, assets: &EditorAssets) -> bool {
        Weak::ptr_eq(&self.0.pool, &Arc::downgrade(&assets.pool))
    }
    pub(crate) fn stamp_size(&self) -> Result<[u32; 2]> {
        let bounds = self.bounds();
        let size = [bounds.width.ceil() as u32, bounds.height.ceil() as u32];
        crate::validate_image_size(size[0], size[1])?;
        Ok(size)
    }
    pub(crate) fn translated_draw(&self) -> Result<DrawAnnotation> {
        let bounds = self.bounds();
        let dx = -(bounds.x as i64);
        let dy = -(bounds.y as i64);
        let point = |value: [u32; 2]| -> Result<[u32; 2]> {
            Ok([
                u32::try_from(i64::from(value[0]) + dx)?,
                u32::try_from(i64::from(value[1]) + dy)?,
            ])
        };
        let area = |value: PixelRegion| -> Result<PixelRegion> {
            let [x, y] = point([value.x, value.y])?;
            Ok(PixelRegion { x, y, ..value })
        };
        Ok(match self.0.draw.clone() {
            DrawAnnotation::Frame(mut value) => {
                value.area = area(value.area)?;
                DrawAnnotation::Frame(value)
            }
            DrawAnnotation::Ellipse(mut value) => {
                value.area = area(value.area)?;
                DrawAnnotation::Ellipse(value)
            }
            DrawAnnotation::Arrow(mut value) => {
                value.start = point(value.start)?;
                value.end = point(value.end)?;
                DrawAnnotation::Arrow(value)
            }
            DrawAnnotation::Pen(mut value) => {
                value.area = area(value.area)?;
                DrawAnnotation::Pen(value)
            }
            DrawAnnotation::Text(mut value) => {
                value.area = area(value.area)?;
                DrawAnnotation::Text(value)
            }
            DrawAnnotation::Highlight(mut value) => {
                value.area = area(value.area)?;
                DrawAnnotation::Highlight(value)
            }
        })
    }
}
