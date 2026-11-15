//! Explicit management of this executable's Linux user autostart entry.

use anyhow::{Result, ensure};
use std::ffi::OsString;

pub fn run(args: &[OsString]) -> Result<()> {
    ensure!(
        args.len() == 1 && matches!(args[0].to_str(), Some("status" | "enable" | "disable")),
        "Usage: screenshots autostart status|enable|disable"
    );
    #[cfg(target_os = "linux")]
    return linux::run(args[0].to_str().unwrap());
    #[cfg(target_os = "windows")]
    return crate::autostart_windows::run(args[0].to_str().unwrap());
    #[cfg(target_os = "macos")]
    return crate::autostart_macos::run(args[0].to_str().unwrap());
    #[cfg(not(any(target_os = "linux", target_os = "windows", target_os = "macos")))]
    anyhow::bail!("Autostart management is not supported on this platform")
}

#[cfg(target_os = "linux")]
mod linux {
    use super::*;
    use anyhow::{Context, bail};
    use std::{
        fs::{File, Metadata, OpenOptions},
        io::{Read, Write},
        os::unix::fs::{DirBuilderExt, MetadataExt, OpenOptionsExt},
        path::{Path, PathBuf},
    };

    const NAME: &str = "me.lejy.screenshots.desktop";
    const MAX_ENTRY_BYTES: u64 = 16 * 1024;

    fn entry_path() -> Result<PathBuf> {
        let base = if let Some(base) = std::env::var_os("XDG_CONFIG_HOME").filter(|v| !v.is_empty())
        {
            let base = PathBuf::from(base);
            ensure!(
                base.is_absolute(),
                "XDG_CONFIG_HOME must be an absolute path"
            );
            base
        } else {
            let home = PathBuf::from(std::env::var_os("HOME").context("HOME is not set")?);
            ensure!(home.is_absolute(), "HOME must be an absolute path");
            home.join(".config")
        };
        Ok(base.join("autostart").join(NAME))
    }

    fn exec_argument(executable: &Path) -> Result<String> {
        ensure!(
            executable.is_absolute(),
            "Autostart executable must be an absolute path"
        );
        let value = executable
            .to_str()
            .context("Autostart executable path must be UTF-8")?;
        ensure!(
            !value.contains('=') && !value.chars().any(char::is_control),
            "Autostart executable path must not contain '=' or control characters"
        );
        let mut quoted = String::from("\"");
        for character in value.chars() {
            match character {
                '\\' => quoted.push_str("\\\\\\\\"),
                '"' => quoted.push_str("\\\\\""),
                '`' => quoted.push_str("\\\\`"),
                '$' => quoted.push_str("\\\\$"),
                '%' => quoted.push_str("%%"),
                other => quoted.push(other),
            }
        }
        quoted.push('"');
        Ok(quoted)
    }

    fn contents(executable: &Path) -> Result<Vec<u8>> {
        let executable = exec_argument(executable)?;
        let text = format!(
            "# Managed by screenshots autostart v1; edits are never overwritten.\n[Desktop Entry]\nType=Application\nVersion=1.0\nName=Screenshots\nComment=Open Screenshots after login\nExec={executable} --show\nTerminal=false\n"
        );
        ensure!(
            text.len() as u64 <= MAX_ENTRY_BYTES,
            "Autostart entry exceeds 16 KiB"
        );
        Ok(text.into_bytes())
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
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                return Ok(Entry::Missing);
            }
            Err(error) => return Err(error.into()),
        };
        if !metadata.is_file() || metadata.len() > MAX_ENTRY_BYTES {
            return Ok(Entry::Conflict);
        }
        let file = OpenOptions::new()
            .read(true)
            .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK)
            .open(path)?;
        let opened = file.metadata()?;
        if !opened.is_file() || opened.len() > MAX_ENTRY_BYTES || !same_file(&metadata, &opened) {
            return Ok(Entry::Conflict);
        }
        let mut bytes = Vec::new();
        file.take(MAX_ENTRY_BYTES + 1).read_to_end(&mut bytes)?;
        let current = path.symlink_metadata()?;
        if bytes != expected || !same_file(&opened, &current) {
            return Ok(Entry::Conflict);
        }
        Ok(Entry::Owned(current))
    }

    fn enable(path: &Path, expected: &[u8]) -> Result<()> {
        let parent = path
            .parent()
            .context("Autostart entry has no parent directory")?;
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
                .context("Autostart entry appeared before publication; existing entry preserved")?;
            File::open(parent)?
                .sync_all()
                .context("Autostart entry was created, but directory synchronization failed")?;
            Ok(())
        })();
        drop(file);
        let cleanup = std::fs::remove_file(&temporary);
        result?;
        cleanup.context("Autostart entry was created, but temporary file cleanup failed")?;
        Ok(())
    }

    pub(super) fn run(action: &str) -> Result<()> {
        let path = entry_path()?;
        let executable = std::env::current_exe()?.canonicalize()?;
        let expected = contents(&executable)?;
        let entry = inspect(&path, &expected)?;
        let status = match (action, entry) {
            ("status", Entry::Missing) | ("disable", Entry::Missing) => "disabled",
            ("status" | "enable", Entry::Owned(_)) => "enabled",
            ("status", Entry::Conflict) => "conflict",
            ("enable", Entry::Missing) => {
                enable(&path, &expected)?;
                "enabled"
            }
            ("disable", Entry::Owned(metadata)) => {
                let current = path.symlink_metadata()?;
                ensure!(
                    same_file(&metadata, &current) && current.is_file(),
                    "Autostart entry changed before removal; preserved"
                );
                std::fs::remove_file(&path)?;
                File::open(path.parent().unwrap())?
                    .sync_all()
                    .context("Autostart entry was removed, but directory synchronization failed")?;
                "disabled"
            }
            (_, Entry::Conflict) => {
                bail!("Autostart entry conflicts with this executable; existing entry preserved")
            }
            _ => unreachable!("action was validated"),
        };
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

        #[test]
        fn desktop_exec_quotes_reserved_characters_and_literal_field_codes() {
            let path = Path::new("/tmp/a b'c\"d\\e$f`g%F;|&<>~*?()#");
            let quoted = exec_argument(path).unwrap();
            assert_eq!(
                quoted,
                "\"/tmp/a b'c\\\\\"d\\\\\\\\e\\\\$f\\\\`g%%F;|&<>~*?()#\""
            );
            let value = quoted.replace("\\\\", "\\");
            let parsed = glib::shell_parse_argv(&value).unwrap();
            assert_eq!(parsed.len(), 1);
            assert_eq!(
                parsed[0].to_str().unwrap().replace("%%", "%"),
                path.to_str().unwrap()
            );
        }

        #[test]
        fn desktop_exec_rejects_invalid_executable_paths() {
            for path in [
                "relative",
                "/tmp/a=b",
                "/tmp/a\nb",
                "/tmp/a\tb",
                "/tmp/a\0b",
            ] {
                assert!(exec_argument(Path::new(path)).is_err());
            }
            use std::os::unix::ffi::OsStringExt;
            let path = PathBuf::from(OsString::from_vec(vec![b'/', 0xff]));
            assert!(exec_argument(&path).is_err());
        }
    }
}
