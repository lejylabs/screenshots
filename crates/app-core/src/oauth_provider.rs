//! Explicit native public-client OAuth. No discovery, idle runtime or automatic refresh.

use anyhow::{Context, Result, bail, ensure};
use base64::Engine;
use hmac::Mac;
use serde::{Deserialize, Serialize};
use sha2_10::{Digest, Sha256};
use std::{
    collections::{BTreeMap, HashSet},
    fs::File,
    io::{Read, Write},
    net::{TcpListener, TcpStream},
    path::{Path, PathBuf},
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};
use zeroize::{Zeroize, Zeroizing};

const PREFIX: &[u8] = b"screenshots-oauth-v1\0";
const LOGIN_LIMIT: Duration = Duration::from_secs(180);
const TICK: Duration = Duration::from_millis(25);
const BODY_LIMIT: usize = 32 * 1024;

#[derive(Clone, Copy, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
enum Flow {
    AuthorizationCode,
    DeviceCode,
}

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct Fields {
    version: u32,
    flow: Flow,
    client_id: String,
    token_endpoint: String,
    #[serde(default)]
    issuer: Option<String>,
    #[serde(default)]
    authorization_endpoint: Option<String>,
    #[serde(default)]
    device_authorization_endpoint: Option<String>,
    scopes: Vec<String>,
    credential: String,
    upload_destination: PathBuf,
    #[serde(default)]
    authorization_parameters: BTreeMap<String, String>,
    #[serde(default)]
    token_parameters: BTreeMap<String, String>,
}

/// A fully admitted profile and destination snapshot. Loading reads no credentials.
pub struct OAuthProfile {
    fields: Fields,
    upload_target: String,
    refresh_target: String,
    allow_loopback_http: bool,
}

#[derive(Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum OAuthEvent {
    Authorization {
        url: String,
        expires_in: u64,
    },
    DeviceAuthorization {
        verification_uri: String,
        user_code: String,
        expires_in: u64,
    },
}
impl Drop for OAuthEvent {
    fn drop(&mut self) {
        if let Self::DeviceAuthorization { user_code, .. } = self {
            user_code.zeroize();
        }
    }
}

#[derive(Serialize)]
pub struct OAuthOutcome {
    pub access_stored: bool,
    pub refresh_available: bool,
    pub expires_at: Option<u64>,
}

fn endpoint(text: &str, allow_loopback_http: bool) -> Result<reqwest::Url> {
    ensure!(
        (1..=2048).contains(&text.len()) && !text.chars().any(char::is_control),
        "Invalid bounded OAuth endpoint"
    );
    let url = reqwest::Url::parse(text).map_err(|_| anyhow::anyhow!("Invalid OAuth endpoint"))?;
    let loopback = url
        .host_str()
        .is_some_and(|host| matches!(host, "127.0.0.1" | "[::1]" | "::1"));
    ensure!(
        (url.scheme() == "https" || (allow_loopback_http && url.scheme() == "http" && loopback))
            && url.host_str().is_some()
            && url.username().is_empty()
            && url.password().is_none()
            && url.query().is_none()
            && url.fragment().is_none()
            && url.as_str() == text,
        "OAuth endpoints require canonical HTTPS without userinfo, query or fragment; explicit development HTTP is literal-loopback only"
    );
    Ok(url)
}

fn issuer_endpoint(text: &str, allow_loopback_http: bool) -> Result<()> {
    let url = reqwest::Url::parse(text).map_err(|_| anyhow::anyhow!("Invalid OAuth issuer"))?;
    ensure!(
        url.as_str() == text || url.as_str().strip_suffix('/') == Some(text),
        "Invalid OAuth issuer representation"
    );
    endpoint(url.as_str(), allow_loopback_http)?;
    Ok(())
}
fn opaque(value: &str, limit: usize) -> bool {
    (1..=limit).contains(&value.len()) && value.bytes().all(|byte| (0x20..=0x7e).contains(&byte))
}
fn reference(value: &str) -> bool {
    (1..=64).contains(&value.len())
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || b"_-".contains(&byte))
}
fn printable(value: &str, limit: usize) -> bool {
    (1..=limit).contains(&value.len()) && value.bytes().all(|byte| (0x21..=0x7e).contains(&byte))
}
fn scope(value: &str) -> bool {
    (1..=128).contains(&value.len())
        && value.bytes().all(|byte| {
            byte == 0x21 || (0x23..=0x5b).contains(&byte) || (0x5d..=0x7e).contains(&byte)
        })
}
fn parameters(values: &BTreeMap<String, String>) -> Result<()> {
    const RESERVED: &[&str] = &[
        "client_id",
        "client_secret",
        "response_type",
        "redirect_uri",
        "scope",
        "state",
        "code",
        "code_challenge",
        "code_challenge_method",
        "code_verifier",
        "grant_type",
        "refresh_token",
        "device_code",
        "username",
        "password",
    ];
    ensure!(
        values.len() <= 16
            && values.iter().all(|(key, value)| reference(key)
                && !RESERVED.contains(&key.as_str())
                && (1..=1024).contains(&value.len())
                && !value.chars().any(char::is_control)),
        "Invalid, reserved or oversized OAuth provider parameter"
    );
    Ok(())
}

