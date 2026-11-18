//! Bounded readers and deterministic writers for the portable `.tar.gz` and `.zip`
//! archives. Only regular files are supported; links, devices and directories in
//! incoming archives are refused.
use anyhow::{Context, Result, bail, ensure};
use std::io::{self, Read, Seek, SeekFrom, Write};

/// One regular member presented to a reader callback.
pub struct Member<'a> {
    pub name: String,
    pub size: u64,
    pub mode: u32,
    pub reader: &'a mut dyn Read,
}

fn octal(field: &[u8]) -> Result<u64> {
    if field.first().is_some_and(|b| b & 0x80 != 0) {
        // GNU base-256 numeric extension.
        let mut value: u64 = u64::from(field[0] & 0x7f);
        for byte in &field[1..] {
            value = value.checked_mul(256).and_then(|v| v.checked_add(u64::from(*byte))).context("Tar number overflow")?;
        }
        return Ok(value);
    }
    let text = field.iter().take_while(|b| **b != 0).copied().collect::<Vec<_>>();
    let text = std::str::from_utf8(&text).context("Invalid tar number")?.trim();
    if text.is_empty() {
        return Ok(0);
    }
    u64::from_str_radix(text, 8).context("Invalid tar number")
}
fn field_text(field: &[u8]) -> Result<String> {
    let end = field.iter().position(|b| *b == 0).unwrap_or(field.len());
    String::from_utf8(field[..end].to_vec()).context("Tar names must be UTF-8")
}
fn read_full(reader: &mut dyn Read, buffer: &mut [u8]) -> Result<bool> {
    let mut filled = 0;
    while filled < buffer.len() {
        let read = reader.read(&mut buffer[filled..])?;
        if read == 0 {
            ensure!(filled == 0, "Truncated tar header");
            return Ok(false);
        }
        filled += read;
    }
    Ok(true)
}
fn skip(reader: &mut dyn Read, mut count: u64) -> Result<()> {
    let mut buffer = [0u8; 65536];
    while count > 0 {
        let want = buffer.len().min(count as usize);
        let read = reader.read(&mut buffer[..want])?;
        ensure!(read > 0, "Truncated tar member");
        count -= read as u64;
    }
    Ok(())
}
fn bounded_bytes(reader: &mut dyn Read, size: u64) -> Result<Vec<u8>> {
    let mut data = vec![0; size as usize];
    reader.read_exact(&mut data).context("Truncated tar metadata")?;
    skip(reader, padding(size))?;
    Ok(data)
}
fn padding(size: u64) -> u64 {
    (512 - size % 512) % 512
}

pub struct TarLimits {
    pub entries: usize,
    pub member: u64,
    pub total: u64,
}

