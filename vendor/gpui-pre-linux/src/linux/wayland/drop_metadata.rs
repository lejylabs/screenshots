//! Shared bounded native URI metadata. Never fetches or opens a reference.
pub(super) use gpui::NativeDropMetadata as Metadata;
pub(super) fn parse(bytes: &[u8]) -> anyhow::Result<Metadata> {
    Metadata::parse(bytes)
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
