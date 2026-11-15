//! Explicit file management for this executable's user LaunchAgent.

use anyhow::{Context, Result, bail, ensure};
use std::{
    fs::{File, Metadata, OpenOptions},
    io::{Read, Write},
    os::unix::{
        fs::{DirBuilderExt, MetadataExt, OpenOptionsExt},
        io::AsRawFd,
    },
    path::{Path, PathBuf},
};

const LABEL: &str = "me.lejy.screenshots";
const MAX_BYTES: u64 = 16 * 1024;

fn entry_path(home: &Path) -> Result<PathBuf> {
    ensure!(home.is_absolute(), "HOME must be an absolute path");
    Ok(home.join("Library/LaunchAgents/me.lejy.screenshots.plist"))
}

fn contents(executable: &Path) -> Result<Vec<u8>> {
    ensure!(
        executable.is_absolute(),
        "Autostart executable must be an absolute path"
    );
    let path = executable
        .to_str()
        .context("Autostart executable path must be UTF-8")?;
    ensure!(
        !path
            .chars()
            .any(|c| c.is_control() || matches!(c, '\u{fffe}' | '\u{ffff}')),
        "Autostart executable path contains an invalid XML character"
    );
    let mut escaped = String::new();
    for character in path.chars() {
        escaped.push_str(match character {
            '&' => "&amp;",
            '<' => "&lt;",
            '>' => "&gt;",
            '"' => "&quot;",
            '\'' => "&apos;",
            _ => {
                escaped.push(character);
                continue;
            }
        });
    }
    let data = format!(
        "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n<!-- Managed by screenshots autostart v1; edits are never overwritten. -->\n<!DOCTYPE plist PUBLIC \"-//Apple//DTD PLIST 1.0//EN\" \"http://www.apple.com/DTDs/PropertyList-1.0.dtd\">\n<plist version=\"1.0\">\n<dict>\n  <key>Label</key>\n  <string>{LABEL}</string>\n  <key>ProgramArguments</key>\n  <array>\n    <string>{escaped}</string>\n    <string>--show</string>\n  </array>\n  <key>RunAtLoad</key>\n  <true/>\n</dict>\n</plist>\n"
    );
    ensure!(
        data.len() as u64 <= MAX_BYTES,
        "Autostart plist exceeds 16 KiB"
    );
    Ok(data.into_bytes())
}

enum Entry {
    Missing,
    Owned(Metadata),
    Conflict,
}

fn same_file(first: &Metadata, second: &Metadata) -> bool {
    first.dev() == second.dev()
        && first.ino() == second.ino()
        && first.len() == second.len()
        && first.mtime() == second.mtime()
        && first.mtime_nsec() == second.mtime_nsec()
        && first.ctime() == second.ctime()
        && first.ctime_nsec() == second.ctime_nsec()
}

fn inspect(path: &Path, expected: &[u8]) -> Result<Entry> {
    let metadata = match path.symlink_metadata() {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(Entry::Missing),
        Err(error) => return Err(error.into()),
    };
    if !metadata.is_file() || metadata.len() > MAX_BYTES {
        return Ok(Entry::Conflict);
    }
    let file = OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK)
        .open(path)?;
    let opened = file.metadata()?;
    if !opened.is_file() || opened.len() > MAX_BYTES || !same_file(&metadata, &opened) {
        return Ok(Entry::Conflict);
    }
    let mut data = Vec::new();
    file.take(MAX_BYTES + 1).read_to_end(&mut data)?;
    let current = path.symlink_metadata()?;
    if data != expected || !same_file(&opened, &current) {
        return Ok(Entry::Conflict);
    }
    Ok(Entry::Owned(current))
}

