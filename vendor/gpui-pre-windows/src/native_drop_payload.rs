//! Held adapter: callable only from the separately contained receiving STA.
//! R10 proves the native prerequisite; this production candidate remains held.
use anyhow::{Result, bail, ensure};
use gpui::{ExternalPaths, ImageFormat, http_client::Url};
use std::sync::Arc;
use windows::{
    Win32::{
        Foundation::HGLOBAL,
        System::{
            Com::{DVASPECT_CONTENT, FORMATETC, IDataObject, STGMEDIUM, TYMED_HGLOBAL},
            DataExchange::RegisterClipboardFormatW,
            Memory::{GlobalLock, GlobalSize, GlobalUnlock},
            Ole::{CF_DIB, CF_DIBV5, CF_HDROP, ReleaseStgMedium},
        },
    },
    core::{PCWSTR, w},
};

const METADATA_BYTES: usize = 64 * 1024;
const IMAGE_BYTES: usize = 32 * 1024 * 1024;
const URL_BYTES: usize = 4096;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum Format {
    Files,
    Png,
    Jpeg,
    Webp,
    DibV5,
    Dib,
    UrlWide,
    UrlAscii,
}
pub(super) enum Payload {
    Files(ExternalPaths),
    Image {
        format: ImageFormat,
        bytes: Arc<Vec<u8>>,
    },
    Url(Arc<str>),
}
// Payloads intentionally have no Debug implementation: paths and URL contents
// must never be logged by the native acquisition failure path.
pub(super) struct Formats([(Format, u16); 8]);
impl Formats {
    pub(super) fn register() -> Result<Self> {
        fn registered(name: PCWSTR) -> Result<u16> {
            let value = unsafe { RegisterClipboardFormatW(name) };
            ensure!(
                value != 0 && value <= u16::MAX as u32,
                "Native drop format unavailable"
            );
            Ok(value as u16)
        }
        Ok(Self([
            (Format::Files, CF_HDROP.0),
            (Format::Png, registered(w!("PNG"))?),
            (Format::Jpeg, registered(w!("JFIF"))?),
            (Format::Webp, registered(w!("image/webp"))?),
            (Format::DibV5, CF_DIBV5.0),
            (Format::Dib, CF_DIB.0),
            (Format::UrlWide, registered(w!("UniformResourceLocatorW"))?),
            (Format::UrlAscii, registered(w!("UniformResourceLocator"))?),
        ]))
    }
    fn descriptor(&self, format: Format) -> FORMATETC {
        FORMATETC {
            cfFormat: self.0.iter().find(|entry| entry.0 == format).unwrap().1,
            ptd: std::ptr::null_mut(),
            dwAspect: DVASPECT_CONTENT.0,
            lindex: -1,
            tymed: TYMED_HGLOBAL.0 as u32,
        }
    }
    /// A fixed query list avoids attacker-controlled enumeration/allocation.
    pub(super) fn negotiate(
        &self,
        source: &IDataObject,
        check: &dyn Fn() -> Result<()>,
    ) -> Result<Format> {
        for &(format, _) in &self.0 {
            check()?;
            let supported = unsafe { source.QueryGetData(&self.descriptor(format)) }
                == windows::Win32::Foundation::S_OK;
            check()?;
            if supported {
                return Ok(format);
            }
        }
        bail!("Drop does not contain a supported file, image or HTTP(S) URL")
    }

