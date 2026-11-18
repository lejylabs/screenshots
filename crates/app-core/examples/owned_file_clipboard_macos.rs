//! Native-main-thread fixture: a uniquely named private board, never the user's clipboard.
#[cfg(target_os = "macos")]
#[path = "../../../apps/desktop/src/file_clipboard_macos.rs"]
mod publisher;
#[cfg(target_os = "macos")]
fn main() -> anyhow::Result<()> {
    use std::os::unix::{ffi::OsStrExt, fs::DirBuilderExt};
    let root = std::env::temp_dir().join(format!("owned-file-board-{}", uuid::Uuid::new_v4()));
    std::fs::DirBuilder::new().mode(0o700).create(&root)?;
    let source = root.join("Grüße #?% file.bin");
    std::fs::write(&source, b"private synthetic reference; no image decoding")?;
    let name = format!("screenshots.owned-file-fixture.{}", uuid::Uuid::new_v4());
    let result = (|| -> anyhow::Result<()> {
        let uri = app_core::prepare_file_clipboard_uri(&source)?;
        publisher::publish(&uri, &name)?;
        let app_core::ClipboardSnapshot::Files { files } =
            app_core::inspect_owned_macos_file_pasteboard(&name)?
        else {
            anyhow::bail!("Owned native file reader returned another representation");
        };
        anyhow::ensure!(
            files.len() == 1
                && files[0].native == std::fs::canonicalize(&source)?.as_os_str().as_bytes(),
            "Named native file URL did not round-trip"
        );
        anyhow::ensure!(
            std::fs::read(&source)? == b"private synthetic reference; no image decoding",
            "File clipboard modified its source"
        );
        println!(
            "{{\"owned_named_pasteboard\":true,\"file_reference_round_trip\":true,\"source_unchanged\":true}}"
        );
        Ok(())
    })();
    std::fs::remove_dir_all(&root)?;
    result
}
#[cfg(not(target_os = "macos"))]
fn main() {
    eprintln!("This fixture requires native macOS; it never uses another clipboard.");
}
