use anyhow::{Result, ensure};
use serde::{Deserialize, Serialize};

pub const MAX_LAYERS: usize = 100;
pub const MAX_POINTS: usize = 8192;
pub const MAX_METADATA: usize = 4 * 1024 * 1024;
pub const MAX_ASSET_BYTES: u64 = 256 * 1024 * 1024;
pub const MAX_FONT_BYTES: usize = 8 * 1024 * 1024;
pub const MAX_TEXT_RASTER: usize = 1024 * 1024;

#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Rect {
    pub x: f64,
    pub y: f64,
    pub width: f64,
    pub height: f64,
}
impl Rect {
    pub fn new(x: f64, y: f64, width: f64, height: f64) -> Self {
        Self {
            x,
            y,
            width,
            height,
        }
    }
    pub fn validate(self) -> Result<()> {
        ensure!(
            [self.x, self.y, self.width, self.height]
                .iter()
                .all(|value| value.is_finite() && value.abs() <= 1_048_576.0)
                && self.width > 0.0
                && self.height > 0.0,
            "Invalid editor rectangle"
        );
        Ok(())
    }
    pub fn contains(self, point: [f64; 2]) -> bool {
        point[0] >= self.x
            && point[1] >= self.y
            && point[0] < self.x + self.width
            && point[1] < self.y + self.height
    }
    pub fn corners(self) -> [[f64; 2]; 4] {
        [
            [self.x, self.y],
            [self.x + self.width, self.y],
            [self.x + self.width, self.y + self.height],
            [self.x, self.y + self.height],
        ]
    }
    pub fn from_points(points: impl IntoIterator<Item = [f64; 2]>) -> Result<Self> {
        let mut min = [f64::INFINITY; 2];
        let mut max = [f64::NEG_INFINITY; 2];
        for point in points {
            for axis in 0..2 {
                min[axis] = min[axis].min(point[axis]);
                max[axis] = max[axis].max(point[axis]);
            }
        }
        let value = Self::new(min[0], min[1], max[0] - min[0], max[1] - min[1]);
        value.validate()?;
        Ok(value)
    }
}

