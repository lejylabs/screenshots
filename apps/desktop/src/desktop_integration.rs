//! Explicit user desktop integration; no work is started by normal idle rendering.

use anyhow::{Context, Result, bail, ensure};
use std::{
    ffi::OsString,
    io::Write,
    path::Path,
    process::Command,
    time::{Duration, Instant},
};

const ID: &str = "me.lejy.screenshots";
const MAX_ENTRY: u64 = 16 * 1024;

#[cfg(target_os = "windows")]
#[path = "desktop_integration_windows.rs"]
mod windows;

fn emit(value: serde_json::Value, completed: &str) -> Result<()> {
    let mut output = std::io::stdout().lock();
    writeln!(output, "{value}")
        .and_then(|_| output.flush())
        .context(completed.to_owned())
}

pub(crate) fn run(args: &[OsString]) -> Result<()> {
    match args.first().and_then(|value| value.to_str()) {
        Some("associations")
            if args.len() == 2
                && matches!(args[1].to_str(), Some("status" | "enable" | "disable")) =>
        {
            let action = args[1].to_str().unwrap();
            let result = associations(action)?;
            emit(
                result,
                "Association operation completed, but its result could not be written; inspect status",
            )
        }
        Some("notify") if args.len() == 3 => {
            let title = args[1]
                .to_str()
                .context("Notification title must be UTF-8")?;
            let body = args[2]
                .to_str()
                .context("Notification body must be UTF-8")?;
            ensure!(
                !title.is_empty() && title.len() <= 128 && !title.chars().any(char::is_control),
                "Notification title must contain 1–128 bytes without controls"
            );
            ensure!(
                !body.is_empty()
                    && body.len() <= 2048
                    && !body.chars().any(|ch| ch.is_control() && ch != '\n'),
                "Notification body must contain 1–2048 bytes without controls other than newline"
            );
            submit_notification(title, body)?;
            emit(
                serde_json::json!({"status":"submitted","platform":std::env::consts::OS}),
                "Notification was submitted, but its result could not be written",
            )
        }
        _ => bail!(
            "Usage: screenshots desktop associations status|enable|disable; screenshots desktop notify TITLE BODY"
        ),
    }
}

fn helper(command: Command) -> Result<()> {
    let deadline = Instant::now() + Duration::from_secs(5);
    #[cfg(unix)]
    {
        let cancellation = crate::record::unix::Cancellation::new()?;
        crate::record::unix::engine(command, false, &cancellation, deadline)
            .map(|_| ()).map_err(|_| anyhow::anyhow!("Desktop helper failed, exceeded its deadline, or returned excessive diagnostics"))
    }
    #[cfg(target_os = "windows")]
    {
        crate::record_windows::engine(command, None, deadline)
            .map(|_| ())
            .map_err(|_| {
                anyhow::anyhow!(
                    "Desktop helper failed, exceeded its deadline, or returned excessive output"
                )
            })
    }
    #[cfg(not(any(unix, target_os = "windows")))]
    {
        let _ = command;
        let _ = deadline;
        bail!("Desktop integration is unsupported on this platform")
    }
}

fn markup(value: &str) -> String {
    value
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
}

fn notification_command(title: &str, body: &str) -> Command {
    #[cfg(target_os = "linux")]
    {
        let mut command = Command::new("notify-send");
        command.args([
            "--app-name",
            "Screenshots",
            "--icon",
            "camera-photo",
            "--expire-time",
            "5000",
            "--",
            title,
            &markup(body),
        ]);
        command
    }
    #[cfg(target_os = "macos")]
    {
        let mut command = Command::new("/usr/bin/osascript");
        command.args(["-e", "on run argv\ndisplay notification (item 2 of argv) with title (item 1 of argv)\nend run", "--", title, body]);
        command
    }
    #[cfg(target_os = "windows")]
    {
        let mut command = Command::new("powershell.exe");
        // Text stays in environment values, never in PowerShell source or XML markup.
        command.args(["-NoLogo", "-NoProfile", "-NonInteractive", "-Command", "$ErrorActionPreference='Stop';[Windows.UI.Notifications.ToastNotificationManager,Windows.UI.Notifications,ContentType=WindowsRuntime]>$null;[Windows.Data.Xml.Dom.XmlDocument,Windows.Data.Xml.Dom.XmlDocument,ContentType=WindowsRuntime]>$null;$x=New-Object Windows.Data.Xml.Dom.XmlDocument;$x.LoadXml('<toast><visual><binding template=\"ToastGeneric\"><text/><text/></binding></visual></toast>');$n=$x.GetElementsByTagName('text');$n.Item(0).AppendChild($x.CreateTextNode($env:SCREENSHOTS_NOTIFICATION_TITLE))>$null;$n.Item(1).AppendChild($x.CreateTextNode($env:SCREENSHOTS_NOTIFICATION_BODY))>$null;$t=[Windows.UI.Notifications.ToastNotification]::new($x);[Windows.UI.Notifications.ToastNotificationManager]::CreateToastNotifier($env:SCREENSHOTS_NOTIFICATION_AUMID).Show($t)"]);
        command
            .env("SCREENSHOTS_NOTIFICATION_TITLE", title)
            .env("SCREENSHOTS_NOTIFICATION_BODY", body);
        command
    }
    #[cfg(not(any(target_os = "linux", target_os = "macos", target_os = "windows")))]
    {
        let _ = (title, body);
        Command::new("screenshots-unsupported-desktop")
    }
}

