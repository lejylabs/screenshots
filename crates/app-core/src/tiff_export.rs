//! Explicit uncompressed straight-RGBA TIFF output without changing capture formats.

use crate::CaptureError;
use image::RgbaImage;
use std::{
    io::{BufWriter, Write},
    path::Path,
};

pub fn save_tiff(pixels: &RgbaImage, destination: &Path) -> Result<(), CaptureError> {
    if !destination.extension().is_some_and(|value| {
        value.eq_ignore_ascii_case("tif") || value.eq_ignore_ascii_case("tiff")
    }) {
        return Err(CaptureError::Backend(
            "TIFF output requires a .tif or .tiff extension".into(),
        ));
    }
    crate::validate_image_size(pixels.width(), pixels.height())?;
    if pixels.as_raw().len() as u64 != u64::from(pixels.width()) * u64::from(pixels.height()) * 4 {
        return Err(CaptureError::Backend(
            "TIFF pixel backing does not match its dimensions".into(),
        ));
    }
    crate::capture::write_output(destination, |file| {
        let mut writer = BufWriter::with_capacity(64 * 1024, file);
        let encode = (|| -> Result<(), tiff::TiffError> {
            let mut encoder = tiff::encoder::TiffEncoder::new(&mut writer)?;
            let mut output = encoder
                .new_image::<tiff::encoder::colortype::RGBA8>(pixels.width(), pixels.height())?;
            // Label the existing fourth channel; extra_samples() would add a fifth one.
            output
                .encoder()
                .write_tag(tiff::tags::Tag::ExtraSamples, &[2u16][..])?;
            output.write_data(pixels.as_raw())?;
            Ok(())
        })();
        encode.map_err(|error| match error {
            tiff::TiffError::IoError(error) => CaptureError::Io(error),
            _ => CaptureError::Backend("TIFF encoding failed".into()),
        })?;
        writer.flush()?;
        Ok(())
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use image::Rgba;
    struct Fixture(std::path::PathBuf);
    impl Fixture {
        fn new() -> Self {
            let root =
                std::env::temp_dir().join(format!("screenshots-tiff-{}", uuid::Uuid::new_v4()));
            std::fs::create_dir(&root).unwrap();
            Self(root)
        }
        fn output(&self) -> std::path::PathBuf {
            self.0.join("output.TIFF")
        }
    }
    impl Drop for Fixture {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn straight_alpha_tags_and_exact_odd_rgba_pixels_round_trip() {
        let fixture = Fixture::new();
        let pixels = RgbaImage::from_fn(3, 5, |x, y| {
            Rgba([x as u8 * 60, y as u8 * 30, 197, [0, 128, 255][x as usize]])
        });
        let original = pixels.clone();
        save_tiff(&pixels, &fixture.output()).unwrap();
        assert_eq!(image::open(fixture.output()).unwrap().into_rgba8(), pixels);
        assert_eq!(pixels, original);
        let mut decoder = tiff::decoder::Decoder::new(std::io::BufReader::new(
            std::fs::File::open(fixture.output()).unwrap(),
        ))
        .unwrap();
        assert_eq!(decoder.dimensions().unwrap(), (3, 5));
        assert_eq!(
            decoder
                .get_tag_u32(tiff::tags::Tag::SamplesPerPixel)
                .unwrap(),
            4
        );
        assert_eq!(
            decoder
                .get_tag_u16_vec(tiff::tags::Tag::ExtraSamples)
                .unwrap(),
            [2]
        );
        assert_eq!(
            decoder.get_tag_u32(tiff::tags::Tag::Compression).unwrap(),
            1
        );
        assert!(!decoder.more_images());
        assert_eq!(std::fs::read_dir(&fixture.0).unwrap().count(), 1);
    }

    #[test]
    fn invalid_input_and_existing_destinations_never_publish_or_replace() {
        let fixture = Fixture::new();
        let pixels = RgbaImage::from_pixel(1, 1, Rgba([1, 2, 3, 4]));
        assert!(save_tiff(&RgbaImage::new(0, 1), &fixture.output()).is_err());
        assert!(save_tiff(&pixels, &fixture.0.join("wrong.png")).is_err());
        let extra = RgbaImage::from_raw(1, 1, vec![1; 8]).unwrap();
        assert!(save_tiff(&extra, &fixture.output()).is_err());
        assert!(!fixture.output().exists());
        std::fs::write(fixture.output(), b"preserve existing").unwrap();
        assert!(save_tiff(&pixels, &fixture.output()).is_err());
        assert_eq!(
            std::fs::read(fixture.output()).unwrap(),
            b"preserve existing"
        );
        let directory = fixture.0.join("directory.tif");
        std::fs::create_dir(&directory).unwrap();
        assert!(save_tiff(&pixels, &directory).is_err());
        assert_eq!(std::fs::read_dir(&fixture.0).unwrap().count(), 2);
    }

    #[cfg(unix)]
    #[test]
    fn symlink_collision_preserves_target_without_staging_leaks() {
        let fixture = Fixture::new();
        let target = fixture.0.join("target");
        std::fs::write(&target, b"keep").unwrap();
        std::os::unix::fs::symlink(&target, fixture.output()).unwrap();
        assert!(
            save_tiff(
                &RgbaImage::from_pixel(1, 1, Rgba([0, 0, 0, 0])),
                &fixture.output()
            )
            .is_err()
        );
        assert_eq!(std::fs::read(target).unwrap(), b"keep");
        assert!(
            fixture
                .output()
                .symlink_metadata()
                .unwrap()
                .file_type()
                .is_symlink()
        );
        assert_eq!(std::fs::read_dir(&fixture.0).unwrap().count(), 2);
    }
}
