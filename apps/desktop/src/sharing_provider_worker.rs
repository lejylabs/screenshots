//! Private SFTP worker: raw libcurl is initialized only in this isolated child.
//!
//! The curl CLI cannot disable every Windows SSH agent backend. This worker
//! sets SSH_AUTH_TYPES to exactly PASSWORD or PUBLICKEY, never AGENT/KEYBOARD.

use super::sharing_provider::{Credentials, NativeSftpConfig};
use anyhow::{Context, Result, ensure};
use std::{
    ffi::{CStr, OsString},
    io::{Read, Write},
    os::raw::{c_char, c_long, c_void},
    sync::{Mutex, MutexGuard},
};
use zeroize::{Zeroize, Zeroizing};

const HOST_SHA256: curl_sys::CURLoption = curl_sys::CURLOPTTYPE_OBJECTPOINT + 311;
// CURLPROTO_SFTP from the public curl header (curl-sys does not expose it).
const SFTP_PROTOCOL: c_long = 1 << 5;
const AUTH_PUBLICKEY: c_long = 1;
const AUTH_PASSWORD: c_long = 2;
static GLOBAL: Mutex<()> = Mutex::new(());

struct Global {
    _lock: MutexGuard<'static, ()>,
}
impl Global {
    fn new() -> Result<Self> {
        let lock = GLOBAL
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        ensure!(
            unsafe { curl_sys::curl_global_init(curl_sys::CURL_GLOBAL_DEFAULT) }
                == curl_sys::CURLE_OK,
            "Native provider engine initialization failed"
        );
        Ok(Self { _lock: lock })
    }
}
impl Drop for Global {
    fn drop(&mut self) {
        unsafe {
            curl_sys::curl_global_cleanup();
        }
    }
}

struct Easy(*mut curl_sys::CURL);
impl Easy {
    fn new() -> Result<Self> {
        let value = unsafe { curl_sys::curl_easy_init() };
        ensure!(
            !value.is_null(),
            "Native provider engine handle allocation failed"
        );
        Ok(Self(value))
    }
    fn long(&self, option: curl_sys::CURLoption, value: c_long) -> Result<()> {
        ensure!(
            unsafe { curl_sys::curl_easy_setopt(self.0, option, value) } == curl_sys::CURLE_OK,
            "Native provider engine rejected a required option"
        );
        Ok(())
    }
    fn string(&self, option: curl_sys::CURLoption, value: &str) -> Result<()> {
        ensure!(
            !value.as_bytes().contains(&0),
            "Native provider option contains a null byte"
        );
        let mut bytes = Zeroizing::new(value.as_bytes().to_vec());
        bytes.push(0);
        // All string options used here are copied by libcurl (no POSTFIELDS).
        ensure!(
            unsafe { curl_sys::curl_easy_setopt(self.0, option, bytes.as_ptr().cast::<c_char>()) }
                == curl_sys::CURLE_OK,
            "Native provider engine rejected a required option"
        );
        Ok(())
    }
}
impl Drop for Easy {
    fn drop(&mut self) {
        unsafe {
            curl_sys::curl_easy_cleanup(self.0);
        }
    }
}

fn has_sftp() -> bool {
    let version = unsafe { curl_sys::curl_version_info(curl_sys::CURLVERSION_FIRST) };
    if version.is_null() {
        return false;
    }
    let protocols = unsafe { (*version).protocols };
    if protocols.is_null() {
        return false;
    }
    for index in 0..64 {
        let name = unsafe { *protocols.add(index) };
        if name.is_null() {
            return false;
        }
        if unsafe { CStr::from_ptr(name) }.to_bytes() == b"sftp" {
            return true;
        }
    }
    false
}

fn authentication(credentials: &Credentials) -> c_long {
    match credentials {
        Credentials::Password { .. } => AUTH_PASSWORD,
        Credentials::SshKey { .. } => AUTH_PUBLICKEY,
    }
}

struct State<'a> {
    input: &'a mut dyn Read,
    remaining: u64,
    received: usize,
    failed: bool,
}

