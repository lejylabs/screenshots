//! Explicit SFTP/FTPS/FTP uploads with scoped secrets and bounded streaming.

use anyhow::{Context, Result, bail, ensure};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{
    ffi::OsString,
    fs::File,
    io::{IsTerminal, Read, Write},
    path::{Path, PathBuf},
    process::{Child, Command},
    sync::mpsc::{self, RecvTimeoutError},
    thread::JoinHandle,
    time::{Duration, Instant},
};
use zeroize::{Zeroize, Zeroizing};

const PREFIX: &[u8] = b"screenshots-provider-v1\0";
const MAX_FILE: u64 = 64 * 1024 * 1024;
pub(super) const INTERVAL: Duration = Duration::from_millis(25);

#[derive(Clone, Copy, Deserialize, Serialize, PartialEq)]
#[serde(rename_all = "lowercase")]
pub(super) enum Protocol {
    Sftp,
    Ftps,
    Ftp,
}
impl Protocol {
    fn name(self) -> &'static str {
        match self {
            Self::Sftp => "sftp",
            Self::Ftps => "ftps",
            Self::Ftp => "ftp",
        }
    }
}

#[derive(Clone, Deserialize)]
#[serde(deny_unknown_fields)]
struct Profile {
    version: u32,
    protocol: Protocol,
    host: String,
    port: u16,
    remote_directory: String,
    credential: String,
    host_sha256: Option<String>,
    public_base: Option<String>,
    #[serde(default)]
    allow_plaintext: bool,
    ca_file: Option<PathBuf>,
}

fn bounded_profile(path: &Path) -> Result<Vec<u8>> {
    ensure!(
        path.as_os_str().as_encoded_bytes().len() <= 4096,
        "Provider profile path exceeds 4096 bytes"
    );
    let metadata = path
        .metadata()
        .context("Could not inspect provider profile")?;
    ensure!(
        metadata.is_file() && metadata.len() <= 16384,
        "Provider profile must be a regular file of at most 16 KiB"
    );
    let mut open = std::fs::OpenOptions::new();
    open.read(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        open.custom_flags(libc::O_NONBLOCK);
    }
    let file = open.open(path).context("Could not open provider profile")?;
    let metadata = file.metadata()?;
    ensure!(
        metadata.is_file() && metadata.len() <= 16384,
        "Provider profile must be a regular file of at most 16 KiB"
    );
    let mut bytes = Vec::with_capacity(metadata.len() as usize);
    file.take(metadata.len()).read_to_end(&mut bytes)?;
    Ok(bytes)
}

fn reference(value: &str) -> bool {
    (1..=64).contains(&value.len())
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || b"_-".contains(&byte))
}

fn host(value: &str) -> bool {
    if value.parse::<std::net::IpAddr>().is_ok() {
        return true;
    }
    (1..=253).contains(&value.len())
        && value.is_ascii()
        && value == value.to_ascii_lowercase()
        && value.split('.').all(|part| {
            (1..=63).contains(&part.len())
                && part
                    .as_bytes()
                    .first()
                    .is_some_and(u8::is_ascii_alphanumeric)
                && part
                    .as_bytes()
                    .last()
                    .is_some_and(u8::is_ascii_alphanumeric)
                && part
                    .bytes()
                    .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-')
        })
}

fn path_segments(value: &str) -> bool {
    value == "/"
        || (value.len() <= 1024
            && value.starts_with('/')
            && value[1..].split('/').all(|part| {
                (1..=128).contains(&part.len())
                    && !matches!(part, "." | "..")
                    && part
                        .bytes()
                        .all(|byte| byte.is_ascii_alphanumeric() || b"_.-~".contains(&byte))
            }))
}

fn host_pin(value: &str) -> bool {
    let value = value.strip_prefix("SHA256:").unwrap_or(value);
    let unpadded = value.strip_suffix('=').unwrap_or(value);
    unpadded.len() == 43
        && value.len() <= 44
        && unpadded
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || b"+/".contains(&byte))
        && unpadded
            .as_bytes()
            .last()
            .is_some_and(|byte| b"AEIMQUYcgkosw048".contains(byte))
}

