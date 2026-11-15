//! Current-user Open With registration; existing default handlers are untouched.

use super::*;
use std::{os::windows::ffi::OsStrExt, ptr};
use windows_sys::Win32::{
    Foundation::{ERROR_FILE_NOT_FOUND, ERROR_MORE_DATA, ERROR_PATH_NOT_FOUND, ERROR_SUCCESS},
    System::Registry::{
        HKEY, HKEY_CURRENT_USER, KEY_QUERY_VALUE, KEY_SET_VALUE, REG_NONE, REG_OPTION_NON_VOLATILE,
        REG_SZ, RegCloseKey, RegCreateKeyExW, RegDeleteValueW, RegOpenKeyExW, RegQueryValueExW,
        RegSetValueExW,
    },
};

const BASE: &str = r"Software\Classes";
const PROGID: &str = "Screenshots.Image";
const MARKER: &str = "Screenshots associations v1";
struct Key(HKEY);
impl Drop for Key {
    fn drop(&mut self) {
        unsafe { RegCloseKey(self.0) };
    }
}
fn wide(value: &str) -> Vec<u16> {
    value.encode_utf16().chain(Some(0)).collect()
}
fn check(code: u32) -> Result<()> {
    if code == ERROR_SUCCESS {
        Ok(())
    } else {
        Err(std::io::Error::from_raw_os_error(code as i32))
            .context("Current-user association registry operation failed")
    }
}
fn open(path: &str, write: bool, create: bool) -> Result<Option<Key>> {
    let path = wide(path);
    let mut key = ptr::null_mut();
    let access = KEY_QUERY_VALUE | if write { KEY_SET_VALUE } else { 0 };
    let result = unsafe {
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
    if !create && matches!(result, ERROR_FILE_NOT_FOUND | ERROR_PATH_NOT_FOUND) {
        return Ok(None);
    }
    check(result)?;
    Ok(Some(Key(key)))
}
#[derive(Clone)]
struct Value {
    path: String,
    name: String,
    kind: u32,
    bytes: Vec<u8>,
}
fn string(path: String, name: &str, value: &str) -> Value {
    Value {
        path,
        name: name.into(),
        kind: REG_SZ,
        bytes: wide(value).into_iter().flat_map(u16::to_le_bytes).collect(),
    }
}
#[derive(PartialEq)]
enum State {
    Missing,
    Owned,
    Conflict,
}
fn inspect(value: &Value) -> Result<State> {
    let Some(key) = open(&value.path, false, false)? else {
        return Ok(State::Missing);
    };
    let name = wide(&value.name);
    let mut kind = 0;
    let mut length = 0;
    let code = unsafe {
        RegQueryValueExW(
            key.0,
            name.as_ptr(),
            ptr::null(),
            &mut kind,
            ptr::null_mut(),
            &mut length,
        )
    };
    if code == ERROR_FILE_NOT_FOUND {
        return Ok(State::Missing);
    }
    check(code)?;
    if kind != value.kind || length as u64 > MAX_ENTRY || length as usize != value.bytes.len() {
        return Ok(State::Conflict);
    }
    let mut bytes = vec![0; length as usize];
    let expected = length;
    let code = unsafe {
        RegQueryValueExW(
            key.0,
            name.as_ptr(),
            ptr::null(),
            &mut kind,
            bytes.as_mut_ptr(),
            &mut length,
        )
    };
    if code == ERROR_MORE_DATA {
        return Ok(State::Conflict);
    }
    if code == ERROR_FILE_NOT_FOUND {
        return Ok(State::Missing);
    }
    check(code)?;
    Ok(
        if kind == value.kind && length == expected && bytes == value.bytes {
            State::Owned
        } else {
            State::Conflict
        },
    )
}
fn set(value: &Value) -> Result<()> {
    let key = open(&value.path, true, true)?.unwrap();
    let name = wide(&value.name);
    check(unsafe {
        RegSetValueExW(
            key.0,
            name.as_ptr(),
            0,
            value.kind,
            value.bytes.as_ptr(),
            value.bytes.len() as u32,
        )
    })
}
fn remove(value: &Value) -> Result<()> {
    let Some(key) = open(&value.path, true, false)? else {
        return Ok(());
    };
    let name = wide(&value.name);
    let result = unsafe { RegDeleteValueW(key.0, name.as_ptr()) };
    if result == ERROR_FILE_NOT_FOUND {
        Ok(())
    } else {
        check(result)
    }
}
fn values(base: &str, executable: &Path) -> Result<Vec<Value>> {
    ensure!(
        executable.is_absolute(),
        "Association executable must be absolute"
    );
    let path = executable
        .to_str()
        .context("Association executable must be UTF-8")?;
    ensure!(
        !path.contains('"') && !path.chars().any(char::is_control) && path.len() <= 4096,
        "Invalid association executable path"
    );
    let prog = format!("{base}\\{PROGID}");
    let mut result = vec![
        string(prog.clone(), "ManagedBy", MARKER),
        string(prog.clone(), "", "Screenshots image"),
        string(
            format!("{prog}\\shell\\open\\command"),
            "",
            &format!("\"{path}\" --open \"%1\""),
        ),
        string(
            format!("{base}\\AppUserModelId\\{ID}"),
            "DisplayName",
            "Screenshots",
        ),
    ];
    for extension in [".png", ".jpg", ".jpeg", ".webp", ".bmp", ".tif", ".tiff"] {
        result.push(Value {
            path: format!("{base}\\{extension}\\OpenWithProgids"),
            name: PROGID.into(),
            kind: REG_NONE,
            bytes: Vec::new(),
        });
    }
    Ok(result)
}
fn manage(action: &str, entries: &[Value]) -> Result<&'static str> {
    let states = entries.iter().map(inspect).collect::<Result<Vec<_>>>()?;
    if states.iter().any(|state| *state == State::Conflict)
        || (states[0] == State::Missing && states.iter().any(|state| *state != State::Missing))
    {
        if action == "status" {
            return Ok("conflict");
        }
        bail!("Association registration conflicts with existing user values; all values preserved");
    }
    if action == "status" {
        return Ok(if states.iter().all(|state| *state == State::Missing) {
            "disabled"
        } else if states.iter().all(|state| *state == State::Owned) {
            "enabled"
        } else {
            "partial"
        });
    }
    if action == "enable" {
        for (entry, state) in entries.iter().zip(states) {
            if state == State::Missing {
                ensure!(
                    inspect(entry)? == State::Missing,
                    "Association changed before writing; inspect partial status"
                );
                set(entry).context("Association registration may be partial; inspect status")?;
            }
        }
        Ok("enabled")
    } else {
        // Remove the ownership marker last so retry can identify a partial removal.
        for entry in entries.iter().skip(1).chain(entries.iter().take(1)) {
            match inspect(entry)? {
                State::Missing => {}
                State::Owned => {
                    remove(entry).context("Association removal may be partial; inspect status")?
                }
                State::Conflict => {
                    bail!("Association changed before removal; remaining values preserved")
                }
            }
        }
        Ok("disabled")
    }
}
pub(super) fn associations(action: &str) -> Result<serde_json::Value> {
    let expected = values(BASE, &std::env::current_exe()?.canonicalize()?)?;
    let status = manage(action, &expected)?;
    if action != "status" {
        #[link(name = "shell32")]
        unsafe extern "system" {
            fn SHChangeNotify(
                event: i32,
                flags: u32,
                item1: *const std::ffi::c_void,
                item2: *const std::ffi::c_void,
            );
        }
        unsafe {
            SHChangeNotify(0x08000000, 0, ptr::null(), ptr::null());
        }
    }
    Ok(
        serde_json::json!({"action":action,"scope":"user_open_with","status":status,"registry_scope":"HKEY_CURRENT_USER\\Software\\Classes","default_handlers":"unchanged"}),
    )
}

