//! Explicit management of one current-user Windows Run value.

use anyhow::{Context, Result, bail, ensure};
use std::{io::Write, os::windows::ffi::OsStrExt, path::Path, ptr};
use windows_sys::Win32::{
    Foundation::{ERROR_FILE_NOT_FOUND, ERROR_MORE_DATA, ERROR_PATH_NOT_FOUND, ERROR_SUCCESS},
    System::Registry::{
        HKEY, HKEY_CURRENT_USER, KEY_QUERY_VALUE, KEY_SET_VALUE, REG_OPTION_NON_VOLATILE, REG_SZ,
        RegCloseKey, RegCreateKeyExW, RegDeleteValueW, RegOpenKeyExW, RegQueryValueExW,
        RegSetValueExW,
    },
};

const RUN_KEY: &str = r"Software\Microsoft\Windows\CurrentVersion\Run";
const VALUE_NAME: &str = "Screenshots";
const MAX_VALUE_BYTES: usize = 16 * 1024;

struct OwnedKey(HKEY);

impl Drop for OwnedKey {
    fn drop(&mut self) {
        // This handle was returned by a successful registry open/create call.
        unsafe { RegCloseKey(self.0) };
    }
}

fn wide(value: &str) -> Vec<u16> {
    value.encode_utf16().chain(Some(0)).collect()
}

fn checked(status: u32, context: &str) -> Result<()> {
    if status != ERROR_SUCCESS {
        return Err(std::io::Error::from_raw_os_error(status as i32)).context(context.to_owned());
    }
    Ok(())
}

fn open_key(path: &str, write: bool, create: bool) -> Result<Option<OwnedKey>> {
    let path = wide(path);
    let mut key = ptr::null_mut();
    let access = KEY_QUERY_VALUE | if write { KEY_SET_VALUE } else { 0 };
    // Buffers are NUL-terminated and remain valid through the synchronous call.
    let status = unsafe {
        if create {
            RegCreateKeyExW(
                HKEY_CURRENT_USER,
                path.as_ptr(),
                0,
                ptr::null(),
                REG_OPTION_NON_VOLATILE,
                access,
                ptr::null(),
                &mut key,
                ptr::null_mut(),
            )
        } else {
            RegOpenKeyExW(HKEY_CURRENT_USER, path.as_ptr(), 0, access, &mut key)
        }
    };
    if !create && matches!(status, ERROR_FILE_NOT_FOUND | ERROR_PATH_NOT_FOUND) {
        return Ok(None);
    }
    checked(status, "Could not open current-user autostart registry key")?;
    Ok(Some(OwnedKey(key)))
}

#[derive(Debug, PartialEq, Eq)]
enum Entry {
    Missing,
    Owned,
    Conflict,
}

fn inspect(key: &OwnedKey, name: &str, expected: &[u16]) -> Result<Entry> {
    let name = wide(name);
    let mut value_type = 0;
    let mut bytes = 0;
    // Query only the value size/type first; no allocation follows an unbounded size.
    let status = unsafe {
        RegQueryValueExW(
            key.0,
            name.as_ptr(),
            ptr::null(),
            &mut value_type,
            ptr::null_mut(),
            &mut bytes,
        )
    };
    if status == ERROR_FILE_NOT_FOUND {
        return Ok(Entry::Missing);
    }
    checked(status, "Could not inspect autostart registry value")?;
    if value_type != REG_SZ || bytes as usize > MAX_VALUE_BYTES || bytes < 2 || bytes % 2 != 0 {
        return Ok(Entry::Conflict);
    }
    let mut value = vec![0u16; bytes as usize / 2];
    let allocated = bytes;
    // Vec<u16> provides aligned storage; capacity is the byte count supplied to Win32.
    let status = unsafe {
        RegQueryValueExW(
            key.0,
            name.as_ptr(),
            ptr::null(),
            &mut value_type,
            value.as_mut_ptr().cast(),
            &mut bytes,
        )
    };
    if status == ERROR_MORE_DATA {
        return Ok(Entry::Conflict);
    }
    if status == ERROR_FILE_NOT_FOUND {
        return Ok(Entry::Missing);
    }
    checked(status, "Could not read autostart registry value")?;
    if value_type != REG_SZ || bytes != allocated || value != expected {
        return Ok(Entry::Conflict);
    }
    Ok(Entry::Owned)
}

fn quoted_command(executable: &Path) -> Result<Vec<u16>> {
    ensure!(
        executable.is_absolute(),
        "Autostart executable must be an absolute path"
    );
    let path: Vec<u16> = executable.as_os_str().encode_wide().collect();
    ensure!(
        !path.is_empty() && !path.iter().any(|unit| *unit < 32 || *unit == 127),
        "Autostart executable must not contain control characters"
    );
    ensure!(
        path.last() != Some(&(b'\\' as u16)) && !path.contains(&(b'"' as u16)),
        "Autostart executable must be a file path without quotes or a trailing separator"
    );
    let mut command = vec![b'"' as u16];
    command.extend(path);
    command.push(b'"' as u16);
    command.push(0);
    ensure!(
        command.len() <= 260,
        "Windows Run command exceeds 260 UTF-16 units including its terminator"
    );
    Ok(command)
}

