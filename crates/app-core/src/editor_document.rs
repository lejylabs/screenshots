//! Immutable source/assets, metadata-only editing history and one shared renderer.
use crate::CapturedImage;
use anyhow::{Context, Result, ensure};
use image::RgbaImage;
use std::{
    collections::{BTreeMap, BTreeSet, VecDeque},
    sync::{
        Arc, Mutex, Weak,
        atomic::{AtomicU64, AtomicUsize, Ordering},
    },
};
#[path = "editor_document_types.rs"]
mod types;
pub use types::*;
#[path = "editor_document_legacy.rs"]
mod legacy;
#[path = "editor_document_render.rs"]
mod render;
#[path = "editor_document_text.rs"]
mod text;
pub use legacy::LegacyAnnotation;

#[derive(Default)]
struct Counters {
    raster: AtomicU64,
    fonts: AtomicU64,
    images: AtomicUsize,
    font_sources: AtomicUsize,
    mask_bytes: AtomicU64,
    masks: AtomicUsize,
}
enum LeaseKind {
    Raster,
    Font,
    Mask,
}
struct Lease {
    counters: Arc<Counters>,
    bytes: u64,
    kind: LeaseKind,
}
impl Drop for Lease {
    fn drop(&mut self) {
        if matches!(self.kind, LeaseKind::Font) {
            self.counters.fonts.fetch_sub(self.bytes, Ordering::AcqRel);
            self.counters.font_sources.fetch_sub(1, Ordering::AcqRel);
        } else {
            self.counters.raster.fetch_sub(self.bytes, Ordering::AcqRel);
            if matches!(self.kind, LeaseKind::Raster) {
                self.counters.images.fetch_sub(1, Ordering::AcqRel);
            } else {
                self.counters
                    .mask_bytes
                    .fetch_sub(self.bytes, Ordering::AcqRel);
                self.counters.masks.fetch_sub(1, Ordering::AcqRel);
            }
        }
    }
}
struct RasterData {
    pixels: Arc<RgbaImage>,
    _lease: Lease,
}
struct FontData {
    bytes: Arc<[u8]>,
    _lease: Lease,
}
struct MaskData {
    _bytes: Arc<[u8]>,
    _lease: Lease,
}
#[derive(Clone)]
struct FontAsset {
    data: Arc<FontData>,
    index: u32,
    admission: text::FontAdmission,
}
#[derive(Default)]
struct Registry {
    rasters: BTreeMap<usize, Weak<RasterData>>,
    fonts: BTreeMap<usize, Weak<FontData>>,
    masks: BTreeMap<usize, Weak<MaskData>>,
}
#[derive(Default)]
struct Pool {
    counters: Arc<Counters>,
    registry: Mutex<Registry>,
}
impl Pool {
    fn raster(&self, pixels: Arc<RgbaImage>) -> Result<Arc<RasterData>> {
        crate::validate_image_size(pixels.width(), pixels.height())?;
        let bytes = u64::from(pixels.width()) * u64::from(pixels.height()) * 4;
        ensure!(
            pixels.as_raw().len() as u64 == bytes,
            "Editor raster backing does not match its dimensions"
        );
        let key = Arc::as_ptr(&pixels) as usize;
        let mut registry = self
            .registry
            .lock()
            .map_err(|_| anyhow::anyhow!("Editor asset pool unavailable"))?;
        registry.rasters.retain(|_, value| value.strong_count() > 0);
        if let Some(data) = registry.rasters.get(&key).and_then(Weak::upgrade) {
            return Ok(data);
        }
        ensure!(
            self.counters
                .raster
                .load(Ordering::Acquire)
                .checked_add(bytes)
                .is_some_and(|sum| sum <= MAX_ASSET_BYTES)
                && self.counters.images.load(Ordering::Acquire) < 64,
            "Editor retained raster assets exceed their budget"
        );
        self.counters.raster.fetch_add(bytes, Ordering::AcqRel);
        self.counters.images.fetch_add(1, Ordering::AcqRel);
        let data = Arc::new(RasterData {
            pixels,
            _lease: Lease {
                counters: self.counters.clone(),
                bytes,
                kind: LeaseKind::Raster,
            },
        });
        registry.rasters.insert(key, Arc::downgrade(&data));
        Ok(data)
    }
    fn font(&self, bytes: Arc<[u8]>) -> Result<Arc<FontData>> {
        ensure!(
            !bytes.is_empty() && bytes.len() <= 2 * 1024 * 1024,
            "Editor font source exceeds 2 MiB"
        );
        let key = bytes.as_ptr() as usize;
        let length = bytes.len() as u64;
        let mut registry = self
            .registry
            .lock()
            .map_err(|_| anyhow::anyhow!("Editor asset pool unavailable"))?;
        registry.fonts.retain(|_, value| value.strong_count() > 0);
        if let Some(data) = registry.fonts.get(&key).and_then(Weak::upgrade) {
            ensure!(
                data.bytes.len() == bytes.len(),
                "Editor font backing extent changed"
            );
            return Ok(data);
        }
        ensure!(
            self.counters
                .fonts
                .load(Ordering::Acquire)
                .checked_add(length)
                .is_some_and(|sum| sum <= MAX_FONT_BYTES as u64)
                && self.counters.font_sources.load(Ordering::Acquire) < 64,
            "Editor retained font sources exceed their byte/count budget"
        );
        self.counters.fonts.fetch_add(length, Ordering::AcqRel);
        self.counters.font_sources.fetch_add(1, Ordering::AcqRel);
        let data = Arc::new(FontData {
            bytes,
            _lease: Lease {
                counters: self.counters.clone(),
                bytes: length,
                kind: LeaseKind::Font,
            },
        });
        registry.fonts.insert(key, Arc::downgrade(&data));
        Ok(data)
    }
    fn mask(&self, bytes: Arc<[u8]>) -> Result<Arc<MaskData>> {
        let key = bytes.as_ptr() as usize;
        let length = bytes.len() as u64;
        let mut registry = self
            .registry
            .lock()
            .map_err(|_| anyhow::anyhow!("Editor asset pool unavailable"))?;
        registry.masks.retain(|_, value| value.strong_count() > 0);
        if let Some(data) = registry.masks.get(&key).and_then(Weak::upgrade) {
            ensure!(
                data._bytes.len() == bytes.len(),
                "Editor mask backing extent changed"
            );
            return Ok(data);
        }
        ensure!(
            self.counters
                .raster
                .load(Ordering::Acquire)
                .checked_add(length)
                .is_some_and(|sum| sum <= MAX_ASSET_BYTES)
                && self
                    .counters
                    .mask_bytes
                    .load(Ordering::Acquire)
                    .checked_add(length)
                    .is_some_and(|sum| sum <= 4 * 1024 * 1024)
                && self.counters.masks.load(Ordering::Acquire) < 1024,
            "Legacy editor masks exceed their retained byte/count budget"
        );
        self.counters.raster.fetch_add(length, Ordering::AcqRel);
        self.counters.mask_bytes.fetch_add(length, Ordering::AcqRel);
        self.counters.masks.fetch_add(1, Ordering::AcqRel);
        let data = Arc::new(MaskData {
            _bytes: bytes,
            _lease: Lease {
                counters: self.counters.clone(),
                bytes: length,
                kind: LeaseKind::Mask,
            },
        });
        registry.masks.insert(key, Arc::downgrade(&data));
        Ok(data)
    }
}