    /// No foreground caller is permitted. The external Job/deadline must already
    /// cover queries, unmarshaling, copying and source/medium release together.
    pub(super) fn acquire(
        &self,
        source: &IDataObject,
        format: Format,
        check: &dyn Fn() -> Result<()>,
    ) -> Result<Payload> {
        check()?;
        let medium = Medium(
            unsafe { source.GetData(&self.descriptor(format)) }
                .map_err(|_| anyhow::anyhow!("Could not read native drop data"))?,
        );
        check()?;
        ensure!(
            medium.0.tymed == TYMED_HGLOBAL.0 as u32,
            "Native drop returned an unsupported medium"
        );
        let global = unsafe { medium.0.u.hGlobal };
        let payload = if format == Format::Files {
            Payload::Files(crate::clipboard::read_file_paths(global).ok_or_else(|| {
                anyhow::anyhow!("Native file drop metadata is invalid or oversized")
            })?)
        } else {
            let size = unsafe { GlobalSize(global) };
            let maximum = if matches!(format, Format::UrlWide | Format::UrlAscii) {
                METADATA_BYTES
            } else {
                IMAGE_BYTES
            };
            ensure!(
                size > 0 && size <= maximum,
                "Native drop exceeds its transfer bound"
            );
            let locked = Locked::new(global)?;
            let bytes = unsafe { std::slice::from_raw_parts(locked.1.cast::<u8>(), size) };
            match format {
                Format::Png | Format::Jpeg | Format::Webp => {
                    let kind = match format {
                        Format::Png => ImageFormat::Png,
                        Format::Jpeg => ImageFormat::Jpeg,
                        _ => ImageFormat::Webp,
                    };
                    Payload::Image {
                        format: kind,
                        bytes: Arc::new(copy(bytes, 0)?),
                    }
                }
                Format::DibV5 | Format::Dib => Payload::Image {
                    format: ImageFormat::Bmp,
                    bytes: Arc::new(dib_as_bmp(bytes)?),
                },
                Format::UrlWide | Format::UrlAscii => {
                    Payload::Url(parse_url(bytes, format == Format::UrlWide)?)
                }
                Format::Files => unreachable!(),
            }
        };
        drop(medium);
        // A successfully copied body is still private until source release and
        // the controller's retained-generation/cancellation check both complete.
        check()?;
        Ok(payload)
    }
}
struct Medium(STGMEDIUM);
impl Drop for Medium {
    fn drop(&mut self) {
        unsafe { ReleaseStgMedium(&mut self.0) };
    }
}
struct Locked(HGLOBAL, *mut std::ffi::c_void);
impl Locked {
    fn new(global: HGLOBAL) -> Result<Self> {
        let pointer = unsafe { GlobalLock(global) };
        ensure!(!pointer.is_null(), "Native drop storage is unavailable");
        Ok(Self(global, pointer))
    }
}
impl Drop for Locked {
    fn drop(&mut self) {
        let _ = unsafe { GlobalUnlock(self.0) };
    }
}
fn copy(bytes: &[u8], prefix: usize) -> Result<Vec<u8>> {
    let size = bytes
        .len()
        .checked_add(prefix)
        .ok_or_else(|| anyhow::anyhow!("Native image extent overflow"))?;
    ensure!(size <= IMAGE_BYTES, "Native image exceeds32 MiB");
    let mut output = Vec::new();
    output.try_reserve_exact(size)?;
    ensure!(
        output.capacity() <= IMAGE_BYTES,
        "Native image allocation exceeds32 MiB"
    );
    output.resize(prefix, 0);
    output.extend_from_slice(bytes);
    Ok(output)
}
fn dib_as_bmp(bytes: &[u8]) -> Result<Vec<u8>> {
    ensure!(bytes.len() >= 40, "Native bitmap header is truncated");
    let field = |offset| u32::from_le_bytes(bytes[offset..offset + 4].try_into().unwrap());
    let header = field(0) as usize;
    ensure!(
        matches!(header, 40 | 108 | 124) && bytes.len() >= header,
        "Native bitmap header is unsupported"
    );
    let depth = u16::from_le_bytes(bytes[14..16].try_into().unwrap());
    let compression = field(16);
    ensure!(
        matches!(depth, 24 | 32)
            && (compression == 0 || (compression == 3 && depth == 32))
            && field(32) == 0,
        "Native bitmap layout is unsupported"
    );
    if header == 124 {
        ensure!(
            field(112) == 0 && field(116) == 0,
            "Native bitmap profiles require encoded image import"
        );
    }
    let offset = header
        + if header == 40 && compression == 3 {
            12
        } else {
            0
        };
    ensure!(bytes.len() >= offset, "Native bitmap masks are truncated");
    // One representation copy adds the 14-byte BMP header in place. There is no
    // image encoding, PNG staging, pixel decode or additional DIB vector here.
    let mut output = copy(bytes, 14)?;
    let extent = output.len() as u32;
    output[..2].copy_from_slice(b"BM");
    output[2..6].copy_from_slice(&extent.to_le_bytes());
    output[10..14].copy_from_slice(&((offset + 14) as u32).to_le_bytes());
    Ok(output)
}
fn parse_url(bytes: &[u8], wide: bool) -> Result<Arc<str>> {
    ensure!(
        bytes.len() <= METADATA_BYTES,
        "Native URL metadata exceeds64 KiB"
    );
    let text = if wide {
        ensure!(
            bytes.len().is_multiple_of(2),
            "Native Unicode URL extent is invalid"
        );
        let mut units = Vec::new();
        let mut terminated = false;
        for pair in bytes.as_chunks::<2>().0 {
            let unit = u16::from_le_bytes(*pair);
            if unit == 0 {
                terminated = true;
            } else {
                ensure!(
                    !terminated && units.len() < URL_BYTES,
                    "Native URL is multiple or oversized"
                );
                units.push(unit);
            }
        }
        ensure!(terminated, "Native URL has no terminator");
        String::from_utf16(&units)?
    } else {
        let end = bytes
            .iter()
            .position(|byte| *byte == 0)
            .ok_or_else(|| anyhow::anyhow!("Native URL has no terminator"))?;
        ensure!(
            end <= URL_BYTES
                && bytes[end..].iter().all(|byte| *byte == 0)
                && bytes[..end].is_ascii(),
            "Native ASCII URL is multiple, oversized or non-ASCII"
        );
        std::str::from_utf8(&bytes[..end])?.to_owned()
    };
    ensure!(
        !text.is_empty()
            && text.len() <= URL_BYTES
            && !text.chars().any(|c| c.is_control() || c.is_whitespace()),
        "Native URL is empty, oversized or contains whitespace"
    );
    let parsed = Url::parse(&text)?;
    ensure!(
        matches!(parsed.scheme(), "http" | "https")
            && parsed.has_host()
            && parsed.username().is_empty()
            && parsed.password().is_none(),
        "Drop exactly one HTTP(S) URL without credentials"
    );
    ensure!(
        parsed.as_str().len() <= URL_BYTES,
        "Canonical native URL exceeds4096 bytes"
    );
    Ok(Arc::from(parsed.as_str()))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn native_url_requires_one_terminated_safe_reference() {
        assert_eq!(
            &*parse_url(b"https://example.invalid/a%20b.png\0", false).unwrap(),
            "https://example.invalid/a%20b.png"
        );
        assert_eq!(
            &*parse_url(b"HTTP://EXAMPLE.INVALID/a\0", false).unwrap(),
            "http://example.invalid/a"
        );
        let wide = "https://example.invalid/\u{e9}.png\0"
            .encode_utf16()
            .flat_map(u16::to_le_bytes)
            .collect::<Vec<_>>();
        assert!(parse_url(&wide, true).is_ok());
        let expanded = format!("https://example.invalid/{}\0", "é".repeat(1400))
            .encode_utf16()
            .flat_map(u16::to_le_bytes)
            .collect::<Vec<_>>();
        assert!(parse_url(&expanded, true).is_err());
        for bytes in [
            b"https://example.invalid/a".as_slice(),
            b"https://u:p@example.invalid/a\0",
            b"file:///C:/a.png\0",
            b"https://example.invalid/a\0https://example.invalid/b\0",
            b"https://example.invalid/a\n\0",
            b"https://example.invalid/\xff\0",
        ] {
            assert!(parse_url(bytes, false).is_err());
        }
        assert!(parse_url(&vec![0; METADATA_BYTES + 1], false).is_err());
        assert!(parse_url(&[0, 0, 1], true).is_err());
    }
    #[test]
    fn dib_wrap_adds_only_header_and_preserves_native_rows_and_alpha() {
        let mut dib = vec![0; 124 + 4];
        for (offset, value) in [
            (0, 124_u32),
            (4, 1),
            (8, 1),
            (16, 3),
            (40, 0x00ff0000),
            (44, 0x0000ff00),
            (48, 0x000000ff),
            (52, 0xff000000),
        ] {
            dib[offset..offset + 4].copy_from_slice(&value.to_le_bytes());
        }
        dib[12..14].copy_from_slice(&1_u16.to_le_bytes());
        dib[14..16].copy_from_slice(&32_u16.to_le_bytes());
        dib[124..].copy_from_slice(&[30, 20, 10, 0]);
        let bmp = dib_as_bmp(&dib).unwrap();
        assert_eq!(&bmp[..2], b"BM");
        assert_eq!(u32::from_le_bytes(bmp[10..14].try_into().unwrap()), 138);
        assert_eq!(&bmp[14..], &dib);
        dib[112..116].copy_from_slice(&1_u32.to_le_bytes());
        assert!(dib_as_bmp(&dib).is_err());
        assert!(dib_as_bmp(&vec![0; 39]).is_err());
        assert!(copy(&vec![0; IMAGE_BYTES], 14).is_err());
    }
}
