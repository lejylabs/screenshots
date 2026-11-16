use super::*;
use crate::{PixelRegion, image_cancel::ImageWork};
use anyhow::{Context, Result, ensure};
use image::{Rgba, RgbaImage};

const VECTOR_SCRATCH: u64 = 4 * 1024 * 1024;
const KERNEL_SCRATCH: u64 = 3 * 1024 * 1024;
const OVERHEAD: u64 = 32 * 1024 * 1024;
const TILE_WIDTH: u32 = 256;
const TILE_HEIGHT: u32 = 32;

fn zeroed(length: usize, work: &mut ImageWork<'_, impl Fn() -> bool>) -> Result<Vec<u8>> {
    let mut bytes = Vec::with_capacity(length);
    let zeros = [0u8; 16384];
    while bytes.len() < length {
        let count = (length - bytes.len()).min(zeros.len());
        work.extend(&mut bytes, &zeros[..count])?;
    }
    Ok(bytes)
}

pub(super) fn output_size(spec: &DocumentSpec) -> Result<[u32; 2]> {
    let crop = spec.crop.unwrap_or(PixelRegion {
        x: 0,
        y: 0,
        width: spec.canvas[0],
        height: spec.canvas[1],
    });
    let [left, top, right, bottom] = spec.export.padding;
    let width = crop
        .width
        .checked_add(left)
        .and_then(|width| width.checked_add(right))
        .context("Editor output dimensions overflow")?;
    let height = crop
        .height
        .checked_add(top)
        .and_then(|height| height.checked_add(bottom))
        .context("Editor output dimensions overflow")?;
    crate::validate_image_size(width, height)?;
    Ok([width, height])
}
fn extent(size: [u32; 2]) -> u64 {
    u64::from(size[0]) * u64::from(size[1]) * 4
}
struct Budget {
    fixed: u64,
    limit: u64,
}
impl Budget {
    fn new(snapshot: &EditorSnapshot) -> Result<Self> {
        let source = snapshot.assets.raster_bytes();
        let output = extent(output_size(&snapshot.spec)?);
        let fixed =
            source + snapshot.assets.font_bytes() + snapshot.metadata_bytes as u64 + 256 * 1024;
        let limit = source
            .checked_mul(2)
            .and_then(|bytes| bytes.checked_add(output * 2))
            .and_then(|bytes| bytes.checked_add(OVERHEAD))
            .context("Editor work budget overflow")?;
        Ok(Self { fixed, limit })
    }
    fn admit(&self, bytes: u64) -> Result<()> {
        ensure!(
            self.fixed
                .checked_add(bytes)
                .is_some_and(|peak| peak <= self.limit),
            "Editor work extent exceeds 2*S + 2*E + 32 MiB; reduce canvas, crop or assets"
        );
        Ok(())
    }
}
fn clipped(rect: Rect, size: [u32; 2], margin: f64) -> Option<PixelRegion> {
    let left = (rect.x - margin).floor().clamp(0.0, size[0] as f64) as u32;
    let top = (rect.y - margin).floor().clamp(0.0, size[1] as f64) as u32;
    let right = (rect.x + rect.width + margin)
        .ceil()
        .clamp(0.0, size[0] as f64) as u32;
    let bottom = (rect.y + rect.height + margin)
        .ceil()
        .clamp(0.0, size[1] as f64) as u32;
    (right > left && bottom > top).then_some(PixelRegion {
        x: left,
        y: top,
        width: right - left,
        height: bottom - top,
    })
}
#[derive(Clone)]
struct PrivacyMask {
    polygon: [[f64; 2]; 4],
    extent: Rect,
}
impl PrivacyMask {
    fn new(layer: &Layer) -> Result<Self> {
        Ok(Self {
            polygon: layer
                .bounds
                .corners()
                .map(|point| layer.transform.point(point)),
            extent: layer.transform.bounds(layer.bounds)?,
        })
    }
    fn intersects(&self, x: u32, y: u32) -> bool {
        // Positive-area separating-axis test; touching an aligned boundary alone
        // does not paint the adjacent pixel. Every genuinely intersected pixel is opaque.
        let pixel = [
            [x as f64, y as f64],
            [x as f64 + 1.0, y as f64],
            [x as f64 + 1.0, y as f64 + 1.0],
            [x as f64, y as f64 + 1.0],
        ];
        let edge0 = [
            self.polygon[1][0] - self.polygon[0][0],
            self.polygon[1][1] - self.polygon[0][1],
        ];
        let edge1 = [
            self.polygon[3][0] - self.polygon[0][0],
            self.polygon[3][1] - self.polygon[0][1],
        ];
        for mut axis in [
            [1.0, 0.0],
            [0.0, 1.0],
            [-edge0[1], edge0[0]],
            [-edge1[1], edge1[0]],
        ] {
            let length = axis[0].hypot(axis[1]);
            axis[0] /= length;
            axis[1] /= length;
            let project = |points: &[[f64; 2]; 4]| {
                let mut min = f64::INFINITY;
                let mut max = f64::NEG_INFINITY;
                for point in points {
                    let value = point[0] * axis[0] + point[1] * axis[1];
                    min = min.min(value);
                    max = max.max(value);
                }
                (min, max)
            };
            let (a, b) = project(&self.polygon);
            let (c, d) = project(&pixel);
            if a >= d || c >= b {
                return false;
            }
        }
        true
    }
}
struct ProtectedSource<'a> {
    pixels: &'a RgbaImage,
    bits: Vec<u8>,
}
impl<'a> ProtectedSource<'a> {
    fn new(
        pixels: &'a RgbaImage,
        base: Affine,
        masks: &[PrivacyMask],
        work: &mut ImageWork<'_, impl Fn() -> bool>,
    ) -> Result<Self> {
        let mut result = Self {
            pixels,
            bits: Vec::new(),
        };
        if masks.is_empty() {
            return Ok(result);
        }
        let inverse = base.inverse()?;
        let count = u64::from(pixels.width()) * u64::from(pixels.height());
        result.bits = zeroed((count.div_ceil(8)) as usize, work)?;
        for mask in masks {
            let polygon = mask.polygon.map(|point| inverse.point(point));
            let bounds = Rect::from_points(polygon)?;
            let projected = PrivacyMask {
                polygon,
                extent: bounds,
            };
            let Some(area) = clipped(bounds, [pixels.width(), pixels.height()], 0.0) else {
                continue;
            };
            for y in area.y..area.y + area.height {
                for x in area.x..area.x + area.width {
                    work.pixel()?;
                    if !projected.intersects(x, y) {
                        continue;
                    }
                    let index = u64::from(y) * u64::from(pixels.width()) + u64::from(x);
                    result.bits[index as usize / 8] |= 1 << (index % 8);
                }
            }
        }
        Ok(result)
    }
    fn pixel(&self, x: i64, y: i64) -> [u8; 4] {
        if x < 0
            || y < 0
            || x >= i64::from(self.pixels.width())
            || y >= i64::from(self.pixels.height())
        {
            return [0; 4];
        }
        let index = y as u64 * u64::from(self.pixels.width()) + x as u64;
        if !self.bits.is_empty() && self.bits[index as usize / 8] & (1 << (index % 8)) != 0 {
            [0, 0, 0, 255]
        } else {
            self.pixels.get_pixel(x as u32, y as u32).0
        }
    }
    fn sample(&self, point: [f64; 2], interpolation: Interpolation) -> [u8; 4] {
        sample_frame(
            point,
            interpolation,
            [self.pixels.width(), self.pixels.height()],
            |x, y| self.pixel(x, y),
        )
    }
}
fn sample(
    point: [f64; 2],
    interpolation: Interpolation,
    pixel: impl Fn(i64, i64) -> [u8; 4],
) -> [u8; 4] {
    if matches!(interpolation, Interpolation::Nearest) {
        return pixel(point[0].floor() as i64, point[1].floor() as i64);
    }
    let x = point[0] - 0.5;
    let y = point[1] - 0.5;
    let left = x.floor() as i64;
    let top = y.floor() as i64;
    let fx = x - x.floor();
    let fy = y - y.floor();
    let mut sums = [0.0f64; 4];
    for (x, y, weight) in [
        (left, top, (1.0 - fx) * (1.0 - fy)),
        (left + 1, top, fx * (1.0 - fy)),
        (left, top + 1, (1.0 - fx) * fy),
        (left + 1, top + 1, fx * fy),
    ] {
        let value = pixel(x, y);
        let alpha = value[3] as f64;
        for channel in 0..3 {
            sums[channel] += value[channel] as f64 * alpha * weight;
        }
        sums[3] += alpha * weight;
    }
    if sums[3] <= 0.0 {
        return [0; 4];
    }
    [
        (sums[0] / sums[3]).round().clamp(0.0, 255.0) as u8,
        (sums[1] / sums[3]).round().clamp(0.0, 255.0) as u8,
        (sums[2] / sums[3]).round().clamp(0.0, 255.0) as u8,
        sums[3].round().clamp(0.0, 255.0) as u8,
    ]
}
fn stage_sample(stage: &RgbaImage, point: [f64; 2], interpolation: Interpolation) -> [u8; 4] {
    sample_frame(
        point,
        interpolation,
        [stage.width(), stage.height()],
        |x, y| stage.get_pixel(x as u32, y as u32).0,
    )
}
fn source_mask(
    source: &ProtectedSource<'_>,
    bytes: &mut [u8],
    work: &mut ImageWork<'_, impl Fn() -> bool>,
) -> Result<()> {
    for (index, mask) in source.bits.iter().copied().enumerate() {
        work.charge(1)?;
        let mut mask = mask;
        while mask != 0 {
            work.pixel()?;
            let bit = mask.trailing_zeros() as usize;
            let pixel = index * 8 + bit;
            bytes[pixel * 4..pixel * 4 + 4].copy_from_slice(&[0, 0, 0, 255]);
            mask &= mask - 1;
        }
    }
    Ok(())
}
fn base_image(
    source: &ProtectedSource<'_>,
    size: [u32; 2],
    inverse: Affine,
    interpolation: Interpolation,
    work: &mut ImageWork<'_, impl Fn() -> bool>,
) -> Result<RgbaImage> {
    if inverse == Affine::IDENTITY && size == [source.pixels.width(), source.pixels.height()] {
        let mut bytes = Vec::with_capacity(source.pixels.as_raw().len());
        work.extend(&mut bytes, source.pixels.as_raw())?;
        source_mask(source, &mut bytes, work)?;
        return RgbaImage::from_raw(size[0], size[1], bytes).context("Invalid editor base extent");
    }
    let mut output = rgba(size, work)?;
    for y in 0..size[1] {
        for x in 0..size[0] {
            work.pixel()?;
            output.put_pixel(
                x,
                y,
                Rgba(source.sample(
                    inverse.point([x as f64 + 0.5, y as f64 + 0.5]),
                    interpolation,
                )),
            );
        }
    }
    Ok(output)
}
fn sample_frame(
    point: [f64; 2],
    interpolation: Interpolation,
    size: [u32; 2],
    pixel: impl Fn(i64, i64) -> [u8; 4],
) -> [u8; 4] {
    if !point.iter().all(|value| value.is_finite())
        || point[0] < 0.0
        || point[1] < 0.0
        || point[0] >= size[0] as f64
        || point[1] >= size[1] as f64
    {
        return [0; 4];
    }
    sample(point, interpolation, |x, y| {
        pixel(
            x.clamp(0, i64::from(size[0]) - 1),
            y.clamp(0, i64::from(size[1]) - 1),
        )
    })
}
pub(super) fn blend(destination: &mut Rgba<u8>, source: [u8; 4], opacity: u8) {
    let alpha = (u32::from(source[3]) * u32::from(opacity) + 127) / 255;
    if alpha == 0 {
        return;
    }
    if alpha == 255 {
        *destination = Rgba([source[0], source[1], source[2], 255]);
        return;
    }
    let old = u32::from(destination[3]) * (255 - alpha);
    let denominator = alpha * 255 + old;
    for channel in 0..3 {
        destination[channel] = ((u32::from(source[channel]) * alpha * 255
            + u32::from(destination[channel]) * old
            + denominator / 2)
            / denominator) as u8;
    }
    destination[3] = ((denominator + 127) / 255) as u8;
}
fn protect(
    pixels: &mut RgbaImage,
    masks: &[PrivacyMask],
    work: &mut ImageWork<'_, impl Fn() -> bool>,
) -> Result<()> {
    for mask in masks {
        let Some(area) = clipped(mask.extent, [pixels.width(), pixels.height()], 0.0) else {
            continue;
        };
        for y in area.y..area.y + area.height {
            for x in area.x..area.x + area.width {
                work.pixel()?;
                if mask.intersects(x, y) {
                    pixels.put_pixel(x, y, Rgba([0, 0, 0, 255]));
                }
            }
        }
    }
    Ok(())
}

