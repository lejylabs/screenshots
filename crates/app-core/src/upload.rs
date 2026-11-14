use anyhow::{Context, Result, ensure};
use serde::{Deserialize, Serialize};
use std::{collections::BTreeMap, io::Read, path::Path, sync::Arc, time::Duration};
use tokio::io::{AsyncReadExt, AsyncSeekExt};

#[derive(Clone, Copy, Default)]
pub struct UploadProgress {
    pub prepared: u64,
    pub total: u64,
}

#[derive(Clone)]
pub struct UploadProgressReporter(tokio::sync::watch::Sender<UploadProgress>);
pub struct UploadProgressReceiver(tokio::sync::watch::Receiver<UploadProgress>);

pub fn upload_progress_channel() -> (UploadProgressReporter, UploadProgressReceiver) {
    let (sender, receiver) = tokio::sync::watch::channel(UploadProgress::default());
    (
        UploadProgressReporter(sender),
        UploadProgressReceiver(receiver),
    )
}
impl UploadProgressReceiver {
    pub async fn next(&mut self) -> Option<UploadProgress> {
        self.0.changed().await.ok()?;
        Some(*self.0.borrow_and_update())
    }
}

struct ProgressReader<R> {
    reader: R,
    progress: Option<UploadProgressReporter>,
    prepared: u64,
    total: u64,
    last: std::time::Instant,
}
impl<R: tokio::io::AsyncRead + Unpin> tokio::io::AsyncRead for ProgressReader<R> {
    fn poll_read(
        self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
        buffer: &mut tokio::io::ReadBuf<'_>,
    ) -> std::task::Poll<std::io::Result<()>> {
        let this = self.get_mut();
        let before = buffer.filled().len();
        let result = std::pin::Pin::new(&mut this.reader).poll_read(cx, buffer);
        if this.progress.is_some()
            && matches!(result, std::task::Poll::Ready(Ok(())))
            && buffer.filled().len() > before
        {
            this.prepared += (buffer.filled().len() - before) as u64;
            if this.prepared == this.total || this.last.elapsed() >= Duration::from_millis(250) {
                if let Some(progress) = &this.progress {
                    progress.0.send_replace(UploadProgress {
                        prepared: this.prepared,
                        total: this.total,
                    });
                }
                this.last = std::time::Instant::now();
            }
        }
        result
    }
}

const MAX_RESPONSE: usize = 1024 * 1024;
const MAX_CUSTOM_HEADERS: usize = 8;
const MAX_QUERY_PARAMETERS: usize = 8;
const MAX_FORM_FIELDS: usize = 8;
const MAX_JSON_FIELDS: usize = 8;
const JSON_BASE64_PLACEHOLDER: &str = "{base64}";
const MAX_LINK_PREFIX: usize = 256;
const MAX_LINK_SUFFIX: usize = 64;
const MAX_LINK_FRAGMENT: usize = 512;
const MAX_REGEX_PATTERN: usize = 256;
const MAX_REGEX_SIZE: usize = 64 * 1024;
const MAX_REGEX_NEST: u32 = 32;
const MAX_REGEX_GROUP_INDEX: usize = 8;
const MAX_XML_PATH: usize = 256;
const MAX_XML_STEPS: usize = 8;
const MAX_XML_INDEX: usize = 256;
const MAX_XML_NODES: u32 = 4096;
const RESERVED_QUERY_NAMES: &[&str] = &[
    "password",
    "passwd",
    "secret",
    "token",
    "access_token",
    "refresh_token",
    "auth_token",
    "api_key",
    "apikey",
    "api-key",
    "authorization",
    "auth",
    "client_secret",
    "key",
];
const RESERVED_REQUEST_HEADERS: &[&str] = &[
    "authorization",
    "proxy-authorization",
    "www-authenticate",
    "x-api-key",
    "cookie",
    "set-cookie",
    "host",
    "content-length",
    "content-type",
    "transfer-encoding",
    "connection",
    "te",
    "trailer",
    "upgrade",
    "keep-alive",
    "expect",
    "if-none-match",
    "accept-encoding",
    "proxy-authenticate",
];

#[derive(Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct UploadDestination {
    url: String,
    #[serde(default)]
    method: UploadMethod,
    #[serde(default = "default_field")]
    field: String,
    #[serde(default)]
    json_pointer: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    response: Option<UploadResponse>,
    #[serde(default)]
    allow_http: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    credential: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    authentication: Option<UploadAuthentication>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    headers: BTreeMap<String, String>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    query: BTreeMap<String, String>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    form: BTreeMap<String, String>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    json: BTreeMap<String, String>,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    link_prefix: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    link_suffix: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    thumbnail: Option<UploadResultParser>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    deletion: Option<UploadResultParser>,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Eq, Serialize)]
#[serde(deny_unknown_fields)]
struct UploadResultParser {
    #[serde(default)]
    json_pointer: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    response: Option<UploadResponse>,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    link_prefix: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    link_suffix: String,
}

/// Successful upload links. Thumbnail and deletion URLs are session-only extras.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct UploadOutcome {
    pub url: String,
    pub thumbnail: Option<String>,
    pub deletion: Option<String>,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum UploadAuthentication {
    #[default]
    Bearer,
    ApiKey,
    Basic,
}
impl UploadAuthentication {
    pub fn header_name(self) -> &'static str {
        match self {
            Self::Bearer | Self::Basic => "authorization",
            Self::ApiKey => "x-api-key",
        }
    }
}

fn default_field() -> String {
    "file".into()
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "UPPERCASE")]
pub enum UploadMethod {
    #[default]
    Post,
    Put,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Eq, Serialize)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
enum UploadResponse {
    Text,
    Header {
        name: String,
    },
    RequestUrl,
    Regex {
        pattern: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        group: Option<String>,
    },
    Xml {
        path: String,
    },
}

#[derive(Clone, Debug, PartialEq, Eq)]
enum XmlPathStep {
    Child { name: String, position: usize },
    Attribute { name: String },
    Text,
}

impl UploadDestination {
    /// Build a credential-free manual destination without performing IO or networking.
    pub fn manual(
        url: String,
        method: UploadMethod,
        field: String,
        json_pointer: Option<String>,
        allow_http: bool,
    ) -> Result<Self> {
        let response = json_pointer.is_none().then_some(UploadResponse::Text);
        let field = if method == UploadMethod::Post {
            field.trim().into()
        } else {
            default_field()
        };
        let destination = Self {
            url: url.trim().into(),
            method,
            field,
            json_pointer: json_pointer.unwrap_or_default().trim().into(),
            response,
            allow_http,
            credential: None,
            authentication: None,
            headers: BTreeMap::new(),
            query: BTreeMap::new(),
            form: BTreeMap::new(),
            json: BTreeMap::new(),
            link_prefix: String::new(),
            link_suffix: String::new(),
            thumbnail: None,
            deletion: None,
        };
        destination.validate()?;
        Ok(destination)
    }

    /// Attach bounded custom request headers without networking or secrets.
    pub fn with_headers<I, N, V>(mut self, headers: I) -> Result<Self>
    where
        I: IntoIterator<Item = (N, V)>,
        N: AsRef<str>,
        V: AsRef<str>,
    {
        self.headers = headers
            .into_iter()
            .map(|(name, value)| {
                (
                    name.as_ref().trim().to_owned(),
                    value.as_ref().trim().to_owned(),
                )
            })
            .collect();
        self.validate()?;
        Ok(self)
    }

    pub fn request_headers(&self) -> &BTreeMap<String, String> {
        &self.headers
    }

    /// Attach bounded query parameters without networking or secrets.
    pub fn with_query<I, N, V>(mut self, query: I) -> Result<Self>
    where
        I: IntoIterator<Item = (N, V)>,
        N: AsRef<str>,
        V: AsRef<str>,
    {
        self.query = query
            .into_iter()
            .map(|(name, value)| {
                (
                    name.as_ref().trim().to_owned(),
                    value.as_ref().trim().to_owned(),
                )
            })
            .collect();
        self.validate()?;
        Ok(self)
    }

    pub fn request_query(&self) -> &BTreeMap<String, String> {
        &self.query
    }

    /// Attach bounded extra multipart fields without networking or secrets.
    pub fn with_form<I, N, V>(mut self, form: I) -> Result<Self>
    where
        I: IntoIterator<Item = (N, V)>,
        N: AsRef<str>,
        V: AsRef<str>,
    {
        self.form = form
            .into_iter()
            .map(|(name, value)| {
                (
                    name.as_ref().trim().to_owned(),
                    value.as_ref().trim().to_owned(),
                )
            })
            .collect();
        self.validate()?;
        Ok(self)
    }

    pub fn request_form(&self) -> &BTreeMap<String, String> {
        &self.form
    }

    /// Attach a bounded JSON body with one `{base64}` PNG field, without networking or secrets.
    pub fn with_json<I, N, V>(mut self, json: I) -> Result<Self>
    where
        I: IntoIterator<Item = (N, V)>,
        N: AsRef<str>,
        V: AsRef<str>,
    {
        self.json = json
            .into_iter()
            .map(|(name, value)| {
                (
                    name.as_ref().trim().to_owned(),
                    value.as_ref().trim().to_owned(),
                )
            })
            .collect();
        self.validate()?;
        Ok(self)
    }

    pub fn request_json(&self) -> &BTreeMap<String, String> {
        &self.json
    }

    /// Attach a bounded result-link prefix/suffix without networking or secrets.
    pub fn with_link_affixes(
        mut self,
        prefix: impl AsRef<str>,
        suffix: impl AsRef<str>,
    ) -> Result<Self> {
        self.link_prefix = prefix.as_ref().trim().to_owned();
        self.link_suffix = suffix.as_ref().trim().to_owned();
        self.validate()?;
        Ok(self)
    }

    pub fn link_prefix(&self) -> &str {
        &self.link_prefix
    }

    pub fn link_suffix(&self) -> &str {
        &self.link_suffix
    }

    /// Attach a bounded regex result parser without networking or secrets.
    pub fn with_regex(mut self, pattern: impl AsRef<str>, group: Option<&str>) -> Result<Self> {
        self.response = Some(UploadResponse::Regex {
            pattern: pattern.as_ref().trim().to_owned(),
            group: group
                .map(str::trim)
                .filter(|value| !value.is_empty())
                .map(str::to_owned),
        });
        self.json_pointer.clear();
        self.validate()?;
        Ok(self)
    }

    pub fn regex_pattern(&self) -> Option<&str> {
        match &self.response {
            Some(UploadResponse::Regex { pattern, .. }) => Some(pattern),
            _ => None,
        }
    }

    pub fn regex_group(&self) -> Option<&str> {
        match &self.response {
            Some(UploadResponse::Regex { group, .. }) => group.as_deref(),
            _ => None,
        }
    }

    /// Attach a bounded XML result parser without networking or secrets.
    pub fn with_xml(mut self, path: impl AsRef<str>) -> Result<Self> {
        self.response = Some(UploadResponse::Xml {
            path: path.as_ref().trim().to_owned(),
        });
        self.json_pointer.clear();
        self.validate()?;
        Ok(self)
    }

    pub fn xml_path(&self) -> Option<&str> {
        match &self.response {
            Some(UploadResponse::Xml { path }) => Some(path),
            _ => None,
        }
    }

    /// Attach a thumbnail result parser from a ShareX-style template, without networking.
    pub fn with_thumbnail(mut self, template: impl AsRef<str>) -> Result<Self> {
        self.thumbnail = Some(UploadResultParser::from_sharex(
            template.as_ref(),
            self.method,
            self.allow_http,
            "thumbnail",
        )?);
        self.validate()?;
        Ok(self)
    }

    /// Attach a deletion result parser from a ShareX-style template, without networking.
    pub fn with_deletion(mut self, template: impl AsRef<str>) -> Result<Self> {
        self.deletion = Some(UploadResultParser::from_sharex(
            template.as_ref(),
            self.method,
            self.allow_http,
            "deletion",
        )?);
        self.validate()?;
        Ok(self)
    }

    pub fn thumbnail_template(&self) -> Option<String> {
        self.thumbnail
            .as_ref()
            .and_then(|parser| parser.to_sharex().ok())
    }

    pub fn deletion_template(&self) -> Option<String> {
        self.deletion
            .as_ref()
            .and_then(|parser| parser.to_sharex().ok())
    }

    /// Attach a credential reference without reading or writing secrets.
    pub fn with_authentication(
        mut self,
        reference: String,
        authentication: UploadAuthentication,
    ) -> Result<Self> {
        self.credential = Some(reference.trim().into());
        self.authentication = Some(authentication);
        self.validate()?;
        Ok(self)
    }

    /// Validate, store the secret in the OS credential store, and return the updated profile.
    /// The secret is never written to the destination JSON.
    pub fn attach_credential(
        self,
        reference: String,
        authentication: UploadAuthentication,
        username: String,
        secret: String,
    ) -> Result<Self> {
        let username = zeroize::Zeroizing::new(username);
        let secret = zeroize::Zeroizing::new(secret);
        let destination = self.with_authentication(reference, authentication)?;
        let target = destination
            .credential_target()
            .context("Authenticated destinations require a Windows credential target")?;
        crate::upload_credentials::save(&target, authentication, &username, &secret)?;
        Ok(destination)
    }

    /// Delete the OS secret for this destination's credential target and return
    /// a credential-free copy. Forget destination does not call this.
    pub fn detach_credential(self) -> Result<Self> {
        let target = self
            .credential_target()
            .context("This destination has no Windows credential to remove")?;
        crate::upload_credentials::delete(&target)?;
        let mut destination = self;
        destination.credential = None;
        destination.authentication = None;
        destination.validate()?;
        Ok(destination)
    }

    pub fn credential_reference(&self) -> Option<&str> {
        self.credential.as_deref()
    }

    pub fn authentication(&self) -> Option<UploadAuthentication> {
        self.authentication
    }

    pub fn credential_target(&self) -> Option<String> {
        let name = self.credential.as_ref()?;
        let origin = self.validate().ok()?.origin().ascii_serialization();
        Some(format!("Screenshots/upload/{origin}/{name}"))
    }
    pub fn request_method(&self) -> &'static str {
        match self.method {
            UploadMethod::Post => "POST",
            UploadMethod::Put => "PUT",
        }
    }
    fn remembered_path() -> Result<std::path::PathBuf> {
        Ok(crate::CaptureSettings::config_path()?.with_file_name("upload-destination.json"))
    }
    pub fn load_remembered() -> Result<Option<Self>> {
        Self::load_optional(&Self::remembered_path()?)
    }
    fn load_optional(path: &Path) -> Result<Option<Self>> {
        match Self::load(path) {
            Ok(value) => Ok(Some(value)),
            Err(error)
                if error
                    .downcast_ref::<std::io::Error>()
                    .is_some_and(|error| error.kind() == std::io::ErrorKind::NotFound) =>
            {
                Ok(None)
            }
            Err(error) => Err(error),
        }
    }
    pub fn remember(&self) -> Result<()> {
        self.save_at(&Self::remembered_path()?)
    }
    fn save_at(&self, path: &Path) -> Result<()> {
        use std::io::Write;
        self.validate()?;
        let parent = path.parent().context("Invalid upload settings path")?;
        std::fs::create_dir_all(parent)?;
        let temporary = parent.join(format!("upload-{}.part", uuid::Uuid::new_v4()));
        let result = (|| -> Result<()> {
            let mut file = std::fs::OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(&temporary)?;
            file.write_all(&serde_json::to_vec_pretty(self)?)?;
            file.sync_all()?;
            drop(file);
            std::fs::rename(&temporary, path)?;
            Ok(())
        })();
        if result.is_err() {
            let _ = std::fs::remove_file(temporary);
        }
        result
    }
    pub fn forget_remembered() -> Result<()> {
        Self::remove_at(&Self::remembered_path()?)
    }
    fn remove_at(path: &Path) -> Result<()> {
        match std::fs::remove_file(path) {
            Ok(()) => Ok(()),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(error) => Err(error.into()),
        }
    }

    pub fn load(path: &Path) -> Result<Self> {
        let mut bytes = Vec::new();
        std::fs::File::open(path)?
            .take(16385)
            .read_to_end(&mut bytes)?;
        ensure!(bytes.len() <= 16384, "Upload configuration exceeds 16 KiB");
        let destination: Self =
            serde_json::from_slice(&bytes).context("Invalid upload configuration")?;
        destination.validate()?;
        Ok(destination)
    }

    fn validate(&self) -> Result<reqwest::Url> {
        ensure!(
            self.authentication.is_none() || self.credential.is_some(),
            "Authentication requires an OS credential reference"
        );
        if let Some(response) = &self.response {
            if matches!(response, UploadResponse::RequestUrl) {
                ensure!(
                    matches!(self.method, UploadMethod::Put),
                    "request_url response mode requires PUT to a resource URL"
                );
            }
            ensure!(
                self.json_pointer.is_empty(),
                "Choose either json_pointer or response, not both"
            );
            if let UploadResponse::Header { name } = response {
                ensure!(
                    name.len() <= 64
                        && reqwest::header::HeaderName::from_bytes(name.as_bytes()).is_ok(),
                    "Use a valid response header name of at most 64 bytes"
                );
            }
            if let UploadResponse::Regex { pattern, group } = response {
                validate_regex_response(pattern, group.as_deref(), "result")?;
            }
            if let UploadResponse::Xml { path } = response {
                validate_xml_response(path, "result")?;
            }
        }
        ensure!(
            self.url.len() <= 2048 && !self.url.chars().any(char::is_control),
            "Upload URL is invalid or exceeds 2048 bytes"
        );
        let url = reqwest::Url::parse(&self.url).context("Invalid upload URL")?;
        if let Some(name) = &self.credential {
            ensure!(
                url.scheme() == "https",
                "Upload credentials require HTTPS, including for self-hosted destinations"
            );
            ensure!(
                !name.is_empty()
                    && name.len() <= 64
                    && name
                        .bytes()
                        .all(|byte| byte.is_ascii_alphanumeric() || b"_-".contains(&byte)),
                "Credential reference must contain 1–64 letters, digits, underscores or hyphens"
            );
            ensure!(
                cfg!(target_os = "windows"),
                "OS upload credential lookup is currently supported on Windows only"
            );
        }
        ensure!(
            url.scheme() == "https" || (url.scheme() == "http" && self.allow_http),
            "Use HTTPS, or explicitly enable allow_http for your HTTP destination"
        );
        ensure!(
            url.host_str().is_some()
                && url.username().is_empty()
                && url.password().is_none()
                && url.query().is_none()
                && url.fragment().is_none(),
            "Upload URLs must have a host and no embedded credentials, query or fragment"
        );
        ensure!(
            !self.field.is_empty()
                && self.field.len() <= 64
                && self
                    .field
                    .bytes()
                    .all(|byte| byte.is_ascii_alphanumeric() || byte == b'_' || byte == b'-'),
            "Use a multipart field of 1–64 letters, digits, underscores or hyphens"
        );
        ensure!(
            self.json_pointer.len() <= 256
                && (self.json_pointer.is_empty() || self.json_pointer.starts_with('/')),
            "Use a JSON pointer up to 256 bytes, such as /url"
        );
        ensure!(
            self.json_pointer
                .split('~')
                .skip(1)
                .all(|part| part.starts_with('0') || part.starts_with('1')),
            "Invalid JSON pointer escape; use ~0 for ~ and ~1 for /"
        );
        validate_custom_query(&self.query)?;
        validate_custom_form(&self.form, self.method, &self.field)?;
        validate_custom_json(&self.json, self.method, &self.form)?;
        validate_custom_headers(&self.headers)?;
        self.validate_link_affixes()?;
        if matches!(self.response, Some(UploadResponse::RequestUrl)) {
            ensure!(
                self.thumbnail.is_none() && self.deletion.is_none(),
                "Thumbnail and deletion results cannot be combined with PUT request-URL mode"
            );
        }
        if let Some(parser) = &self.thumbnail {
            parser.validate(self.method, self.allow_http, "thumbnail")?;
        }
        if let Some(parser) = &self.deletion {
            parser.validate(self.method, self.allow_http, "deletion")?;
        }
        let url = self.apply_query(url)?;
        if matches!(self.response, Some(UploadResponse::RequestUrl)) {
            validate_link(url.as_str())?;
        }
        Ok(url)
    }

    fn apply_query(&self, mut url: reqwest::Url) -> Result<reqwest::Url> {
        if self.query.is_empty() {
            return Ok(url);
        }
        {
            let mut pairs = url.query_pairs_mut();
            for (name, value) in &self.query {
                pairs.append_pair(name, value);
            }
        }
        ensure!(
            url.as_str().len() <= 2048 && !url.as_str().chars().any(char::is_control),
            "Upload URL with query parameters exceeds 2048 bytes"
        );
        Ok(url)
    }

    fn validate_link_affixes(&self) -> Result<()> {
        validate_parser_affixes(
            &self.link_prefix,
            &self.link_suffix,
            self.response.as_ref(),
            self.allow_http,
        )
    }

    fn result_link(&self, value: &str) -> Result<String> {
        compose_result_link(&self.link_prefix, &self.link_suffix, value)
    }

    fn extra_needs_json(&self) -> bool {
        self.thumbnail
            .as_ref()
            .is_some_and(UploadResultParser::needs_json)
            || self
                .deletion
                .as_ref()
                .is_some_and(UploadResultParser::needs_json)
    }

    fn extra_needs_text(&self) -> bool {
        self.thumbnail
            .as_ref()
            .is_some_and(UploadResultParser::needs_text)
            || self
                .deletion
                .as_ref()
                .is_some_and(UploadResultParser::needs_text)
    }

    pub fn origin(&self) -> String {
        self.validate()
            .map(|url| url.origin().ascii_serialization())
            .unwrap_or_default()
    }

    /// Load a documented ShareX `.sxcu` subset without networking or secrets.
    pub fn from_sharex(path: &Path) -> Result<(Self, Option<String>)> {
        Self::from_sharex_at(path)
    }

    fn from_sharex_at(path: &Path) -> Result<(Self, Option<String>)> {
        let mut bytes = Vec::new();
        std::fs::File::open(path)?
            .take(16385)
            .read_to_end(&mut bytes)?;
        ensure!(bytes.len() <= 16384, "ShareX profile exceeds 16 KiB");
        let profile: ShareXCustomUploader =
            serde_json::from_slice(&bytes).context("Invalid ShareX profile")?;
        profile.into_destination()
    }

    /// Write a documented ShareX `.sxcu` subset without secrets or overwriting.
    pub fn to_sharex(&self, path: &Path) -> Result<()> {
        self.to_sharex_at(path, None)
    }

    fn to_sharex_at(&self, path: &Path, name: Option<&str>) -> Result<()> {
        use std::io::Write;
        let profile = ShareXCustomUploader::from_destination(self, name)?;
        let bytes = serde_json::to_vec_pretty(&profile)?;
        ensure!(bytes.len() <= 16384, "ShareX profile exceeds 16 KiB");
        let parent = path.parent().context("Invalid ShareX profile path")?;
        std::fs::create_dir_all(parent)?;
        let mut file = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(path)
            .map_err(|error| {
                if error.kind() == std::io::ErrorKind::AlreadyExists {
                    anyhow::anyhow!(
                        "The file already exists. Choose a new name; existing files are never overwritten."
                    )
                } else {
                    error.into()
                }
            })?;
        let result = (|| -> Result<()> {
            file.write_all(&bytes)?;
            file.sync_all()?;
            Ok(())
        })();
        if result.is_err() {
            drop(file);
            let _ = std::fs::remove_file(path);
        }
        result
    }
}