impl Profile {
    fn load(path: &Path) -> Result<Self> {
        let bytes = bounded_profile(path)?;
        ensure!(
            bytes
                .iter()
                .copied()
                .find(|byte| !byte.is_ascii_whitespace())
                == Some(b'{'),
            "Provider profile must be a JSON object"
        );
        let value: Self = serde_json::from_slice(&bytes)
            .map_err(|_| anyhow::anyhow!("Invalid provider profile"))?;
        ensure!(
            value.version == 1
                && value.port != 0
                && host(&value.host)
                && reference(&value.credential)
                && path_segments(&value.remote_directory),
            "Invalid provider version, host, port, directory or credential reference"
        );
        ensure!(
            (value.protocol == Protocol::Ftp) == value.allow_plaintext,
            "Plaintext FTP requires allow_plaintext=true; secure protocols forbid it"
        );
        if value.protocol == Protocol::Sftp {
            ensure!(
                value.remote_directory != "/~" && !value.remote_directory.starts_with("/~/"),
                "SFTP home-relative directory syntax is not an absolute remote path"
            );
            ensure!(
                value.host_sha256.as_deref().is_some_and(host_pin),
                "SFTP requires a SHA256 known-host pin"
            );
            ensure!(value.ca_file.is_none(), "SFTP does not use a TLS CA file");
        } else {
            ensure!(
                value.host_sha256.is_none(),
                "Host-key pins apply only to SFTP"
            );
        }
        if let Some(path) = &value.ca_file {
            ensure!(
                value.protocol == Protocol::Ftps
                    && path.as_os_str().as_encoded_bytes().len() <= 4096,
                "A CA file applies only to FTPS"
            );
            let metadata = path
                .metadata()
                .context("Could not inspect provider CA file")?;
            ensure!(
                metadata.is_file() && metadata.len() <= 1024 * 1024 && path.to_str().is_some(),
                "Provider CA must be a regular UTF-8 path file of at most 1 MiB"
            );
        }
        if let Some(base) = &value.public_base {
            ensure!(
                base.len() <= 2012,
                "Provider public URL base exceeds its generated-object limit"
            );
            let validated = app_core::UploadOutcome {
                url: base.clone(),
                thumbnail: None,
                deletion: None,
            }
            .validate_links()?;
            ensure!(
                validated.url == *base
                    && base.starts_with("https://")
                    && base.ends_with('/')
                    && !base.contains(['?', '#']),
                "Public base must be a canonical HTTPS directory URL"
            );
        }
        Ok(value)
    }
    fn target(&self) -> String {
        let scope = format!("{}://{}:{}", self.protocol.name(), self.host, self.port);
        let hash = Sha256::digest(scope.as_bytes())
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect::<String>();
        format!(
            "Screenshots/provider/{}/{hash}/{}",
            self.protocol.name(),
            self.credential
        )
    }
    fn remote(&self, filename: &str) -> (String, String) {
        let host = if self.host.contains(':') {
            format!("[{}]", self.host)
        } else {
            self.host.clone()
        };
        let path = format!(
            "{}/{}",
            self.remote_directory.trim_end_matches('/'),
            filename
        );
        let scheme = if self.protocol == Protocol::Sftp {
            "sftp"
        } else {
            "ftp"
        };
        let slash = if self.protocol == Protocol::Sftp {
            ""
        } else {
            "/"
        };
        (
            format!("{scheme}://{host}:{}{slash}{path}", self.port),
            path,
        )
    }
}

#[derive(Deserialize, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub(super) enum Credentials {
    Password {
        username: String,
        password: String,
    },
    SshKey {
        username: String,
        private_key: String,
        #[serde(default)]
        passphrase: String,
    },
}
impl Drop for Credentials {
    fn drop(&mut self) {
        match self {
            Self::Password { username, password } => {
                username.zeroize();
                password.zeroize();
            }
            Self::SshKey {
                username,
                private_key,
                passphrase,
            } => {
                username.zeroize();
                private_key.zeroize();
                passphrase.zeroize();
            }
        }
    }
}
impl Credentials {
    fn decode(bytes: &[u8], protocol: Protocol) -> Result<Self> {
        ensure!(
            bytes.len() <= 4096
                && bytes
                    .iter()
                    .copied()
                    .find(|byte| !byte.is_ascii_whitespace())
                    == Some(b'{'),
            "Invalid provider credential JSON"
        );
        let result: Self = serde_json::from_slice(bytes)
            .map_err(|_| anyhow::anyhow!("Invalid provider credential JSON"))?;
        result.validate(protocol)?;
        Ok(result)
    }
    fn validate(&self, protocol: Protocol) -> Result<()> {
        let (username, secret) = match self {
            Self::Password { username, password } => (username, password),
            Self::SshKey {
                username,
                passphrase,
                ..
            } => (username, passphrase),
        };
        ensure!(
            (1..=128).contains(&username.len())
                && username.trim() == username
                && !username.contains(':')
                && !username.chars().any(char::is_control),
            "Invalid provider username"
        );
        ensure!(
            secret.len() <= 1024 && !secret.chars().any(char::is_control),
            "Invalid provider secret"
        );
        if let Self::SshKey { private_key, .. } = self {
            ensure!(
                protocol == Protocol::Sftp
                    && !private_key.is_empty()
                    && private_key.len() <= 4096
                    && !private_key.chars().any(char::is_control),
                "SSH keys apply only to SFTP"
            );
            let metadata = Path::new(private_key)
                .metadata()
                .context("Could not inspect explicit SSH key")?;
            ensure!(
                metadata.is_file() && metadata.len() <= 1024 * 1024,
                "SSH key must be a regular file of at most 1 MiB"
            );
        }
        Ok(())
    }
    fn payload(&self) -> Result<Zeroizing<Vec<u8>>> {
        let mut bytes = Zeroizing::new(Vec::with_capacity(2560));
        bytes.extend_from_slice(PREFIX);
        serde_json::to_writer(&mut *bytes, self)
            .map_err(|_| anyhow::anyhow!("Could not encode provider credential"))?;
        ensure!(
            bytes.len() <= 2560,
            "Provider credential exceeds 2560 bytes"
        );
        Ok(bytes)
    }
}

