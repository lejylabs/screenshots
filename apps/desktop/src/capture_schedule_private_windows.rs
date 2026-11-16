//! Native current-user-only schedule state admission. Same ownership policy as task IPC.
use anyhow::{Result, ensure};
use std::{
    fs::File,
    os::windows::{
        ffi::OsStrExt,
        io::{AsRawHandle, FromRawHandle, OwnedHandle},
    },
    path::Path,
};
use windows_sys::Win32::{
    Foundation::*,
    Security::{Authorization::*, *},
    Storage::FileSystem::*,
    System::Threading::*,
};
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
            "Could not identify scheduler user"
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
            "Invalid scheduler user token"
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
            "Could not read scheduler user token"
        );
        let user = unsafe { &*buffer.as_ptr().cast::<TOKEN_USER>() };
        let sid = user.User.Sid;
        let mut text = std::ptr::null_mut();
        ensure!(
            unsafe { ConvertSidToStringSidW(sid, &mut text) } != 0,
            "Could not encode scheduler user SID"
        );
        let result = (|| {
            let mut length = 0;
            while length < 256 && unsafe { *text.add(length) } != 0 {
                length += 1;
            }
            ensure!(length < 256, "Invalid scheduler user SID");
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
            "Could not prepare private scheduler permissions"
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
        "Schedule endpoint DACL is missing"
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
        "Schedule endpoint DACL is not restricted to one user"
    );
    Ok(
        unsafe { std::slice::from_raw_parts(acl.cast::<u8>(), size.AclBytesInUse as usize) }
            .to_vec(),
    )
}
fn path_wide(path: &Path) -> Result<Vec<u16>> {
    ensure!(
        path.as_os_str().as_encoded_bytes().len() <= 4096,
        "Schedule endpoint path is too long"
    );
    let mut wide = path.as_os_str().encode_wide().collect::<Vec<_>>();
    ensure!(
        !wide.is_empty() && !wide.contains(&0),
        "Invalid scheduler endpoint path"
    );
    wide.push(0);
    Ok(wide)
}
fn validate_inner(file: &File, user: &User, expected: &Descriptor) -> Result<u64> {
    let mut info: BY_HANDLE_FILE_INFORMATION = unsafe { std::mem::zeroed() };
    ensure!(
        unsafe { GetFileInformationByHandle(file.as_raw_handle(), &mut info) } != 0
            && info.dwFileAttributes & (FILE_ATTRIBUTE_REPARSE_POINT | FILE_ATTRIBUTE_DIRECTORY)
                == 0
            && info.nNumberOfLinks == 1
            && info.nFileSizeHigh == 0
            && info.nFileSizeLow <= 32 * 1024 * 1024 + 65536,
        "Schedule endpoint must be a private regular file within 32 MiB"
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
        "Could not read scheduler endpoint permissions"
    );
    let actual = Descriptor(actual);
    let (mut control, mut revision) = (0, 0);
    ensure!(
        !owner.is_null() && unsafe { EqualSid(owner, user.sid) } != 0,
        "Schedule endpoint belongs to another user"
    );
    // SQLite creates rollback sidecars with the private parent's inherited user ACE.
    // Admit only that inheritance marker, preserving the exact owner, mask and SID.
    let mut actual_acl = acl_bytes(actual.0)?;
    let inherited = actual_acl
        .get(9)
        .is_some_and(|flags| flags & INHERITED_ACE as u8 != 0);
    if let Some(flags) = actual_acl.get_mut(9) {
        *flags &= !(INHERITED_ACE as u8);
    }
    ensure!(
        unsafe { GetSecurityDescriptorControl(actual.0, &mut control, &mut revision) } != 0
            && (control & SE_DACL_PROTECTED != 0 || inherited)
            && actual_acl == acl_bytes(expected.0)?,
        "Schedule file permissions are not current-user-only"
    );
    Ok(u64::from(info.nFileSizeLow))
}