/// Stream a gzip-compressed tar, visiting each regular member in order. Pax and GNU
/// long-name metadata are honoured within fixed bounds; anything else is refused.
pub fn read_tar_gz(
    source: impl Read,
    limits: &TarLimits,
    visit: &mut dyn FnMut(Member<'_>) -> Result<()>,
) -> Result<()> {
    let mut stream = flate2::read::GzDecoder::new(source);
    let reader: &mut dyn Read = &mut stream;
    let mut entries = 0usize;
    let mut total = 0u64;
    let mut long_name: Option<String> = None;
    let mut pax_path: Option<String> = None;
    let mut pax_size: Option<u64> = None;
    let mut header = [0u8; 512];
    loop {
        ensure!(read_full(reader, &mut header)?, "Truncated tar header");
        if header.iter().all(|b| *b == 0) {
            return Ok(());
        }
        let stored = octal(&header[148..156])?;
        let computed: u64 = header.iter().enumerate().map(|(i, b)| if (148..156).contains(&i) { 32 } else { u64::from(*b) }).sum();
        ensure!(stored == computed, "Tar header checksum mismatch");
        entries += 1;
        ensure!(entries <= limits.entries * 2, "Excessive tar metadata");
        let kind = header[156];
        let mut size = octal(&header[124..136])?;
        total = total.saturating_add(512 + size.div_ceil(512) * 512);
        ensure!(total <= limits.total, "Tar exceeds its expanded-byte budget");
        match kind {
            b'x' => {
                ensure!(size <= 65536, "Tar extended metadata exceeds 64 KiB");
                let data = bounded_bytes(reader, size)?;
                let (path, length) = pax(&data)?;
                pax_path = path.or(pax_path);
                pax_size = length.or(pax_size);
            }
            b'g' => {
                ensure!(size <= 65536, "Tar extended metadata exceeds 64 KiB");
                bounded_bytes(reader, size)?;
            }
            b'L' => {
                ensure!(size <= 4096, "Tar long-name metadata exceeds 4096 bytes");
                let data = bounded_bytes(reader, size)?;
                long_name = Some(field_text(&data)?);
            }
            b'0' | 0 => {
                if let Some(length) = pax_size.take() {
                    size = length;
                }
                ensure!(size <= limits.member, "Tar contains an excessive member");
                let mut name = field_text(&header[0..100])?;
                if &header[257..262] == b"ustar" {
                    let prefix = field_text(&header[345..500])?;
                    if !prefix.is_empty() {
                        name = format!("{prefix}/{name}");
                    }
                }
                if let Some(long) = long_name.take() {
                    name = long;
                }
                if let Some(path) = pax_path.take() {
                    name = path;
                }
                ensure!(!name.ends_with('/'), "Package contains a nonregular entry");
                let mode = octal(&header[100..108])? as u32;
                let mut limited = (&mut *reader).take(size);
                visit(Member { name, size, mode, reader: &mut limited })?;
                // Drain anything the visitor left so the stream stays aligned.
                io::copy(&mut limited, &mut io::sink())?;
                skip(reader, padding(size))?;
            }
            _ => bail!("Package contains a nonregular entry"),
        }
    }
}
fn pax(data: &[u8]) -> Result<(Option<String>, Option<u64>)> {
    let mut rest = data;
    let (mut path, mut size) = (None, None);
    while !rest.is_empty() {
        let space = rest.iter().position(|b| *b == b' ').context("Invalid pax record")?;
        let length: usize = std::str::from_utf8(&rest[..space])?.parse().context("Invalid pax record")?;
        ensure!(length > space + 1 && length <= rest.len() && rest[length - 1] == b'\n', "Invalid pax record");
        let record = std::str::from_utf8(&rest[space + 1..length - 1]).context("Invalid pax record")?;
        let (key, value) = record.split_once('=').context("Invalid pax record")?;
        match key {
            "path" => path = Some(value.to_owned()),
            "size" => size = Some(value.parse().context("Invalid pax size")?),
            "linkpath" => bail!("Package contains a nonregular entry"),
            _ => {}
        }
        rest = &rest[length..];
    }
    Ok((path, size))
}

pub struct ZipLimits {
    pub entries: usize,
    pub member: u64,
}

/// Read a single-volume, non-Zip64 archive's central directory and visit its
/// regular members. Stored and deflated members are supported; CRCs are verified.
pub fn read_zip(
    mut file: impl Read + Seek,
    limits: &ZipLimits,
    visit: &mut dyn FnMut(Member<'_>) -> Result<()>,
) -> Result<()> {
    let length = file.seek(SeekFrom::End(0))?;
    let start = length.saturating_sub(65557);
    file.seek(SeekFrom::Start(start))?;
    let mut tail = Vec::new();
    file.read_to_end(&mut tail)?;
    let position = tail.windows(4).rposition(|w| w == b"PK\x05\x06").context("ZIP end directory is missing")?;
    ensure!(position + 22 <= tail.len(), "ZIP end directory is missing");
    let le16 = |b: &[u8], at: usize| u16::from_le_bytes([b[at], b[at + 1]]);
    let le32 = |b: &[u8], at: usize| u32::from_le_bytes([b[at], b[at + 1], b[at + 2], b[at + 3]]);
    let eocd = &tail[position..];
    let (disk, directory_disk, disk_count, count) = (le16(eocd, 4), le16(eocd, 6), le16(eocd, 8), le16(eocd, 10));
    let (size, offset, comment) = (le32(eocd, 12), le32(eocd, 16), le16(eocd, 20));
    ensure!(
        position + 22 + comment as usize == tail.len()
            && disk == 0
            && directory_disk == 0
            && disk_count == count
            && count > 0
            && (count as usize) <= limits.entries
            && size <= 16 * 1024 * 1024
            && u64::from(offset) + u64::from(size) <= length
            && count != 0xffff
            && size != 0xffff_ffff
            && offset != 0xffff_ffff,
        "ZIP directory exceeds its bounded single-volume contract"
    );
    file.seek(SeekFrom::Start(u64::from(offset)))?;
    let mut directory = vec![0; size as usize];
    file.read_exact(&mut directory)?;
    struct Entry {
        name: String,
        method: u16,
        crc: u32,
        compressed: u64,
        size: u64,
        mode: u32,
        local: u64,
    }
    let mut entries = Vec::with_capacity(count as usize);
    let mut at = 0usize;
    for _ in 0..count {
        ensure!(at + 46 <= directory.len() && le32(&directory, at) == 0x0201_4b50, "Invalid ZIP central directory");
        let d = &directory[at..];
        let flags = le16(d, 8);
        let method = le16(d, 10);
        let (name_len, extra_len, comment_len) = (le16(d, 28) as usize, le16(d, 30) as usize, le16(d, 32) as usize);
        ensure!(at + 46 + name_len + extra_len + comment_len <= directory.len(), "Invalid ZIP central directory");
        let raw = &d[46..46 + name_len];
        ensure!(flags & 0x0001 == 0, "Encrypted ZIP members are not supported");
        ensure!(flags & 0x0800 != 0 || raw.is_ascii(), "ZIP names must be UTF-8");
        let name = String::from_utf8(raw.to_vec()).context("ZIP names must be UTF-8")?;
        let (compressed, size) = (u64::from(le32(d, 20)), u64::from(le32(d, 24)));
        ensure!(compressed != 0xffff_ffff && size != 0xffff_ffff, "ZIP64 members are not supported");
        entries.push(Entry {
            name,
            method,
            crc: le32(d, 16),
            compressed,
            size,
            mode: le32(d, 38) >> 16,
            local: u64::from(le32(d, 42)),
        });
        at += 46 + name_len + extra_len + comment_len;
    }
    for entry in entries {
        let format = entry.mode & 0o170000;
        ensure!(!entry.name.ends_with('/') && (format == 0 || format == 0o100000), "Package contains a nonregular entry");
        ensure!(entry.size <= limits.member, "Duplicate or excessive archive entry");
        file.seek(SeekFrom::Start(entry.local))?;
        let mut local = [0u8; 30];
        file.read_exact(&mut local)?;
        ensure!(le32(&local, 0) == 0x0403_4b50, "Invalid ZIP local header");
        let skip_bytes = u64::from(le16(&local, 26)) + u64::from(le16(&local, 28));
        file.seek(SeekFrom::Current(skip_bytes as i64))?;
        let raw = (&mut file).take(entry.compressed);
        let mut checked = Crc { inner: Box::new(match entry.method {
            0 => Box::new(raw) as Box<dyn Read + '_>,
            8 => Box::new(flate2::read::DeflateDecoder::new(raw)),
            _ => bail!("Unsupported ZIP compression method"),
        }), hasher: crc32fast::Hasher::new(), remaining: entry.size, expected: entry.crc };
        visit(Member { name: entry.name, size: entry.size, mode: entry.mode, reader: &mut checked })?;
        io::copy(&mut checked, &mut io::sink())?;
        checked.finish()?;
    }
    Ok(())
}
struct Crc<'a> {
    inner: Box<dyn Read + 'a>,
    hasher: crc32fast::Hasher,
    remaining: u64,
    expected: u32,
}
impl Read for Crc<'_> {
    fn read(&mut self, buffer: &mut [u8]) -> io::Result<usize> {
        let read = self.inner.read(buffer)?;
        if read as u64 > self.remaining {
            return Err(io::Error::other("ZIP member exceeded its declared size"));
        }
        self.remaining -= read as u64;
        self.hasher.update(&buffer[..read]);
        Ok(read)
    }
}
impl Crc<'_> {
    fn finish(self) -> Result<()> {
        ensure!(self.remaining == 0, "Truncated archive member");
        ensure!(self.hasher.finalize() == self.expected, "ZIP member CRC mismatch");
        Ok(())
    }
}

