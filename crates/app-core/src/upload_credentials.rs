use anyhow::Result;
#[cfg(any(target_os = "windows", test))]
use anyhow::ensure;
#[cfg(any(target_os = "windows", test))]
use zeroize::Zeroizing;

#[cfg(any(target_os = "windows", test))]
fn bearer_header(token: &str) -> Result<reqwest::header::HeaderValue> {
    ensure!(
        !token.is_empty()
            && token.len() <= 2560
            && token
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || b"-._~+/=".contains(&byte)),
        "Stored upload credential is not a valid bounded Bearer token"
    );
    let text = Zeroizing::new(format!("Bearer {token}"));
    let mut header = reqwest::header::HeaderValue::from_str(&text)?;
    header.set_sensitive(true);
    Ok(header)
}

#[cfg(not(target_os = "windows"))]
pub(crate) fn read_bearer(_: &str) -> Result<reqwest::header::HeaderValue> {
    anyhow::bail!("Upload credential lookup is not implemented on this platform")
}

#[cfg(target_os = "windows")]
pub(crate) fn read_bearer(target: &str) -> Result<reqwest::header::HeaderValue> {
    use windows_sys::Win32::Security::Credentials::*;
    use zeroize::Zeroize;
    struct Credential(*mut CREDENTIALW);
    impl Drop for Credential {
        fn drop(&mut self) {
            unsafe {
                let value = &*self.0;
                if !value.CredentialBlob.is_null()
                    && value.CredentialBlobSize <= CRED_MAX_CREDENTIAL_BLOB_SIZE
                {
                    std::slice::from_raw_parts_mut(
                        value.CredentialBlob,
                        value.CredentialBlobSize as usize,
                    )
                    .zeroize();
                }
                CredFree(self.0.cast());
            }
        }
    }
    let name: Vec<u16> = target.encode_utf16().chain(Some(0)).collect();
    let mut pointer = std::ptr::null_mut();
    if unsafe { CredReadW(name.as_ptr(), CRED_TYPE_GENERIC, 0, &mut pointer) } == 0 {
        anyhow::bail!(
            "No readable Windows upload credential for this HTTPS origin (OS error {})",
            std::io::Error::last_os_error()
                .raw_os_error()
                .unwrap_or_default()
        );
    }
    ensure!(!pointer.is_null(), "Windows returned no upload credential");
    let value = Credential(pointer);
    let credential = unsafe { &*value.0 };
    ensure!(
        !credential.CredentialBlob.is_null()
            && credential.CredentialBlobSize > 0
            && credential.CredentialBlobSize <= CRED_MAX_CREDENTIAL_BLOB_SIZE
            && credential.CredentialBlobSize % 2 == 0,
        "Stored upload credential must be a UTF-16 password"
    );
    let bytes = unsafe {
        std::slice::from_raw_parts(
            credential.CredentialBlob,
            credential.CredentialBlobSize as usize,
        )
    };
    let mut units = Zeroizing::new(
        bytes
            .chunks_exact(2)
            .map(|pair| u16::from_le_bytes([pair[0], pair[1]]))
            .collect::<Vec<_>>(),
    );
    if units.last() == Some(&0) {
        units.pop();
    }
    let token = Zeroizing::new(
        String::from_utf16(&units)
            .map_err(|_| anyhow::anyhow!("Stored upload credential is not valid UTF-16"))?,
    );
    bearer_header(&token)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn bearer_tokens_reject_injection_and_are_sensitive() {
        for token in [
            "",
            "bad\r\nInjected",
            "two words",
            "\0",
            "ü",
            &"x".repeat(2561),
        ] {
            assert!(bearer_header(token).is_err());
        }
        let value = bearer_header("synthetic-token_123.abc==").unwrap();
        assert!(value.is_sensitive());
        assert_eq!(value.to_str().unwrap(), "Bearer synthetic-token_123.abc==");
    }
    #[cfg(target_os = "windows")]
    #[test]
    #[ignore = "creates and deletes one uniquely named synthetic Windows credential; no real secrets or networking"]
    fn windows_credential_round_trip_is_scoped_and_removed() {
        use windows_sys::Win32::Security::Credentials::*;
        let target = format!(
            "Screenshots/upload/https://example.invalid/test-{}",
            uuid::Uuid::new_v4()
        );
        let mut name: Vec<u16> = target.encode_utf16().chain(Some(0)).collect();
        struct Cleanup(Vec<u16>);
        impl Drop for Cleanup {
            fn drop(&mut self) {
                unsafe {
                    CredDeleteW(self.0.as_ptr(), CRED_TYPE_GENERIC, 0);
                }
            }
        }
        let cleanup = Cleanup(name.clone());
        let mut password: Vec<u16> = "synthetic-upload-token".encode_utf16().collect();
        let credential = CREDENTIALW {
            Type: CRED_TYPE_GENERIC,
            TargetName: name.as_mut_ptr(),
            CredentialBlobSize: (password.len() * 2) as u32,
            CredentialBlob: password.as_mut_ptr().cast(),
            Persist: CRED_PERSIST_SESSION,
            ..Default::default()
        };
        assert_ne!(unsafe { CredWriteW(&credential, 0) }, 0);
        let started = std::time::Instant::now();
        assert_eq!(
            read_bearer(&target).unwrap().to_str().unwrap(),
            "Bearer synthetic-upload-token"
        );
        eprintln!(
            "Synthetic Windows credential lookup and Bearer construction: {:?}",
            started.elapsed()
        );
        assert!(read_bearer(&target.replace("example.invalid", "other.invalid")).is_err());
        drop(cleanup);
        assert!(read_bearer(&target).is_err());
    }
}
