//! Explicit exact RGBA8 BMP output, without changing selectable capture formats.

use crate::CaptureError;
use image::{ImageEncoder, RgbaImage};
use std::{
    io::{BufWriter, Write},
    path::Path,
};

pub fn save_bmp(pixels: &RgbaImage, destination: &Path) -> Result<(), CaptureError> {
    if !destination
        .extension()
        .is_some_and(|value| value.eq_ignore_ascii_case("bmp"))
    {
        return Err(CaptureError::Backend(
            "BMP output requires a .bmp extension".into(),
        ));
    }
    crate::validate_image_size(pixels.width(), pixels.height())?;
    let expected = u64::from(pixels.width()) * u64::from(pixels.height()) * 4;
    if pixels.as_raw().len() as u64 != expected {
        return Err(CaptureError::Backend(
            "BMP pixel backing does not match its dimensions".into(),
        ));
    }
    crate::capture::write_output(destination, |file| {
        let mut writer = BufWriter::with_capacity(64 * 1024, file);
        image::codecs::bmp::BmpEncoder::new(&mut writer).write_image(
            pixels.as_raw(),
            pixels.width(),
            pixels.height(),
            image::ExtendedColorType::Rgba8,
        )?;
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
                std::env::temp_dir().join(format!("screenshots-bmp-{}", uuid::Uuid::new_v4()));
            std::fs::create_dir(&root).unwrap();
            Self(root)
        }
        fn output(&self) -> std::path::PathBuf {
            self.0.join("output.BMP")
        }
    }
    impl Drop for Fixture {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn rgba_round_trip_keeps_odd_dimensions_alpha_and_hidden_colors() {
        let fixture = Fixture::new();
        let pixels = RgbaImage::from_fn(3, 5, |x, y| {
            Rgba([x as u8 * 60, y as u8 * 30, 197, [0, 128, 255][x as usize]])
        });
        let original = pixels.clone();
        save_bmp(&pixels, &fixture.output()).unwrap();
        assert_eq!(image::open(fixture.output()).unwrap().into_rgba8(), pixels);
        assert_eq!(pixels, original);
        let bytes = std::fs::read(fixture.output()).unwrap();
        let word = |offset| u32::from_le_bytes(bytes[offset..offset + 4].try_into().unwrap());
        assert_eq!(&bytes[..2], b"BM");
        assert_eq!(word(2) as usize, bytes.len());
        assert_eq!(word(10), 122);
        assert_eq!(word(14), 108);
        assert_eq!(word(18), 3);
        assert_eq!(word(22), 5);
        assert_eq!(u16::from_le_bytes(bytes[28..30].try_into().unwrap()), 32);
        assert_eq!(word(30), 3);
        assert_eq!(
            [word(54), word(58), word(62), word(66)],
            [0x00ff0000, 0x0000ff00, 0x000000ff, 0xff000000]
        );
        assert_eq!(bytes.len(), 122 + 3 * 5 * 4);
        assert_eq!(std::fs::read_dir(&fixture.0).unwrap().count(), 1);
    }

    #[test]
    fn invalid_inputs_and_destinations_never_publish_or_replace() {
        let fixture = Fixture::new();
        let pixels = RgbaImage::from_pixel(1, 1, Rgba([1, 2, 3, 4]));
        assert!(save_bmp(&RgbaImage::new(0, 1), &fixture.output()).is_err());
        assert!(save_bmp(&pixels, &fixture.0.join("wrong.png")).is_err());
        let oversized_backing = RgbaImage::from_raw(1, 1, vec![1; 8]).unwrap();
        assert!(save_bmp(&oversized_backing, &fixture.output()).is_err());
        assert!(!fixture.output().exists());
        std::fs::write(fixture.output(), b"preserve existing").unwrap();
        assert!(save_bmp(&pixels, &fixture.output()).is_err());
        assert_eq!(
            std::fs::read(fixture.output()).unwrap(),
            b"preserve existing"
        );
        let directory = fixture.0.join("directory.bmp");
        std::fs::create_dir(&directory).unwrap();
        assert!(save_bmp(&pixels, &directory).is_err());
        assert_eq!(std::fs::read_dir(&fixture.0).unwrap().count(), 2);
    }

    #[cfg(unix)]
    #[test]
    fn symlink_collision_never_modifies_target_or_leaves_staging() {
        let fixture = Fixture::new();
        let target = fixture.0.join("target");
        std::fs::write(&target, b"keep").unwrap();
        std::os::unix::fs::symlink(&target, fixture.output()).unwrap();
        assert!(
            save_bmp(
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