#[derive(Deserialize, Serialize)]
struct ShareXCustomUploader {
    #[serde(rename = "Version", default, skip_serializing_if = "Option::is_none")]
    version: Option<String>,
    #[serde(rename = "Name", default, skip_serializing_if = "Option::is_none")]
    name: Option<String>,
    #[serde(rename = "DestinationType")]
    destination_type: String,
    #[serde(
        rename = "RequestMethod",
        default,
        skip_serializing_if = "Option::is_none"
    )]
    request_method: Option<String>,
    #[serde(rename = "RequestURL")]
    request_url: String,
    #[serde(rename = "Body", default, skip_serializing_if = "Option::is_none")]
    body: Option<String>,
    #[serde(
        rename = "FileFormName",
        default,
        skip_serializing_if = "Option::is_none"
    )]
    file_form_name: Option<String>,
    #[serde(rename = "URL", default, skip_serializing_if = "Option::is_none")]
    url: Option<String>,
    #[serde(
        rename = "ThumbnailURL",
        default,
        skip_serializing_if = "Option::is_none"
    )]
    thumbnail_url: Option<String>,
    #[serde(
        rename = "DeletionURL",
        default,
        skip_serializing_if = "Option::is_none"
    )]
    deletion_url: Option<String>,
    #[serde(rename = "Headers", default, skip_serializing_if = "Option::is_none")]
    headers: Option<serde_json::Value>,
    #[serde(
        rename = "Parameters",
        default,
        skip_serializing_if = "Option::is_none"
    )]
    parameters: Option<serde_json::Value>,
    #[serde(rename = "Arguments", default, skip_serializing_if = "Option::is_none")]
    arguments: Option<serde_json::Value>,
    #[serde(rename = "Data", default, skip_serializing_if = "Option::is_none")]
    data: Option<serde_json::Value>,
}

impl ShareXCustomUploader {
    fn into_destination(self) -> Result<(UploadDestination, Option<String>)> {
        ensure!(
            self.destination_type
                .split(',')
                .any(|kind| kind.trim().eq_ignore_ascii_case("ImageUploader")),
            "ShareX profile must include ImageUploader"
        );
        let headers = sharex_string_map(&self.headers, "headers")?;
        let query = sharex_string_map(&self.parameters, "query parameters")?;
        ensure!(
            !self.request_url.contains('{') && !self.request_url.contains('}'),
            "ShareX request URL syntax is not supported"
        );
        let method_name = self
            .request_method
            .as_deref()
            .unwrap_or("POST")
            .trim()
            .to_ascii_uppercase();
        let method = match method_name.as_str() {
            "POST" => UploadMethod::Post,
            "PUT" => UploadMethod::Put,
            _ => anyhow::bail!("ShareX request method {method_name} is not supported"),
        };
        let body = self
            .body
            .as_deref()
            .unwrap_or(if method == UploadMethod::Post {
                "MultipartFormData"
            } else {
                "Binary"
            })
            .trim();
        match (method, body) {
            (UploadMethod::Post, "MultipartFormData") | (UploadMethod::Post, "JSON") => {}
            (UploadMethod::Put, "Binary") => {}
            _ => anyhow::bail!(
                "ShareX import supports POST multipart, POST JSON or PUT binary bodies only"
            ),
        }
        let (form, json) = if body == "JSON" {
            sharex_reject_object(&self.arguments, "form fields")?;
            let json = sharex_string_map(&self.data, "JSON body fields")?;
            ensure!(
                !json.is_empty(),
                "ShareX JSON bodies need a Data object with one {{base64}} field"
            );
            (BTreeMap::new(), json)
        } else {
            sharex_reject_object(&self.data, "JSON body fields")?;
            (
                sharex_string_map(&self.arguments, "form fields")?,
                BTreeMap::new(),
            )
        };
        let field = if method == UploadMethod::Post {
            self.file_form_name
                .as_deref()
                .unwrap_or("file")
                .trim()
                .to_owned()
        } else {
            default_field()
        };
        let (json_pointer, response, link_prefix, link_suffix) =
            sharex_response(self.url.as_deref().unwrap_or(""), method)?;
        let thumbnail = match self
            .thumbnail_url
            .as_deref()
            .map(str::trim)
            .filter(|value| !value.is_empty())
        {
            Some(template) => Some(UploadResultParser::from_sharex(
                template,
                method,
                false,
                "thumbnail",
            )?),
            None => None,
        };
        let deletion = match self
            .deletion_url
            .as_deref()
            .map(str::trim)
            .filter(|value| !value.is_empty())
        {
            Some(template) => Some(UploadResultParser::from_sharex(
                template, method, false, "deletion",
            )?),
            None => None,
        };
        let destination = UploadDestination {
            url: self.request_url.trim().into(),
            method,
            field,
            json_pointer,
            response,
            allow_http: false,
            credential: None,
            authentication: None,
            headers,
            query,
            form,
            json,
            link_prefix,
            link_suffix,
            thumbnail,
            deletion,
        };
        destination.validate()?;
        let name = match self
            .name
            .as_deref()
            .map(str::trim)
            .filter(|name| !name.is_empty())
        {
            Some(name) => {
                ensure!(
                    name.len() <= 64 && !name.chars().any(char::is_control),
                    "ShareX profile name must be 1–64 bytes without control characters"
                );
                Some(name.to_owned())
            }
            None => None,
        };
        Ok((destination, name))
    }

    fn from_destination(destination: &UploadDestination, name: Option<&str>) -> Result<Self> {
        let url = destination.validate()?;
        ensure!(url.scheme() == "https", "ShareX export requires HTTPS");
        let name = match name.map(str::trim).filter(|name| !name.is_empty()) {
            Some(name) => {
                ensure!(
                    name.len() <= 64 && !name.chars().any(char::is_control),
                    "ShareX profile name must be 1–64 bytes without control characters"
                );
                Some(name.to_owned())
            }
            None => None,
        };
        let url = Some(sharex_url_template(
            destination.response.as_ref(),
            &destination.json_pointer,
            &destination.link_prefix,
            &destination.link_suffix,
        )?);
        Ok(Self {
            version: Some("17.0.0".into()),
            name,
            destination_type: "ImageUploader".into(),
            request_method: Some(destination.request_method().into()),
            request_url: destination.url.clone(),
            body: Some(
                if !destination.json.is_empty() {
                    "JSON"
                } else if destination.method == UploadMethod::Post {
                    "MultipartFormData"
                } else {
                    "Binary"
                }
                .into(),
            ),
            file_form_name: (destination.method == UploadMethod::Post
                && destination.json.is_empty())
            .then(|| destination.field.clone()),
            url,
            thumbnail_url: destination
                .thumbnail
                .as_ref()
                .map(UploadResultParser::to_sharex)
                .transpose()?,
            deletion_url: destination
                .deletion
                .as_ref()
                .map(UploadResultParser::to_sharex)
                .transpose()?,
            headers: if destination.headers.is_empty() {
                None
            } else {
                Some(serde_json::to_value(&destination.headers)?)
            },
            parameters: if destination.query.is_empty() {
                None
            } else {
                Some(serde_json::to_value(&destination.query)?)
            },
            arguments: if destination.form.is_empty() {
                None
            } else {
                Some(serde_json::to_value(&destination.form)?)
            },
            data: if destination.json.is_empty() {
                None
            } else {
                Some(serde_json::to_value(&destination.json)?)
            },
        })
    }
}

fn sharex_reject_object(value: &Option<serde_json::Value>, label: &str) -> Result<()> {
    match value {
        None | Some(serde_json::Value::Null) => Ok(()),
        Some(serde_json::Value::Object(map)) if map.is_empty() => Ok(()),
        Some(_) => anyhow::bail!("{label} are not supported in this ShareX import subset"),
    }
}

fn sharex_string_map(
    value: &Option<serde_json::Value>,
    label: &str,
) -> Result<BTreeMap<String, String>> {
    match value {
        None | Some(serde_json::Value::Null) => Ok(BTreeMap::new()),
        Some(serde_json::Value::Object(map)) => {
            let mut pairs = BTreeMap::new();
            for (name, value) in map {
                let Some(text) = value.as_str() else {
                    anyhow::bail!("ShareX {label} must be string pairs");
                };
                pairs.insert(name.trim().to_owned(), text.trim().to_owned());
            }
            Ok(pairs)
        }
        Some(_) => anyhow::bail!("ShareX {label} must be an object of string pairs"),
    }
}

fn contains_upload_template(value: &str) -> bool {
    value.bytes().any(|byte| matches!(byte, b'{' | b'}' | b'$'))
}

fn validate_custom_query(query: &BTreeMap<String, String>) -> Result<()> {
    ensure!(
        query.len() <= MAX_QUERY_PARAMETERS,
        "At most 8 query parameters"
    );
    let mut seen = Vec::with_capacity(query.len());
    for (name, value) in query {
        ensure!(
            !name.is_empty()
                && name.len() <= 64
                && name.trim() == name
                && !name.chars().any(char::is_control)
                && !contains_upload_template(name)
                && name
                    .bytes()
                    .all(|byte| byte.is_ascii_alphanumeric() || byte == b'_' || byte == b'-'),
            "Use a query parameter name of 1–64 letters, digits, underscores or hyphens without templates"
        );
        let lower = name.to_ascii_lowercase();
        ensure!(
            !RESERVED_QUERY_NAMES.contains(&lower.as_str()),
            "Query parameters cannot set {name}"
        );
        ensure!(
            !seen.iter().any(|existing| existing == &lower),
            "Query parameter names must be unique ignoring case"
        );
        seen.push(lower);
        ensure!(
            !value.is_empty()
                && value.len() <= 256
                && value.trim() == value
                && !value.chars().any(char::is_control)
                && !contains_upload_template(value),
            "Use a query parameter value of 1–256 bytes without templates or control characters"
        );
    }
    Ok(())
}

fn validate_custom_form(
    form: &BTreeMap<String, String>,
    method: UploadMethod,
    field: &str,
) -> Result<()> {
    ensure!(
        form.is_empty() || matches!(method, UploadMethod::Post),
        "Extra form fields require POST multipart"
    );
    ensure!(form.len() <= MAX_FORM_FIELDS, "At most 8 extra form fields");
    let mut seen = Vec::with_capacity(form.len());
    for (name, value) in form {
        ensure!(
            !name.is_empty()
                && name.len() <= 64
                && name.trim() == name
                && !name.chars().any(char::is_control)
                && !contains_upload_template(name)
                && name
                    .bytes()
                    .all(|byte| byte.is_ascii_alphanumeric() || byte == b'_' || byte == b'-'),
            "Use a form field name of 1–64 letters, digits, underscores or hyphens without templates"
        );
        let lower = name.to_ascii_lowercase();
        ensure!(
            !RESERVED_QUERY_NAMES.contains(&lower.as_str()),
            "Form fields cannot set {name}"
        );
        ensure!(
            !lower.eq_ignore_ascii_case(field),
            "Extra form fields cannot reuse the multipart file field"
        );
        ensure!(
            !seen.iter().any(|existing| existing == &lower),
            "Form field names must be unique ignoring case"
        );
        seen.push(lower);
        ensure!(
            !value.is_empty()
                && value.len() <= 256
                && value.trim() == value
                && !value.chars().any(char::is_control)
                && !contains_upload_template(value),
            "Use a form field value of 1–256 bytes without templates or control characters"
        );
    }
    Ok(())
}

fn validate_custom_json(
    json: &BTreeMap<String, String>,
    method: UploadMethod,
    form: &BTreeMap<String, String>,
) -> Result<()> {
    if json.is_empty() {
        return Ok(());
    }
    ensure!(
        matches!(method, UploadMethod::Post),
        "JSON bodies require POST"
    );
    ensure!(
        form.is_empty(),
        "JSON bodies cannot be combined with extra form fields"
    );
    ensure!(json.len() <= MAX_JSON_FIELDS, "At most 8 JSON body fields");
    let mut seen = Vec::with_capacity(json.len());
    let mut base64_fields = 0usize;
    for (name, value) in json {
        ensure!(
            !name.is_empty()
                && name.len() <= 64
                && name.trim() == name
                && !name.chars().any(char::is_control)
                && !contains_upload_template(name)
                && name
                    .bytes()
                    .all(|byte| byte.is_ascii_alphanumeric() || byte == b'_' || byte == b'-'),
            "Use a JSON body field name of 1–64 letters, digits, underscores or hyphens without templates"
        );
        let lower = name.to_ascii_lowercase();
        ensure!(
            !RESERVED_QUERY_NAMES.contains(&lower.as_str()),
            "JSON body fields cannot set {name}"
        );
        ensure!(
            !seen.iter().any(|existing| existing == &lower),
            "JSON body field names must be unique ignoring case"
        );
        seen.push(lower);
        if value == JSON_BASE64_PLACEHOLDER {
            base64_fields += 1;
            continue;
        }
        ensure!(
            !value.is_empty()
                && value.len() <= 256
                && value.trim() == value
                && !value.chars().any(char::is_control)
                && !contains_upload_template(value),
            "Use a JSON body field value of 1–256 bytes without templates or control characters, or exactly {{base64}}"
        );
    }
    ensure!(
        base64_fields == 1,
        "JSON bodies need exactly one {{base64}} field for the PNG"
    );
    Ok(())
}

fn json_request_body(json: &BTreeMap<String, String>, png: &[u8]) -> Result<Vec<u8>> {
    use base64::Engine;
    let mut encoded = Some(base64::engine::general_purpose::STANDARD.encode(png));
    let mut object = serde_json::Map::with_capacity(json.len());
    for (name, value) in json {
        let text = if value == JSON_BASE64_PLACEHOLDER {
            encoded
                .take()
                .context("JSON bodies need exactly one {{base64}} field for the PNG")?
        } else {
            value.clone()
        };
        object.insert(name.clone(), serde_json::Value::String(text));
    }
    serde_json::to_vec(&serde_json::Value::Object(object))
        .context("Could not build the JSON upload body")
}

fn validate_custom_headers(headers: &BTreeMap<String, String>) -> Result<()> {
    ensure!(
        headers.len() <= MAX_CUSTOM_HEADERS,
        "At most 8 custom request headers"
    );
    let mut seen = Vec::with_capacity(headers.len());
    for (name, value) in headers {
        ensure!(
            !name.is_empty()
                && name.len() <= 64
                && name.trim() == name
                && !name.chars().any(char::is_control)
                && !contains_upload_template(name)
                && reqwest::header::HeaderName::from_bytes(name.as_bytes()).is_ok(),
            "Use a valid custom request header name of 1–64 bytes without templates"
        );
        let lower = name.to_ascii_lowercase();
        ensure!(
            !RESERVED_REQUEST_HEADERS.contains(&lower.as_str()),
            "Custom request headers cannot set {name}"
        );
        ensure!(
            !seen.iter().any(|existing| existing == &lower),
            "Custom request header names must be unique ignoring case"
        );
        seen.push(lower);
        ensure!(
            !value.is_empty()
                && value.len() <= 256
                && value.trim() == value
                && !value.chars().any(char::is_control)
                && !contains_upload_template(value)
                && reqwest::header::HeaderValue::from_str(value).is_ok(),
            "Use a valid custom request header value of 1–256 bytes without templates or control characters"
        );
    }
    Ok(())
}

fn sharex_response(
    url: &str,
    method: UploadMethod,
) -> Result<(String, Option<UploadResponse>, String, String)> {
    let url = url.trim();
    if url.is_empty() {
        return Ok((
            String::new(),
            Some(UploadResponse::Text),
            String::new(),
            String::new(),
        ));
    }
    ensure!(
        url.len() <= 256 && !url.chars().any(char::is_control),
        "ShareX URL parser is invalid or exceeds 256 bytes"
    );
    let (prefix, token, suffix) = split_sharex_url_template(url)?;
    let (pointer, response) = parse_sharex_url_token(&token, method)?;
    if matches!(response, Some(UploadResponse::RequestUrl)) {
        ensure!(
            prefix.is_empty() && suffix.is_empty(),
            "composed {{responseurl}} is not supported"
        );
    }
    Ok((pointer, response, prefix, suffix))
}

fn split_sharex_url_template(url: &str) -> Result<(String, String, String)> {
    let mut start = None;
    let mut end = None;
    let mut escaped = false;
    for (index, ch) in url.char_indices() {
        if escaped {
            escaped = false;
            continue;
        }
        if ch == '\\' {
            escaped = true;
            continue;
        }
        if ch == '{' && start.is_none() {
            start = Some(index);
        } else if ch == '}' && start.is_some() {
            end = Some(index);
            break;
        }
    }
    let start = start.context("This ShareX URL syntax is not supported")?;
    let end = end.context("This ShareX URL syntax is not supported")?;
    let prefix = &url[..start];
    let suffix = &url[end + 1..];
    ensure!(
        !prefix.contains(['{', '}', '$']) && !suffix.contains(['{', '}', '$']),
        "ShareX URL templates support one parser token"
    );
    Ok((prefix.into(), url[start..=end].into(), suffix.into()))
}

fn parse_sharex_url_token(
    token: &str,
    method: UploadMethod,
) -> Result<(String, Option<UploadResponse>)> {
    if token == "{response}" {
        return Ok((String::new(), Some(UploadResponse::Text)));
    }
    if token == "{responseurl}" {
        ensure!(
            method == UploadMethod::Put,
            "{{responseurl}} requires a PUT ShareX profile"
        );
        return Ok((String::new(), Some(UploadResponse::RequestUrl)));
    }
    if token == "{xml}" {
        anyhow::bail!("ShareX XML path is required");
    }
    if let Some(inner) = token
        .strip_prefix("{xml:")
        .and_then(|value| value.strip_suffix('}'))
    {
        let parts = split_sharex_escaped(inner, '|')?;
        ensure!(parts.len() == 1, "ShareX xml input syntax is not supported");
        let path = parts[0].trim();
        validate_xml_response(path, "result")?;
        return Ok((
            String::new(),
            Some(UploadResponse::Xml {
                path: path.to_owned(),
            }),
        ));
    }
    if token == "{regex}" {
        anyhow::bail!("ShareX RegexList is not supported");
    }
    if let Some(inner) = token
        .strip_prefix("{regex:")
        .and_then(|value| value.strip_suffix('}'))
    {
        let parts = split_sharex_escaped(inner, '|')?;
        ensure!(
            parts.len() <= 2,
            "ShareX regex input syntax is not supported"
        );
        let pattern = parts[0].trim();
        let group = parts
            .get(1)
            .map(String::as_str)
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(str::to_owned);
        validate_regex_response(pattern, group.as_deref(), "result")?;
        return Ok((
            String::new(),
            Some(UploadResponse::Regex {
                pattern: pattern.to_owned(),
                group,
            }),
        ));
    }
    if let Some(name) = token
        .strip_prefix("{header:")
        .and_then(|value| value.strip_suffix('}'))
    {
        let name = name.trim();
        ensure!(
            !name.is_empty() && name.len() <= 64 && !name.chars().any(char::is_control),
            "Use a ShareX response header of 1–64 bytes"
        );
        return Ok((
            String::new(),
            Some(UploadResponse::Header { name: name.into() }),
        ));
    }
    if let Some(path) = token
        .strip_prefix("{json:")
        .and_then(|value| value.strip_suffix('}'))
    {
        ensure!(
            !path.contains('|'),
            "ShareX json input syntax is not supported"
        );
        return Ok((sharex_json_pointer(path)?, None));
    }
    anyhow::bail!("This ShareX URL syntax is not supported")
}

fn sharex_json_pointer(path: &str) -> Result<String> {
    let path = path.trim();
    ensure!(
        !path.is_empty() && path.len() <= 256 && !path.chars().any(char::is_control),
        "Unsupported ShareX JSON path"
    );
    let mut pointer = String::new();
    let mut rest = path;
    while !rest.is_empty() {
        if let Some(stripped) = rest.strip_prefix('[') {
            let end = stripped.find(']').context("Unsupported ShareX JSON path")?;
            let index = &stripped[..end];
            ensure!(
                !index.is_empty()
                    && index.bytes().all(|byte| byte.is_ascii_digit())
                    && !(index.len() > 1 && index.starts_with('0')),
                "Unsupported ShareX JSON path"
            );
            pointer.push('/');
            pointer.push_str(index);
            rest = &stripped[end + 1..];
            continue;
        }
        if let Some(stripped) = rest.strip_prefix('.') {
            rest = stripped;
            continue;
        }
        let end = rest.find(['.', '[']).unwrap_or(rest.len());
        let key = &rest[..end];
        ensure!(
            !key.is_empty()
                && key
                    .bytes()
                    .all(|byte| byte.is_ascii_alphanumeric() || byte == b'_'),
            "Unsupported ShareX JSON path"
        );
        pointer.push('/');
        pointer.push_str(key);
        rest = &rest[end..];
    }
    ensure!(!pointer.is_empty(), "Unsupported ShareX JSON path");
    Ok(pointer)
}

fn sharex_json_path(pointer: &str) -> Result<String> {
    ensure!(
        pointer.starts_with('/') && pointer.len() > 1 && !pointer.contains('~'),
        "JSON pointer escapes are not supported in ShareX export"
    );
    let mut path = String::new();
    for segment in pointer[1..].split('/') {
        ensure!(
            !segment.is_empty() && !segment.chars().any(char::is_control),
            "Unsupported ShareX JSON path"
        );
        if segment.bytes().all(|byte| byte.is_ascii_digit())
            && !(segment.len() > 1 && segment.starts_with('0'))
        {
            path.push('[');
            path.push_str(segment);
            path.push(']');
            continue;
        }
        ensure!(
            segment
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || byte == b'_'),
            "Unsupported ShareX JSON path"
        );
        if !path.is_empty() {
            path.push('.');
        }
        path.push_str(segment);
    }
    ensure!(!path.is_empty(), "Unsupported ShareX JSON path");
    Ok(path)
}

#[derive(Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct NamedUploadDestination {
    name: String,
    profile: UploadDestination,
}

impl NamedUploadDestination {
    pub fn new(name: String, profile: UploadDestination) -> Result<Self> {
        let entry = Self {
            name: name.trim().into(),
            profile,
        };
        entry.validate()?;
        Ok(entry)
    }

    pub fn name(&self) -> &str {
        &self.name
    }

    pub fn profile(&self) -> &UploadDestination {
        &self.profile
    }

    fn validate(&self) -> Result<()> {
        ensure!(
            !self.name.is_empty()
                && self.name.len() <= 64
                && self.name.trim() == self.name
                && !self.name.chars().any(char::is_control),
            "Use a destination name of 1–64 bytes without control characters"
        );
        self.profile.validate()?;
        Ok(())
    }
}

#[derive(Clone, Default, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct UploadDestinationCatalog {
    destinations: Vec<NamedUploadDestination>,
}

impl UploadDestinationCatalog {
    fn path() -> Result<std::path::PathBuf> {
        Ok(crate::CaptureSettings::config_path()?.with_file_name("upload-destinations.json"))
    }

    pub fn load() -> Result<Self> {
        Self::load_at(&Self::path()?)
    }

