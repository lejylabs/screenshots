//! Explicit outline-font preparation; no system database, bitmap cache or idle work.
use super::*;
use crate::{PixelRegion, image_cancel::ImageWork};
use anyhow::{Context, Result, ensure};
use image::RgbaImage;
use skrifa::{
    FontRef, MetadataProvider,
    instance::Size,
    outline::{DrawSettings, Hinting, OutlinePen},
};
use std::ops::Range;
use unicode_bidi::BidiInfo;
use unicode_script::{Script, UnicodeScript};
use unicode_segmentation::UnicodeSegmentation;

#[derive(Clone, Copy)]
pub(super) struct FontAdmission {
    layout_budget: u64,
    weight: u16,
    italic: bool,
    variable_weight: bool,
    variable_italic: bool,
}
fn u16_at(bytes: &[u8], offset: usize) -> Result<u16> {
    Ok(u16::from_be_bytes(
        bytes
            .get(offset..offset + 2)
            .context("Truncated editor font")?
            .try_into()?,
    ))
}
fn u32_at(bytes: &[u8], offset: usize) -> Result<u32> {
    Ok(u32::from_be_bytes(
        bytes
            .get(offset..offset + 4)
            .context("Truncated editor font")?
            .try_into()?,
    ))
}
pub(super) fn admit_font(bytes: &[u8], index: u32) -> Result<FontAdmission> {
    ensure!(
        !bytes.is_empty() && bytes.len() <= 2 * 1024 * 1024 && index < 16,
        "Editor font source/index exceeds its bound"
    );
    let offset = if bytes.starts_with(b"ttcf") {
        let count = u32_at(bytes, 8)?;
        ensure!(
            (1..=16).contains(&count) && index < count,
            "Invalid editor font collection"
        );
        u32_at(bytes, 12 + index as usize * 4)? as usize
    } else {
        ensure!(index == 0, "A standalone editor font requires face zero");
        0
    };
    let signature = bytes
        .get(offset..offset + 4)
        .context("Invalid editor font face")?;
    ensure!(
        signature == [0, 1, 0, 0] || signature == b"OTTO" || signature == b"true",
        "Editor fonts require an OpenType outline face"
    );
    let count = u16_at(bytes, offset + 4)? as usize;
    ensure!(
        count > 0 && count <= 256,
        "Editor font table count exceeds its bound"
    );
    let mut tags = BTreeSet::new();
    let mut layout_bytes = 0u64;
    let mut outline = false;
    for entry in 0..count {
        let start = offset + 12 + entry * 16;
        let tag: [u8; 4] = bytes
            .get(start..start + 4)
            .context("Truncated editor font directory")?
            .try_into()?;
        let location = u32_at(bytes, start + 8)? as u64;
        let length = u32_at(bytes, start + 12)? as u64;
        ensure!(
            tags.insert(tag)
                && location
                    .checked_add(length)
                    .is_some_and(|end| end <= bytes.len() as u64),
            "Invalid editor font table extent or duplicate"
        );
        ensure!(
            !matches!(&tag, b"morx" | b"mort" | b"kerx"),
            "Editor outline preparation requires bounded OpenType layout tables"
        );
        if matches!(&tag, b"GSUB" | b"GPOS" | b"GDEF") {
            layout_bytes = layout_bytes
                .checked_add(length)
                .context("Editor font layout extent overflow")?;
        }
        outline |= matches!(&tag, b"glyf" | b"CFF " | b"CFF2");
    }
    ensure!(outline, "Editor font has no supported scalable outlines");
    let font =
        FontRef::from_index(bytes, index).map_err(|_| anyhow::anyhow!("Invalid editor font"))?;
    let metrics = font.metrics(Size::new(1.0), skrifa::instance::LocationRef::default());
    ensure!(
        (16..=16384).contains(&metrics.units_per_em)
            && (1..=65535).contains(&metrics.glyph_count)
            && [metrics.ascent, metrics.descent, metrics.leading]
                .iter()
                .all(|value| value.is_finite() && value.abs() <= 32.0),
        "Invalid editor font metrics"
    );
    // Legacy kern uses a separate AAT cache, outside HarfRust's OT budget.
    // Its allocation-free native-parser preflight precedes all Shaper creation.
    let kern_bytes = if tags.contains(b"kern") {
        harfrust::kern_preprocessing_size(bytes, index)
            .context("Editor legacy kern preprocessing exceeds its bounded work/cache allowance")?
            as u64
    } else {
        0
    };
    let axes = font.axes();
    ensure!(
        axes.len() <= 16,
        "Editor font variation axes exceed their bound"
    );
    let variable_weight = axes
        .iter()
        .any(|axis| axis.tag() == skrifa::raw::types::Tag::new(b"wght"));
    let variable_italic = axes.iter().any(|axis| {
        axis.tag() == skrifa::raw::types::Tag::new(b"ital")
            || axis.tag() == skrifa::raw::types::Tag::new(b"slnt")
    });
    let attributes = font.attributes();
    let weight = attributes.weight.value().round().clamp(1.0, 1000.0) as u16;
    Ok(FontAdmission {
        layout_budget: layout_bytes
            .saturating_mul(256)
            .clamp(4 * 1024 * 1024, 128 * 1024 * 1024)
            .checked_add(kern_bytes)
            .context("Editor font cache extent overflow")?,
        weight,
        italic: !matches!(attributes.style, skrifa::attribute::Style::Normal),
        variable_weight,
        variable_italic,
    })
}
pub(super) fn scratch_budget(text: &RichText, assets: &EditorAssets) -> Result<u64> {
    let mut maximum = 0;
    for span in &text.spans {
        for id in &span.style.fonts {
            maximum = maximum.max(
                assets
                    .fonts
                    .get(id)
                    .context("Unknown editor font asset")?
                    .admission
                    .layout_budget,
            );
        }
    }
    // HarfRust's bounded expansion buffer, plans/mappings, glyph placements,
    // explicit outline scratch and path storage. There is no glyph bitmap cache.
    Ok(maximum + 12 * 1024 * 1024)
}
#[derive(Clone)]
struct SpanRange {
    range: Range<usize>,
    style: usize,
}
#[derive(Clone, Copy)]
struct Glyph {
    font: usize,
    style: usize,
    id: u32,
    x: f32,
    y: f32,
    pen: f32,
    advance: f32,
    baseline: f32,
    empty_allowed: bool,
}
struct Line {
    glyphs: Vec<Glyph>,
    width: f32,
    ascent: f32,
    height: f32,
}
struct Layout<'a> {
    text: String,
    ranges: Vec<SpanRange>,
    spans: &'a [TextSpan],
    fonts: Vec<(&'a str, &'a FontAsset)>,
}
impl<'a> Layout<'a> {
    fn new(text: &'a RichText, assets: &'a EditorAssets) -> Self {
        let mut source = String::with_capacity(text.bytes());
        let mut ranges = Vec::with_capacity(text.spans.len());
        for (index, span) in text.spans.iter().enumerate() {
            let start = source.len();
            for character in span.text.chars() {
                if character == '\t' {
                    source.push_str("    ");
                } else {
                    source.push(character);
                }
            }
            ranges.push(SpanRange {
                range: start..source.len(),
                style: index,
            });
        }
        Self {
            text: source,
            ranges,
            spans: &text.spans,
            fonts: assets
                .fonts
                .iter()
                .map(|(id, font)| (id.as_str(), font))
                .collect(),
        }
    }
    fn style(&self, byte: usize) -> usize {
        self.ranges
            .iter()
            .find(|span| span.range.contains(&byte))
            .map_or(self.spans.len() - 1, |span| span.style)
    }
    fn choose_font(&self, grapheme: &str, style: &TextStyle) -> Result<usize> {
        let mut selected = None;
        for (order, id) in style.fonts.iter().enumerate() {
            let (index, (_, asset)) = self
                .fonts
                .iter()
                .enumerate()
                .find(|(_, entry)| entry.0 == id)
                .context("Unknown editor font asset")?;
            let font = FontRef::from_index(&asset.data.bytes, asset.index)
                .map_err(|_| anyhow::anyhow!("Invalid admitted editor font"))?;
            let charmap = font.charmap();
            if !grapheme
                .chars()
                .all(|character| ignorable(character) || charmap.map(character).is_some())
            {
                continue;
            }
            let weight = if asset.admission.variable_weight {
                0
            } else {
                style.weight.abs_diff(asset.admission.weight) as usize
            };
            let italic = usize::from(
                style.italic && !asset.admission.italic && !asset.admission.variable_italic,
            ) * 200;
            let score = order * 1000 + weight + italic;
            if selected.is_none_or(|(_, previous)| score < previous) {
                selected = Some((index, score));
            }
        }
        selected
            .map(|(index, _)| index)
            .context("Supplied editor fonts do not cover a grapheme")
    }
    fn shape(
        &self,
        range: Range<usize>,
        line_height: f32,
        work: &mut ImageWork<'_, impl Fn() -> bool>,
    ) -> Result<Line> {
        work.check()?;
        let source = &self.text[range.clone()];
        let bidi = BidiInfo::new(source, None);
        let mut result = Line {
            glyphs: Vec::new(),
            width: 0.0,
            ascent: 0.0,
            height: 0.0,
        };
        if source.is_empty() {
            let style = &self.spans[self.style(range.start)].style;
            return Ok(Line {
                glyphs: Vec::new(),
                width: 0.0,
                ascent: style.size,
                height: style.size * line_height,
            });
        }
        ensure!(
            bidi.paragraphs.len() <= 1,
            "Text layout received multiple paragraphs"
        );
        let paragraph = bidi
            .paragraphs
            .first()
            .context("Invalid editor text paragraph")?;
        let (levels, runs) = bidi.visual_runs(paragraph, 0..source.len());
        for visual in runs {
            let rtl = levels[visual.start].is_rtl();
            let mut logical: Vec<(Range<usize>, usize, usize, Script)> = Vec::new();
            let mut previous = Script::Common;
            for (offset, grapheme) in source[visual.clone()].grapheme_indices(true) {
                work.charge(grapheme.len().max(1))?;
                let begin = visual.start + offset;
                let end = begin + grapheme.len();
                let style = self.style(range.start + begin);
                let font = self.choose_font(grapheme, &self.spans[style].style)?;
                let script = grapheme
                    .chars()
                    .map(|character| character.script())
                    .find(|script| !matches!(script, Script::Common | Script::Inherited))
                    .unwrap_or(previous);
                if !matches!(script, Script::Common | Script::Inherited) {
                    previous = script;
                }
                let merge = logical.last().is_some_and(|(_, f, s, old_script)| {
                    *f == font
                        && *old_script == script
                        && same_shape_style(&self.spans[*s].style, &self.spans[style].style)
                });
                if merge {
                    logical.last_mut().unwrap().0.end = end;
                } else {
                    logical.push((begin..end, font, style, script));
                }
            }
            if rtl {
                logical.reverse();
            }
            for (part, font_index, style_index, script) in logical {
                work.check()?;
                let text = &source[part.clone()];
                ensure!(
                    text.chars().count() <= 128,
                    "Contextual editor text run exceeds 128 scalars; use natural shorter lines"
                );
                let asset = self.fonts[font_index].1;
                let style = &self.spans[style_index].style;
                let blob: Arc<dyn AsRef<[u8]> + Send + Sync> = Arc::new(asset.data.bytes.clone());
                let face = harfrust::Font::new(blob, asset.index)
                    .context("Could not prepare admitted editor font")?;
                let face = face
                    .instance_builder()
                    .variations([
                        (harfrust::Tag::new(b"wght"), style.weight as f32),
                        (
                            harfrust::Tag::new(b"ital"),
                            if style.italic { 1.0 } else { 0.0 },
                        ),
                        (
                            harfrust::Tag::new(b"slnt"),
                            if style.italic { -14.0 } else { 0.0 },
                        ),
                    ])
                    .build();
                let shaper = harfrust::ShaperFont::new(&face);
                ensure!(
                    !shaper.preprocessing_budget_exhausted(),
                    "Editor font preprocessing exhausted its work/allocation bound"
                );
                let mut buffer = harfrust::Buffer::new();
                buffer.set_fail_on_runtime_budget_exhaustion(true);
                buffer.push_str(text);
                buffer.set_direction(if rtl {
                    harfrust::Direction::RightToLeft
                } else {
                    harfrust::Direction::LeftToRight
                });
                buffer.guess_segment_properties();
                let tag = script.short_name().as_bytes();
                if tag.len() == 4 {
                    buffer.set_script(harfrust::Script::from_iso15924_tag(harfrust::Tag::new(
                        tag.try_into()?,
                    )));
                }
                buffer.set_pre_context(&source[..part.start]);
                buffer.set_post_context(&source[part.end..]);
                harfrust::shape(&shaper, &mut buffer, harfrust::ShapeOptions::new())
                    .map_err(|_| anyhow::anyhow!("Editor font shaping failed"))?;
                work.check()?;
                ensure!(
                    !shaper.preprocessing_budget_exhausted(),
                    "Editor font preprocessing exhausted its work/allocation bound"
                );
                ensure!(
                    buffer.allocation_successful() && buffer.glyph_infos().len() <= 8192,
                    "Editor shaping exhausted its work/glyph bound"
                );
                ensure!(
                    result
                        .glyphs
                        .len()
                        .checked_add(buffer.glyph_infos().len())
                        .is_some_and(|count| count <= 8192),
                    "Editor text exceeds its placed-glyph bound"
                );
                let scale = style.size / f32::from(face.units_per_em());
                let mut advance = 0.0f32;
                for (info, position) in buffer.glyph_infos().iter().zip(buffer.glyph_positions()) {
                    work.charge(32)?;
                    let cluster = part.start + info.cluster as usize;
                    ensure!(
                        cluster < source.len() && source.is_char_boundary(cluster),
                        "Invalid shaped editor cluster"
                    );
                    let style_index = self.style(range.start + cluster);
                    let character = source[cluster..].chars().next().unwrap();
                    ensure!(
                        info.glyph_id != 0 || character.is_whitespace() || ignorable(character),
                        "Supplied editor font is missing a shaped glyph"
                    );
                    ensure!(
                        [
                            position.x_advance,
                            position.y_advance,
                            position.x_offset,
                            position.y_offset
                        ]
                        .iter()
                        .all(|value| value.unsigned_abs() <= 16_777_216),
                        "Invalid editor glyph positioning"
                    );
                    let glyph_advance = position.x_advance as f32 * scale;
                    result.glyphs.push(Glyph {
                        font: font_index,
                        style: style_index,
                        id: info.glyph_id,
                        x: result.width + advance + position.x_offset as f32 * scale,
                        y: -(position.y_offset as f32) * scale,
                        pen: result.width + advance,
                        advance: glyph_advance,
                        baseline: 0.0,
                        empty_allowed: character.is_whitespace() || ignorable(character),
                    });
                    advance += glyph_advance;
                }
                ensure!(
                    result.glyphs.len() <= 8192 && advance.is_finite() && advance >= 0.0,
                    "Invalid or excessive editor glyph layout"
                );
                result.width += advance;
                let font = FontRef::from_index(&asset.data.bytes, asset.index)
                    .map_err(|_| anyhow::anyhow!("Invalid admitted editor font"))?;
                let location = font.axes().location([
                    ("wght", style.weight as f32),
                    ("ital", if style.italic { 1.0 } else { 0.0 }),
                    ("slnt", if style.italic { -14.0 } else { 0.0 }),
                ]);
                let metrics = font.metrics(Size::new(style.size), &location);
                ensure!(
                    [metrics.ascent, metrics.descent, metrics.leading]
                        .iter()
                        .all(|value| value.is_finite() && value.abs() <= 8192.0),
                    "Invalid scaled editor font metrics"
                );
                result.ascent = result.ascent.max(metrics.ascent);
                result.height = result
                    .height
                    .max((metrics.ascent - metrics.descent + metrics.leading) * line_height);
            }
        }
        Ok(result)
    }
}
fn same_shape_style(a: &TextStyle, b: &TextStyle) -> bool {
    a.fonts == b.fonts && a.size == b.size && a.weight == b.weight && a.italic == b.italic
}
fn ignorable(character: char) -> bool {
    matches!(character,'\u{200c}'..='\u{200f}'|'\u{202a}'..='\u{202e}'|'\u{2066}'..='\u{2069}'|'\u{fe0e}'|'\u{fe0f}')
}

