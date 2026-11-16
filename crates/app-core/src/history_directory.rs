//! Bounded operation-scoped storage directory creation; no idle scanning.
use super::save::native::Directory;
use anyhow::{Context, Result, ensure};
use std::path::Path;

pub(crate) fn ensure_storage_directory(path: &Path) -> Result<()> {
    let started = std::time::Instant::now();
    let absolute = std::path::absolute(if path.as_os_str().is_empty() {
        Path::new(".")
    } else {
        path
    })?;
    ensure!(
        absolute.as_os_str().as_encoded_bytes().len() <= 4096,
        "Storage directory path exceeds 4096 bytes"
    );
    let mut ancestor = absolute.as_path();
    let mut missing = Vec::<String>::new();
    loop {
        ensure!(
            started.elapsed() < std::time::Duration::from_secs(30),
            "Storage directory preparation exceeded its cooperative deadline"
        );
        match std::fs::metadata(ancestor) {
            Ok(metadata) => {
                ensure!(metadata.is_dir(), "Storage parent is not a directory");
                break;
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                ensure!(
                    missing.len() < 64,
                    "Storage directory creation exceeds 64 missing components"
                );
                let name = ancestor
                    .file_name()
                    .and_then(|n| n.to_str())
                    .context("Missing storage directories need normal UTF-8 path components")?;
                missing.push(name.into());
                ancestor = ancestor
                    .parent()
                    .context("Storage directory has no existing ancestor")?;
            }
            Err(error) => {
                return Err(error).context("Could not inspect storage directory ancestor");
            }
        }
    }
    let canonical = ancestor.canonicalize()?;
    let planned = missing
        .iter()
        .rev()
        .fold(canonical.clone(), |path, name| path.join(name));
    ensure!(
        planned.as_os_str().as_encoded_bytes().len() <= 4096,
        "Resolved storage path exceeds 4096 bytes"
    );
    let mut directory = Directory::open(&canonical, None)?;
    #[cfg(unix)]
    if let Some(parent) = canonical.parent() {
        // A prior attempt can have created this final existing ancestor and then
        // failed its parent barrier. Retry that one entry, not the whole home tree.
        let parent = Directory::open(parent, None)?;
        #[cfg(test)]
        super::save::checkpoint("ancestor_parent_sync")?;
        parent.sync()?;
    }
    for name in missing.iter().rev() {
        ensure!(
            started.elapsed() < std::time::Duration::from_secs(30),
            "Storage directory preparation exceeded its cooperative deadline"
        );
        directory = directory.create_child(name)?;
    }
    directory.verify()?;
    Ok(())
}
