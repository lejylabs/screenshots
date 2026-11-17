//! Explicit capture/Library encoding policy. Read and write only on a worker.

use anyhow::{Context, Result, ensure};
use image::{ImageEncoder, RgbaImage};
use serde::{Deserialize, Serialize};
use std::{
    fs::{self, OpenOptions},
    io::{Read, Write},
    path::{Path, PathBuf},
};

pub const MAX_SAVED_IMAGE_BYTES: u64 = crate::MAX_CAPTURE_BYTES + 16 * 1024 * 1024;
pub const MAX_SAVED_JPEG_WEBP_BYTES: u64 = 32 * 1024 * 1024;
const MAX_SETTINGS_BYTES: u64 = 16 * 1024;

fn deserialize_object<'de, D, T>(deserializer: D) -> std::result::Result<T, D::Error>
where
    D: serde::Deserializer<'de>,
    T: Deserialize<'de>,
{
    struct Object<T>(std::marker::PhantomData<T>);
    impl<'de, T: Deserialize<'de>> serde::de::Visitor<'de> for Object<T> {
        type Value = T;
        fn expecting(&self, formatter: &mut std::fmt::Formatter) -> std::fmt::Result {
            formatter.write_str("a format policy JSON object")
        }
        fn visit_map<M: serde::de::MapAccess<'de>>(
            self,
            map: M,
        ) -> std::result::Result<T, M::Error> {
            T::deserialize(serde::de::value::MapAccessDeserializer::new(map))
        }
    }
    deserializer.deserialize_map(Object(std::marker::PhantomData))
}

#[cfg(test)]
thread_local! { static BEFORE_CATALOG_OPEN: std::cell::Cell<Option<fn(&Path)>> = const { std::cell::Cell::new(None) }; }
#[cfg(test)]
thread_local! { static AFTER_CATALOG_METADATA: std::cell::Cell<Option<Box<dyn FnOnce()>>> = const { std::cell::Cell::new(None) }; }

/// A quality value cannot be constructed or deserialized outside 1–100.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(transparent)]
pub struct CaptureImageQuality(u8);

impl CaptureImageQuality {
    pub fn new(value: u8) -> Result<Self> {
        ensure!(
            (1..=100).contains(&value),
            "Image quality must be between 1 and 100"
        );
        Ok(Self(value))
    }

    pub fn get(self) -> u8 {
        self.0
    }
}

impl<'de> Deserialize<'de> for CaptureImageQuality {
    fn deserialize<D: serde::Deserializer<'de>>(
        deserializer: D,
    ) -> std::result::Result<Self, D::Error> {
        Self::new(u8::deserialize(deserializer)?).map_err(serde::de::Error::custom)
    }
}

/// JPEG is explicitly lossy; WebP is lossless unless `webp_lossy` is selected.
/// Quality is rejected on lossless policies, rather than silently ignored.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize)]
#[serde(tag = "format", rename_all = "snake_case", deny_unknown_fields)]
pub enum CaptureFormatPolicy {
    #[default]
    Png,
    Jpeg {
        quality: CaptureImageQuality,
    },
    WebpLossless,
    WebpLossy {
        quality: CaptureImageQuality,
    },
}

impl<'de> Deserialize<'de> for CaptureFormatPolicy {
    fn deserialize<D: serde::Deserializer<'de>>(
        deserializer: D,
    ) -> std::result::Result<Self, D::Error> {
        // Serde's internally tagged unit variants otherwise discard extra fields.
        #[derive(Deserialize)]
        #[serde(tag = "format", rename_all = "snake_case", deny_unknown_fields)]
        enum StrictPolicy {
            Png {},
            Jpeg { quality: CaptureImageQuality },
            WebpLossless {},
            WebpLossy { quality: CaptureImageQuality },
        }
        Ok(match deserialize_object(deserializer)? {
            StrictPolicy::Png {} => Self::Png,
            StrictPolicy::Jpeg { quality } => Self::Jpeg { quality },
            StrictPolicy::WebpLossless {} => Self::WebpLossless,
            StrictPolicy::WebpLossy { quality } => Self::WebpLossy { quality },
        })
    }
}