fn local_extent(layer: &Layer) -> Rect {
    let mut bounds = layer.bounds;
    let mut margin = 0.0;
    let shape = match &layer.content {
        LayerKind::Draw { shape, style } => {
            margin = style.width * 4.0;
            Some(shape)
        }
        LayerKind::Erase { shape, width, .. } => {
            margin = *width / 2.0;
            Some(shape)
        }
        LayerKind::Magnifier { style, .. } => {
            margin = style.width;
            None
        }
        _ => None,
    };
    let mut include = |point: [f64; 2]| {
        let right = (bounds.x + bounds.width).max(point[0]);
        let bottom = (bounds.y + bounds.height).max(point[1]);
        bounds.x = bounds.x.min(point[0]);
        bounds.y = bounds.y.min(point[1]);
        bounds.width = right - bounds.x;
        bounds.height = bottom - bounds.y;
    };
    if let Some(shape) = shape {
        match shape {
            Shape::SpeechBubble { tail, .. } => include(*tail),
            Shape::Arrow { start, end } => {
                include(*start);
                include(*end);
            }
            Shape::Path { points, .. } => {
                for point in points {
                    include(*point)
                }
            }
            _ => {}
        }
    }
    Rect::new(
        bounds.x - margin,
        bounds.y - margin,
        bounds.width + margin * 2.0,
        bounds.height + margin * 2.0,
    )
}
fn native_rect(bounds: Rect) -> Result<tiny_skia::Rect> {
    tiny_skia::Rect::from_xywh(
        bounds.x as f32,
        bounds.y as f32,
        bounds.width as f32,
        bounds.height as f32,
    )
    .context("Editor geometry cannot be rasterized")
}
fn bubble(bounds: Rect, tail: [f64; 2], corner: f64) -> Result<tiny_skia::Path> {
    let [left, top, right, bottom] = [
        bounds.x,
        bounds.y,
        bounds.x + bounds.width,
        bounds.y + bounds.height,
    ];
    let radius = corner.min(bounds.width.min(bounds.height) / 4.0);
    let distances = [
        (top - tail[1]).max(0.0),
        (tail[0] - right).max(0.0),
        (tail[1] - bottom).max(0.0),
        (left - tail[0]).max(0.0),
    ];
    let chosen = distances
        .iter()
        .enumerate()
        .max_by(|a, b| a.1.total_cmp(b.1))
        .filter(|(_, distance)| **distance > 0.0)
        .map(|(side, _)| side);
    let mut path = tiny_skia::PathBuilder::new();
    path.move_to((left + radius) as f32, top as f32);
    let mut edge = |side_index: usize, start: [f64; 2], end: [f64; 2]| {
        if chosen == Some(side_index) {
            let horizontal = side_index.is_multiple_of(2);
            let length = if horizontal {
                bounds.width
            } else {
                bounds.height
            };
            let half = (length / 8.0).min(12.0);
            let low = if horizontal { left } else { top };
            let high = if horizontal { right } else { bottom };
            let anchor = if horizontal { tail[0] } else { tail[1] }
                .clamp(low + radius + half, high - radius - half);
            let forward = side_index < 2;
            let first = anchor + if forward { -half } else { half };
            let last = anchor + if forward { half } else { -half };
            let coordinate = if horizontal { start[1] } else { start[0] };
            let first = if horizontal {
                [first, coordinate]
            } else {
                [coordinate, first]
            };
            let last = if horizontal {
                [last, coordinate]
            } else {
                [coordinate, last]
            };
            path.line_to(first[0] as f32, first[1] as f32);
            path.line_to(tail[0] as f32, tail[1] as f32);
            path.line_to(last[0] as f32, last[1] as f32);
        }
        path.line_to(end[0] as f32, end[1] as f32);
    };
    edge(0, [left + radius, top], [right - radius, top]);
    path.quad_to(
        right as f32,
        top as f32,
        right as f32,
        (top + radius) as f32,
    );
    // Recreate the short edge closures so each corner releases its mutable borrow.
    fn emit_edge(
        path: &mut tiny_skia::PathBuilder,
        chosen: Option<usize>,
        index: usize,
        tail: [f64; 2],
        bounds: Rect,
        radius: f64,
        end: [f64; 2],
    ) {
        if chosen == Some(index) {
            let horizontal = index.is_multiple_of(2);
            let low = if horizontal { bounds.x } else { bounds.y };
            let high = low
                + if horizontal {
                    bounds.width
                } else {
                    bounds.height
                };
            let half = ((high - low) / 8.0).min(12.0);
            let anchor = if horizontal { tail[0] } else { tail[1] }
                .clamp(low + radius + half, high - radius - half);
            let first = anchor + if index < 2 { -half } else { half };
            let last = anchor + if index < 2 { half } else { -half };
            let axis = if horizontal { end[1] } else { end[0] };
            for point in [
                if horizontal {
                    [first, axis]
                } else {
                    [axis, first]
                },
                tail,
                if horizontal {
                    [last, axis]
                } else {
                    [axis, last]
                },
            ] {
                path.line_to(point[0] as f32, point[1] as f32);
            }
        }
        path.line_to(end[0] as f32, end[1] as f32);
    }
    emit_edge(
        &mut path,
        chosen,
        1,
        tail,
        bounds,
        radius,
        [right, bottom - radius],
    );
    path.quad_to(
        right as f32,
        bottom as f32,
        (right - radius) as f32,
        bottom as f32,
    );
    emit_edge(
        &mut path,
        chosen,
        2,
        tail,
        bounds,
        radius,
        [left + radius, bottom],
    );
    path.quad_to(
        left as f32,
        bottom as f32,
        left as f32,
        (bottom - radius) as f32,
    );
    emit_edge(
        &mut path,
        chosen,
        3,
        tail,
        bounds,
        radius,
        [left, top + radius],
    );
    path.quad_to(left as f32, top as f32, (left + radius) as f32, top as f32);
    path.close();
    path.finish().context("Invalid editor bubble")
}
fn shape_path(shape: &Shape, bounds: Rect) -> Result<tiny_skia::Path> {
    match shape {
        Shape::Rectangle => Ok(tiny_skia::PathBuilder::from_rect(native_rect(bounds)?)),
        Shape::Ellipse => tiny_skia::PathBuilder::from_oval(native_rect(bounds)?)
            .context("Invalid editor ellipse"),
        Shape::SpeechBubble { tail, corner } => bubble(bounds, *tail, *corner),
        Shape::Arrow { start, end } => {
            let mut path = tiny_skia::PathBuilder::new();
            path.move_to(start[0] as f32, start[1] as f32);
            path.line_to(end[0] as f32, end[1] as f32);
            path.finish().context("Invalid editor arrow")
        }
        Shape::Path { points, closed } => {
            let mut path = tiny_skia::PathBuilder::new();
            let first = points.first().context("Empty editor path")?;
            path.move_to(first[0] as f32, first[1] as f32);
            for point in &points[1..] {
                path.line_to(point[0] as f32, point[1] as f32);
            }
            if *closed {
                path.close();
            }
            path.finish().context("Invalid editor path")
        }
    }
}
#[derive(Clone, Copy)]
pub(super) struct VectorInk<'a> {
    pub(super) path: &'a tiny_skia::Path,
    pub(super) transform: Affine,
    pub(super) color: [u8; 4],
    pub(super) stroke: Option<f64>,
    pub(super) fill: bool,
    pub(super) opacity: u8,
}
pub(super) fn vector(
    pixels: &mut RgbaImage,
    ink: VectorInk<'_>,
    area: PixelRegion,
    work: &mut ImageWork<'_, impl Fn() -> bool>,
) -> Result<()> {
    let color = ink.color;
    let opacity = ink.opacity;
    if color[3] == 0 || opacity == 0 {
        return Ok(());
    }
    coverage(
        [pixels.width(), pixels.height()],
        ink,
        area,
        work,
        |x, y, mask| {
            let alpha =
                (u32::from(color[3]) * u32::from(mask) * u32::from(opacity) + 32512) / 65025;
            blend(
                pixels.get_pixel_mut(x, y),
                [color[0], color[1], color[2], alpha as u8],
                255,
            );
        },
    )
}
fn coverage(
    size: [u32; 2],
    ink: VectorInk<'_>,
    area: PixelRegion,
    work: &mut ImageWork<'_, impl Fn() -> bool>,
    mut write: impl FnMut(u32, u32, u8),
) -> Result<()> {
    let VectorInk {
        path,
        transform,
        stroke,
        fill,
        ..
    } = ink;
    let mut paint = tiny_skia::Paint::default();
    paint.set_color_rgba8(255, 255, 255, 255);
    paint.anti_alias = true;
    let [a, b, c, d, tx, ty] = transform.0;
    for top in (area.y..area.y + area.height).step_by(TILE_HEIGHT as usize) {
        for left in (area.x..area.x + area.width).step_by(TILE_WIDTH as usize) {
            let width = (area.x + area.width - left).min(TILE_WIDTH);
            let height = (area.y + area.height - top).min(TILE_HEIGHT);
            let origin = [left.saturating_sub(1), top.saturating_sub(1)];
            let right = (left + width + 1).min(size[0]);
            let bottom = (top + height + 1).min(size[1]);
            work.charge(((right - origin[0]) * (bottom - origin[1]) * 4) as usize)?;
            let mut tile = tiny_skia::Pixmap::new(right - origin[0], bottom - origin[1])
                .context("Could not allocate editor vector tile")?;
            let transform = tiny_skia::Transform::from_row(
                a as f32,
                b as f32,
                c as f32,
                d as f32,
                (tx - origin[0] as f64) as f32,
                (ty - origin[1] as f64) as f32,
            );
            if fill {
                tile.fill_path(path, &paint, tiny_skia::FillRule::Winding, transform, None);
            }
            if let Some(width) = stroke {
                tile.stroke_path(
                    path,
                    &paint,
                    &tiny_skia::Stroke {
                        width: width as f32,
                        line_cap: tiny_skia::LineCap::Butt,
                        line_join: tiny_skia::LineJoin::Bevel,
                        miter_limit: 1.0,
                        ..Default::default()
                    },
                    transform,
                    None,
                );
            }
            work.check()?;
            for y in top..top + height {
                for x in left..left + width {
                    work.pixel()?;
                    let index = (((y - origin[1]) * tile.width() + x - origin[0]) * 4) as usize;
                    write(x, y, tile.data()[index + 3]);
                }
            }
        }
    }
    Ok(())
}

