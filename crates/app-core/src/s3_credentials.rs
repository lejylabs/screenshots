//! Operation-scoped native storage for opaque, separately validated S3 credentials.

use anyhow::{Result, ensure};
use zeroize::Zeroizing;

const MAX_PAYLOAD_BYTES: usize = 4096;

fn validate_target(target: &str) -> Result<()> {
    ensure!(
        target.len() <= 2304 && !target.chars().any(char::is_control),
        "S3 credential target is invalid"
    );
    let remainder = target
        .strip_prefix("Screenshots/s3/")
        .ok_or_else(|| anyhow::anyhow!("S3 credential target is invalid"))?;
    let (origin_bucket, reference) = remainder
        .rsplit_once('/')
        .ok_or_else(|| anyhow::anyhow!("S3 credential target is invalid"))?;
    let (origin, bucket) = origin_bucket
        .rsplit_once('/')
        .ok_or_else(|| anyhow::anyhow!("S3 credential target is invalid"))?;
    ensure!(
        !reference.is_empty()
            && reference.len() <= 64
            && reference
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || b"_-".contains(&byte)),
        "S3 credential reference is invalid"
    );
    ensure!(
        (3..=63).contains(&bucket.len())
            && bucket.bytes().all(|byte| byte.is_ascii_lowercase()
                || byte.is_ascii_digit()
                || b".-".contains(&byte))
            && bucket.split('.').all(|part| !part.is_empty()
                && part
                    .as_bytes()
                    .first()
                    .is_some_and(u8::is_ascii_alphanumeric)
                && part
                    .as_bytes()
                    .last()
                    .is_some_and(u8::is_ascii_alphanumeric))
            && bucket.parse::<std::net::Ipv4Addr>().is_err(),
        "S3 credential bucket is invalid"
    );
    ensure!(origin.len() <= 2048, "S3 credential origin is invalid");
    let url = reqwest::Url::parse(origin)
        .map_err(|_| anyhow::anyhow!("S3 credential origin is invalid"))?;
    ensure!(
        url.scheme() == "https"
            && url.host_str().is_some()
            && url.username().is_empty()
            && url.password().is_none()
            && url.query().is_none()
            && url.fragment().is_none()
            && url.origin().ascii_serialization() == origin,
        "S3 credential target requires a canonical HTTPS origin"
    );
    Ok(())
}

fn validate_payload(payload: &[u8]) -> Result<()> {
    ensure!(
        !payload.is_empty() && payload.len() <= MAX_PAYLOAD_BYTES,
        "S3 credential payload must contain 1–4096 bytes"
    );
    Ok(())
}

#[cfg(any(target_os = "linux", target_os = "macos"))]
#[path = "upload_credentials_native.rs"]
mod native;

/// Save opaque credential bytes after the caller has validated their versioned format.
pub fn save_s3_credentials(target: &str, payload: &[u8]) -> Result<()> {
    validate_target(target)?;
    validate_payload(payload)?;
    platform::write(target, payload)
}

/// Read one scoped credential payload; the returned allocation is wiped on drop.
pub fn read_s3_credentials(target: &str) -> Result<Zeroizing<Vec<u8>>> {
    validate_target(target)?;
    let payload = platform::read(target)?;
    validate_payload(&payload)?;
    Ok(payload)
}

/// Remove this exact scoped entry; a missing entry is already removed.
pub fn remove_s3_credentials(target: &str) -> Result<()> {
    validate_target(target)?;
    platform::remove(target)
}

#[cfg(any(target_os = "linux", target_os = "macos"))]
use native as platform;

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
            "No readable Windows S3 credential for this target"
        );
        ensure!(!pointer.is_null(), "Windows returned no S3 credential");
        let guard = Credential(pointer);
        let credential = unsafe { &*guard.0 };
        ensure!(
            !credential.CredentialBlob.is_null()
                && credential.CredentialBlobSize > 0
                && credential.CredentialBlobSize <= CRED_MAX_CREDENTIAL_BLOB_SIZE
                && credential.CredentialBlobSize as usize <= MAX_PAYLOAD_BYTES,
            "Stored Windows S3 credential exceeds its raw payload limit"
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
            "S3 credential exceeds the Windows 2560-byte raw payload limit"
        );
        let mut name: Vec<u16> = target.encode_utf16().chain(Some(0)).collect();
        let mut payload = Zeroizing::new(payload.to_vec());
        let mut username: Vec<u16> = "s3".encode_utf16().chain(Some(0)).collect();
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
        ensure!(result != 0, "Could not save the Windows S3 credential");
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
        anyhow::bail!("Could not remove the Windows S3 credential")
    }
}