fn set_value(key: &OwnedKey, name: &str, value: &[u16]) -> Result<()> {
    let name = wide(name);
    // Value is valid UTF-16 storage including its NUL terminator, bounded by quoted_command.
    checked(
        unsafe {
            RegSetValueExW(
                key.0,
                name.as_ptr(),
                0,
                REG_SZ,
                value.as_ptr().cast(),
                std::mem::size_of_val(value) as u32,
            )
        },
        "Could not create autostart registry value",
    )
}

fn manage(action: &str, key_path: &str, expected: &[u16]) -> Result<&'static str> {
    ensure!(
        matches!(action, "status" | "enable" | "disable"),
        "Invalid autostart action"
    );
    let Some(key) = open_key(key_path, action != "status", action == "enable")? else {
        return Ok("disabled");
    };
    match (action, inspect(&key, VALUE_NAME, expected)?) {
        ("status", Entry::Conflict) => Ok("conflict"),
        ("status" | "enable", Entry::Owned) => Ok("enabled"),
        ("status" | "disable", Entry::Missing) => Ok("disabled"),
        ("enable", Entry::Missing) => {
            ensure!(
                inspect(&key, VALUE_NAME, expected)? == Entry::Missing,
                "Autostart value changed before creation; preserved"
            );
            set_value(&key, VALUE_NAME, expected)?;
            Ok("enabled")
        }
        ("disable", Entry::Owned) => {
            ensure!(
                inspect(&key, VALUE_NAME, expected)? == Entry::Owned,
                "Autostart value changed before removal; preserved"
            );
            let name = wide(VALUE_NAME);
            // The named value was rechecked; no other values or the containing key are removed.
            checked(
                unsafe { RegDeleteValueW(key.0, name.as_ptr()) },
                "Could not remove autostart registry value",
            )?;
            Ok("disabled")
        }
        (_, Entry::Conflict) => {
            bail!("Autostart value conflicts with this executable; existing value preserved")
        }
        _ => unreachable!("action was validated"),
    }
}