extern "C" fn read(data: *mut c_char, size: usize, count: usize, state: *mut c_void) -> usize {
    if state.is_null() {
        return curl_sys::CURL_READFUNC_ABORT;
    }
    let state = unsafe { &mut *state.cast::<State<'_>>() };
    let Some(capacity) = size.checked_mul(count) else {
        state.failed = true;
        return curl_sys::CURL_READFUNC_ABORT;
    };
    if state.remaining == 0 || capacity == 0 {
        return 0;
    }
    if data.is_null() {
        state.failed = true;
        return curl_sys::CURL_READFUNC_ABORT;
    }
    let length = capacity.min(64 * 1024).min(state.remaining as usize);
    let buffer = unsafe { std::slice::from_raw_parts_mut(data.cast::<u8>(), length) };
    match state.input.read(buffer) {
        Ok(0) | Err(_) => {
            state.failed = true;
            curl_sys::CURL_READFUNC_ABORT
        }
        Ok(read) => {
            state.remaining -= read as u64;
            read
        }
    }
}

extern "C" fn discard(_: *mut c_char, size: usize, count: usize, state: *mut c_void) -> usize {
    if state.is_null() {
        return 0;
    }
    let state = unsafe { &mut *state.cast::<State<'_>>() };
    let Some(length) = size.checked_mul(count) else {
        state.failed = true;
        return 0;
    };
    let Some(total) = state
        .received
        .checked_add(length)
        .filter(|total| *total <= 4096)
    else {
        state.failed = true;
        return 0;
    };
    state.received = total;
    length
}

pub(super) fn transfer(config: &NativeSftpConfig, input: &mut dyn Read) -> Result<()> {
    let _global = Global::new()?;
    ensure!(
        has_sftp(),
        "Native provider library was built without required SFTP support"
    );
    let mut state = State {
        input,
        remaining: config.bytes,
        received: 0,
        failed: false,
    };
    let easy = Easy::new()?;
    easy.string(curl_sys::CURLOPT_URL, &config.url)?;
    easy.string(curl_sys::CURLOPT_PROXY, "")?;
    easy.string(curl_sys::CURLOPT_NOPROXY, "*")?;
    for (option, value) in [
        (curl_sys::CURLOPT_UPLOAD, 1),
        (curl_sys::CURLOPT_NOSIGNAL, 1),
        (curl_sys::CURLOPT_VERBOSE, 0),
        (curl_sys::CURLOPT_NETRC, 0),
        (curl_sys::CURLOPT_CONNECTTIMEOUT, 5),
        (curl_sys::CURLOPT_TIMEOUT, 60),
        (curl_sys::CURLOPT_FOLLOWLOCATION, 0),
        (curl_sys::CURLOPT_MAXREDIRS, 0),
        (curl_sys::CURLOPT_FRESH_CONNECT, 1),
        (curl_sys::CURLOPT_FORBID_REUSE, 1),
        (curl_sys::CURLOPT_PROTOCOLS, SFTP_PROTOCOL),
        (curl_sys::CURLOPT_REDIR_PROTOCOLS, 0),
    ] {
        easy.long(option, value)?;
    }
    ensure!(
        unsafe {
            curl_sys::curl_easy_setopt(
                easy.0,
                curl_sys::CURLOPT_INFILESIZE_LARGE,
                config.bytes as curl_sys::curl_off_t,
            )
        } == curl_sys::CURLE_OK,
        "Native provider engine rejected its byte limit"
    );
    let pin = config
        .host_sha256
        .strip_prefix("SHA256:")
        .unwrap_or(&config.host_sha256);
    easy.string(HOST_SHA256, pin)?;
    // No fallback mask: direct password or the explicitly supplied private key.
    easy.long(
        curl_sys::CURLOPT_SSH_AUTH_TYPES,
        authentication(&config.credentials),
    )?;
    // Derive the public key only from the explicitly supplied private key.
    easy.string(curl_sys::CURLOPT_SSH_PUBLIC_KEYFILE, "")?;
    match &config.credentials {
        Credentials::Password { username, password } => {
            easy.string(curl_sys::CURLOPT_USERNAME, username)?;
            easy.string(curl_sys::CURLOPT_PASSWORD, password)?;
        }
        Credentials::SshKey {
            username,
            private_key,
            passphrase,
        } => {
            easy.string(curl_sys::CURLOPT_USERNAME, username)?;
            easy.string(curl_sys::CURLOPT_SSH_PRIVATE_KEYFILE, private_key)?;
            if !passphrase.is_empty() {
                easy.string(curl_sys::CURLOPT_KEYPASSWD, passphrase)?;
            }
        }
    }
    let pointer = (&mut state as *mut State<'_>).cast::<c_void>();
    ensure!(
        unsafe {
            curl_sys::curl_easy_setopt(easy.0, curl_sys::CURLOPT_INFILE, pointer)
                == curl_sys::CURLE_OK
                && curl_sys::curl_easy_setopt(easy.0, curl_sys::CURLOPT_FILE, pointer)
                    == curl_sys::CURLE_OK
                && curl_sys::curl_easy_setopt(
                    easy.0,
                    curl_sys::CURLOPT_READFUNCTION,
                    read as curl_sys::curl_read_callback,
                ) == curl_sys::CURLE_OK
                && curl_sys::curl_easy_setopt(
                    easy.0,
                    curl_sys::CURLOPT_WRITEFUNCTION,
                    discard as curl_sys::curl_write_callback,
                ) == curl_sys::CURLE_OK
        },
        "Native provider engine rejected bounded callbacks"
    );
    let result = unsafe { curl_sys::curl_easy_perform(easy.0) };
    let mut uploaded = 0f64;
    let observed = unsafe {
        curl_sys::curl_easy_getinfo(
            easy.0,
            curl_sys::CURLINFO_SIZE_UPLOAD,
            &mut uploaded as *mut f64,
        )
    };
    // Destroy callbacks while their borrowed state still exists.
    drop(easy);
    ensure!(
        result == curl_sys::CURLE_OK
            && observed == curl_sys::CURLE_OK
            && !state.failed
            && state.remaining == 0
            && uploaded == config.bytes as f64,
        "Native SFTP transfer failed or did not confirm every input byte"
    );
    Ok(())
}

pub(crate) fn run(args: &[OsString]) -> Result<()> {
    ensure!(
        args.len() == 2 && args[0] == "--config",
        "Invalid private provider-engine invocation"
    );
    let path = args[1].to_str().context("Invalid private provider pipe")?;
    #[cfg(unix)]
    ensure!(
        path == "/dev/fd/3",
        "Private provider engine requires its inherited configuration pipe"
    );
    #[cfg(target_os = "windows")]
    {
        let identifier = path
            .strip_prefix(r"\\.\pipe\screenshots-provider-")
            .context("Private provider engine requires its configuration pipe")?;
        ensure!(
            uuid::Uuid::parse_str(identifier).is_ok(),
            "Invalid private provider pipe identifier"
        );
    }
    let mut configuration = Zeroizing::new(Vec::with_capacity(8193));
    std::fs::File::open(path)
        .context("Could not open private provider pipe")?
        .take(8193)
        .read_to_end(&mut configuration)
        .context("Could not read private provider configuration")?;
    let config = NativeSftpConfig::decode(&configuration)?;
    configuration.zeroize();
    transfer(&config, &mut std::io::stdin().lock())?;
    let mut output = std::io::stdout().lock();
    writeln!(output, "000\n{}", config.bytes)
        .and_then(|_| output.flush())
        .context("SFTP completed but its result could not be written")
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn linked_provider_library_has_sftp_support() {
        let _global = Global::new().unwrap();
        assert!(
            has_sftp(),
            "Build Windows libcurl with the pinned curl[ssh,ssl] vcpkg manifest"
        );
    }
    #[test]
    fn ssh_authentication_never_includes_agent_or_keyboard_fallback() {
        let password = Credentials::Password {
            username: "synthetic".into(),
            password: "not-a-real-secret".into(),
        };
        let key = Credentials::SshKey {
            username: "synthetic".into(),
            private_key: "explicit-key".into(),
            passphrase: String::new(),
        };
        assert_eq!(authentication(&password), 2);
        assert_eq!(authentication(&key), 1);
        for credential in [&password, &key] {
            assert_eq!(authentication(credential) & ((1 << 4) | (1 << 3)), 0);
        }
    }
    #[test]
    fn callbacks_preserve_the_source_extent_and_stop_on_short_input() {
        let mut input = &b"abcdefgh"[..];
        let mut state = State {
            input: &mut input,
            remaining: 5,
            received: 0,
            failed: false,
        };
        let pointer = (&mut state as *mut State<'_>).cast::<c_void>();
        let mut output = [0u8; 16];
        assert_eq!(read(output.as_mut_ptr().cast(), 1, 16, pointer), 5);
        assert_eq!(&output[..5], b"abcde");
        assert_eq!(read(output.as_mut_ptr().cast(), 1, 16, pointer), 0);
        assert!(!state.failed);
        state.remaining = 10;
        assert_eq!(read(output.as_mut_ptr().cast(), 1, 16, pointer), 3);
        assert_eq!(
            read(output.as_mut_ptr().cast(), 1, 16, pointer),
            curl_sys::CURL_READFUNC_ABORT
        );
        assert!(state.failed);
    }
}