// ---- Writers ----

/// Deterministic POSIX tar writer (ustar, with pax records for long names).
pub struct TarWriter<W: Write> {
    out: W,
}
impl<W: Write> TarWriter<W> {
    pub fn new(out: W) -> Self {
        Self { out }
    }
    fn header(&mut self, name: &str, size: u64, mode: u32, mtime: u64, kind: u8, link: &str) -> Result<()> {
        let mut block = [0u8; 512];
        let (prefix, short) = split_ustar(name);
        let mut pairs = Vec::new();
        if name.len() > 100 && prefix.is_none() {
            pairs.push(("path", name));
        }
        if link.len() > 100 {
            pairs.push(("linkpath", link));
        }
        if !pairs.is_empty() {
            let mut records = Vec::new();
            for (key, value) in pairs {
                // A record's length prefix counts its own digits.
                let body = format!(" {key}={value}\n");
                let mut length = body.len() + 1;
                while format!("{length}{body}").len() != length {
                    length = format!("{length}{body}").len();
                }
                records.extend_from_slice(format!("{length}{body}").as_bytes());
            }
            self.header("././@PaxHeader", records.len() as u64, 0o644, mtime, b'x', "")?;
            self.out.write_all(&records)?;
            self.out.write_all(&vec![0; padding(records.len() as u64) as usize])?;
        }
        let stored = &short.as_bytes()[..short.len().min(100)];
        block[..stored.len()].copy_from_slice(stored);
        if let Some(prefix) = prefix {
            block[345..345 + prefix.len()].copy_from_slice(prefix.as_bytes());
        }
        let number = |block: &mut [u8], range: std::ops::Range<usize>, value: u64| {
            let width = range.len() - 1;
            let text = format!("{value:0width$o}");
            block[range.start..range.start + width].copy_from_slice(&text.as_bytes()[text.len() - width..]);
        };
        number(&mut block, 100..108, u64::from(mode));
        number(&mut block, 108..116, 0);
        number(&mut block, 116..124, 0);
        number(&mut block, 124..136, size);
        number(&mut block, 136..148, mtime);
        block[156] = kind;
        let link = &link.as_bytes()[..link.len().min(100)];
        block[157..157 + link.len()].copy_from_slice(link);
        block[257..263].copy_from_slice(b"ustar\0");
        block[263..265].copy_from_slice(b"00");
        block[148..156].fill(b' ');
        let sum: u64 = block.iter().map(|b| u64::from(*b)).sum();
        let text = format!("{sum:06o}\0 ");
        block[148..156].copy_from_slice(text.as_bytes());
        self.out.write_all(&block)?;
        Ok(())
    }
    pub fn file(&mut self, name: &str, mode: u32, mtime: u64, source: &mut dyn Read, size: u64) -> Result<()> {
        self.header(name, size, mode, mtime, b'0', "")?;
        let copied = io::copy(&mut source.take(size), &mut self.out)?;
        ensure!(copied == size, "Archive input was truncated");
        self.out.write_all(&vec![0; padding(size) as usize])?;
        Ok(())
    }
    pub fn symlink(&mut self, name: &str, target: &str, mtime: u64) -> Result<()> {
        self.header(name, 0, 0o777, mtime, b'2', target)
    }
    pub fn finish(mut self) -> Result<W> {
        self.out.write_all(&[0u8; 1024])?;
        Ok(self.out)
    }
}
fn split_ustar(name: &str) -> (Option<&str>, &str) {
    if name.len() <= 100 {
        return (None, name);
    }
    for (index, _) in name.match_indices('/') {
        if index <= 155 && name.len() - index - 1 <= 100 && index > 0 {
            return (Some(&name[..index]), &name[index + 1..]);
        }
    }
    (None, name)
}

