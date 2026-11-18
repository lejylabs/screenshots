//! One explicitly installed owner for all Unix credential stores; no inline fallback.
use anyhow::{Result, ensure};
use std::sync::OnceLock;
use zeroize::Zeroizing;

const MAX_PAYLOAD_BYTES: usize = 8192;
#[path = "clipboard_input_budget.rs"]
mod budget;
#[path = "upload_credentials_native.rs"]
mod native;

// Deliberately neither Debug nor Serialize: payloads only enter the private binary channel.
#[doc(hidden)]
pub enum NativeCredentialCall {
    Read {
        target: String,
    },
    Write {
        target: String,
        payload: Zeroizing<Vec<u8>>,
    },
    Remove {
        target: String,
    },
}
impl NativeCredentialCall {
    pub fn target(&self) -> &str {
        match self {
            Self::Read { target } | Self::Write { target, .. } | Self::Remove { target } => target,
        }
    }
    pub fn payload_limit(&self) -> usize {
        if self.target().starts_with("Screenshots/provider/") {
            2560
        } else if self.target().starts_with("Screenshots/s3/") {
            4096
        } else {
            MAX_PAYLOAD_BYTES
        }
    }
    pub fn validate(&self) -> Result<()> {
        let target = self.target();
        if target.starts_with("Screenshots/provider/") {
            crate::provider_credentials::validate_target(target)?;
        } else if target.starts_with("Screenshots/s3/") {
            crate::s3_credentials::validate_target(target)?;
        } else {
            crate::upload_credentials::validate_managed_target(target)?;
        }
        if let Self::Write { payload, .. } = self {
            ensure!(
                !payload.is_empty() && payload.len() <= self.payload_limit(),
                "Invalid native credential payload size"
            );
        }
        Ok(())
    }
    /// Only the authenticated child dispatcher invokes this. Never a parent fallback.
    pub fn run_in_owned_child(self) -> Result<Zeroizing<Vec<u8>>> {
        ensure!(
            EXECUTOR.get().is_none(),
            "Native credential access requires the private child"
        );
        self.validate()?;
        let _resource = budget::Budget::enter()?;
        match self {
            Self::Read { target } => native::read(&target),
            Self::Write { target, payload } => {
                native::write(&target, &payload)?;
                Ok(Zeroizing::new(Vec::new()))
            }
            Self::Remove { target } => {
                native::remove(&target)?;
                Ok(Zeroizing::new(Vec::new()))
            }
        }
    }
}
#[doc(hidden)]
pub type NativeCredentialExecutor =
    fn(NativeCredentialCall, &crate::NativeCaptureCheck<'_>) -> Result<Zeroizing<Vec<u8>>>;
static EXECUTOR: OnceLock<NativeCredentialExecutor> = OnceLock::new();
#[doc(hidden)]
pub fn install_native_credential_executor(executor: NativeCredentialExecutor) -> Result<()> {
    EXECUTOR
        .set(executor)
        .map_err(|_| anyhow::anyhow!("Native credential owner already installed"))
}
#[doc(hidden)]
pub fn execute(
    call: NativeCredentialCall,
    check: &crate::NativeCaptureCheck<'_>,
) -> Result<Zeroizing<Vec<u8>>> {
    call.validate()?;
    check()?;
    let mutation = !matches!(call, NativeCredentialCall::Read { .. });
    let limit = call.payload_limit();
    let owner = EXECUTOR
        .get()
        .ok_or_else(|| anyhow::anyhow!("Native credential owner is unavailable"))?;
    // Never forward provider errors, target names or payloads to logs/status.
    (|| -> Result<Zeroizing<Vec<u8>>> {
        let result = owner(call, check)?;
        check()?;
        ensure!(result.len() <= limit && (mutation || !result.is_empty()), "Invalid native credential result size");
        ensure!(!mutation || result.is_empty(), "Unexpected native credential mutation payload");
        Ok(result)
    })().map_err(|_| if mutation {
        anyhow::anyhow!("Native credential change was not confirmed; it may already have applied. Check the keyring before retrying")
    } else {
        anyhow::anyhow!("Native credential read failed, was canceled, or exceeded its deadline; no credential was returned")
    })
}

pub(crate) fn read(target: &str) -> Result<Zeroizing<Vec<u8>>> {
    execute(
        NativeCredentialCall::Read {
            target: target.into(),
        },
        &|| Ok(()),
    )
}
pub(crate) fn write(target: &str, payload: &[u8]) -> Result<()> {
    execute(
        NativeCredentialCall::Write {
            target: target.into(),
            payload: Zeroizing::new(payload.to_vec()),
        },
        &|| Ok(()),
    )
    .map(|_| ())
}
pub(crate) fn remove(target: &str) -> Result<()> {
    execute(
        NativeCredentialCall::Remove {
            target: target.into(),
        },
        &|| Ok(()),
    )
    .map(|_| ())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn no_installed_owner_fails_closed_without_native_access() {
        let target = "Screenshots/upload/https://example.invalid/private_fixture";
        let error = read(target).unwrap_err().to_string();
        assert_eq!(error, "Native credential owner is unavailable");
        assert!(!error.contains(target));
        let call = NativeCredentialCall::Write {
            target: target.into(),
            payload: Zeroizing::new(vec![0; MAX_PAYLOAD_BYTES + 1]),
        };
        assert!(call.validate().is_err());
        assert!(
            NativeCredentialCall::Remove {
                target: "unrelated-keyring-entry".into()
            }
            .validate()
            .is_err()
        );
    }
}