fn config_line(config: &mut String, name: &str, value: &str) {
    config.push_str(name);
    config.push_str(" = \"");
    for ch in value.chars() {
        match ch {
            '\n' => config.push_str("\\n"),
            '\r' => config.push_str("\\r"),
            '\t' => config.push_str("\\t"),
            '\\' | '"' => {
                config.push('\\');
                config.push(ch);
            }
            _ => config.push(ch),
        }
    }
    config.push_str("\"\n");
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct NativeSftpConfig {
    pub(super) version: u32,
    pub(super) url: String,
    pub(super) host_sha256: String,
    pub(super) credentials: Credentials,
    pub(super) bytes: u64,
}

impl NativeSftpConfig {
    pub(super) fn decode(bytes: &[u8]) -> Result<Self> {
        ensure!(
            bytes.len() <= 8192
                && bytes
                    .iter()
                    .copied()
                    .find(|byte| !byte.is_ascii_whitespace())
                    == Some(b'{'),
            "Invalid private provider configuration"
        );
        let value: Self = serde_json::from_slice(bytes)
            .map_err(|_| anyhow::anyhow!("Invalid private provider configuration"))?;
        ensure!(
            value.version == 1
                && value.bytes <= MAX_FILE
                && value.url.len() <= 2048
                && !value.url.chars().any(char::is_control),
            "Private provider configuration exceeds its limits"
        );
        let rest = value
            .url
            .strip_prefix("sftp://")
            .context("Private provider engine only supports SFTP")?;
        let (authority, path) = rest
            .split_once('/')
            .context("Invalid private provider URL")?;
        let full_path = format!("/{path}");
        let (directory, filename) = full_path
            .rsplit_once('/')
            .context("Invalid private provider path")?;
        let directory = if directory.is_empty() { "/" } else { directory };
        ensure!(
            !authority.is_empty()
                && !authority.contains('@')
                && !value.url.contains(['?', '#', '\\'])
                && path_segments(directory)
                && filename.len() == 36
                && filename.ends_with(".bin")
                && filename.as_bytes()[..32]
                    .iter()
                    .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(byte))
                && path != "~"
                && !path.starts_with("~/"),
            "Invalid private provider URL"
        );
        ensure!(
            host_pin(&value.host_sha256),
            "Private SFTP engine requires a known-host SHA256 pin"
        );
        value.credentials.validate(Protocol::Sftp)?;
        Ok(value)
    }
}

fn native_sftp_config(
    profile: &Profile,
    credentials: &Credentials,
    url: &str,
    length: u64,
) -> Result<Zeroizing<Vec<u8>>> {
    #[derive(Serialize)]
    struct Fields<'a> {
        version: u32,
        url: &'a str,
        host_sha256: &'a str,
        credentials: &'a Credentials,
        bytes: u64,
    }
    let mut bytes = Zeroizing::new(Vec::with_capacity(8192));
    serde_json::to_writer(
        &mut *bytes,
        &Fields {
            version: 1,
            url,
            host_sha256: profile.host_sha256.as_deref().unwrap(),
            credentials,
            bytes: length,
        },
    )
    .map_err(|_| anyhow::anyhow!("Could not encode private provider configuration"))?;
    ensure!(
        bytes.len() <= 8192,
        "Private provider configuration exceeds 8 KiB"
    );
    Ok(bytes)
}

fn config(profile: &Profile, credentials: &Credentials, url: &str) -> Result<Zeroizing<Vec<u8>>> {
    ensure!(
        profile.protocol != Protocol::Sftp,
        "SFTP requires its constrained native engine"
    );
    let mut value = Zeroizing::new(String::with_capacity(8192));
    value.push_str("silent\nfail\nno-netrc\nno-location\nconnect-timeout = 5\nmax-time = 60\nretry = 0\nmax-redirs = 0\nftp-skip-pasv-ip\n");
    for (key, text) in [
        ("url", url),
        ("upload-file", "-"),
        ("proxy", ""),
        ("noproxy", "*"),
        ("proto", "=ftp,ftps"),
        (
            "output",
            if cfg!(target_os = "windows") {
                "NUL"
            } else {
                "/dev/null"
            },
        ),
        ("write-out", "%{response_code}\n%{size_upload}\n"),
    ] {
        config_line(&mut value, key, text);
    }
    let user = match credentials {
        Credentials::Password { username, password } => {
            Zeroizing::new(format!("{username}:{password}"))
        }
        Credentials::SshKey { .. } => bail!("FTP requires a password credential"),
    };
    config_line(&mut value, "user", &user);
    if profile.protocol == Protocol::Ftps {
        value.push_str("ssl-reqd\ntlsv1.2\n");
        if let Some(path) = &profile.ca_file {
            config_line(&mut value, "cacert", path.to_str().unwrap());
        }
    }
    ensure!(
        value.len() <= 8192,
        "Provider transport configuration exceeds 8 KiB"
    );
    Ok(Zeroizing::new(value.as_bytes().to_vec()))
}

pub(super) fn curl() -> Command {
    let mut command = Command::new(if cfg!(target_os = "windows") {
        "curl.exe"
    } else {
        "curl"
    });
    command.arg("-q");
    scrub_environment(&mut command);
    command
}

pub(super) fn scrub_environment(command: &mut Command) {
    for key in [
        "HTTP_PROXY",
        "HTTPS_PROXY",
        "ALL_PROXY",
        "http_proxy",
        "https_proxy",
        "all_proxy",
        "CURL_HOME",
        "CURL_TRACE",
        "CURL_CA_BUNDLE",
        "SSL_CERT_FILE",
        "SSL_CERT_DIR",
        "SSLKEYLOGFILE",
        "SSH_AUTH_SOCK",
        "NETRC",
    ] {
        command.env_remove(key);
    }
}

