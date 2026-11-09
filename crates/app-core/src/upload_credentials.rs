use crate::upload::UploadAuthentication;
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
pub(crate) fn read_header(
    _: &str,
    _: UploadAuthentication,
) -> Result<reqwest::header::HeaderValue> {
    anyhow::bail!("Upload credential lookup is not implemented on this platform")
}

#[cfg(target_os = "windows")]
pub(crate) fn read_header(
    target: &str,
    authentication: UploadAuthentication,
) -> Result<reqwest::header::HeaderValue> {
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
    if matches!(authentication, UploadAuthentication::Basic) {
        ensure!(
            !credential.UserName.is_null(),
            "Basic authentication requires a stored username"
        );
        let mut length = 0;
        // CredReadW owns this NUL-terminated native string until the guard frees it.
        while length <= CRED_MAX_USERNAME_LENGTH as usize
            && unsafe { *credential.UserName.add(length) } != 0
        {
            length += 1;
        }
        ensure!(
            length > 0 && length <= CRED_MAX_USERNAME_LENGTH as usize,
            "Stored Basic username is empty or too long"
        );
        let username = Zeroizing::new(
            String::from_utf16(unsafe { std::slice::from_raw_parts(credential.UserName, length) })
                .map_err(|_| anyhow::anyhow!("Stored Basic username is not valid UTF-16"))?,
        );
        return basic_header(&username, &token);
    }
    credential_header(&token, authentication)
}

#[cfg(any(target_os = "windows", test))]
fn basic_header(username: &str, password: &str) -> Result<reqwest::header::HeaderValue> {
    use base64::Engine;
    ensure!(
        !username.is_empty() && !username.contains(':') && !username.chars().any(char::is_control),
        "Basic username must be nonempty and contain no colon or control characters"
    );
    ensure!(
        !password.is_empty() && !password.chars().any(char::is_control),
        "Basic password must be nonempty and contain no control characters"
    );
    ensure!(
        username
            .len()
            .saturating_add(password.len())
            .saturating_add(1)
            <= 4096,
        "Combined Basic credentials exceed 4096 UTF-8 bytes"
    );
    let plain = Zeroizing::new(format!("{username}:{password}"));
    let encoded =
        Zeroizing::new(base64::engine::general_purpose::STANDARD.encode(plain.as_bytes()));
    let text = Zeroizing::new(format!("Basic {}", encoded.as_str()));
    let mut header = reqwest::header::HeaderValue::from_str(&text)?;
    header.set_sensitive(true);
    Ok(header)
}

#[cfg(any(target_os = "windows", test))]
fn credential_header(
    token: &str,
    authentication: UploadAuthentication,
) -> Result<reqwest::header::HeaderValue> {
    if matches!(authentication, UploadAuthentication::Bearer) {
        return bearer_header(token);
    }
    ensure!(
        matches!(authentication, UploadAuthentication::ApiKey),
        "Basic authentication requires a username and password"
    );
    ensure!(
        !token.is_empty()
            && token.len() <= 2560
            && token.bytes().all(|byte| (0x21..=0x7e).contains(&byte)),
        "Stored API key must be bounded printable ASCII without spaces or controls"
    );
    let mut header = reqwest::header::HeaderValue::from_str(token)?;
    header.set_sensitive(true);
    Ok(header)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn basic_credentials_encode_unicode_and_keep_header_sensitive() {
        use base64::Engine;
        assert_eq!(UploadAuthentication::Basic.header_name(), "authorization");
        let header = basic_header("upload-user", "päss word:123").unwrap();
        assert!(header.is_sensitive());
        let encoded = header.to_str().unwrap().strip_prefix("Basic ").unwrap();
        assert_eq!(
            base64::engine::general_purpose::STANDARD
                .decode(encoded)
                .unwrap(),
            "upload-user:päss word:123".as_bytes()
        );
        for (user, password) in [
            ("", "password"),
            ("user:name", "password"),
            ("user\nname", "password"),
            ("user", ""),
            ("user", "bad\r\npassword"),
            ("user", &"x".repeat(4096)),
        ] {
            assert!(basic_header(user, password).is_err());
        }
    }

    #[test]
    fn api_keys_are_unprefixed_sensitive_and_reject_injection() {
        assert_eq!(UploadAuthentication::ApiKey.header_name(), "x-api-key");
        let header = credential_header("synthetic:key@v1!", UploadAuthentication::ApiKey).unwrap();
        assert_eq!(header.to_str().unwrap(), "synthetic:key@v1!");
        assert!(header.is_sensitive());
        for value in [
            "",
            "two words",
            "key\r\nx-extra: value",
            "ü",
            &"x".repeat(2561),
        ] {
            assert!(credential_header(value, UploadAuthentication::ApiKey).is_err());
        }
        assert!(credential_header("synthetic:key@v1!", UploadAuthentication::Bearer).is_err());
    }

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
        let mut username: Vec<u16> = "upload-user".encode_utf16().chain(Some(0)).collect();
        let credential = CREDENTIALW {
            Type: CRED_TYPE_GENERIC,
            TargetName: name.as_mut_ptr(),
            CredentialBlobSize: (password.len() * 2) as u32,
            CredentialBlob: password.as_mut_ptr().cast(),
            Persist: CRED_PERSIST_SESSION,
            UserName: username.as_mut_ptr(),
            ..Default::default()
        };
        assert_ne!(unsafe { CredWriteW(&credential, 0) }, 0);
        let started = std::time::Instant::now();
        assert_eq!(
            read_header(&target, UploadAuthentication::Bearer)
                .unwrap()
                .to_str()
                .unwrap(),
            "Bearer synthetic-upload-token"
        );
        eprintln!(
            "Synthetic Windows credential lookup and Bearer construction: {:?}",
            started.elapsed()
        );
        let api_key = read_header(&target, UploadAuthentication::ApiKey).unwrap();
        assert_eq!(api_key.to_str().unwrap(), "synthetic-upload-token");
        assert!(api_key.is_sensitive());
        let started = std::time::Instant::now();
        let basic = read_header(&target, UploadAuthentication::Basic).unwrap();
        assert!(basic.is_sensitive());
        assert_eq!(
            basic.to_str().unwrap(),
            basic_header("upload-user", "synthetic-upload-token")
                .unwrap()
                .to_str()
                .unwrap()
        );
        eprintln!(
            "Synthetic Windows Basic credential lookup and construction: {:?}",
            started.elapsed()
        );
        assert!(
            read_header(
                &target.replace("example.invalid", "other.invalid"),
                UploadAuthentication::Bearer
            )
            .is_err()
        );
        drop(cleanup);
        assert!(read_header(&target, UploadAuthentication::Bearer).is_err());
    }
}