struct Pen<'a, 'b, C: Fn() -> bool> {
    work: &'a mut ImageWork<'b, C>,
    path: tiny_skia::PathBuilder,
    origin: [f32; 2],
    skew: f32,
    current: [f32; 2],
    start: [f32; 2],
    segments: usize,
    error: Option<anyhow::Error>,
}
impl<C: Fn() -> bool> Pen<'_, '_, C> {
    fn point(&mut self, x: f32, y: f32) -> Option<[f32; 2]> {
        if self.error.is_some() {
            return None;
        }
        let point = [self.origin[0] + x + self.skew * y, self.origin[1] - y];
        if !point
            .iter()
            .all(|value| value.is_finite() && value.abs() <= 1_048_576.0)
        {
            self.error = Some(anyhow::anyhow!(
                "Editor glyph coordinates exceed their bound"
            ));
            None
        } else {
            Some(point)
        }
    }
    fn charge(&mut self) -> bool {
        if self.error.is_some() {
            return false;
        }
        self.segments += 1;
        if self.segments > 8192 {
            self.error = Some(anyhow::anyhow!("Editor glyph path exceeds 8192 segments"));
            return false;
        }
        if let Err(error) = self.work.charge(16) {
            self.error = Some(error.into());
            return false;
        }
        true
    }
    fn line(&mut self, point: [f32; 2]) {
        if self.charge() {
            self.path.line_to(point[0], point[1]);
            self.current = point;
        }
    }
    fn quad(&mut self, a: [f32; 2], b: [f32; 2], c: [f32; 2], depth: u8) {
        if self.error.is_some() {
            return;
        }
        let middle = [(a[0] + c[0]) * 0.5, (a[1] + c[1]) * 0.5];
        if (b[0] - middle[0]).hypot(b[1] - middle[1]) <= 0.25 {
            self.line(c);
            return;
        }
        if depth >= 16 {
            self.error = Some(anyhow::anyhow!(
                "Editor glyph curve exceeds its subdivision bound"
            ));
            return;
        }
        let ab = [(a[0] + b[0]) * 0.5, (a[1] + b[1]) * 0.5];
        let bc = [(b[0] + c[0]) * 0.5, (b[1] + c[1]) * 0.5];
        let split = [(ab[0] + bc[0]) * 0.5, (ab[1] + bc[1]) * 0.5];
        self.quad(a, ab, split, depth + 1);
        self.quad(split, bc, c, depth + 1);
    }
    fn cubic(&mut self, a: [f32; 2], b: [f32; 2], c: [f32; 2], d: [f32; 2], depth: u8) {
        if self.error.is_some() {
            return;
        }
        let one = [a[0] + (d[0] - a[0]) / 3.0, a[1] + (d[1] - a[1]) / 3.0];
        let two = [
            a[0] + (d[0] - a[0]) * 2.0 / 3.0,
            a[1] + (d[1] - a[1]) * 2.0 / 3.0,
        ];
        if (b[0] - one[0])
            .hypot(b[1] - one[1])
            .max((c[0] - two[0]).hypot(c[1] - two[1]))
            <= 0.25
        {
            self.line(d);
            return;
        }
        if depth >= 16 {
            self.error = Some(anyhow::anyhow!(
                "Editor glyph curve exceeds its subdivision bound"
            ));
            return;
        }
        let mid = |x: [f32; 2], y: [f32; 2]| [(x[0] + y[0]) * 0.5, (x[1] + y[1]) * 0.5];
        let ab = mid(a, b);
        let bc = mid(b, c);
        let cd = mid(c, d);
        let abc = mid(ab, bc);
        let bcd = mid(bc, cd);
        let split = mid(abc, bcd);
        self.cubic(a, ab, abc, split, depth + 1);
        self.cubic(split, bcd, cd, d, depth + 1);
    }
}
impl<C: Fn() -> bool> OutlinePen for Pen<'_, '_, C> {
    fn move_to(&mut self, x: f32, y: f32) {
        if let Some(point) = self.point(x, y)
            && self.charge()
        {
            self.path.move_to(point[0], point[1]);
            self.current = point;
            self.start = point;
        }
    }
    fn line_to(&mut self, x: f32, y: f32) {
        if let Some(point) = self.point(x, y) {
            self.line(point);
        }
    }
    fn quad_to(&mut self, x0: f32, y0: f32, x: f32, y: f32) {
        if let (Some(control), Some(end)) = (self.point(x0, y0), self.point(x, y)) {
            self.quad(self.current, control, end, 0);
        }
    }
    fn curve_to(&mut self, x0: f32, y0: f32, x1: f32, y1: f32, x: f32, y: f32) {
        if let (Some(a), Some(b), Some(end)) =
            (self.point(x0, y0), self.point(x1, y1), self.point(x, y))
        {
            self.cubic(self.current, a, b, end, 0);
        }
    }
    fn close(&mut self) {
        if self.charge() {
            self.path.close();
            self.current = self.start;
        }
    }
}
struct GlyphInk {
    path: Option<tiny_skia::Path>,
    bold: Option<f64>,
    decorations: Vec<Rect>,
}
fn glyph_ink(
    glyph: Glyph,
    layout: &Layout<'_>,
    work: &mut ImageWork<'_, impl Fn() -> bool>,
) -> Result<GlyphInk> {
    work.check()?;
    let asset = layout.fonts[glyph.font].1;
    let style = &layout.spans[glyph.style].style;
    let font = FontRef::from_index(&asset.data.bytes, asset.index)
        .map_err(|_| anyhow::anyhow!("Invalid admitted editor font"))?;
    let location = font.axes().location([
        ("wght", style.weight as f32),
        ("ital", if style.italic { 1.0 } else { 0.0 }),
        ("slnt", if style.italic { -14.0 } else { 0.0 }),
    ]);
    let bold = (!asset.admission.variable_weight && style.weight > asset.admission.weight)
        .then_some(
            ((style.weight - asset.admission.weight) as f64 / 500.0 * f64::from(style.size) * 0.04)
                .min(f64::from(style.size) * 0.08),
        );
    let mut decorations = Vec::with_capacity(2);
    let metrics = font.metrics(Size::new(style.size), &location);
    if glyph.advance > 0.0 {
        for (enabled, metric, fallback) in [
            (style.underline, metrics.underline, -style.size * 0.1),
            (style.strike, metrics.strikeout, style.size * 0.35),
        ] {
            if enabled {
                let offset = metric.map_or(fallback, |metric| metric.offset);
                let thickness = metric.map_or((style.size * 0.06).max(1.0), |metric| {
                    metric.thickness.max(1.0)
                });
                let area = Rect::new(
                    glyph.pen as f64,
                    (glyph.baseline - offset) as f64,
                    glyph.advance as f64,
                    thickness as f64,
                );
                area.validate()?;
                decorations.push(area);
            }
        }
    }
    if glyph.id == 0 && glyph.empty_allowed {
        return Ok(GlyphInk {
            path: None,
            bold,
            decorations,
        });
    }
    let outlines = font.outline_glyphs();
    let Some(outline) = outlines.get(skrifa::GlyphId::new(glyph.id)) else {
        ensure!(
            glyph.empty_allowed,
            "Editor font glyph has no scalable outline"
        );
        return Ok(GlyphInk {
            path: None,
            bold,
            decorations,
        });
    };
    let size = outline.draw_memory_size(Hinting::None);
    ensure!(
        size <= 1024 * 1024,
        "Editor glyph outline scratch exceeds 1 MiB"
    );
    let mut memory = Vec::with_capacity(size);
    let zeroes = [0u8; 16384];
    while memory.len() < size {
        let count = (size - memory.len()).min(zeroes.len());
        work.extend(&mut memory, &zeroes[..count])?;
    }
    let mut pen = Pen {
        work,
        path: tiny_skia::PathBuilder::new(),
        origin: [glyph.x, glyph.y],
        skew: if style.italic && !asset.admission.italic && !asset.admission.variable_italic {
            0.249328
        } else {
            0.0
        },
        current: [0.0; 2],
        start: [0.0; 2],
        segments: 0,
        error: None,
    };
    outline
        .draw(
            DrawSettings::unhinted(Size::new(style.size), &location).with_memory(Some(&mut memory)),
            &mut pen,
        )
        .map_err(|_| anyhow::anyhow!("Editor glyph outline preparation failed"))?;
    if let Some(error) = pen.error {
        return Err(error);
    }
    let path = pen.path.finish();
    ensure!(
        path.is_some() || glyph.empty_allowed,
        "Editor font glyph has no visible outline"
    );
    Ok(GlyphInk {
        path,
        bold,
        decorations,
    })
}
fn lines(
    layout: &Layout<'_>,
    text: &RichText,
    size: [u32; 2],
    work: &mut ImageWork<'_, impl Fn() -> bool>,
) -> Result<Vec<Glyph>> {
    let mut placed = Vec::new();
    let mut baseline = 0.0f32;
    let mut line_count = 0;
    let mut paragraph_start = 0;
    for paragraph in layout.text.split('\n') {
        let paragraph_end = paragraph_start + paragraph.len();
        let mut start = paragraph_start;
        let mut previous = start;
        let mut append = |line: Line, work: &mut ImageWork<'_, _>| -> Result<()> {
            ensure!(
                line.width <= size[0] as f32 + 0.01,
                "Editor text has an unbreakable line wider than its bounds"
            );
            line_count += 1;
            ensure!(line_count <= 64, "Editor wrapped text exceeds 64 lines");
            let height = line.height.max(1.0);
            ensure!(
                baseline + height <= size[1] as f32 + 0.01,
                "Editor text is taller than its bounds"
            );
            let x = match text.alignment {
                Alignment::Left => 0.0,
                Alignment::Center => (size[0] as f32 - line.width) / 2.0,
                Alignment::Right => size[0] as f32 - line.width,
            };
            ensure!(
                placed
                    .len()
                    .checked_add(line.glyphs.len())
                    .is_some_and(|count| count <= 8192),
                "Editor text exceeds 8192 placed glyphs"
            );
            for mut glyph in line.glyphs {
                work.charge(40)?;
                glyph.x += x;
                glyph.pen += x;
                glyph.baseline = baseline + line.ascent;
                glyph.y += glyph.baseline;
                placed.push(glyph);
            }
            baseline += height;
            Ok(())
        };
        for (end, _) in unicode_linebreak::linebreaks(paragraph) {
            let end = paragraph_start + end;
            let candidate = layout.shape(start..end, text.line_height, work)?;
            if candidate.width > size[0] as f32 + 0.01 {
                ensure!(
                    previous > start,
                    "Editor text has an unbreakable line wider than its bounds"
                );
                append(layout.shape(start..previous, text.line_height, work)?, work)?;
                start = previous;
            }
            previous = end;
        }
        if start < paragraph_end || paragraph.is_empty() {
            append(
                layout.shape(start..paragraph_end, text.line_height, work)?,
                work,
            )?;
        }
        paragraph_start = paragraph_end + 1;
    }
    Ok(placed)
}
pub(super) fn admit_frame(size: [u32; 2]) -> Result<()> {
    ensure!(
        size[0] > 0
            && size[1] > 0
            && size[0] <= 2048
            && size[1] <= 1024
            && u64::from(size[0]) * u64::from(size[1]) * 4 <= MAX_TEXT_RASTER as u64,
        "Editor text raster exceeds 1 MiB or its dimension bounds"
    );
    Ok(())
}