impl OAuthProfile {
    pub fn load(path: &Path, allow_loopback_http: bool) -> Result<Self> {
        ensure!(
            path.as_os_str().as_encoded_bytes().len() <= 4096,
            "OAuth profile path exceeds 4096 bytes"
        );
        let mut options = File::options();
        options.read(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.custom_flags(libc::O_NONBLOCK);
        }
        let file = options.open(path).context("Could not open OAuth profile")?;
        let metadata = file.metadata()?;
        ensure!(
            metadata.is_file() && metadata.len() <= 16 * 1024,
            "OAuth profile must be a regular file of at most 16 KiB"
        );
        let mut bytes = Vec::with_capacity(metadata.len() as usize);
        file.take(metadata.len() + 1).read_to_end(&mut bytes)?;
        ensure!(
            bytes.len() as u64 == metadata.len(),
            "OAuth profile changed while reading"
        );
        let fields: Fields = serde_json::from_slice(&bytes)
            .map_err(|_| anyhow::anyhow!("Invalid strict version-1 OAuth profile"))?;
        ensure!(
            fields.version == 1 && opaque(&fields.client_id, 1024) && reference(&fields.credential),
            "Invalid OAuth version, client ID or credential reference"
        );
        endpoint(&fields.token_endpoint, allow_loopback_http)?;
        if let Some(issuer) = &fields.issuer {
            issuer_endpoint(issuer, allow_loopback_http)?;
        }
        match fields.flow {
            Flow::AuthorizationCode => {
                ensure!(
                    fields.device_authorization_endpoint.is_none(),
                    "Code flow cannot configure a device endpoint"
                );
                endpoint(
                    fields
                        .authorization_endpoint
                        .as_deref()
                        .context("Code flow requires an authorization endpoint")?,
                    allow_loopback_http,
                )?;
            }
            Flow::DeviceCode => {
                ensure!(
                    fields.authorization_endpoint.is_none()
                        && fields.authorization_parameters.is_empty(),
                    "Device flow cannot configure code-flow authorization fields"
                );
                endpoint(
                    fields
                        .device_authorization_endpoint
                        .as_deref()
                        .context("Device flow requires an authorization endpoint")?,
                    allow_loopback_http,
                )?;
            }
        }
        ensure!(
            fields.scopes.len() <= 32
                && fields.scopes.iter().all(|value| scope(value))
                && fields.scopes.iter().collect::<HashSet<_>>().len() == fields.scopes.len(),
            "Invalid, duplicate or oversized OAuth scopes"
        );
        parameters(&fields.authorization_parameters)?;
        parameters(&fields.token_parameters)?;
        ensure!(
            Form::new(&fields.token_parameters)?.0.len() <= 4096,
            "OAuth provider token parameters exceed the reserved 4 KiB budget"
        );
        ensure!(
            fields
                .upload_destination
                .as_os_str()
                .as_encoded_bytes()
                .len()
                <= 4096,
            "OAuth upload profile path exceeds 4096 bytes"
        );
        let destination_path = if fields.upload_destination.is_absolute() {
            fields.upload_destination.clone()
        } else {
            path.parent()
                .unwrap_or(Path::new("."))
                .join(&fields.upload_destination)
        };
        let destination = crate::UploadDestination::load(&destination_path)?;
        ensure!(
            destination.authentication() == Some(crate::UploadAuthentication::Bearer)
                && destination.credential_reference() == Some(fields.credential.as_str()),
            "OAuth requires an existing HTTPS Bearer destination with the matching credential reference"
        );
        let upload_target = destination
            .credential_target()
            .context("OAuth upload destination has no credential target")?;
        let canonical = serde_json::to_vec(&(
            &fields.flow,
            &fields.client_id,
            &fields.token_endpoint,
            &fields.issuer,
            &fields.authorization_endpoint,
            &fields.device_authorization_endpoint,
            &fields.scopes,
            &fields.authorization_parameters,
            &fields.token_parameters,
            &upload_target,
        ))?;
        let digest = Sha256::digest(canonical);
        let hash: String = digest.iter().map(|byte| format!("{byte:02x}")).collect();
        let refresh_target = format!("Screenshots/provider/oauth/{hash}/{}", fields.credential);
        Ok(Self {
            fields,
            upload_target,
            refresh_target,
            allow_loopback_http,
        })
    }

    pub fn targets(&self) -> (&str, &str) {
        (&self.upload_target, &self.refresh_target)
    }

    pub fn login(
        &self,
        check: &dyn Fn() -> Result<()>,
        emit: &mut dyn FnMut(OAuthEvent) -> Result<()>,
    ) -> Result<OAuthOutcome> {
        check()?;
        let deadline = Instant::now() + LOGIN_LIMIT;
        let response = match self.fields.flow {
            Flow::AuthorizationCode => self.code_login(deadline, check, emit)?,
            Flow::DeviceCode => self.device_login(deadline, check, emit)?,
        };
        check()?;
        persist(
            &NativeStore,
            &self.refresh_target,
            &self.upload_target,
            response,
            None,
            check,
        )
    }

