//! Explicit, bounded hashing of one regular file without desktop initialization.

use anyhow::{Context, Result, ensure};
use serde_json::json;
use sha2::{Digest as _, Sha256};
use std::{
    ffi::OsString,
    fs::{Metadata, OpenOptions},
    io::{Read as _, Write as _},
    path::Path,
    time::{Duration, Instant},
};

const MAX_FILE_BYTES: u64 = 1024 * 1024 * 1024;
const READ_BUFFER_BYTES: usize = 64 * 1024;
const DEADLINE: Duration = Duration::from_secs(30);
const USAGE: &str = "Usage: screenshots hash-file FILE [--expect HEX]";

pub fn run(args: &[OsString]) -> Result<()> {
    ensure!(args.len() == 1 || args.len() == 3, USAGE);
    let expected = if args.len() == 3 {
        ensure!(args[1] == "--expect", USAGE);
        let value = args[2]
            .to_str()
            .context("Expected SHA-256 must be exactly 64 ASCII hexadecimal digits")?;
        ensure!(
            value.len() == 64 && value.bytes().all(|byte| byte.is_ascii_hexdigit()),
            "Expected SHA-256 must be exactly 64 ASCII hexadecimal digits"
        );
        Some(value)
    } else {
        None
    };

    let started = Instant::now();
    let path = Path::new(&args[0]);
    let preflight = path.metadata().context("Could not inspect hash input")?;
    validate_file(&preflight)?;
    check_deadline(started)?;
    let mut options = OpenOptions::new();
    options.read(true);
    // Avoid blocking on a FIFO substituted between preflight and open on Unix.
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt as _;
        options.custom_flags(libc::O_NONBLOCK);
    }
    let file = options.open(path).context("Could not open hash input")?;
    let metadata = file
        .metadata()
        .context("Could not inspect opened hash input")?;
    validate_file(&metadata)?;
    let bytes = metadata.len();
    let digest = hash_bytes(&file, bytes, started)?;
    ensure!(
        file.metadata()
            .context("Could not recheck hash input")?
            .len()
            == bytes,
        "Hash input length changed while reading"
    );
    check_deadline(started)?;

    let matches = expected.map(|expected| expected.eq_ignore_ascii_case(&digest));
    let mut result = json!({"algorithm":"sha256", "digest":digest, "bytes":bytes});
    if let Some(matches) = matches {
        result["matches"] = json!(matches);
    }
    let mut output = std::io::stdout().lock();
    writeln!(output, "{result}").context("Could not write file hash")?;
    output.flush().context("Could not flush file hash")?;
    ensure!(
        matches != Some(false),
        "SHA-256 digest does not match expected value"
    );
    Ok(())
}

fn validate_file(metadata: &Metadata) -> Result<()> {
    ensure!(metadata.is_file(), "Hash input must be a regular file");
    ensure!(
        metadata.len() <= MAX_FILE_BYTES,
        "Hash input exceeds the 1 GiB limit"
    );
    Ok(())
}

fn check_deadline(started: Instant) -> Result<()> {
    ensure!(
        started.elapsed() < DEADLINE,
        "File hashing exceeded the 30-second deadline"
    );
    Ok(())
}

fn hash_bytes(reader: impl std::io::Read, bytes: u64, started: Instant) -> Result<String> {
    use std::fmt::Write as _;

    let mut source = reader.take(bytes);
    let mut buffer = [0u8; READ_BUFFER_BYTES];
    let mut hasher = Sha256::new();
    let mut read_bytes = 0;
    while read_bytes < bytes {
        check_deadline(started)?;
        let read = match source.read(&mut buffer) {
            Err(error) if error.kind() == std::io::ErrorKind::Interrupted => continue,
            result => result.context("Could not read hash input")?,
        };
        check_deadline(started)?;
        ensure!(read > 0, "Hash input ended before its reported length");
        hasher.update(&buffer[..read]);
        read_bytes += read as u64;
    }
    let mut digest = String::with_capacity(64);
    for byte in hasher.finalize() {
        write!(&mut digest, "{byte:02x}")?;
    }
    Ok(digest)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reading_is_limited_to_the_admitted_extent_and_rejects_short_input() {
        let digest = hash_bytes(&b"abcignored"[..], 3, Instant::now()).unwrap();
        assert_eq!(
            digest,
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
        assert!(hash_bytes(&b"ab"[..], 3, Instant::now()).is_err());
    }

    #[test]
    fn expired_deadline_fails_before_reading() {
        struct Unreadable;
        impl std::io::Read for Unreadable {
            fn read(&mut self, _buffer: &mut [u8]) -> std::io::Result<usize> {
                panic!("An expired operation must not read");
            }
        }
        assert!(hash_bytes(Unreadable, 1, Instant::now() - DEADLINE).is_err());
    }
}
