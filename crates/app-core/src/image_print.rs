//! Metadata-free one-page printing from final managed sRGB pixels.
//! Blocking writers/native print APIs belong in an operation-owned isolated worker.
use anyhow::{Result, ensure};
use image::RgbaImage;
use std::io::Write;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PrintPaper {
    A4,
    Letter,
}
impl PrintPaper {
    pub fn points(self) -> (f64, f64) {
        match self {
            Self::A4 => (210.0 * 72.0 / 25.4, 297.0 * 72.0 / 25.4),
            Self::Letter => (612.0, 792.0),
        }
    }
}

/// Aspect-preserving centered fit, allowing enlargement, with a 1/4 inch margin.
pub fn print_fit(width: u32, height: u32, paper: PrintPaper) -> Result<(f64, f64, f64, f64)> {
    ensure!(width > 0 && height > 0, "Print image is empty");
    let (page_width, page_height) = paper.points();
    let scale =
        ((page_width - 36.0) / f64::from(width)).min((page_height - 36.0) / f64::from(height));
    let width = f64::from(width) * scale;
    let height = f64::from(height) * scale;
    Ok((
        (page_width - width) / 2.0,
        (page_height - height) / 2.0,
        width,
        height,
    ))
}

pub fn print_rgb(pixel: &[u8]) -> [u8; 3] {
    let alpha = u32::from(pixel[3]);
    std::array::from_fn(|channel| {
        ((u32::from(pixel[channel]) * alpha + 255 * (255 - alpha) + 127) / 255) as u8
    })
}

/// Streams only RGB samples and numeric geometry; no source metadata or names.
/// At most 64 KiB staging; RGBA is borrowed. Output is <=7 bytes/pixel + 4 KiB.
pub fn write_print_postscript(
    image: &RgbaImage,
    paper: PrintPaper,
    mut output: impl Write,
    cancelled: &impl Fn() -> bool,
) -> Result<()> {
    ensure!(!cancelled(), "Print canceled before submission");
    ensure!(
        image.as_raw().len() as u64 <= crate::MAX_CAPTURE_BYTES,
        "Print pixels exceed image budget"
    );
    let (x, y, width, height) = print_fit(image.width(), image.height(), paper)?;
    let (page_width, page_height) = paper.points();
    write!(
        output,
        "%!PS-Adobe-3.0\n%%Pages: 1\n%%Title: Screenshot\n%%EndComments\n<< /PageSize [{page_width:.6} {page_height:.6}] >> setpagedevice\n%%Page: 1 1\ngsave\n{x:.6} {y:.6} translate\n{width:.6} {height:.6} scale\n{} {} 8 [{} 0 0 -{} 0 {}]\n/pixel 3 string def\n{{ currentfile pixel readhexstring pop }} false 3 colorimage\n",
        image.width(),
        image.height(),
        image.width(),
        image.height(),
        image.height()
    )?;
    let mut bytes = Vec::with_capacity(64 * 1024);
    const HEX: &[u8; 16] = b"0123456789abcdef";
    for (index, pixel) in image.as_raw().chunks_exact(4).enumerate() {
        if bytes.len() + 7 > 64 * 1024 {
            ensure!(!cancelled(), "Print canceled; spool outcome is unknown");
            output.write_all(&bytes)?;
            bytes.clear();
        }
        for value in print_rgb(pixel) {
            bytes.push(HEX[usize::from(value >> 4)]);
            bytes.push(HEX[usize::from(value & 15)]);
        }
        // Bounded ASCII lines also work with conservative PostScript spool filters.
        if index % 13 == 12 {
            bytes.push(b'\n');
        }
    }
    output.write_all(&bytes)?;
    output.write_all(b"\ngrestore\nshowpage\n%%EOF\n")?;
    output.flush()?;
    Ok(())
}

#[cfg(target_os = "windows")]
#[path = "image_print_windows.rs"]
mod windows;
#[cfg(target_os = "windows")]
pub use windows::print_windows_pixels;

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn alpha_and_aspect_are_explicit() -> Result<()> {
        assert_eq!(print_rgb(&[0, 20, 255, 0]), [255, 255, 255]);
        assert_eq!(print_rgb(&[0, 20, 255, 255]), [0, 20, 255]);
        assert_eq!(print_rgb(&[0, 0, 0, 128]), [127, 127, 127]);
        let (x, y, width, height) = print_fit(200, 100, PrintPaper::Letter)?;
        assert_eq!((x, width, height), (18.0, 576.0, 288.0));
        assert_eq!(y, 252.0);
        assert!(print_fit(0, 1, PrintPaper::A4).is_err());
        Ok(())
    }
    #[test]
    fn stream_contains_only_final_pixels_and_fixed_title() -> Result<()> {
        let image = RgbaImage::from_raw(2, 1, vec![1, 2, 3, 255, 0, 0, 0, 0]).unwrap();
        let mut output = Vec::new();
        write_print_postscript(&image, PrintPaper::Letter, &mut output, &|| false)?;
        let output = String::from_utf8(output)?;
        assert!(output.contains("010203ffffff"));
        assert!(output.starts_with("%!PS-Adobe-3.0\n%%Pages: 1\n"));
        assert!(output.ends_with("showpage\n%%EOF\n"));
        assert_eq!(output.matches("showpage").count(), 1);
        assert!(!output.contains("ICC") && !output.contains("EXIF"));
        assert!(write_print_postscript(&image, PrintPaper::A4, Vec::new(), &|| true).is_err());
        Ok(())
    }
    #[test]
    fn writer_failure_is_not_success() {
        struct Broken;
        impl Write for Broken {
            fn write(&mut self, _: &[u8]) -> std::io::Result<usize> {
                Err(std::io::ErrorKind::BrokenPipe.into())
            }
            fn flush(&mut self) -> std::io::Result<()> {
                Ok(())
            }
        }
        assert!(
            write_print_postscript(&RgbaImage::new(1, 1), PrintPaper::A4, Broken, &|| false)
                .is_err()
        );
    }
}