    fn load_at(path: &Path) -> Result<Self> {
        match std::fs::File::open(path) {
            Ok(file) => {
                let mut bytes = Vec::new();
                file.take(65537).read_to_end(&mut bytes)?;
                ensure!(
                    bytes.len() <= 65536,
                    "Named destination list exceeds 64 KiB"
                );
                let catalog: Self =
                    serde_json::from_slice(&bytes).context("Invalid named destination list")?;
                catalog.validate()?;
                Ok(catalog)
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(Self::default()),
            Err(error) => Err(error.into()),
        }
    }

    pub fn save(&self) -> Result<()> {
        self.save_at(&Self::path()?)
    }

    fn save_at(&self, path: &Path) -> Result<()> {
        use std::io::Write;
        self.validate()?;
        let bytes = serde_json::to_vec_pretty(self)?;
        ensure!(
            bytes.len() <= 65536,
            "Named destination list exceeds 64 KiB"
        );
        let parent = path.parent().context("Invalid upload settings path")?;
        std::fs::create_dir_all(parent)?;
        let temporary = parent.join(format!("upload-named-{}.part", uuid::Uuid::new_v4()));
        let result = (|| -> Result<()> {
            let mut file = std::fs::OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(&temporary)?;
            file.write_all(&bytes)?;
            file.sync_all()?;
            drop(file);
            std::fs::rename(&temporary, path)?;
            Ok(())
        })();
        if result.is_err() {
            let _ = std::fs::remove_file(&temporary);
        }
        result
    }

    pub fn from_entries(destinations: Vec<NamedUploadDestination>) -> Result<Self> {
        let catalog = Self { destinations };
        catalog.validate()?;
        Ok(catalog)
    }

    pub fn entries(&self) -> &[NamedUploadDestination] {
        &self.destinations
    }

    pub fn into_entries(self) -> Vec<NamedUploadDestination> {
        self.destinations
    }

    pub fn upsert(&mut self, name: String, destination: UploadDestination) -> Result<()> {
        let entry = NamedUploadDestination::new(name, destination)?;
        if let Some(existing) = self
            .destinations
            .iter_mut()
            .find(|item| item.name == entry.name)
        {
            *existing = entry;
        } else {
            ensure!(
                self.destinations.len() < 16,
                "At most 16 named destinations can be saved"
            );
            self.destinations.push(entry);
        }
        self.validate()?;
        Ok(())
    }

    pub fn remove(&mut self, name: &str) -> bool {
        let before = self.destinations.len();
        self.destinations.retain(|item| item.name != name);
        before != self.destinations.len()
    }

    pub fn get(&self, name: &str) -> Option<&UploadDestination> {
        self.destinations
            .iter()
            .find(|item| item.name == name)
            .map(|item| &item.profile)
    }

    fn validate(&self) -> Result<()> {
        ensure!(
            self.destinations.len() <= 16,
            "At most 16 named destinations can be saved"
        );
        for (index, entry) in self.destinations.iter().enumerate() {
            entry.validate()?;
            ensure!(
                !self.destinations[..index]
                    .iter()
                    .any(|other| other.name == entry.name),
                "Destination names must be unique"
            );
        }
        Ok(())
    }
}

const UPLOAD_CONFIGURATION_MAX_BYTES: usize = 128 * 1024;

#[derive(Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct UploadConfiguration {
    version: u32,
    #[serde(default)]
    current: Option<UploadDestination>,
    #[serde(default)]
    destinations: Vec<NamedUploadDestination>,
}

impl UploadConfiguration {
    pub fn new(
        current: Option<UploadDestination>,
        destinations: Vec<NamedUploadDestination>,
    ) -> Result<Self> {
        let configuration = Self {
            version: 1,
            current,
            destinations,
        };
        configuration.validate()?;
        Ok(configuration)
    }

    pub fn current(&self) -> Option<&UploadDestination> {
        self.current.as_ref()
    }

    pub fn destinations(&self) -> &[NamedUploadDestination] {
        &self.destinations
    }

    pub fn into_parts(self) -> (Option<UploadDestination>, Vec<NamedUploadDestination>) {
        (self.current, self.destinations)
    }

    pub fn load(path: &Path) -> Result<Self> {
        Self::load_at(path)
    }

    fn load_at(path: &Path) -> Result<Self> {
        let mut bytes = Vec::new();
        std::fs::File::open(path)?
            .take((UPLOAD_CONFIGURATION_MAX_BYTES + 1) as u64)
            .read_to_end(&mut bytes)?;
        ensure!(
            bytes.len() <= UPLOAD_CONFIGURATION_MAX_BYTES,
            "Destination backup exceeds 128 KiB"
        );
        let configuration: Self =
            serde_json::from_slice(&bytes).context("Invalid destination backup")?;
        configuration.validate()?;
        Ok(configuration)
    }

    pub fn save(&self, path: &Path) -> Result<()> {
        self.save_at(path)
    }

    fn save_at(&self, path: &Path) -> Result<()> {
        use std::io::Write;
        self.validate()?;
        let bytes = serde_json::to_vec_pretty(self)?;
        ensure!(
            bytes.len() <= UPLOAD_CONFIGURATION_MAX_BYTES,
            "Destination backup exceeds 128 KiB"
        );
        let parent = path.parent().context("Invalid destination backup path")?;
        std::fs::create_dir_all(parent)?;
        let mut file = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(path)
            .map_err(|error| {
                if error.kind() == std::io::ErrorKind::AlreadyExists {
                    anyhow::anyhow!(
                        "The file already exists. Choose a new name; existing files are never overwritten."
                    )
                } else {
                    error.into()
                }
            })?;
        let result = (|| -> Result<()> {
            file.write_all(&bytes)?;
            file.sync_all()?;
            Ok(())
        })();
        if result.is_err() {
            drop(file);
            let _ = std::fs::remove_file(path);
        }
        result
    }

    pub fn persist(&self) -> Result<()> {
        let config = crate::CaptureSettings::config_path()?;
        let directory = config.parent().context("Invalid upload settings path")?;
        self.persist_at(directory)
    }

    fn persist_at(&self, directory: &Path) -> Result<()> {
        self.validate()?;
        let catalog = UploadDestinationCatalog::from_entries(self.destinations.clone())?;
        catalog.save_at(&directory.join("upload-destinations.json"))?;
        let remembered = directory.join("upload-destination.json");
        match &self.current {
            Some(destination) => destination.save_at(&remembered),
            None => UploadDestination::remove_at(&remembered),
        }
    }

    fn validate(&self) -> Result<()> {
        ensure!(self.version == 1, "Unsupported destination backup version");
        if let Some(current) = &self.current {
            current.validate()?;
        }
        UploadDestinationCatalog::from_entries(self.destinations.clone())?;
        Ok(())
    }
}

#[derive(Clone, Default)]
pub struct UploadCancellation(Arc<tokio::sync::Notify>);
impl UploadCancellation {
    pub fn cancel(&self) {
        self.0.notify_one();
    }
}

/// Run on a background executor. The operation owns and drops its network runtime/client.
pub fn upload_png(
    destination: &UploadDestination,
    path: &Path,
    cancel: UploadCancellation,
) -> Result<String> {
    Ok(run_upload(
        destination,
        UploadPng::Path(path.to_path_buf()),
        cancel,
        None,
    )?
    .url)
}

pub fn upload_png_with_progress(
    destination: &UploadDestination,
    path: &Path,
    cancel: UploadCancellation,
    progress: UploadProgressReporter,
) -> Result<UploadOutcome> {
    run_upload(
        destination,
        UploadPng::Path(path.to_path_buf()),
        cancel,
        Some(progress),
    )
}

/// Upload an already-encoded PNG buffer. Does not write a library file.
pub fn upload_png_bytes_with_progress(
    destination: &UploadDestination,
    png: Vec<u8>,
    cancel: UploadCancellation,
    progress: Option<UploadProgressReporter>,
) -> Result<UploadOutcome> {
    run_upload(destination, UploadPng::Bytes(png), cancel, progress)
}

/// Upload a throwaway 1×1 PNG through the existing transfer path, then delete it.
/// The destination profile, capture library and last-saved image are not modified.
pub fn probe_upload(
    destination: &UploadDestination,
    cancel: UploadCancellation,
    progress: Option<UploadProgressReporter>,
) -> Result<UploadOutcome> {
    let root =
        std::env::temp_dir().join(format!("screenshots-upload-probe-{}", uuid::Uuid::new_v4()));
    struct Cleanup(std::path::PathBuf);
    impl Drop for Cleanup {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }
    std::fs::create_dir_all(&root)?;
    let cleanup = Cleanup(root.clone());
    let path = write_probe_png(&root)?;
    let result = run_upload(destination, UploadPng::Path(path), cancel, progress);
    drop(cleanup);
    result
}

enum UploadPng {
    Path(std::path::PathBuf),
    Bytes(Vec<u8>),
}

fn write_probe_png(root: &Path) -> Result<std::path::PathBuf> {
    std::fs::create_dir_all(root)?;
    let path = root.join("probe.png");
    image::RgbaImage::new(1, 1)
        .save(&path)
        .context("Could not write the destination test image")?;
    Ok(path)
}

fn run_upload(
    destination: &UploadDestination,
    source: UploadPng,
    cancel: UploadCancellation,
    progress: Option<UploadProgressReporter>,
) -> Result<UploadOutcome> {
    let url = destination.validate()?;
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .max_blocking_threads(1)
        .build()?;
    runtime.block_on(async {
        tokio::select! {
            biased;
            _ = cancel.0.notified() => anyhow::bail!("Upload canceled. The server may already have received the file."),
            result = tokio::time::timeout(Duration::from_secs(60), transfer(destination, source, url, progress)) => {
                result.context("Upload exceeded its 60 second deadline; the server may have received the file")?
            }
        }
    })
}

async fn transfer(
    destination: &UploadDestination,
    source: UploadPng,
    url: reqwest::Url,
    progress: Option<UploadProgressReporter>,
) -> Result<UploadOutcome> {
    const PNG_SIGNATURE: [u8; 8] = [137, 80, 78, 71, 13, 10, 26, 10];
    let (length, png, mut file) = match source {
        UploadPng::Path(path) => {
            ensure!(path.is_absolute(), "Upload source must be an absolute path");
            let mut file = tokio::fs::File::open(&path)
                .await
                .context("Could not open the saved PNG")?;
            file.set_max_buf_size(64 * 1024);
            let metadata = file.metadata().await?;
            ensure!(
                metadata.is_file()
                    && metadata.len() >= 8
                    && metadata.len() <= crate::MAX_CAPTURE_BYTES + 16 * 1024 * 1024,
                "PNG file exceeds the upload size limit or is not a regular file"
            );
            let mut signature = [0; 8];
            file.read_exact(&mut signature).await?;
            ensure!(signature == PNG_SIGNATURE, "Upload source is not a PNG");
            file.rewind().await?;
            (metadata.len(), None, Some(file))
        }
        UploadPng::Bytes(png) => {
            ensure!(
                png.len() >= 8 && (png.len() as u64) <= crate::MAX_CAPTURE_BYTES + 16 * 1024 * 1024,
                "PNG file exceeds the upload size limit or is not a regular file"
            );
            ensure!(
                png.starts_with(&PNG_SIGNATURE),
                "Upload source is not a PNG"
            );
            (png.len() as u64, Some(png), None)
        }
    };
    let client = reqwest::Client::builder()
        .redirect(reqwest::redirect::Policy::none())
        .retry(reqwest::retry::never())
        .connect_timeout(Duration::from_secs(5))
        .timeout(Duration::from_secs(60))
        .http1_only()
        .pool_max_idle_per_host(0)
        .no_proxy()
        .no_gzip()
        .no_brotli()
        .no_deflate()
        .no_zstd()
        .build()?;
    let resource_url =
        matches!(destination.response, Some(UploadResponse::RequestUrl)).then(|| url.to_string());
    let mut request = if !destination.json.is_empty() {
        let png = match png {
            Some(png) => png,
            None => {
                let mut file = file.take().context("PNG file is missing")?;
                let mut png = Vec::new();
                png.try_reserve(length as usize)
                    .context("PNG file exceeds available memory for a JSON body")?;
                let mut buf = vec![0u8; 64 * 1024];
                let mut remaining = length;
                while remaining > 0 {
                    let want = buf.len().min(remaining as usize);
                    let n = file.read(&mut buf[..want]).await?;
                    ensure!(n > 0, "PNG file ended before the declared size");
                    png.extend_from_slice(&buf[..n]);
                    remaining -= n as u64;
                }
                png
            }
        };
        let json_bytes = json_request_body(&destination.json, &png)?;
        drop(png);
        let total = json_bytes.len() as u64;
        if let Some(progress) = &progress {
            progress
                .0
                .send_replace(UploadProgress { prepared: 0, total });
        }
        let reader = ProgressReader {
            reader: std::io::Cursor::new(json_bytes),
            progress,
            prepared: 0,
            total,
            last: std::time::Instant::now(),
        };
        let body = reqwest::Body::wrap_stream(tokio_util::io::ReaderStream::with_capacity(
            reader,
            64 * 1024,
        ));
        client
            .post(url)
            .header(reqwest::header::CONTENT_TYPE, "application/json")
            .header(reqwest::header::CONTENT_LENGTH, total)
            .body(body)
    } else {
        if let Some(progress) = &progress {
            progress.0.send_replace(UploadProgress {
                prepared: 0,
                total: length,
            });
        }
        let body = if let Some(png) = png {
            let reader = ProgressReader {
                reader: std::io::Cursor::new(png),
                progress,
                prepared: 0,
                total: length,
                last: std::time::Instant::now(),
            };
            reqwest::Body::wrap_stream(tokio_util::io::ReaderStream::with_capacity(
                reader,
                64 * 1024,
            ))
        } else {
            let file = file.take().context("PNG file is missing")?;
            let reader = ProgressReader {
                reader: file.take(length),
                progress,
                prepared: 0,
                total: length,
                last: std::time::Instant::now(),
            };
            reqwest::Body::wrap_stream(tokio_util::io::ReaderStream::with_capacity(
                reader,
                64 * 1024,
            ))
        };
        match destination.method {
            UploadMethod::Post => {
                let part = reqwest::multipart::Part::stream_with_length(body, length)
                    .file_name("screenshot.png")
                    .mime_str("image/png")?;
                let mut form = reqwest::multipart::Form::new();
                for (name, value) in &destination.form {
                    form = form.text(name.clone(), value.clone());
                }
                client
                    .post(url)
                    .multipart(form.part(destination.field.clone(), part))
            }
            UploadMethod::Put => client
                .put(url)
                .header(reqwest::header::CONTENT_TYPE, "image/png")
                .header(reqwest::header::CONTENT_LENGTH, length)
                .header(reqwest::header::IF_NONE_MATCH, "*")
                .body(body),
        }
    };
    for (name, value) in &destination.headers {
        request = request.header(
            reqwest::header::HeaderName::from_bytes(name.as_bytes())
                .map_err(|_| anyhow::anyhow!("Invalid custom request header name"))?,
            reqwest::header::HeaderValue::from_str(value)
                .map_err(|_| anyhow::anyhow!("Invalid custom request header value"))?,
        );
    }
    if let Some(target) = destination.credential_target() {
        let authentication = destination.authentication.unwrap_or_default();
        request = request.header(
            authentication.header_name(),
            crate::upload_credentials::read_header(&target, authentication)?,
        );
    }
    let mut response = request
        .header(reqwest::header::ACCEPT_ENCODING, "identity")
        .send()
        .await
        .map_err(|error| error.without_url())
        .context("Upload request failed; the server may have received the file")?;
    ensure!(
        response.status().is_success(),
        "Upload returned HTTP {}; redirects are not followed and uploads are not retried",
        response.status().as_u16()
    );
    ensure!(
        response
            .content_length()
            .is_none_or(|length| length <= MAX_RESPONSE as u64),
        "Upload response exceeds 1 MiB; the file may already be uploaded"
    );
    let headers = response.headers().clone();
    if let Some(url) = resource_url {
        ensure!(
            matches!(response.status().as_u16(), 200 | 201 | 204),
            "PUT did not confirm resource completion (HTTP {}); no completed resource link is available",
            response.status().as_u16()
        );
        return Ok(UploadOutcome {
            url: validate_link(&url)?,
            thumbnail: None,
            deletion: None,
        });
    }
    let need_json = destination.response.is_none() || destination.extra_needs_json();
    let need_text =
        response_needs_text(destination.response.as_ref()) || destination.extra_needs_text();
    let mut json: Option<serde_json::Value> = None;
    let mut text: Option<String> = None;
    if need_json || need_text {
        let mut bytes = Vec::new();
        while let Some(chunk) = response
            .chunk()
            .await
            .map_err(|error| error.without_url())
            .context("Upload response interrupted; the server may have received the file")?
        {
            ensure!(
                bytes.len().saturating_add(chunk.len()) <= MAX_RESPONSE,
                "Upload response exceeds 1 MiB; the file may already be uploaded"
            );
            bytes.extend_from_slice(&chunk);
        }
        if need_text {
            text = Some(
                std::str::from_utf8(&bytes)
                    .context("Upload completed, but its response is not UTF-8 text")?
                    .trim()
                    .to_owned(),
            );
        }
        if need_json {
            json = Some(
                serde_json::from_slice(&bytes)
                    .context("Upload completed, but its response is not valid JSON")?,
            );
        }
    }
    let url = if let Some(UploadResponse::Header { name }) = &destination.response {
        destination.result_link(&header_link(&headers, name)?)?
    } else if matches!(destination.response, Some(UploadResponse::Text)) {
        destination.result_link(
            text.as_deref()
                .context("Upload completed, but its response is not UTF-8 text")?,
        )?
    } else if let Some(UploadResponse::Regex { pattern, group }) = &destination.response {
        destination.result_link(&regex_capture(
            text.as_deref()
                .context("Upload completed, but its response is not UTF-8 text")?,
            pattern,
            group.as_deref(),
            "result",
        )?)?
    } else if let Some(UploadResponse::Xml { path }) = &destination.response {
        destination.result_link(&xml_capture(
            text.as_deref()
                .context("Upload completed, but its response is not UTF-8 text")?,
            path,
            "result",
        )?)?
    } else {
        let value = json
            .as_ref()
            .and_then(|json| json.pointer(&destination.json_pointer))
            .and_then(json_pointer_text)
            .context("Upload completed, but the configured JSON pointer contains no link")?;
        destination.result_link(&value)?
    };
    Ok(UploadOutcome {
        url,
        thumbnail: destination
            .thumbnail
            .as_ref()
            .map(|parser| parser.extract(&headers, json.as_ref(), text.as_deref(), "thumbnail"))
            .transpose()?,
        deletion: destination
            .deletion
            .as_ref()
            .map(|parser| parser.extract(&headers, json.as_ref(), text.as_deref(), "deletion"))
            .transpose()?,
    })
}

fn json_pointer_text(value: &serde_json::Value) -> Option<String> {
    match value {
        serde_json::Value::String(text) => Some(text.clone()),
        serde_json::Value::Number(number) => Some(number.to_string()),
        _ => None,
    }
}

impl UploadResultParser {
    fn from_sharex(
        template: &str,
        method: UploadMethod,
        allow_http: bool,
        label: &str,
    ) -> Result<Self> {
        let template = template.trim();
        ensure!(!template.is_empty(), "Use a {label} ShareX URL template");
        let (json_pointer, response, link_prefix, link_suffix) = sharex_response(template, method)?;
        let parser = Self {
            json_pointer,
            response,
            link_prefix,
            link_suffix,
        };
        parser.validate(method, allow_http, label)?;
        Ok(parser)
    }

    fn to_sharex(&self) -> Result<String> {
        sharex_url_template(
            self.response.as_ref(),
            &self.json_pointer,
            &self.link_prefix,
            &self.link_suffix,
        )
    }

    fn validate(&self, method: UploadMethod, allow_http: bool, label: &str) -> Result<()> {
        if let Some(response) = &self.response {
            ensure!(
                !matches!(response, UploadResponse::RequestUrl),
                "{label} results cannot use {{responseurl}}"
            );
            ensure!(
                self.json_pointer.is_empty(),
                "Choose either json_pointer or response for {label}, not both"
            );
            if let UploadResponse::Header { name } = response {
                ensure!(
                    name.len() <= 64
                        && reqwest::header::HeaderName::from_bytes(name.as_bytes()).is_ok(),
                    "Use a valid {label} response header name of at most 64 bytes"
                );
            }
            if let UploadResponse::Regex { pattern, group } = response {
                validate_regex_response(pattern, group.as_deref(), label)?;
            }
            if let UploadResponse::Xml { path } = response {
                validate_xml_response(path, label)?;
            }
        } else {
            ensure!(
                self.json_pointer.len() <= 256
                    && !self.json_pointer.is_empty()
                    && self.json_pointer.starts_with('/'),
                "Use a {label} JSON pointer up to 256 bytes, such as /thumbnail_url"
            );
            ensure!(
                self.json_pointer
                    .split('~')
                    .skip(1)
                    .all(|part| part.starts_with('0') || part.starts_with('1')),
                "Invalid {label} JSON pointer escape; use ~0 for ~ and ~1 for /"
            );
        }
        validate_parser_affixes(
            &self.link_prefix,
            &self.link_suffix,
            self.response.as_ref(),
            allow_http,
        )?;
        let _ = method;
        Ok(())
    }

    fn needs_json(&self) -> bool {
        self.response.is_none()
    }

    fn needs_text(&self) -> bool {
        response_needs_text(self.response.as_ref())
    }

    fn extract(
        &self,
        headers: &reqwest::header::HeaderMap,
        json: Option<&serde_json::Value>,
        text: Option<&str>,
        label: &str,
    ) -> Result<String> {
        if let Some(UploadResponse::Header { name }) = &self.response {
            return compose_result_link(
                &self.link_prefix,
                &self.link_suffix,
                &header_link(headers, name)?,
            );
        }
        if matches!(self.response, Some(UploadResponse::Text)) {
            return compose_result_link(
                &self.link_prefix,
                &self.link_suffix,
                text.with_context(|| {
                    format!("Upload completed, but the configured {label} text response is missing")
                })?,
            );
        }
        if let Some(UploadResponse::Regex { pattern, group }) = &self.response {
            return compose_result_link(
                &self.link_prefix,
                &self.link_suffix,
                &regex_capture(
                    text.with_context(|| {
                        format!(
                            "Upload completed, but the configured {label} text response is missing"
                        )
                    })?,
                    pattern,
                    group.as_deref(),
                    label,
                )?,
            );
        }
        if let Some(UploadResponse::Xml { path }) = &self.response {
            return compose_result_link(
                &self.link_prefix,
                &self.link_suffix,
                &xml_capture(
                    text.with_context(|| {
                        format!(
                            "Upload completed, but the configured {label} text response is missing"
                        )
                    })?,
                    path,
                    label,
                )?,
            );
        }
        let value = json
            .with_context(|| {
                format!("Upload completed, but the configured {label} JSON response is missing")
            })?
            .pointer(&self.json_pointer)
            .and_then(json_pointer_text)
            .with_context(|| {
                format!(
                    "Upload completed, but the configured {label} JSON pointer contains no link"
                )
            })?;
        compose_result_link(&self.link_prefix, &self.link_suffix, &value)
    }
}

fn response_needs_text(response: Option<&UploadResponse>) -> bool {
    matches!(
        response,
        Some(UploadResponse::Text | UploadResponse::Regex { .. } | UploadResponse::Xml { .. })
    )
}