fn enable(path: &Path, expected: &[u8]) -> Result<()> {
    let parent = path
        .parent()
        .context("Autostart plist has no parent directory")?;
    std::fs::DirBuilder::new()
        .recursive(true)
        .mode(0o700)
        .create(parent)?;
    let temporary = parent.join(format!(
        ".screenshots-autostart-{}.part",
        uuid::Uuid::new_v4()
    ));
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(&temporary)?;
    let result = (|| -> Result<()> {
        file.write_all(expected)?;
        file.sync_all()?;
        std::fs::hard_link(&temporary, path)
            .context("Autostart plist appeared before publication; existing file preserved")?;
        sync_directory(parent)
            .context("Autostart plist was created, but directory synchronization failed")?;
        Ok(())
    })();
    drop(file);
    let cleanup = std::fs::remove_file(&temporary);
    result?;
    cleanup.context("Autostart plist was created, but temporary file cleanup failed")?;
    Ok(())
}

fn sync_directory(path: &Path) -> std::io::Result<()> {
    let directory = File::open(path)?;
    loop {
        // Use directory fsync; macOS File::sync_all requests the stronger F_FULLFSYNC.
        if unsafe { libc::fsync(directory.as_raw_fd()) } == 0 {
            return Ok(());
        }
        let error = std::io::Error::last_os_error();
        if error.kind() != std::io::ErrorKind::Interrupted {
            return Err(error);
        }
    }
}

fn manage(action: &str, path: &Path, expected: &[u8]) -> Result<&'static str> {
    ensure!(
        matches!(action, "status" | "enable" | "disable"),
        "Invalid autostart action"
    );
    match (action, inspect(path, expected)?) {
        ("status", Entry::Missing) | ("disable", Entry::Missing) => Ok("disabled"),
        ("status" | "enable", Entry::Owned(_)) => Ok("enabled"),
        ("status", Entry::Conflict) => Ok("conflict"),
        ("enable", Entry::Missing) => {
            enable(path, expected)?;
            Ok("enabled")
        }
        ("disable", Entry::Owned(metadata)) => {
            let current = path.symlink_metadata()?;
            ensure!(
                same_file(&metadata, &current) && current.is_file(),
                "Autostart plist changed before removal; preserved"
            );
            std::fs::remove_file(path)?;
            sync_directory(path.parent().unwrap())
                .context("Autostart plist was removed, but directory synchronization failed")?;
            Ok("disabled")
        }
        (_, Entry::Conflict) => {
            bail!("Autostart plist conflicts with this executable; existing file preserved")
        }
        _ => unreachable!("action was validated"),
    }
}

