//! Native Windows press-only hotkeys and private endpoint handles.
use super::{EndpointInfo, Host, Key, Wake, json_object};
use anyhow::{Context, Result, ensure};
use std::{
    fs::File,
    io::{Read, Write},
    os::windows::{
        ffi::OsStrExt,
        io::{AsRawHandle, FromRawHandle, OwnedHandle},
    },
    path::Path,
    sync::atomic::Ordering,
    sync::{Arc, Mutex, OnceLock, Weak},
};
use windows_sys::Win32::{
    Foundation::*,
    Security::{Authorization::*, *},
    Storage::FileSystem::*,
    System::Threading::*,
    UI::{Input::KeyboardAndMouse::*, WindowsAndMessaging::*},
};
use zeroize::Zeroizing;

pub(super) struct Native {
    host: Arc<Host>,
    thread: u32,
    registered: Vec<i32>,
}
fn key(key: &Key) -> Result<(u32, u32)> {
    let mods = MOD_NOREPEAT
        | (if key.modifiers & 1 != 0 {
            MOD_CONTROL
        } else {
            0
        })
        | (if key.modifiers & 2 != 0 { MOD_ALT } else { 0 })
        | (if key.modifiers & 4 != 0 { MOD_SHIFT } else { 0 })
        | (if key.modifiers & 8 != 0 { MOD_WIN } else { 0 });
    let vk = if key.name.len() == 1 {
        u32::from(key.name.as_bytes()[0])
    } else if key.name == "PrintScreen" {
        u32::from(VK_SNAPSHOT)
    } else {
        0x70 + key
            .name
            .strip_prefix('F')
            .context("Unsupported Windows task key")?
            .parse::<u32>()?
            - 1
    };
    Ok((mods, vk))
}
impl Native {
    pub(super) fn new(host: Arc<Host>) -> Result<Self> {
        let mut message: MSG = unsafe { std::mem::zeroed() };
        unsafe {
            PeekMessageW(&mut message, std::ptr::null_mut(), 0, 0, PM_NOREMOVE);
        }
        let mut native = Self {
            host,
            thread: unsafe { GetCurrentThreadId() },
            registered: Vec::new(),
        };
        for (index, entry) in native.host.config.keys.iter().enumerate() {
            let (mods, vk) = key(entry)?;
            let id = index as i32 + 1;
            ensure!(
                unsafe { RegisterHotKey(std::ptr::null_mut(), id, mods, vk) } != 0,
                "A configured Windows task hotkey is unavailable or conflicted"
            );
            native.registered.push(id);
        }
        Ok(native)
    }
    pub(super) fn wake(&self) -> Wake {
        let thread = self.thread;
        Arc::new(move || unsafe {
            PostThreadMessageW(thread, WM_QUIT, 0, 0);
        })
    }
    pub(super) fn run(&mut self) -> Result<()> {
        let mut message: MSG = unsafe { std::mem::zeroed() };
        while !self.host.stopping.load(Ordering::SeqCst) {
            let ready = unsafe { GetMessageW(&mut message, std::ptr::null_mut(), 0, 0) };
            ensure!(ready >= 0, "Windows task hotkey event loop failed");
            if ready == 0 {
                break;
            }
            if message.message == WM_HOTKEY {
                if let Some(entry) = message
                    .wParam
                    .checked_sub(1)
                    .and_then(|index| self.host.config.keys.get(index))
                {
                    let _ = self.host.trigger(entry.profile);
                }
            }
        }
        Ok(())
    }
}
impl Drop for Native {
    fn drop(&mut self) {
        unsafe {
            for id in &self.registered {
                UnregisterHotKey(std::ptr::null_mut(), *id);
            }
        }
    }
}

static SIGNAL_HOST: OnceLock<Mutex<Option<Weak<Host>>>> = OnceLock::new();
unsafe extern "system" fn console_signal(code: u32) -> i32 {
    if matches!(code, 0 | 1 | 2 | 5 | 6) {
        if let Some(host) = SIGNAL_HOST
            .get()
            .and_then(|entry| entry.lock().ok())
            .and_then(|entry| entry.as_ref().and_then(Weak::upgrade))
        {
            host.stop();
        }
        1
    } else {
        0
    }
}
pub(super) struct SignalGuard;
impl SignalGuard {
    pub(super) fn new(host: Arc<Host>) -> Result<Self> {
        *SIGNAL_HOST.get_or_init(|| Mutex::new(None)).lock().unwrap() = Some(Arc::downgrade(&host));
        ensure!(
            unsafe {
                windows_sys::Win32::System::Console::SetConsoleCtrlHandler(Some(console_signal), 1)
            } != 0,
            "Could not install task-host console cancellation"
        );
        Ok(Self)
    }
}
impl Drop for SignalGuard {
    fn drop(&mut self) {
        unsafe {
            windows_sys::Win32::System::Console::SetConsoleCtrlHandler(Some(console_signal), 0);
        }
        *SIGNAL_HOST.get().unwrap().lock().unwrap() = None;
    }
}