    fn code_login(
        &self,
        deadline: Instant,
        check: &dyn Fn() -> Result<()>,
        emit: &mut dyn FnMut(OAuthEvent) -> Result<()>,
    ) -> Result<Token> {
        let listener = TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, 0))
            .context("Could not bind private OAuth callback")?;
        listener.set_nonblocking(true)?;
        let address = listener.local_addr()?;
        let redirect = format!("http://{address}/oauth/callback");
        let verifier = Zeroizing::new(random());
        let state = Zeroizing::new(random());
        let mut url = endpoint(
            self.fields.authorization_endpoint.as_deref().unwrap(),
            self.allow_loopback_http,
        )?;
        {
            let mut query = url.query_pairs_mut();
            query
                .append_pair("response_type", "code")
                .append_pair("client_id", &self.fields.client_id)
                .append_pair("redirect_uri", &redirect)
                .append_pair("scope", &self.fields.scopes.join(" "))
                .append_pair("state", &state)
                .append_pair("code_challenge", &challenge(&verifier))
                .append_pair("code_challenge_method", "S256");
            for (key, value) in &self.fields.authorization_parameters {
                query.append_pair(key, value);
            }
        }
        ensure!(
            url.as_str().len() <= 8192,
            "OAuth authorization URL exceeds 8 KiB"
        );
        emit(OAuthEvent::Authorization {
            url: url.into(),
            expires_in: LOGIN_LIMIT.as_secs(),
        })?;
        let mut code = None;
        let mut admitted = 0;
        while code.is_none() {
            active(check, deadline)?;
            match listener.accept() {
                Ok((mut stream, peer)) => {
                    admitted += 1;
                    ensure!(
                        admitted <= 16 && peer.ip().is_loopback(),
                        "OAuth callback connection budget exceeded"
                    );
                    let result = callback(
                        &mut stream,
                        &address.to_string(),
                        &state,
                        self.fields.issuer.as_deref(),
                        deadline,
                        check,
                    );
                    let success = result.is_ok();
                    let message = if success {
                        "Authorization received. You may close this window."
                    } else {
                        "Authorization callback refused."
                    };
                    stream.set_write_timeout(Some(Duration::from_millis(250)))?;
                    let response = format!(
                        "HTTP/1.1 {}\r\nContent-Type: text/plain; charset=utf-8\r\nContent-Length: {}\r\nConnection: close\r\nCache-Control: no-store\r\nReferrer-Policy: no-referrer\r\n\r\n{message}",
                        if success { "200 OK" } else { "400 Bad Request" },
                        message.len()
                    );
                    let _ = stream.write_all(response.as_bytes());
                    match result {
                        Ok(value) => code = Some(value),
                        Err(error) if error.downcast_ref::<CallbackDenied>().is_some() => {
                            return Err(error);
                        }
                        Err(_) => {}
                    }
                }
                Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                    std::thread::sleep(TICK)
                }
                Err(_) => bail!("OAuth callback listener failed"),
            }
        }
        drop(listener);
        let code = code.unwrap();
        let mut form = Form::new(&self.fields.token_parameters)?;
        for (name, value) in [
            ("grant_type", "authorization_code"),
            ("client_id", &self.fields.client_id),
            ("code", &code),
            ("redirect_uri", &redirect),
            ("code_verifier", &verifier),
        ] {
            form.add(name, value)?;
        }
        let body = Http::new()?.post(&self.fields.token_endpoint, form, deadline, check, false)?;
        Token::decode(&body)
    }

    fn device_login(
        &self,
        deadline: Instant,
        check: &dyn Fn() -> Result<()>,
        emit: &mut dyn FnMut(OAuthEvent) -> Result<()>,
    ) -> Result<Token> {
        let http = Http::new()?;
        let mut form = Form::new(&BTreeMap::new())?;
        form.add("client_id", &self.fields.client_id)?;
        form.add("scope", &self.fields.scopes.join(" "))?;
        let body = http.post(
            self.fields
                .device_authorization_endpoint
                .as_deref()
                .unwrap(),
            form,
            deadline,
            check,
            false,
        )?;
        let grant: DeviceGrant = serde_json::from_slice(&body)
            .map_err(|_| anyhow::anyhow!("Invalid OAuth device authorization response"))?;
        ensure!(
            opaque(&grant.device_code, 2048)
                && opaque(&grant.user_code, 128)
                && (1..=86400).contains(&grant.expires_in),
            "Invalid or oversized OAuth device authorization response"
        );
        let mut verification = reqwest::Url::parse(&grant.verification_uri)
            .map_err(|_| anyhow::anyhow!("Invalid OAuth verification URI"))?;
        ensure!(
            grant.verification_uri.len() <= 2048
                && verification.as_str() == grant.verification_uri
                && verification.fragment().is_none(),
            "Invalid OAuth verification URI"
        );
        verification.set_query(None);
        endpoint(verification.as_str(), self.allow_loopback_http)?;
        let mut interval = grant.interval.unwrap_or(5);
        ensure!(
            (1..=60).contains(&interval),
            "Invalid OAuth device polling interval"
        );
        let deadline = deadline.min(Instant::now() + Duration::from_secs(grant.expires_in));
        emit(OAuthEvent::DeviceAuthorization {
            verification_uri: grant.verification_uri.clone(),
            user_code: grant.user_code.clone(),
            expires_in: deadline.saturating_duration_since(Instant::now()).as_secs(),
        })?;
        loop {
            wait_until(
                Instant::now() + Duration::from_secs(interval),
                deadline,
                check,
            )?;
            let mut form = Form::new(&self.fields.token_parameters)?;
            form.add("grant_type", "urn:ietf:params:oauth:grant-type:device_code")?;
            form.add("device_code", &grant.device_code)?;
            form.add("client_id", &self.fields.client_id)?;
            let body = http.post(&self.fields.token_endpoint, form, deadline, check, true)?;
            let status: ErrorResponse = serde_json::from_slice(&body)
                .map_err(|_| anyhow::anyhow!("Invalid OAuth token response"))?;
            match status.error.as_deref() {
                Some("authorization_pending") => {}
                Some("slow_down") => {
                    interval = interval
                        .checked_add(5)
                        .filter(|value| *value <= 60)
                        .context("OAuth device polling interval exceeded its bound")?;
                }
                Some(_) => bail!("OAuth device authorization was denied or expired"),
                None => return Token::decode(&body),
            }
        }
    }

    pub fn refresh(&self, check: &dyn Fn() -> Result<()>) -> Result<OAuthOutcome> {
        check()?;
        let bytes = crate::read_provider_credentials(&self.refresh_target)?;
        check()?;
        let previous = Stored::decode(&bytes)?;
        let refresh = previous
            .refresh_token
            .as_deref()
            .context("This OAuth connection has no refresh token; log in explicitly")?;
        let mut form = Form::new(&self.fields.token_parameters)?;
        form.add("grant_type", "refresh_token")?;
        form.add("client_id", &self.fields.client_id)?;
        form.add("refresh_token", refresh)?;
        let body = Http::new()?.post(
            &self.fields.token_endpoint,
            form,
            Instant::now() + Duration::from_secs(60),
            check,
            false,
        )?;
        let response = Token::decode(&body)?;
        persist(
            &NativeStore,
            &self.refresh_target,
            &self.upload_target,
            response,
            Some(&previous),
            check,
        )
    }

    pub fn remove(&self, check: &dyn Fn() -> Result<()>) -> Result<()> {
        check()?;
        let refresh = crate::remove_provider_credentials(&self.refresh_target);
        let access = crate::upload_credentials::delete(&self.upload_target);
        ensure!(
            refresh.is_ok() && access.is_ok(),
            "OAuth removal incomplete: refresh state {} and access credential {}; no other entries were touched",
            if refresh.is_ok() {
                "removed"
            } else {
                "could not be confirmed removed"
            },
            if access.is_ok() {
                "removed"
            } else {
                "could not be confirmed removed"
            }
        );
        Ok(())
    }
}

fn active(check: &dyn Fn() -> Result<()>, deadline: Instant) -> Result<()> {
    check()?;
    ensure!(
        Instant::now() < deadline,
        "OAuth operation exceeded its deadline"
    );
    Ok(())
}
fn wait_until(until: Instant, deadline: Instant, check: &dyn Fn() -> Result<()>) -> Result<()> {
    loop {
        active(check, deadline)?;
        let now = Instant::now();
        if now >= until {
            return Ok(());
        }
        std::thread::sleep(TICK.min(until.saturating_duration_since(now)));
    }
}
fn random() -> String {
    // Three OS-random UUIDs provide 366 random bits within PKCE's 128-char cap.
    format!(
        "{}{}{}",
        uuid::Uuid::new_v4().simple(),
        uuid::Uuid::new_v4().simple(),
        uuid::Uuid::new_v4().simple()
    )
}

fn challenge(verifier: &str) -> String {
    base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(Sha256::digest(verifier.as_bytes()))
}
fn same_state(expected: &str, actual: &str) -> bool {
    if expected.len() != actual.len() {
        return false;
    }
    // Use the existing MAC verifier's constant-time comparison rather than ==.
    let tag = |state: &str| {
        let mut mac = hmac::Hmac::<Sha256>::new_from_slice(state.as_bytes())
            .expect("HMAC accepts every key length");
        mac.update(b"screenshots-oauth-callback-state-v1");
        mac
    };
    let expected = tag(expected).finalize().into_bytes();
    tag(actual).verify_slice(&expected).is_ok()
}