fn mix(original: [u8; 4], filtered: [u8; 4], opacity: u8) -> [u8; 4] {
    if opacity == 255 {
        return filtered;
    }
    if opacity == 0 {
        return original;
    }
    let a = u64::from(original[3]) * u64::from(255 - opacity);
    let b = u64::from(filtered[3]) * u64::from(opacity);
    let total = a + b;
    if total == 0 {
        return [0; 4];
    }
    let mut output = [0; 4];
    for channel in 0..3 {
        output[channel] =
            ((u64::from(original[channel]) * a + u64::from(filtered[channel]) * b + total / 2)
                / total) as u8;
    }
    output[3] = ((total + 127) / 255) as u8;
    output
}
struct FilterSource<R> {
    size: [u32; 2],
    read: R,
    transparent_edges: bool,
}
fn horizontal_blur<R: Fn(u32, u32) -> [u8; 4]>(
    source: &FilterSource<R>,
    y: u32,
    left: u32,
    radius: u32,
    output: &mut [[u64; 4]],
    work: &mut ImageWork<'_, impl Fn() -> bool>,
) -> Result<()> {
    let read = |x: u32| {
        let pixel = (source.read)(x, y);
        [
            u64::from(pixel[0]) * u64::from(pixel[3]),
            u64::from(pixel[1]) * u64::from(pixel[3]),
            u64::from(pixel[2]) * u64::from(pixel[3]),
            u64::from(pixel[3]),
        ]
    };
    work.charge((output.len() * 3 + 2 * radius as usize + 1) * 4)?;
    let mut sum = [0u64; 4];
    for x in left.saturating_sub(radius)..=(left + radius).min(source.size[0] - 1) {
        let pixel = read(x);
        for channel in 0..4 {
            sum[channel] += pixel[channel];
        }
    }
    for (offset, value) in output.iter_mut().enumerate() {
        *value = sum;
        let x = left + offset as u32;
        if x >= radius {
            let pixel = read(x - radius);
            for channel in 0..4 {
                sum[channel] -= pixel[channel];
            }
        }
        if x + radius + 1 < source.size[0] {
            let pixel = read(x + radius + 1);
            for channel in 0..4 {
                sum[channel] += pixel[channel];
            }
        }
    }
    Ok(())
}
fn filtered<R: Fn(u32, u32) -> [u8; 4]>(
    source: FilterSource<R>,
    area: PixelRegion,
    radius: u32,
    mut write: impl FnMut(u32, u32, [u8; 4]),
    work: &mut ImageWork<'_, impl Fn() -> bool>,
) -> Result<()> {
    const WIDTH: usize = 512;
    let rows = (radius * 2 + 1) as usize;
    let mut ring = Vec::with_capacity(rows * WIDTH);
    while ring.len() < rows * WIDTH {
        let count = (rows * WIDTH - ring.len()).min(2048);
        work.charge(count * 32)?;
        ring.extend(std::iter::repeat_n([0u64; 4], count));
    }
    let mut sums = vec![[0u64; 4]; WIDTH];
    for left in (area.x..area.x + area.width).step_by(WIDTH) {
        let width = (area.x + area.width - left).min(WIDTH as u32) as usize;
        work.charge(width * 4)?;
        sums[..width].fill([0; 4]);
        for y in area.y.saturating_sub(radius)..=(area.y + radius).min(source.size[1] - 1) {
            let row = &mut ring[(y as usize % rows) * WIDTH..][..width];
            horizontal_blur(&source, y, left, radius, row, work)?;
            work.charge(width * 4)?;
            for (sum, pixel) in sums[..width].iter_mut().zip(row) {
                for channel in 0..4 {
                    sum[channel] += pixel[channel];
                }
            }
        }
        for y in area.y..area.y + area.height {
            work.charge((width + 1) * 4)?;
            let row_count = (y + radius).min(source.size[1] - 1) - y.saturating_sub(radius) + 1;
            for (offset, sum) in sums[..width].iter().enumerate() {
                let x = left + offset as u32;
                let columns = (x + radius).min(source.size[0] - 1) - x.saturating_sub(radius) + 1;
                let mut color = [0; 4];
                for channel in 0..3 {
                    color[channel] = sum[channel].checked_div(sum[3]).unwrap_or(0) as u8;
                }
                let count = if source.transparent_edges {
                    (radius * 2 + 1) * (radius * 2 + 1)
                } else {
                    row_count * columns
                };
                color[3] = (sum[3] / u64::from(count)) as u8;
                write(x, y, color);
            }
            if y + 1 == area.y + area.height {
                break;
            }
            if y >= radius {
                let row = &ring[((y - radius) as usize % rows) * WIDTH..][..width];
                work.charge(width * 4)?;
                for (sum, pixel) in sums[..width].iter_mut().zip(row) {
                    for channel in 0..4 {
                        sum[channel] -= pixel[channel];
                    }
                }
            }
            if y + radius + 1 < source.size[1] {
                let next = y + radius + 1;
                let row = &mut ring[(next as usize % rows) * WIDTH..][..width];
                horizontal_blur(&source, next, left, radius, row, work)?;
                work.charge(width * 4)?;
                for (sum, pixel) in sums[..width].iter_mut().zip(row) {
                    for channel in 0..4 {
                        sum[channel] += pixel[channel];
                    }
                }
            }
        }
    }
    Ok(())
}
fn blurred(
    source: &RgbaImage,
    area: PixelRegion,
    radius: u32,
    write: impl FnMut(u32, u32, [u8; 4]),
    work: &mut ImageWork<'_, impl Fn() -> bool>,
) -> Result<()> {
    filtered(
        FilterSource {
            size: [source.width(), source.height()],
            read: |x, y| source.get_pixel(x, y).0,
            transparent_edges: false,
        },
        area,
        radius,
        write,
        work,
    )
}