fn submit_notification(title: &str, body: &str) -> Result<()> {
    let command = notification_command(title, body);
    #[cfg(target_os = "windows")]
    let command = {
        let mut command = command;
        command.env(
            "SCREENSHOTS_NOTIFICATION_AUMID",
            windows::notification_id()?,
        );
        command
    };
    helper(command).context("Notification was not confirmed submitted")
}

#[cfg(target_os = "linux")]
fn associations(action: &str) -> Result<serde_json::Value> {
    linux::associations(action)
}
#[cfg(target_os = "windows")]
fn associations(action: &str) -> Result<serde_json::Value> {
    windows::associations(action)
}
#[cfg(target_os = "macos")]
fn associations(action: &str) -> Result<serde_json::Value> {
    let executable = std::env::current_exe()?.canonicalize()?;
    let bundle = executable
        .parent()
        .and_then(Path::parent)
        .and_then(Path::parent)
        .filter(|path| path.extension().is_some_and(|value| value == "app"))
        .context("Open With registration on macOS requires the installed Screenshots.app bundle")?;
    let info = bundle.join("Contents/Info.plist");
    let metadata = info.metadata()?;
    ensure!(
        metadata.is_file() && metadata.len() <= MAX_ENTRY,
        "Application bundle metadata is unavailable or excessive"
    );
    let mut command = Command::new(
        "/System/Library/Frameworks/CoreServices.framework/Frameworks/LaunchServices.framework/Support/lsregister",
    );
    if action != "status" {
        command
            .arg(if action == "disable" { "-u" } else { "-f" })
            .arg(bundle);
        helper(command).context("LaunchServices registration was not confirmed")?;
    }
    Ok(
        serde_json::json!({"action":action,"scope":"user_open_with","status":if action=="disable" {"disabled"} else if action=="enable" {"enabled"} else {"bundle_available"},"path":bundle.to_string_lossy(),"path_encoding":"lossy-native"}),
    )
}
#[cfg(not(any(target_os = "linux", target_os = "macos", target_os = "windows")))]
fn associations(_: &str) -> Result<serde_json::Value> {
    bail!("Open With registration is unsupported on this platform")
}

#[cfg(target_os = "linux")]
mod linux {
    use super::*;
    use std::{
        fs::{File, Metadata, OpenOptions},
        io::Read,
        os::unix::fs::{DirBuilderExt, MetadataExt, OpenOptionsExt},
        path::PathBuf,
    };

    fn path() -> Result<PathBuf> {
        let base = if let Some(value) =
            std::env::var_os("XDG_DATA_HOME").filter(|value| !value.is_empty())
        {
            PathBuf::from(value)
        } else {
            PathBuf::from(std::env::var_os("HOME").context("HOME is not set")?).join(".local/share")
        };
        ensure!(base.is_absolute(), "Desktop data home must be absolute");
        Ok(base.join("applications/me.lejy.screenshots.desktop"))
    }

    fn exec(path: &Path) -> Result<String> {
        let value = path
            .to_str()
            .context("Association executable must be UTF-8")?;
        ensure!(
            path.is_absolute() && !value.contains('=') && !value.chars().any(char::is_control),
            "Invalid association executable path"
        );
        let mut result = String::from("\"");
        for ch in value.chars() {
            match ch {
                '\\' => result.push_str("\\\\\\\\"),
                '"' => result.push_str("\\\\\""),
                '`' => result.push_str("\\\\`"),
                '$' => result.push_str("\\\\$"),
                '%' => result.push_str("%%"),
                _ => result.push(ch),
            }
        }
        result.push('"');
        Ok(result)
    }

    fn contents(executable: &Path) -> Result<Vec<u8>> {
        let data = format!(
            "# Managed by screenshots associations v1; edited entries are preserved.\n[Desktop Entry]\nType=Application\nVersion=1.0\nName=Screenshots\nComment=Open an image for capture editing\nExec={} --open %f\nTerminal=false\nCategories=Graphics;Utility;\nMimeType=image/png;image/jpeg;image/webp;image/bmp;image/tiff;\nStartupNotify=true\n",
            exec(executable)?
        );
        ensure!(
            data.len() as u64 <= MAX_ENTRY,
            "Association entry exceeds 16 KiB"
        );
        Ok(data.into_bytes())
    }