pub(super) fn prepare(
    text: &RichText,
    assets: &EditorAssets,
    size: [u32; 2],
    work: &mut ImageWork<'_, impl Fn() -> bool>,
) -> Result<RgbaImage> {
    text.validate()?;
    admit_frame(size)?;
    let layout = Layout::new(text, assets);
    let glyphs = lines(&layout, text, size, work)?;
    let mut bounds: Option<Rect> = None;
    let mut include = |area: Rect| {
        bounds = Some(if let Some(old) = bounds {
            let left = old.x.min(area.x);
            let top = old.y.min(area.y);
            let right = (old.x + old.width).max(area.x + area.width);
            let bottom = (old.y + old.height).max(area.y + area.height);
            Rect::new(left, top, right - left, bottom - top)
        } else {
            area
        });
    };
    for glyph in &glyphs {
        let ink = glyph_ink(*glyph, &layout, work)?;
        if let Some(path) = &ink.path {
            let area = path.bounds();
            let extra = ink.bold.unwrap_or(0.0) / 2.0;
            include(Rect::new(
                area.left() as f64 - extra,
                area.top() as f64 - extra,
                area.width() as f64 + extra * 2.0,
                area.height() as f64 + extra * 2.0,
            ));
        }
        for area in ink.decorations {
            include(area);
        }
    }
    let bounds = bounds.context("Editor text has no visible glyphs")?;
    ensure!(
        bounds.width <= size[0] as f64 && bounds.height <= size[1] as f64,
        "Editor text ink exceeds its frame"
    );
    let shift = [
        0.0f64.clamp(-bounds.x, size[0] as f64 - bounds.x - bounds.width),
        0.0f64.clamp(-bounds.y, size[1] as f64 - bounds.y - bounds.height),
    ];
    let mut output = render::rgba(size, work)?;
    let mut coverage = render::rgba(size, work)?;
    let area = PixelRegion {
        x: 0,
        y: 0,
        width: size[0],
        height: size[1],
    };
    for (span_index, span) in text.spans.iter().enumerate() {
        if span.style.color[3] == 0 {
            continue;
        }
        for bytes in coverage.as_mut().chunks_mut(16384) {
            work.charge(bytes.len())?;
            bytes.fill(0);
        }
        for glyph in glyphs
            .iter()
            .copied()
            .filter(|glyph| glyph.style == span_index)
        {
            let ink = glyph_ink(glyph, &layout, work)?;
            if let Some(path) = ink.path {
                render::vector(
                    &mut coverage,
                    render::VectorInk {
                        path: &path,
                        transform: Affine::translate(shift[0], shift[1]),
                        color: [255; 4],
                        stroke: ink.bold,
                        fill: true,
                        opacity: 255,
                    },
                    area,
                    work,
                )?;
            }
            for decoration in ink.decorations {
                let path = tiny_skia::PathBuilder::from_rect(
                    tiny_skia::Rect::from_xywh(
                        decoration.x as f32,
                        decoration.y as f32,
                        decoration.width as f32,
                        decoration.height as f32,
                    )
                    .context("Invalid text decoration")?,
                );
                render::vector(
                    &mut coverage,
                    render::VectorInk {
                        path: &path,
                        transform: Affine::translate(shift[0], shift[1]),
                        color: [255; 4],
                        stroke: None,
                        fill: true,
                        opacity: 255,
                    },
                    area,
                    work,
                )?;
            }
        }
        for (destination, mask) in output.pixels_mut().zip(coverage.pixels()) {
            work.pixel()?;
            let mut color = span.style.color;
            color[3] = ((u32::from(color[3]) * u32::from(mask[3]) + 127) / 255) as u8;
            render::blend(destination, color, 255);
        }
    }
    work.check()?;
    Ok(output)
}

#[cfg(test)]
#[path = "editor_document_text_tests.rs"]
mod tests;