pub fn run(action: &str) -> Result<()> {
    let executable = std::env::current_exe()?.canonicalize()?;
    let expected = quoted_command(&executable)?;
    let status = manage(action, RUN_KEY, &expected)?;
    let result = serde_json::json!({"scope":"managed_user_entry", "status":status, "registry_key":format!("HKEY_CURRENT_USER\\{RUN_KEY}"), "value_name":VALUE_NAME});
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
    use windows_sys::Win32::System::Registry::{REG_BINARY, RegDeleteTreeW};

    #[test]
    fn quoted_command_keeps_unicode_spaces_shell_characters_and_backslashes_literal() {
        let path = Path::new(r"C:\Programs and tools\Grüße $`% & !\screenshots.exe");
        assert_eq!(
            quoted_command(path).unwrap(),
            wide(&format!("\"{}\"", path.display()))
        );
        for path in [
            r"relative.exe",
            "C:\\bad\nfile.exe",
            "C:\\bad\0file.exe",
            "C:\\bad\"file.exe",
            "C:\\directory\\",
        ] {
            assert!(quoted_command(Path::new(path)).is_err());
        }
        assert!(quoted_command(Path::new(&format!("C:\\{}.exe", "x".repeat(260)))).is_err());
        let prefixed = Path::new(r"\\?\C:\Program Files\Screenshots\screenshots.exe");
        assert_eq!(
            quoted_command(prefixed).unwrap(),
            wide(&format!("\"{}\"", prefixed.display()))
        );
    }

    #[test]
    fn quoted_canonical_command_launches_the_test_executable_without_a_shell() {
        use windows_sys::Win32::{
            Foundation::{CloseHandle, WAIT_OBJECT_0},
            System::Threading::{
                CreateProcessW, GetExitCodeProcess, PROCESS_INFORMATION, STARTUPINFOW,
                TerminateProcess, WaitForSingleObject,
            },
        };
        let executable = std::env::current_exe().unwrap().canonicalize().unwrap();
        let mut command = quoted_command(&executable).unwrap();
        command.pop();
        command.extend(" --list".encode_utf16().chain(Some(0)));
        let startup = STARTUPINFOW {
            cb: std::mem::size_of::<STARTUPINFOW>() as u32,
            ..Default::default()
        };
        let mut process = PROCESS_INFORMATION::default();
        // Only the generated quoted path and a static harmless test-list argument are executed.
        assert_ne!(
            unsafe {
                CreateProcessW(
                    ptr::null(),
                    command.as_mut_ptr(),
                    ptr::null(),
                    ptr::null(),
                    0,
                    0,
                    ptr::null(),
                    ptr::null(),
                    &startup,
                    &mut process,
                )
            },
            0,
            "{}",
            std::io::Error::last_os_error()
        );
        let wait = unsafe { WaitForSingleObject(process.hProcess, 10_000) };
        if wait != WAIT_OBJECT_0 {
            unsafe {
                TerminateProcess(process.hProcess, 1);
                WaitForSingleObject(process.hProcess, 10_000);
            }
        }
        let mut code = 1;
        let queried = unsafe { GetExitCodeProcess(process.hProcess, &mut code) };
        unsafe {
            CloseHandle(process.hThread);
            CloseHandle(process.hProcess);
        }
        assert_eq!(wait, WAIT_OBJECT_0);
        assert_ne!(queried, 0);
        assert_eq!(code, 0);
    }

    fn isolated_checks(key_path: &str) -> Result<()> {
        let expected = quoted_command(Path::new(r"C:\Test\Screenshots.exe"))?;
        assert_eq!(manage("status", key_path, &expected)?, "disabled");
        assert_eq!(manage("disable", key_path, &expected)?, "disabled");
        assert!(open_key(key_path, false, false)?.is_none());
        assert_eq!(manage("enable", key_path, &expected)?, "enabled");
        assert_eq!(manage("enable", key_path, &expected)?, "enabled");
        assert_eq!(manage("status", key_path, &expected)?, "enabled");
        let key = open_key(key_path, true, false)?.unwrap();
        set_value(&key, "OtherProgram", &wide("leave this value"))?;
        for (label, kind, data) in [
            ("edited", REG_SZ, wide("edited command")),
            ("wrong type", REG_BINARY, expected.clone()),
            ("oversized", REG_SZ, vec![0u16; MAX_VALUE_BYTES / 2 + 1]),
            ("embedded NUL", REG_SZ, wide("edited\0hidden command")),
        ] {
            let name = wide(VALUE_NAME);
            checked(
                unsafe {
                    RegSetValueExW(
                        key.0,
                        name.as_ptr(),
                        0,
                        kind,
                        data.as_ptr().cast(),
                        std::mem::size_of_val(data.as_slice()) as u32,
                    )
                },
                "Write isolated fixture",
            )?;
            let stored = raw_value(&key)?;
            assert_eq!(
                manage("status", key_path, &expected)?,
                "conflict",
                "{label}"
            );
            assert!(manage("enable", key_path, &expected).is_err());
            assert!(manage("disable", key_path, &expected).is_err());
            assert_eq!(raw_value(&key)?, stored, "{label} must remain unchanged");
            assert_eq!(
                inspect(&key, "OtherProgram", &wide("leave this value"))?,
                Entry::Owned
            );
        }
        set_value(&key, VALUE_NAME, &expected)?;
        assert_eq!(manage("disable", key_path, &expected)?, "disabled");
        assert_eq!(manage("disable", key_path, &expected)?, "disabled");
        assert_eq!(
            inspect(&key, "OtherProgram", &wide("leave this value"))?,
            Entry::Owned
        );
        Ok(())
    }

    fn raw_value(key: &OwnedKey) -> Result<(u32, Vec<u8>)> {
        let name = wide(VALUE_NAME);
        let mut kind = 0;
        let mut size = 0;
        checked(
            unsafe {
                RegQueryValueExW(
                    key.0,
                    name.as_ptr(),
                    ptr::null(),
                    &mut kind,
                    ptr::null_mut(),
                    &mut size,
                )
            },
            "Read fixture size",
        )?;
        let mut data = vec![0; size as usize];
        checked(
            unsafe {
                RegQueryValueExW(
                    key.0,
                    name.as_ptr(),
                    ptr::null(),
                    &mut kind,
                    data.as_mut_ptr(),
                    &mut size,
                )
            },
            "Read fixture bytes",
        )?;
        data.truncate(size as usize);
        Ok((kind, data))
    }

    #[test]
    fn registry_actions_use_only_an_isolated_test_key_and_preserve_other_values() {
        const CHILD_KEY: &str = "SCREENSHOTS_AUTOSTART_TEST_REGISTRY_KEY";
        if let Some(path) = std::env::var_os(CHILD_KEY) {
            let path = path.to_str().unwrap();
            assert!(path.starts_with(r"Software\ScreenshotsTest\"));
            isolated_checks(path).unwrap();
            return;
        }
        let key_path = format!(r"Software\ScreenshotsTest\{}", uuid::Uuid::new_v4());
        let mut child = std::process::Command::new(std::env::current_exe().unwrap())
            .args(["--exact", "autostart_windows::tests::registry_actions_use_only_an_isolated_test_key_and_preserve_other_values", "--nocapture"])
            .env(CHILD_KEY, &key_path).spawn().unwrap();
        let started = std::time::Instant::now();
        let status = loop {
            if let Some(status) = child.try_wait().unwrap() {
                break Some(status);
            }
            if started.elapsed() >= std::time::Duration::from_secs(10) {
                child.kill().unwrap();
                child.wait().unwrap();
                break None;
            }
            std::thread::sleep(std::time::Duration::from_millis(10));
        };
        let path = wide(&key_path);
        // The unique fixture subtree never points at the real Run key.
        let cleanup = unsafe { RegDeleteTreeW(HKEY_CURRENT_USER, path.as_ptr()) };
        assert!(matches!(cleanup, ERROR_SUCCESS | ERROR_FILE_NOT_FOUND));
        assert!(
            status.is_some_and(|status| status.success()),
            "isolated registry checks failed or timed out"
        );
    }
}