fn pixelate(
    stage: &RgbaImage,
    output: &mut RgbaImage,
    layer: &Layer,
    area: PixelRegion,
    block: u32,
    budget: &Budget,
    other: u64,
    work: &mut ImageWork<'_, impl Fn() -> bool>,
) -> Result<()> {
    let left = area.x / block;
    let top = area.y / block;
    let right = (area.x + area.width).div_ceil(block);
    let bottom = (area.y + area.height).div_ceil(block);
    let columns = right - left;
    let rows = bottom - top;
    let bytes = u64::from(columns) * u64::from(rows) * 4;
    budget.admit(other + bytes)?;
    let mut colors = Vec::with_capacity((bytes / 4) as usize);
    for by in top..bottom {
        for bx in left..right {
            let mut sum = [0u64; 4];
            let mut count = 0u64;
            for y in by * block..((by + 1) * block).min(stage.height()) {
                for x in bx * block..((bx + 1) * block).min(stage.width()) {
                    work.pixel()?;
                    let pixel = stage.get_pixel(x, y).0;
                    for channel in 0..3 {
                        sum[channel] += u64::from(pixel[channel]) * u64::from(pixel[3]);
                    }
                    sum[3] += u64::from(pixel[3]);
                    count += 1;
                }
            }
            let mut color = [0u8; 4];
            for channel in 0..3 {
                color[channel] = sum[channel].checked_div(sum[3]).unwrap_or(0) as u8;
            }
            color[3] = (sum[3] / count) as u8;
            colors.push(color);
        }
    }
    let inverse = layer.transform.inverse()?;
    for y in area.y..area.y + area.height {
        for x in area.x..area.x + area.width {
            work.pixel()?;
            if layer
                .bounds
                .contains(inverse.point([x as f64 + 0.5, y as f64 + 0.5]))
            {
                let color = colors[((y / block - top) * columns + x / block - left) as usize];
                let original = output.get_pixel(x, y).0;
                output.put_pixel(x, y, Rgba(mix(original, color, layer.opacity)));
            }
        }
    }
    Ok(())
}
fn ellipse_contains(bounds: Rect, point: [f64; 2]) -> bool {
    let x = (point[0] - bounds.x) / bounds.width * 2.0 - 1.0;
    let y = (point[1] - bounds.y) / bounds.height * 2.0 - 1.0;
    x * x + y * y <= 1.0
}

