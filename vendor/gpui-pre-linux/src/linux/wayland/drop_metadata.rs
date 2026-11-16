//! Bounded native URI metadata. Never fetches an image or opens referenced files.
use gpui::{ExternalPaths, ExternalUrlDrop};
use http_client::Url;
use std::{path::PathBuf, sync::Arc};
pub(super) enum Metadata {
    Files(ExternalPaths),
    Url(ExternalUrlDrop),
}
pub(super) fn parse(bytes: &[u8]) -> anyhow::Result<Metadata> {
    anyhow::ensure!(bytes.len() <= 64 * 1024, "Drag URI metadata exceeds64 KiB");
    let text = std::str::from_utf8(bytes)?;
    let mut files = smallvec::SmallVec::new();
    let mut urls = Vec::new();
    let mut count = 0usize;
    for line in text.lines() {
        let line = line.trim_end_matches('\r');
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        count += 1;
        anyhow::ensure!(
            count <= 32 && line.len() <= 12 * 1024,
            "Too many or oversized drag references"
        );
        let url = Url::parse(line)?;
        match url.scheme() {
            "file" => {
                let path = url
                    .to_file_path()
                    .map_err(|_| anyhow::anyhow!("Unsupported file URL"))?;
                validate_path(&path)?;
                files.push(path);
            }
            "http" | "https" => {
                anyhow::ensure!(
                    line.len() <= 4096 && url.username().is_empty() && url.password().is_none(),
                    "Invalid image URL drop"
                );
                urls.push(Arc::<str>::from(line));
            }
            _ => anyhow::bail!("Unsupported drag URI scheme"),
        }
    }
    anyhow::ensure!(count > 0, "Empty drag URI list");
    if !files.is_empty() {
        anyhow::ensure!(
            urls.is_empty(),
            "Mixed file and web URL drops are unsupported"
        );
        Ok(Metadata::Files(ExternalPaths(files)))
    } else {
        anyhow::ensure!(urls.len() == 1, "Drop exactly one image URL");
        Ok(Metadata::Url(ExternalUrlDrop {
            url: urls.remove(0),
        }))
    }
}
fn validate_path(path: &PathBuf) -> anyhow::Result<()> {
    anyhow::ensure!(
        path.is_absolute() && path.as_os_str().as_encoded_bytes().len() <= 4096,
        "Invalid or oversized native drop path"
    );
    Ok(())
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn file_metadata_preserves_native_paths_without_opening_them() {
        let Metadata::Files(paths) =
            parse(b"# comment\r\nfile:///definitely-not-existing/%FF.png\r\n").unwrap()
        else {
            panic!()
        };
        assert_eq!(paths.paths().len(), 1);
        #[cfg(unix)]
        {
            use std::os::unix::ffi::OsStrExt;
            assert!(paths.paths()[0].as_os_str().as_bytes().contains(&255));
        }
    }
    #[test]
    fn image_url_and_raw_metadata_have_finite_private_shapes() {
        let Metadata::Url(url) =
            parse(b"https://example.invalid/image.png?private=synthetic\r\n").unwrap()
        else {
            panic!()
        };
        assert!(!format!("{url:?}").contains("synthetic"));
        for bytes in [
            b"file:///tmp/a\nhttps://example.invalid/a".as_slice(),
            b"data:image/png,123",
            b"https://example.invalid/a\nhttps://example.invalid/b",
        ] {
            assert!(parse(bytes).is_err());
        }
        assert!(parse(&vec![b'x'; 64 * 1024 + 1]).is_err());
    }
}