fn validate_regex_group(group: &str, label: &str) -> Result<()> {
    if let Ok(index) = group.parse::<usize>() {
        ensure!(
            group.bytes().all(|byte| byte.is_ascii_digit())
                && !(group.len() > 1 && group.starts_with('0'))
                && index <= MAX_REGEX_GROUP_INDEX,
            "Use a {label} regex group index from 0 to {MAX_REGEX_GROUP_INDEX}"
        );
        return Ok(());
    }
    ensure!(
        group.len() <= 32
            && group
                .bytes()
                .next()
                .is_some_and(|byte| byte.is_ascii_alphabetic() || byte == b'_')
            && group
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || byte == b'_'),
        "Use a {label} regex group name of 1–32 letters, digits or underscores"
    );
    Ok(())
}

fn compile_result_regex(pattern: &str) -> Result<regex::Regex> {
    regex::RegexBuilder::new(pattern)
        .size_limit(MAX_REGEX_SIZE)
        .dfa_size_limit(MAX_REGEX_SIZE)
        .nest_limit(MAX_REGEX_NEST)
        .build()
        .context("Upload regex pattern is invalid or too complex")
}

fn validate_regex_response(pattern: &str, group: Option<&str>, label: &str) -> Result<()> {
    ensure!(
        !pattern.is_empty()
            && pattern.len() <= MAX_REGEX_PATTERN
            && pattern.trim() == pattern
            && !pattern.chars().any(char::is_control),
        "Use a {label} regex of 1–256 bytes without control characters"
    );
    if let Some(group) = group {
        validate_regex_group(group, label)?;
    }
    let compiled = compile_result_regex(pattern)?;
    if let Some(group) = group {
        if let Ok(index) = group.parse::<usize>() {
            ensure!(
                compiled.captures_len() > index,
                "{label} regex has no capture group {group}"
            );
        } else {
            ensure!(
                compiled.capture_names().any(|name| name == Some(group)),
                "{label} regex has no capture group named {group}"
            );
        }
    }
    Ok(())
}

fn regex_capture(text: &str, pattern: &str, group: Option<&str>, label: &str) -> Result<String> {
    let compiled = compile_result_regex(pattern)?;
    let captures = compiled.captures(text).with_context(|| {
        format!("Upload completed, but the configured {label} regex found no match")
    })?;
    let value = match group {
        None => captures.get(0).map(|matched| matched.as_str()),
        Some(group) => {
            if let Ok(index) = group.parse::<usize>() {
                captures.get(index).map(|matched| matched.as_str())
            } else {
                captures.name(group).map(|matched| matched.as_str())
            }
        }
    }
    .with_context(|| {
        format!("Upload completed, but the configured {label} regex group is empty")
    })?;
    ensure!(
        !value.is_empty(),
        "Upload completed, but the configured {label} regex group is empty"
    );
    Ok(value.to_owned())
}

fn is_xml_name(name: &str) -> bool {
    let mut bytes = name.bytes();
    matches!(
        bytes.next(),
        Some(byte) if byte.is_ascii_alphabetic() || byte == b'_'
    ) && bytes
        .all(|byte| byte.is_ascii_alphanumeric() || byte == b'_' || byte == b'-' || byte == b'.')
        && name.len() <= 64
}

fn parse_xml_child_step(step: &str, label: &str) -> Result<(String, usize)> {
    if let Some(name) = step.strip_suffix(']') {
        let (name, index) = name
            .split_once('[')
            .context(format!("Use a valid {label} XML path step"))?;
        ensure!(
            is_xml_name(name),
            "Use a {label} XML element name of 1–64 letters, digits, underscores, hyphens or dots"
        );
        ensure!(
            !index.is_empty()
                && index.bytes().all(|byte| byte.is_ascii_digit())
                && !(index.len() > 1 && index.starts_with('0')),
            "Use a {label} XML index from 1 to {MAX_XML_INDEX}"
        );
        let position: usize = index.parse()?;
        ensure!(
            (1..=MAX_XML_INDEX).contains(&position),
            "Use a {label} XML index from 1 to {MAX_XML_INDEX}"
        );
        return Ok((name.to_owned(), position));
    }
    ensure!(
        is_xml_name(step),
        "Use a {label} XML element name of 1–64 letters, digits, underscores, hyphens or dots"
    );
    Ok((step.to_owned(), 1))
}

fn parse_xml_path(path: &str, label: &str) -> Result<Vec<XmlPathStep>> {
    ensure!(
        !path.is_empty()
            && path.len() <= MAX_XML_PATH
            && path.trim() == path
            && path.starts_with('/')
            && !path.contains("//")
            && !path
                .chars()
                .any(|character| character.is_whitespace() || character.is_control()),
        "Use a {label} XML path of 1–256 bytes starting with /"
    );
    let mut steps = Vec::new();
    for (index, step) in path.split('/').enumerate() {
        if index == 0 {
            ensure!(step.is_empty(), "Use a {label} XML path starting with /");
            continue;
        }
        ensure!(
            !step.is_empty(),
            "Use a {label} XML path without empty steps"
        );
        ensure!(
            steps.len() < MAX_XML_STEPS,
            "Use a {label} XML path of at most {MAX_XML_STEPS} steps"
        );
        if step == "text()" {
            steps.push(XmlPathStep::Text);
            continue;
        }
        if let Some(name) = step.strip_prefix('@') {
            ensure!(
                is_xml_name(name),
                "Use a {label} XML attribute name of 1–64 letters, digits, underscores, hyphens or dots"
            );
            steps.push(XmlPathStep::Attribute {
                name: name.to_owned(),
            });
            continue;
        }
        let (name, position) = parse_xml_child_step(step, label)?;
        steps.push(XmlPathStep::Child { name, position });
    }
    ensure!(
        !steps.is_empty(),
        "Use a {label} XML path with at least one step"
    );
    ensure!(
        matches!(steps.first(), Some(XmlPathStep::Child { .. })),
        "Use a {label} XML path that starts with an element"
    );
    ensure!(
        steps.iter().enumerate().all(|(index, step)| {
            index + 1 == steps.len() || matches!(step, XmlPathStep::Child { .. })
        }),
        "Use text() or @attribute only as the last {label} XML step"
    );
    Ok(steps)
}

fn validate_xml_response(path: &str, label: &str) -> Result<()> {
    let _ = parse_xml_path(path, label)?;
    Ok(())
}

fn xml_inner_text(node: roxmltree::Node<'_, '_>) -> String {
    node.descendants()
        .filter(|child| child.is_text())
        .filter_map(|child| child.text())
        .collect::<String>()
        .trim()
        .to_owned()
}

fn xml_capture(text: &str, path: &str, label: &str) -> Result<String> {
    let steps = parse_xml_path(path, label)?;
    let document = roxmltree::Document::parse_with_options(
        text,
        roxmltree::ParsingOptions {
            nodes_limit: MAX_XML_NODES,
            ..roxmltree::ParsingOptions::default()
        },
    )
    .with_context(|| format!("Upload completed, but the configured {label} XML is invalid"))?;
    let mut current = document.root();
    let last = steps.len() - 1;
    for (index, step) in steps.iter().enumerate() {
        match step {
            XmlPathStep::Child { name, position } => {
                current = current
                    .children()
                    .filter(|node| node.is_element() && node.tag_name().name() == name)
                    .nth(position - 1)
                    .with_context(|| {
                        format!(
                            "Upload completed, but the configured {label} XML path contains no link"
                        )
                    })?;
            }
            XmlPathStep::Attribute { name } => {
                ensure!(
                    index == last,
                    "Use @attribute only as the last {label} XML step"
                );
                let value = current.attribute(name.as_str()).with_context(|| {
                    format!(
                        "Upload completed, but the configured {label} XML path contains no link"
                    )
                })?;
                let value = value.trim();
                ensure!(
                    !value.is_empty(),
                    "Upload completed, but the configured {label} XML path is empty"
                );
                return Ok(value.to_owned());
            }
            XmlPathStep::Text => {
                ensure!(
                    index == last,
                    "Use text() only as the last {label} XML step"
                );
                let value = xml_inner_text(current);
                ensure!(
                    !value.is_empty(),
                    "Upload completed, but the configured {label} XML path is empty"
                );
                return Ok(value);
            }
        }
    }
    let value = xml_inner_text(current);
    ensure!(
        !value.is_empty(),
        "Upload completed, but the configured {label} XML path is empty"
    );
    Ok(value)
}

fn split_sharex_escaped(value: &str, delimiter: char) -> Result<Vec<String>> {
    let mut parts = Vec::new();
    let mut current = String::new();
    let mut chars = value.chars();
    while let Some(ch) = chars.next() {
        if ch == '\\' {
            match chars.next() {
                Some(escaped) if matches!(escaped, '{' | '}' | '|' | '\\') => current.push(escaped),
                Some(other) => {
                    current.push('\\');
                    current.push(other);
                }
                None => anyhow::bail!("This ShareX URL syntax is not supported"),
            }
        } else if ch == delimiter {
            parts.push(std::mem::take(&mut current));
        } else {
            current.push(ch);
        }
    }
    parts.push(current);
    Ok(parts)
}

fn sharex_escape(value: &str) -> String {
    let mut escaped = String::new();
    for ch in value.chars() {
        if matches!(ch, '{' | '}' | '|' | '\\') {
            escaped.push('\\');
        }
        escaped.push(ch);
    }
    escaped
}

fn header_link(headers: &reqwest::header::HeaderMap, name: &str) -> Result<String> {
    let mut values = headers.get_all(name).iter();
    let value = values
        .next()
        .context("Upload completed, but the configured response header is missing")?;
    ensure!(
        values.next().is_none(),
        "Upload completed, but the response link header is repeated"
    );
    Ok(value
        .to_str()
        .context("Upload response header is not valid text")?
        .trim()
        .to_owned())
}

fn validate_parser_affixes(
    prefix: &str,
    suffix: &str,
    response: Option<&UploadResponse>,
    allow_http: bool,
) -> Result<()> {
    if matches!(response, Some(UploadResponse::RequestUrl)) {
        ensure!(
            prefix.is_empty() && suffix.is_empty(),
            "PUT request-URL results cannot use a link prefix or suffix"
        );
        return Ok(());
    }
    ensure!(
        prefix.trim() == prefix
            && prefix.len() <= MAX_LINK_PREFIX
            && !prefix.chars().any(char::is_control)
            && !contains_upload_template(prefix),
        "Use a result link prefix of at most 256 bytes without templates or control characters"
    );
    ensure!(
        suffix.trim() == suffix
            && suffix.len() <= MAX_LINK_SUFFIX
            && !suffix.chars().any(char::is_control)
            && !contains_upload_template(suffix)
            && !suffix.contains("://"),
        "Use a result link suffix of at most 64 bytes without templates, schemes or control characters"
    );
    if prefix.is_empty() {
        return Ok(());
    }
    let parsed =
        reqwest::Url::parse(prefix).context("Result link prefix is not an absolute URL")?;
    ensure!(
        parsed.host_str().is_some()
            && parsed.username().is_empty()
            && parsed.password().is_none()
            && parsed.query().is_none()
            && parsed.fragment().is_none(),
        "Result link prefixes must have a host and no embedded credentials, query or fragment"
    );
    ensure!(
        parsed.scheme() == "https" || (parsed.scheme() == "http" && allow_http),
        "Use HTTPS for the result link prefix, or explicitly enable allow_http"
    );
    Ok(())
}

fn compose_result_link(prefix: &str, suffix: &str, value: &str) -> Result<String> {
    let value = value.trim();
    if prefix.is_empty() && suffix.is_empty() {
        return validate_link(value);
    }
    if prefix.is_empty() {
        let base = validate_link(value)?;
        let composed = format!("{base}{suffix}");
        let link = validate_link(&composed)?;
        let base_url = reqwest::Url::parse(&base)?;
        let composed_url = reqwest::Url::parse(&link)?;
        ensure!(
            base_url.origin().ascii_serialization() == composed_url.origin().ascii_serialization(),
            "Composed result link must keep the extracted URL origin"
        );
        return Ok(link);
    }
    ensure!(
        !value.is_empty()
            && value.len() <= MAX_LINK_FRAGMENT
            && !value
                .chars()
                .any(|character| character.is_whitespace() || character.is_control())
            && !contains_upload_template(value)
            && !value.contains("://")
            && !value.contains('\\')
            && !value.contains('?')
            && !value.contains('#'),
        "Use a result path of 1–512 bytes without templates, schemes, query, fragment or control characters"
    );
    let composed = format!("{prefix}{value}{suffix}");
    let link = validate_link(&composed)?;
    let prefix_url =
        reqwest::Url::parse(prefix).context("Result link prefix is not an absolute URL")?;
    let composed_url = reqwest::Url::parse(&link)?;
    ensure!(
        prefix_url.origin().ascii_serialization() == composed_url.origin().ascii_serialization(),
        "Composed result link must keep the configured prefix origin"
    );
    Ok(link)
}

fn sharex_url_template(
    response: Option<&UploadResponse>,
    json_pointer: &str,
    prefix: &str,
    suffix: &str,
) -> Result<String> {
    let token = match (response, json_pointer) {
        (Some(UploadResponse::Text), "") => "{response}".into(),
        (Some(UploadResponse::Header { name }), "") => format!("{{header:{name}}}"),
        (Some(UploadResponse::RequestUrl), "") => "{responseurl}".into(),
        (
            Some(UploadResponse::Regex {
                pattern,
                group: None,
            }),
            "",
        ) => {
            format!("{{regex:{}}}", sharex_escape(pattern))
        }
        (
            Some(UploadResponse::Regex {
                pattern,
                group: Some(group),
            }),
            "",
        ) => {
            format!("{{regex:{}|{}}}", sharex_escape(pattern), group)
        }
        (Some(UploadResponse::Xml { path }), "") => {
            format!("{{xml:{}}}", sharex_escape(path))
        }
        (None, pointer) if !pointer.is_empty() => {
            format!("{{json:{}}}", sharex_json_path(pointer)?)
        }
        _ => anyhow::bail!(
            "ShareX export needs a JSON pointer, text, header, regex, XML or PUT request-URL response"
        ),
    };
    let url = format!("{prefix}{token}{suffix}");
    ensure!(
        url.len() <= 256 && !url.chars().any(char::is_control),
        "ShareX URL parser is invalid or exceeds 256 bytes"
    );
    Ok(url)
}