/// Preserve is a byte copy. Conversion always names the new encoding explicitly.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize)]
#[serde(tag = "mode", rename_all = "snake_case", deny_unknown_fields)]
pub enum LibraryExportPolicy {
    #[default]
    Preserve,
    Convert {
        encoding: CaptureFormatPolicy,
    },
}

impl<'de> Deserialize<'de> for LibraryExportPolicy {
    fn deserialize<D: serde::Deserializer<'de>>(
        deserializer: D,
    ) -> std::result::Result<Self, D::Error> {
        #[derive(Deserialize)]
        #[serde(tag = "mode", rename_all = "snake_case", deny_unknown_fields)]
        enum StrictPolicy {
            Preserve {},
            Convert { encoding: CaptureFormatPolicy },
        }
        Ok(match deserialize_object(deserializer)? {
            StrictPolicy::Preserve {} => Self::Preserve,
            StrictPolicy::Convert { encoding } => Self::Convert { encoding },
        })
    }
}

impl CaptureFormatPolicy {
    pub fn format(self) -> crate::ImageExportFormat {
        match self {
            Self::Png => crate::ImageExportFormat::Png,
            Self::Jpeg { .. } => crate::ImageExportFormat::Jpeg,
            Self::WebpLossless | Self::WebpLossy { .. } => crate::ImageExportFormat::WebP,
        }
    }

    pub fn extension(self) -> &'static str {
        self.format().extension()
    }

    pub fn mime_type(self) -> &'static str {
        image_mime_type(self.format())
    }

    pub fn is_lossy(self) -> bool {
        matches!(self, Self::Jpeg { .. } | Self::WebpLossy { .. })
    }

    pub fn encoded_limit(self) -> u64 {
        encoded_limit(self.format())
    }

    /// Select the filename before capture; changing this never changes pixel ownership.
    pub fn destination(self, png_default: &Path) -> PathBuf {
        png_default.with_extension(self.extension())
    }

    pub fn validate_destination(self, path: &Path) -> Result<()> {
        ensure!(
            crate::ImageExportFormat::from_path(path)? == self.format(),
            "Image policy does not match the destination extension"
        );
        Ok(())
    }

    pub fn validate_dimensions(self, width: u32, height: u32) -> Result<()> {
        if matches!(self, Self::WebpLossy { .. }) {
            crate::webp_lossy::validate_output_size(width, height)?;
        } else {
            self.format().validate_output_size(width, height)?;
        }
        Ok(())
    }

    /// Validate transparency before reserving a journal or creating output files.
    pub fn validate_pixels(self, pixels: &RgbaImage, cancelled: &impl Fn() -> bool) -> Result<()> {
        self.validate_dimensions(pixels.width(), pixels.height())?;
        let mut work = crate::image_cancel::ImageWork::new(cancelled)?;
        ensure!(
            pixels.as_raw().capacity() as u64 <= crate::MAX_CAPTURE_BYTES
                && pixels.as_raw().len() as u64
                    == u64::from(pixels.width()) * u64::from(pixels.height()) * 4,
            "Capture image backing exceeds 256 MiB"
        );
        if matches!(self, Self::Jpeg { .. }) {
            for pixel in pixels.pixels() {
                work.pixel()?;
                ensure!(
                    pixel[3] == 255,
                    "JPEG cannot preserve transparency. Choose PNG or WebP."
                );
            }
        }
        work.check()?;
        Ok(())
    }

    /// Encode directly into an owned journal stage; no encoded full-frame Vec.
    pub(crate) fn encode_to(
        self,
        pixels: &RgbaImage,
        writer: &mut dyn Write,
        cancelled: &impl Fn() -> bool,
    ) -> Result<()> {
        self.validate_pixels(pixels, cancelled)?;
        let mut limited = LimitedWriter {
            inner: writer,
            remaining: self.encoded_limit(),
        };
        if let Self::WebpLossy { quality } = self {
            crate::webp_lossy::encode_lossy_webp_to(
                pixels,
                &mut limited,
                quality.get(),
                cancelled,
            )?;
        } else {
            let mut buffered =
                crate::image_cancel::CheckedImageWriter::new(&mut limited, cancelled)?;
            match self {
                Self::Png => image::codecs::png::PngEncoder::new_with_quality(
                    &mut buffered,
                    image::codecs::png::CompressionType::Fast,
                    image::codecs::png::FilterType::Adaptive,
                )
                .write_image(
                    pixels.as_raw(),
                    pixels.width(),
                    pixels.height(),
                    image::ExtendedColorType::Rgba8,
                )?,
                Self::Jpeg { quality } => {
                    image::codecs::jpeg::JpegEncoder::new_with_quality(&mut buffered, quality.get())
                        .encode_image(pixels)?
                }
                Self::WebpLossless => image::codecs::webp::WebPEncoder::new_lossless(&mut buffered)
                    .write_image(
                        pixels.as_raw(),
                        pixels.width(),
                        pixels.height(),
                        image::ExtendedColorType::Rgba8,
                    )?,
                Self::WebpLossy { .. } => unreachable!(),
            }
            buffered.flush()?;
        }
        ensure!(!cancelled(), crate::CaptureError::Canceled);
        Ok(())
    }
}

