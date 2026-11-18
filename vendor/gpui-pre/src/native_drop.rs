//! Process-acquisition seam for native Unix drops. Installation stores a pointer only.
use crate::ImageFormat;
use std::sync::{Arc, OnceLock, atomic::AtomicBool};

/// Metadata captured from the native gesture; no payload acquisition on the event thread.
#[derive(Clone)]
pub enum NativeDropSource {
    /// Xdnd offer on the receiving X11 connection.
    X11 {
        /// The local numeric display of the actual receiving connection.
        display: Arc<str>,
        /// Source window that must still own XdndSelection.
        source: u32,
        /// Receiving window; the worker checks its parent-process PID metadata.
        target: u32,
        /// Inline XdndEnter target atoms.
        types: [u32; 3],
        /// Whether the bounded XdndTypeList property must be consulted instead.
        extended: bool,
        /// Actual XdndPosition/Drop timestamp; CurrentTime is not substituted.
        timestamp: u32,
    },
    /// Actual NSDraggingInfo pasteboard descriptor.
    MacPasteboard {
        /// Bounded native pasteboard name, never the general clipboard.
        name: Arc<str>,
        /// Native ownership change count.
        change_count: i64,
    },
}
/// An original encoded raster or URI metadata, with no implicit decode/fetch.
pub enum NativeDropData {
    /// None denotes a metadata-only image offer, before Drop.
    Image {
        /// Negotiated supported original encoded representation.
        format: ImageFormat,
        /// One bounded body; absent before the final Drop.
        bytes: Option<Arc<Vec<u8>>>,
    },
    /// One bounded native URI list, retaining byte-exact file URLs.
    UriList(Arc<Vec<u8>>),
}
/// Result owned by one operation until foreground delivery or discard.
pub struct NativeDropRead {
    /// Acquired native representation.
    pub data: NativeDropData,
    /// X11 root-to-target coordinate translation, if applicable.
    pub root_offset: Option<[i16; 2]>,
}
/// Executed only on a background executor. The adapter must contain foreign calls.
pub type NativeDropReader = fn(NativeDropSource, bool, &AtomicBool) -> Option<NativeDropRead>;
static READER: OnceLock<NativeDropReader> = OnceLock::new();

/// Installs one reader without starting a process, native call, thread or runtime.
pub fn install_native_drop_reader(reader: NativeDropReader) -> anyhow::Result<()> {
    READER
        .set(reader)
        .map_err(|_| anyhow::anyhow!("Native drop reader already installed"))
}
/// Returns the explicitly installed acquisition adapter, without starting it.
pub fn native_drop_reader() -> Option<NativeDropReader> {
    READER.get().copied()
}

/// Validated URI metadata shared by native Unix receivers; it never opens or fetches a reference.
pub enum NativeDropMetadata {
    /// Bounded native file paths.
    Files(crate::ExternalPaths),
    /// Exactly one credential-free HTTP(S) reference.
    Url(crate::ExternalUrlDrop),
}
impl NativeDropMetadata {
    /// Parses a bounded native URI list all-or-nothing, preserving native path bytes.
    pub fn parse(bytes: &[u8]) -> anyhow::Result<Self> {
        use http_client::Url;
        anyhow::ensure!(
            bytes.len() <= 64 * 1024,
            "Native drag metadata exceeds 64 KiB"
        );
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
                count <= 32 && line.len() <= 12 * 1024 && !line.chars().any(char::is_control),
                "Invalid or oversized native references"
            );
            let url = Url::parse(line)?;
            match url.scheme() {
                "file" => {
                    let path = url
                        .to_file_path()
                        .map_err(|_| anyhow::anyhow!("Unsupported native file URL"))?;
                    anyhow::ensure!(
                        path.is_absolute()
                            && path.as_os_str().as_encoded_bytes().len() <= 4096
                            && !path.as_os_str().as_encoded_bytes().contains(&0),
                        "Invalid native path"
                    );
                    files.push(path);
                }
                "http" | "https" => {
                    anyhow::ensure!(
                        line.len() <= 4096 && url.username().is_empty() && url.password().is_none(),
                        "Invalid native image URL"
                    );
                    urls.push(Arc::<str>::from(line));
                }
                _ => anyhow::bail!("Unsupported native drag URI scheme"),
            }
        }
        anyhow::ensure!(count > 0, "Empty native URI list");
        if !files.is_empty() {
            anyhow::ensure!(
                urls.is_empty(),
                "Mixed native file and web URL offers are unsupported"
            );
            Ok(Self::Files(crate::ExternalPaths(files)))
        } else {
            anyhow::ensure!(urls.len() == 1, "Drop exactly one image URL");
            Ok(Self::Url(crate::ExternalUrlDrop {
                url: urls.remove(0),
            }))
        }
    }
}