/// Column-vector affine: [a*x+c*y+tx, b*x+d*y+ty].
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(transparent)]
pub struct Affine(pub [f64; 6]);
impl Default for Affine {
    fn default() -> Self {
        Self::IDENTITY
    }
}
impl Affine {
    pub const IDENTITY: Self = Self([1.0, 0.0, 0.0, 1.0, 0.0, 0.0]);
    pub fn translate(x: f64, y: f64) -> Self {
        Self([1.0, 0.0, 0.0, 1.0, x, y])
    }
    pub fn scale(x: f64, y: f64) -> Self {
        Self([x, 0.0, 0.0, y, 0.0, 0.0])
    }
    pub fn rotate(degrees: f64, pivot: [f64; 2]) -> Self {
        let (sin, cos) = match degrees.rem_euclid(360.0) {
            0.0 => (0.0, 1.0),
            90.0 => (1.0, 0.0),
            180.0 => (0.0, -1.0),
            270.0 => (-1.0, 0.0),
            _ => degrees.to_radians().sin_cos(),
        };
        Self::translate(pivot[0], pivot[1])
            .then(Self([cos, sin, -sin, cos, 0.0, 0.0]))
            .then(Self::translate(-pivot[0], -pivot[1]))
    }
    /// self * local: apply local first, then self.
    pub fn then(self, local: Self) -> Self {
        let [a, b, c, d, x, y] = self.0;
        let [e, f, g, h, u, v] = local.0;
        Self([
            a * e + c * f,
            b * e + d * f,
            a * g + c * h,
            b * g + d * h,
            a * u + c * v + x,
            b * u + d * v + y,
        ])
    }
    pub fn point(self, p: [f64; 2]) -> [f64; 2] {
        let [a, b, c, d, x, y] = self.0;
        [a * p[0] + c * p[1] + x, b * p[0] + d * p[1] + y]
    }
    pub fn inverse(self) -> Result<Self> {
        self.validate()?;
        let [a, b, c, d, x, y] = self.0;
        let determinant = a * d - b * c;
        Ok(Self([
            d / determinant,
            -b / determinant,
            -c / determinant,
            a / determinant,
            (c * y - d * x) / determinant,
            (b * x - a * y) / determinant,
        ]))
    }
    pub fn validate(self) -> Result<()> {
        let [a, b, c, d, _, _] = self.0;
        let determinant = a * d - b * c;
        ensure!(
            self.0
                .iter()
                .all(|value| value.is_finite() && value.abs() <= 1_048_576.0)
                && determinant.is_finite()
                && determinant.abs() >= 1e-8
                && [a, b, c, d].iter().all(|value| value.abs() <= 1024.0),
            "Invalid or singular editor transform"
        );
        Ok(())
    }
    pub fn bounds(self, rect: Rect) -> Result<Rect> {
        self.validate()?;
        Rect::from_points(rect.corners().map(|point| self.point(point)))
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[derive(Default)]
pub enum Interpolation {
    #[default]
    Nearest,
    Bilinear,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AssetKind {
    Image,
    Logo,
    Clipboard,
    Emoji,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EraseMode {
    Transparent,
    RestoreBase,
}
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EffectInput {
    #[default]
    Previous,
    ProtectedBase,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[derive(Default)]
pub enum Alignment {
    #[default]
    Left,
    Center,
    Right,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Style {
    pub fill: Option<[u8; 4]>,
    pub stroke: Option<[u8; 4]>,
    pub width: f64,
}
impl Default for Style {
    fn default() -> Self {
        Self {
            fill: None,
            stroke: Some([239, 68, 68, 255]),
            width: 3.0,
        }
    }
}
impl Style {
    pub fn validate(&self) -> Result<()> {
        ensure!(
            self.width.is_finite()
                && self.width >= 0.0
                && self.width <= 256.0
                && (self.fill.is_some() || self.stroke.is_some() && self.width > 0.0),
            "Invalid editor fill/stroke style"
        );
        Ok(())
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "shape", rename_all = "snake_case", deny_unknown_fields)]
pub enum Shape {
    Rectangle,
    Ellipse,
    Arrow {
        start: [f64; 2],
        end: [f64; 2],
    },
    Path {
        points: Vec<[f64; 2]>,
        #[serde(default)]
        closed: bool,
    },
    SpeechBubble {
        tail: [f64; 2],
        corner: f64,
    },
}
impl Shape {
    pub(super) fn points(&self) -> usize {
        match self {
            Self::Path { points, .. } => points.len(),
            _ => 0,
        }
    }
    pub(super) fn validate(&self) -> Result<()> {
        let valid = |p: &[f64; 2]| {
            p.iter()
                .all(|value| value.is_finite() && value.abs() <= 1_048_576.0)
        };
        ensure!(
            match self {
                Self::Arrow { start, end } => valid(start) && valid(end) && start != end,
                Self::Path { points, .. } =>
                    !points.is_empty() && points.len() <= MAX_POINTS && points.iter().all(valid),
                Self::SpeechBubble { tail, corner } =>
                    valid(tail) && corner.is_finite() && *corner >= 0.0 && *corner <= 1024.0,
                _ => true,
            },
            "Invalid editor shape geometry"
        );
        Ok(())
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TextStyle {
    pub fonts: Vec<String>,
    pub size: f32,
    #[serde(default = "regular_weight")]
    pub weight: u16,
    #[serde(default)]
    pub italic: bool,
    #[serde(default)]
    pub underline: bool,
    #[serde(default)]
    pub strike: bool,
    pub color: [u8; 4],
}
fn regular_weight() -> u16 {
    400
}
impl TextStyle {
    pub(super) fn validate(&self) -> Result<()> {
        ensure!(
            !self.fonts.is_empty()
                && self.fonts.len() <= 8
                && self.size.is_finite()
                && (4.0..=256.0).contains(&self.size)
                && (100..=900).contains(&self.weight),
            "Invalid editor text style"
        );
        for id in &self.fonts {
            asset_id(id)?;
        }
        Ok(())
    }
}
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TextSpan {
    pub text: String,
    pub style: TextStyle,
}
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RichText {
    pub spans: Vec<TextSpan>,
    #[serde(default)]
    pub alignment: Alignment,
    #[serde(default = "default_line_height")]
    pub line_height: f32,
}
fn default_line_height() -> f32 {
    1.25
}
impl RichText {
    pub(super) fn bytes(&self) -> usize {
        self.spans.iter().map(|span| span.text.len()).sum()
    }
    pub(super) fn validate(&self) -> Result<()> {
        ensure!(
            !self.spans.is_empty()
                && self.spans.len() <= 32
                && self.bytes() <= 4096
                && self.line_height.is_finite()
                && (1.0..=4.0).contains(&self.line_height),
            "Editor text exceeds its bounds"
        );
        let mut lines = 1;
        let mut visible = false;
        for span in &self.spans {
            span.style.validate()?;
            for character in span.text.chars() {
                ensure!(
                    !character.is_control() || matches!(character, '\n' | '\t'),
                    "Invalid editor text control"
                );
                lines += usize::from(character == '\n');
                visible |= !character.is_whitespace();
            }
        }
        ensure!(
            lines <= 64 && visible,
            "Editor text must contain visible bounded lines"
        );
        Ok(())
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "tool", rename_all = "snake_case", deny_unknown_fields)]
pub enum LayerKind {
    #[serde(skip)]
    Legacy {
        annotation: super::LegacyAnnotation,
    },
    Draw {
        shape: Shape,
        style: Style,
    },
    Highlight {
        color: [u8; 4],
        #[serde(default)]
        outside: bool,
    },
    Raster {
        asset: String,
        #[serde(default)]
        interpolation: Interpolation,
        kind: AssetKind,
    },
    Text {
        text: RichText,
    },
    Blur {
        radius: u32,
        #[serde(default)]
        input: EffectInput,
    },
    Pixelate {
        block: u32,
        #[serde(default)]
        input: EffectInput,
    },
    Clone {
        source: Affine,
        #[serde(default)]
        interpolation: Interpolation,
    },
    Magnifier {
        source: Affine,
        #[serde(default)]
        interpolation: Interpolation,
        style: Style,
    },
    Erase {
        mode: EraseMode,
        shape: Shape,
        #[serde(default = "eraser_width")]
        width: f64,
    },
    Redaction,
}
impl LayerKind {
    pub(super) fn validate(&self) -> Result<()> {
        match self {
            Self::Draw { shape, style } => {
                shape.validate()?;
                style.validate()?;
            }
            Self::Raster { asset, .. } => asset_id(asset)?,
            Self::Text { text } => text.validate()?,
            Self::Blur { radius, .. } => ensure!(
                (1..=64).contains(radius),
                "Editor blur radius must be 1..64"
            ),
            Self::Pixelate { block, .. } => ensure!(
                (2..=128).contains(block),
                "Editor pixelation block must be 2..128"
            ),
            Self::Clone { source, .. } => source.validate()?,
            Self::Magnifier { source, style, .. } => {
                source.validate()?;
                style.validate()?;
            }
            Self::Erase { shape, width, .. } => {
                shape.validate()?;
                ensure!(
                    matches!(
                        shape,
                        Shape::Rectangle | Shape::Ellipse | Shape::Path { .. }
                    ),
                    "Erasers require rectangle, ellipse or path geometry"
                );
                ensure!(
                    width.is_finite() && (1.0..=256.0).contains(width),
                    "Invalid editor eraser width"
                );
            }
            _ => {}
        }
        Ok(())
    }
    pub(super) fn points(&self) -> usize {
        match self {
            Self::Draw { shape, .. } | Self::Erase { shape, .. } => shape.points(),
            Self::Legacy { annotation } => annotation.pen_points(),
            _ => 0,
        }
    }
    pub(super) fn text_bytes(&self) -> usize {
        match self {
            Self::Text { text } => text.bytes(),
            _ => 0,
        }
    }
    pub(super) fn source_transform(&mut self, transform: Affine) {
        match self {
            Self::Clone { source, .. } | Self::Magnifier { source, .. } => {
                *source = transform.then(*source)
            }
            _ => {}
        }
    }
}
fn opaque() -> u8 {
    255
}
fn eraser_width() -> f64 {
    12.0
}
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Layer {
    pub id: u64,
    pub bounds: Rect,
    #[serde(default)]
    pub transform: Affine,
    #[serde(default = "opaque")]
    pub opacity: u8,
    pub content: LayerKind,
}
impl Layer {
    pub(super) fn validate(&self) -> Result<()> {
        ensure!(self.id > 0, "Editor layer IDs must be nonzero");
        self.bounds.validate()?;
        self.transform.validate()?;
        self.transform.bounds(self.bounds)?.validate()?;
        self.content.validate()?;
        if let LayerKind::Legacy { annotation } = &self.content {
            ensure!(
                self.bounds == annotation.bounds(),
                "Legacy intrinsic bounds are immutable; use its affine transform"
            );
        }
        ensure!(
            !matches!(self.content, LayerKind::Redaction) || self.opacity == 255,
            "Opaque redactions cannot have translucent opacity"
        );
        Ok(())
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[derive(Default)]
pub enum ExportFormat {
    #[default]
    Png,
    Jpeg,
    Webp,
    WebpLossy,
}
impl ExportFormat {
    pub(super) fn validate_output_size(self, width: u32, height: u32) -> Result<()> {
        match self {
            Self::Png => crate::ImageExportFormat::Png.validate_output_size(width, height),
            Self::Jpeg => crate::ImageExportFormat::Jpeg.validate_output_size(width, height),
            Self::Webp => crate::ImageExportFormat::WebP.validate_output_size(width, height),
            Self::WebpLossy => crate::webp_lossy::validate_output_size(width, height),
        }?;
        Ok(())
    }
}
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Shadow {
    pub radius: u32,
    pub offset: [i32; 2],
    pub color: [u8; 4],
}
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Watermark {
    pub asset: String,
    pub area: Rect,
    #[serde(default = "opaque")]
    pub opacity: u8,
}
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ExportPreset {
    #[serde(default)]
    pub format: ExportFormat,
    #[serde(default = "default_quality")]
    pub quality: u8,
    #[serde(default)]
    pub padding: [u32; 4],
    #[serde(default)]
    pub background: [u8; 4],
    #[serde(default)]
    pub shadow: Option<Shadow>,
    #[serde(default)]
    pub watermark: Option<Watermark>,
}
fn default_quality() -> u8 {
    90
}
impl Default for ExportPreset {
    fn default() -> Self {
        Self {
            format: ExportFormat::Png,
            quality: 90,
            padding: [0; 4],
            background: [0; 4],
            shadow: None,
            watermark: None,
        }
    }
}
impl ExportPreset {
    pub fn validate(&self) -> Result<()> {
        ensure!(
            (1..=100).contains(&self.quality) && self.padding.iter().all(|value| *value <= 8192),
            "Invalid editor export preset"
        );
        if let Some(shadow) = &self.shadow {
            ensure!(
                shadow.radius <= 64
                    && shadow
                        .offset
                        .iter()
                        .all(|value| value.unsigned_abs() <= 8192),
                "Invalid editor shadow"
            );
        }
        if let Some(watermark) = &self.watermark {
            asset_id(&watermark.asset)?;
            watermark.area.validate()?;
        }
        Ok(())
    }
    pub fn validate_extension(&self, path: &std::path::Path) -> Result<()> {
        let extension = path
            .extension()
            .and_then(std::ffi::OsStr::to_str)
            .unwrap_or("");
        ensure!(
            match self.format {
                ExportFormat::Png => extension.eq_ignore_ascii_case("png"),
                ExportFormat::Jpeg =>
                    extension.eq_ignore_ascii_case("jpg") || extension.eq_ignore_ascii_case("jpeg"),
                ExportFormat::Webp | ExportFormat::WebpLossy =>
                    extension.eq_ignore_ascii_case("webp"),
            },
            "Editor export preset conflicts with the output extension"
        );
        Ok(())
    }
}
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ToolPreset {
    pub name: String,
    pub content: LayerKind,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct NamedExportPreset {
    pub name: String,
    pub preset: ExportPreset,
}
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PresetBook {
    pub version: u32,
    #[serde(default)]
    pub tools: Vec<ToolPreset>,
    #[serde(default)]
    pub exports: Vec<NamedExportPreset>,
}
impl Default for PresetBook {
    fn default() -> Self {
        Self {
            version: 1,
            tools: Vec::new(),
            exports: Vec::new(),
        }
    }
}
impl PresetBook {
    pub fn validate(&self) -> Result<()> {
        ensure!(
            self.version == 1 && self.tools.len() <= 32 && self.exports.len() <= 32,
            "Invalid or excessive editor preset collection"
        );
        let mut names = std::collections::BTreeSet::new();
        for preset in &self.tools {
            preset.validate()?;
            ensure!(
                names.insert(preset.name.as_str()),
                "Duplicate editor tool preset name"
            );
        }
        names.clear();
        for preset in &self.exports {
            ensure!(
                !preset.name.is_empty()
                    && preset.name.len() <= 64
                    && !preset.name.chars().any(char::is_control),
                "Invalid editor export preset name"
            );
            ensure!(
                names.insert(preset.name.as_str()),
                "Duplicate editor export preset name"
            );
            preset.preset.validate()?;
        }
        Ok(())
    }
    pub fn read_json(bytes: &[u8]) -> Result<Self> {
        ensure!(
            bytes.len() <= 256 * 1024,
            "Editor preset collection exceeds 256 KiB"
        );
        let value: Self = serde_json::from_slice(bytes)
            .map_err(|_| anyhow::anyhow!("Invalid strict editor preset collection"))?;
        value.validate()?;
        Ok(value)
    }
    pub fn apply_tool(
        &self,
        name: &str,
        document: &mut super::EditorDocument,
        id: u64,
    ) -> Result<bool> {
        self.validate()?;
        let preset = self
            .tools
            .iter()
            .find(|preset| preset.name == name)
            .ok_or_else(|| anyhow::anyhow!("Unknown editor tool preset"))?;
        document.apply_preset(id, preset)
    }
    pub fn apply_export(&self, name: &str, document: &mut super::EditorDocument) -> Result<bool> {
        self.validate()?;
        let preset = self
            .exports
            .iter()
            .find(|preset| preset.name == name)
            .ok_or_else(|| anyhow::anyhow!("Unknown editor export preset"))?;
        document.set_export(preset.preset.clone())
    }
}
impl ToolPreset {
    pub fn validate(&self) -> Result<()> {
        ensure!(
            !matches!(self.content, LayerKind::Legacy { .. }),
            "Programmatic legacy annotations cannot be serialized as tool presets"
        );
        ensure!(
            !self.name.is_empty()
                && self.name.len() <= 64
                && !self.name.chars().any(char::is_control),
            "Invalid editor preset name"
        );
        self.content.validate()
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CanvasClip {
    pub bounds: Rect,
    pub transform: Affine,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DocumentSpec {
    pub version: u32,
    pub canvas: [u32; 2],
    #[serde(default)]
    pub base: Affine,
    #[serde(default)]
    pub interpolation: Interpolation,
    #[serde(default)]
    pub crop: Option<crate::PixelRegion>,
    #[serde(default)]
    pub clip: Option<CanvasClip>,
    #[serde(default)]
    pub layers: Vec<Layer>,
    #[serde(default)]
    pub export: ExportPreset,
}
impl DocumentSpec {
    pub fn new(width: u32, height: u32) -> Self {
        Self {
            version: 1,
            canvas: [width, height],
            base: Affine::IDENTITY,
            interpolation: Interpolation::Nearest,
            crop: None,
            clip: None,
            layers: Vec::new(),
            export: ExportPreset::default(),
        }
    }
    pub fn validate(&self) -> Result<()> {
        ensure!(self.version == 1, "Unsupported editor document version");
        crate::validate_image_size(self.canvas[0], self.canvas[1])?;
        self.base.validate()?;
        self.export.validate()?;
        if let Some(clip) = &self.clip {
            clip.bounds.validate()?;
            clip.transform.validate()?;
            ensure!(
                self.crop.is_some(),
                "A transformed crop requires an output crop"
            );
        }
        ensure!(
            self.layers.len() <= MAX_LAYERS,
            "Editor document exceeds 100 layers"
        );
        let mut ids = std::collections::BTreeSet::new();
        let mut points = 0;
        let mut text = 0;
        for layer in &self.layers {
            layer.validate()?;
            ensure!(ids.insert(layer.id), "Duplicate editor layer ID");
            points += layer.content.points();
            text += layer.content.text_bytes();
        }
        ensure!(
            points <= MAX_POINTS && text <= 32768,
            "Editor geometry/text budget exceeded"
        );
        if let Some(crop) = self.crop {
            ensure!(
                crop.width > 0
                    && crop.height > 0
                    && crop
                        .x
                        .checked_add(crop.width)
                        .is_some_and(|end| end <= self.canvas[0])
                    && crop
                        .y
                        .checked_add(crop.height)
                        .is_some_and(|end| end <= self.canvas[1]),
                "Editor crop is outside its canvas"
            );
        }
        Ok(())
    }
    pub fn read_json(bytes: &[u8]) -> Result<Self> {
        ensure!(
            bytes.len() <= 256 * 1024,
            "Editor document spec exceeds 256 KiB"
        );
        let value: Self = serde_json::from_slice(bytes)
            .map_err(|_| anyhow::anyhow!("Invalid strict editor document spec"))?;
        value.validate()?;
        Ok(value)
    }
    pub fn metadata_bytes(&self) -> usize {
        // Count retained Vec/String capacities, independently of shared pixel/font backing.
        std::mem::size_of::<Self>()
            + self.layers.capacity() * std::mem::size_of::<Layer>()
            + self
                .layers
                .iter()
                .map(|layer| match &layer.content {
                    LayerKind::Draw {
                        shape: Shape::Path { points, .. },
                        ..
                    }
                    | LayerKind::Erase {
                        shape: Shape::Path { points, .. },
                        ..
                    } => points.capacity() * 16,
                    LayerKind::Raster { asset, .. } => asset.capacity(),
                    LayerKind::Text { text } => {
                        text.spans.capacity() * std::mem::size_of::<TextSpan>()
                            + text
                                .spans
                                .iter()
                                .map(|span| {
                                    span.text.capacity()
                                        + span.style.fonts.capacity()
                                            * std::mem::size_of::<String>()
                                        + span
                                            .style
                                            .fonts
                                            .iter()
                                            .map(String::capacity)
                                            .sum::<usize>()
                                })
                                .sum::<usize>()
                    }
                    LayerKind::Legacy { annotation } => annotation.metadata_bytes(),
                    _ => 0,
                })
                .sum::<usize>()
            + self
                .export
                .watermark
                .as_ref()
                .map_or(0, |watermark| watermark.asset.capacity())
    }
}
pub(super) fn asset_id(value: &str) -> Result<()> {
    ensure!(
        !value.is_empty()
            && value.len() <= 64
            && value
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.'))
            && !matches!(value, "source" | "base"),
        "Invalid or reserved editor asset ID"
    );
    Ok(())
}