#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
pub fn run(action: &str) -> Result<()> {
    let home = std::env::var_os("HOME").context("HOME is not set")?;
    let path = entry_path(Path::new(&home))?;
    let executable = std::env::current_exe()?.canonicalize()?;
    let expected = contents(&executable)?;
    let status = manage(action, &path, &expected)?;
    let result = serde_json::json!({"scope":"managed_user_entry", "status":status, "path":path.to_string_lossy(), "path_encoding":"lossy-native"});
    let mut stdout = std::io::stdout().lock();
    writeln!(stdout, "{result}")
        .and_then(|_| stdout.flush())
        .context(
            "Autostart action completed, but its result could not be written; inspect status",
        )?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::{
        ffi::{OsStrExt, OsStringExt},
        fs::{PermissionsExt, symlink},
    };

    struct Fixture(PathBuf);
    impl Fixture {
        fn new() -> Self {
            let root = std::env::temp_dir()
                .join(format!("screenshots-launchagent-{}", uuid::Uuid::new_v4()));
            std::fs::create_dir(&root).unwrap();
            Self(root)
        }
        fn path(&self) -> PathBuf {
            entry_path(&self.0).unwrap()
        }
        fn data(&self) -> Vec<u8> {
            contents(Path::new(
                "/Applications/Screenshots.app/Contents/MacOS/screenshots",
            ))
            .unwrap()
        }
    }
    impl Drop for Fixture {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn plist_escapes_xml_but_preserves_unicode_and_literal_arguments() {
        let data = contents(Path::new("/Apps/Grüße & < > \" ' $ ` %/screenshots")).unwrap();
        let text = String::from_utf8(data).unwrap();
        assert!(text.contains(
            "<string>/Apps/Grüße &amp; &lt; &gt; &quot; &apos; $ ` %/screenshots</string>"
        ));
        assert!(text.contains("<key>RunAtLoad</key>\n  <true/>"));
        assert!(text.contains("<string>--show</string>"));
        assert!(!text.contains("KeepAlive"));
        for path in ["relative", "/a\nb", "/a\0b", "/a\u{ffff}b"] {
            assert!(contents(Path::new(path)).is_err());
        }
        let invalid = std::ffi::OsString::from_vec(vec![b'/', 0xff]);
        assert!(contents(Path::new(&invalid)).is_err());
        assert!(contents(Path::new(&format!("/{}", "&".repeat(4096)))).is_err());
        assert!(entry_path(Path::new("relative")).is_err());
    }

    #[test]
    fn lifecycle_is_idempotent_private_and_does_not_create_on_status_or_disable() {
        let fixture = Fixture::new();
        let path = fixture.path();
        let data = fixture.data();
        assert_eq!(manage("status", &path, &data).unwrap(), "disabled");
        assert_eq!(manage("disable", &path, &data).unwrap(), "disabled");
        assert!(std::fs::read_dir(&fixture.0).unwrap().next().is_none());
        assert_eq!(manage("enable", &path, &data).unwrap(), "enabled");
        assert_eq!(std::fs::read(&path).unwrap(), data);
        assert_eq!(
            std::fs::metadata(&path).unwrap().permissions().mode() & 0o777,
            0o600
        );
        assert_eq!(manage("enable", &path, &data).unwrap(), "enabled");
        assert_eq!(manage("status", &path, &data).unwrap(), "enabled");
        assert_eq!(
            std::fs::read_dir(path.parent().unwrap()).unwrap().count(),
            1
        );
        assert_eq!(manage("disable", &path, &data).unwrap(), "disabled");
        assert_eq!(manage("disable", &path, &data).unwrap(), "disabled");
        assert!(!path.exists());
    }

    #[test]
    fn conflicting_files_symlinks_directories_and_fifos_are_never_replaced() {
        for kind in [
            "foreign",
            "edited",
            "other executable",
            "oversized",
            "directory",
            "symlink",
            "fifo",
        ] {
            let fixture = Fixture::new();
            let path = fixture.path();
            let data = fixture.data();
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            match kind {
                "foreign" => std::fs::write(&path, b"foreign plist").unwrap(),
                "edited" => {
                    let mut edited = data.clone();
                    edited.extend_from_slice(b"<!-- edited -->");
                    std::fs::write(&path, edited).unwrap();
                }
                "other executable" => {
                    std::fs::write(&path, contents(Path::new("/Other/screenshots")).unwrap())
                        .unwrap()
                }
                "oversized" => std::fs::write(&path, vec![b'x'; MAX_BYTES as usize + 1]).unwrap(),
                "directory" => std::fs::create_dir(&path).unwrap(),
                "symlink" => {
                    std::fs::write(fixture.0.join("target"), b"preserve target").unwrap();
                    symlink(fixture.0.join("target"), &path).unwrap();
                }
                "fifo" => {
                    let native = std::ffi::CString::new(path.as_os_str().as_bytes()).unwrap();
                    assert_eq!(unsafe { libc::mkfifo(native.as_ptr(), 0o600) }, 0);
                }
                _ => unreachable!(),
            }
            let metadata = path.symlink_metadata().unwrap();
            let bytes = metadata.is_file().then(|| std::fs::read(&path).unwrap());
            assert_eq!(
                manage("status", &path, &data).unwrap(),
                "conflict",
                "{kind}"
            );
            assert!(manage("enable", &path, &data).is_err(), "{kind}");
            assert!(manage("disable", &path, &data).is_err(), "{kind}");
            assert!(same_file(&metadata, &path.symlink_metadata().unwrap()));
            if let Some(bytes) = bytes {
                assert_eq!(std::fs::read(&path).unwrap(), bytes);
            }
            if kind == "symlink" {
                assert_eq!(
                    std::fs::read(fixture.0.join("target")).unwrap(),
                    b"preserve target"
                );
            }
        }
    }
}
