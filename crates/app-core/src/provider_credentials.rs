//! Exact-scope opaque provider secrets, stored only by explicit operations.

use anyhow::{Result, ensure};
use zeroize::Zeroizing;

const MAX_PAYLOAD_BYTES: usize = 2560;

pub(crate) fn validate_target(target: &str) -> Result<()> {
    ensure!(
        target.len() <= 256 && !target.chars().any(char::is_control),
        "Invalid provider credential target"
    );
    let rest = target
        .strip_prefix("Screenshots/provider/")
        .ok_or_else(|| anyhow::anyhow!("Invalid provider credential target"))?;
    let parts = rest.split('/').collect::<Vec<_>>();
    ensure!(
        parts.len() == 3
            && matches!(parts[0], "sftp" | "ftps" | "ftp" | "oauth")
            && parts[1].len() == 64
            && parts[1]
                .bytes()
                .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
            && (1..=64).contains(&parts[2].len())
            && parts[2]
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || b"_-".contains(&byte)),
        "Invalid provider credential target"
    );
    Ok(())
}

fn validate_payload(payload: &[u8]) -> Result<()> {
    ensure!(
        (1..=MAX_PAYLOAD_BYTES).contains(&payload.len()),
        "Provider credential must contain 1–2560 bytes"
    );
    Ok(())
}

#[cfg(any(target_os = "linux", target_os = "macos"))]
use crate::native_credentials as native;
#[cfg(any(target_os = "linux", target_os = "macos"))]
use native as platform;

pub fn save_provider_credentials(target: &str, payload: &[u8]) -> Result<()> {
    validate_target(target)?;
    validate_payload(payload)?;
    platform::write(target, payload)
}

pub fn read_provider_credentials(target: &str) -> Result<Zeroizing<Vec<u8>>> {
    validate_target(target)?;
    let payload = platform::read(target)?;
    validate_payload(&payload)?;
    Ok(payload)
}

pub fn remove_provider_credentials(target: &str) -> Result<()> {
    validate_target(target)?;
    platform::remove(target)
}

#[cfg(target_os = "windows")]
mod platform {
    use super::*;
    use windows_sys::Win32::Security::Credentials::*;
    use zeroize::Zeroize;

    struct Credential(*mut CREDENTIALW);
    impl Drop for Credential {
        fn drop(&mut self) {
            unsafe {
                let credential = &*self.0;
                if !credential.CredentialBlob.is_null()
                    && credential.CredentialBlobSize <= CRED_MAX_CREDENTIAL_BLOB_SIZE
                {
                    std::slice::from_raw_parts_mut(
                        credential.CredentialBlob,
                        credential.CredentialBlobSize as usize,
                    )
                    .zeroize();
                }
                CredFree(self.0.cast());
            }
        }
    }

    pub(super) fn read(target: &str) -> Result<Zeroizing<Vec<u8>>> {
        let name: Vec<u16> = target.encode_utf16().chain(Some(0)).collect();
        let mut pointer = std::ptr::null_mut();
        ensure!(
            unsafe { CredReadW(name.as_ptr(), CRED_TYPE_GENERIC, 0, &mut pointer) } != 0,
            "No readable Windows provider credential for this target"
        );
        ensure!(
            !pointer.is_null(),
            "Windows returned no provider credential"
        );
        let guard = Credential(pointer);
        let credential = unsafe { &*guard.0 };
        ensure!(
            !credential.CredentialBlob.is_null()
                && (1..=MAX_PAYLOAD_BYTES).contains(&(credential.CredentialBlobSize as usize)),
            "Stored Windows provider credential exceeds its limit"
        );
        let bytes = unsafe {
            std::slice::from_raw_parts(
                credential.CredentialBlob,
                credential.CredentialBlobSize as usize,
            )
        };
        Ok(Zeroizing::new(bytes.to_vec()))
    }

    pub(super) fn write(target: &str, payload: &[u8]) -> Result<()> {
        ensure!(
            payload.len() <= CRED_MAX_CREDENTIAL_BLOB_SIZE as usize,
            "Provider credential exceeds the Windows storage limit"
        );
        let mut name: Vec<u16> = target.encode_utf16().chain(Some(0)).collect();
        let mut payload = Zeroizing::new(payload.to_vec());
        let mut username: Vec<u16> = "provider".encode_utf16().chain(Some(0)).collect();
        let credential = CREDENTIALW {
            Type: CRED_TYPE_GENERIC,
            TargetName: name.as_mut_ptr(),
            CredentialBlobSize: payload.len() as u32,
            CredentialBlob: payload.as_mut_ptr(),
            Persist: CRED_PERSIST_LOCAL_MACHINE,
            UserName: username.as_mut_ptr(),
            ..Default::default()
        };
        let result = unsafe { CredWriteW(&credential, 0) };
        payload.zeroize();
        ensure!(result != 0, "Could not save Windows provider credential");
        Ok(())
    }

    pub(super) fn remove(target: &str) -> Result<()> {
        let name: Vec<u16> = target.encode_utf16().chain(Some(0)).collect();
        if unsafe { CredDeleteW(name.as_ptr(), CRED_TYPE_GENERIC, 0) } != 0
            || std::io::Error::last_os_error().raw_os_error()
                == Some(windows_sys::Win32::Foundation::ERROR_NOT_FOUND as i32)
        {
            return Ok(());
        }
        anyhow::bail!("Could not remove Windows provider credential")
    }
}

#[cfg(not(any(target_os = "windows", target_os = "linux", target_os = "macos")))]
mod platform {
    use super::*;
    pub(super) fn read(_: &str) -> Result<Zeroizing<Vec<u8>>> {
        anyhow::bail!("Native provider credentials are unsupported on this platform")
    }
    pub(super) fn write(_: &str, _: &[u8]) -> Result<()> {
        anyhow::bail!("Native provider credentials are unsupported on this platform")
    }
    pub(super) fn remove(_: &str) -> Result<()> {
        anyhow::bail!("Native provider credentials are unsupported on this platform")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn targets_and_payloads_are_bounded_before_native_access() {
        let valid = format!("Screenshots/provider/sftp/{}/test_1", "0".repeat(64));
        validate_target(&valid).unwrap();
        for invalid in [
            valid.replace("sftp", "https"),
            valid.replace("test_1", "test/extra"),
            valid.replace("test_1", ""),
            valid.replace(&"0".repeat(64), &"A".repeat(64)),
            "Screenshots/upload/https://example.invalid/ref".into(),
        ] {
            assert!(read_provider_credentials(&invalid).is_err());
            assert!(save_provider_credentials(&invalid, b"test").is_err());
            assert!(remove_provider_credentials(&invalid).is_err());
        }
        assert!(save_provider_credentials(&valid, &[]).is_err());
        assert!(save_provider_credentials(&valid, &vec![0; MAX_PAYLOAD_BYTES + 1]).is_err());
        validate_payload(&vec![0; MAX_PAYLOAD_BYTES]).unwrap();
    }
}