/// Write `.tar.gz` with a fixed gzip header (no name, given mtime).
pub fn gzip_writer<W: Write>(out: W, mtime: u32) -> flate2::write::GzEncoder<W> {
    flate2::GzBuilder::new().mtime(mtime).write(out, flate2::Compression::best())
}

/// Deterministic deflate ZIP writer; sizes are patched after each member.
pub struct ZipWriter<W: Write + Seek> {
    out: W,
    central: Vec<u8>,
    count: u16,
}
impl<W: Write + Seek> ZipWriter<W> {
    pub fn new(out: W) -> Self {
        Self { out, central: Vec::new(), count: 0 }
    }
    /// `date` is (year, month, day, hour, minute, second), clamped to 1980.
    pub fn file(&mut self, name: &str, mode: u32, date: (u32, u32, u32, u32, u32, u32), source: &mut dyn Read) -> Result<()> {
        ensure!(self.count < 0xffff, "Too many ZIP members");
        let (year, month, day, hour, minute, second) = date;
        let dos_time = ((hour << 11) | (minute << 5) | (second / 2)) as u16;
        let dos_date = (((year.max(1980) - 1980) << 9) | (month << 5) | day) as u16;
        let flags: u16 = if name.is_ascii() { 0 } else { 0x0800 };
        let offset = self.out.stream_position()?;
        ensure!(offset < 0xffff_ffff, "ZIP exceeds 4 GiB");
        let mut local = Vec::with_capacity(30 + name.len());
        local.extend_from_slice(&0x0403_4b50u32.to_le_bytes());
        local.extend_from_slice(&20u16.to_le_bytes());
        local.extend_from_slice(&flags.to_le_bytes());
        local.extend_from_slice(&8u16.to_le_bytes());
        local.extend_from_slice(&dos_time.to_le_bytes());
        local.extend_from_slice(&dos_date.to_le_bytes());
        local.extend_from_slice(&[0; 12]);
        local.extend_from_slice(&(name.len() as u16).to_le_bytes());
        local.extend_from_slice(&0u16.to_le_bytes());
        local.extend_from_slice(name.as_bytes());
        self.out.write_all(&local)?;
        let data_start = self.out.stream_position()?;
        let mut hasher = crc32fast::Hasher::new();
        let mut size = 0u64;
        {
            let mut encoder = flate2::write::DeflateEncoder::new(&mut self.out, flate2::Compression::default());
            let mut buffer = [0u8; 65536];
            loop {
                let read = source.read(&mut buffer)?;
                if read == 0 {
                    break;
                }
                hasher.update(&buffer[..read]);
                size += read as u64;
                encoder.write_all(&buffer[..read])?;
            }
            encoder.finish()?;
        }
        let end = self.out.stream_position()?;
        let compressed = end - data_start;
        ensure!(size < 0xffff_ffff && compressed < 0xffff_ffff, "ZIP64 members are not supported");
        let crc = hasher.finalize();
        self.out.seek(SeekFrom::Start(offset + 14))?;
        self.out.write_all(&crc.to_le_bytes())?;
        self.out.write_all(&(compressed as u32).to_le_bytes())?;
        self.out.write_all(&(size as u32).to_le_bytes())?;
        self.out.seek(SeekFrom::Start(end))?;
        let c = &mut self.central;
        c.extend_from_slice(&0x0201_4b50u32.to_le_bytes());
        c.extend_from_slice(&(0x0300u16 | 20).to_le_bytes());
        c.extend_from_slice(&20u16.to_le_bytes());
        c.extend_from_slice(&flags.to_le_bytes());
        c.extend_from_slice(&8u16.to_le_bytes());
        c.extend_from_slice(&dos_time.to_le_bytes());
        c.extend_from_slice(&dos_date.to_le_bytes());
        c.extend_from_slice(&crc.to_le_bytes());
        c.extend_from_slice(&(compressed as u32).to_le_bytes());
        c.extend_from_slice(&(size as u32).to_le_bytes());
        c.extend_from_slice(&(name.len() as u16).to_le_bytes());
        c.extend_from_slice(&[0; 8]);
        c.extend_from_slice(&((0o100000 | mode) << 16).to_le_bytes());
        c.extend_from_slice(&(offset as u32).to_le_bytes());
        c.extend_from_slice(name.as_bytes());
        self.count += 1;
        Ok(())
    }
    pub fn finish(mut self) -> Result<W> {
        let offset = self.out.stream_position()?;
        self.out.write_all(&self.central)?;
        let mut end = Vec::with_capacity(22);
        end.extend_from_slice(&0x0605_4b50u32.to_le_bytes());
        end.extend_from_slice(&[0; 4]);
        end.extend_from_slice(&self.count.to_le_bytes());
        end.extend_from_slice(&self.count.to_le_bytes());
        end.extend_from_slice(&(self.central.len() as u32).to_le_bytes());
        end.extend_from_slice(&(offset as u32).to_le_bytes());
        end.extend_from_slice(&0u16.to_le_bytes());
        self.out.write_all(&end)?;
        Ok(self.out)
    }
}

