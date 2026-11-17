//! Fixed, private inherited-pipe protocol; payloads have no Debug representation.
use anyhow::{Result, ensure};
use gpui::{ExternalPaths, ImageFormat};
use std::{
    io::{Read, Write},
    os::windows::ffi::{OsStrExt, OsStringExt},
    path::PathBuf,
    sync::Arc,
};

pub(super) const HELLO: u32 = 1;
pub(super) const ACK: u32 = 2;
pub(super) const REGISTER: u32 = 3;
pub(super) const REMOVE: u32 = 4;
pub(super) const RELEASE: u32 = 5;
pub(super) const STOP: u32 = 7;
pub(super) const ENTER: u32 = 10;
pub(super) const MOTION: u32 = 11;
pub(super) const LEAVE: u32 = 12;
pub(super) const SUBMIT: u32 = 13;
pub(super) const FAILED: u32 = 14;
pub(super) const REGISTERED: u32 = 15;
pub(super) const REMOVED: u32 = 16;
pub(super) const FILES: u32 = 1;
pub(super) const PNG: u32 = 2;
pub(super) const JPEG: u32 = 3;
pub(super) const WEBP: u32 = 4;
pub(super) const BMP: u32 = 5;
pub(super) const URL: u32 = 6;
pub(super) const ALLOW: u32 = 17;
const MAGIC: &[u8; 8] = b"SCODR001";
pub(super) const META: usize = 65536 + 4 + 32 * 4;
pub(super) const IMAGE: usize = 32 * 1024 * 1024;

