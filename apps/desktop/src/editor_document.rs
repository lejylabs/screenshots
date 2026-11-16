//! Explicit scene-file consumer of the same non-UI editor model and renderer.
use anyhow::{Context, Result, bail, ensure};
use app_core::editor_document::*;
use serde::Deserialize;
use std::{
    ffi::OsString,
    io::{Read, Write},
    path::{Path, PathBuf},
    sync::{Arc, atomic::Ordering},
};

const USAGE: &str =
    "Usage: screenshots editor-document inspect SCENE.json | render INPUT SCENE.json OUTPUT";
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RasterFile {
    id: String,
    path: PathBuf,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct FontFile {
    id: String,
    path: PathBuf,
    #[serde(default)]
    index: u32,
}
#[derive(Deserialize)]
#[serde(tag = "action", rename_all = "snake_case", deny_unknown_fields)]
enum Edit {
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
        area: Option<app_core::PixelRegion>,
    },
    ToolPreset {
        id: u64,
        name: String,
    },
    Undo,
    Redo,
    Reset,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Scene {
    version: u32,
    document: DocumentSpec,
    #[serde(default)]
    rasters: Vec<RasterFile>,
    #[serde(default)]
    fonts: Vec<FontFile>,
    #[serde(default)]
    presets: PresetBook,
    #[serde(default)]
    export_preset: Option<String>,
    #[serde(default)]
    edits: Vec<Edit>,
}
fn read_file(path: &Path, limit: u64) -> Result<Vec<u8>> {
    let (mut file, id) =
        app_core::FileIdentity::open(path).context("Could not open editor document input")?;
    let length = file.metadata()?.len();
    ensure!(
        length > 0 && length <= limit,
        "Editor document input exceeds its readable extent"
    );
    let mut bytes = Vec::with_capacity(length as usize);
    Read::by_ref(&mut file)
        .take(length + 1)
        .read_to_end(&mut bytes)?;
    ensure!(
        bytes.len() as u64 == length,
        "Editor document input changed while reading"
    );
    id.verify_file(&file)?;
    Ok(bytes)
}
fn id(value: &str) -> Result<()> {
    ensure!(
        !value.is_empty()
            && value.len() <= 64
            && value
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.'))
            && !matches!(value, "base" | "source"),
        "Invalid or reserved editor asset ID"
    );
    Ok(())
}
fn asset_path(parent: &Path, path: &Path) -> Result<PathBuf> {
    ensure!(
        !path.as_os_str().is_empty() && path.as_os_str().as_encoded_bytes().len() <= 4096,
        "Editor asset path exceeds its bound"
    );
    Ok(if path.is_absolute() {
        path.to_owned()
    } else {
        parent.join(path)
    })
}
impl Scene {
    fn read(path: &Path) -> Result<Self> {
        let bytes = read_file(path, 512 * 1024)?;
        let value: Self = serde_json::from_slice(&bytes)
            .map_err(|_| anyhow::anyhow!("Invalid strict editor scene"))?;
        ensure!(
            value.version == 1
                && value.rasters.len() <= 63
                && value.fonts.len() <= 64
                && value.edits.len() <= 100,
            "Invalid or excessive editor scene entries"
        );
        value.document.validate()?;
        ensure!(
            serde_json::to_vec(&value.document)?.len() <= 256 * 1024,
            "Editor document metadata exceeds 256 KiB"
        );
        value.presets.validate()?;
        let mut names = std::collections::BTreeSet::new();
        for asset in &value.rasters {
            id(&asset.id)?;
            ensure!(names.insert(&asset.id), "Duplicate editor raster asset ID");
            asset_path(Path::new("."), &asset.path)?;
        }
        names.clear();
        for asset in &value.fonts {
            id(&asset.id)?;
            ensure!(
                names.insert(&asset.id) && asset.index < 16,
                "Duplicate editor font asset ID or invalid face"
            );
            asset_path(Path::new("."), &asset.path)?;
        }
        let references = |content: &LayerKind| -> Result<()> {
            match content {
                LayerKind::Raster { asset, .. } => ensure!(
                    value.rasters.iter().any(|entry| entry.id == *asset),
                    "Unknown declared editor raster asset"
                ),
                LayerKind::Text { text } => {
                    for span in &text.spans {
                        for id in &span.style.fonts {
                            ensure!(
                                value.fonts.iter().any(|entry| entry.id == *id),
                                "Unknown declared editor font asset"
                            );
                        }
                    }
                }
                _ => {}
            }
            Ok(())
        };
        for layer in &value.document.layers {
            references(&layer.content)?;
        }
        for edit in &value.edits {
            match edit {
                Edit::TransformLayer { transform, .. }
                | Edit::TransformDocument { transform, .. }
                | Edit::Duplicate { transform, .. } => transform.validate()?,
                Edit::Insert { layer } => {
                    let mut spec =
                        DocumentSpec::new(value.document.canvas[0], value.document.canvas[1]);
                    spec.layers.push(layer.clone());
                    spec.validate()?;
                    references(&layer.content)?;
                }
                Edit::Scale { width, height, .. } | Edit::ResizeCanvas { width, height, .. } => {
                    app_core::validate_image_size(*width, *height)?
                }
                Edit::ToolPreset { name, .. } => {
                    let preset = value
                        .presets
                        .tools
                        .iter()
                        .find(|preset| preset.name == *name)
                        .context("Unknown editor tool preset")?;
                    references(&preset.content)?;
                }
                _ => {}
            }
        }
        if let Some(watermark) = &value.export()?.watermark {
            ensure!(
                value
                    .rasters
                    .iter()
                    .any(|entry| entry.id == watermark.asset),
                "Unknown declared editor watermark asset"
            );
        }
        Ok(value)
    }
    fn export(&self) -> Result<ExportPreset> {
        if let Some(name) = &self.export_preset {
            Ok(self
                .presets
                .exports
                .iter()
                .find(|preset| preset.name == *name)
                .context("Unknown editor export preset")?
                .preset
                .clone())
        } else {
            Ok(self.document.export.clone())
        }
    }
    fn apply(&self, document: &mut EditorDocument, cancelled: &impl Fn() -> bool) -> Result<()> {
        for edit in &self.edits {
            check(cancelled)?;
            match edit {
                Edit::Insert { layer } => {
                    document.insert(layer.clone())?;
                }
                Edit::Remove { id } => {
                    document.remove(*id)?;
                }
                Edit::Duplicate {
                    id,
                    new_id,
                    transform,
                } => {
                    document.duplicate(*id, *new_id, *transform)?;
                }
                Edit::Reorder { id, index } => {
                    document.reorder(*id, *index)?;
                }
                Edit::TransformLayer { id, transform } => {
                    document.transform_layer(*id, *transform)?;
                }
                Edit::TransformDocument { transform, canvas } => {
                    document.transform_document(*transform, *canvas)?;
                }
                Edit::ResizeCanvas {
                    width,
                    height,
                    anchor,
                } => {
                    document.resize_canvas(*width, *height, *anchor)?;
                }
                Edit::Scale {
                    width,
                    height,
                    interpolation,
                } => {
                    document.scale_to(*width, *height, *interpolation)?;
                }
                Edit::Crop { area } => {
                    document.set_crop(*area)?;
                }
                Edit::ToolPreset { id, name } => {
                    self.presets.apply_tool(name, document, *id)?;
                }
                Edit::Undo => {
                    document.undo();
                }
                Edit::Redo => {
                    document.redo();
                }
                Edit::Reset => {
                    document.reset()?;
                }
            }
        }
        // Export options are the final explicit request, independently of undo
        // within the edit sequence. The same preset was checked against OUTPUT.
        document.set_export(self.export()?)?;
        check(cancelled)
    }
}
fn check(cancelled: &impl Fn() -> bool) -> Result<()> {
    ensure!(!cancelled(), "Editor document canceled");
    Ok(())
}
fn refuse_source_alias(
    source: &app_core::FileIdentity,
    asset: &app_core::FileIdentity,
) -> Result<()> {
    ensure!(
        !asset.same_object(source),
        "The original source file cannot be reinserted as an external asset; use protected clone sampling"
    );
    Ok(())
}

#[cfg(target_os = "windows")]
static WINDOWS_CANCELED: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
#[cfg(target_os = "windows")]
unsafe extern "system" fn console(code: u32) -> i32 {
    if matches!(code, 0 | 1 | 2 | 5 | 6) {
        WINDOWS_CANCELED.store(true, Ordering::Release);
        1
    } else {
        0
    }
}
struct Cancellation {
    #[cfg(unix)]
    _scope: crate::record::unix::Cancellation,
    #[cfg(unix)]
    flag: Arc<std::sync::atomic::AtomicBool>,
}
impl Cancellation {
    fn new() -> Result<Self> {
        #[cfg(unix)]
        {
            let scope = crate::record::unix::Cancellation::new()?;
            let flag = scope.flag();
            Ok(Self {
                _scope: scope,
                flag,
            })
        }
        #[cfg(target_os = "windows")]
        {
            WINDOWS_CANCELED.store(false, Ordering::Release);
            ensure!(
                unsafe {
                    windows_sys::Win32::System::Console::SetConsoleCtrlHandler(Some(console), 1)
                } != 0,
                "Could not install editor cancellation"
            );
            Ok(Self {})
        }
        #[cfg(not(any(unix, target_os = "windows")))]
        bail!("Editor command cancellation is unavailable on this platform")
    }
    fn canceled(&self) -> bool {
        #[cfg(unix)]
        return self.flag.load(Ordering::Acquire);
        #[cfg(target_os = "windows")]
        return WINDOWS_CANCELED.load(Ordering::Acquire);
        #[cfg(not(any(unix, target_os = "windows")))]
        true
    }
}
impl Drop for Cancellation {
    fn drop(&mut self) {
        #[cfg(target_os = "windows")]
        unsafe {
            windows_sys::Win32::System::Console::SetConsoleCtrlHandler(Some(console), 0);
        }
    }
}

fn render(
    input: &Path,
    profile: &Path,
    destination: &Path,
    cancelled: &impl Fn() -> bool,
) -> Result<serde_json::Value> {
    let scene = Scene::read(profile)?;
    let export = scene.export()?;
    export.validate_extension(destination)?;
    ensure!(
        destination.as_os_str().as_encoded_bytes().len() <= 4096,
        "Editor output path exceeds its bound"
    );
    match destination.symlink_metadata() {
        Ok(_) => bail!("Editor output already exists"),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(_) => bail!("Could not inspect editor output"),
    }
    check(cancelled)?;
    let (source_file, source_id) =
        app_core::FileIdentity::open(input).context("Could not open editor source")?;
    let pixels = app_core::load_editor_image_with_identity_and_limit(
        input,
        &source_id,
        app_core::MAX_CAPTURE_BYTES,
    )?;
    source_id.verify_file(&source_file)?;
    let mut remaining = app_core::MAX_CAPTURE_BYTES - pixels.as_raw().len() as u64;
    let mut assets = EditorAssets::new();
    let parent = profile.parent().unwrap_or(Path::new("."));
    let mut raster_cache: Vec<(app_core::FileIdentity, Arc<image::RgbaImage>)> = Vec::new();
    for asset in &scene.rasters {
        check(cancelled)?;
        source_id.verify_file(&source_file)?;
        let path = asset_path(parent, &asset.path)?;
        let (file, identity) =
            app_core::FileIdentity::open(&path).context("Could not open editor raster asset")?;
        refuse_source_alias(&source_id, &identity)?;
        let image =
            if let Some((_, image)) = raster_cache.iter().find(|(known, _)| *known == identity) {
                image.clone()
            } else {
                ensure!(remaining > 0, "Editor decoded asset budget exhausted");
                let image = Arc::new(app_core::load_editor_image_with_identity_and_limit(
                    &path, &identity, remaining,
                )?);
                identity.verify_file(&file)?;
                remaining -= image.as_raw().len() as u64;
                raster_cache.push((identity, image.clone()));
                image
            };
        assets.insert_raster(&asset.id, image)?;
    }
    let mut font_cache: Vec<(app_core::FileIdentity, Arc<[u8]>)> = Vec::new();
    for asset in &scene.fonts {
        check(cancelled)?;
        let path = asset_path(parent, &asset.path)?;
        let (mut file, identity) =
            app_core::FileIdentity::open(&path).context("Could not open editor font asset")?;
        let bytes =
            if let Some((_, bytes)) = font_cache.iter().find(|(known, _)| *known == identity) {
                bytes.clone()
            } else {
                let remaining = MAX_FONT_BYTES as u64 - assets.font_bytes();
                let length = file.metadata()?.len();
                ensure!(
                    length > 0 && length <= remaining.min(2 * 1024 * 1024),
                    "Editor font source exceeds the remaining byte budget"
                );
                let mut bytes = Vec::with_capacity(length as usize);
                Read::by_ref(&mut file)
                    .take(length + 1)
                    .read_to_end(&mut bytes)?;
                identity.verify_file(&file)?;
                ensure!(
                    bytes.len() as u64 == length,
                    "Editor font source changed while reading"
                );
                let bytes: Arc<[u8]> = Arc::from(bytes);
                font_cache.push((identity, bytes.clone()));
                bytes
            };
        assets.insert_font(&asset.id, bytes, asset.index)?;
    }
    drop(raster_cache);
    drop(font_cache);
    drop(source_file);
    check(cancelled)?;
    let mut spec = scene.document.clone();
    spec.export = export.clone();
    let mut document = EditorDocument::new(crate::cli::frame(pixels), assets, spec)?;
    scene.apply(&mut document, cancelled)?;
    drop(scene);
    let snapshot = document.snapshot();
    let output = snapshot.render(cancelled)?;
    check(cancelled)?;
    match export.format {
        ExportFormat::WebpLossy => {
            app_core::save_lossy_webp(&output.pixels, destination, export.quality, cancelled)?
        }
        _ => {
            output.save_image_as_with_cancel(
                destination,
                export.quality,
                app_core::ExportSize::Original,
                cancelled,
            )?;
        }
    }
    let memory = document.memory();
    Ok(
        serde_json::json!({"saved":true,"format":match export.format{ExportFormat::Png=>"png",ExportFormat::Jpeg=>"jpeg",ExportFormat::Webp=>"webp",ExportFormat::WebpLossy=>"webp_lossy"},"width":output.width,"height":output.height,"layers":document.spec().layers.len(),"retained_source_bytes":memory.retained_raster_bytes,"retained_font_bytes":memory.retained_font_bytes,"metadata_bytes":memory.metadata_bytes}),
    )
}
pub(crate) fn run(args: &[OsString]) -> Result<()> {
    let scope = Cancellation::new()?;
    let result = match args.first().and_then(|arg| arg.to_str()) {
        Some("inspect") if args.len() == 2 => {
            let scene = Scene::read(Path::new(&args[1]))?;
            Ok(
                serde_json::json!({"version":1,"canvas":scene.document.canvas,"layers":scene.document.layers.len(),"rasters":scene.rasters.len(),"fonts":scene.fonts.len(),"edit_commands":scene.edits.len(),"tool_presets":scene.presets.tools.len(),"export_presets":scene.presets.exports.len()}),
            )
        }
        Some("render") if args.len() == 4 => render(
            Path::new(&args[1]),
            Path::new(&args[2]),
            Path::new(&args[3]),
            &|| scope.canceled(),
        ),
        _ => bail!(USAGE),
    };
    // Restore native termination before a successful result can block stdout.
    drop(scope);
    let result = result?;
    let mut output = std::io::stdout().lock();
    writeln!(output, "{result}")
        .and_then(|_| output.flush())
        .context("Editor operation completed but its result could not be written")
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn source_alias_policy_survives_hardlink_and_version_change_after_verification() -> Result<()> {
        struct Directory(PathBuf);
        impl Drop for Directory {
            fn drop(&mut self) {
                let _ = std::fs::remove_dir_all(&self.0);
            }
        }
        let directory = Directory(std::env::temp_dir().join(format!(
            "screenshots-editor-alias-race-{}",
            uuid::Uuid::new_v4()
        )));
        std::fs::create_dir(&directory.0)?;
        let source = directory.0.join("source");
        let alias = directory.0.join("alias");
        std::fs::write(&source, b"synthetic source")?;
        let (source_file, admitted) = app_core::FileIdentity::open(&source)?;
        admitted.verify_file(&source_file)?;
        std::fs::hard_link(&source, &alias)?;
        std::fs::OpenOptions::new()
            .append(true)
            .open(&alias)?
            .write_all(b" changed")?;
        let (asset_file, current) = app_core::FileIdentity::open(&alias)?;
        assert_ne!(admitted, current);
        assert!(refuse_source_alias(&admitted, &current).is_err());
        drop(asset_file);
        drop(source_file);
        Ok(())
    }
}