fn callback(
    stream: &mut TcpStream,
    host: &str,
    state: &str,
    issuer: Option<&str>,
    deadline: Instant,
    check: &dyn Fn() -> Result<()>,
) -> Result<Zeroizing<String>> {
    stream.set_read_timeout(Some(Duration::from_millis(250)))?;
    let local_deadline = deadline.min(Instant::now() + Duration::from_secs(2));
    let mut bytes = Zeroizing::new(Vec::with_capacity(8192));
    while !bytes.ends_with(b"\r\n\r\n") {
        active(check, local_deadline)?;
        ensure!(bytes.len() < 8192, "OAuth callback exceeds 8 KiB");
        let mut byte = [0u8; 1];
        match stream.read(&mut byte) {
            Ok(0) => bail!("OAuth callback ended early"),
            Ok(_) => bytes.push(byte[0]),
            Err(error)
                if matches!(
                    error.kind(),
                    std::io::ErrorKind::TimedOut | std::io::ErrorKind::WouldBlock
                ) => {}
            Err(_) => bail!("OAuth callback read failed"),
        }
    }
    parse_callback(&bytes, host, state, issuer)
}
#[derive(Debug)]
struct CallbackDenied;
impl std::fmt::Display for CallbackDenied {
    fn fmt(&self, out: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        out.write_str("OAuth authorization was denied")
    }
}
impl std::error::Error for CallbackDenied {}
fn parse_callback(
    bytes: &[u8],
    host: &str,
    state: &str,
    issuer: Option<&str>,
) -> Result<Zeroizing<String>> {
    ensure!(
        bytes.len() <= 8192 && bytes.is_ascii(),
        "Invalid OAuth callback headers"
    );
    let text = std::str::from_utf8(bytes).map_err(|_| anyhow::anyhow!("Invalid OAuth callback"))?;
    let mut lines = text.split("\r\n");
    let request = lines.next().unwrap_or_default();
    let parts: Vec<_> = request.split(' ').collect();
    ensure!(
        parts.len() == 3 && parts[0] == "GET" && parts[2] == "HTTP/1.1",
        "OAuth callback requires a local GET request"
    );
    let mut hosts = 0;
    for line in lines.take_while(|line| !line.is_empty()) {
        let (key, value) = line
            .split_once(':')
            .context("Invalid OAuth callback header")?;
        ensure!(
            !key.is_empty()
                && key.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-')
                && !value.chars().any(|ch| ch.is_control() && ch != '\t'),
            "Invalid OAuth callback header"
        );
        if key.eq_ignore_ascii_case("host") {
            hosts += 1;
            ensure!(value.trim() == host, "OAuth callback Host mismatch");
        }
        ensure!(
            !key.eq_ignore_ascii_case("content-length")
                && !key.eq_ignore_ascii_case("transfer-encoding"),
            "OAuth callback body is forbidden"
        );
    }
    ensure!(
        hosts == 1,
        "OAuth callback requires exactly one matching Host"
    );
    let query = parts[1]
        .strip_prefix("/oauth/callback?")
        .context("OAuth callback path mismatch")?;
    let raw = query.as_bytes();
    for (i, byte) in raw.iter().enumerate() {
        if *byte == b'%' {
            ensure!(
                raw.get(i + 1).is_some_and(u8::is_ascii_hexdigit)
                    && raw.get(i + 2).is_some_and(u8::is_ascii_hexdigit),
                "Malformed OAuth callback encoding"
            );
        }
    }
    ensure!(!query.contains('#'), "OAuth callback fragment is forbidden");
    fn decode(value: &str) -> Result<Zeroizing<String>> {
        let mut bytes = Zeroizing::new(Vec::with_capacity(value.len()));
        let mut input = value.bytes();
        while let Some(byte) = input.next() {
            match byte {
                b'+' => bytes.push(b' '),
                b'%' => {
                    let a = input
                        .next()
                        .and_then(|byte| (byte as char).to_digit(16))
                        .context("Malformed OAuth callback encoding")?;
                    let b = input
                        .next()
                        .and_then(|byte| (byte as char).to_digit(16))
                        .context("Malformed OAuth callback encoding")?;
                    bytes.push((a * 16 + b) as u8);
                }
                _ => bytes.push(byte),
            }
        }
        Ok(Zeroizing::new(
            std::str::from_utf8(&bytes)
                .map_err(|_| anyhow::anyhow!("Invalid OAuth callback UTF-8"))?
                .to_owned(),
        ))
    }
    let mut seen = HashSet::new();
    let mut code = None;
    let mut actual_state = None;
    let mut actual_issuer = None;
    let mut denied = false;
    for (index, pair) in query.split('&').enumerate() {
        ensure!(index < 32, "OAuth callback parameter budget exceeded");
        let (key, value) = pair
            .split_once('=')
            .context("Invalid OAuth callback parameter")?;
        let key = decode(key)?;
        let value = decode(value)?;
        ensure!(
            printable(&key, 64)
                && value.len() <= 2048
                && value.bytes().all(|byte| (0x20..=0x7e).contains(&byte)),
            "Invalid bounded OAuth callback parameter"
        );
        ensure!(
            seen.insert(key.to_string()),
            "Duplicate OAuth callback parameter"
        );
        match key.as_str() {
            "code" => {
                ensure!(opaque(&value, 2048), "Invalid OAuth authorization code");
                code = Some(value);
            }
            "state" => actual_state = Some(value),
            "error" => {
                ensure!(printable(&value, 128), "Invalid OAuth callback error");
                denied = true;
            }
            "iss" => actual_issuer = Some(value),
            _ => {} // OAuth clients ignore bounded extension response parameters.
        }
    }
    ensure!(
        actual_state
            .as_deref()
            .is_some_and(|actual| same_state(state, actual)),
        "OAuth callback state mismatch"
    );
    ensure!(
        actual_issuer.as_deref().map(String::as_str) == issuer,
        "OAuth callback issuer mismatch or unconfigured issuer"
    );
    if denied {
        ensure!(
            code.is_none(),
            "OAuth callback contains both a code and an error"
        );
        return Err(CallbackDenied.into());
    }
    code.context("OAuth callback has no authorization code")
}

struct Form(Zeroizing<Vec<u8>>);
impl Form {
    fn new(parameters: &BTreeMap<String, String>) -> Result<Self> {
        let mut form = Self(Zeroizing::new(Vec::with_capacity(16 * 1024)));
        for (key, value) in parameters {
            form.add(key, value)?;
        }
        Ok(form)
    }
    fn add(&mut self, name: &str, value: &str) -> Result<()> {
        fn encode(output: &mut Vec<u8>, text: &str) {
            const HEX: &[u8] = b"0123456789ABCDEF";
            for byte in text.bytes() {
                match byte {
                    b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'*' => {
                        output.push(byte)
                    }
                    b' ' => output.push(b'+'),
                    _ => output.extend_from_slice(&[
                        b'%',
                        HEX[(byte >> 4) as usize],
                        HEX[(byte & 15) as usize],
                    ]),
                }
            }
        }
        ensure!(
            self.0
                .len()
                .saturating_add(name.len().saturating_add(value.len()).saturating_mul(3))
                .saturating_add(2)
                <= 16 * 1024,
            "OAuth request exceeds 16 KiB"
        );
        if !self.0.is_empty() {
            self.0.push(b'&');
        }
        encode(&mut self.0, name);
        self.0.push(b'=');
        encode(&mut self.0, value);
        Ok(())
    }
}