fn select_notification_id(
    application: Option<String>,
    package_absent: bool,
    registered: bool,
) -> Result<String> {
    if let Some(application) = application {
        ensure!(
            !application.is_empty()
                && application.len() <= 2048
                && !application.chars().any(char::is_control),
            "Invalid native application identity"
        );
        return Ok(application);
    }
    ensure!(
        package_absent,
        "Packaged notification identity is unavailable"
    );
    ensure!(
        registered,
        "Unpackaged notification identity is not registered; enable desktop associations first"
    );
    Ok(ID.into())
}

pub(super) fn notification_id() -> Result<String> {
    #[link(name = "kernel32")]
    unsafe extern "system" {
        fn GetCurrentApplicationUserModelId(length: *mut u32, identity: *mut u16) -> i32;
        fn GetCurrentPackageFullName(length: *mut u32, name: *mut u16) -> i32;
    }
    let mut length = 0;
    let status = unsafe { GetCurrentApplicationUserModelId(&mut length, ptr::null_mut()) };
    if status == 122 {
        ensure!(
            (2..=1024).contains(&length),
            "Native application identity exceeds its limit"
        );
        let mut identity = vec![0u16; length as usize];
        ensure!(
            unsafe { GetCurrentApplicationUserModelId(&mut length, identity.as_mut_ptr()) } == 0,
            "Could not read native application identity"
        );
        ensure!(
            length as usize == identity.len()
                && identity.last() == Some(&0)
                && !identity[..identity.len() - 1].contains(&0),
            "Invalid native application identity"
        );
        let application = String::from_utf16(&identity[..identity.len() - 1])
            .context("Native application identity is not Unicode")?;
        return select_notification_id(Some(application), false, false);
    }
    ensure!(
        status == 15703 || status == 15700,
        "Could not query native application identity"
    );
    let package_absent =
        unsafe { GetCurrentPackageFullName(&mut length, ptr::null_mut()) } == 15700;
    let registered = inspect(&string(
        format!("{BASE}\\AppUserModelId\\{ID}"),
        "DisplayName",
        "Screenshots",
    ))? == State::Owned
        && inspect(&string(format!("{BASE}\\{PROGID}"), "ManagedBy", MARKER))? == State::Owned;
    select_notification_id(None, package_absent, registered)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn notifications_keep_packaged_aumid_and_require_registered_unpacked_identity() {
        assert_eq!(
            select_notification_id(
                Some("me.lejy.screenshots_abcd123!Screenshots".into()),
                false,
                false
            )
            .unwrap(),
            "me.lejy.screenshots_abcd123!Screenshots"
        );
        assert!(select_notification_id(None, false, true).is_err());
        assert!(select_notification_id(None, true, false).is_err());
        assert_eq!(select_notification_id(None, true, true).unwrap(), ID);
    }
    #[test]
    fn isolated_registry_registration_preserves_conflicts_and_other_values() {
        let base = format!(
            r"Software\ScreenshotsTests\Associations\{}",
            uuid::Uuid::new_v4()
        );
        let expected = values(&base, Path::new(r"C:\A folder\screenshots.exe")).unwrap();
        assert_eq!(manage("status", &expected).unwrap(), "disabled");
        assert_eq!(manage("enable", &expected).unwrap(), "enabled");
        assert_eq!(manage("enable", &expected).unwrap(), "enabled");
        assert_eq!(manage("status", &expected).unwrap(), "enabled");
        let foreign = string(expected[4].path.clone(), "Other.Application", "unchanged");
        set(&foreign).unwrap();
        assert_eq!(manage("disable", &expected).unwrap(), "disabled");
        assert!(inspect(&foreign).unwrap() == State::Owned);
        manage("enable", &expected).unwrap();
        let mut edited = expected[2].clone();
        edited.bytes = wide("foreign command")
            .into_iter()
            .flat_map(u16::to_le_bytes)
            .collect();
        set(&edited).unwrap();
        assert_eq!(manage("status", &expected).unwrap(), "conflict");
        assert!(manage("disable", &expected).is_err());
        assert!(inspect(&edited).unwrap() == State::Owned);
        use windows_sys::Win32::System::Registry::RegDeleteTreeW;
        unsafe {
            assert_eq!(
                RegDeleteTreeW(HKEY_CURRENT_USER, wide(&base).as_ptr()),
                ERROR_SUCCESS
            );
        }
    }
    #[test]
    fn command_quotes_one_path_and_one_file_argument() {
        let expected = values(BASE, Path::new(r"C:\space $`% &\screenshots.exe")).unwrap();
        let command = String::from_utf16(
            &expected[2]
                .bytes
                .chunks_exact(2)
                .map(|bytes| u16::from_le_bytes(bytes.try_into().unwrap()))
                .collect::<Vec<_>>(),
        )
        .unwrap();
        assert_eq!(
            command,
            "\"C:\\space $`% &\\screenshots.exe\" --open \"%1\"\0"
        );
        assert!(values(BASE, Path::new("relative.exe")).is_err());
        assert!(values(BASE, Path::new("C:\\a\n.exe")).is_err());
    }
}