fn draw_shape(
    output: &mut RgbaImage,
    layer: &Layer,
    shape: &Shape,
    style: &Style,
    area: PixelRegion,
    work: &mut ImageWork<'_, impl Fn() -> bool>,
) -> Result<()> {
    if let Shape::Path { points, .. } = shape
        && points.len() == 1
    {
        let radius = style.width.max(1.0) / 2.0;
        let bounds = Rect::new(
            points[0][0] - radius,
            points[0][1] - radius,
            radius * 2.0,
            radius * 2.0,
        );
        let path = shape_path(&Shape::Ellipse, bounds)?;
        return vector(
            output,
            VectorInk {
                path: &path,
                transform: layer.transform,
                color: style.stroke.or(style.fill).unwrap_or([0; 4]),
                stroke: None,
                fill: true,
                opacity: layer.opacity,
            },
            area,
            work,
        );
    }
    let path = shape_path(shape, layer.bounds)?;
    if let Some(color) = style.fill {
        vector(
            output,
            VectorInk {
                path: &path,
                transform: layer.transform,
                color,
                stroke: None,
                fill: true,
                opacity: layer.opacity,
            },
            area,
            work,
        )?;
    }
    if let Some(color) = style.stroke
        && style.width > 0.0
    {
        vector(
            output,
            VectorInk {
                path: &path,
                transform: layer.transform,
                color,
                stroke: Some(style.width),
                fill: false,
                opacity: layer.opacity,
            },
            area,
            work,
        )?;
    }
    if let Shape::Arrow { start, end } = shape
        && let Some(color) = style.stroke.or(style.fill)
    {
        let delta = [end[0] - start[0], end[1] - start[1]];
        let length = delta[0].hypot(delta[1]);
        let head = (style.width * 4.0).max(6.0).min(length * 0.45);
        let back = [
            end[0] - delta[0] / length * head,
            end[1] - delta[1] / length * head,
        ];
        let wing = [
            -delta[1] / length * head * 0.5,
            delta[0] / length * head * 0.5,
        ];
        let mut head = tiny_skia::PathBuilder::new();
        head.move_to(end[0] as f32, end[1] as f32);
        head.line_to((back[0] + wing[0]) as f32, (back[1] + wing[1]) as f32);
        head.line_to((back[0] - wing[0]) as f32, (back[1] - wing[1]) as f32);
        head.close();
        vector(
            output,
            VectorInk {
                path: &head.finish().context("Invalid editor arrowhead")?,
                transform: layer.transform,
                color,
                stroke: None,
                fill: true,
                opacity: layer.opacity,
            },
            area,
            work,
        )?;
    }
    Ok(())
}