    fn same(a: &Metadata, b: &Metadata) -> bool {
        a.dev() == b.dev()
            && a.ino() == b.ino()
            && a.len() == b.len()
            && a.mtime() == b.mtime()
            && a.mtime_nsec() == b.mtime_nsec()
            && a.ctime() == b.ctime()
            && a.ctime_nsec() == b.ctime_nsec()
    }
    enum State {
        Missing,
        Owned(Metadata),
        Conflict,
    }
    fn inspect(path: &Path, expected: &[u8]) -> Result<State> {
        let metadata = match path.symlink_metadata() {
            Ok(value) => value,
            Err(value) if value.kind() == std::io::ErrorKind::NotFound => {
                return Ok(State::Missing);
            }
            Err(value) => return Err(value.into()),
        };
        if !metadata.is_file() || metadata.len() > MAX_ENTRY {
            return Ok(State::Conflict);
        }
        let file = OpenOptions::new()
            .read(true)
            .custom_flags(libc::O_NONBLOCK | libc::O_NOFOLLOW)
            .open(path)?;
        let opened = file.metadata()?;
        if !opened.is_file() || opened.len() > MAX_ENTRY || !same(&metadata, &opened) {
            return Ok(State::Conflict);
        }
        let mut bytes = Vec::with_capacity(expected.len());
        file.take(MAX_ENTRY + 1).read_to_end(&mut bytes)?;
        let current = path.symlink_metadata()?;
        if bytes != expected || !same(&opened, &current) {
            return Ok(State::Conflict);
        }
        Ok(State::Owned(current))
    }
    fn publish(path: &Path, bytes: &[u8]) -> Result<()> {
        let parent = path.parent().context("Association has no parent")?;
        std::fs::DirBuilder::new()
            .recursive(true)
            .mode(0o700)
            .create(parent)?;
        let temporary = parent.join(format!(
            ".screenshots-association-{}.part",
            uuid::Uuid::new_v4()
        ));
        let result = (|| -> Result<()> {
            let mut file = OpenOptions::new()
                .write(true)
                .create_new(true)
                .mode(0o600)
                .open(&temporary)?;
            file.write_all(bytes)?;
            file.sync_all()?;
            std::fs::hard_link(&temporary, path)
                .context("Association destination appeared; existing entry preserved")?;
            File::open(parent)
                .and_then(|directory| directory.sync_all())
                .context("Association was created but directory sync failed")?;
            Ok(())
        })();
        let cleanup = std::fs::remove_file(&temporary);
        result?;
        cleanup.context("Association was created but staging cleanup failed")?;
        Ok(())
    }
    pub(super) fn associations(action: &str) -> Result<serde_json::Value> {
        let path = path()?;
        let expected = contents(&std::env::current_exe()?.canonicalize()?)?;
        let status = match (action, inspect(&path, &expected)?) {
            ("status", State::Missing) | ("disable", State::Missing) => "disabled",
            ("status" | "enable", State::Owned(_)) => "enabled",
            ("status", State::Conflict) => "conflict",
            ("enable", State::Missing) => {
                publish(&path, &expected)?;
                "enabled"
            }
            ("disable", State::Owned(metadata)) => {
                let current = path.symlink_metadata()?;
                ensure!(
                    current.is_file() && same(&metadata, &current),
                    "Association entry changed before removal; preserved"
                );
                std::fs::remove_file(&path)?;
                File::open(path.parent().unwrap())
                    .and_then(|directory| directory.sync_all())
                    .context("Association removed but directory sync failed")?;
                "disabled"
            }
            (_, State::Conflict) => {
                bail!("Association conflicts with this executable; existing entry preserved")
            }
            _ => unreachable!(),
        };
        // Updating the cache is optional; the owned desktop file is the source of truth.
        let refresh = if action != "status" {
            let mut command = Command::new("update-desktop-database");
            command.arg(path.parent().unwrap());
            match command.get_program().to_str().and_then(|name| {
                std::env::var_os("PATH").map(|paths| {
                    std::env::split_paths(&paths).any(|base| base.join(name).is_file())
                })
            }) {
                Some(true) => {
                    helper(command).context(
                        "Association operation completed, but desktop cache refresh failed",
                    )?;
                    "completed"
                }
                _ => "tool_unavailable",
            }
        } else {
            "not_requested"
        };
        Ok(
            serde_json::json!({"action":action,"status":status,"scope":"user_open_with","path":path.to_string_lossy(),"path_encoding":"lossy-native","cache_refresh":refresh}),
        )
    }

    #[cfg(test)]
    mod tests {
        use super::*;
        #[test]
        fn entry_quotes_one_executable_and_one_file_field() {
            let text = String::from_utf8(contents(Path::new("/a b/$`%\\\"/screenshots")).unwrap())
                .unwrap();
            assert!(text.contains(" --open %f\n"));
            assert_eq!(text.matches("%f").count(), 1);
            assert!(text.contains("%%"));
            assert!(contents(Path::new("relative")).is_err());
            assert!(contents(Path::new("/a\nfile")).is_err());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn notification_body_escapes_markup_and_never_becomes_script() {
        assert_eq!(markup("<tag>&$`'\""), "&lt;tag&gt;&amp;$`'\"");
        let command = notification_command("A $`<", "B<&");
        #[cfg(target_os = "linux")]
        assert_eq!(command.get_args().last().unwrap(), "B&lt;&amp;");
        #[cfg(target_os = "windows")]
        assert!(!command.get_args().any(|arg| arg == "A $`<"));
    }
}