fn source(path: &Path) -> Result<(File, u64)> {
    ensure!(
        path.as_os_str().as_encoded_bytes().len() <= 4096,
        "Provider input path exceeds 4096 bytes"
    );
    let check = |metadata: std::fs::Metadata| -> Result<u64> {
        ensure!(
            metadata.is_file() && metadata.len() <= MAX_FILE,
            "Provider input must be a regular file of at most 64 MiB"
        );
        Ok(metadata.len())
    };
    check(
        path.metadata()
            .context("Could not inspect provider input")?,
    )?;
    let mut options = std::fs::OpenOptions::new();
    options.read(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.custom_flags(libc::O_NONBLOCK);
    }
    let file = options
        .open(path)
        .context("Could not open provider input")?;
    let length = check(file.metadata()?)?;
    Ok((file, length))
}

fn verify_result(bytes: &[u8], protocol: Protocol, length: u64) -> Result<()> {
    let value = std::str::from_utf8(bytes).context("Invalid provider transfer result")?;
    let mut lines = value.lines();
    let status = lines
        .next()
        .and_then(|line| line.parse::<u16>().ok())
        .context("Invalid provider status")?;
    let uploaded = lines
        .next()
        .and_then(|line| line.parse::<u64>().ok())
        .context("Invalid provider byte count")?;
    ensure!(
        lines.next().is_none()
            && uploaded == length
            && if protocol == Protocol::Sftp {
                status == 0
            } else {
                (200..=299).contains(&status)
            },
        "Provider did not confirm the complete upload"
    );
    Ok(())
}

#[derive(Clone)]
pub(crate) struct PreparedUpload(Profile);

fn checked_credentials(
    target: &str,
    check: &impl Fn() -> Result<()>,
    read: impl FnOnce(&str) -> Result<Zeroizing<Vec<u8>>>,
) -> Result<Zeroizing<Vec<u8>>> {
    check()?;
    let value = read(target)?;
    check()?;
    Ok(value)
}

impl PreparedUpload {
    pub(crate) fn read(path: &Path) -> Result<Self> {
        Profile::load(path).map(Self)
    }
    pub(crate) fn has_public_link(&self) -> bool {
        self.0.public_base.is_some()
    }
    pub(crate) fn upload_file(&self, path: &Path) -> Result<serde_json::Value> {
        with_operation_cancellation(|check| self.upload_file_with_cancel(path, check))
    }
    pub(crate) fn upload_file_with_cancel(
        &self,
        path: &Path,
        check: impl Fn() -> Result<()>,
    ) -> Result<serde_json::Value> {
        let profile = &self.0;
        check()?;
        let (file, length) = source(path)?;
        check()?;
        let secret = checked_credentials(
            &profile.target(),
            &check,
            app_core::read_provider_credentials,
        )?;
        let bytes = secret
            .strip_prefix(PREFIX)
            .context("Stored provider credential has an unsupported format")?;
        let credentials = Credentials::decode(bytes, profile.protocol)?;
        check()?;
        let filename = format!("{}.bin", uuid::Uuid::new_v4().simple());
        let (url, key) = profile.remote(&filename);
        let configuration = if profile.protocol == Protocol::Sftp {
            native_sftp_config(profile, &credentials, &url, length)?
        } else {
            config(profile, &credentials, &url)?
        };
        drop(credentials);
        drop(secret);
        #[cfg(unix)]
        let output = crate::sharing_provider_unix::transfer(
            configuration,
            file,
            length,
            &check,
            profile.protocol == Protocol::Sftp,
        )?;
        #[cfg(target_os = "windows")]
        let output = crate::sharing_provider_windows::transfer(
            configuration,
            file,
            length,
            &check,
            profile.protocol == Protocol::Sftp,
        )?;
        #[cfg(not(any(unix, target_os = "windows")))]
        let output: Vec<u8> = {
            let _ = (configuration, file, length);
            bail!("Provider transfer is unsupported on this platform");
        };
        verify_result(&output, profile.protocol, length).context(
            "Upload outcome is uncertain; a remote object may remain and no retry was attempted",
        )?;
        Ok(
            serde_json::json!({"protocol":profile.protocol.name(),"key":key,"bytes":length,"url":profile.public_base.as_ref().map(|base| format!("{base}{filename}"))}),
        )
    }
}

fn with_operation_cancellation<T>(
    operation: impl FnOnce(&dyn Fn() -> Result<()>) -> Result<T>,
) -> Result<T> {
    #[cfg(unix)]
    {
        let cancellation = crate::record::unix::Cancellation::new()?;
        let deadline = Instant::now() + Duration::from_secs(60);
        operation(&|| cancellation.check(deadline))
    }
    #[cfg(not(unix))]
    operation(&|| Ok(()))
}
pub(crate) fn upload_profile(profile: &Path, path: &Path) -> Result<serde_json::Value> {
    PreparedUpload::read(profile)?.upload_file(path)
}

pub(crate) fn validate_profile(path: &Path) -> Result<()> {
    Profile::load(path).map(|_| ())
}

pub(crate) fn upload(args: &[OsString]) -> Result<()> {
    ensure!(
        args.len() == 2,
        "Usage: screenshots provider-upload PROFILE FILE"
    );
    let result = upload_profile(Path::new(&args[0]), Path::new(&args[1]))?;
    write_outcome(&result)
}