pub(crate) fn image_mime_type(format: crate::ImageExportFormat) -> &'static str {
    match format {
        crate::ImageExportFormat::Png => "image/png",
        crate::ImageExportFormat::Jpeg => "image/jpeg",
        crate::ImageExportFormat::WebP => "image/webp",
    }
}

pub(crate) fn format_from_mime(mime: &str) -> Result<crate::ImageExportFormat> {
    match mime {
        "image/png" => Ok(crate::ImageExportFormat::Png),
        "image/jpeg" => Ok(crate::ImageExportFormat::Jpeg),
        "image/webp" => Ok(crate::ImageExportFormat::WebP),
        _ => anyhow::bail!("History image MIME must be PNG, JPEG or WebP"),
    }
}

pub(crate) fn is_png(format: &crate::ImageExportFormat) -> bool {
    *format == crate::ImageExportFormat::Png
}

pub(crate) fn encoded_limit(format: crate::ImageExportFormat) -> u64 {
    if format == crate::ImageExportFormat::Png {
        MAX_SAVED_IMAGE_BYTES
    } else {
        MAX_SAVED_JPEG_WEBP_BYTES
    }
}

struct LimitedWriter<'a> {
    inner: &'a mut dyn Write,
    remaining: u64,
}

impl Write for LimitedWriter<'_> {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        if bytes.len() as u64 > self.remaining {
            return Err(std::io::Error::other(
                "Capture encoding exceeds its bounded file limit",
            ));
        }
        let count = self.inner.write(bytes)?;
        self.remaining -= count as u64;
        Ok(count)
    }
    fn flush(&mut self) -> std::io::Result<()> {
        self.inner.flush()
    }
}

/// Separate from UI settings, whose legacy saves reconstruct known fields.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct CaptureFormatSettings {
    version: VersionOne,
    pub capture: CaptureFormatPolicy,
    pub library_export: LibraryExportPolicy,
}

impl<'de> Deserialize<'de> for CaptureFormatSettings {
    fn deserialize<D: serde::Deserializer<'de>>(
        deserializer: D,
    ) -> std::result::Result<Self, D::Error> {
        #[derive(Deserialize)]
        #[serde(deny_unknown_fields)]
        struct StrictSettings {
            version: VersionOne,
            #[serde(default)]
            capture: CaptureFormatPolicy,
            #[serde(default)]
            library_export: LibraryExportPolicy,
        }
        let settings: StrictSettings = deserialize_object(deserializer)?;
        Ok(Self {
            version: settings.version,
            capture: settings.capture,
            library_export: settings.library_export,
        })
    }
}

/// Observed file presence is distinct from the effective defaults for a missing file.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum CaptureFormatPresence {
    Absent,
    Present(CaptureFormatSettings),
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
struct VersionOne;
impl Serialize for VersionOne {
    fn serialize<S: serde::Serializer>(
        &self,
        serializer: S,
    ) -> std::result::Result<S::Ok, S::Error> {
        serializer.serialize_u8(1)
    }
}
impl<'de> Deserialize<'de> for VersionOne {
    fn deserialize<D: serde::Deserializer<'de>>(
        deserializer: D,
    ) -> std::result::Result<Self, D::Error> {
        if u8::deserialize(deserializer)? == 1 {
            Ok(Self)
        } else {
            Err(serde::de::Error::custom(
                "Unsupported capture format settings version",
            ))
        }
    }
}