pub(crate) fn validate_link(value: &str) -> Result<String> {
    ensure!(
        value.len() <= 2048 && !value.chars().any(char::is_control),
        "Upload response link is invalid or too long"
    );
    let link = reqwest::Url::parse(value).context("Upload response link is not an absolute URL")?;
    ensure!(
        matches!(link.scheme(), "http" | "https")
            && link.host_str().is_some()
            && link.username().is_empty()
            && link.password().is_none(),
        "Upload response link must be an HTTP(S) URL without embedded credentials"
    );
    Ok(link.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{
        io::Write,
        net::{TcpListener, TcpStream},
        sync::mpsc,
        thread,
    };

    #[test]
    fn manual_destinations_validate_and_reuse_saved_profile_policy() -> Result<()> {
        for method in [UploadMethod::Post, UploadMethod::Put] {
            for pointer in [None, Some("/result/url".to_string()), Some(String::new())] {
                let config = UploadDestination::manual(
                    " https://example.invalid/upload ".into(),
                    method,
                    " file ".into(),
                    pointer.clone(),
                    false,
                )?;
                assert_eq!(config.url, "https://example.invalid/upload");
                assert_eq!(config.field, "file");
                assert!(config.credential_target().is_none() && config.authentication.is_none());
                assert_eq!(
                    config.request_method(),
                    if method == UploadMethod::Post {
                        "POST"
                    } else {
                        "PUT"
                    }
                );
                assert_eq!(config.json_pointer, pointer.clone().unwrap_or_default());
                assert_eq!(
                    matches!(config.response, Some(UploadResponse::Text)),
                    pointer.is_none()
                );
                let root = std::env::temp_dir()
                    .join(format!("manual-destination-{}", uuid::Uuid::new_v4()));
                std::fs::create_dir_all(&root)?;
                let path = root.join("destination.json");
                config.save_at(&path)?;
                let loaded = UploadDestination::load(&path)?;
                assert_eq!(loaded.request_method(), config.request_method());
                assert_eq!(loaded.json_pointer, config.json_pointer);
                assert!(loaded.credential_target().is_none());
                std::fs::remove_file(path)?;
                std::fs::remove_dir(root)?;
            }
        }
        for url in [
            "",
            "file:///tmp/image",
            "http://localhost/upload",
            "https://user:pass@example.invalid/",
            "https://example.invalid/?token=x",
            "https://example.invalid/#fragment",
        ] {
            assert!(
                UploadDestination::manual(
                    url.into(),
                    UploadMethod::Post,
                    "file".into(),
                    None,
                    false
                )
                .is_err()
            );
        }
        assert!(
            UploadDestination::manual(
                "http://localhost/upload".into(),
                UploadMethod::Put,
                "file".into(),
                None,
                true
            )
            .is_ok()
        );
        assert!(
            UploadDestination::manual(
                "https://example.invalid/".into(),
                UploadMethod::Post,
                "bad field".into(),
                None,
                false
            )
            .is_err()
        );
        assert!(
            UploadDestination::manual(
                "https://example.invalid/".into(),
                UploadMethod::Post,
                "file".into(),
                Some("/bad~2".into()),
                false
            )
            .is_err()
        );
        Ok(())
    }

    #[test]
    fn custom_request_headers_validate_persist_and_reject_reserved_names() -> Result<()> {
        let destination = UploadDestination::manual(
            "https://example.invalid/upload".into(),
            UploadMethod::Post,
            "file".into(),
            Some("/url".into()),
            false,
        )?;
        assert!(destination.request_headers().is_empty());
        let destination = destination
            .with_headers([(" X-Client ", " screenshots "), ("X-Request-Id", "abc-123")])?;
        assert_eq!(
            destination
                .request_headers()
                .get("X-Client")
                .map(String::as_str),
            Some("screenshots")
        );
        assert_eq!(
            destination
                .request_headers()
                .get("X-Request-Id")
                .map(String::as_str),
            Some("abc-123")
        );
        let root =
            std::env::temp_dir().join(format!("screenshots-headers-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&root)?;
        let path = root.join("destination.json");
        let other = root.join("settings.json");
        std::fs::write(&other, b"keep-settings")?;
        destination.save_at(&path)?;
        let loaded = UploadDestination::load(&path)?;
        assert_eq!(loaded.request_headers(), destination.request_headers());
        let json = std::fs::read_to_string(&path)?;
        assert!(json.contains("X-Client"));
        assert!(json.contains("screenshots"));
        assert!(!json.contains("Authorization"));
        assert!(!json.contains("secret"));
        std::fs::write(
            &path,
            r#"{"url":"https://example.invalid/upload","field":"file","json_pointer":"/url","headers":{"X-Client":"screenshots"}}"#,
        )?;
        let loaded = UploadDestination::load(&path)?;
        assert_eq!(
            loaded.request_headers().get("X-Client").map(String::as_str),
            Some("screenshots")
        );
        for (name, value) in [
            ("Authorization", "Bearer secret"),
            ("authorization", "x"),
            ("X-API-Key", "secret"),
            ("Content-Type", "text/plain"),
            ("Accept-Encoding", "gzip"),
            ("If-None-Match", "*"),
            ("Host", "evil.example"),
            ("Cookie", "a=b"),
            ("X-Foo", "{json:token}"),
            ("X-Foo", "$filename$"),
            ("X-Foo", "a\nb"),
            ("", "x"),
            ("bad name", "x"),
            ("X-Foo", ""),
        ] {
            assert!(
                destination.clone().with_headers([(name, value)]).is_err(),
                "{name}={value}"
            );
        }
        assert!(
            destination
                .clone()
                .with_headers([("X-Foo", "x".repeat(257))])
                .is_err()
        );
        assert!(
            destination
                .clone()
                .with_headers([("x".repeat(65), "x")])
                .is_err()
        );
        let many: Vec<(String, String)> = (0..9)
            .map(|index| (format!("X-H{index}"), "v".into()))
            .collect();
        assert!(destination.clone().with_headers(many).is_err());
        assert!(
            destination
                .clone()
                .with_headers([("X-Foo", "a"), ("x-foo", "b")])
                .is_err()
        );
        std::fs::write(
            &path,
            r#"{"url":"https://example.invalid/upload","field":"file","json_pointer":"/url","headers":{"Authorization":"Bearer secret"}}"#,
        )?;
        assert!(UploadDestination::load(&path).is_err());
        assert_eq!(std::fs::read(&other)?, b"keep-settings");
        std::fs::remove_dir_all(root)?;
        Ok(())
    }

    #[test]
    fn query_parameters_validate_persist_and_reject_secrets() -> Result<()> {
        let destination = UploadDestination::manual(
            "https://example.invalid/upload".into(),
            UploadMethod::Post,
            "file".into(),
            Some("/url".into()),
            false,
        )?;
        assert!(destination.request_query().is_empty());
        let destination =
            destination.with_query([(" format ", " json "), ("client_id", "public-app")])?;
        assert_eq!(
            destination
                .request_query()
                .get("format")
                .map(String::as_str),
            Some("json")
        );
        assert_eq!(
            destination
                .request_query()
                .get("client_id")
                .map(String::as_str),
            Some("public-app")
        );
        let composed = destination.validate()?;
        assert_eq!(composed.query(), Some("client_id=public-app&format=json"));
        assert_eq!(destination.origin(), "https://example.invalid");
        let root = std::env::temp_dir().join(format!("screenshots-query-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&root)?;
        let path = root.join("destination.json");
        let other = root.join("settings.json");
        std::fs::write(&other, b"keep-settings")?;
        destination.save_at(&path)?;
        let loaded = UploadDestination::load(&path)?;
        assert_eq!(loaded.request_query(), destination.request_query());
        let json = std::fs::read_to_string(&path)?;
        assert!(json.contains("\"query\""));
        assert!(json.contains("format"));
        assert!(!json.contains("api_key"));
        assert!(!json.contains("secret"));
        assert!(!json.contains("?format="));
        std::fs::write(
            &path,
            r#"{"url":"https://example.invalid/upload","field":"file","json_pointer":"/url","query":{"format":"json"}}"#,
        )?;
        let loaded = UploadDestination::load(&path)?;
        assert_eq!(
            loaded.request_query().get("format").map(String::as_str),
            Some("json")
        );
        for (name, value) in [
            ("api_key", "secret"),
            ("key", "secret"),
            ("token", "x"),
            ("Authorization", "x"),
            ("format", "{json:token}"),
            ("format", "$filename$"),
            ("format", "a\nb"),
            ("", "x"),
            ("bad name", "x"),
            ("format", ""),
        ] {
            assert!(
                destination.clone().with_query([(name, value)]).is_err(),
                "{name}={value}"
            );
        }
        assert!(
            destination
                .clone()
                .with_query([("format", "x".repeat(257))])
                .is_err()
        );
        assert!(
            destination
                .clone()
                .with_query([("x".repeat(65), "x")])
                .is_err()
        );
        let many: Vec<(String, String)> = (0..9)
            .map(|index| (format!("q{index}"), "v".into()))
            .collect();
        assert!(destination.clone().with_query(many).is_err());
        assert!(
            destination
                .clone()
                .with_query([("format", "a"), ("Format", "b")])
                .is_err()
        );
        std::fs::write(
            &path,
            r#"{"url":"https://example.invalid/upload","field":"file","json_pointer":"/url","query":{"api_key":"secret"}}"#,
        )?;
        assert!(UploadDestination::load(&path).is_err());
        std::fs::write(
            &path,
            r#"{"url":"https://example.invalid/upload?format=json","field":"file","json_pointer":"/url"}"#,
        )?;
        assert!(UploadDestination::load(&path).is_err());
        let long = format!("https://example.invalid/{}", "a".repeat(2020));
        assert!(
            UploadDestination::manual(
                long,
                UploadMethod::Post,
                "file".into(),
                Some("/url".into()),
                false
            )?
            .with_query([("format", "json")])
            .is_err()
        );
        assert_eq!(std::fs::read(&other)?, b"keep-settings");
        std::fs::remove_dir_all(root)?;
        Ok(())
    }

    #[test]
    fn form_fields_validate_persist_and_reject_secrets() -> Result<()> {
        let destination = UploadDestination::manual(
            "https://example.invalid/upload".into(),
            UploadMethod::Post,
            "file".into(),
            Some("/url".into()),
            false,
        )?;
        assert!(destination.request_form().is_empty());
        let destination = destination.with_form([(" title ", " screenshot "), ("type", "file")])?;
        assert_eq!(
            destination.request_form().get("title").map(String::as_str),
            Some("screenshot")
        );
        assert_eq!(
            destination.request_form().get("type").map(String::as_str),
            Some("file")
        );
        let root = std::env::temp_dir().join(format!("screenshots-form-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&root)?;
        let path = root.join("destination.json");
        let other = root.join("settings.json");
        std::fs::write(&other, b"keep-settings")?;
        destination.save_at(&path)?;
        let loaded = UploadDestination::load(&path)?;
        assert_eq!(loaded.request_form(), destination.request_form());
        let json = std::fs::read_to_string(&path)?;
        assert!(json.contains("\"form\""));
        assert!(json.contains("title"));
        assert!(!json.contains("api_key"));
        assert!(!json.contains("secret"));
        std::fs::write(
            &path,
            r#"{"url":"https://example.invalid/upload","field":"file","json_pointer":"/url","form":{"title":"screenshot"}}"#,
        )?;
        let loaded = UploadDestination::load(&path)?;
        assert_eq!(
            loaded.request_form().get("title").map(String::as_str),
            Some("screenshot")
        );
        for (name, value) in [
            ("api_key", "secret"),
            ("key", "secret"),
            ("token", "x"),
            ("file", "x"),
            ("title", "{json:token}"),
            ("title", "$filename$"),
            ("title", "a\nb"),
            ("", "x"),
            ("bad name", "x"),
            ("title", ""),
        ] {
            assert!(
                destination.clone().with_form([(name, value)]).is_err(),
                "{name}={value}"
            );
        }
        assert!(
            destination
                .clone()
                .with_form([("title", "x".repeat(257))])
                .is_err()
        );
        assert!(
            destination
                .clone()
                .with_form([("x".repeat(65), "x")])
                .is_err()
        );
        let many: Vec<(String, String)> = (0..9)
            .map(|index| (format!("f{index}"), "v".into()))
            .collect();
        assert!(destination.clone().with_form(many).is_err());
        assert!(
            destination
                .clone()
                .with_form([("title", "a"), ("Title", "b")])
                .is_err()
        );
        std::fs::write(
            &path,
            r#"{"url":"https://example.invalid/upload","field":"file","json_pointer":"/url","form":{"api_key":"secret"}}"#,
        )?;
        assert!(UploadDestination::load(&path).is_err());
        assert!(
            UploadDestination::manual(
                "https://example.invalid/image.png".into(),
                UploadMethod::Put,
                "file".into(),
                Some("/url".into()),
                false,
            )?
            .with_form([("title", "screenshot")])
            .is_err()
        );
        assert_eq!(std::fs::read(&other)?, b"keep-settings");
        std::fs::remove_dir_all(root)?;
        Ok(())
    }

    #[test]
    fn json_bodies_validate_persist_and_reject_secrets() -> Result<()> {
        let destination = UploadDestination::manual(
            "https://example.invalid/upload".into(),
            UploadMethod::Post,
            "file".into(),
            Some("/url".into()),
            false,
        )?;
        assert!(destination.request_json().is_empty());
        let destination = destination.with_json([(" image ", " {base64} "), ("type", "file")])?;
        assert_eq!(
            destination.request_json().get("image").map(String::as_str),
            Some("{base64}")
        );
        assert_eq!(
            destination.request_json().get("type").map(String::as_str),
            Some("file")
        );
        let root =
            std::env::temp_dir().join(format!("screenshots-json-body-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&root)?;
        let path = root.join("destination.json");
        let other = root.join("settings.json");
        std::fs::write(&other, b"keep-settings")?;
        destination.save_at(&path)?;
        let loaded = UploadDestination::load(&path)?;
        assert_eq!(loaded.request_json(), destination.request_json());
        let json = std::fs::read_to_string(&path)?;
        assert!(json.contains("\"json\""));
        assert!(json.contains("{base64}"));
        assert!(!json.contains("api_key"));
        assert!(!json.contains("secret"));
        std::fs::write(
            &path,
            r#"{"url":"https://example.invalid/upload","field":"file","json_pointer":"/url","json":{"image":"{base64}","type":"file"}}"#,
        )?;
        let loaded = UploadDestination::load(&path)?;
        assert_eq!(
            loaded.request_json().get("image").map(String::as_str),
            Some("{base64}")
        );
        for (name, value) in [
            ("api_key", "secret"),
            ("key", "secret"),
            ("token", "x"),
            ("image", "{input}"),
            ("image", "{filename}"),
            ("image", "prefix{base64}"),
            ("image", "$filename$"),
            ("image", "a\nb"),
            ("", "x"),
            ("bad name", "x"),
            ("type", ""),
        ] {
            assert!(
                destination.clone().with_json([(name, value)]).is_err(),
                "{name}={value}"
            );
        }
        assert!(
            destination
                .clone()
                .with_json([("type", "x".repeat(257))])
                .is_err()
        );
        assert!(
            destination
                .clone()
                .with_json([("x".repeat(65), "{base64}")])
                .is_err()
        );
        let many: Vec<(String, String)> = (0..8)
            .map(|index| {
                (
                    format!("f{index}"),
                    if index == 0 {
                        "{base64}".into()
                    } else {
                        "v".into()
                    },
                )
            })
            .collect();
        assert!(destination.clone().with_json(many).is_ok());
        let too_many: Vec<(String, String)> = (0..9)
            .map(|index| {
                (
                    format!("f{index}"),
                    if index == 0 {
                        "{base64}".into()
                    } else {
                        "v".into()
                    },
                )
            })
            .collect();
        assert!(destination.clone().with_json(too_many).is_err());
        assert!(
            destination
                .clone()
                .with_json([("image", "{base64}"), ("Image", "file")])
                .is_err()
        );
        assert!(
            destination
                .clone()
                .with_json([("image", "{base64}"), ("thumb", "{base64}")])
                .is_err()
        );
        assert!(destination.clone().with_json([("type", "file")]).is_err());
        std::fs::write(
            &path,
            r#"{"url":"https://example.invalid/upload","field":"file","json_pointer":"/url","json":{"api_key":"secret"}}"#,
        )?;
        assert!(UploadDestination::load(&path).is_err());
        assert!(
            destination
                .clone()
                .with_form([("title", "screenshot")])
                .is_err()
        );
        assert!(
            UploadDestination::manual(
                "https://example.invalid/image.png".into(),
                UploadMethod::Put,
                "file".into(),
                Some("/url".into()),
                false,
            )?
            .with_json([("image", "{base64}")])
            .is_err()
        );
        assert_eq!(std::fs::read(&other)?, b"keep-settings");
        std::fs::remove_dir_all(root)?;
        Ok(())
    }

    #[test]
    fn composed_result_links_validate_persist_and_keep_origin() -> Result<()> {
        let destination = UploadDestination::manual(
            "https://example.invalid/upload".into(),
            UploadMethod::Post,
            "file".into(),
            Some("/id".into()),
            false,
        )?;
        assert!(destination.link_prefix().is_empty());
        assert!(destination.link_suffix().is_empty());
        let destination =
            destination.with_link_affixes(" https://cdn.example.invalid/images/ ", " .png ")?;
        assert_eq!(
            destination.link_prefix(),
            "https://cdn.example.invalid/images/"
        );
        assert_eq!(destination.link_suffix(), ".png");
        assert_eq!(
            destination.result_link("abc")?,
            "https://cdn.example.invalid/images/abc.png"
        );
        assert_eq!(
            destination.result_link("123")?,
            "https://cdn.example.invalid/images/123.png"
        );
        let root = std::env::temp_dir().join(format!(
            "screenshots-composed-link-{}",
            uuid::Uuid::new_v4()
        ));
        std::fs::create_dir_all(&root)?;
        let path = root.join("destination.json");
        let other = root.join("settings.json");
        std::fs::write(&other, b"keep-settings")?;
        destination.save_at(&path)?;
        let loaded = UploadDestination::load(&path)?;
        assert_eq!(loaded.link_prefix(), destination.link_prefix());
        assert_eq!(loaded.link_suffix(), destination.link_suffix());
        let json = std::fs::read_to_string(&path)?;
        assert!(json.contains("link_prefix"));
        assert!(json.contains("link_suffix"));
        assert!(!json.contains("api_key"));
        std::fs::write(
            &path,
            r#"{"url":"https://example.invalid/upload","field":"file","json_pointer":"/id","link_prefix":"https://cdn.example.invalid/","link_suffix":".png"}"#,
        )?;
        let loaded = UploadDestination::load(&path)?;
        assert_eq!(
            loaded.result_link("abc")?,
            "https://cdn.example.invalid/abc.png"
        );
        for (prefix, suffix) in [
            ("https://cdn.example.invalid/{json:id}", ""),
            ("https://user:secret@cdn.example.invalid/", ""),
            ("https://cdn.example.invalid/?x=1", ""),
            ("https://cdn.example.invalid/#frag", ""),
            ("https://", ""),
            ("http://cdn.example.invalid/", ""),
            ("file:///tmp/", ""),
            ("", "://png"),
            ("https://cdn.example.invalid/", "{json:id}"),
            ("https://cdn.example.invalid/", "a\nb"),
        ] {
            assert!(
                destination
                    .clone()
                    .with_link_affixes(prefix, suffix)
                    .is_err(),
                "{prefix}{suffix}"
            );
        }
        assert!(destination.result_link("https://evil.invalid/x").is_err());
        assert!(destination.result_link("abc?x=1").is_err());
        assert!(destination.result_link("abc#frag").is_err());
        assert!(destination.result_link("").is_err());
        assert!(destination.result_link(&"x".repeat(513)).is_err());
        let suffix_only = UploadDestination::manual(
            "https://example.invalid/upload".into(),
            UploadMethod::Post,
            "file".into(),
            Some("/url".into()),
            false,
        )?
        .with_link_affixes("", ".png")?;
        assert_eq!(
            suffix_only.result_link("https://cdn.example.invalid/image")?,
            "https://cdn.example.invalid/image.png"
        );
        assert!(suffix_only.result_link("/relative").is_err());
        let mut put = UploadDestination::manual(
            "https://example.invalid/image.png".into(),
            UploadMethod::Put,
            "file".into(),
            None,
            false,
        )?;
        put.response = Some(UploadResponse::RequestUrl);
        put.json_pointer.clear();
        assert!(
            put.clone()
                .with_link_affixes("https://cdn.example.invalid/", "")
                .is_err()
        );
        assert_eq!(std::fs::read(&other)?, b"keep-settings");
        std::fs::remove_dir_all(root)?;
        Ok(())
    }

    #[test]
    fn extra_result_urls_validate_persist_and_reject_request_url() -> Result<()> {
        let destination = UploadDestination::manual(
            "https://example.invalid/upload".into(),
            UploadMethod::Post,
            "file".into(),
            Some("/url".into()),
            false,
        )?
        .with_thumbnail(" {json:thumbnail_url} ")?
        .with_deletion("{json:delete}")?;
        assert_eq!(
            destination.thumbnail_template().as_deref(),
            Some("{json:thumbnail_url}")
        );
        assert_eq!(
            destination.deletion_template().as_deref(),
            Some("{json:delete}")
        );
        let composed = destination
            .clone()
            .with_thumbnail("https://cdn.example.invalid/{json:thumb}.png")?;
        assert_eq!(
            composed.thumbnail_template().as_deref(),
            Some("https://cdn.example.invalid/{json:thumb}.png")
        );
        let headed = destination.clone().with_deletion("{header:X-Delete}")?;
        assert_eq!(
            headed.deletion_template().as_deref(),
            Some("{header:X-Delete}")
        );
        let root =
            std::env::temp_dir().join(format!("screenshots-extra-links-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&root)?;
        let path = root.join("destination.json");
        let other = root.join("settings.json");
        std::fs::write(&other, b"keep-settings")?;
        destination.save_at(&path)?;
        let loaded = UploadDestination::load(&path)?;
        assert_eq!(
            loaded.thumbnail_template(),
            destination.thumbnail_template()
        );
        assert_eq!(loaded.deletion_template(), destination.deletion_template());
        let json = std::fs::read_to_string(&path)?;
        assert!(json.contains("thumbnail_url"));
        assert!(json.contains("\"deletion\""));
        assert!(!json.contains("api_key"));
        std::fs::write(
            &path,
            r#"{"url":"https://example.invalid/upload","field":"file","json_pointer":"/url","thumbnail":{"json_pointer":"/thumbnail_url"},"deletion":{"json_pointer":"/delete"}}"#,
        )?;
        let loaded = UploadDestination::load(&path)?;
        assert_eq!(
            loaded.thumbnail_template().as_deref(),
            Some("{json:thumbnail_url}")
        );
        assert_eq!(loaded.deletion_template().as_deref(), Some("{json:delete}"));
        std::fs::write(
            &path,
            r#"{"url":"https://example.invalid/upload","field":"file","json_pointer":"/url","thumbnail":{"json_pointer":"/thumb","unknown":true}}"#,
        )?;
        assert!(UploadDestination::load(&path).is_err());
        assert!(destination.clone().with_thumbnail("{regex}").is_err());
        assert!(destination.clone().with_thumbnail("{xml}").is_err());
        assert!(destination.clone().with_thumbnail("{xml://url}").is_err());
        assert!(destination.clone().with_thumbnail("{responseurl}").is_err());
        assert!(destination.clone().with_deletion("").is_err());
        let mut put = UploadDestination::manual(
            "https://example.invalid/image.png".into(),
            UploadMethod::Put,
            "file".into(),
            None,
            false,
        )?;
        put.response = Some(UploadResponse::RequestUrl);
        put.json_pointer.clear();
        assert!(put.clone().with_thumbnail("{json:thumb}").is_err());
        assert!(put.with_deletion("{json:delete}").is_err());
        std::fs::write(
            &path,
            r#"{"url":"https://example.invalid/image.png","method":"PUT","response":{"type":"request_url"},"thumbnail":{"json_pointer":"/thumb"}}"#,
        )?;
        assert!(UploadDestination::load(&path).is_err());
        assert_eq!(std::fs::read(&other)?, b"keep-settings");
        std::fs::remove_dir_all(root)?;
        Ok(())
    }

    #[test]
    fn regex_result_urls_validate_persist_and_capture_groups() -> Result<()> {
        let destination = UploadDestination::manual(
            "https://example.invalid/upload".into(),
            UploadMethod::Post,
            "file".into(),
            Some("/url".into()),
            false,
        )?
        .with_regex(r#" href="(.+)" "#, Some(" 1 "))?;
        assert_eq!(destination.regex_pattern(), Some(r#"href="(.+)""#));
        assert_eq!(destination.regex_group(), Some("1"));
        assert_eq!(
            regex_capture(
                r#"<a href="https://example.invalid/share/abc">ok</a>"#,
                destination.regex_pattern().unwrap(),
                destination.regex_group(),
                "result",
            )?,
            "https://example.invalid/share/abc"
        );
        let named = destination
            .clone()
            .with_regex(r#"href="(?<url>[^"]+)""#, Some("url"))?;
        assert_eq!(named.regex_group(), Some("url"));
        assert_eq!(
            regex_capture(
                r#"<a href="https://example.invalid/share/abc">ok</a>"#,
                named.regex_pattern().unwrap(),
                named.regex_group(),
                "result",
            )?,
            "https://example.invalid/share/abc"
        );
        let whole = destination
            .clone()
            .with_regex(r#"https://example\.invalid/\S+"#, None)?;
        assert!(whole.regex_group().is_none());
        let composed = destination.clone().with_regex("id=([0-9]+)", Some("1"))?;
        let composed = composed.with_link_affixes("https://cdn.example.invalid/", ".png")?;
        assert_eq!(
            composed.result_link(&regex_capture(
                "id=123",
                "id=([0-9]+)",
                Some("1"),
                "result"
            )?)?,
            "https://cdn.example.invalid/123.png"
        );
        let root =
            std::env::temp_dir().join(format!("screenshots-regex-link-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&root)?;
        let path = root.join("destination.json");
        let other = root.join("settings.json");
        std::fs::write(&other, b"keep-settings")?;
        destination.save_at(&path)?;
        let loaded = UploadDestination::load(&path)?;
        assert_eq!(loaded.regex_pattern(), destination.regex_pattern());
        assert_eq!(loaded.regex_group(), destination.regex_group());
        let json = std::fs::read_to_string(&path)?;
        assert!(json.contains("\"regex\""));
        assert!(json.contains("href="));
        assert!(!json.contains("api_key"));
        std::fs::write(
            &path,
            r#"{"url":"https://example.invalid/upload","field":"file","response":{"type":"regex","pattern":"href=\"([^\"]+)\"","group":"1"}}"#,
        )?;
        let loaded = UploadDestination::load(&path)?;
        assert_eq!(loaded.regex_pattern(), Some(r#"href="([^"]+)""#));
        assert_eq!(loaded.regex_group(), Some("1"));
        assert!(destination.clone().with_regex("", None).is_err());
        assert!(destination.clone().with_regex("(", None).is_err());
        assert!(
            destination
                .clone()
                .with_regex(r#"(?<=href=").+"#, None)
                .is_err()
        );
        assert!(destination.clone().with_regex("href", Some("9")).is_err());
        assert!(
            destination
                .clone()
                .with_regex("href", Some("missing"))
                .is_err()
        );
        assert!(
            destination
                .clone()
                .with_regex(&"a".repeat(257), None)
                .is_err()
        );
        let extras = destination
            .clone()
            .with_thumbnail(r#"{regex:thumb="(.+)"|1}"#)?;
        assert_eq!(
            extras.thumbnail_template().as_deref(),
            Some(r#"{regex:thumb="(.+)"|1}"#)
        );
        assert_eq!(std::fs::read(&other)?, b"keep-settings");
        std::fs::remove_dir_all(root)?;
        Ok(())
    }

    #[test]
    fn xml_result_urls_validate_persist_and_select_nodes() -> Result<()> {
        let body = r#"<?xml version="1.0" encoding="UTF-8"?>
<files>
    <file>
        <name>image.png</name>
        <url>https://example.invalid/share/abc</url>
    </file>
    <file href="https://example.invalid/share/second">
        <url>https://example.invalid/share/other</url>
    </file>
</files>"#;
        let destination = UploadDestination::manual(
            "https://example.invalid/upload".into(),
            UploadMethod::Post,
            "file".into(),
            Some("/url".into()),
            false,
        )?
        .with_xml(" /files/file[1]/url ")?;
        assert_eq!(destination.xml_path(), Some("/files/file[1]/url"));
        assert_eq!(
            xml_capture(body, destination.xml_path().unwrap(), "result")?,
            "https://example.invalid/share/abc"
        );
        assert_eq!(
            xml_capture(body, "/files/file/url", "result")?,
            "https://example.invalid/share/abc"
        );
        assert_eq!(
            xml_capture(body, "/files/file[2]/@href", "result")?,
            "https://example.invalid/share/second"
        );
        assert_eq!(
            xml_capture(body, "/files/file[1]/url/text()", "result")?,
            "https://example.invalid/share/abc"
        );
        let composed = destination.clone().with_xml("/files/file[1]/name")?;
        let composed = composed.with_link_affixes("https://cdn.example.invalid/", "")?;
        assert_eq!(
            composed.result_link(&xml_capture(body, "/files/file[1]/name", "result")?)?,
            "https://cdn.example.invalid/image.png"
        );
        let root =
            std::env::temp_dir().join(format!("screenshots-xml-link-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&root)?;
        let path = root.join("destination.json");
        let other = root.join("settings.json");
        std::fs::write(&other, b"keep-settings")?;
        destination.save_at(&path)?;
        let loaded = UploadDestination::load(&path)?;
        assert_eq!(loaded.xml_path(), destination.xml_path());
        let json = std::fs::read_to_string(&path)?;
        assert!(json.contains("\"xml\""));
        assert!(json.contains("/files/file[1]/url"));
        assert!(!json.contains("api_key"));
        std::fs::write(
            &path,
            r#"{"url":"https://example.invalid/upload","field":"file","response":{"type":"xml","path":"/files/file[1]/url"}}"#,
        )?;
        let loaded = UploadDestination::load(&path)?;
        assert_eq!(loaded.xml_path(), Some("/files/file[1]/url"));
        assert!(destination.clone().with_xml("").is_err());
        assert!(destination.clone().with_xml("files/file/url").is_err());
        assert!(destination.clone().with_xml("//files/file/url").is_err());
        assert!(destination.clone().with_xml("/files/*").is_err());
        assert!(destination.clone().with_xml("/files/file[0]/url").is_err());
        assert!(
            destination
                .clone()
                .with_xml("/files/file[257]/url")
                .is_err()
        );
        assert!(destination.clone().with_xml("/@href").is_err());
        assert!(destination.clone().with_xml("/text()").is_err());
        assert!(destination.clone().with_xml("/a/b/c/d/e/f/g/h/i").is_err());
        assert!(destination.clone().with_xml(&"/a".repeat(130)).is_err());
        assert!(xml_capture("not xml", "/files/url", "result").is_err());
        assert!(xml_capture(
            r#"<!DOCTYPE files [<!ELEMENT files ANY>]><files><url>https://example.invalid/x</url></files>"#,
            "/files/url",
            "result",
        )
        .is_err());
        assert!(xml_capture(body, "/files/file[3]/url", "result").is_err());
        let extras = destination
            .clone()
            .with_thumbnail("{xml:/files/file[1]/name}")?;
        assert_eq!(
            extras.thumbnail_template().as_deref(),
            Some("{xml:/files/file[1]/name}")
        );
        assert_eq!(std::fs::read(&other)?, b"keep-settings");
        std::fs::remove_dir_all(root)?;
        Ok(())
    }

    #[test]
    fn progress_coalesces_closes_and_counts_only_consumed_png_bytes() {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        runtime.block_on(async {
            let (reporter, mut receiver) = upload_progress_channel();
            for prepared in 0..1000 {
                reporter.0.send_replace(UploadProgress {
                    prepared,
                    total: 2000,
                });
            }
            let latest = tokio::time::timeout(Duration::from_secs(1), receiver.next())
                .await
                .unwrap()
                .unwrap();
            assert_eq!((latest.prepared, latest.total), (999, 2000));
            drop(reporter);
            assert!(
                tokio::time::timeout(Duration::from_secs(1), receiver.next())
                    .await
                    .unwrap()
                    .is_none()
            );
            let (reporter, mut receiver) = upload_progress_channel();
            let bytes = vec![12u8; 128 * 1024];
            let mut reader = ProgressReader {
                reader: bytes.as_slice(),
                progress: Some(reporter),
                prepared: 0,
                total: bytes.len() as u64,
                last: std::time::Instant::now(),
            };
            let mut output = Vec::new();
            reader.read_to_end(&mut output).await.unwrap();
            assert_eq!(output, bytes);
            let progress = tokio::time::timeout(Duration::from_secs(1), receiver.next())
                .await
                .unwrap()
                .unwrap();
            assert_eq!(
                (progress.prepared, progress.total),
                (bytes.len() as u64, bytes.len() as u64)
            );
            drop(reader);
            assert!(receiver.next().await.is_none());
        });
    }

    fn fixture() -> (std::path::PathBuf, std::path::PathBuf) {
        let root =
            std::env::temp_dir().join(format!("screenshots-upload-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&root).unwrap();
        let path = root.join("private-source-name.png");
        image::RgbaImage::new(3, 2).save(&path).unwrap();
        (root, path)
    }
    fn destination(url: String) -> UploadDestination {
        UploadDestination::manual(
            url,
            UploadMethod::Post,
            "file".into(),
            Some("/result/url".into()),
            true,
        )
        .unwrap()
    }
    fn read_request(stream: &mut TcpStream) -> Vec<u8> {
        // Accepted sockets can inherit the listener's nonblocking mode on Windows.
        stream.set_nonblocking(false).unwrap();
        stream
            .set_read_timeout(Some(Duration::from_secs(3)))
            .unwrap();
        stream
            .set_write_timeout(Some(Duration::from_secs(3)))
            .unwrap();
        let mut request = Vec::new();
        let mut byte = [0; 1];
        while !request.ends_with(b"\r\n\r\n") {
            assert!(request.len() < 8192);
            stream.read_exact(&mut byte).unwrap();
            request.push(byte[0]);
        }
        let headers = String::from_utf8(request.clone()).unwrap();
        let length: usize = headers
            .lines()
            .find_map(|line| {
                let (name, value) = line.split_once(':')?;
                name.eq_ignore_ascii_case("content-length")
                    .then(|| value.trim().parse().unwrap())
            })
            .expect("Multipart request must have a bounded known length");
        assert!(length < 1024 * 1024);
        let offset = request.len();
        request.resize(offset + length, 0);
        stream.read_exact(&mut request[offset..]).unwrap();
        request
    }
    fn server(
        response: Vec<u8>,
        received: Option<mpsc::SyncSender<()>>,
        stall: Duration,
    ) -> (String, thread::JoinHandle<Vec<u8>>) {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let url = format!("http://{}/upload", listener.local_addr().unwrap());
        listener.set_nonblocking(true).unwrap();
        let thread = thread::spawn(move || {
            let started = std::time::Instant::now();
            let mut stream = loop {
                match listener.accept() {
                    Ok((stream, _)) => break stream,
                    Err(error)
                        if error.kind() == std::io::ErrorKind::WouldBlock
                            && started.elapsed() < Duration::from_secs(3) =>
                    {
                        thread::sleep(Duration::from_millis(1))
                    }
                    error => panic!("Loopback upload server accept failed: {error:?}"),
                }
            };
            let request = read_request(&mut stream);
            if let Some(received) = received {
                received.send(()).unwrap();
            }
            if !stall.is_zero() {
                thread::sleep(stall);
            }
            let _ = stream.write_all(&response);
            request
        });
        (url, thread)
    }
    fn response(json: &str) -> Vec<u8> {
        format!(
            "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{json}",
            json.len()
        )
        .into_bytes()
    }
    fn probe_temp_names() -> std::collections::HashSet<String> {
        std::fs::read_dir(std::env::temp_dir())
            .into_iter()
            .flatten()
            .filter_map(|entry| entry.ok())
            .filter_map(|entry| {
                let name = entry.file_name().into_string().ok()?;
                name.starts_with("screenshots-upload-probe-")
                    .then_some(name)
            })
            .collect()
    }

    #[test]
    fn probe_png_is_tiny_and_probe_upload_cleans_up_without_connecting() -> Result<()> {
        let root = std::env::temp_dir().join(format!(
            "screenshots-probe-png-check-{}",
            uuid::Uuid::new_v4()
        ));
        let path = write_probe_png(&root)?;
        let bytes = std::fs::read(&path)?;
        assert!(bytes.starts_with(&[137, 80, 78, 71, 13, 10, 26, 10]));
        assert!(bytes.len() < 1024);
        let (width, height) = image::image_dimensions(&path)?;
        assert_eq!((width, height), (1, 1));
        std::fs::remove_dir_all(&root)?;

        let before = probe_temp_names();
        let mut config = destination("https://example.invalid/upload".into());
        config.url = "https://user:pass@example.invalid/upload".into();
        assert!(probe_upload(&config, UploadCancellation::default(), None).is_err());
        assert_eq!(before, probe_temp_names());
        Ok(())
    }

    #[test]
    fn remembered_destination_round_trips_replaces_and_forgets_without_touching_other_settings()
    -> Result<()> {
        let root =
            std::env::temp_dir().join(format!("screenshots-profile-{}", uuid::Uuid::new_v4()));
        let path = root.join("upload-destination.json");
        assert!(UploadDestination::load_optional(&path)?.is_none());
        assert!(!root.exists());
        let first = destination("https://first.example.invalid/upload".into());
        first.save_at(&path)?;
        assert_eq!(
            UploadDestination::load_optional(&path)?.unwrap().origin(),
            first.origin()
        );
        let second = destination("https://second.example.invalid/upload".into());
        second.save_at(&path)?;
        let bytes = std::fs::read(&path)?;
        assert_eq!(
            UploadDestination::load_optional(&path)?.unwrap().origin(),
            second.origin()
        );
        let mut invalid = first;
        invalid.url = "https://user:secret@example.invalid/".into();
        assert!(invalid.save_at(&path).is_err());
        assert_eq!(std::fs::read(&path)?, bytes);
        let other = root.join("settings.json");
        std::fs::write(&other, b"keep")?;
        std::fs::write(&path, b"invalid JSON")?;
        assert!(UploadDestination::load_optional(&path).is_err());
        UploadDestination::remove_at(&path)?;
        UploadDestination::remove_at(&path)?;
        assert!(UploadDestination::load_optional(&path)?.is_none());
        assert_eq!(std::fs::read(&other)?, b"keep");
        assert_eq!(std::fs::read_dir(&root)?.count(), 1);
        let blocked = other.join("upload-destination.json");
        assert!(second.save_at(&blocked).is_err());
        assert_eq!(std::fs::read(&other)?, b"keep");
        std::fs::remove_dir_all(root)?;
        Ok(())
    }

    #[test]
    fn named_destinations_round_trip_replace_remove_and_keep_other_files() -> Result<()> {
        let root = std::env::temp_dir().join(format!("screenshots-named-{}", uuid::Uuid::new_v4()));
        let path = root.join("upload-destinations.json");
        let remembered = root.join("upload-destination.json");
        std::fs::create_dir_all(&root)?;
        std::fs::write(&remembered, b"keep-current")?;
        assert!(
            UploadDestinationCatalog::load_at(&path)?
                .entries()
                .is_empty()
        );
        let mut catalog = UploadDestinationCatalog::default();
        catalog.upsert(
            " Host ".into(),
            destination("https://first.example.invalid/upload".into()),
        )?;
        assert_eq!(catalog.entries()[0].name(), "Host");
        catalog.save_at(&path)?;
        let loaded = UploadDestinationCatalog::load_at(&path)?;
        assert_eq!(loaded.entries().len(), 1);
        assert_eq!(loaded.entries()[0].name(), "Host");
        assert_eq!(
            loaded.get("Host").unwrap().origin(),
            "https://first.example.invalid"
        );
        let mut catalog = loaded;
        catalog.upsert(
            "Host".into(),
            destination("https://second.example.invalid/upload".into()),
        )?;
        catalog.upsert(
            "Other".into(),
            destination("https://other.example.invalid/upload".into()),
        )?;
        catalog.save_at(&path)?;
        let loaded = UploadDestinationCatalog::load_at(&path)?;
        assert_eq!(loaded.entries().len(), 2);
        assert_eq!(
            loaded.get("Host").unwrap().origin(),
            "https://second.example.invalid"
        );
        let text = String::from_utf8(std::fs::read(&path)?)?;
        assert!(!text.contains("secret"));
        assert!(
            UploadDestination::manual(
                "https://user:secret@example.invalid/upload".into(),
                UploadMethod::Post,
                "file".into(),
                Some("/result/url".into()),
                true,
            )
            .is_err()
        );
        let bytes = std::fs::read(&path)?;
        let mut catalog = UploadDestinationCatalog::load_at(&path)?;
        catalog
            .upsert(
                "".into(),
                destination("https://third.example.invalid/upload".into()),
            )
            .unwrap_err();
        assert_eq!(std::fs::read(&path)?, bytes);
        for name in ["", "bad\nname", &"x".repeat(65)] {
            assert!(
                NamedUploadDestination::new(
                    name.into(),
                    destination("https://example.invalid/upload".into())
                )
                .is_err()
            );
        }
        let mut catalog = UploadDestinationCatalog::default();
        for index in 0..16 {
            catalog.upsert(
                format!("Host {index}"),
                destination("https://example.invalid/upload".into()),
            )?;
        }
        assert!(
            catalog
                .upsert(
                    "Overflow".into(),
                    destination("https://example.invalid/upload".into()),
                )
                .is_err()
        );
        catalog.upsert(
            "Host 0".into(),
            destination("https://replaced.example.invalid/upload".into()),
        )?;
        assert_eq!(catalog.entries().len(), 16);
        assert_eq!(
            catalog.get("Host 0").unwrap().origin(),
            "https://replaced.example.invalid"
        );
        assert!(catalog.remove("Host 0"));
        assert!(!catalog.remove("Host 0"));
        assert_eq!(catalog.entries().len(), 15);
        catalog.save_at(&path)?;
        assert_eq!(std::fs::read(&remembered)?, b"keep-current");
        let blocked = remembered.join("upload-destinations.json");
        assert!(catalog.save_at(&blocked).is_err());
        assert_eq!(std::fs::read(&remembered)?, b"keep-current");
        std::fs::write(&path, b"invalid JSON")?;
        assert!(UploadDestinationCatalog::load_at(&path).is_err());
        std::fs::remove_dir_all(root)?;
        Ok(())
    }

    #[test]
    fn upload_configuration_round_trips_without_secrets_or_overwrites() -> Result<()> {
        let root = std::env::temp_dir().join(format!(
            "screenshots-upload-config-{}",
            uuid::Uuid::new_v4()
        ));
        std::fs::create_dir_all(&root)?;
        let export = root.join("backup.json");
        let live = root.join("live");
        std::fs::create_dir_all(&live)?;
        let remembered = live.join("upload-destination.json");
        let other = live.join("settings.json");
        std::fs::write(&other, b"keep-settings")?;
        std::fs::write(&remembered, b"keep-current")?;
        let current = destination("https://current.example.invalid/upload".into())
            .with_authentication("main".into(), UploadAuthentication::Bearer)?;
        let named = NamedUploadDestination::new(
            " Host ".into(),
            destination("https://named.example.invalid/upload".into()),
        )?;
        let configuration = UploadConfiguration::new(Some(current), vec![named])?;
        configuration.save_at(&export)?;
        let text = String::from_utf8(std::fs::read(&export)?)?;
        assert!(text.contains("\"version\": 1"));
        assert!(text.contains("main"));
        assert!(text.contains("Host"));
        assert!(!text.contains("secret"));
        assert!(
            UploadDestination::manual(
                "https://user:secret@example.invalid/upload".into(),
                UploadMethod::Post,
                "file".into(),
                Some("/result/url".into()),
                true,
            )
            .is_err()
        );
        let loaded = UploadConfiguration::load_at(&export)?;
        assert_eq!(
            loaded.current().unwrap().origin(),
            "https://current.example.invalid"
        );
        assert_eq!(loaded.destinations()[0].name(), "Host");
        loaded.persist_at(&live)?;
        assert_eq!(
            UploadDestination::load_optional(&remembered)?
                .unwrap()
                .origin(),
            "https://current.example.invalid"
        );
        assert_eq!(
            UploadDestinationCatalog::load_at(&live.join("upload-destinations.json"))?.entries()[0]
                .name(),
            "Host"
        );
        assert_eq!(std::fs::read(&other)?, b"keep-settings");
        let empty = UploadConfiguration::new(None, Vec::new())?;
        empty.persist_at(&live)?;
        assert!(UploadDestination::load_optional(&remembered)?.is_none());
        assert!(
            UploadDestinationCatalog::load_at(&live.join("upload-destinations.json"))?
                .entries()
                .is_empty()
        );
        assert_eq!(std::fs::read(&other)?, b"keep-settings");
        let existing = std::fs::read(&export)?;
        assert!(configuration.save_at(&export).is_err());
        assert_eq!(std::fs::read(&export)?, existing);
        assert!(UploadConfiguration::load_at(&other).is_err());
        std::fs::write(&export, b"{\"version\":2}")?;
        assert!(UploadConfiguration::load_at(&export).is_err());
        let blocked = other.join("backup.json");
        assert!(configuration.save_at(&blocked).is_err());
        assert_eq!(std::fs::read(&other)?, b"keep-settings");
        std::fs::remove_dir_all(root)?;
        Ok(())
    }

    #[test]
    fn sharex_profiles_map_supported_subset_and_reject_secrets() -> Result<()> {
        let root =
            std::env::temp_dir().join(format!("screenshots-sharex-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&root)?;
        let path = root.join("host.sxcu");
        let other = root.join("settings.json");
        std::fs::write(&other, b"keep-settings")?;
        std::fs::write(
            &path,
            r#"{
                "Version": "17.0.0",
                "Name": " Host ",
                "DestinationType": "ImageUploader, FileUploader",
                "RequestMethod": "POST",
                "RequestURL": "https://example.invalid/upload",
                "Body": "MultipartFormData",
                "FileFormName": "file",
                "URL": "{json:url}",
                "ThumbnailURL": "{json:thumbnail_url}"
            }"#,
        )?;
        let (destination, name) = UploadDestination::from_sharex_at(&path)?;
        assert_eq!(name.as_deref(), Some("Host"));
        assert_eq!(destination.origin(), "https://example.invalid");
        assert_eq!(destination.method, UploadMethod::Post);
        assert_eq!(destination.field, "file");
        assert_eq!(destination.json_pointer, "/url");
        assert!(destination.response.is_none());
        assert_eq!(
            destination.thumbnail_template().as_deref(),
            Some("{json:thumbnail_url}")
        );
        assert!(destination.deletion_template().is_none());
        std::fs::write(
            &path,
            r#"{"DestinationType":"ImageUploader","RequestURL":"https://example.invalid/upload","URL":"{json:files[0].url}"}"#,
        )?;
        let (destination, name) = UploadDestination::from_sharex_at(&path)?;
        assert!(name.is_none());
        assert_eq!(destination.json_pointer, "/files/0/url");
        std::fs::write(
            &path,
            r#"{"DestinationType":"ImageUploader","RequestMethod":"PUT","RequestURL":"https://example.invalid/image.png","Body":"Binary","URL":"{responseurl}"}"#,
        )?;
        let (destination, _) = UploadDestination::from_sharex_at(&path)?;
        assert_eq!(destination.method, UploadMethod::Put);
        assert_eq!(destination.response, Some(UploadResponse::RequestUrl));
        std::fs::write(
            &path,
            r#"{"DestinationType":"ImageUploader","RequestURL":"https://example.invalid/upload","URL":"{header:Location}"}"#,
        )?;
        let (destination, _) = UploadDestination::from_sharex_at(&path)?;
        assert_eq!(
            destination.response,
            Some(UploadResponse::Header {
                name: "Location".into()
            })
        );
        std::fs::write(
            &path,
            r#"{"DestinationType":"ImageUploader","RequestURL":"https://example.invalid/upload"}"#,
        )?;
        let (destination, _) = UploadDestination::from_sharex_at(&path)?;
        assert_eq!(destination.response, Some(UploadResponse::Text));
        std::fs::write(
            &path,
            r#"{"DestinationType":"ImageUploader","RequestURL":"https://example.invalid/upload","Headers":{"X-Client":"screenshots"},"URL":"{json:url}"}"#,
        )?;
        let (destination, _) = UploadDestination::from_sharex_at(&path)?;
        assert_eq!(
            destination
                .request_headers()
                .get("X-Client")
                .map(String::as_str),
            Some("screenshots")
        );
        std::fs::write(
            &path,
            r#"{"DestinationType":"ImageUploader","RequestURL":"https://example.invalid/upload","Parameters":{"format":"json"},"URL":"{json:url}"}"#,
        )?;
        let (destination, _) = UploadDestination::from_sharex_at(&path)?;
        assert_eq!(
            destination
                .request_query()
                .get("format")
                .map(String::as_str),
            Some("json")
        );
        std::fs::write(
            &path,
            r#"{"DestinationType":"ImageUploader","RequestURL":"https://example.invalid/upload","Arguments":{"title":"screenshot"},"URL":"{json:url}"}"#,
        )?;
        let (destination, _) = UploadDestination::from_sharex_at(&path)?;
        assert_eq!(
            destination.request_form().get("title").map(String::as_str),
            Some("screenshot")
        );
        std::fs::write(
            &path,
            r#"{"DestinationType":"ImageUploader","RequestURL":"https://example.invalid/upload","Body":"JSON","Data":{"image":"{base64}","type":"file"},"URL":"{json:url}"}"#,
        )?;
        let (destination, _) = UploadDestination::from_sharex_at(&path)?;
        assert_eq!(
            destination.request_json().get("image").map(String::as_str),
            Some("{base64}")
        );
        assert_eq!(
            destination.request_json().get("type").map(String::as_str),
            Some("file")
        );
        assert!(destination.request_form().is_empty());
        std::fs::write(
            &path,
            r#"{"DestinationType":"ImageUploader","RequestURL":"https://example.invalid/upload","URL":"https://cdn.example.invalid/{json:id}.png"}"#,
        )?;
        let (destination, _) = UploadDestination::from_sharex_at(&path)?;
        assert_eq!(destination.json_pointer, "/id");
        assert_eq!(destination.link_prefix(), "https://cdn.example.invalid/");
        assert_eq!(destination.link_suffix(), ".png");
        assert_eq!(
            destination.result_link("abc")?,
            "https://cdn.example.invalid/abc.png"
        );
        std::fs::write(
            &path,
            r#"{"DestinationType":"ImageUploader","RequestURL":"https://example.invalid/upload","URL":"{regex:href=\"(.+)\"|1}"}"#,
        )?;
        let (destination, _) = UploadDestination::from_sharex_at(&path)?;
        assert_eq!(destination.regex_pattern(), Some(r#"href="(.+)""#));
        assert_eq!(destination.regex_group(), Some("1"));
        std::fs::write(
            &path,
            r#"{"DestinationType":"ImageUploader","RequestURL":"https://example.invalid/upload","URL":"{regex:href=\"(?<url>.+)\"|url}"}"#,
        )?;
        let (destination, _) = UploadDestination::from_sharex_at(&path)?;
        assert_eq!(destination.regex_group(), Some("url"));
        std::fs::write(
            &path,
            r#"{"DestinationType":"ImageUploader","RequestURL":"https://example.invalid/upload","URL":"{regex:https://example\\.invalid/\\S+}"}"#,
        )?;
        let (destination, _) = UploadDestination::from_sharex_at(&path)?;
        assert_eq!(
            destination.regex_pattern(),
            Some(r#"https://example\.invalid/\S+"#)
        );
        assert!(destination.regex_group().is_none());
        std::fs::write(
            &path,
            r#"{"DestinationType":"ImageUploader","RequestURL":"https://example.invalid/upload","URL":"{xml:/files/file[1]/url}"}"#,
        )?;
        let (destination, _) = UploadDestination::from_sharex_at(&path)?;
        assert_eq!(destination.xml_path(), Some("/files/file[1]/url"));
        for profile in [
            r#"{"DestinationType":"FileUploader","RequestURL":"https://example.invalid/upload"}"#,
            r#"{"DestinationType":"ImageUploader","RequestURL":"http://example.invalid/upload"}"#,
            r#"{"DestinationType":"ImageUploader","RequestURL":"https://user:secret@example.invalid/upload"}"#,
            r#"{"DestinationType":"ImageUploader","RequestMethod":"GET","RequestURL":"https://example.invalid/upload"}"#,
            r#"{"DestinationType":"ImageUploader","RequestURL":"https://example.invalid/upload","Headers":{"Authorization":"Bearer secret"}}"#,
            r#"{"DestinationType":"ImageUploader","RequestURL":"https://example.invalid/upload","Headers":{"X-Client":"{json:token}"}}"#,
            r#"{"DestinationType":"ImageUploader","RequestURL":"https://example.invalid/upload","Parameters":{"api_key":"secret"}}"#,
            r#"{"DestinationType":"ImageUploader","RequestURL":"https://example.invalid/upload","Parameters":{"format":"{json:token}"}}"#,
            r#"{"DestinationType":"ImageUploader","RequestURL":"https://example.invalid/upload","Arguments":{"api_key":"secret"}}"#,
            r#"{"DestinationType":"ImageUploader","RequestURL":"https://example.invalid/upload","Arguments":{"title":"{json:token}"}}"#,
            r#"{"DestinationType":"ImageUploader","RequestMethod":"PUT","RequestURL":"https://example.invalid/image.png","Body":"Binary","Arguments":{"title":"screenshot"}}"#,
            r#"{"DestinationType":"ImageUploader","RequestURL":"https://example.invalid/upload","Body":"JSON"}"#,
            r#"{"DestinationType":"ImageUploader","RequestURL":"https://example.invalid/upload","Body":"JSON","Data":{"image":"{base64}"},"Arguments":{"title":"screenshot"}}"#,
            r#"{"DestinationType":"ImageUploader","RequestURL":"https://example.invalid/upload","Body":"JSON","Data":{"image":"{input}"}}"#,
            r#"{"DestinationType":"ImageUploader","RequestURL":"https://example.invalid/upload","Body":"JSON","Data":{"api_key":"{base64}"}}"#,
            r#"{"DestinationType":"ImageUploader","RequestURL":"https://example.invalid/upload","Body":"JSON","Data":{"image":"{base64}","thumb":"{base64}"}}"#,
            r#"{"DestinationType":"ImageUploader","RequestMethod":"PUT","RequestURL":"https://example.invalid/image.png","Body":"JSON","Data":{"image":"{base64}"}}"#,
            r#"{"DestinationType":"ImageUploader","RequestURL":"https://example.invalid/upload","URL":"{regex}"}"#,
            r#"{"DestinationType":"ImageUploader","RequestURL":"https://example.invalid/upload","URL":"{xml}"}"#,
            r#"{"DestinationType":"ImageUploader","RequestURL":"https://example.invalid/upload","URL":"{xml:input|/url}"}"#,
            r#"{"DestinationType":"ImageUploader","RequestURL":"https://example.invalid/upload","URL":"{xml://files/url}"}"#,
            r#"{"DestinationType":"ImageUploader","RequestURL":"https://example.invalid/upload","URL":"{xml:files/url}"}"#,
            r#"{"DestinationType":"ImageUploader","RequestURL":"https://example.invalid/upload","URL":"{regex:(?<=href=\").+(?=\")}"}"#,
            r#"{"DestinationType":"ImageUploader","RequestURL":"https://example.invalid/upload","URL":"{regex:a|b|c}"}"#,
            r#"{"DestinationType":"ImageUploader","RequestURL":"https://example.invalid/{json:url}"}"#,
            r#"{"DestinationType":"ImageUploader","RequestURL":"https://example.invalid/upload","URL":"https://cdn.example.invalid/{json:id}/{json:name}"}"#,
            r#"{"DestinationType":"ImageUploader","RequestURL":"https://example.invalid/upload","URL":"https://cdn.example.invalid/{json:id|regex:.+}"}"#,
            r#"{"DestinationType":"ImageUploader","RequestMethod":"PUT","RequestURL":"https://example.invalid/image.png","Body":"Binary","URL":"https://cdn.example.invalid/{responseurl}"}"#,
            r#"{"DestinationType":"ImageUploader","RequestURL":"https://example.invalid/upload","URL":"{json:url}","ThumbnailURL":"{regex}"}"#,
            r#"{"DestinationType":"ImageUploader","RequestURL":"https://example.invalid/upload","URL":"{json:url}","DeletionURL":"{responseurl}"}"#,
            r#"{"DestinationType":"ImageUploader","RequestMethod":"PUT","RequestURL":"https://example.invalid/image.png","Body":"Binary","URL":"{responseurl}","DeletionURL":"{json:delete}"}"#,
        ] {
            std::fs::write(&path, profile)?;
            assert!(
                UploadDestination::from_sharex_at(&path).is_err(),
                "{profile}"
            );
        }
        assert_eq!(std::fs::read(&other)?, b"keep-settings");
        std::fs::remove_dir_all(root)?;
        Ok(())
    }

    #[test]
    fn sharex_export_round_trips_supported_subset_without_secrets() -> Result<()> {
        let root = std::env::temp_dir().join(format!(
            "screenshots-sharex-export-{}",
            uuid::Uuid::new_v4()
        ));
        std::fs::create_dir_all(&root)?;
        let path = root.join("host.sxcu");
        let other = root.join("settings.json");
        std::fs::write(&other, b"keep-settings")?;
        let destination = UploadDestination::manual(
            "https://example.invalid/upload".into(),
            UploadMethod::Post,
            "file".into(),
            Some("/files/0/url".into()),
            false,
        )?;
        destination.to_sharex_at(&path, Some("Host"))?;
        let exported = std::fs::read_to_string(&path)?;
        assert!(exported.contains("ImageUploader"));
        assert!(exported.contains("{json:files[0].url}"));
        assert!(exported.contains("Host"));
        assert!(!exported.contains("secret"));
        assert!(!exported.contains("credential"));
        assert!(!exported.contains("authentication"));
        let (imported, name) = UploadDestination::from_sharex_at(&path)?;
        assert_eq!(name.as_deref(), Some("Host"));
        assert_eq!(imported.origin(), destination.origin());
        assert_eq!(imported.method, destination.method);
        assert_eq!(imported.field, destination.field);
        assert_eq!(imported.json_pointer, "/files/0/url");
        assert!(imported.response.is_none());
        assert!(imported.credential.is_none());
        std::fs::remove_file(&path)?;
        let headed = destination
            .clone()
            .with_headers([("X-Client", "screenshots")])?;
        headed.to_sharex_at(&path, Some("Host"))?;
        let exported = std::fs::read_to_string(&path)?;
        assert!(exported.contains("X-Client"));
        assert!(exported.contains("screenshots"));
        assert!(!exported.contains("Authorization"));
        let (imported, _) = UploadDestination::from_sharex_at(&path)?;
        assert_eq!(
            imported
                .request_headers()
                .get("X-Client")
                .map(String::as_str),
            Some("screenshots")
        );
        std::fs::remove_file(&path)?;
        let queried = destination.clone().with_query([("format", "json")])?;
        queried.to_sharex_at(&path, Some("Host"))?;
        let exported = std::fs::read_to_string(&path)?;
        assert!(exported.contains("format"));
        assert!(exported.contains("json"));
        assert!(!exported.contains("api_key"));
        assert!(!exported.contains("?format="));
        let (imported, _) = UploadDestination::from_sharex_at(&path)?;
        assert_eq!(
            imported.request_query().get("format").map(String::as_str),
            Some("json")
        );
        std::fs::remove_file(&path)?;
        let formed = destination.clone().with_form([("title", "screenshot")])?;
        formed.to_sharex_at(&path, Some("Host"))?;
        let exported = std::fs::read_to_string(&path)?;
        assert!(exported.contains("title"));
        assert!(exported.contains("screenshot"));
        assert!(!exported.contains("api_key"));
        let (imported, _) = UploadDestination::from_sharex_at(&path)?;
        assert_eq!(
            imported.request_form().get("title").map(String::as_str),
            Some("screenshot")
        );
        std::fs::remove_file(&path)?;
        let json_body = destination
            .clone()
            .with_json([("image", "{base64}"), ("type", "file")])?;
        json_body.to_sharex_at(&path, Some("Host"))?;
        let exported = std::fs::read_to_string(&path)?;
        assert!(exported.contains("\"JSON\""));
        assert!(exported.contains("{base64}"));
        assert!(exported.contains("\"type\""));
        assert!(!exported.contains("api_key"));
        assert!(!exported.contains("FileFormName"));
        assert!(!exported.contains("Arguments"));
        let (imported, _) = UploadDestination::from_sharex_at(&path)?;
        assert_eq!(
            imported.request_json().get("image").map(String::as_str),
            Some("{base64}")
        );
        assert_eq!(
            imported.request_json().get("type").map(String::as_str),
            Some("file")
        );
        assert!(imported.request_form().is_empty());
        std::fs::remove_file(&path)?;
        let composed = UploadDestination::manual(
            "https://example.invalid/upload".into(),
            UploadMethod::Post,
            "file".into(),
            Some("/id".into()),
            false,
        )?
        .with_link_affixes("https://cdn.example.invalid/", ".png")?;
        composed.to_sharex_at(&path, Some("Host"))?;
        let exported = std::fs::read_to_string(&path)?;
        assert!(exported.contains("https://cdn.example.invalid/{json:id}.png"));
        assert!(!exported.contains("api_key"));
        let (imported, _) = UploadDestination::from_sharex_at(&path)?;
        assert_eq!(imported.json_pointer, "/id");
        assert_eq!(imported.link_prefix(), "https://cdn.example.invalid/");
        assert_eq!(imported.link_suffix(), ".png");
        std::fs::remove_file(&path)?;
        let extras = destination
            .clone()
            .with_thumbnail("{json:thumbnail_url}")?
            .with_deletion("{json:delete}")?;
        extras.to_sharex_at(&path, Some("Host"))?;
        let exported = std::fs::read_to_string(&path)?;
        assert!(exported.contains("\"ThumbnailURL\": \"{json:thumbnail_url}\""));
        assert!(exported.contains("\"DeletionURL\": \"{json:delete}\""));
        assert!(!exported.contains("api_key"));
        let (imported, _) = UploadDestination::from_sharex_at(&path)?;
        assert_eq!(
            imported.thumbnail_template().as_deref(),
            Some("{json:thumbnail_url}")
        );
        assert_eq!(
            imported.deletion_template().as_deref(),
            Some("{json:delete}")
        );
        std::fs::remove_file(&path)?;
        let regexed = UploadDestination::manual(
            "https://example.invalid/upload".into(),
            UploadMethod::Post,
            "file".into(),
            Some("/url".into()),
            false,
        )?
        .with_regex(r#"href="(.+)""#, Some("1"))?;
        regexed.to_sharex_at(&path, Some("Host"))?;
        let exported = std::fs::read_to_string(&path)?;
        assert!(exported.contains(r#"{regex:href=\"(.+)\"|1}"#));
        assert!(!exported.contains("api_key"));
        let (imported, _) = UploadDestination::from_sharex_at(&path)?;
        assert_eq!(imported.regex_pattern(), Some(r#"href="(.+)""#));
        assert_eq!(imported.regex_group(), Some("1"));
        std::fs::remove_file(&path)?;
        let xmled = UploadDestination::manual(
            "https://example.invalid/upload".into(),
            UploadMethod::Post,
            "file".into(),
            Some("/url".into()),
            false,
        )?
        .with_xml("/files/file[1]/url")?;
        xmled.to_sharex_at(&path, Some("Host"))?;
        let exported = std::fs::read_to_string(&path)?;
        assert!(exported.contains("{xml:/files/file[1]/url}"));
        assert!(!exported.contains("api_key"));
        let (imported, _) = UploadDestination::from_sharex_at(&path)?;
        assert_eq!(imported.xml_path(), Some("/files/file[1]/url"));
        std::fs::remove_file(&path)?;
        let mut header = UploadDestination::manual(
            "https://example.invalid/upload".into(),
            UploadMethod::Post,
            "image".into(),
            None,
            false,
        )?;
        header.response = Some(UploadResponse::Header {
            name: "Location".into(),
        });
        header.json_pointer.clear();
        header.to_sharex_at(&path, None)?;
        let exported = std::fs::read_to_string(&path)?;
        assert!(exported.contains("{header:Location}"));
        assert!(!exported.contains("\"Name\""));
        let (imported, name) = UploadDestination::from_sharex_at(&path)?;
        assert!(name.is_none());
        assert_eq!(
            imported.response,
            Some(UploadResponse::Header {
                name: "Location".into()
            })
        );
        std::fs::remove_file(&path)?;
        let mut put = UploadDestination::manual(
            "https://example.invalid/image.png".into(),
            UploadMethod::Put,
            "file".into(),
            None,
            false,
        )?;
        put.response = Some(UploadResponse::RequestUrl);
        put.json_pointer.clear();
        put.to_sharex(&path)?;
        let exported = std::fs::read_to_string(&path)?;
        assert!(exported.contains("{responseurl}"));
        assert!(exported.contains("\"PUT\""));
        assert!(!exported.contains("FileFormName"));
        let (imported, _) = UploadDestination::from_sharex_at(&path)?;
        assert_eq!(imported.response, Some(UploadResponse::RequestUrl));
        std::fs::remove_file(&path)?;
        let text = UploadDestination::manual(
            "https://example.invalid/upload".into(),
            UploadMethod::Post,
            "file".into(),
            None,
            false,
        )?;
        text.to_sharex(&path)?;
        let exported = std::fs::read_to_string(&path)?;
        assert!(exported.contains("{response}"));
        let (imported, _) = UploadDestination::from_sharex_at(&path)?;
        assert_eq!(imported.response, Some(UploadResponse::Text));
        let existing = std::fs::read(&path)?;
        assert!(text.to_sharex(&path).is_err());
        assert_eq!(std::fs::read(&path)?, existing);
        #[cfg(target_os = "windows")]
        {
            let authenticated = destination
                .clone()
                .with_authentication("main".into(), UploadAuthentication::Bearer)?;
            let secret_path = root.join("auth.sxcu");
            authenticated.to_sharex(&secret_path)?;
            let exported = std::fs::read_to_string(&secret_path)?;
            assert!(!exported.contains("main"));
            assert!(!exported.contains("Bearer"));
            assert!(!exported.contains("secret"));
            let (imported, _) = UploadDestination::from_sharex_at(&secret_path)?;
            assert!(imported.credential.is_none() && imported.authentication.is_none());
        }
        let http = UploadDestination::manual(
            "http://127.0.0.1/upload".into(),
            UploadMethod::Post,
            "file".into(),
            None,
            true,
        )?;
        assert!(http.to_sharex(&root.join("http.sxcu")).is_err());
        assert!(!root.join("http.sxcu").exists());
        let mut escaped = destination.clone();
        escaped.json_pointer = "/foo~1bar".into();
        assert!(escaped.to_sharex(&root.join("escaped.sxcu")).is_err());
        assert!(!root.join("escaped.sxcu").exists());
        assert_eq!(std::fs::read(&other)?, b"keep-settings");
        std::fs::remove_dir_all(root)?;
        Ok(())
    }

    #[test]
    fn response_modes_validate_configuration_and_share_safe_link_rules() -> Result<()> {
        let (root, _) = fixture();
        let path = root.join("destination.json");
        for response in [
            r#"{"type":"text"}"#,
            r#"{"type":"header","name":"Location"}"#,
            r#"{"type":"regex","pattern":"https://\\S+"}"#,
        ] {
            std::fs::write(
                &path,
                format!(
                    r#"{{"url":"https://example.invalid/upload","field":"file","response":{response}}}"#
                ),
            )?;
            let profile = UploadDestination::load(&path)?;
            profile.save_at(&path)?;
            assert!(UploadDestination::load(&path)?.response.is_some());
        }
        let mut profile = destination("https://example.invalid/upload".into());
        profile.response = Some(UploadResponse::Text);
        assert!(profile.validate().is_err());
        profile.json_pointer.clear();
        assert!(profile.validate().is_ok());
        for name in ["", "bad header", "Location\r\nInjected", &"x".repeat(65)] {
            profile.response = Some(UploadResponse::Header { name: name.into() });
            assert!(profile.validate().is_err());
        }
        for value in [
            "javascript:alert(1)",
            "file:///private",
            "/relative",
            "https://user:pass@example.invalid/",
            "https://example.invalid/a\nb",
            &format!("https://example.invalid/{}", "x".repeat(2048)),
        ] {
            assert!(validate_link(value).is_err());
        }
        assert_eq!(
            validate_link("https://example.invalid/image")?,
            "https://example.invalid/image"
        );
        std::fs::remove_dir_all(root)?;
        Ok(())
    }

    #[test]
    #[ignore = "local loopback response-format tests with synthetic PNG; no external connections"]
    fn upload_reads_plain_text_and_header_links_without_following_them() {
        let (root, path) = fixture();
        for (mode, response_bytes, expected) in [
            (UploadResponse::Text, response(" \r\nhttps://example.invalid/plain\n"), Some("https://example.invalid/plain")),
            (UploadResponse::Text, response("javascript:alert(1)"), None),
            (UploadResponse::Header{name:"location".into()}, b"HTTP/1.1 201 Created\r\nLocation: https://example.invalid/header\r\nContent-Length: 0\r\nConnection: close\r\n\r\n".to_vec(), Some("https://example.invalid/header")),
            (UploadResponse::Header{name:"Location".into()}, response("{}"), None),
            (UploadResponse::Header{name:"Location".into()}, b"HTTP/1.1 200 OK\r\nLocation: file:///private\r\nContent-Length: 0\r\nConnection: close\r\n\r\n".to_vec(), None),
            (UploadResponse::Header{name:"Location".into()}, b"HTTP/1.1 200 OK\r\nLocation: https://example.invalid/one\r\nLocation: https://example.invalid/two\r\nContent-Length: 0\r\nConnection: close\r\n\r\n".to_vec(), None),
        ] {
            let (url, task) = server(response_bytes,None,Duration::ZERO);
            let mut config = destination(url); config.json_pointer.clear(); config.response = Some(mode);
            let started = std::time::Instant::now();
            let result = upload_png(&config,&path,UploadCancellation::default());
            if expected.is_some() { eprintln!("Synthetic PNG text/header response including runtime teardown: {:?}", started.elapsed()); }
            match expected { Some(link) => assert_eq!(result.unwrap(), link), None => assert!(result.is_err()) }
            task.join().unwrap();
        }
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn upload_method_defaults_and_remembered_put_profiles_are_validated() -> Result<()> {
        let (root, _) = fixture();
        let path = root.join("destination.json");
        std::fs::write(&path, br#"{"url":"https://example.invalid/image.png","method":"PUT","response":{"type":"header","name":"Location"}}"#)?;
        let profile = UploadDestination::load(&path)?;
        assert!(matches!(profile.method, UploadMethod::Put));
        profile.save_at(&path)?;
        assert!(matches!(
            UploadDestination::load(&path)?.method,
            UploadMethod::Put
        ));
        std::fs::write(
            &path,
            br#"{"url":"https://example.invalid/upload","json_pointer":"/url"}"#,
        )?;
        let legacy = UploadDestination::load(&path)?;
        assert!(matches!(legacy.method, UploadMethod::Post));
        assert_eq!(legacy.field, "file");
        for method in ["GET", "DELETE", "PATCH", "put"] {
            std::fs::write(
                &path,
                format!(r#"{{"url":"https://example.invalid/upload","method":"{method}"}}"#),
            )?;
            assert!(UploadDestination::load(&path).is_err());
        }
        std::fs::remove_dir_all(root)?;
        Ok(())
    }

    #[test]
    #[ignore = "local loopback PUT tests with synthetic PNG; no external connections"]
    fn upload_put_streams_exact_png_with_creation_precondition_and_preserves_errors() {
        let (root, path) = fixture();
        let original = std::fs::read(&path).unwrap();
        let (url, task) = server(
            response(r#"{"result":{"url":"https://example.invalid/put.png"}}"#),
            None,
            Duration::ZERO,
        );
        let mut config = destination(url);
        config.method = UploadMethod::Put;
        let started = std::time::Instant::now();
        assert_eq!(
            upload_png(&config, &path, UploadCancellation::default()).unwrap(),
            "https://example.invalid/put.png"
        );
        eprintln!(
            "Synthetic PNG PUT upload including runtime teardown: {:?}",
            started.elapsed()
        );
        let request = task.join().unwrap();
        let boundary = request
            .windows(4)
            .position(|part| part == b"\r\n\r\n")
            .unwrap()
            + 4;
        let headers = String::from_utf8_lossy(&request[..boundary]).to_ascii_lowercase();
        assert!(headers.starts_with("put /upload http/1.1"));
        assert!(headers.contains("content-type: image/png\r\n"));
        assert!(headers.contains("if-none-match: *\r\n"));
        assert!(!headers.contains("multipart"));
        assert_eq!(&request[boundary..], original.as_slice());
        let (url, task) = server(
            b"HTTP/1.1 412 Precondition Failed\r\nContent-Length: 0\r\nConnection: close\r\n\r\n"
                .to_vec(),
            None,
            Duration::ZERO,
        );
        config.url = url;
        assert!(
            upload_png(&config, &path, UploadCancellation::default())
                .unwrap_err()
                .to_string()
                .contains("412")
        );
        task.join().unwrap();
        assert_eq!(std::fs::read(&path).unwrap(), original);
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn credential_references_are_bound_to_https_origin_and_never_contain_tokens() -> Result<()> {
        let mut config = destination("http://example.invalid/upload".into());
        config.credential = Some("main".into());
        assert!(config.validate().is_err());
        config.url = "https://example.invalid/upload".into();
        if cfg!(target_os = "windows") {
            config.validate()?;
            assert_eq!(
                config.credential_target().as_deref(),
                Some("Screenshots/upload/https://example.invalid/main")
            );
            let original = config.credential_target();
            config.url = "https://other.invalid/upload".into();
            assert_ne!(config.credential_target(), original);
            config.url = "https://example.invalid:444/upload".into();
            assert_ne!(config.credential_target(), original);
        } else {
            assert!(config.validate().is_err());
        }
        for name in ["", "../../other", "with spaces", "name\0", &"x".repeat(65)] {
            config.credential = Some(name.into());
            assert!(config.validate().is_err());
        }
        Ok(())
    }

    #[test]
    #[cfg(target_os = "windows")]
    #[ignore = "checks a unique nonexistent credential and verifies no connection to a loopback listener"]
    fn missing_bearer_credential_prevents_any_request() {
        let (root, path) = fixture();
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        listener.set_nonblocking(true).unwrap();
        let mut config = destination(format!("https://{}/upload", listener.local_addr().unwrap()));
        config.credential = Some(format!("missing-{}", uuid::Uuid::new_v4()));
        for authentication in [
            None,
            Some(UploadAuthentication::ApiKey),
            Some(UploadAuthentication::Basic),
        ] {
            config.authentication = authentication;
            let error = upload_png(&config, &path, UploadCancellation::default()).unwrap_err();
            assert!(
                error
                    .to_string()
                    .contains("No readable Windows upload credential")
            );
            assert_eq!(
                listener.accept().unwrap_err().kind(),
                std::io::ErrorKind::WouldBlock
            );
        }
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn api_key_profiles_require_credentials_and_round_trip_without_secrets() -> Result<()> {
        let mut config = destination("https://example.invalid/upload".into());
        config.authentication = Some(UploadAuthentication::ApiKey);
        assert!(config.validate().is_err());
        config.credential = Some("main".into());
        if cfg!(target_os = "windows") {
            config.validate()?;
            let target = config.credential_target();
            let (root, _) = fixture();
            let path = root.join("destination.json");
            config.save_at(&path)?;
            let loaded = UploadDestination::load(&path)?;
            assert_eq!(loaded.authentication.unwrap().header_name(), "x-api-key");
            assert_eq!(loaded.credential_target(), target);
            config.authentication = None;
            assert_eq!(
                config.authentication.unwrap_or_default().header_name(),
                "authorization"
            );
            assert_eq!(config.credential_target(), target);
            std::fs::remove_dir_all(root)?;
        }
        config.url = "http://example.invalid/upload".into();
        config.allow_http = true;
        assert!(config.validate().is_err());
        Ok(())
    }

    #[test]
    fn basic_profiles_require_an_os_reference_and_persist_only_the_mode() -> Result<()> {
        let mut config = destination("https://example.invalid/upload".into());
        config.authentication = Some(UploadAuthentication::Basic);
        assert!(config.validate().is_err());
        config.credential = Some("account".into());
        if cfg!(target_os = "windows") {
            let (root, _) = fixture();
            let path = root.join("destination.json");
            config.save_at(&path)?;
            let loaded = UploadDestination::load(&path)?;
            assert!(matches!(
                loaded.authentication,
                Some(UploadAuthentication::Basic)
            ));
            assert_eq!(loaded.credential_target(), config.credential_target());
            std::fs::remove_dir_all(root)?;
        }
        config.url = "http://example.invalid/upload".into();
        config.allow_http = true;
        assert!(config.validate().is_err());
        Ok(())
    }

    #[test]
    fn native_credential_attachment_validates_without_storing_secrets() -> Result<()> {
        let destination = UploadDestination::manual(
            "https://example.invalid/upload".into(),
            UploadMethod::Post,
            "file".into(),
            None,
            false,
        )?;
        let attached = destination
            .clone()
            .with_authentication("main".into(), UploadAuthentication::ApiKey);
        if cfg!(target_os = "windows") {
            let attached = attached?;
            assert_eq!(attached.credential_reference(), Some("main"));
            assert_eq!(
                attached.authentication(),
                Some(UploadAuthentication::ApiKey)
            );
            assert_eq!(
                attached.credential_target().as_deref(),
                Some("Screenshots/upload/https://example.invalid/main")
            );
            let root = std::env::temp_dir().join(format!("native-cred-{}", uuid::Uuid::new_v4()));
            std::fs::create_dir_all(&root)?;
            let path = root.join("destination.json");
            attached.save_at(&path)?;
            let text = String::from_utf8(std::fs::read(&path)?)?;
            assert!(text.contains("main"));
            assert!(text.contains("api_key"));
            assert!(!text.contains("secret"));
            std::fs::remove_dir_all(root)?;
        } else {
            assert!(attached.is_err());
        }
        assert!(
            UploadDestination::manual(
                "http://example.invalid/upload".into(),
                UploadMethod::Post,
                "file".into(),
                None,
                true,
            )?
            .with_authentication("main".into(), UploadAuthentication::Bearer)
            .is_err()
        );
        for secret in ["", "two words", "bad\r\ntoken"] {
            let Err(error) = destination.clone().attach_credential(
                "main".into(),
                UploadAuthentication::Bearer,
                String::new(),
                secret.into(),
            ) else {
                panic!("invalid secret must be rejected");
            };
            let error = error.to_string();
            if !secret.is_empty() {
                assert!(!error.contains(secret), "{error}");
            }
        }
        Ok(())
    }

    #[test]
    #[cfg(target_os = "windows")]
    #[ignore = "creates and deletes one uniquely named synthetic Windows credential; no real secrets or networking"]
    fn attach_credential_writes_an_os_secret_without_embedding_it() -> Result<()> {
        use windows_sys::Win32::Security::Credentials::*;
        let name = format!("test-{}", uuid::Uuid::new_v4());
        let destination = UploadDestination::manual(
            "https://example.invalid/upload".into(),
            UploadMethod::Post,
            "file".into(),
            None,
            false,
        )?
        .attach_credential(
            name,
            UploadAuthentication::Bearer,
            String::new(),
            "synthetic-token_123.abc==".into(),
        )?;
        let target = destination.credential_target().unwrap();
        struct Cleanup(Vec<u16>);
        impl Drop for Cleanup {
            fn drop(&mut self) {
                unsafe {
                    CredDeleteW(self.0.as_ptr(), CRED_TYPE_GENERIC, 0);
                }
            }
        }
        let _cleanup = Cleanup(target.encode_utf16().chain(Some(0)).collect());
        let text = String::from_utf8(serde_json::to_vec(&destination)?)?;
        assert!(!text.contains("synthetic-token"));
        assert_eq!(
            crate::upload_credentials::read_header(&target, UploadAuthentication::Bearer)?
                .to_str()
                .unwrap(),
            "Bearer synthetic-token_123.abc=="
        );
        Ok(())
    }

    #[test]
    fn detach_credential_requires_an_attached_https_reference_without_os_writes() -> Result<()> {
        let destination = UploadDestination::manual(
            "https://example.invalid/upload".into(),
            UploadMethod::Post,
            "file".into(),
            None,
            false,
        )?;
        assert!(destination.clone().detach_credential().is_err());
        if cfg!(target_os = "windows") {
            let attached =
                destination.with_authentication("main".into(), UploadAuthentication::Bearer)?;
            assert_eq!(attached.credential_reference(), Some("main"));
            let text = String::from_utf8(serde_json::to_vec(&attached)?)?;
            assert!(text.contains("main"));
            assert!(!text.contains("secret"));
        }
        Ok(())
    }

    #[test]
    #[cfg(target_os = "windows")]
    #[ignore = "creates and deletes one uniquely named synthetic Windows credential; no real secrets or networking"]
    fn detach_credential_removes_the_os_secret_and_clears_the_reference() -> Result<()> {
        let name = format!("test-{}", uuid::Uuid::new_v4());
        let destination = UploadDestination::manual(
            "https://example.invalid/upload".into(),
            UploadMethod::Post,
            "file".into(),
            None,
            false,
        )?
        .attach_credential(
            name,
            UploadAuthentication::Bearer,
            String::new(),
            "synthetic-token_123.abc==".into(),
        )?;
        let target = destination.credential_target().unwrap();
        struct Cleanup(Vec<u16>);
        impl Drop for Cleanup {
            fn drop(&mut self) {
                unsafe {
                    windows_sys::Win32::Security::Credentials::CredDeleteW(
                        self.0.as_ptr(),
                        windows_sys::Win32::Security::Credentials::CRED_TYPE_GENERIC,
                        0,
                    );
                }
            }
        }
        let _cleanup = Cleanup(target.encode_utf16().chain(Some(0)).collect());
        let detached = destination.detach_credential()?;
        assert!(detached.credential_reference().is_none());
        assert!(detached.authentication().is_none());
        let text = String::from_utf8(serde_json::to_vec(&detached)?)?;
        assert!(!text.contains("synthetic-token"));
        assert!(!text.contains("credential"));
        assert!(
            crate::upload_credentials::read_header(&target, UploadAuthentication::Bearer).is_err()
        );
        Ok(())
    }

    #[test]
    fn request_url_profiles_require_put_and_preserve_explicit_mode() -> Result<()> {
        let mut config = destination("https://example.invalid/images/a.png".into());
        config.json_pointer.clear();
        config.response = Some(UploadResponse::RequestUrl);
        assert!(config.validate().is_err());
        config.method = UploadMethod::Put;
        config.validate()?;
        let (root, _) = fixture();
        let path = root.join("destination.json");
        config.save_at(&path)?;
        assert!(matches!(
            UploadDestination::load(&path)?.response,
            Some(UploadResponse::RequestUrl)
        ));
        config.json_pointer = "/url".into();
        assert!(config.validate().is_err());
        config.json_pointer.clear();
        config.url = format!("https://example.invalid/{}", "é".repeat(600));
        assert!(config.validate().is_err());
        std::fs::remove_dir_all(root)?;
        Ok(())
    }

    #[test]
    #[ignore = "PUT status-only responses from local loopback servers; synthetic PNG only"]
    fn request_url_mode_uses_only_confirmed_put_resource_urls() {
        let (root, path) = fixture();
        let original = std::fs::read(&path).unwrap();
        for (status, success) in [
            (200, true),
            (201, true),
            (204, true),
            (202, false),
            (206, false),
            (307, false),
            (412, false),
        ] {
            let reply=format!("HTTP/1.1 {status} Test\r\nLocation: https://example.invalid/not-the-resource\r\nContent-Length: 0\r\nConnection: close\r\n\r\n").into_bytes();
            let (url, task) = server(reply, None, Duration::ZERO);
            let mut config = destination(url.clone());
            config.method = UploadMethod::Put;
            config.json_pointer.clear();
            config.response = Some(UploadResponse::RequestUrl);
            let started = std::time::Instant::now();
            let result = upload_png(&config, &path, UploadCancellation::default());
            if success {
                assert_eq!(result.unwrap(), url);
                if status == 204 {
                    eprintln!(
                        "Synthetic PNG status-only PUT including teardown: {:?}",
                        started.elapsed()
                    );
                }
            } else {
                assert!(result.is_err());
            }
            let request = task.join().unwrap();
            assert!(request.starts_with(b"PUT /upload HTTP/1.1"));
        }
        assert_eq!(std::fs::read(&path).unwrap(), original);
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn destination_validation_rejects_implicit_http_secrets_and_unbounded_settings() {
        let mut config = destination("https://example.invalid/upload".into());
        assert!(config.validate().is_ok());
        assert_eq!(config.origin(), "https://example.invalid");
        for url in [
            "file:///tmp/test",
            "https://user:pass@example.invalid/",
            "https://example.invalid/?token=secret",
            "https://example.invalid/#fragment",
        ] {
            config.url = url.into();
            assert!(config.validate().is_err());
        }
        config.url = "http://127.0.0.1/upload".into();
        config.allow_http = false;
        assert!(config.validate().is_err());
        config.allow_http = true;
        config.field = "x\r\nInjected".into();
        assert!(config.validate().is_err());
        config.field = "file".into();
        config.json_pointer = "/bad~2escape".into();
        assert!(config.validate().is_err());
        let (root, _) = fixture();
        let path = root.join("destination.json");
        std::fs::write(
            &path,
            br#"{"url":"https://example.invalid/upload","field":"file","json_pointer":"/url"}"#,
        )
        .unwrap();
        assert!(UploadDestination::load(&path).is_ok());
        std::fs::write(&path, br#"{"url":"https://example.invalid/upload","field":"file","json_pointer":"/url","authorization":"secret"}"#).unwrap();
        assert!(UploadDestination::load(&path).is_err());
        std::fs::write(&path, vec![b' '; 16385]).unwrap();
        assert!(UploadDestination::load(&path).is_err());
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    #[ignore = "uses a local loopback HTTP server and a throwaway 1×1 PNG; no external connections"]
    fn probe_upload_sends_a_throwaway_png_and_parses_the_returned_link() {
        let before = probe_temp_names();
        let (url, server) = server(
            response(r#"{"result":{"url":"https://example.invalid/share/probe"}}"#),
            None,
            Duration::ZERO,
        );
        let started = std::time::Instant::now();
        assert_eq!(
            probe_upload(&destination(url), UploadCancellation::default(), None)
                .unwrap()
                .url,
            "https://example.invalid/share/probe"
        );
        eprintln!(
            "Synthetic 1×1 probe including runtime/client setup and teardown: {:?}",
            started.elapsed()
        );
        let request = server.join().unwrap();
        assert!(
            request
                .windows(8)
                .any(|chunk| chunk == b"\x89PNG\r\n\x1a\n")
        );
        assert!(request.len() < 8192);
        assert_eq!(before, probe_temp_names());
    }

    #[test]
    #[ignore = "uses a local loopback HTTP server and synthetic PNG; no external connections"]
    fn upload_sends_query_parameters_on_the_wire() {
        let (root, path) = fixture();
        let (url, server) = server(
            response(r#"{"result":{"url":"https://example.invalid/share/query"}}"#),
            None,
            Duration::ZERO,
        );
        let destination = destination(url).with_query([("format", "json")]).unwrap();
        assert_eq!(
            upload_png(&destination, &path, UploadCancellation::default()).unwrap(),
            "https://example.invalid/share/query"
        );
        let request = String::from_utf8(server.join().unwrap()).unwrap();
        let line = request.lines().next().unwrap();
        assert!(line.starts_with("POST /upload?format=json HTTP/1.1"));
        assert!(!request.to_ascii_lowercase().contains("api_key"));
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    #[ignore = "uses a local loopback HTTP server and synthetic PNG; no external connections"]
    fn upload_sends_extra_form_fields_on_the_wire() {
        let (root, path) = fixture();
        let (url, server) = server(
            response(r#"{"result":{"url":"https://example.invalid/share/form"}}"#),
            None,
            Duration::ZERO,
        );
        let destination = destination(url)
            .with_form([("title", "screenshot")])
            .unwrap();
        assert_eq!(
            upload_png(&destination, &path, UploadCancellation::default()).unwrap(),
            "https://example.invalid/share/form"
        );
        let request = String::from_utf8(server.join().unwrap()).unwrap();
        assert!(request.contains("name=\"title\""));
        assert!(request.contains("screenshot"));
        assert!(request.contains("name=\"file\""));
        assert!(!request.to_ascii_lowercase().contains("api_key"));
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    #[ignore = "uses a local loopback HTTP server and synthetic PNG; no external connections"]
    fn upload_sends_json_body_on_the_wire() {
        use base64::Engine;
        let (root, path) = fixture();
        let png = std::fs::read(&path).unwrap();
        let encoded = base64::engine::general_purpose::STANDARD.encode(&png);
        let (url, server) = server(
            response(r#"{"result":{"url":"https://example.invalid/share/json"}}"#),
            None,
            Duration::ZERO,
        );
        let destination = destination(url)
            .with_json([("image", "{base64}"), ("type", "file")])
            .unwrap();
        assert_eq!(
            upload_png(&destination, &path, UploadCancellation::default()).unwrap(),
            "https://example.invalid/share/json"
        );
        let request = String::from_utf8(server.join().unwrap()).unwrap();
        let lower = request.to_ascii_lowercase();
        assert!(lower.contains("content-type: application/json"));
        assert!(request.contains(&format!(r#"{{"image":"{encoded}","type":"file"}}"#)));
        assert!(!request.contains("multipart"));
        assert!(!request.contains("name=\"file\""));
        assert!(!lower.contains("api_key"));
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    #[ignore = "uses a local loopback HTTP server and synthetic PNG; no external connections"]
    fn upload_composes_result_link_from_json_id() {
        let (root, path) = fixture();
        let (url, server) = server(response(r#"{"result":{"id":123}}"#), None, Duration::ZERO);
        let mut destination = destination(url)
            .with_link_affixes("https://cdn.example.invalid/", ".png")
            .unwrap();
        destination.json_pointer = "/result/id".into();
        assert_eq!(
            upload_png(&destination, &path, UploadCancellation::default()).unwrap(),
            "https://cdn.example.invalid/123.png"
        );
        let _ = server.join().unwrap();
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    #[ignore = "uses a local loopback HTTP server and synthetic PNG; no external connections"]
    fn upload_extracts_thumbnail_and_deletion_urls() {
        let (root, path) = fixture();
        let (url, server) = server(
            response(
                r#"{"result":{"url":"https://example.invalid/share/abc","thumbnail_url":"https://example.invalid/t.png","delete":"https://example.invalid/delete/abc"}}"#,
            ),
            None,
            Duration::ZERO,
        );
        let extras = destination(url)
            .with_thumbnail("{json:result.thumbnail_url}")
            .unwrap()
            .with_deletion("{json:result.delete}")
            .unwrap();
        let outcome = run_upload(
            &extras,
            UploadPng::Path(path.clone()),
            UploadCancellation::default(),
            None,
        )
        .unwrap();
        assert_eq!(outcome.url, "https://example.invalid/share/abc");
        assert_eq!(
            outcome.thumbnail.as_deref(),
            Some("https://example.invalid/t.png")
        );
        assert_eq!(
            outcome.deletion.as_deref(),
            Some("https://example.invalid/delete/abc")
        );
        let _ = server.join().unwrap();
        let (url, task) = self::server(
            response(r#"{"result":{"url":"https://example.invalid/share/abc"}}"#),
            None,
            Duration::ZERO,
        );
        let missing = destination(url)
            .with_thumbnail("{json:result.thumbnail_url}")
            .unwrap();
        let error = upload_png(&missing, &path, UploadCancellation::default()).unwrap_err();
        assert!(error.to_string().contains("thumbnail"));
        let _ = task.join().unwrap();
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    #[ignore = "uses a local loopback HTTP server and synthetic PNG; no external connections"]
    fn upload_extracts_regex_capture_from_html() {
        let (root, path) = fixture();
        let (url, server) = server(
            response(r#"<a href="https://example.invalid/share/abc">ok</a>"#),
            None,
            Duration::ZERO,
        );
        let parsed = destination(url)
            .with_regex(r#"href="([^"]+)""#, Some("1"))
            .unwrap();
        assert_eq!(
            upload_png(&parsed, &path, UploadCancellation::default()).unwrap(),
            "https://example.invalid/share/abc"
        );
        let _ = server.join().unwrap();
        let (url, task) = self::server(response("no link here"), None, Duration::ZERO);
        let missing = destination(url)
            .with_regex(r#"href="([^"]+)""#, Some("1"))
            .unwrap();
        let error = upload_png(&missing, &path, UploadCancellation::default()).unwrap_err();
        assert!(error.to_string().contains("regex"));
        let _ = task.join().unwrap();
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    #[ignore = "uses a local loopback HTTP server and synthetic PNG; no external connections"]
    fn upload_extracts_xml_path_from_body() {
        let (root, path) = fixture();
        let (url, server) = server(
            response(
                r#"<?xml version="1.0"?><files><file><url>https://example.invalid/share/abc</url></file></files>"#,
            ),
            None,
            Duration::ZERO,
        );
        let parsed = destination(url).with_xml("/files/file[1]/url").unwrap();
        assert_eq!(
            upload_png(&parsed, &path, UploadCancellation::default()).unwrap(),
            "https://example.invalid/share/abc"
        );
        let _ = server.join().unwrap();
        let (url, task) = self::server(
            response(r#"<files><file><name>image.png</name></file></files>"#),
            None,
            Duration::ZERO,
        );
        let missing = destination(url).with_xml("/files/file[1]/url").unwrap();
        let error = upload_png(&missing, &path, UploadCancellation::default()).unwrap_err();
        assert!(error.to_string().contains("XML"));
        let _ = task.join().unwrap();
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn in_memory_png_upload_rejects_invalid_buffers_without_connecting() -> Result<()> {
        let destination = UploadDestination::manual(
            "https://example.invalid/upload".into(),
            UploadMethod::Post,
            "file".into(),
            Some("/url".into()),
            false,
        )?;
        let cancel = UploadCancellation::default();
        assert!(
            upload_png_bytes_with_progress(&destination, Vec::new(), cancel.clone(), None).is_err()
        );
        assert!(
            upload_png_bytes_with_progress(&destination, b"not a png".to_vec(), cancel, None)
                .is_err()
        );
        Ok(())
    }

    #[test]
    #[ignore = "uses a local loopback HTTP server and synthetic PNG; no external connections"]
    fn upload_sends_encoded_pixels_without_a_library_file() {
        let (root, path) = fixture();
        let png = std::fs::read(&path).unwrap();
        std::fs::remove_file(&path).unwrap();
        let (url, server) = server(
            response(r#"{"result":{"url":"https://example.invalid/share/memory"}}"#),
            None,
            Duration::ZERO,
        );
        assert_eq!(
            upload_png_bytes_with_progress(
                &destination(url),
                png,
                UploadCancellation::default(),
                None
            )
            .unwrap()
            .url,
            "https://example.invalid/share/memory"
        );
        let _ = server.join().unwrap();
        assert!(!path.exists());
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    #[ignore = "uses a local loopback HTTP server and synthetic PNG; no external connections"]
    fn upload_sends_custom_request_headers_on_the_wire() {
        let (root, path) = fixture();
        let (url, server) = server(
            response(r#"{"result":{"url":"https://example.invalid/share/headers"}}"#),
            None,
            Duration::ZERO,
        );
        let destination = destination(url)
            .with_headers([("X-Client", "screenshots")])
            .unwrap();
        assert_eq!(
            upload_png(&destination, &path, UploadCancellation::default()).unwrap(),
            "https://example.invalid/share/headers"
        );
        let request = String::from_utf8(server.join().unwrap()).unwrap();
        let lower = request.to_ascii_lowercase();
        assert!(lower.contains("x-client: screenshots"));
        assert!(!lower.contains("authorization"));
        assert!(!lower.contains("x-api-key"));
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    #[ignore = "uses a local loopback HTTP server and synthetic PNG; no external connections"]
    fn upload_streams_png_and_rejects_unsafe_or_oversized_responses() {
        let (root, path) = fixture();
        let original = std::fs::read(&path).unwrap();
        let (url, server) = server(
            response(r#"{"result":{"url":"https://example.invalid/share/abc"}}"#),
            None,
            Duration::ZERO,
        );
        let started = std::time::Instant::now();
        assert_eq!(
            upload_png(&destination(url), &path, UploadCancellation::default()).unwrap(),
            "https://example.invalid/share/abc"
        );
        eprintln!(
            "Synthetic PNG loopback upload including runtime/client setup and teardown: {:?}",
            started.elapsed()
        );
        let request = server.join().unwrap();
        let (url, task) = self::server(
            response(r#"{"result":{"url":"https://example.invalid/share/progress"}}"#),
            None,
            Duration::ZERO,
        );
        let (reporter, mut progress) = upload_progress_channel();
        let started = std::time::Instant::now();
        assert_eq!(
            upload_png_with_progress(
                &destination(url),
                &path,
                UploadCancellation::default(),
                reporter
            )
            .unwrap()
            .url,
            "https://example.invalid/share/progress"
        );
        eprintln!(
            "Synthetic PNG loopback upload with progress including teardown: {:?}",
            started.elapsed()
        );
        task.join().unwrap();
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        runtime.block_on(async {
            let final_progress = progress.next().await.unwrap();
            assert_eq!(
                (final_progress.prepared, final_progress.total),
                (original.len() as u64, original.len() as u64)
            );
            assert!(progress.next().await.is_none());
        });
        let text = String::from_utf8_lossy(&request);
        assert!(text.starts_with("POST /upload HTTP/1.1"));
        assert!(text.contains("name=\"file\""));
        assert!(text.contains("filename=\"screenshot.png\""));
        assert!(!text.contains("private-source-name.png"));
        assert!(
            request
                .windows(original.len())
                .any(|window| window == original)
        );
        for json in [
            r#"{"result":{"url":"javascript:alert(1)"}}"#,
            r#"{"result":{"url":"https://user:secret@example.invalid/"}}"#,
            r#"{"result":{}}"#,
            "not JSON",
        ] {
            let (url, task) = self::server(response(json), None, Duration::ZERO);
            assert!(upload_png(&destination(url), &path, UploadCancellation::default()).is_err());
            task.join().unwrap();
        }
        let (url, task) = self::server(
            b"HTTP/1.1 200 OK\r\nContent-Length: 1048577\r\nConnection: close\r\n\r\n".to_vec(),
            None,
            Duration::ZERO,
        );
        assert!(
            upload_png(&destination(url), &path, UploadCancellation::default())
                .unwrap_err()
                .to_string()
                .contains("1 MiB")
        );
        task.join().unwrap();
        let mut chunked =
            b"HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\nConnection: close\r\n\r\n".to_vec();
        for _ in 0..65 {
            chunked.extend_from_slice(b"4000\r\n");
            chunked.extend_from_slice(&[b'x'; 16384]);
            chunked.extend_from_slice(b"\r\n");
        }
        chunked.extend_from_slice(b"0\r\n\r\n");
        let (url, task) = self::server(chunked, None, Duration::ZERO);
        assert!(
            upload_png(&destination(url), &path, UploadCancellation::default())
                .unwrap_err()
                .to_string()
                .contains("1 MiB")
        );
        task.join().unwrap();
        assert_eq!(std::fs::read(&path).unwrap(), original);
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    #[ignore = "uses local loopback HTTP servers to verify redirect blocking and cancellation"]
    fn upload_does_not_follow_redirects_and_can_cancel_a_waiting_response() {
        let (root, path) = fixture();
        let forbidden = TcpListener::bind("127.0.0.1:0").unwrap();
        forbidden.set_nonblocking(true).unwrap();
        let redirect = format!(
            "HTTP/1.1 307 Temporary Redirect\r\nLocation: http://{}/forbidden\r\nContent-Length: 0\r\nConnection: close\r\n\r\n",
            forbidden.local_addr().unwrap()
        );
        let (url, task) = server(redirect.into_bytes(), None, Duration::ZERO);
        assert!(
            upload_png(&destination(url), &path, UploadCancellation::default())
                .unwrap_err()
                .to_string()
                .contains("307")
        );
        task.join().unwrap();
        assert_eq!(
            forbidden.accept().unwrap_err().kind(),
            std::io::ErrorKind::WouldBlock
        );
        let (sent, received) = mpsc::sync_channel(1);
        let (url, task) = server(Vec::new(), Some(sent), Duration::from_millis(150));
        let cancel = UploadCancellation::default();
        let signal = cancel.clone();
        let image = path.clone();
        let worker = thread::spawn(move || upload_png(&destination(url), &image, signal));
        received.recv_timeout(Duration::from_secs(3)).unwrap();
        cancel.cancel();
        assert!(
            worker
                .join()
                .unwrap()
                .unwrap_err()
                .to_string()
                .contains("canceled")
        );
        task.join().unwrap();
        let cancel = UploadCancellation::default();
        cancel.cancel();
        assert!(
            upload_png(
                &destination("http://127.0.0.1:9/upload".into()),
                &path,
                cancel
            )
            .unwrap_err()
            .to_string()
            .contains("canceled")
        );
        std::fs::remove_dir_all(root).unwrap();
    }
}
