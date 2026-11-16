//! Metadata-only history and transitions used both by inspection/preflight and rendering documents.
use super::*;
use serde::{Deserialize, Serialize};
#[derive(Clone, Default)]
pub struct AssetCatalog {
    rasters: BTreeSet<String>,
    fonts: BTreeSet<String>,
    legacy_pool: Option<Weak<Pool>>,
}
impl AssetCatalog {
    pub fn declared(
        rasters: impl IntoIterator<Item = String>,
        fonts: impl IntoIterator<Item = String>,
    ) -> Result<Self> {
        let mut value = Self::default();
        for id in rasters {
            types::asset_id(&id)?;
            let id = id.as_str().to_owned();
            ensure!(
                value.rasters.len() < 64 && value.rasters.insert(id),
                "Duplicate or excessive declared raster asset"
            );
        }
        for id in fonts {
            types::asset_id(&id)?;
            let id = id.as_str().to_owned();
            ensure!(
                value.fonts.len() < 64 && value.fonts.insert(id),
                "Duplicate or excessive declared font asset"
            );
        }
        Ok(value)
    }
    pub(super) fn from_assets(assets: &EditorAssets) -> Self {
        Self {
            rasters: assets.rasters.keys().cloned().collect(),
            fonts: assets.fonts.keys().cloned().collect(),
            legacy_pool: Some(Arc::downgrade(&assets.pool)),
        }
    }
    pub fn validate_references(&self, spec: &DocumentSpec) -> Result<()> {
        for layer in &spec.layers {
            match &layer.content {
                LayerKind::Raster { asset, .. } => ensure!(
                    self.rasters.contains(asset),
                    "Unknown declared editor raster asset"
                ),
                LayerKind::Text { text } => {
                    for span in &text.spans {
                        for id in &span.style.fonts {
                            ensure!(
                                self.fonts.contains(id),
                                "Unknown declared editor font asset"
                            );
                        }
                    }
                }
                LayerKind::Legacy { annotation } => ensure!(
                    self.legacy_pool
                        .as_ref()
                        .is_some_and(|pool| annotation.belongs_to_pool(pool)),
                    "Legacy annotation reservations belong to another asset bank"
                ),
                _ => {}
            }
        }
        if let Some(watermark) = &spec.export.watermark {
            ensure!(
                self.rasters.contains(&watermark.asset),
                "Unknown declared editor watermark asset"
            );
        }
        Ok(())
    }
}
#[derive(Clone)]
pub struct EditorState {
    pub(super) current: Arc<DocumentSpec>,
    pub(super) undo: VecDeque<Arc<DocumentSpec>>,
    pub(super) redo: Vec<Arc<DocumentSpec>>,
    pub(super) revision: u64,
    pub(super) catalog: AssetCatalog,
    source_size: [u32; 2],
}
impl EditorState {
    pub fn new(spec: DocumentSpec, source_size: [u32; 2], catalog: AssetCatalog) -> Result<Self> {
        crate::validate_image_size(source_size[0], source_size[1])?;
        spec.validate()?;
        ensure!(
            spec.metadata_bytes() <= MAX_METADATA,
            "Editor document metadata exceeds 4 MiB"
        );
        catalog.validate_references(&spec)?;
        Ok(Self {
            current: Arc::new(spec),
            undo: VecDeque::new(),
            redo: Vec::new(),
            revision: 1,
            catalog,
            source_size,
        })
    }
    pub fn source_size(&self) -> [u32; 2] {
        self.source_size
    }
    pub fn clear_history(&mut self) {
        self.undo.clear();
        self.redo.clear();
    }
    pub fn spec(&self) -> &DocumentSpec {
        &self.current
    }
    /// Plans the final crop/padding dimensions without reading or decoding assets.
    pub fn planned_output_size(&self) -> Result<[u32; 2]> {
        render::output_size(&self.current)
    }
    pub fn revision(&self) -> u64 {
        self.revision
    }
    pub fn metadata_bytes(&self) -> usize {
        self.current.metadata_bytes()
            + self
                .undo
                .iter()
                .chain(self.redo.iter())
                .map(|spec| spec.metadata_bytes())
                .sum::<usize>()
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
        self.catalog.validate_references(&spec)?;
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
        self.replace(DocumentSpec::new(self.source_size[0], self.source_size[1]))
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(tag = "action", rename_all = "snake_case", deny_unknown_fields)]
pub enum EditorEdit {
    Insert {
        layer: Layer,
    },
    Remove {
        id: u64,
    },
    Duplicate {
        id: u64,
        new_id: u64,
        transform: Affine,
    },
    Reorder {
        id: u64,
        index: usize,
    },
    TransformLayer {
        id: u64,
        transform: Affine,
    },
    TransformDocument {
        transform: Affine,
        canvas: [u32; 2],
    },
    ResizeCanvas {
        width: u32,
        height: u32,
        anchor: [f64; 2],
    },
    Scale {
        width: u32,
        height: u32,
        #[serde(default)]
        interpolation: Interpolation,
    },
    Crop {
        area: Option<crate::PixelRegion>,
    },
    ToolPreset {
        id: u64,
        name: String,
    },
    Undo,
    Redo,
    Reset,
}

impl EditorState {
    pub fn apply(&mut self, edit: &EditorEdit, presets: &PresetBook) -> Result<()> {
        match edit {
            EditorEdit::Insert { layer } => {
                self.insert(layer.clone())?;
            }
            EditorEdit::Remove { id } => {
                self.remove(*id)?;
            }
            EditorEdit::Duplicate {
                id,
                new_id,
                transform,
            } => {
                self.duplicate(*id, *new_id, *transform)?;
            }
            EditorEdit::Reorder { id, index } => {
                self.reorder(*id, *index)?;
            }
            EditorEdit::TransformLayer { id, transform } => {
                self.transform_layer(*id, *transform)?;
            }
            EditorEdit::TransformDocument { transform, canvas } => {
                self.transform_document(*transform, *canvas)?;
            }
            EditorEdit::ResizeCanvas {
                width,
                height,
                anchor,
            } => {
                self.resize_canvas(*width, *height, *anchor)?;
            }
            EditorEdit::Scale {
                width,
                height,
                interpolation,
            } => {
                self.scale_to(*width, *height, *interpolation)?;
            }
            EditorEdit::Crop { area } => {
                self.set_crop(*area)?;
            }
            EditorEdit::ToolPreset { id, name } => {
                presets.validate()?;
                let preset = presets
                    .tools
                    .iter()
                    .find(|preset| preset.name == *name)
                    .context("Unknown editor tool preset")?;
                self.apply_preset(*id, preset)?;
            }
            EditorEdit::Undo => {
                self.undo();
            }
            EditorEdit::Redo => {
                self.redo();
            }
            EditorEdit::Reset => {
                self.reset()?;
            }
        }
        Ok(())
    }
}