#[cfg(not(any(target_os = "windows", target_os = "linux", target_os = "macos")))]
mod platform {
    use super::*;
    pub(super) fn read(_: &str) -> Result<Zeroizing<Vec<u8>>> {
        anyhow::bail!("Native S3 credentials are unsupported on this platform")
    }
    pub(super) fn write(_: &str, _: &[u8]) -> Result<()> {
        anyhow::bail!("Native S3 credentials are unsupported on this platform")
    }
    pub(super) fn remove(_: &str) -> Result<()> {
        anyhow::bail!("Native S3 credentials are unsupported on this platform")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn targets_are_strictly_scoped_to_origin_bucket_and_reference() {
        for target in [
            "Screenshots/s3/https://example.invalid/bucket-name/ref_1",
            "Screenshots/s3/https://127.0.0.1:8443/bucket.example/ref-1",
            "Screenshots/s3/https://[::1]:8443/bucket/ref",
        ] {
            validate_target(target).unwrap();
        }
        for target in [
            "Screenshots/upload/https://example.invalid/bucket/ref",
            "Screenshots/s3/http://example.invalid/bucket/ref",
            "Screenshots/s3/https://example.invalid:443/bucket/ref",
            "Screenshots/s3/https://EXAMPLE.invalid/bucket/ref",
            "Screenshots/s3/https://example.invalid/path/bucket/ref",
            "Screenshots/s3/https://user@example.invalid/bucket/ref",
            "Screenshots/s3/https://example.invalid?query/bucket/ref",
            "Screenshots/s3/https://example.invalid/Bucket/ref",
            "Screenshots/s3/https://example.invalid/ab/ref",
            "Screenshots/s3/https://example.invalid/bucket..name/ref",
            "Screenshots/s3/https://example.invalid/bucket.-name/ref",
            "Screenshots/s3/https://example.invalid/192.168.1.1/ref",
            "Screenshots/s3/https://example.invalid/bucket/",
            "Screenshots/s3/https://example.invalid/bucket/ref\n",
            "Screenshots/s3/https://example.invalid/bucket/ref/path",
        ] {
            assert!(validate_target(target).is_err(), "{target}");
        }
        assert!(
            validate_target(&format!(
                "Screenshots/s3/https://example.invalid/bucket/{}",
                "r".repeat(65)
            ))
            .is_err()
        );
    }

    #[test]
    fn opaque_payload_admission_precedes_any_native_access() {
        assert!(validate_payload(&[]).is_err());
        assert!(validate_payload(&vec![0; MAX_PAYLOAD_BYTES + 1]).is_err());
        validate_payload(&[0, 255, 0]).unwrap();
        validate_payload(&vec![0; MAX_PAYLOAD_BYTES]).unwrap();
        let target = "Screenshots/s3/https://example.invalid/bucket/ref";
        assert!(save_s3_credentials(target, &[]).is_err());
        assert!(save_s3_credentials(target, &vec![0; MAX_PAYLOAD_BYTES + 1]).is_err());
        assert!(read_s3_credentials("not-an-s3-target").is_err());
        assert!(remove_s3_credentials("not-an-s3-target").is_err());
    }

    #[cfg(target_os = "linux")]
    #[test]
    #[ignore = "writes one unique synthetic native keyring entry; requires an unlocked local Secret Service"]
    fn native_s3_roundtrip_is_raw_and_separate_from_upload_credentials() {
        struct Cleanup(String);
        impl Drop for Cleanup {
            fn drop(&mut self) {
                let _ = remove_s3_credentials(&self.0);
            }
        }
        let target = format!(
            "Screenshots/s3/https://example.invalid/synthetic-bucket/test-{}",
            uuid::Uuid::new_v4()
        );
        let _cleanup = Cleanup(target.clone());
        let payload = b"synthetic\0raw\xffpayload";
        save_s3_credentials(&target, payload).unwrap();
        assert_eq!(read_s3_credentials(&target).unwrap().as_slice(), payload);
        remove_s3_credentials(&target).unwrap();
        remove_s3_credentials(&target).unwrap();
        assert!(read_s3_credentials(&target).is_err());
    }
}
