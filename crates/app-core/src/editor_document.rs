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
#[path = "editor_document_state.rs"]
mod state;
pub use state::{AssetCatalog, EditorEdit, EditorState};

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
        let extent = u64::from(pixels.width()) * u64::from(pixels.height()) * 4;
        ensure!(
            pixels.as_raw().len() as u64 == extent,
            "Editor raster backing does not match its dimensions"
        );
        // ImageBuffer::from_raw keeps the Vec allocation, including unused slack.
        let bytes = pixels.as_raw().capacity() as u64;
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
        AssetCatalog::from_assets(self).validate_references(spec)
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
    state: EditorState,
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
        let state = EditorState::new(
            spec,
            [source.width, source.height],
            AssetCatalog::from_assets(&assets),
        )?;
        Self::from_state(source, assets, state)
    }
    pub fn from_state(
        source: CapturedImage,
        assets: EditorAssets,
        mut state: EditorState,
    ) -> Result<Self> {
        ensure!(
            source.display_id.capacity() <= 4096
                && source
                    .window_id
                    .as_ref()
                    .is_none_or(|id| id.capacity() <= 4096)
                && source.png_path.capacity() <= 4096,
            "Editor source metadata exceeds its bound"
        );
        ensure!(
            source.width == source.pixels.width()
                && source.height == source.pixels.height()
                && state.source_size() == [source.width, source.height],
            "Editor source dimensions differ from the prepared metadata plan"
        );
        ensure!(
            !assets
                .rasters
                .values()
                .any(|asset| Arc::ptr_eq(&asset.pixels, &source.pixels)),
            "The original source cannot be inserted as an external raster asset"
        );
        let catalog = AssetCatalog::from_assets(&assets);
        for spec in std::iter::once(&state.current)
            .chain(state.undo.iter())
            .chain(state.redo.iter())
        {
            catalog.validate_references(spec)?;
        }
        state.catalog = catalog;
        let source_data = assets.pool.raster(source.pixels.clone())?;
        Ok(Self {
            source,
            source_data,
            assets,
            state,
        })
    }
    pub fn from_image(source: CapturedImage) -> Result<Self> {
        let spec = DocumentSpec::new(source.width, source.height);
        Self::new(source, EditorAssets::new(), spec)
    }
    pub fn spec(&self) -> &DocumentSpec {
        self.state.spec()
    }
    pub fn revision(&self) -> u64 {
        self.state.revision()
    }
    fn metadata_bytes(&self) -> usize {
        self.state.metadata_bytes()
    }
    pub fn memory(&self) -> DocumentMemory {
        DocumentMemory {
            retained_raster_bytes: self.assets.raster_bytes(),
            retained_font_bytes: self.assets.font_bytes(),
            retained_rasters: self.assets.raster_count(),
            retained_masks: self.assets.pool.counters.masks.load(Ordering::Acquire),
            metadata_bytes: self.metadata_bytes(),
            undo_states: self.state.undo.len(),
            redo_states: self.state.redo.len(),
        }
    }
    pub fn snapshot(&self) -> EditorSnapshot {
        EditorSnapshot {
            source: self.source.clone(),
            _source_data: self.source_data.clone(),
            assets: self.assets.clone(),
            spec: self.state.current.clone(),
            metadata_bytes: self.metadata_bytes(),
            revision: self.state.revision,
        }
    }

    pub fn replace(&mut self, spec: DocumentSpec) -> Result<bool> {
        self.state.replace(spec)
    }
    pub fn edit(&mut self, change: impl FnOnce(&mut DocumentSpec) -> Result<()>) -> Result<bool> {
        self.state.edit(change)
    }
    pub fn insert(&mut self, layer: Layer) -> Result<bool> {
        self.state.insert(layer)
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
        self.state.remove(id)
    }
    pub fn reorder(&mut self, id: u64, index: usize) -> Result<bool> {
        self.state.reorder(id, index)
    }
    pub fn transform_layer(&mut self, id: u64, delta: Affine) -> Result<bool> {
        self.state.transform_layer(id, delta)
    }
    pub fn apply_preset(&mut self, id: u64, preset: &ToolPreset) -> Result<bool> {
        self.state.apply_preset(id, preset)
    }
    pub fn set_export(&mut self, preset: ExportPreset) -> Result<bool> {
        self.state.set_export(preset)
    }
    pub fn set_crop(&mut self, crop: Option<crate::PixelRegion>) -> Result<bool> {
        self.state.set_crop(crop)
    }
    pub fn transform_document(&mut self, delta: Affine, canvas: [u32; 2]) -> Result<bool> {
        self.state.transform_document(delta, canvas)
    }
    pub fn scale_to(
        &mut self,
        width: u32,
        height: u32,
        interpolation: Interpolation,
    ) -> Result<bool> {
        self.state.scale_to(width, height, interpolation)
    }
    pub fn resize_canvas(&mut self, width: u32, height: u32, anchor: [f64; 2]) -> Result<bool> {
        self.state.resize_canvas(width, height, anchor)
    }
    pub fn undo(&mut self) -> bool {
        self.state.undo()
    }
    pub fn redo(&mut self) -> bool {
        self.state.redo()
    }
    pub fn duplicate(&mut self, id: u64, new_id: u64, delta: Affine) -> Result<bool> {
        self.state.duplicate(id, new_id, delta)
    }
    pub fn reset(&mut self) -> Result<bool> {
        self.state.reset()
    }
    pub fn insert_raster(&mut self, id: &str, pixels: Arc<RgbaImage>) -> Result<()> {
        ensure!(
            !Arc::ptr_eq(&pixels, &self.source.pixels),
            "The original source cannot be inserted as an external raster asset"
        );
        self.assets.insert_raster(id, pixels)?;
        self.state.catalog = AssetCatalog::from_assets(&self.assets);
        Ok(())
    }
    pub fn insert_font(&mut self, id: &str, bytes: Arc<[u8]>, index: u32) -> Result<()> {
        self.assets.insert_font(id, bytes, index)?;
        self.state.catalog = AssetCatalog::from_assets(&self.assets);
        Ok(())
    }
    pub fn collect_unused_assets(&mut self) {
        self.assets.retain_references(
            std::iter::once(self.state.current.clone())
                .chain(self.state.undo.iter().cloned())
                .chain(self.state.redo.iter().cloned()),
        );
        self.state.catalog = AssetCatalog::from_assets(&self.assets);
    }
    pub fn clear_history(&mut self) {
        self.state.clear_history();
        self.collect_unused_assets();
    }
}

#[cfg(test)]
#[path = "editor_document_tests.rs"]
mod tests;