fn write_outcome(result: &serde_json::Value) -> Result<()> {
    let mut output = std::io::stdout().lock();
    writeln!(output,"{result}").and_then(|_|output.flush()).context("Provider upload completed but its result could not be written; the remote object remains")
}

pub(crate) fn credentials(args: &[OsString]) -> Result<()> {
    ensure!(
        args.len() == 2,
        "Usage: screenshots provider-credentials target|store|remove PROFILE"
    );
    let action = args[0]
        .to_str()
        .context("Invalid provider credential action")?;
    ensure!(
        matches!(action, "target" | "store" | "remove"),
        "Invalid provider credential action"
    );
    let profile = Profile::load(Path::new(&args[1]))?;
    let target = profile.target();
    let result = match action {
        "target" => serde_json::json!({"target":target}),
        "store" => {
            ensure!(
                !std::io::stdin().is_terminal(),
                "Pipe provider credential JSON on stdin; interactive secret echo is disabled"
            );
            let mut input = Zeroizing::new(Vec::with_capacity(4097));
            std::io::stdin().lock().take(4097).read_to_end(&mut input)?;
            let value = Credentials::decode(&input, profile.protocol)?;
            app_core::save_provider_credentials(&target, &value.payload()?)?;
            serde_json::json!({"stored":true,"profile_file_changed":false})
        }
        "remove" => {
            app_core::remove_provider_credentials(&target)?;
            serde_json::json!({"removed":true,"profile_file_changed":false})
        }
        _ => unreachable!(),
    };
    let mut output = std::io::stdout().lock();
    writeln!(output, "{result}")
        .and_then(|_| output.flush())
        .context("Provider credential action completed but its result could not be written")
}

pub(super) trait Process {
    fn child(&mut self) -> &mut Child;
    fn config(&mut self) -> Result<bool>;
    fn stop(&mut self);
}

struct Active<P: Process> {
    process: P,
    workers: Vec<JoinHandle<()>>,
}
impl<P: Process> Drop for Active<P> {
    fn drop(&mut self) {
        self.process.stop();
        for worker in self.workers.drain(..) {
            let _ = worker.join();
        }
    }
}

fn drain(
    mut input: impl Read,
    cap: usize,
    keep: bool,
) -> std::result::Result<Vec<u8>, &'static str> {
    let mut result = Vec::new();
    let mut count = 0usize;
    let mut buffer = Zeroizing::new(vec![0; 4096]);
    loop {
        let read = input
            .read(&mut buffer)
            .map_err(|_| "Provider pipe failed")?;
        if read == 0 {
            return Ok(result);
        }
        count += read;
        if count > cap {
            return Err("Provider output exceeded its limit");
        }
        if keep {
            result.extend_from_slice(&buffer[..read]);
        }
    }
}

