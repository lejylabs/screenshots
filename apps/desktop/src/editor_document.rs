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
    edits: Vec<EditorEdit>,
    #[serde(default)]
    source_size: Option<[u32; 2]>,
    #[serde(skip)]
    prepared: Option<EditorState>,
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
    fn read(path: &Path, cancelled: &impl Fn() -> bool) -> Result<Self> {
        check(cancelled)?;
        let bytes = read_file(path, 512 * 1024)?;
        let mut value: Self = serde_json::from_slice(&bytes)
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
        let catalog = AssetCatalog::declared(
            value.rasters.iter().map(|entry| entry.id.clone()),
            value.fonts.iter().map(|entry| entry.id.clone()),
        )?;
        let export = value.export()?;
        let mut spec = value.document.clone();
        spec.export = export.clone();
        let mut state = EditorState::new(
            spec,
            value.source_size.unwrap_or(value.document.canvas),
            catalog,
        )?;
        for edit in &value.edits {
            check(cancelled)?;
            state.apply(edit, &value.presets)?;
        }
        state.set_export(export)?;
        state.planned_output_size()?;
        value.prepared = Some(state);
        check(cancelled)?;
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
    let mut scene = Scene::read(profile, cancelled)?;
    let state = scene
        .prepared
        .take()
        .context("Editor metadata plan missing")?;
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
    let pixels = app_core::load_editor_image_with_identity_limit_and_dimensions_with_cancel(
        input,
        &source_id,
        app_core::MAX_CAPTURE_BYTES,
        state.source_size(),
        cancelled,
    )?;
    ensure!(
        [pixels.width(), pixels.height()] == state.source_size(),
        "Oriented editor input dimensions differ from source_size"
    );
    source_id.verify_file(&source_file)?;
    let source_backing = pixels.as_raw().capacity() as u64;
    ensure!(
        source_backing <= app_core::MAX_CAPTURE_BYTES,
        "Editor source backing allocation exceeds its budget"
    );
    let mut remaining = app_core::MAX_CAPTURE_BYTES - source_backing;
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
                let image = Arc::new(
                    app_core::load_editor_image_with_identity_and_limit_with_cancel(
                        &path, &identity, remaining, cancelled,
                    )?,
                );
                identity.verify_file(&file)?;
                let backing = image.as_raw().capacity() as u64;
                ensure!(
                    backing <= remaining,
                    "Editor raster backing exceeds remaining allocation budget"
                );
                remaining -= backing;
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
        check(cancelled)?;
        assets.insert_font(&asset.id, bytes, asset.index)?;
        check(cancelled)?;
    }
    drop(raster_cache);
    drop(font_cache);
    drop(source_file);
    check(cancelled)?;
    drop(scene);
    let document = EditorDocument::from_state(crate::cli::frame(pixels), assets, state)?;
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
            let scene = Scene::read(Path::new(&args[1]), &|| scope.canceled())?;
            let prepared = scene
                .prepared
                .as_ref()
                .context("Editor metadata plan missing")?;
            Ok(
                serde_json::json!({"version":1,"canvas":prepared.spec().canvas,"source_size":prepared.source_size(),"layers":prepared.spec().layers.len(),"rasters":scene.rasters.len(),"fonts":scene.fonts.len(),"edit_commands":scene.edits.len(),"tool_presets":scene.presets.tools.len(),"export_presets":scene.presets.exports.len()}),
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
