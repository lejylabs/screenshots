use anyhow::{Result, ensure};
use zeroize::Zeroizing;

// Native calls are operation-scoped; no runtime, cache, watcher or idle connection.
#[cfg(target_os = "linux")]
mod platform {
    use super::*;
    use dbus_secret_service::{EncryptionType, SecretService};
    use std::collections::HashMap;

    fn validate_bus_address(address: &str) -> Result<()> {
        ensure!(
            !address.is_empty()
                && address.len() <= 4096
                && !address.chars().any(char::is_control)
                && address.split(';').count() <= 4
                && address.split(';').all(|entry| entry.starts_with("unix:")),
            "Upload credentials require a local Unix D-Bus session address"
        );
        Ok(())
    }

    fn connect() -> Result<SecretService> {
        let address = std::env::var("DBUS_SESSION_BUS_ADDRESS").map_err(|_| {
            anyhow::anyhow!(
                "Upload credentials require an explicit local Unix D-Bus session address"
            )
        })?;
        validate_bus_address(&address)?;
        SecretService::connect_with_max_prompt_timeout(EncryptionType::Plain, 0)
            .map_err(|_| anyhow::anyhow!("Secret Service is unavailable; unlock the desktop keyring before using upload credentials"))
    }

    fn attributes(target: &str) -> HashMap<&str, &str> {
        HashMap::from([("application", "me.lejy.screenshots"), ("target", target)])
    }

    fn find<'a>(
        service: &'a SecretService,
        target: &str,
    ) -> Result<Option<dbus_secret_service::Item<'a>>> {
        let mut matches = service.search_items(attributes(target)).map_err(|_| {
            anyhow::anyhow!("Could not search upload credentials in Secret Service")
        })?;
        ensure!(
            matches.locked.is_empty(),
            "Upload credential is locked; unlock the desktop keyring first"
        );
        ensure!(
            matches.unlocked.len() <= 1,
            "Multiple upload credentials match this target; remove duplicate entries in the keyring manager"
        );
        Ok(matches.unlocked.pop())
    }

    pub(super) fn read(target: &str) -> Result<Zeroizing<Vec<u8>>> {
        let service = connect()?;
        let item = find(&service, target)?.ok_or_else(|| {
            anyhow::anyhow!("No upload credential exists for this HTTPS origin/reference")
        })?;
        let secret = Zeroizing::new(item.get_secret().map_err(|_| {
            anyhow::anyhow!("Could not read the upload credential; keyring interaction is disabled")
        })?);
        ensure!(
            secret.len() <= super::super::MAX_PAYLOAD_BYTES,
            "Stored upload credential exceeds its storage limit"
        );
        Ok(secret)
    }

    pub(super) fn write(target: &str, payload: &[u8]) -> Result<()> {
        let service = connect()?;
        if let Some(item) = find(&service, target)? {
            item.set_secret(payload, "application/octet-stream")
                .map_err(|_| {
                    anyhow::anyhow!(
                        "Could not update the upload credential; keyring interaction is disabled"
                    )
                })?;
        } else {
            let collection = service.get_default_collection().map_err(|_| {
                anyhow::anyhow!("No default Secret Service collection is available")
            })?;
            ensure!(
                !collection
                    .is_locked()
                    .map_err(|_| anyhow::anyhow!("Could not check the desktop keyring"))?,
                "Default keyring is locked; unlock it before saving credentials"
            );
            collection
                .create_item(
                    "screenshots upload credential",
                    attributes(target),
                    payload,
                    false,
                    "application/octet-stream",
                )
                .map_err(|_| {
                    anyhow::anyhow!(
                        "Could not save the upload credential; keyring interaction is disabled"
                    )
                })?;
        }
        Ok(())
    }

    pub(super) fn remove(target: &str) -> Result<()> {
        let service = connect()?;
        if let Some(item) = find(&service, target)? {
            item.delete().map_err(|_| {
                anyhow::anyhow!(
                    "Could not remove the upload credential; keyring interaction is disabled"
                )
            })?;
        }
        Ok(())
    }

    #[cfg(test)]
    mod tests {
        use super::*;

        #[test]
        fn plain_sessions_only_admit_bounded_local_unix_addresses() {
            for address in [
                "unix:path=/run/user/1000/bus",
                "unix:abstract=/tmp/dbus-synthetic,guid=abc",
                "unix:runtime=yes",
            ] {
                assert!(validate_bus_address(address).is_ok());
            }
            for address in [
                "",
                "tcp:host=127.0.0.1,port=1234",
                "nonce-tcp:host=localhost",
                "autolaunch:",
                "unix:path=/tmp/bus;tcp:host=example.invalid",
                "unix:path=/tmp/bus\n",
                "unix:path=/tmp/bus;",
                &format!("unix:path={}", "x".repeat(4096)),
            ] {
                assert!(validate_bus_address(address).is_err());
            }
        }
    }
}

#[cfg(target_os = "macos")]
mod platform {
    use super::*;
    use core_foundation::{
        base::TCFType,
        string::{CFString, CFStringRef},
    };
    use security_framework::passwords::{self, PasswordOptions};
    use security_framework_sys::base::errSecItemNotFound;
    use security_framework_sys::item::kSecUseAuthenticationUI;

    unsafe extern "C" {
        // Security.framework exports this constant; the pinned sys crate omits it.
        static kSecUseAuthenticationUIFail: CFStringRef;
    }

    #[allow(deprecated)]
    fn options(target: &str) -> PasswordOptions {
        let mut options =
            PasswordOptions::new_generic_password("me.lejy.screenshots.upload", target);
        options.set_access_synchronized(Some(false));
        options.use_protected_keychain();
        // Per-request policy avoids process-wide changes to Keychain interaction.
        unsafe {
            options.query.push((
                CFString::wrap_under_get_rule(kSecUseAuthenticationUI),
                CFString::wrap_under_get_rule(kSecUseAuthenticationUIFail).into_CFType(),
            ));
        }
        options
    }

    pub(super) fn read(target: &str) -> Result<Zeroizing<Vec<u8>>> {
        let payload = Zeroizing::new(passwords::generic_password(options(target))
            .map_err(|_| anyhow::anyhow!("No readable upload credential in Keychain; unlock the login keychain and grant access before uploading"))?);
        ensure!(
            payload.len() <= super::super::MAX_PAYLOAD_BYTES,
            "Stored upload credential exceeds its storage limit"
        );
        Ok(payload)
    }

    pub(super) fn write(target: &str, payload: &[u8]) -> Result<()> {
        passwords::set_generic_password_options(payload, options(target))
            .map_err(|_| anyhow::anyhow!("Could not save upload credential in Keychain; interactive authorization is disabled"))
    }

    pub(super) fn remove(target: &str) -> Result<()> {
        match passwords::delete_generic_password_options(options(target)) {
            Ok(()) => Ok(()),
            Err(error) if error.code() == errSecItemNotFound => Ok(()),
            Err(_) => anyhow::bail!(
                "Could not remove upload credential from Keychain; interactive authorization is disabled"
            ),
        }
    }
}

pub(super) fn read(target: &str) -> Result<Zeroizing<Vec<u8>>> {
    platform::read(target)
}
pub(super) fn write(target: &str, payload: &[u8]) -> Result<()> {
    platform::write(target, payload)
}
pub(super) fn remove(target: &str) -> Result<()> {
    platform::remove(target)
}