pub(super) fn execute<P: Process>(
    process: P,
    mut file: File,
    length: u64,
    deadline: Instant,
    check: impl Fn() -> Result<()>,
) -> Result<Vec<u8>> {
    let mut active = Active {
        process,
        workers: Vec::with_capacity(3),
    };
    let stdout = active
        .process
        .child()
        .stdout
        .take()
        .context("Provider result pipe is unavailable")?;
    let stderr = active
        .process
        .child()
        .stderr
        .take()
        .context("Provider diagnostic pipe is unavailable")?;
    let mut stdin = active
        .process
        .child()
        .stdin
        .take()
        .context("Provider input pipe is unavailable")?;
    let (sender, messages) = mpsc::sync_channel(3);
    for (kind, input, cap, keep) in [
        (0, Box::new(stdout) as Box<dyn Read + Send>, 4096, true),
        (1, Box::new(stderr) as Box<dyn Read + Send>, 16384, false),
    ] {
        let sender = sender.clone();
        active.workers.push(
            std::thread::Builder::new()
                .name("provider-output".into())
                .stack_size(256 * 1024)
                .spawn(move || {
                    let _ = sender.send((kind, drain(input, cap, keep)));
                })?,
        );
    }
    active.workers.push(
        std::thread::Builder::new()
            .name("provider-input".into())
            .stack_size(256 * 1024)
            .spawn(move || {
                let result = (|| {
                    let mut remaining = length;
                    let mut buffer = Zeroizing::new(vec![0; 64 * 1024]);
                    while remaining > 0 {
                        let size = usize::try_from(remaining.min(buffer.len() as u64)).unwrap();
                        let count = file
                            .read(&mut buffer[..size])
                            .map_err(|_| "Provider source read failed")?;
                        if count == 0 {
                            return Err("Provider source became shorter");
                        }
                        stdin
                            .write_all(&buffer[..count])
                            .map_err(|_| "Provider input pipe failed")?;
                        remaining -= count as u64;
                    }
                    drop(stdin);
                    Ok(Vec::new())
                })();
                let _ = sender.send((2, result));
            })?,
    );
    let mut received = [false; 3];
    let mut result = Vec::new();
    loop {
        check()?;
        ensure!(
            Instant::now() < deadline,
            "Provider transfer exceeded its 60 second deadline"
        );
        let delivered = active.process.config()?;
        if let Some(status) = active
            .process
            .child()
            .try_wait()
            .context("Could not observe provider transfer")?
        {
            ensure!(
                status.success(),
                "Provider transfer failed (engine exit {}); verify installed curl protocol support, credentials and server trust",
                status.code().unwrap_or(-1)
            );
            if received.iter().all(|done| *done) {
                ensure!(delivered, "Provider configuration was not delivered");
                return Ok(result);
            }
        }
        match messages
            .recv_timeout(INTERVAL.min(deadline.saturating_duration_since(Instant::now())))
        {
            Ok((kind, bytes)) => {
                let bytes = bytes.map_err(anyhow::Error::msg)?;
                ensure!(!received[kind], "Duplicate provider completion");
                received[kind] = true;
                if kind == 0 {
                    result = bytes;
                }
            }
            Err(RecvTimeoutError::Timeout) => {}
            Err(RecvTimeoutError::Disconnected) => {
                ensure!(
                    received.iter().all(|done| *done),
                    "Provider worker ended without completion"
                );
                std::thread::sleep(INTERVAL);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn transport_cannot_inherit_tls_session_key_logging() {
        assert!(
            curl()
                .get_envs()
                .any(|(key, value)| key == "SSLKEYLOGFILE" && value.is_none())
        );
    }
    #[test]
    fn public_url_base_reserves_space_for_the_generated_uuid_filename() {
        let fixture = Fixture::new();
        let mut value = ftp_profile();
        let prefix = "https://example.invalid/";
        let base = format!("{prefix}{}/", "a".repeat(2012 - prefix.len() - 1));
        value["public_base"] = base.clone().into();
        assert!(Profile::load(&fixture.profile(value.clone())).is_ok());
        value["public_base"] = format!("{base}a/").into();
        assert!(Profile::load(&fixture.profile(value)).is_err());
    }
    #[test]
    fn sftp_rejects_curl_home_directory_aliases() {
        let fixture = Fixture::new();
        let mut value = ftp_profile();
        value["protocol"] = "sftp".into();
        value["port"] = 22.into();
        value["host_sha256"] = format!("{}=", "A".repeat(43)).into();
        for path in ["/~", "/~/uploads"] {
            value["remote_directory"] = path.into();
            assert!(Profile::load(&fixture.profile(value.clone())).is_err());
        }
        value["remote_directory"] = "/uploads/~/images".into();
        assert!(Profile::load(&fixture.profile(value)).is_ok());
    }
    struct Fixture(PathBuf);
    impl Fixture {
        fn new() -> Self {
            let path =
                std::env::temp_dir().join(format!("screenshots-provider-{}", uuid::Uuid::new_v4()));
            std::fs::create_dir(&path).unwrap();
            Self(path)
        }
        fn path(&self, name: &str) -> PathBuf {
            self.0.join(name)
        }
        fn profile(&self, value: serde_json::Value) -> PathBuf {
            let path = self.path("profile.json");
            std::fs::write(&path, serde_json::to_vec(&value).unwrap()).unwrap();
            path
        }
    }
    impl Drop for Fixture {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }
    fn ftp_profile() -> serde_json::Value {
        serde_json::json!({"version":1,"protocol":"ftps","host":"example.invalid","port":21,"remote_directory":"/uploads","credential":"fixture"})
    }

    #[test]
    fn profiles_fail_closed_and_targets_bind_protocol_host_port_and_reference() {
        let fixture = Fixture::new();
        let value = ftp_profile();
        let path = fixture.profile(value.clone());
        let profile = Profile::load(&path).unwrap();
        assert!(profile.target().starts_with("Screenshots/provider/ftps/"));
        let (url, key) = profile.remote("synthetic.bin");
        assert_eq!(url, "ftp://example.invalid:21//uploads/synthetic.bin");
        assert_eq!(key, "/uploads/synthetic.bin");
        let original = profile.target();
        for (field, bad) in [
            ("version", 2.into()),
            ("host", "EXAMPLE.invalid".into()),
            ("host", "user@example.invalid".into()),
            ("host", "example.invalid:21".into()),
            ("port", 0.into()),
            ("remote_directory", "/../other".into()),
            ("remote_directory", "relative".into()),
            ("credential", "".into()),
            ("allow_plaintext", true.into()),
            ("public_base", "https://user:secret@example.invalid/".into()),
        ] {
            let mut invalid = value.clone();
            invalid[field] = bad;
            let path = fixture.profile(invalid);
            assert!(Profile::load(&path).is_err(), "{field}");
        }
        let mut changed = value.clone();
        changed["port"] = 2121.into();
        assert_ne!(
            Profile::load(&fixture.profile(changed)).unwrap().target(),
            original
        );
        let mut plain = value.clone();
        plain["protocol"] = "ftp".into();
        assert!(Profile::load(&fixture.profile(plain.clone())).is_err());
        plain["allow_plaintext"] = true.into();
        Profile::load(&fixture.profile(plain)).unwrap();
        let mut ssh = value;
        ssh["protocol"] = "sftp".into();
        ssh["port"] = 22.into();
        assert!(Profile::load(&fixture.profile(ssh.clone())).is_err());
        ssh["host_sha256"] = format!("{}=", "A".repeat(43)).into();
        let profile = Profile::load(&fixture.profile(ssh)).unwrap();
        assert_eq!(
            profile.remote("synthetic.bin").0,
            "sftp://example.invalid:22/uploads/synthetic.bin"
        );
    }

    #[test]
    fn credentials_use_versioned_native_payloads_and_config_escape_secrets() {
        let credential = Credentials::decode(
            br#"{"kind":"password","username":"synthetic","password":"SECRET_\"\\_VALUE"}"#,
            Protocol::Ftps,
        )
        .unwrap();
        let payload = credential.payload().unwrap();
        assert!(payload.starts_with(PREFIX));
        assert!(payload.len() <= 2560);
        let fixture = Fixture::new();
        let profile = Profile::load(&fixture.profile(ftp_profile())).unwrap();
        let config = config(&profile, &credential, "ftp://example.invalid/new.bin").unwrap();
        let text = std::str::from_utf8(&config).unwrap();
        assert!(text.contains("user = \"synthetic:SECRET_\\\"\\\\_VALUE\""));
        assert!(text.contains("write-out = \"%{response_code}\\n%{size_upload}\\n\""));
        assert!(text.contains("ssl-reqd"));
        let command = curl();
        assert!(
            command
                .get_args()
                .all(|arg| !arg.to_string_lossy().contains("SECRET"))
        );
        for invalid in [
            br#"{"kind":"password","username":"user:extra","password":"secret"}"#.as_slice(),
            br#"{"kind":"password","username":"user","password":"secret\n"}"#,
            br#"{"kind":"password","username":"user","password":"secret","unknown":1}"#,
        ] {
            assert!(Credentials::decode(invalid, Protocol::Ftps).is_err());
        }
    }

    #[test]
    fn private_sftp_configuration_is_bounded_and_cannot_fall_back_to_cli_authentication() {
        let fixture = Fixture::new();
        let mut value = ftp_profile();
        value["protocol"] = "sftp".into();
        value["host_sha256"] = format!("{}=", "A".repeat(43)).into();
        let profile = Profile::load(&fixture.profile(value)).unwrap();
        let credential = Credentials::decode(
            br#"{"kind":"password","username":"synthetic","password":"synthetic-only"}"#,
            Protocol::Sftp,
        )
        .unwrap();
        let url = profile.remote("0123456789abcdef0123456789abcdef.bin").0;
        assert!(config(&profile, &credential, &url).is_err());
        let bytes = native_sftp_config(&profile, &credential, &url, 12).unwrap();
        let parsed = NativeSftpConfig::decode(&bytes).unwrap();
        assert_eq!(parsed.bytes, 12);
        assert_eq!(parsed.url, url);
        let original: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
        for (key, bad) in [
            ("bytes", (MAX_FILE + 1).into()),
            (
                "url",
                "ftp://example.invalid/uploads/0123456789abcdef0123456789abcdef.bin".into(),
            ),
            (
                "url",
                "sftp://user@example.invalid/uploads/0123456789abcdef0123456789abcdef.bin".into(),
            ),
            (
                "url",
                "sftp://example.invalid/~/0123456789abcdef0123456789abcdef.bin".into(),
            ),
            ("url", "sftp://example.invalid/uploads/invalid.bin".into()),
            ("unknown", true.into()),
        ] {
            let mut invalid = original.clone();
            invalid[key] = bad;
            assert!(
                NativeSftpConfig::decode(&serde_json::to_vec(&invalid).unwrap()).is_err(),
                "{key}"
            );
        }
    }

    #[test]
    fn complete_transfer_results_and_source_admission_are_bounded() {
        verify_result(b"226\n3\n", Protocol::Ftps, 3).unwrap();
        verify_result(b"000\n0\n", Protocol::Sftp, 0).unwrap();
        for value in [
            b"550\n3\n".as_slice(),
            b"226\n2\n",
            b"226\n3\nextra\n",
            b"secret",
        ] {
            assert!(verify_result(value, Protocol::Ftps, 3).is_err());
        }
        let fixture = Fixture::new();
        let path = fixture.path("source.bin");
        std::fs::write(&path, b"valid").unwrap();
        assert_eq!(source(&path).unwrap().1, 5);
        assert!(source(&fixture.0).is_err());
        let file = std::fs::OpenOptions::new().write(true).open(&path).unwrap();
        file.set_len(MAX_FILE + 1).unwrap();
        assert!(source(&path).is_err());
    }

    #[test]
    fn cancellation_during_native_lookup_is_observed_before_any_transfer() {
        let canceled = std::cell::Cell::new(false);
        let checks = std::cell::Cell::new(0);
        let result = checked_credentials(
            "synthetic-target",
            &|| {
                checks.set(checks.get() + 1);
                ensure!(!canceled.get(), "Outer task canceled");
                Ok(())
            },
            |_| {
                canceled.set(true);
                Ok(Zeroizing::new(b"must not reach a transport".to_vec()))
            },
        );
        assert!(
            result
                .unwrap_err()
                .to_string()
                .contains("Outer task canceled")
        );
        assert_eq!(checks.get(), 2);
    }

    #[cfg(unix)]
    #[test]
    fn retired_operation_remains_terminable_during_blocked_success_output() {
        const CHILD: &str = "SCREENSHOTS_PROVIDER_OUTPUT_CHILD";
        if let Some(marker) = std::env::var_os(CHILD) {
            let value = with_operation_cancellation(|check| {
                check()?;
                Ok(serde_json::json!({"bytes": 0, "completed": true}))
            })
            .unwrap();
            std::fs::write(&marker, b"ready").unwrap();
            std::io::stdin().read_exact(&mut [0u8; 1]).unwrap();
            std::fs::write(&marker, b"writing").unwrap();
            write_outcome(&value).unwrap();
            return;
        }
        use std::{os::fd::AsRawFd, process::Stdio};
        let fixture = Fixture::new();
        let marker = fixture.path("phase");
        let (_reader, mut writer) = std::io::pipe().unwrap();
        let mut child = Command::new(std::env::current_exe().unwrap())
            .args(["--exact", "sharing_provider::tests::retired_operation_remains_terminable_during_blocked_success_output", "--nocapture"])
            .env(CHILD, &marker)
            .stdin(Stdio::piped())
            .stdout(writer.try_clone().unwrap())
            .stderr(Stdio::null())
            .spawn().unwrap();
        let deadline = Instant::now() + Duration::from_secs(5);
        let wait_phase = |expected: &[u8]| {
            while std::fs::read(&marker).ok().as_deref() != Some(expected) {
                if Instant::now() >= deadline {
                    return false;
                }
                std::thread::sleep(INTERVAL);
            }
            true
        };
        let result = (|| -> Result<()> {
            ensure!(
                wait_phase(b"ready"),
                "Provider result fixture did not become ready"
            );
            let descriptor = writer.as_raw_fd();
            let flags = unsafe { libc::fcntl(descriptor, libc::F_GETFL) };
            ensure!(
                flags >= 0
                    && unsafe { libc::fcntl(descriptor, libc::F_SETFL, flags | libc::O_NONBLOCK) }
                        == 0,
                "Could not fill fixture output pipe"
            );
            for block in [&[0u8; 4096][..], &[0u8; 1][..]] {
                loop {
                    match writer.write(block) {
                        Ok(_) => {}
                        Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => break,
                        Err(error) => return Err(error.into()),
                    }
                }
            }
            ensure!(
                unsafe { libc::fcntl(descriptor, libc::F_SETFL, flags) } == 0,
                "Could not restore fixture output blocking"
            );
            child.stdin.take().unwrap().write_all(b"x")?;
            ensure!(
                wait_phase(b"writing"),
                "Provider result fixture did not enter output"
            );
            std::thread::sleep(Duration::from_millis(50));
            ensure!(child.try_wait()?.is_none(), "Result output did not block");
            ensure!(
                unsafe { libc::kill(child.id() as i32, libc::SIGTERM) } == 0,
                "Could not signal blocked provider result"
            );
            loop {
                if let Some(status) = child.try_wait()? {
                    ensure!(!status.success(), "SIGTERM unexpectedly returned success");
                    return Ok(());
                }
                ensure!(
                    Instant::now() < deadline,
                    "Provider result ignored SIGTERM after operation cleanup"
                );
                std::thread::sleep(INTERVAL);
            }
        })();
        let _ = child.kill();
        let _ = child.wait();
        result.unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn private_config_and_bounded_stdin_reach_only_the_exact_child() {
        const CHILD: &str = "SCREENSHOTS_PROVIDER_TEST_CHILD";
        if std::env::var_os(CHILD).is_some() {
            let path = PathBuf::from(std::env::var_os("SCREENSHOTS_PROVIDER_SOURCE").unwrap());
            let (file, length) = source(&path).unwrap();
            std::fs::OpenOptions::new()
                .append(true)
                .open(&path)
                .unwrap()
                .write_all(b"later growth must not upload")
                .unwrap();
            let output = crate::sharing_provider_unix::transfer(
                Zeroizing::new(b"SYNTHETIC_SECRET_CONFIG".to_vec()),
                file,
                length,
                &|| Ok(()),
                false,
            )
            .unwrap();
            assert_eq!(output, b"000\n5\n");
            return;
        }
        use std::os::unix::fs::PermissionsExt;
        let fixture = Fixture::new();
        let engine = fixture.path("curl");
        std::fs::write(&engine,"#!/bin/sh\nconfig=$(/bin/cat /dev/fd/3)\n[ \"$config\" = 'SYNTHETIC_SECRET_CONFIG' ] || exit 7\ncase \"$*\" in *SYNTHETIC_SECRET*)exit 8;;esac\n/bin/cat > \"$SCREENSHOTS_PROVIDER_CAPTURE\"\nprintf '000\\n5\\n'\n").unwrap();
        std::fs::set_permissions(&engine, std::fs::Permissions::from_mode(0o700)).unwrap();
        let path = fixture.path("source.bin");
        std::fs::write(&path, b"first").unwrap();
        let capture = fixture.path("capture.bin");
        let mut child=Command::new(std::env::current_exe().unwrap()).args(["--exact","sharing_provider::tests::private_config_and_bounded_stdin_reach_only_the_exact_child","--nocapture"]).env(CHILD,"1").env("PATH",&fixture.0).env("SCREENSHOTS_PROVIDER_SOURCE",&path).env("SCREENSHOTS_PROVIDER_CAPTURE",&capture).stdin(std::process::Stdio::null()).stdout(std::process::Stdio::null()).stderr(std::process::Stdio::null()).spawn().unwrap();
        let deadline = Instant::now() + Duration::from_secs(5);
        loop {
            if let Some(status) = child.try_wait().unwrap() {
                assert!(status.success());
                break;
            }
            if Instant::now() >= deadline {
                let _ = child.kill();
                let _ = child.wait();
                panic!("Provider config/stream fixture blocked")
            }
            std::thread::sleep(INTERVAL);
        }
        assert_eq!(std::fs::read(capture).unwrap(), b"first");
    }
}