/// An immutable snapshot retains the same pixel/font leases after later edits or GC.
#[derive(Clone, Default)]
pub struct EditorAssets {
    pool: Arc<Pool>,
    rasters: BTreeMap<String, Arc<RasterData>>,
    fonts: BTreeMap<String, FontAsset>,
}
impl EditorAssets {
    pub fn new() -> Self {
        Self::default()
    }
    pub fn insert_raster(&mut self, id: &str, pixels: Arc<RgbaImage>) -> Result<()> {
        types::asset_id(id)?;
        ensure!(
            !self.rasters.contains_key(id) && self.rasters.len() < 64,
            "Duplicate or excessive editor raster asset"
        );
        let data = self.pool.raster(pixels)?;
        self.rasters.insert(id.to_owned(), data);
        Ok(())
    }
    pub fn insert_font(&mut self, id: &str, bytes: Arc<[u8]>, index: u32) -> Result<()> {
        types::asset_id(id)?;
        ensure!(
            !self.fonts.contains_key(id) && self.fonts.len() < 64,
            "Duplicate or excessive editor font asset"
        );
        let admission = text::admit_font(&bytes, index)?;
        let data = self.pool.font(bytes)?;
        self.fonts.insert(
            id.to_owned(),
            FontAsset {
                data,
                index,
                admission,
            },
        );
        Ok(())
    }
    pub fn raster_bytes(&self) -> u64 {
        self.pool.counters.raster.load(Ordering::Acquire)
    }
    pub fn font_bytes(&self) -> u64 {
        self.pool.counters.fonts.load(Ordering::Acquire)
    }
    pub fn raster_count(&self) -> usize {
        self.pool.counters.images.load(Ordering::Acquire)
    }
    fn validate_references(&self, spec: &DocumentSpec) -> Result<()> {
        for layer in &spec.layers {
            match &layer.content {
                LayerKind::Legacy { annotation } => ensure!(
                    annotation.belongs_to(self),
                    "Legacy annotation reservations belong to another asset bank"
                ),
                LayerKind::Raster { asset, .. } => ensure!(
                    self.rasters.contains_key(asset),
                    "Unknown editor raster asset"
                ),
                LayerKind::Text { text } => {
                    for span in &text.spans {
                        for id in &span.style.fonts {
                            ensure!(self.fonts.contains_key(id), "Unknown editor font asset");
                        }
                    }
                }
                _ => {}
            }
        }
        if let Some(watermark) = &spec.export.watermark {
            ensure!(
                self.rasters.contains_key(&watermark.asset),
                "Unknown editor watermark asset"
            );
        }
        Ok(())
    }
    fn retain_references(&mut self, specs: impl Iterator<Item = Arc<DocumentSpec>>) {
        let mut rasters = BTreeSet::new();
        let mut fonts = BTreeSet::new();
        for spec in specs {
            if let Some(watermark) = &spec.export.watermark {
                rasters.insert(watermark.asset.clone());
            }
            for layer in &spec.layers {
                match &layer.content {
                    LayerKind::Raster { asset, .. } => {
                        rasters.insert(asset.clone());
                    }
                    LayerKind::Text { text } => {
                        for span in &text.spans {
                            fonts.extend(span.style.fonts.iter().cloned());
                        }
                    }
                    _ => {}
                }
            }
        }
        self.rasters.retain(|id, _| rasters.contains(id));
        self.fonts.retain(|id, _| fonts.contains(id));
    }
}