#[derive(Clone, Copy, Default)]
pub(super) struct Header {
    pub kind: u32,
    pub format: u32,
    pub window: u64,
    pub generation: u64,
    pub reader: u64,
    pub version: u64,
    pub x: i32,
    pub y: i32,
}
pub(super) struct Packet {
    pub header: Header,
    pub bytes: Vec<u8>,
}
pub(super) enum Data {
    Files(ExternalPaths),
    Image(ImageFormat, Arc<Vec<u8>>),
    Url(Arc<str>),
    ImageKind(ImageFormat),
}
pub(super) fn image_format(code: u32) -> Result<ImageFormat> {
    Ok(match code {
        PNG => ImageFormat::Png,
        JPEG => ImageFormat::Jpeg,
        WEBP => ImageFormat::Webp,
        BMP => ImageFormat::Bmp,
        _ => anyhow::bail!("Invalid native image kind"),
    })
}
pub(super) fn image_code(format: ImageFormat) -> Result<u32> {
    Ok(match format {
        ImageFormat::Png => PNG,
        ImageFormat::Jpeg => JPEG,
        ImageFormat::Webp => WEBP,
        ImageFormat::Bmp => BMP,
        _ => anyhow::bail!("Unsupported native image kind"),
    })
}
fn length_limit(h: Header) -> Result<usize> {
    Ok(match h.kind {
        HELLO | ACK => 16,
        REGISTER => 8,
        REMOVE | RELEASE | STOP | MOTION | LEAVE | FAILED | REGISTERED | REMOVED | ALLOW | 30
        | 31 | 32 => 0,
        ENTER if matches!(h.format, PNG | JPEG | WEBP | BMP) => 0,
        ENTER | SUBMIT if h.format == FILES => META,
        ENTER | SUBMIT if h.format == URL => 4096,
        SUBMIT if matches!(h.format, PNG | JPEG | WEBP | BMP) => IMAGE,
        _ => anyhow::bail!("Invalid private native drop packet"),
    })
}
pub(super) fn write(output: &mut impl Write, h: Header, bytes: &[u8]) -> Result<()> {
    ensure!(
        bytes.len() <= length_limit(h)?,
        "Native drop packet exceeds its bound"
    );
    let mut raw = [0u8; 64];
    raw[..8].copy_from_slice(MAGIC);
    raw[8..12].copy_from_slice(&h.kind.to_le_bytes());
    raw[12..16].copy_from_slice(&h.format.to_le_bytes());
    for (offset, value) in [
        (16, h.window),
        (24, h.generation),
        (32, h.reader),
        (40, h.version),
    ] {
        raw[offset..offset + 8].copy_from_slice(&value.to_le_bytes());
    }
    raw[48..52].copy_from_slice(&h.x.to_le_bytes());
    raw[52..56].copy_from_slice(&h.y.to_le_bytes());
    raw[56..60].copy_from_slice(&(bytes.len() as u32).to_le_bytes());
    output.write_all(&raw)?;
    for chunk in bytes.chunks(64 * 1024) {
        output.write_all(chunk)?;
    }
    output.flush()?;
    Ok(())
}
pub(super) fn read(input: &mut impl Read) -> Result<Packet> {
    let mut raw = [0u8; 64];
    input.read_exact(&mut raw)?;
    ensure!(
        &raw[..8] == MAGIC && raw[60..] == [0; 4],
        "Invalid private native drop header"
    );
    let u32_at = |n| u32::from_le_bytes(raw[n..n + 4].try_into().unwrap());
    let u64_at = |n| u64::from_le_bytes(raw[n..n + 8].try_into().unwrap());
    let h = Header {
        kind: u32_at(8),
        format: u32_at(12),
        window: u64_at(16),
        generation: u64_at(24),
        reader: u64_at(32),
        version: u64_at(40),
        x: i32::from_le_bytes(raw[48..52].try_into().unwrap()),
        y: i32::from_le_bytes(raw[52..56].try_into().unwrap()),
    };
    let len = u32_at(56) as usize;
    ensure!(
        len <= length_limit(h)?,
        "Native drop packet exceeds its bound"
    );
    let mut bytes = Vec::new();
    bytes.try_reserve_exact(len)?;
    ensure!(
        bytes.capacity() <= length_limit(h)?,
        "Native drop allocation exceeds its bound"
    );
    bytes.resize(len, 0);
    input.read_exact(&mut bytes)?;
    Ok(Packet { header: h, bytes })
}
pub(super) fn handshake(
    input: &mut impl Read,
    output: &mut impl Write,
    nonce: &[u8; 16],
    parent: bool,
) -> Result<()> {
    if parent {
        write(
            output,
            Header {
                kind: HELLO,
                ..Default::default()
            },
            nonce,
        )?;
    }
    let packet = read(input)?;
    ensure!(
        packet.header.kind == if parent { ACK } else { HELLO } && packet.bytes == nonce,
        "Private native drop authentication failed"
    );
    if !parent {
        write(
            output,
            Header {
                kind: ACK,
                ..Default::default()
            },
            nonce,
        )?;
    }
    Ok(())
}
pub(super) fn encode_files(paths: &ExternalPaths) -> Result<Vec<u8>> {
    ensure!(
        !paths.paths().is_empty() && paths.paths().len() <= 32,
        "Invalid native path count"
    );
    let mut out = Vec::new();
    out.extend_from_slice(&(paths.paths().len() as u32).to_le_bytes());
    let mut total = 0usize;
    for path in paths.paths() {
        let units = path.as_os_str().encode_wide().collect::<Vec<_>>();
        ensure!(
            !units.is_empty() && units.len() <= 2048 && !units.contains(&0),
            "Invalid native path"
        );
        total += units.len() * 2;
        ensure!(total <= 65536, "Native paths exceed64 KiB");
        out.extend_from_slice(&(units.len() as u32).to_le_bytes());
        for unit in units {
            out.extend_from_slice(&unit.to_le_bytes());
        }
    }
    ensure!(out.len() <= META, "Native path packet exceeds its bound");
    Ok(out)
}
pub(super) fn decode(mut packet: Packet) -> Result<Data> {
    let h = packet.header;
    if matches!(h.format, PNG | JPEG | WEBP | BMP) {
        let format = image_format(h.format)?;
        return if h.kind == ENTER {
            Ok(Data::ImageKind(format))
        } else {
            ensure!(!packet.bytes.is_empty(), "Empty dropped image");
            Ok(Data::Image(format, Arc::new(packet.bytes)))
        };
    }
    if h.format == URL {
        let text = String::from_utf8(packet.bytes)?;
        ensure!(
            !text.is_empty()
                && text.len() <= 4096
                && !text.chars().any(|c| c.is_control() || c.is_whitespace()),
            "Invalid native URL"
        );
        let url = gpui::http_client::Url::parse(&text)?;
        ensure!(
            matches!(url.scheme(), "http" | "https")
                && url.has_host()
                && url.username().is_empty()
                && url.password().is_none()
                && url.as_str().len() <= 4096,
            "Invalid native URL"
        );
        return Ok(Data::Url(Arc::from(url.as_str())));
    }
    ensure!(
        h.format == FILES && packet.bytes.len() >= 4,
        "Invalid native path packet"
    );
    let mut cursor = 0;
    let take = |cursor: &mut usize, len: usize| -> Result<&[u8]> {
        let end = cursor
            .checked_add(len)
            .ok_or_else(|| anyhow::anyhow!("Native path extent overflow"))?;
        let bytes = packet
            .bytes
            .get(*cursor..end)
            .ok_or_else(|| anyhow::anyhow!("Truncated native paths"))?;
        *cursor = end;
        Ok(bytes)
    };
    let count = u32::from_le_bytes(take(&mut cursor, 4)?.try_into().unwrap()) as usize;
    ensure!((1..=32).contains(&count), "Invalid native path count");
    let mut paths = smallvec::SmallVec::with_capacity(count);
    ensure!(paths.capacity() <= 32, "Native path capacity exceeds32");
    let mut total = 0usize;
    let mut capacity = 0usize;
    for _ in 0..count {
        let len = u32::from_le_bytes(take(&mut cursor, 4)?.try_into().unwrap()) as usize;
        ensure!((1..=2048).contains(&len), "Invalid native path length");
        total += len * 2;
        ensure!(total <= 65536, "Native paths exceed64 KiB");
        let units = take(&mut cursor, len * 2)?
            .as_chunks::<2>()
            .0
            .iter()
            .map(|u| u16::from_le_bytes(*u))
            .collect::<Vec<_>>();
        ensure!(!units.contains(&0), "Invalid native path terminator");
        let path = PathBuf::from(std::ffi::OsString::from_wide(&units));
        let bytes = path.as_os_str().as_encoded_bytes().len();
        ensure!(
            bytes <= 8192,
            "Native path representation exceeds its bound"
        );
        capacity += path.capacity();
        ensure!(
            path.capacity() <= 8192 && capacity <= 196608,
            "Native path capacity exceeds its bound"
        );
        paths.push(path);
    }
    ensure!(cursor == packet.bytes.len(), "Trailing native path bytes");
    packet.bytes.clear();
    Ok(Data::Files(ExternalPaths(paths)))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn wire_refuses_overflow_before_body_and_preserves_native_path_units() {
        let paths = ExternalPaths(smallvec::smallvec![PathBuf::from(
            std::ffi::OsString::from_wide(&[67, 58, 92, 0xd800, 46, 112, 110, 103])
        )]);
        let body = encode_files(&paths).unwrap();
        let Data::Files(copy) = decode(Packet {
            header: Header {
                kind: SUBMIT,
                format: FILES,
                ..Default::default()
            },
            bytes: body,
        })
        .unwrap() else {
            panic!()
        };
        assert_eq!(copy.paths(), paths.paths());
        let mut packet = Vec::new();
        write(
            &mut packet,
            Header {
                kind: SUBMIT,
                format: PNG,
                ..Default::default()
            },
            b"x",
        )
        .unwrap();
        packet[56..60].copy_from_slice(&((IMAGE + 1) as u32).to_le_bytes());
        assert!(read(&mut packet.as_slice()).is_err());
    }
}