pub(super) fn rgba(
    size: [u32; 2],
    work: &mut ImageWork<'_, impl Fn() -> bool>,
) -> Result<RgbaImage> {
    crate::validate_image_size(size[0], size[1])?;
    RgbaImage::from_raw(size[0], size[1], zeroed(extent(size) as usize, work)?)
        .context("Invalid editor raster extent")
}
fn raster(
    output: &mut RgbaImage,
    image: &RgbaImage,
    bounds: Rect,
    transform: Affine,
    area: PixelRegion,
    interpolation: Interpolation,
    opacity: u8,
    work: &mut ImageWork<'_, impl Fn() -> bool>,
) -> Result<()> {
    let inverse = transform.inverse()?;
    for y in area.y..area.y + area.height {
        for x in area.x..area.x + area.width {
            work.pixel()?;
            let local = inverse.point([x as f64 + 0.5, y as f64 + 0.5]);
            if !bounds.contains(local) {
                continue;
            }
            let point = [
                (local[0] - bounds.x) / bounds.width * image.width() as f64,
                (local[1] - bounds.y) / bounds.height * image.height() as f64,
            ];
            let color = stage_sample(image, point, interpolation);
            blend(output.get_pixel_mut(x, y), color, opacity);
        }
    }
    Ok(())
}
fn clip_visible(spec: &DocumentSpec, point: [f64; 2]) -> Result<bool> {
    Ok(if let Some(clip) = &spec.clip {
        clip.bounds.contains(clip.transform.inverse()?.point(point))
    } else {
        true
    })
}
fn export_scene(
    mut scene: RgbaImage,
    snapshot: &EditorSnapshot,
    masks: &[PrivacyMask],
    budget: &Budget,
    work: &mut ImageWork<'_, impl Fn() -> bool>,
) -> Result<RgbaImage> {
    let spec = &snapshot.spec;
    let crop = spec.crop.unwrap_or(PixelRegion {
        x: 0,
        y: 0,
        width: scene.width(),
        height: scene.height(),
    });
    if crop.x != 0
        || crop.y != 0
        || crop.width != scene.width()
        || crop.height != scene.height()
        || spec.clip.is_some()
    {
        budget.admit(extent(spec.canvas) + extent([crop.width, crop.height]))?;
        let mut cropped = rgba([crop.width, crop.height], work)?;
        let source_row = scene.width() as usize * 4;
        let output_row = crop.width as usize * 4;
        for y in 0..crop.height as usize {
            let start = (y + crop.y as usize) * source_row + crop.x as usize * 4;
            work.copy(
                &mut cropped.as_mut()[y * output_row..(y + 1) * output_row],
                &scene.as_raw()[start..start + output_row],
            )?;
        }
        if spec.clip.is_some() {
            for y in 0..cropped.height() {
                for x in 0..cropped.width() {
                    work.pixel()?;
                    if !clip_visible(
                        spec,
                        [
                            x as f64 + crop.x as f64 + 0.5,
                            y as f64 + crop.y as f64 + 0.5,
                        ],
                    )? {
                        cropped.put_pixel(x, y, Rgba([0; 4]));
                    }
                }
            }
        }
        scene = cropped;
    }
    let size = output_size(spec)?;
    let preset = &spec.export;
    if preset.padding == [0; 4]
        && preset.background == [0; 4]
        && preset.shadow.is_none()
        && preset.watermark.is_none()
    {
        return Ok(scene);
    }
    budget.admit(
        extent([scene.width(), scene.height()]) + extent(size) + KERNEL_SCRATCH + VECTOR_SCRATCH,
    )?;
    let mut output = rgba(size, work)?;
    if preset.background != [0; 4] {
        for bytes in output.as_mut().chunks_mut(16384) {
            work.charge(bytes.len())?;
            bytes.as_chunks_mut::<4>().0.fill(preset.background);
        }
    }
    let [left, top, _, _] = preset.padding;
    if let Some(shadow) = &preset.shadow {
        filtered(
            FilterSource {
                size,
                transparent_edges: true,
                read: |x, y| {
                    let sx = i64::from(x) - i64::from(left) - i64::from(shadow.offset[0]);
                    let sy = i64::from(y) - i64::from(top) - i64::from(shadow.offset[1]);
                    let alpha = if sx >= 0
                        && sy >= 0
                        && sx < i64::from(scene.width())
                        && sy < i64::from(scene.height())
                    {
                        scene.get_pixel(sx as u32, sy as u32)[3]
                    } else {
                        0
                    };
                    [0, 0, 0, alpha]
                },
            },
            PixelRegion {
                x: 0,
                y: 0,
                width: size[0],
                height: size[1],
            },
            shadow.radius,
            |x, y, color| {
                let mut value = shadow.color;
                value[3] = ((u32::from(value[3]) * u32::from(color[3]) + 127) / 255) as u8;
                blend(output.get_pixel_mut(x, y), value, 255);
            },
            work,
        )?;
    }

    for y in 0..scene.height() {
        for x in 0..scene.width() {
            work.pixel()?;
            let pixel = scene.get_pixel(x, y).0;
            if preset.background == [0; 4] && preset.shadow.is_none() {
                output.put_pixel(left + x, top + y, Rgba(pixel));
            } else {
                blend(output.get_pixel_mut(left + x, top + y), pixel, 255);
            }
        }
    }
    drop(scene);
    if let Some(watermark) = &preset.watermark {
        let image = &snapshot
            .assets
            .rasters
            .get(&watermark.asset)
            .context("Unknown editor watermark asset")?
            .pixels;
        if let Some(area) = clipped(watermark.area, size, 0.0) {
            raster(
                &mut output,
                image,
                watermark.area,
                Affine::IDENTITY,
                area,
                Interpolation::Bilinear,
                watermark.opacity,
                work,
            )?;
        }
    }
    // A watermark cannot paint over an active privacy mask. Crop clips still
    // apply, so transformed privacy objects cannot spill into export padding.
    for mask in masks {
        let translation = [left as f64 - crop.x as f64, top as f64 - crop.y as f64];
        let shifted = PrivacyMask {
            polygon: mask
                .polygon
                .map(|point| [point[0] + translation[0], point[1] + translation[1]]),
            extent: Rect::new(
                mask.extent.x + translation[0],
                mask.extent.y + translation[1],
                mask.extent.width,
                mask.extent.height,
            ),
        };
        let Some(area) = clipped(shifted.extent, size, 0.0) else {
            continue;
        };
        for y in area.y..area.y + area.height {
            for x in area.x..area.x + area.width {
                work.pixel()?;
                if x >= left
                    && y >= top
                    && x - left < crop.width
                    && y - top < crop.height
                    && shifted.intersects(x, y)
                    && clip_visible(
                        spec,
                        [
                            x as f64 - translation[0] + 0.5,
                            y as f64 - translation[1] + 0.5,
                        ],
                    )?
                {
                    output.put_pixel(x, y, Rgba([0, 0, 0, 255]));
                }
            }
        }
    }
    Ok(output)
}