pub(super) fn validate(file: &File) -> Result<()> {
    let user = User::current()?;
    let descriptor = Descriptor::new(&user)?;
    validate_inner(file, &user, &descriptor)?;
    Ok(())
}
pub(super) fn open(path: &Path, create: bool) -> Result<File> {
    open_access(path, create, true)
}
pub(super) fn open_access(path: &Path, create: bool, writable: bool) -> Result<File> {
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
            GENERIC_READ | READ_CONTROL | if writable { GENERIC_WRITE | DELETE } else { 0 },
            FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE,
            if create {
                &attributes
            } else {
                std::ptr::null()
            },
            if create { CREATE_NEW } else { OPEN_EXISTING },
            FILE_ATTRIBUTE_NORMAL | FILE_FLAG_OPEN_REPARSE_POINT,
            std::ptr::null_mut(),
        )
    };
    if raw == INVALID_HANDLE_VALUE {
        return Err(std::io::Error::last_os_error().into());
    }
    let file = unsafe { File::from_raw_handle(raw) };
    validate_inner(&file, &user, &descriptor)?;
    Ok(file)
}
/// Protect SQLite sidecars through a private inheritable parent DACL.
pub(super) fn directory(path: &Path, create: bool) -> Result<()> {
    let user = User::current()?;
    let sddl: Vec<u16> = format!("O:{}D:P(A;OICI;FA;;;{})", user.text, user.text)
        .encode_utf16()
        .chain(Some(0))
        .collect();
    let mut raw_descriptor = std::ptr::null_mut();
    ensure!(
        unsafe {
            ConvertStringSecurityDescriptorToSecurityDescriptorW(
                sddl.as_ptr(),
                SDDL_REVISION_1,
                &mut raw_descriptor,
                std::ptr::null_mut(),
            )
        } != 0,
        "Could not prepare private schedule directory"
    );
    let expected = Descriptor(raw_descriptor);
    let path = path_wide(path)?;
    if create {
        let attributes = SECURITY_ATTRIBUTES {
            nLength: std::mem::size_of::<SECURITY_ATTRIBUTES>() as u32,
            lpSecurityDescriptor: expected.0,
            bInheritHandle: 0,
        };
        ensure!(
            unsafe { CreateDirectoryW(path.as_ptr(), &attributes) } != 0,
            "Could not create private schedule directory"
        );
    }
    let raw = unsafe {
        CreateFileW(
            path.as_ptr(),
            GENERIC_READ | READ_CONTROL,
            FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE,
            std::ptr::null(),
            OPEN_EXISTING,
            FILE_FLAG_BACKUP_SEMANTICS | FILE_FLAG_OPEN_REPARSE_POINT,
            std::ptr::null_mut(),
        )
    };
    ensure!(
        raw != INVALID_HANDLE_VALUE,
        "Could not open private schedule directory"
    );
    let file = unsafe { File::from_raw_handle(raw) };
    let mut info: BY_HANDLE_FILE_INFORMATION = unsafe { std::mem::zeroed() };
    ensure!(
        unsafe { GetFileInformationByHandle(file.as_raw_handle(), &mut info) } != 0
            && info.dwFileAttributes & FILE_ATTRIBUTE_REPARSE_POINT == 0
            && info.dwFileAttributes & FILE_ATTRIBUTE_DIRECTORY != 0,
        "Schedule directory must be a regular directory"
    );
    let (mut owner, mut descriptor) = (std::ptr::null_mut(), std::ptr::null_mut());
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
                &mut descriptor,
            )
        } == ERROR_SUCCESS,
        "Could not inspect schedule directory permissions"
    );
    let actual = Descriptor(descriptor);
    ensure!(
        !owner.is_null()
            && unsafe { EqualSid(owner, user.sid) } != 0
            && acl_bytes(actual.0)? == acl_bytes(expected.0)?,
        "Schedule directory must have a current-user-only inheritable DACL; use a new dedicated parent directory"
    );
    Ok(())
}