/// Civil UTC date for a Unix timestamp (for ZIP DOS timestamps).
pub fn utc_civil(epoch: i64) -> (u32, u32, u32, u32, u32, u32) {
    let days = epoch.div_euclid(86400);
    let seconds = epoch.rem_euclid(86400);
    // Howard Hinnant's days-from-civil inverse.
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = if m <= 2 { y + 1 } else { y };
    (y as u32, m as u32, d as u32, (seconds / 3600) as u32, (seconds % 3600 / 60) as u32, (seconds % 60) as u32)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn tar_and_zip_round_trip_long_names_and_modes() {
        let long = format!("root/{}/file.txt", "d".repeat(120));
        let mut tar = Vec::new();
        {
            let mut writer = TarWriter::new(gzip_writer(&mut tar, 0));
            writer.file("root/screenshots", 0o755, 0, &mut &b"binary"[..], 6).unwrap();
            writer.file(&long, 0o644, 0, &mut &b"text"[..], 4).unwrap();
            writer.finish().unwrap().finish().unwrap();
        }
        let mut seen = Vec::new();
        let limits = TarLimits { entries: 10, member: 1024, total: 1 << 20 };
        read_tar_gz(&tar[..], &limits, &mut |m| {
            let mut body = String::new();
            m.reader.read_to_string(&mut body).unwrap();
            seen.push((m.name, m.mode & 0o777, body));
            Ok(())
        })
        .unwrap();
        assert_eq!(seen, vec![("root/screenshots".into(), 0o755, "binary".into()), (long.clone(), 0o644, "text".into())]);

        let mut zip = io::Cursor::new(Vec::new());
        let mut writer = ZipWriter::new(&mut zip);
        writer.file("root/screenshots.exe", 0o755, utc_civil(0), &mut &b"binary"[..]).unwrap();
        writer.file(&long, 0o644, utc_civil(1_700_000_000), &mut &b"text"[..]).unwrap();
        writer.finish().unwrap();
        let mut seen = Vec::new();
        read_zip(io::Cursor::new(zip.into_inner()), &ZipLimits { entries: 10, member: 1024 }, &mut |m| {
            let mut body = String::new();
            m.reader.read_to_string(&mut body).unwrap();
            seen.push((m.name, m.mode & 0o777, body));
            Ok(())
        })
        .unwrap();
        assert_eq!(seen, vec![("root/screenshots.exe".into(), 0o755, "binary".into()), (long, 0o644, "text".into())]);
        assert_eq!(utc_civil(1_700_000_000), (2023, 11, 14, 22, 13, 20));
    }
}