struct Http {
    runtime: tokio::runtime::Runtime,
    client: reqwest::Client,
}
impl Http {
    fn new() -> Result<Self> {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .max_blocking_threads(1)
            .build()?;
        let client = reqwest::Client::builder()
            .redirect(reqwest::redirect::Policy::none())
            .retry(reqwest::retry::never())
            .connect_timeout(Duration::from_secs(5))
            .timeout(Duration::from_secs(60))
            .http1_only()
            .pool_max_idle_per_host(0)
            .no_proxy()
            .tls_sslkeylogfile(false)
            .no_gzip()
            .no_brotli()
            .no_deflate()
            .no_zstd()
            .build()
            .map_err(|_| anyhow::anyhow!("Could not initialize OAuth HTTPS transport"))?;
        Ok(Self { runtime, client })
    }
    fn post(
        &self,
        url: &str,
        form: Form,
        deadline: Instant,
        check: &dyn Fn() -> Result<()>,
        allow_pending: bool,
    ) -> Result<Zeroizing<Vec<u8>>> {
        active(check, deadline)?;
        let deadline = deadline.min(Instant::now() + Duration::from_secs(60));
        self.runtime.block_on(async {
            let request = async {
                let mut response = self.client.post(url).header(reqwest::header::CONTENT_TYPE,"application/x-www-form-urlencoded").header(reqwest::header::ACCEPT,"application/json").header(reqwest::header::ACCEPT_ENCODING,"identity").body(form.0.to_vec()).send().await.map_err(|_| anyhow::anyhow!("OAuth token request failed; no retry was attempted"))?;
                let status=response.status().as_u16();
                ensure!(status==200 || (allow_pending && status==400),"OAuth endpoint returned unexpected HTTP status {}",status);
                ensure!(response.content_length().is_none_or(|length| length <= BODY_LIMIT as u64), "OAuth response exceeds 32 KiB");
                let mut bytes = Zeroizing::new(Vec::with_capacity(BODY_LIMIT));
                while let Some(chunk) = response.chunk().await.map_err(|_| anyhow::anyhow!("OAuth response read failed"))? { ensure!(bytes.len().saturating_add(chunk.len()) <= BODY_LIMIT, "OAuth response exceeds 32 KiB"); bytes.extend_from_slice(&chunk); }
                let error:ErrorResponse=serde_json::from_slice(&bytes).map_err(|_|anyhow::anyhow!("Invalid OAuth JSON response"))?;
                ensure!((status==200 && error.error.is_none()) || (allow_pending && status==400 && matches!(error.error.as_deref(),Some("authorization_pending"|"slow_down"))),"OAuth authorization request was denied or expired");
                Ok(bytes)
            };
            tokio::pin!(request);
            loop { active(check,deadline)?; tokio::select! { value = &mut request => return value, _ = tokio::time::sleep(TICK) => {} } }
        })
    }
}