#[derive(Clone, Copy, Debug)]
pub struct DocumentMemory {
    pub retained_raster_bytes: u64,
    pub retained_font_bytes: u64,
    pub retained_rasters: usize,
    pub retained_masks: usize,
    pub metadata_bytes: usize,
    pub undo_states: usize,
    pub redo_states: usize,
}
pub struct EditorDocument {
    source: CapturedImage,
    source_data: Arc<RasterData>,
    assets: EditorAssets,
    current: Arc<DocumentSpec>,
    undo: VecDeque<Arc<DocumentSpec>>,
    redo: Vec<Arc<DocumentSpec>>,
    revision: u64,
}
#[derive(Clone)]
pub struct EditorSnapshot {
    source: CapturedImage,
    _source_data: Arc<RasterData>,
    assets: EditorAssets,
    spec: Arc<DocumentSpec>,
    metadata_bytes: usize,
    pub revision: u64,
}
impl EditorSnapshot {
    pub fn spec(&self) -> &DocumentSpec {
        &self.spec
    }
    pub fn render(&self, cancelled: impl Fn() -> bool) -> Result<CapturedImage> {
        render::render(self, &cancelled)
    }
    pub fn planned_output_size(&self) -> Result<[u32; 2]> {
        render::output_size(&self.spec)
    }
}
impl EditorDocument {
    pub fn from_legacy(
        source: CapturedImage,
        regions: &[crate::PixelRegion],
        crop: Option<crate::PixelRegion>,
        annotations: &[crate::DrawAnnotation],
        pixelations: &[crate::PixelRegion],
        blurs: &[crate::PixelRegion],
    ) -> Result<Self> {
        ensure!(
            regions.len() + annotations.len() + pixelations.len() + blurs.len() <= MAX_LAYERS,
            "Legacy editor document exceeds 100 objects"
        );
        let assets = EditorAssets::new();
        let size = [source.width, source.height];
        let mut spec = DocumentSpec::new(size[0], size[1]);
        spec.crop = crop;
        let mut push = |bounds: Rect, content: LayerKind| {
            let id = spec.layers.len() as u64 + 1;
            spec.layers.push(Layer {
                id,
                bounds,
                transform: Affine::IDENTITY,
                opacity: 255,
                content,
            });
        };
        for (areas, blur) in [(blurs, true), (pixelations, false)] {
            for area in areas {
                legacy::region(*area, size)?;
                push(
                    Rect::new(
                        area.x as f64,
                        area.y as f64,
                        area.width as f64,
                        area.height as f64,
                    ),
                    if blur {
                        LayerKind::Blur {
                            radius: 8,
                            input: EffectInput::ProtectedBase,
                        }
                    } else {
                        LayerKind::Pixelate {
                            block: 16,
                            input: EffectInput::ProtectedBase,
                        }
                    },
                );
            }
        }
        for draw in annotations {
            let annotation = LegacyAnnotation::new(draw.clone(), size, &assets)?;
            push(annotation.bounds(), LayerKind::Legacy { annotation });
        }
        for area in regions {
            legacy::region(*area, size)?;
            push(
                Rect::new(
                    area.x as f64,
                    area.y as f64,
                    area.width as f64,
                    area.height as f64,
                ),
                LayerKind::Redaction,
            );
        }
        Self::new(source, assets, spec)
    }
    pub fn new(source: CapturedImage, assets: EditorAssets, spec: DocumentSpec) -> Result<Self> {
        spec.validate()?;
        ensure!(
            spec.metadata_bytes() <= MAX_METADATA,
            "Editor document metadata exceeds 4 MiB"
        );
        assets.validate_references(&spec)?;
        ensure!(
            source.display_id.len() <= 4096
                && source.window_id.as_ref().is_none_or(|id| id.len() <= 4096)
                && source.png_path.as_os_str().as_encoded_bytes().len() <= 4096,
            "Editor source metadata exceeds its bound"
        );
        ensure!(
            source.width == source.pixels.width() && source.height == source.pixels.height(),
            "Editor source metadata differs from its backing"
        );
        ensure!(
            !assets
                .rasters
                .values()
                .any(|asset| Arc::ptr_eq(&asset.pixels, &source.pixels)),
            "The original source cannot be inserted as an external raster asset"
        );
        let source_data = assets.pool.raster(source.pixels.clone())?;
        Ok(Self {
            source,
            source_data,
            assets,
            current: Arc::new(spec),
            undo: VecDeque::new(),
            redo: Vec::new(),
            revision: 1,
        })
    }
    pub fn from_image(source: CapturedImage) -> Result<Self> {
        let spec = DocumentSpec::new(source.width, source.height);
        Self::new(source, EditorAssets::new(), spec)
    }
    pub fn spec(&self) -> &DocumentSpec {
        &self.current
    }
    pub fn revision(&self) -> u64 {
        self.revision
    }
    fn metadata_bytes(&self) -> usize {
        self.current.metadata_bytes()
            + self
                .undo
                .iter()
                .chain(self.redo.iter())
                .map(|spec| spec.metadata_bytes())
                .sum::<usize>()
    }
    pub fn memory(&self) -> DocumentMemory {
        DocumentMemory {
            retained_raster_bytes: self.assets.raster_bytes(),
            retained_font_bytes: self.assets.font_bytes(),
            retained_rasters: self.assets.raster_count(),
            retained_masks: self.assets.pool.counters.masks.load(Ordering::Acquire),
            metadata_bytes: self.metadata_bytes(),
            undo_states: self.undo.len(),
            redo_states: self.redo.len(),
        }
    }
    pub fn snapshot(&self) -> EditorSnapshot {
        EditorSnapshot {
            source: self.source.clone(),
            _source_data: self.source_data.clone(),
            assets: self.assets.clone(),
            spec: self.current.clone(),
            metadata_bytes: self.metadata_bytes(),
            revision: self.revision,
        }
    }
    fn prune_history(&mut self) {
        while self.undo.len() + self.redo.len() > 100 || self.metadata_bytes() > MAX_METADATA {
            if self.undo.pop_front().is_none() {
                if !self.redo.is_empty() {
                    self.redo.remove(0);
                } else {
                    break;
                }
            }
        }
    }
    pub fn replace(&mut self, spec: DocumentSpec) -> Result<bool> {
        spec.validate()?;
        ensure!(
            spec.metadata_bytes() <= MAX_METADATA,
            "Editor document metadata exceeds 4 MiB"
        );
        self.assets.validate_references(&spec)?;
        if spec == *self.current {
            return Ok(false);
        }
        let revision = self
            .revision
            .checked_add(1)
            .context("Editor revision exhausted")?;
        self.undo
            .push_back(std::mem::replace(&mut self.current, Arc::new(spec)));
        self.redo.clear();
        self.revision = revision;
        self.prune_history();
        Ok(true)
    }
    pub fn edit(&mut self, change: impl FnOnce(&mut DocumentSpec) -> Result<()>) -> Result<bool> {
        let mut next = (*self.current).clone();
        change(&mut next)?;
        self.replace(next)
    }
    pub fn insert(&mut self, layer: Layer) -> Result<bool> {
        self.edit(|spec| {
            spec.layers.push(layer);
            Ok(())
        })
    }
    pub fn insert_legacy(
        &mut self,
        id: u64,
        draw: crate::DrawAnnotation,
        transform: Affine,
    ) -> Result<bool> {
        let annotation =
            LegacyAnnotation::new(draw, [self.source.width, self.source.height], &self.assets)?;
        self.insert(Layer {
            id,
            bounds: annotation.bounds(),
            transform,
            opacity: 255,
            content: LayerKind::Legacy { annotation },
        })
    }
    pub fn remove(&mut self, id: u64) -> Result<bool> {
        self.edit(|spec| {
            let index = spec
                .layers
                .iter()
                .position(|layer| layer.id == id)
                .context("Unknown editor layer")?;
            spec.layers.remove(index);
            Ok(())
        })
    }
    pub fn reorder(&mut self, id: u64, index: usize) -> Result<bool> {
        self.edit(|spec| {
            ensure!(index < spec.layers.len(), "Invalid editor layer order");
            let previous = spec
                .layers
                .iter()
                .position(|layer| layer.id == id)
                .context("Unknown editor layer")?;
            let layer = spec.layers.remove(previous);
            spec.layers.insert(index, layer);
            Ok(())
        })
    }
    pub fn transform_layer(&mut self, id: u64, delta: Affine) -> Result<bool> {
        delta.validate()?;
        self.edit(|spec| {
            let layer = spec
                .layers
                .iter_mut()
                .find(|layer| layer.id == id)
                .context("Unknown editor layer")?;
            layer.transform = delta.then(layer.transform);
            Ok(())
        })
    }
    pub fn apply_preset(&mut self, id: u64, preset: &ToolPreset) -> Result<bool> {
        preset.validate()?;
        self.edit(|spec| {
            let layer = spec
                .layers
                .iter_mut()
                .find(|layer| layer.id == id)
                .context("Unknown editor layer")?;
            layer.content = preset.content.clone();
            if matches!(layer.content, LayerKind::Redaction) {
                layer.opacity = 255;
            }
            Ok(())
        })
    }
    pub fn set_export(&mut self, preset: ExportPreset) -> Result<bool> {
        self.edit(|spec| {
            spec.export = preset;
            Ok(())
        })
    }
    pub fn set_crop(&mut self, crop: Option<crate::PixelRegion>) -> Result<bool> {
        self.edit(|spec| {
            spec.crop = crop;
            spec.clip = None;
            Ok(())
        })
    }
    pub fn transform_document(&mut self, delta: Affine, canvas: [u32; 2]) -> Result<bool> {
        delta.validate()?;
        crate::validate_image_size(canvas[0], canvas[1])?;
        self.edit(|spec| {
            spec.base = delta.then(spec.base);
            for layer in &mut spec.layers {
                layer.transform = delta.then(layer.transform);
                layer.content.source_transform(delta);
            }
            if let Some(crop) = spec.crop {
                let mut clip = spec.clip.clone().unwrap_or(CanvasClip {
                    bounds: Rect::new(
                        crop.x as f64,
                        crop.y as f64,
                        crop.width as f64,
                        crop.height as f64,
                    ),
                    transform: Affine::IDENTITY,
                });
                clip.transform = delta.then(clip.transform);
                let bounds = clip.transform.bounds(clip.bounds)?;
                let left = bounds.x.floor().clamp(0.0, canvas[0] as f64) as u32;
                let top = bounds.y.floor().clamp(0.0, canvas[1] as f64) as u32;
                let right = (bounds.x + bounds.width)
                    .ceil()
                    .clamp(0.0, canvas[0] as f64) as u32;
                let bottom = (bounds.y + bounds.height)
                    .ceil()
                    .clamp(0.0, canvas[1] as f64) as u32;
                ensure!(
                    right > left && bottom > top,
                    "Transformed crop is outside its canvas"
                );
                spec.crop = Some(crate::PixelRegion {
                    x: left,
                    y: top,
                    width: right - left,
                    height: bottom - top,
                });
                spec.clip = Some(clip);
            }
            spec.canvas = canvas;
            Ok(())
        })
    }
    pub fn scale_to(
        &mut self,
        width: u32,
        height: u32,
        interpolation: Interpolation,
    ) -> Result<bool> {
        let scale = Affine::scale(
            width as f64 / self.current.canvas[0] as f64,
            height as f64 / self.current.canvas[1] as f64,
        );
        let changed = self.transform_document(scale, [width, height])?;
        // Keep scale and interpolation one undo step, rather than a second metadata edit.
        if changed {
            let mut next = (*self.current).clone();
            next.interpolation = interpolation;
            self.current = Arc::new(next);
        } else if self.current.interpolation != interpolation {
            self.edit(|spec| {
                spec.interpolation = interpolation;
                Ok(())
            })?;
            return Ok(true);
        }
        Ok(changed)
    }
    pub fn resize_canvas(&mut self, width: u32, height: u32, anchor: [f64; 2]) -> Result<bool> {
        ensure!(
            anchor
                .iter()
                .all(|value| value.is_finite() && (0.0..=1.0).contains(value)),
            "Invalid editor canvas anchor"
        );
        self.transform_document(
            Affine::translate(
                (width as f64 - self.current.canvas[0] as f64) * anchor[0],
                (height as f64 - self.current.canvas[1] as f64) * anchor[1],
            ),
            [width, height],
        )
    }
    pub fn undo(&mut self) -> bool {
        if self.revision == u64::MAX {
            return false;
        }
        let Some(next) = self.undo.pop_back() else {
            return false;
        };
        self.redo.push(std::mem::replace(&mut self.current, next));
        self.revision += 1;
        self.prune_history();
        true
    }
    pub fn redo(&mut self) -> bool {
        if self.revision == u64::MAX {
            return false;
        }
        let Some(next) = self.redo.pop() else {
            return false;
        };
        self.undo
            .push_back(std::mem::replace(&mut self.current, next));
        self.revision += 1;
        self.prune_history();
        true
    }
    pub fn duplicate(&mut self, id: u64, new_id: u64, delta: Affine) -> Result<bool> {
        delta.validate()?;
        self.edit(|spec| {
            let mut layer = spec
                .layers
                .iter()
                .find(|layer| layer.id == id)
                .context("Unknown editor layer")?
                .clone();
            layer.id = new_id;
            layer.transform = delta.then(layer.transform);
            spec.layers.push(layer);
            Ok(())
        })
    }
    pub fn reset(&mut self) -> Result<bool> {
        self.replace(DocumentSpec::new(self.source.width, self.source.height))
    }
    pub fn insert_raster(&mut self, id: &str, pixels: Arc<RgbaImage>) -> Result<()> {
        ensure!(
            !Arc::ptr_eq(&pixels, &self.source.pixels),
            "The original source cannot be inserted as an external raster asset"
        );
        self.assets.insert_raster(id, pixels)
    }
    pub fn insert_font(&mut self, id: &str, bytes: Arc<[u8]>, index: u32) -> Result<()> {
        self.assets.insert_font(id, bytes, index)
    }
    pub fn collect_unused_assets(&mut self) {
        self.assets.retain_references(
            std::iter::once(self.current.clone())
                .chain(self.undo.iter().cloned())
                .chain(self.redo.iter().cloned()),
        );
    }
    pub fn clear_history(&mut self) {
        self.undo.clear();
        self.redo.clear();
        self.collect_unused_assets();
    }
}

#[cfg(test)]
#[path = "editor_document_tests.rs"]
mod tests;