pub(super) fn render(
    snapshot: &EditorSnapshot,
    cancelled: &impl Fn() -> bool,
) -> Result<CapturedImage> {
    let cancelled = crate::image_cancel::latch(cancelled);
    let mut work = ImageWork::new(&cancelled)?;
    let started = std::time::Instant::now();
    let spec = &snapshot.spec;
    spec.validate()?;
    snapshot.assets.validate_references(spec)?;
    let budget = Budget::new(snapshot)?;
    let canvas_bytes = extent(spec.canvas);
    let masks = spec
        .layers
        .iter()
        .filter(|layer| matches!(layer.content, LayerKind::Redaction))
        .map(PrivacyMask::new)
        .collect::<Result<Vec<_>>>()?;
    let bits = if masks.is_empty() {
        0
    } else {
        (snapshot.source.pixels.as_raw().len() as u64 / 4).div_ceil(8)
    };
    let sampling = spec.layers.iter().any(|layer| {
        matches!(
            layer.content,
            LayerKind::Blur { .. }
                | LayerKind::Pixelate { .. }
                | LayerKind::Clone { .. }
                | LayerKind::Magnifier { .. }
                | LayerKind::Legacy { .. }
        )
    });
    budget
        .admit(canvas_bytes * (1 + u64::from(sampling)) + bits + KERNEL_SCRATCH + VECTOR_SCRATCH)?;
    let crop = spec.crop.unwrap_or(PixelRegion {
        x: 0,
        y: 0,
        width: spec.canvas[0],
        height: spec.canvas[1],
    });
    let cropped_bytes = extent([crop.width, crop.height]);
    if spec.crop.is_some() || spec.clip.is_some() {
        budget.admit(canvas_bytes + cropped_bytes)?;
    }
    if spec.export.padding != [0; 4]
        || spec.export.background != [0; 4]
        || spec.export.shadow.is_some()
        || spec.export.watermark.is_some()
    {
        budget
            .admit(cropped_bytes + extent(output_size(spec)?) + KERNEL_SCRATCH + VECTOR_SCRATCH)?;
    }
    for layer in &spec.layers {
        work.check()?;
        let area = clipped(
            layer.transform.bounds(local_extent(layer))?,
            spec.canvas,
            1.0,
        );
        if let (LayerKind::Pixelate { block, .. }, Some(area)) = (&layer.content, area) {
            let columns = (area.x + area.width).div_ceil(*block) - area.x / block;
            let rows = (area.y + area.height).div_ceil(*block) - area.y / block;
            budget.admit(
                canvas_bytes * 2
                    + bits
                    + KERNEL_SCRATCH
                    + VECTOR_SCRATCH
                    + u64::from(columns) * u64::from(rows) * 4,
            )?;
        }
        if let LayerKind::Text { text } = &layer.content {
            text::admit_frame([
                layer.bounds.width.ceil() as u32,
                layer.bounds.height.ceil() as u32,
            ])?;
            budget.admit(
                canvas_bytes
                    + bits
                    + text::scratch_budget(text, &snapshot.assets)?
                    + MAX_TEXT_RASTER as u64
                    + VECTOR_SCRATCH,
            )?;
        }
        if let LayerKind::Legacy { annotation } = &layer.content {
            let size = annotation.stamp_size()?;
            budget.admit(canvas_bytes + extent(size) * 2 + bits + VECTOR_SCRATCH)?;
        }
    }
    let source = ProtectedSource::new(&snapshot.source.pixels, spec.base, &masks, &mut work)?;
    let base_inverse = spec.base.inverse()?;
    let mut output = base_image(
        &source,
        spec.canvas,
        base_inverse,
        spec.interpolation,
        &mut work,
    )?;
    protect(&mut output, &masks, &mut work)?;
    let mut stage: Option<RgbaImage> = None;
    let mut stage_is_base = false;
    let mut layer_index = 0;
    while layer_index < spec.layers.len() {
        let layer = &spec.layers[layer_index];
        layer_index += 1;
        if let LayerKind::Legacy { annotation } = &layer.content
            && layer.transform == Affine::IDENTITY
            && layer.opacity == 255
        {
            stage.take();
            let mut draws = vec![annotation.draw().clone()];
            while let Some(next) = spec.layers.get(layer_index) {
                let LayerKind::Legacy { annotation } = &next.content else {
                    break;
                };
                if next.transform != Affine::IDENTITY || next.opacity != 255 {
                    break;
                }
                draws.push(annotation.draw().clone());
                layer_index += 1;
            }
            crate::layers::paint_ordered(
                &mut output,
                PixelRegion {
                    x: 0,
                    y: 0,
                    width: spec.canvas[0],
                    height: spec.canvas[1],
                },
                &draws,
                spec.canvas,
                &mut work,
            )?;
            continue;
        }
        work.check()?;
        if layer.opacity == 0 || matches!(layer.content, LayerKind::Redaction) {
            continue;
        }
        let outside = matches!(layer.content, LayerKind::Highlight { outside: true, .. });
        let area = if outside {
            Some(PixelRegion {
                x: 0,
                y: 0,
                width: output.width(),
                height: output.height(),
            })
        } else {
            clipped(
                layer.transform.bounds(local_extent(layer))?,
                spec.canvas,
                1.0,
            )
        };
        let Some(area) = area else { continue };
        let inverse = layer.transform.inverse()?;
        let requires_stage = matches!(
            layer.content,
            LayerKind::Blur { .. }
                | LayerKind::Pixelate { .. }
                | LayerKind::Clone { .. }
                | LayerKind::Magnifier { .. }
        );
        if requires_stage {
            protect(&mut output, &masks, &mut work)?;
            let base_input = matches!(
                layer.content,
                LayerKind::Blur {
                    input: EffectInput::ProtectedBase,
                    ..
                } | LayerKind::Pixelate {
                    input: EffectInput::ProtectedBase,
                    ..
                }
            );
            let new_stage = stage.is_none();
            if new_stage {
                stage = Some(rgba(spec.canvas, &mut work)?);
            }
            if base_input {
                if new_stage || !stage_is_base {
                    let target = stage.as_mut().unwrap();
                    if base_inverse == Affine::IDENTITY
                        && spec.canvas == [source.pixels.width(), source.pixels.height()]
                    {
                        work.copy(target.as_mut(), source.pixels.as_raw())?;
                        source_mask(&source, target.as_mut(), &mut work)?;
                    } else {
                        for y in 0..target.height() {
                            for x in 0..target.width() {
                                work.pixel()?;
                                target.put_pixel(
                                    x,
                                    y,
                                    Rgba(source.sample(
                                        base_inverse.point([x as f64 + 0.5, y as f64 + 0.5]),
                                        spec.interpolation,
                                    )),
                                );
                            }
                        }
                    }
                    protect(target, &masks, &mut work)?;
                }
                stage_is_base = true;
            } else {
                work.copy(stage.as_mut().unwrap().as_mut(), output.as_raw())?;
                stage_is_base = false;
            }
        }
        match &layer.content {
            LayerKind::Legacy { annotation } => {
                stage.take();
                let size = annotation.stamp_size()?;
                let mut stamp = rgba(size, &mut work)?;
                let draw = annotation.translated_draw()?;
                crate::layers::paint_ordered(
                    &mut stamp,
                    PixelRegion {
                        x: 0,
                        y: 0,
                        width: size[0],
                        height: size[1],
                    },
                    std::slice::from_ref(&draw),
                    size,
                    &mut work,
                )?;
                raster(
                    &mut output,
                    &stamp,
                    annotation.bounds(),
                    layer.transform,
                    area,
                    Interpolation::Bilinear,
                    layer.opacity,
                    &mut work,
                )?;
            }
            LayerKind::Draw { shape, style } => {
                draw_shape(&mut output, layer, shape, style, area, &mut work)?
            }
            LayerKind::Highlight { color, outside } => {
                for y in area.y..area.y + area.height {
                    for x in area.x..area.x + area.width {
                        work.pixel()?;
                        if layer
                            .bounds
                            .contains(inverse.point([x as f64 + 0.5, y as f64 + 0.5]))
                            != *outside
                        {
                            blend(output.get_pixel_mut(x, y), *color, layer.opacity);
                        }
                    }
                }
            }
            LayerKind::Raster {
                asset,
                interpolation,
                ..
            } => raster(
                &mut output,
                &snapshot
                    .assets
                    .rasters
                    .get(asset)
                    .context("Unknown editor raster asset")?
                    .pixels,
                layer.bounds,
                layer.transform,
                area,
                *interpolation,
                layer.opacity,
                &mut work,
            )?,
            LayerKind::Text { text } => {
                stage.take();
                let image = text::prepare(
                    text,
                    &snapshot.assets,
                    [
                        layer.bounds.width.ceil() as u32,
                        layer.bounds.height.ceil() as u32,
                    ],
                    &mut work,
                )?;
                for y in area.y..area.y + area.height {
                    for x in area.x..area.x + area.width {
                        work.pixel()?;
                        let local = inverse.point([x as f64 + 0.5, y as f64 + 0.5]);
                        if layer.bounds.contains(local) {
                            let color = stage_sample(
                                &image,
                                [local[0] - layer.bounds.x, local[1] - layer.bounds.y],
                                Interpolation::Bilinear,
                            );
                            blend(output.get_pixel_mut(x, y), color, layer.opacity);
                        }
                    }
                }
            }
            LayerKind::Blur { radius, .. } => blurred(
                stage.as_ref().unwrap(),
                area,
                *radius,
                |x, y, color| {
                    if layer
                        .bounds
                        .contains(inverse.point([x as f64 + 0.5, y as f64 + 0.5]))
                    {
                        let old = output.get_pixel(x, y).0;
                        output.put_pixel(x, y, Rgba(mix(old, color, layer.opacity)));
                    }
                },
                &mut work,
            )?,
            LayerKind::Pixelate { block, .. } => pixelate(
                stage.as_ref().unwrap(),
                &mut output,
                layer,
                area,
                *block,
                &budget,
                canvas_bytes * 2 + bits + KERNEL_SCRATCH + VECTOR_SCRATCH,
                &mut work,
            )?,
            LayerKind::Clone {
                source: map,
                interpolation,
            }
            | LayerKind::Magnifier {
                source: map,
                interpolation,
                ..
            } => {
                let ellipse = matches!(layer.content, LayerKind::Magnifier { .. });
                if let LayerKind::Magnifier { style, .. } = &layer.content
                    && let Some(color) = style.fill
                {
                    let path = shape_path(&Shape::Ellipse, layer.bounds)?;
                    vector(
                        &mut output,
                        VectorInk {
                            path: &path,
                            transform: layer.transform,
                            color,
                            stroke: None,
                            fill: true,
                            opacity: layer.opacity,
                        },
                        area,
                        &mut work,
                    )?;
                }
                for y in area.y..area.y + area.height {
                    for x in area.x..area.x + area.width {
                        work.pixel()?;
                        let local = inverse.point([x as f64 + 0.5, y as f64 + 0.5]);
                        if layer.bounds.contains(local)
                            && (!ellipse || ellipse_contains(layer.bounds, local))
                        {
                            let color = stage_sample(
                                stage.as_ref().unwrap(),
                                map.point(local),
                                *interpolation,
                            );
                            blend(output.get_pixel_mut(x, y), color, layer.opacity);
                        }
                    }
                }
                if let LayerKind::Magnifier { style, .. } = &layer.content {
                    let shape = Shape::Ellipse;
                    let path = shape_path(&shape, layer.bounds)?;
                    if let Some(color) = style.stroke
                        && style.width > 0.0
                    {
                        vector(
                            &mut output,
                            VectorInk {
                                path: &path,
                                transform: layer.transform,
                                color,
                                stroke: Some(style.width),
                                fill: false,
                                opacity: layer.opacity,
                            },
                            area,
                            &mut work,
                        )?;
                    }
                }
            }
            LayerKind::Erase { mode, shape, width } => {
                let (path, stroke, fill) = if let Shape::Path { points, .. } = shape {
                    if points.len() == 1 {
                        let radius = *width / 2.0;
                        (
                            shape_path(
                                &Shape::Ellipse,
                                Rect::new(
                                    points[0][0] - radius,
                                    points[0][1] - radius,
                                    *width,
                                    *width,
                                ),
                            )?,
                            None,
                            true,
                        )
                    } else {
                        (shape_path(shape, layer.bounds)?, Some(*width), false)
                    }
                } else {
                    (shape_path(shape, layer.bounds)?, None, true)
                };
                coverage(
                    spec.canvas,
                    VectorInk {
                        path: &path,
                        transform: layer.transform,
                        color: [255; 4],
                        stroke,
                        fill,
                        opacity: 255,
                    },
                    area,
                    &mut work,
                    |x, y, mask| {
                        if mask == 0 {
                            return;
                        }
                        let opacity =
                            ((u32::from(layer.opacity) * u32::from(mask) + 127) / 255) as u8;
                        let target = match mode {
                            EraseMode::Transparent => [0; 4],
                            EraseMode::RestoreBase => source.sample(
                                base_inverse.point([x as f64 + 0.5, y as f64 + 0.5]),
                                spec.interpolation,
                            ),
                        };
                        let old = output.get_pixel(x, y).0;
                        output.put_pixel(x, y, Rgba(mix(old, target, opacity)));
                    },
                )?;
            }
            LayerKind::Redaction => unreachable!(),
        }
    }
    protect(&mut output, &masks, &mut work)?;
    drop(stage);
    drop(source);
    let output = export_scene(output, snapshot, &masks, &budget, &mut work)?;
    work.check()?;
    let size = output.dimensions();
    let suffix = match spec.export.format {
        ExportFormat::Png => "png",
        ExportFormat::Jpeg => "jpg",
        ExportFormat::Webp | ExportFormat::WebpLossy => "webp",
    };
    Ok(CapturedImage {
        width: size.0,
        height: size.1,
        pixels: Arc::new(output),
        display_geometry: None,
        png_path: snapshot.source.png_path.with_file_name(format!(
            "edited-{}.{}",
            uuid::Uuid::new_v4(),
            suffix
        )),
        elapsed_ms: started.elapsed().as_millis(),
        ..snapshot.source.clone()
    })
}