impl CaptureFormatSettings {
    pub fn for_policy(capture: CaptureFormatPolicy) -> Self {
        Self {
            version: VersionOne,
            capture,
            library_export: LibraryExportPolicy::Preserve,
        }
    }

    pub fn config_path() -> Result<PathBuf> {
        Ok(crate::CaptureSettings::config_path()?.with_file_name("capture-format.json"))
    }

    pub fn load() -> Result<Self> {
        Self::load_at(&Self::config_path()?)
    }

    pub fn load_at(path: &Path) -> Result<Self> {
        Ok(match Self::observe_at(path)? {
            CaptureFormatPresence::Absent => Self::default(),
            CaptureFormatPresence::Present(settings) => settings,
        })
    }

    pub fn observe_at(path: &Path) -> Result<CaptureFormatPresence> {
        match Self::open_catalog(path) {
            Ok(file) => Ok(CaptureFormatPresence::Present(Self::read_opened(file)?)),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                Ok(CaptureFormatPresence::Absent)
            }
            Err(error) => Err(error.into()),
        }
    }

    fn open_catalog(path: &Path) -> std::io::Result<fs::File> {
        let mut options = OpenOptions::new();
        options.read(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.custom_flags(libc::O_NONBLOCK);
        }
        #[cfg(test)]
        BEFORE_CATALOG_OPEN.with(|hook| {
            if let Some(before_open) = hook.take() {
                before_open(path);
            }
        });
        options.open(path)
    }

    fn read_opened(mut file: fs::File) -> Result<Self> {
        let admitted = crate::FileIdentity::from_file(&file)?;
        let metadata = file.metadata()?;
        #[cfg(test)]
        AFTER_CATALOG_METADATA.with(|hook| {
            if let Some(after_metadata) = hook.take() {
                after_metadata();
            }
        });
        ensure!(
            metadata.is_file() && metadata.len() <= MAX_SETTINGS_BYTES,
            "Capture format settings must be a regular file up to 16 KiB"
        );
        let mut bytes = Vec::with_capacity(usize::try_from(metadata.len())?);
        (&mut file).take(metadata.len()).read_to_end(&mut bytes)?;
        ensure!(
            bytes.len() as u64 <= MAX_SETTINGS_BYTES,
            "Capture format settings exceed 16 KiB"
        );
        admitted.verify_file(&file)?;
        serde_json::from_slice(&bytes).context("Invalid capture format settings")
    }

    pub fn save(&self) -> Result<()> {
        self.save_at(&Self::config_path()?)
    }

    /// Explicit credential-free backup; never replaces an existing backup.
    pub fn export(&self, path: &Path) -> Result<()> {
        let bytes = serde_json::to_vec_pretty(self)?;
        crate::capture::write_output(path, |file| {
            file.write_all(&bytes)?;
            Ok(())
        })?;
        Ok(())
    }

    /// Read an explicit backup. A missing backup is an error, unlike missing preferences.
    pub fn import(path: &Path) -> Result<Self> {
        Self::read_opened(
            Self::open_catalog(path).context("Capture format settings backup is unavailable")?,
        )
    }

    pub fn save_at(&self, path: &Path) -> Result<()> {
        let bytes = serde_json::to_vec_pretty(self)?;
        ensure!(
            bytes.len() as u64 <= MAX_SETTINGS_BYTES,
            "Capture format settings exceed 16 KiB"
        );
        let parent = path
            .parent()
            .filter(|p| !p.as_os_str().is_empty())
            .context("Capture format settings need a parent directory")?;
        crate::history::ensure_storage_directory(parent)?;
        let temporary = parent.join(format!(".capture-format-{}.part", uuid::Uuid::new_v4()));
        let result = (|| -> Result<()> {
            let mut file = OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(&temporary)?;
            file.write_all(&bytes)?;
            file.sync_all()?;
            drop(file);
            fs::rename(&temporary, path)?;
            #[cfg(unix)]
            fs::File::open(parent)?.sync_all()?;
            Ok(())
        })();
        if result.is_err() {
            let _ = fs::remove_file(&temporary);
        }
        result
    }
}

#[cfg(test)]
#[path = "capture_format_tests.rs"]
mod tests;