struct User {
    _buffer: Vec<usize>,
    sid: PSID,
    text: String,
}
impl User {
    fn current() -> Result<Self> {
        let mut raw = std::ptr::null_mut();
        ensure!(
            unsafe { OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &mut raw) } != 0,
            "Could not identify task-host user"
        );
        let token = unsafe { OwnedHandle::from_raw_handle(raw) };
        let mut size = 0;
        unsafe {
            GetTokenInformation(
                token.as_raw_handle(),
                TokenUser,
                std::ptr::null_mut(),
                0,
                &mut size,
            );
        }
        ensure!(
            size >= std::mem::size_of::<TOKEN_USER>() as u32 && size <= 65536,
            "Invalid task-host user token"
        );
        let mut buffer = vec![0usize; (size as usize).div_ceil(std::mem::size_of::<usize>())];
        ensure!(
            unsafe {
                GetTokenInformation(
                    token.as_raw_handle(),
                    TokenUser,
                    buffer.as_mut_ptr().cast(),
                    size,
                    &mut size,
                )
            } != 0,
            "Could not read task-host user token"
        );
        let user = unsafe { &*buffer.as_ptr().cast::<TOKEN_USER>() };
        let sid = user.User.Sid;
        let mut text = std::ptr::null_mut();
        ensure!(
            unsafe { ConvertSidToStringSidW(sid, &mut text) } != 0,
            "Could not encode task-host user SID"
        );
        let result = (|| {
            let mut length = 0;
            while length < 256 && unsafe { *text.add(length) } != 0 {
                length += 1;
            }
            ensure!(length < 256, "Invalid task-host user SID");
            Ok(String::from_utf16(unsafe {
                std::slice::from_raw_parts(text, length)
            })?)
        })();
        unsafe {
            LocalFree(text.cast());
        }
        Ok(Self {
            _buffer: buffer,
            sid,
            text: result?,
        })
    }
}
struct Descriptor(PSECURITY_DESCRIPTOR);
impl Descriptor {
    fn new(user: &User) -> Result<Self> {
        let value: Vec<u16> = format!("O:{}D:P(A;;FA;;;{})", user.text, user.text)
            .encode_utf16()
            .chain(Some(0))
            .collect();
        let mut raw = std::ptr::null_mut();
        ensure!(
            unsafe {
                ConvertStringSecurityDescriptorToSecurityDescriptorW(
                    value.as_ptr(),
                    SDDL_REVISION_1,
                    &mut raw,
                    std::ptr::null_mut(),
                )
            } != 0,
            "Could not prepare private task-host permissions"
        );
        Ok(Self(raw))
    }
}
impl Drop for Descriptor {
    fn drop(&mut self) {
        unsafe {
            LocalFree(self.0);
        }
    }
}
fn acl_bytes(descriptor: PSECURITY_DESCRIPTOR) -> Result<Vec<u8>> {
    let (mut present, mut defaulted, mut acl) = (0, 0, std::ptr::null_mut());
    ensure!(
        unsafe { GetSecurityDescriptorDacl(descriptor, &mut present, &mut acl, &mut defaulted) }
            != 0
            && present != 0
            && !acl.is_null(),
        "Task-host endpoint DACL is missing"
    );
    let mut size = ACL_SIZE_INFORMATION::default();
    ensure!(
        unsafe {
            GetAclInformation(
                acl,
                (&mut size as *mut ACL_SIZE_INFORMATION).cast(),
                std::mem::size_of_val(&size) as u32,
                AclSizeInformation,
            )
        } != 0
            && size.AceCount == 1
            && size.AclBytesInUse <= 65536,
        "Task-host endpoint DACL is not restricted to one user"
    );
    Ok(
        unsafe { std::slice::from_raw_parts(acl.cast::<u8>(), size.AclBytesInUse as usize) }
            .to_vec(),
    )
}
fn path_wide(path: &Path) -> Result<Vec<u16>> {
    ensure!(
        path.as_os_str().as_encoded_bytes().len() <= 4096,
        "Task-host endpoint path is too long"
    );
    let mut wide = path.as_os_str().encode_wide().collect::<Vec<_>>();
    ensure!(
        !wide.is_empty() && !wide.contains(&0),
        "Invalid task-host endpoint path"
    );
    wide.push(0);
    Ok(wide)
}
fn validate(file: &File, user: &User, expected: &Descriptor) -> Result<u64> {
    let mut info: BY_HANDLE_FILE_INFORMATION = unsafe { std::mem::zeroed() };
    ensure!(
        unsafe { GetFileInformationByHandle(file.as_raw_handle(), &mut info) } != 0
            && info.dwFileAttributes & (FILE_ATTRIBUTE_REPARSE_POINT | FILE_ATTRIBUTE_DIRECTORY)
                == 0
            && info.nNumberOfLinks == 1
            && info.nFileSizeHigh == 0
            && (1..=4096).contains(&info.nFileSizeLow),
        "Task-host endpoint must be a private regular file within 4096 bytes"
    );
    let (mut owner, mut actual) = (std::ptr::null_mut(), std::ptr::null_mut());
    ensure!(
        unsafe {
            GetSecurityInfo(
                file.as_raw_handle(),
                SE_FILE_OBJECT,
                OWNER_SECURITY_INFORMATION | DACL_SECURITY_INFORMATION,
                &mut owner,
                std::ptr::null_mut(),
                std::ptr::null_mut(),
                std::ptr::null_mut(),
                &mut actual,
            )
        } == ERROR_SUCCESS,
        "Could not read task-host endpoint permissions"
    );
    let actual = Descriptor(actual);
    let (mut control, mut revision) = (0, 0);
    ensure!(
        !owner.is_null() && unsafe { EqualSid(owner, user.sid) } != 0,
        "Task-host endpoint belongs to another user"
    );
    ensure!(
        unsafe { GetSecurityDescriptorControl(actual.0, &mut control, &mut revision) } != 0
            && control & SE_DACL_PROTECTED != 0
            && acl_bytes(actual.0)? == acl_bytes(expected.0)?,
        "Task-host endpoint permissions are not current-user-only"
    );
    Ok(u64::from(info.nFileSizeLow))
}
pub(super) struct EndpointLease {
    file: File,
}
impl EndpointLease {
    pub(super) fn create(path: &Path, value: &EndpointInfo) -> Result<Self> {
        let user = User::current()?;
        let descriptor = Descriptor::new(&user)?;
        let path = path_wide(path)?;
        let attributes = SECURITY_ATTRIBUTES {
            nLength: std::mem::size_of::<SECURITY_ATTRIBUTES>() as u32,
            lpSecurityDescriptor: descriptor.0,
            bInheritHandle: 0,
        };
        let raw = unsafe {
            CreateFileW(
                path.as_ptr(),
                GENERIC_READ | GENERIC_WRITE | READ_CONTROL | DELETE,
                FILE_SHARE_READ | FILE_SHARE_DELETE,
                &attributes,
                CREATE_NEW,
                FILE_ATTRIBUTE_NORMAL | FILE_FLAG_OPEN_REPARSE_POINT,
                std::ptr::null_mut(),
            )
        };
        ensure!(
            raw != INVALID_HANDLE_VALUE,
            "Task-host endpoint already exists or could not be created"
        );
        let mut lease = Self {
            file: unsafe { File::from_raw_handle(raw) },
        };
        let bytes = Zeroizing::new(serde_json::to_vec(value)?);
        ensure!(bytes.len() <= 4096, "Task-host endpoint is too large");
        lease.file.write_all(&bytes)?;
        lease.file.sync_all()?;
        validate(&lease.file, &user, &descriptor)?;
        Ok(lease)
    }
    pub(super) fn cleanup(&mut self) -> Result<()> {
        let mut disposition = FILE_DISPOSITION_INFO { DeleteFile: true };
        ensure!(
            unsafe {
                SetFileInformationByHandle(
                    self.file.as_raw_handle(),
                    FileDispositionInfo,
                    (&mut disposition as *mut FILE_DISPOSITION_INFO).cast(),
                    std::mem::size_of_val(&disposition) as u32,
                )
            } != 0,
            "Could not remove owned task-host endpoint"
        );
        Ok(())
    }
}
impl Drop for EndpointLease {
    fn drop(&mut self) {
        let _ = self.cleanup();
    }
}
pub(super) fn read_endpoint(path: &Path) -> Result<EndpointInfo> {
    let user = User::current()?;
    let descriptor = Descriptor::new(&user)?;
    let path = path_wide(path)?;
    let raw = unsafe {
        CreateFileW(
            path.as_ptr(),
            GENERIC_READ | READ_CONTROL,
            FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE,
            std::ptr::null(),
            OPEN_EXISTING,
            FILE_FLAG_OPEN_REPARSE_POINT,
            std::ptr::null_mut(),
        )
    };
    ensure!(
        raw != INVALID_HANDLE_VALUE,
        "Could not open private task-host endpoint"
    );
    let file = unsafe { File::from_raw_handle(raw) };
    let length = validate(&file, &user, &descriptor)?;
    let mut bytes = Zeroizing::new(Vec::with_capacity(length as usize));
    file.take(length).read_to_end(&mut bytes)?;
    json_object(&bytes)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn native_modifiers_and_keys_are_press_only() {
        let key = Key {
            profile: 0,
            modifiers: 1 | 4,
            name: "F24".into(),
        };
        let (mods, vk) = super::key(&key).unwrap();
        assert_ne!(mods & MOD_NOREPEAT, 0);
        assert_ne!(mods & MOD_CONTROL, 0);
        assert_ne!(mods & MOD_SHIFT, 0);
        assert_eq!(vk, 0x87);
    }
}