#[derive(Deserialize)]
struct DeviceGrant {
    device_code: String,
    user_code: String,
    verification_uri: String,
    expires_in: u64,
    interval: Option<u64>,
}
impl Drop for DeviceGrant {
    fn drop(&mut self) {
        self.device_code.zeroize();
        self.user_code.zeroize();
    }
}
#[derive(Deserialize)]
struct ErrorResponse {
    error: Option<String>,
}
#[derive(Deserialize)]
struct Token {
    access_token: String,
    token_type: String,
    refresh_token: Option<String>,
    expires_in: Option<u64>,
}
impl Token {
    fn decode(bytes: &[u8]) -> Result<Self> {
        ensure!(
            bytes.len() <= BODY_LIMIT,
            "OAuth token response exceeds 32 KiB"
        );
        let token: Self = serde_json::from_slice(bytes)
            .map_err(|_| anyhow::anyhow!("OAuth token response was rejected or invalid"))?;
        ensure!(
            token.token_type.eq_ignore_ascii_case("Bearer")
                && (1..=1024).contains(&token.access_token.len())
                && token
                    .access_token
                    .bytes()
                    .all(|byte| byte.is_ascii_alphanumeric() || b"-._~+/=".contains(&byte))
                && token
                    .refresh_token
                    .as_deref()
                    .is_none_or(|value| opaque(value, 1024))
                && token
                    .expires_in
                    .is_none_or(|value| (1..=31_536_000).contains(&value)),
            "Invalid OAuth bearer token, refresh token or expiry"
        );
        Ok(token)
    }
}
impl Drop for Token {
    fn drop(&mut self) {
        self.access_token.zeroize();
        if let Some(value) = &mut self.refresh_token {
            value.zeroize();
        }
    }
}

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct Stored {
    version: u32,
    refresh_token: Option<String>,
    expires_at: Option<u64>,
}
impl Stored {
    fn decode(bytes: &[u8]) -> Result<Self> {
        ensure!(bytes.len() <= 2560, "Stored OAuth state exceeds its limit");
        let json = bytes
            .strip_prefix(PREFIX)
            .context("Unsupported stored OAuth state version")?;
        let value: Self = serde_json::from_slice(json)
            .map_err(|_| anyhow::anyhow!("Invalid stored OAuth state"))?;
        ensure!(
            value.version == 1
                && value
                    .refresh_token
                    .as_deref()
                    .is_none_or(|token| opaque(token, 1024)),
            "Invalid stored OAuth state"
        );
        Ok(value)
    }
    fn payload(&self) -> Result<Zeroizing<Vec<u8>>> {
        let mut bytes = Zeroizing::new(PREFIX.to_vec());
        serde_json::to_writer(&mut *bytes, self)?;
        ensure!(
            bytes.len() <= 2560,
            "OAuth state exceeds its native storage limit"
        );
        Ok(bytes)
    }
}
impl Drop for Stored {
    fn drop(&mut self) {
        if let Some(value) = &mut self.refresh_token {
            value.zeroize();
        }
    }
}
trait Store {
    fn state(&self, target: &str, payload: &[u8]) -> Result<()>;
    fn access(&self, target: &str, token: &str) -> Result<()>;
}
struct NativeStore;
impl Store for NativeStore {
    fn state(&self, target: &str, payload: &[u8]) -> Result<()> {
        crate::save_provider_credentials(target, payload)
    }
    fn access(&self, target: &str, token: &str) -> Result<()> {
        crate::upload_credentials::save(target, crate::UploadAuthentication::Bearer, "", token)
    }
}
fn persist(
    store: &impl Store,
    refresh_target: &str,
    upload_target: &str,
    response: Token,
    previous: Option<&Stored>,
    check: &dyn Fn() -> Result<()>,
) -> Result<OAuthOutcome> {
    check()?;
    let expires_at = response
        .expires_in
        .map(|seconds| {
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .context("System time is before Unix epoch")?
                .as_secs()
                .checked_add(seconds)
                .context("OAuth expiry overflow")
        })
        .transpose()?;
    let state = Stored {
        version: 1,
        refresh_token: response
            .refresh_token
            .clone()
            .or_else(|| previous.and_then(|state| state.refresh_token.clone())),
        expires_at,
    };
    let payload = state.payload()?;
    store.state(refresh_target, &payload).map_err(|_| {
        anyhow::anyhow!(
            "OAuth refresh state could not be confirmed stored; access credential was not updated"
        )
    })?;
    check().context("OAuth refresh state was stored; access credential was not updated")?;
    store.access(upload_target,&response.access_token).map_err(|_|anyhow::anyhow!("OAuth refresh state was stored; access credential storage could not be confirmed; refresh or remove this connection explicitly"))?;
    check().context("OAuth credentials were stored but the operation was canceled")?;
    Ok(OAuthOutcome {
        access_stored: true,
        refresh_available: state.refresh_token.is_some(),
        expires_at,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{
        cell::RefCell,
        sync::{Arc, Mutex},
    };

    struct Fixture(PathBuf);
    impl Fixture {
        fn new() -> Self {
            let path =
                std::env::temp_dir().join(format!("screenshots-oauth-{}", uuid::Uuid::new_v4()));
            std::fs::create_dir(&path).unwrap();
            Self(path)
        }
        fn path(&self, name: &str) -> PathBuf {
            self.0.join(name)
        }
        fn profile(&self, value: serde_json::Value, development: bool) -> Result<OAuthProfile> {
            let path = self.path("oauth.json");
            std::fs::write(&path, serde_json::to_vec(&value)?).unwrap();
            OAuthProfile::load(&path, development)
        }
        fn destination(&self) {
            let value = crate::UploadDestination::manual(
                "https://example.invalid/upload".into(),
                crate::UploadMethod::Post,
                "file".into(),
                Some("/url".into()),
                false,
            )
            .unwrap()
            .with_authentication("fixture".into(), crate::UploadAuthentication::Bearer)
            .unwrap();
            std::fs::write(
                self.path("upload.json"),
                serde_json::to_vec(&value).unwrap(),
            )
            .unwrap();
        }
    }
    impl Drop for Fixture {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }
    fn profile() -> serde_json::Value {
        serde_json::json!({"version":1,"flow":"authorization_code","client_id":"synthetic-client","token_endpoint":"https://identity.invalid/token","authorization_endpoint":"https://identity.invalid/authorize","scopes":["upload","offline_access"],"credential":"fixture","upload_destination":"upload.json"})
    }
    fn token(refresh: Option<&str>) -> Token {
        Token::decode(&serde_json::to_vec(&serde_json::json!({"access_token":"synthetic-token","token_type":"Bearer","refresh_token":refresh,"expires_in":3600})).unwrap()).unwrap()
    }

    #[test]
    fn pkce_matches_rfc7636_and_random_values_are_independent() {
        assert_eq!(
            challenge("dBjftJeZ4CVP-mB92K27uhbUJU1p1r_wW1gFWFOEjXk"),
            "E9Melhoa2OwvFrEMTJguCHaoeK1t8URWbuGJSstw-cM"
        );
        let values: (String, String) = (random(), random());
        assert_eq!(values.0.len(), 96);
        assert_ne!(values.0, values.1);
        assert!(values.0.bytes().all(|byte| byte.is_ascii_hexdigit()));
    }
    #[test]
    fn profiles_bind_endpoints_client_scopes_and_existing_https_bearer_target() {
        let fixture = Fixture::new();
        fixture.destination();
        let original = fixture.profile(profile(), false).unwrap();
        assert_eq!(
            original.targets().0,
            "Screenshots/upload/https://example.invalid/fixture"
        );
        assert!(
            original
                .targets()
                .1
                .starts_with("Screenshots/provider/oauth/")
        );
        for (key, bad) in [
            ("client_id", "other".into()),
            ("token_endpoint", "https://other.invalid/token".into()),
            ("scopes", serde_json::json!(["upload"])),
            ("issuer", "https://identity.invalid/".into()),
        ] {
            let mut changed = profile();
            changed[key] = bad;
            assert_ne!(
                fixture.profile(changed, false).unwrap().targets().1,
                original.targets().1
            );
        }
        for (key, bad) in [
            ("version", 2.into()),
            ("client_id", "".into()),
            ("credential", "different".into()),
            ("token_endpoint", "http://identity.invalid/token".into()),
            (
                "authorization_endpoint",
                "https://user:password@identity.invalid/authorize".into(),
            ),
            (
                "token_endpoint",
                "https://identity.invalid/token?api_key=plaintext".into(),
            ),
            (
                "authorization_parameters",
                serde_json::json!({"client_secret":"forbidden"}),
            ),
            ("scopes", serde_json::json!(["duplicate", "duplicate"])),
            ("scopes", serde_json::json!(["bad scope"])),
            ("unknown", true.into()),
        ] {
            let mut invalid = profile();
            invalid[key] = bad;
            assert!(fixture.profile(invalid, false).is_err(), "{key}");
        }
        let mut local = profile();
        local["token_endpoint"] = "http://127.0.0.1:5678/token".into();
        assert!(fixture.profile(local.clone(), false).is_err());
        assert!(fixture.profile(local, true).is_ok());
        for bad in [
            "http://localhost:5678/token",
            "http://127.1:5678/token",
            "http://192.0.2.1/token",
        ] {
            let mut local = profile();
            local["token_endpoint"] = bad.into();
            assert!(fixture.profile(local, true).is_err());
        }
        std::fs::write(fixture.path("upload.json"), b"changed after preflight").unwrap();
        assert_eq!(
            original.targets().0,
            "Screenshots/upload/https://example.invalid/fixture"
        );
    }
    fn request(query: &str) -> Vec<u8> {
        format!("GET /oauth/callback?{query} HTTP/1.1\r\nHost: 127.0.0.1:1234\r\n\r\n").into_bytes()
    }
    #[test]
    fn callbacks_require_exact_host_path_state_issuer_and_unambiguous_encoding() {
        let valid = request("code=synthetic%2Bcode&state=expected");
        assert_eq!(
            &**parse_callback(&valid, "127.0.0.1:1234", "expected", None).unwrap(),
            "synthetic+code"
        );
        for query in [
            "code=x&state=wrong",
            "code=x&state=expected&state=expected",
            "code=x&state=expected&%73tate=expected",
            "code=x%QQ&state=expected",
            "code=x%2&state=expected",
            "code=x&state=expected#fragment",
            "code=x&state=expected&iss=https%3A%2F%2Fevil.invalid%2F",
            "error=access_denied&code=x&state=expected",
        ] {
            assert!(
                parse_callback(&request(query), "127.0.0.1:1234", "expected", None).is_err(),
                "{query}"
            );
        }
        let denied = parse_callback(
            &request("state=expected&error=access_denied&error_description=synthetic+denial"),
            "127.0.0.1:1234",
            "expected",
            None,
        )
        .err()
        .unwrap();
        assert!(denied.downcast_ref::<CallbackDenied>().is_some());
        let issuer = request("code=x&state=expected&iss=https%3A%2F%2Fidentity.invalid%2F");
        assert!(
            parse_callback(
                &issuer,
                "127.0.0.1:1234",
                "expected",
                Some("https://identity.invalid/")
            )
            .is_ok()
        );
        assert!(
            parse_callback(
                &valid,
                "127.0.0.1:1234",
                "expected",
                Some("https://identity.invalid/")
            )
            .is_err()
        );
        for invalid in [
            String::from_utf8(valid.clone())
                .unwrap()
                .replace("Host:", "Host: bad\r\nHost:"),
            String::from_utf8(valid.clone())
                .unwrap()
                .replace("Host:", "Content-Length: 1\r\nHost:"),
            String::from_utf8(valid.clone())
                .unwrap()
                .replace("/oauth/callback?", "/other?"),
            String::from_utf8(valid).unwrap().replace("GET ", "POST "),
        ] {
            assert!(
                parse_callback(invalid.as_bytes(), "127.0.0.1:1234", "expected", None).is_err()
            );
        }
    }
    #[test]
    fn tokens_and_native_refresh_payloads_fit_all_platform_limits() {
        let valid = token(Some("synthetic-refresh"));
        assert!(valid.expires_in.is_some());
        for (key, bad) in [
            ("access_token", "bad token".into()),
            ("access_token", "x".repeat(1025).into()),
            ("token_type", "Basic".into()),
            ("refresh_token", "bad\nrefresh".into()),
            ("expires_in", 0.into()),
            ("expires_in", 31_536_001_u64.into()),
        ] {
            let mut value = serde_json::json!({"access_token":"synthetic","token_type":"Bearer","refresh_token":"synthetic-refresh","expires_in":3600});
            value[key] = bad;
            assert!(
                Token::decode(&serde_json::to_vec(&value).unwrap()).is_err(),
                "{key}"
            );
        }
        let state = Stored {
            version: 1,
            refresh_token: Some("\\\"".repeat(512)),
            expires_at: Some(123),
        };
        let bytes = state.payload().unwrap();
        assert!(bytes.len() <= 2560);
        assert_eq!(
            Stored::decode(&bytes).unwrap().refresh_token,
            state.refresh_token
        );
        assert!(Stored::decode(b"wrong-version").is_err());
        assert!(Token::decode(&vec![b' '; BODY_LIMIT + 1]).is_err());
    }
    struct MemoryStore {
        events: RefCell<Vec<&'static str>>,
        states: RefCell<Vec<Vec<u8>>>,
        fail_state: bool,
        fail_access: bool,
    }
    impl MemoryStore {
        fn new() -> Self {
            Self {
                events: RefCell::new(Vec::new()),
                states: RefCell::new(Vec::new()),
                fail_state: false,
                fail_access: false,
            }
        }
    }
    impl Store for MemoryStore {
        fn state(&self, _: &str, payload: &[u8]) -> Result<()> {
            self.events.borrow_mut().push("refresh");
            ensure!(!self.fail_state, "synthetic store failure");
            self.states.borrow_mut().push(payload.to_vec());
            Ok(())
        }
        fn access(&self, _: &str, token: &str) -> Result<()> {
            self.events.borrow_mut().push("access");
            assert_eq!(token, "synthetic-token");
            ensure!(!self.fail_access, "synthetic access failure");
            Ok(())
        }
    }
    #[test]
    fn refresh_rotation_omission_partial_storage_and_cancellation_preserve_recoverable_state() {
        let store = MemoryStore::new();
        let previous = Stored {
            version: 1,
            refresh_token: Some("previous-refresh".into()),
            expires_at: None,
        };
        let outcome = persist(
            &store,
            "refresh",
            "access",
            token(None),
            Some(&previous),
            &|| Ok(()),
        )
        .unwrap();
        assert!(outcome.access_stored && outcome.refresh_available);
        assert_eq!(store.events.borrow().as_slice(), ["refresh", "access"]);
        assert_eq!(
            Stored::decode(&store.states.borrow()[0])
                .unwrap()
                .refresh_token
                .as_deref(),
            Some("previous-refresh")
        );
        let rotated = MemoryStore::new();
        persist(
            &rotated,
            "refresh",
            "access",
            token(Some("rotated-refresh")),
            Some(&previous),
            &|| Ok(()),
        )
        .unwrap();
        assert_eq!(
            Stored::decode(&rotated.states.borrow()[0])
                .unwrap()
                .refresh_token
                .as_deref(),
            Some("rotated-refresh")
        );
        let failed = MemoryStore {
            fail_state: true,
            ..MemoryStore::new()
        };
        assert!(persist(&failed, "refresh", "access", token(None), None, &|| Ok(())).is_err());
        assert_eq!(failed.events.borrow().as_slice(), ["refresh"]);
        let failed = MemoryStore {
            fail_access: true,
            ..MemoryStore::new()
        };
        assert!(
            persist(
                &failed,
                "refresh",
                "access",
                token(Some("recoverable")),
                None,
                &|| Ok(())
            )
            .err()
            .unwrap()
            .to_string()
            .contains("refresh state was stored")
        );
        assert_eq!(
            Stored::decode(&failed.states.borrow()[0])
                .unwrap()
                .refresh_token
                .as_deref(),
            Some("recoverable")
        );
        let canceled = MemoryStore::new();
        let checks = std::cell::Cell::new(0);
        assert!(
            persist(
                &canceled,
                "refresh",
                "access",
                token(Some("recoverable")),
                None,
                &|| {
                    checks.set(checks.get() + 1);
                    ensure!(checks.get() < 2, "outer canceled");
                    Ok(())
                }
            )
            .is_err()
        );
        assert_eq!(canceled.events.borrow().as_slice(), ["refresh"]);
    }
    #[test]
    fn form_encoding_is_bounded_and_round_trips_without_protocol_parameter_injection() {
        let mut form = Form::new(&BTreeMap::new()).unwrap();
        form.add("code", "synthetic+&= café").unwrap();
        assert_eq!(&*form.0, b"code=synthetic%2B%26%3D+caf%C3%A9");
        assert!(form.add("oversized", &"x".repeat(16 * 1024)).is_err());
    }

    fn identity_server(
        replies: Vec<(u16, serde_json::Value)>,
    ) -> (String, std::thread::JoinHandle<Vec<String>>) {
        let listener = TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, 0)).unwrap();
        listener.set_nonblocking(true).unwrap();
        let origin = format!("http://{}", listener.local_addr().unwrap());
        let thread = std::thread::spawn(move || {
            let mut requests = Vec::new();
            let deadline = Instant::now() + Duration::from_secs(30);
            for (status, value) in replies {
                let mut stream = loop {
                    match listener.accept() {
                        Ok((stream, _)) => break stream,
                        Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                            assert!(Instant::now() < deadline);
                            std::thread::sleep(TICK);
                        }
                        Err(error) => panic!("Synthetic identity listener: {error}"),
                    }
                };
                stream
                    .set_read_timeout(Some(Duration::from_secs(5)))
                    .unwrap();
                stream
                    .set_write_timeout(Some(Duration::from_secs(5)))
                    .unwrap();
                let mut header = Vec::new();
                while !header.ends_with(b"\r\n\r\n") {
                    assert!(header.len() < 8192);
                    let mut byte = [0u8; 1];
                    stream.read_exact(&mut byte).unwrap();
                    header.push(byte[0]);
                }
                let header = String::from_utf8(header).unwrap();
                let length = header
                    .lines()
                    .find_map(|line| {
                        line.to_ascii_lowercase()
                            .strip_prefix("content-length:")
                            .map(|value| value.trim().parse::<usize>().unwrap())
                    })
                    .unwrap();
                assert!(length <= 16 * 1024);
                let mut body = vec![0; length];
                stream.read_exact(&mut body).unwrap();
                requests.push(String::from_utf8(body).unwrap());
                let bytes = serde_json::to_vec(&value).unwrap();
                write!(stream,"HTTP/1.1 {status} Synthetic\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",bytes.len()).unwrap();
                stream.write_all(&bytes).unwrap();
            }
            requests
        });
        (origin, thread)
    }
    #[test]
    #[ignore = "Explicit local HTTP/SSH-free OAuth protocol E2E"]
    fn actual_pkce_exchange_validates_callback_and_sends_matching_verifier() {
        let (origin, server) = identity_server(vec![(
            200,
            serde_json::json!({"access_token":"synthetic-token","token_type":"Bearer","refresh_token":"synthetic-refresh","expires_in":3600}),
        )]);
        let fixture = Fixture::new();
        fixture.destination();
        let mut value = profile();
        value["token_endpoint"] = format!("{origin}/token").into();
        value["authorization_endpoint"] = format!("{origin}/authorize").into();
        let profile = fixture.profile(value, true).unwrap();
        let expected = Arc::new(Mutex::new(String::new()));
        let callback_thread = Arc::new(Mutex::new(None));
        let response=profile.code_login(Instant::now()+Duration::from_secs(10),&||Ok(()),&mut|event|{let OAuthEvent::Authorization{ref url,..}=event else{unreachable!()};let url=reqwest::Url::parse(url).unwrap();let params=url.query_pairs().collect::<BTreeMap<_,_>>();assert_eq!(params["code_challenge_method"],"S256");*expected.lock().unwrap()=params["code_challenge"].to_string();let redirect=reqwest::Url::parse(&params["redirect_uri"]).unwrap();let state=params["state"].to_string();*callback_thread.lock().unwrap()=Some(std::thread::spawn(move||{let host=format!("{}:{}",redirect.host_str().unwrap(),redirect.port().unwrap());let mut stream=TcpStream::connect(&host).unwrap();stream.set_read_timeout(Some(Duration::from_secs(5))).unwrap();write!(stream,"GET /oauth/callback?code=synthetic-code&state={state} HTTP/1.1\r\nHost: {host}\r\n\r\n").unwrap();let mut response=String::new();stream.read_to_string(&mut response).unwrap();assert!(response.starts_with("HTTP/1.1 200"));}));Ok(())}).unwrap();
        assert_eq!(response.access_token, "synthetic-token");
        callback_thread
            .lock()
            .unwrap()
            .take()
            .unwrap()
            .join()
            .unwrap();
        let requests = server.join().unwrap();
        let body = reqwest::Url::parse(&format!("http://127.0.0.1/?{}", requests[0])).unwrap();
        let params = body.query_pairs().collect::<BTreeMap<_, _>>();
        assert_eq!(params["grant_type"], "authorization_code");
        assert_eq!(params["code"], "synthetic-code");
        assert_eq!(
            challenge(&params["code_verifier"]),
            *expected.lock().unwrap()
        );
    }
    #[test]
    #[ignore = "Explicit local OAuth device pending/slow-down protocol E2E"]
    fn actual_device_flow_obeys_pending_and_slow_down_before_accepting_tokens() {
        let (origin, server) = identity_server(vec![
            (
                200,
                serde_json::json!({"device_code":"synthetic-device","user_code":"SYNTHETIC-CODE","verification_uri":"https://identity.invalid/device","expires_in":180,"interval":1}),
            ),
            (400, serde_json::json!({"error":"authorization_pending"})),
            (400, serde_json::json!({"error":"slow_down"})),
            (
                200,
                serde_json::json!({"access_token":"synthetic-token","token_type":"Bearer","expires_in":3600}),
            ),
        ]);
        let fixture = Fixture::new();
        fixture.destination();
        let mut value = profile();
        value["flow"] = "device_code".into();
        value
            .as_object_mut()
            .unwrap()
            .remove("authorization_endpoint");
        value["device_authorization_endpoint"] = format!("{origin}/device").into();
        value["token_endpoint"] = format!("{origin}/token").into();
        let profile = fixture.profile(value, true).unwrap();
        let start = Instant::now();
        let response = profile
            .device_login(start + Duration::from_secs(15), &|| Ok(()), &mut |event| {
                let OAuthEvent::DeviceAuthorization {
                    ref user_code,
                    expires_in,
                    ..
                } = event
                else {
                    unreachable!()
                };
                assert_eq!(user_code, "SYNTHETIC-CODE");
                assert!(
                    (1..15).contains(&expires_in),
                    "Device lifetime must reflect the remaining 15-second operation budget"
                );
                Ok(())
            })
            .unwrap();
        assert!(start.elapsed() >= Duration::from_secs(8));
        assert_eq!(response.access_token, "synthetic-token");
        let requests = server.join().unwrap();
        assert_eq!(requests.len(), 4);
        for request in &requests[1..] {
            assert!(request.contains("device_code=synthetic-device"));
        }
    }
}
