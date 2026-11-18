//! Explicit official SXCU execution. The legacy native importer remains separate.
use crate::upload::upload_source::{TrackedUploadReader, UploadSourceGuard, verify_transfer};
use anyhow::{Context, Result, ensure};
use base64::Engine;
use serde::{
    Deserialize, Serialize,
    de::{self, DeserializeSeed, MapAccess, SeqAccess, Visitor},
};
use serde_json::Value;
use std::{
    collections::{BTreeMap, HashSet},
    fs::File,
    io::Read,
    path::{Path, PathBuf},
    sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    },
    time::Duration,
};
use tokio::io::AsyncReadExt;
use zeroize::{Zeroize, Zeroizing};

#[derive(Clone, Copy, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ShareXInputKind {
    Image,
    File,
    Text,
    Url,
    UrlSharing,
}
impl ShareXInputKind {
    fn name(self) -> &'static str {
        match self {
            Self::Image => "image",
            Self::File => "file",
            Self::Text => "text",
            Self::Url => "url",
            Self::UrlSharing => "url_sharing",
        }
    }
    fn flag(self) -> u32 {
        match self {
            Self::Image => 1,
            Self::Text => 2,
            Self::File => 4,
            Self::Url => 8,
            Self::UrlSharing => 16,
        }
    }
    fn is_file(self) -> bool {
        matches!(self, Self::Image | Self::File)
    }
}
#[derive(Serialize)]
pub struct ShareXEvent {
    pub title: String,
    pub text: String,
}
#[derive(Serialize)]
pub struct ShareXOutcome {
    pub success: bool,
    pub status: u16,
    pub url: Option<String>,
    pub thumbnail: Option<String>,
    pub deletion: Option<String>,
    pub error_message: Option<String>,
    pub events: Vec<ShareXEvent>,
}

struct SecretJson(Value);
impl Drop for SecretJson {
    fn drop(&mut self) {
        fn clear(value: &mut Value) {
            match value {
                Value::String(value) => value.zeroize(),
                Value::Array(values) => values.iter_mut().for_each(clear),
                Value::Object(values) => values.values_mut().for_each(clear),
                _ => {}
            }
        }
        clear(&mut self.0);
    }
}
struct StrictSeed(usize);
impl<'de> DeserializeSeed<'de> for StrictSeed {
    type Value = Value;
    fn deserialize<D: serde::Deserializer<'de>>(
        self,
        deserializer: D,
    ) -> std::result::Result<Value, D::Error> {
        if self.0 > 64 {
            return Err(de::Error::custom("JSON depth limit"));
        }
        deserializer.deserialize_any(StrictVisitor(self.0))
    }
}
struct StrictVisitor(usize);
impl<'de> Visitor<'de> for StrictVisitor {
    type Value = Value;
    fn expecting(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        f.write_str("bounded JSON")
    }
    fn visit_bool<E: de::Error>(self, v: bool) -> std::result::Result<Value, E> {
        Ok(v.into())
    }
    fn visit_i64<E: de::Error>(self, v: i64) -> std::result::Result<Value, E> {
        Ok(v.into())
    }
    fn visit_u64<E: de::Error>(self, v: u64) -> std::result::Result<Value, E> {
        Ok(v.into())
    }
    fn visit_f64<E: de::Error>(self, v: f64) -> std::result::Result<Value, E> {
        serde_json::Number::from_f64(v)
            .map(Value::Number)
            .ok_or_else(|| E::custom("Invalid number"))
    }
    fn visit_str<E: de::Error>(self, v: &str) -> std::result::Result<Value, E> {
        Ok(v.into())
    }
    fn visit_string<E: de::Error>(self, v: String) -> std::result::Result<Value, E> {
        Ok(v.into())
    }
    fn visit_none<E: de::Error>(self) -> std::result::Result<Value, E> {
        Ok(Value::Null)
    }
    fn visit_unit<E: de::Error>(self) -> std::result::Result<Value, E> {
        Ok(Value::Null)
    }
    fn visit_seq<A: SeqAccess<'de>>(self, mut seq: A) -> std::result::Result<Value, A::Error> {
        let mut values = Vec::new();
        while let Some(value) = seq.next_element_seed(StrictSeed(self.0 + 1))? {
            if values.len() >= 1024 {
                return Err(de::Error::custom("JSON array limit"));
            }
            values.push(value);
        }
        Ok(Value::Array(values))
    }
    fn visit_map<A: MapAccess<'de>>(self, mut map: A) -> std::result::Result<Value, A::Error> {
        let mut values = serde_json::Map::new();
        while let Some(key) = map.next_key::<String>()? {
            if values.len() >= 1024 || values.contains_key(&key) {
                return Err(de::Error::custom("Duplicate or oversized JSON object"));
            }
            values.insert(key, map.next_value_seed(StrictSeed(self.0 + 1))?);
        }
        Ok(Value::Object(values))
    }
}
fn json(bytes: &[u8]) -> Result<SecretJson> {
    let mut decoder = serde_json::Deserializer::from_slice(bytes);
    let value = StrictSeed(0)
        .deserialize(&mut decoder)
        .map_err(|_| anyhow::anyhow!("Invalid, duplicate or overly nested SXCU JSON"))?;
    decoder
        .end()
        .map_err(|_| anyhow::anyhow!("Trailing SXCU JSON data"))?;
    Ok(SecretJson(value))
}
fn opened(path: &Path, limit: u64, label: &str) -> Result<(File, u64)> {
    ensure!(
        path.as_os_str().as_encoded_bytes().len() <= 4096,
        "{label} path exceeds 4096 bytes"
    );
    let mut options = File::options();
    options.read(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.custom_flags(libc::O_NONBLOCK);
    }
    let file = options
        .open(path)
        .with_context(|| format!("Could not open {label}"))?;
    let metadata = file.metadata()?;
    ensure!(
        metadata.is_file() && metadata.len() <= limit,
        "{label} must be a regular file within its byte limit"
    );
    Ok((file, metadata.len()))
}
fn read(path: &Path, limit: u64, label: &str) -> Result<Zeroizing<Vec<u8>>> {
    let (file, length) = opened(path, limit, label)?;
    let mut bytes = Zeroizing::new(Vec::with_capacity(length as usize));
    file.take(length + 1).read_to_end(&mut bytes)?;
    ensure!(
        bytes.len() as u64 == length,
        "{label} changed while reading"
    );
    Ok(bytes)
}
fn sensitive(key: &str) -> bool {
    let normalized = key.to_ascii_lowercase().replace(['-', '_', ' '], "");
    let native = crate::upload::RESERVED_QUERY_NAMES
        .iter()
        .any(|name| name.replace(['-', '_'], "") == normalized);
    native
        || matches!(
            normalized.as_str(),
            "accesskey"
                | "accesskeyid"
                | "secretaccesskey"
                | "credential"
                | "credentials"
                | "session"
                | "sessionid"
                | "xauth"
                | "xkey"
        )
        || key
            .split(|value: char| !value.is_ascii_alphanumeric())
            .any(|part| {
                matches!(
                    part.to_ascii_lowercase().as_str(),
                    "auth" | "key" | "credential" | "credentials"
                )
            })
        || [
            "authorization",
            "cookie",
            "apikey",
            "token",
            "password",
            "passwd",
            "secret",
            "signature",
        ]
        .iter()
        .any(|part| normalized.contains(part))
}

struct Projected {
    bytes: Zeroizing<Vec<u8>>,
    spans: Vec<(usize, usize)>,
}
fn projected(text: &str, form: bool) -> Projected {
    fn hex(value: u8) -> Option<u8> {
        match value {
            b'0'..=b'9' => Some(value - b'0'),
            b'a'..=b'f' => Some(value - b'a' + 10),
            b'A'..=b'F' => Some(value - b'A' + 10),
            _ => None,
        }
    }
    let source = text.as_bytes();
    let mut bytes = Zeroizing::new(Vec::with_capacity(source.len()));
    let mut spans = Vec::with_capacity(source.len());
    let mut index = 0;
    while index < source.len() {
        let start = index;
        if source[index] == b'%'
            && index + 2 < source.len()
            && let (Some(a), Some(b)) = (hex(source[index + 1]), hex(source[index + 2]))
        {
            bytes.push(a * 16 + b);
            index += 3;
        } else {
            bytes.push(if form && source[index] == b'+' {
                b' '
            } else {
                source[index]
            });
            index += 1;
        }
        spans.push((start, index));
    }
    Projected { bytes, spans }
}
fn delivered_query_name(text: &str) -> Result<Zeroizing<String>> {
    ensure!(
        text.len() <= 16384,
        "SXCU query name exceeds its projection bound"
    );
    let value = projected(text, true);
    let text = std::str::from_utf8(&value.bytes)
        .map_err(|_| anyhow::anyhow!("Invalid UTF-8 SXCU delivered query name"))?;
    Ok(Zeroizing::new(text.to_owned()))
}
fn query_credentials(url: &reqwest::Url) -> Result<Vec<Zeroizing<String>>> {
    let mut values = Vec::new();
    if let Some(query) = url.query() {
        for pair in query.split('&') {
            let (key, value) = pair.split_once('=').unwrap_or((pair, ""));
            let key = delivered_query_name(key)?;
            if sensitive(&key) && !value.is_empty() {
                let value = projected(value, true);
                let value = std::str::from_utf8(&value.bytes)
                    .map_err(|_| anyhow::anyhow!("Invalid UTF-8 SXCU credential projection"))?;
                if !value.is_empty() {
                    values.push(Zeroizing::new(value.to_owned()));
                }
            }
        }
    }
    Ok(values)
}

// Advances in the same order as serde's numeric visits; strict JSON keys are quoted.
struct JsonNumbers<'a> {
    text: &'a str,
    offset: usize,
}
impl<'a> JsonNumbers<'a> {
    fn next(&mut self) -> Option<&'a str> {
        let source = self.text.as_bytes();
        while self.offset < source.len() {
            if source[self.offset] == b'"' {
                self.offset += 1;
                while self.offset < source.len() {
                    if source[self.offset] == b'\\' {
                        self.offset += 2;
                    } else if source[self.offset] == b'"' {
                        self.offset += 1;
                        break;
                    } else {
                        self.offset += 1;
                    }
                }
            } else if source[self.offset] == b'-' || source[self.offset].is_ascii_digit() {
                let start = self.offset;
                self.offset += 1;
                while self.offset < source.len()
                    && (source[self.offset].is_ascii_digit()
                        || matches!(source[self.offset], b'.' | b'e' | b'E' | b'+' | b'-'))
                {
                    self.offset += 1;
                }
                return Some(&self.text[start..self.offset]);
            } else {
                self.offset += 1;
            }
        }
        None
    }
}
#[derive(Default)]
struct BodyAuth<'a> {
    values: Vec<Zeroizing<String>>,
    bytes: usize,
    limited: bool,
    numbers: Option<JsonNumbers<'a>>,
}
impl BodyAuth<'_> {
    fn add_number(&mut self, value: &str, auth: bool) -> Result<()> {
        let Some(raw) = self.numbers.as_mut().and_then(JsonNumbers::next) else {
            self.limited = true;
            anyhow::bail!("SXCU numeric authentication token position is unavailable");
        };
        if auth {
            self.add(value)?;
            self.add(raw)?;
        }
        Ok(())
    }
    fn add_named(&mut self, name: &str, value: &str) -> Result<()> {
        let mut values = Vec::new();
        add_auth_secret(&mut values, name, value)?;
        for value in values {
            self.add(&value)?;
        }
        Ok(())
    }
    fn add(&mut self, value: &str) -> Result<()> {
        if value.is_empty() || self.values.iter().any(|old| old.as_str() == value) {
            return Ok(());
        }
        self.bytes = self.bytes.saturating_add(value.len());
        if self.values.len() >= 128 || self.bytes > 256 * 1024 {
            self.limited = true;
            anyhow::bail!("SXCU body authentication exceeds its bounded value set");
        }
        self.values.push(Zeroizing::new(value.to_owned()));
        Ok(())
    }
}
struct BodySeed<'a, 'text> {
    state: &'a mut BodyAuth<'text>,
    auth: bool,
    cookie: u8,
    depth: usize,
}
impl<'de> DeserializeSeed<'de> for BodySeed<'_, '_> {
    type Value = ();
    fn deserialize<D: serde::Deserializer<'de>>(
        self,
        deserializer: D,
    ) -> std::result::Result<(), D::Error> {
        if self.depth > 64 {
            self.state.limited = true;
            return Err(de::Error::custom("body depth"));
        }
        deserializer.deserialize_any(BodyVisitor(self))
    }
}
struct BodyVisitor<'a, 'text>(BodySeed<'a, 'text>);
impl<'de> Visitor<'de> for BodyVisitor<'_, '_> {
    type Value = ();
    fn expecting(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        f.write_str("bounded configured body")
    }
    fn visit_str<E: de::Error>(self, value: &str) -> std::result::Result<(), E> {
        if self.0.auth {
            self.0
                .state
                .add_named(
                    if self.0.cookie == 1 {
                        "Cookie"
                    } else if self.0.cookie == 2 {
                        "Set-Cookie"
                    } else {
                        "body"
                    },
                    value,
                )
                .map_err(|_| E::custom("body authentication limit"))?;
        }
        Ok(())
    }
    fn visit_string<E: de::Error>(self, value: String) -> std::result::Result<(), E> {
        self.visit_str(&value)
    }
    fn visit_bool<E: de::Error>(self, value: bool) -> std::result::Result<(), E> {
        self.visit_str(if value { "true" } else { "false" })
    }
    fn visit_i64<E: de::Error>(self, value: i64) -> std::result::Result<(), E> {
        self.0
            .state
            .add_number(&value.to_string(), self.0.auth)
            .map_err(|_| E::custom("body authentication limit"))
    }
    fn visit_u64<E: de::Error>(self, value: u64) -> std::result::Result<(), E> {
        self.0
            .state
            .add_number(&value.to_string(), self.0.auth)
            .map_err(|_| E::custom("body authentication limit"))
    }
    fn visit_f64<E: de::Error>(self, value: f64) -> std::result::Result<(), E> {
        self.0
            .state
            .add_number(&value.to_string(), self.0.auth)
            .map_err(|_| E::custom("body authentication limit"))
    }
    fn visit_unit<E: de::Error>(self) -> std::result::Result<(), E> {
        Ok(())
    }
    fn visit_none<E: de::Error>(self) -> std::result::Result<(), E> {
        Ok(())
    }
    fn visit_seq<A: SeqAccess<'de>>(self, mut values: A) -> std::result::Result<(), A::Error> {
        while values
            .next_element_seed(BodySeed {
                state: &mut *self.0.state,
                auth: self.0.auth,
                cookie: self.0.cookie,
                depth: self.0.depth + 1,
            })?
            .is_some()
        {}
        Ok(())
    }
    fn visit_map<A: MapAccess<'de>>(self, mut values: A) -> std::result::Result<(), A::Error> {
        while let Some(key) = values.next_key::<String>()? {
            values.next_value_seed(BodySeed {
                state: &mut *self.0.state,
                auth: self.0.auth || sensitive(&key),
                cookie: if key.eq_ignore_ascii_case("cookie") {
                    1
                } else if key.eq_ignore_ascii_case("set-cookie") {
                    2
                } else {
                    self.0.cookie
                },
                depth: self.0.depth + 1,
            })?;
        }
        Ok(())
    }
}
fn body_credentials(text: &str) -> Result<Vec<Zeroizing<String>>> {
    ensure!(
        text.len() <= 512 * 1024,
        "SXCU configured body exceeds 512 KiB"
    );
    let mut state = BodyAuth::default();
    let trimmed = text.trim_start();
    if trimmed.starts_with(['{', '[']) {
        state.numbers = Some(JsonNumbers { text, offset: 0 });
        let mut decoder = serde_json::Deserializer::from_str(text);
        let parsed = BodySeed {
            state: &mut state,
            auth: false,
            cookie: 0,
            depth: 0,
        }
        .deserialize(&mut decoder)
        .is_ok()
            && decoder.end().is_ok();
        ensure!(
            !state.limited,
            "SXCU body authentication or depth exceeds its bound"
        );
        if parsed {
            return Ok(state.values);
        }
    }
    if trimmed.starts_with('<') {
        let upper = Zeroizing::new(text.to_ascii_uppercase());
        ensure!(
            !upper.contains("<!DOCTYPE") && !upper.contains("<!ENTITY"),
            "SXCU configured XML forbids DTD and external entities"
        );
        let options = roxmltree::ParsingOptions {
            allow_dtd: false,
            nodes_limit: 65536,
            ..Default::default()
        };
        match roxmltree::Document::parse_with_options(text, options) {
            Ok(document) => {
                for node in document.descendants().filter(|node| node.is_element()) {
                    ensure!(
                        node.ancestors().count() <= 66,
                        "SXCU configured XML depth exceeds its bound"
                    );
                    let auth = node
                        .ancestors()
                        .any(|parent| parent.is_element() && sensitive(parent.tag_name().name()));
                    let cookie = node
                        .ancestors()
                        .filter(|parent| parent.is_element())
                        .find_map(|parent| {
                            let name = parent.tag_name().name();
                            if name.eq_ignore_ascii_case("Cookie") {
                                Some("Cookie")
                            } else if name.eq_ignore_ascii_case("Set-Cookie") {
                                Some("Set-Cookie")
                            } else {
                                None
                            }
                        });
                    if auth {
                        if let Some(cookie) = cookie {
                            // Individual text values are headers; never concatenate sibling elements.
                            for text in node
                                .children()
                                .filter(|child| child.is_text())
                                .filter_map(|child| child.text())
                            {
                                if !text.trim().is_empty() {
                                    state.add_named(cookie, text)?;
                                }
                            }
                        } else {
                            let value = Zeroizing::new(
                                node.descendants()
                                    .filter_map(|child| {
                                        child.is_text().then(|| child.text()).flatten()
                                    })
                                    .collect::<String>(),
                            );
                            state.add_named(node.tag_name().name(), &value)?;
                        }
                    }
                    for attribute in node.attributes() {
                        if auth || sensitive(attribute.name()) {
                            let name = if attribute.name().eq_ignore_ascii_case("Cookie") {
                                "Cookie"
                            } else if attribute.name().eq_ignore_ascii_case("Set-Cookie") {
                                "Set-Cookie"
                            } else {
                                cookie.unwrap_or(attribute.name())
                            };
                            state.add_named(name, attribute.value())?;
                        }
                    }
                }
                return Ok(state.values);
            }
            Err(roxmltree::Error::NodesLimitReached | roxmltree::Error::DtdDetected) => {
                anyhow::bail!("SXCU configured XML exceeds its safe parser bounds")
            }
            Err(_) => {}
        }
    }
    for pair in text.split('&') {
        if let Some((key, value)) = pair.split_once('=') {
            let key = projected(key, true);
            let key = std::str::from_utf8(&key.bytes)
                .map_err(|_| anyhow::anyhow!("Invalid UTF-8 configured form name"))?;
            if sensitive(key) {
                let value = projected(value, true);
                let value = std::str::from_utf8(&value.bytes)
                    .map_err(|_| anyhow::anyhow!("Invalid UTF-8 configured form credential"))?;
                state.add_named(key, value)?;
            }
        }
    }
    Ok(state.values)
}

fn one() -> u32 {
    1
}
#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct Options {
    #[serde(default = "one")]
    version: u32,
    #[serde(default)]
    allow_http: bool,
    #[serde(default)]
    filename: Option<String>,
    #[serde(default)]
    answers: BTreeMap<String, String>,
    #[serde(default)]
    answer_indices: BTreeMap<String, u32>,
    #[serde(default)]
    function_names: BTreeMap<String, String>,
    #[serde(default)]
    name_values: BTreeMap<String, String>,
    #[serde(default)]
    allow_environment_names: bool,
    #[serde(default)]
    credential_profile: Option<PathBuf>,
    #[serde(default)]
    credential_origin: Option<String>,
}
impl Default for Options {
    fn default() -> Self {
        Self {
            version: 1,
            allow_http: false,
            filename: None,
            answers: BTreeMap::new(),
            answer_indices: BTreeMap::new(),
            function_names: BTreeMap::new(),
            name_values: BTreeMap::new(),
            allow_environment_names: false,
            credential_profile: None,
            credential_origin: None,
        }
    }
}
impl Drop for Options {
    fn drop(&mut self) {
        for value in self
            .answers
            .values_mut()
            .chain(self.name_values.values_mut())
        {
            value.zeroize();
        }
    }
}
impl Options {
    fn load(path: Option<&Path>) -> Result<Self> {
        let Some(path) = path else {
            return Ok(Self::default());
        };
        let bytes = read(path, 65536, "SXCU options")?;
        let mut value = json(&bytes)?;
        let mut options: Self = serde_json::from_value(std::mem::take(&mut value.0))
            .map_err(|_| anyhow::anyhow!("Invalid strict SXCU execution options"))?;
        ensure!(
            options.version == 1
                && options.answers.len() + options.answer_indices.len() <= 128
                && options.function_names.len() <= 128
                && options.name_values.len() <= 32,
            "Invalid or oversized SXCU execution options"
        );
        let mut total = 0usize;
        for (key, value) in options
            .answers
            .iter()
            .chain(options.name_values.iter())
            .chain(options.function_names.iter())
        {
            ensure!(
                !key.is_empty()
                    && key.len() <= 256
                    && value.len() <= 4096
                    && !key.chars().any(char::is_control),
                "Invalid SXCU answer/context entry"
            );
            total = total.saturating_add(key.len() + value.len());
        }
        for (key, index) in &options.answer_indices {
            ensure!(
                !key.is_empty()
                    && key.len() <= 256
                    && !key.chars().any(char::is_control)
                    && *index < 16384
                    && !options.answers.contains_key(key),
                "Invalid, duplicate or oversized SXCU selection index"
            );
            total = total.saturating_add(key.len() + 4);
        }
        for name in options.function_names.values() {
            ensure!(
                [
                    "filename",
                    "input",
                    "response",
                    "responseurl",
                    "header",
                    "base64",
                    "random",
                    "select",
                    "inputbox",
                    "prompt",
                    "outputbox",
                    "json",
                    "regex",
                    "xml"
                ]
                .iter()
                .any(|function| function.eq_ignore_ascii_case(name)),
                "SXCU computed-name bindings must name a published function"
            );
        }
        ensure!(total <= 32768, "SXCU answers/context exceed 32 KiB");
        if let Some(name) = &options.filename {
            filename(name)?;
        }
        if let Some(profile) = &mut options.credential_profile {
            ensure!(
                profile.as_os_str().as_encoded_bytes().len() <= 4096,
                "SXCU credential profile path exceeds its limit"
            );
            if profile.is_relative() {
                *profile = path.parent().unwrap_or(Path::new(".")).join(&profile);
            }
        }
        if let Some(origin) = &options.credential_origin {
            ensure!(
                canonical_origin(origin)? == *origin,
                "SXCU credential origin must be canonical HTTPS"
            );
        }
        Ok(options)
    }
}
fn filename(name: &str) -> Result<()> {
    ensure!(
        (1..=128).contains(&name.len())
            && !name.contains(['/', '\\'])
            && name != "."
            && name != ".."
            && !name.chars().any(char::is_control),
        "Public SXCU filename must be a path-free name within 128 bytes"
    );
    Ok(())
}
fn canonical_origin(text: &str) -> Result<String> {
    let url =
        reqwest::Url::parse(text).map_err(|_| anyhow::anyhow!("Invalid SXCU credential origin"))?;
    ensure!(
        url.scheme() == "https"
            && url.host_str().is_some()
            && url.username().is_empty()
            && url.password().is_none()
            && url.query().is_none()
            && url.fragment().is_none()
            && matches!(url.path(), "" | "/"),
        "SXCU credentials require an HTTPS origin without path/userinfo/query"
    );
    Ok(url.origin().ascii_serialization())
}
struct Metadata {
    method: String,
    body: String,
    destinations: u32,
    field: String,
    original_origin: Option<String>,
    literal_auth: bool,
    has_url: bool,
}
impl Metadata {
    fn read(value: &Value) -> Result<Self> {
        let object = value
            .as_object()
            .context("SXCU profile must be an object")?;
        let known = [
            "version",
            "name",
            "destinationtype",
            "requestmethod",
            "requesturl",
            "parameters",
            "headers",
            "body",
            "arguments",
            "fileformname",
            "data",
            "url",
            "thumbnailurl",
            "deletionurl",
            "errormessage",
        ];
        let mut fields = BTreeMap::new();
        for (key, value) in object {
            let key = key.to_ascii_lowercase();
            ensure!(
                known.contains(&key.as_str()) && fields.insert(key, value).is_none(),
                "Unknown or duplicate SXCU field"
            );
        }
        let string = |key: &str| -> Result<&str> {
            match fields.get(key) {
                None | Some(Value::Null) => Ok(""),
                Some(Value::String(value)) => Ok(value),
                _ => anyhow::bail!(
                    "SXCU string field has an invalid type; object Data is an application legacy profile, not official SXCU"
                ),
            }
        };
        let version = string("version")?;
        ensure!(version.len() <= 64, "SXCU version is too long");
        let components: Vec<u32> = version
            .split('.')
            .map(str::parse)
            .collect::<std::result::Result<_, _>>()
            .map_err(|_| {
                anyhow::anyhow!("SXCU version must contain two to four numeric components")
            })?;
        ensure!(
            (2..=4).contains(&components.len()),
            "Unsupported SXCU version"
        );
        let mut normalized = [0u32; 4];
        normalized[..components.len()].copy_from_slice(&components);
        ensure!(
            normalized > [12, 3, 1, 0],
            "Official SXCU versions at or below 12.3.1 are unsupported"
        );
        let enumeration = |key: &str, names: &[&str], default: &str| -> Result<String> {
            match fields.get(key) {
                None | Some(Value::Null) => Ok(default.to_owned()),
                Some(Value::String(value)) => names
                    .iter()
                    .find(|name| name.eq_ignore_ascii_case(value.trim()))
                    .map(|name| (*name).to_owned())
                    .context("Unsupported SXCU enum value"),
                Some(Value::Number(value)) => value
                    .as_u64()
                    .and_then(|index| names.get(index as usize))
                    .map(|name| (*name).to_owned())
                    .context("Unsupported SXCU enum value"),
                _ => anyhow::bail!("Invalid SXCU enum type"),
            }
        };
        let method = enumeration(
            "requestmethod",
            &["GET", "POST", "PUT", "PATCH", "DELETE"],
            "POST",
        )?;
        let body = enumeration(
            "body",
            &[
                "None",
                "MultipartFormData",
                "FormURLEncoded",
                "JSON",
                "XML",
                "Binary",
            ],
            "None",
        )?;
        let destinations = match fields.get("destinationtype") {
            Some(Value::Number(value)) => value
                .as_u64()
                .filter(|value| *value <= 31)
                .context("Invalid SXCU destination flags")?
                as u32,
            Some(Value::String(value)) => {
                let mut result = 0;
                for name in value
                    .split(',')
                    .map(str::trim)
                    .filter(|name| !name.is_empty())
                {
                    if name.eq_ignore_ascii_case("none") {
                        continue;
                    }
                    let index = [
                        "ImageUploader",
                        "TextUploader",
                        "FileUploader",
                        "URLShortener",
                        "URLSharingService",
                    ]
                    .iter()
                    .position(|candidate| candidate.eq_ignore_ascii_case(name))
                    .context("Unsupported SXCU destination flag")?;
                    result |= 1 << index;
                }
                result
            }
            None | Some(Value::Null) => 0,
            _ => anyhow::bail!("Invalid SXCU destination flags"),
        };
        let request = string("requesturl")?;
        ensure!(!request.is_empty(), "SXCU RequestURL is required");
        for key in [
            "name",
            "fileformname",
            "data",
            "url",
            "thumbnailurl",
            "deletionurl",
            "errormessage",
        ] {
            let _ = string(key)?;
        }
        let mut literal_auth = false;
        for map_kind in ["headers", "parameters", "arguments"] {
            if let Some(map) = fields.get(map_kind).filter(|value| !value.is_null()) {
                let map = map
                    .as_object()
                    .context("SXCU request maps must be objects")?;
                ensure!(map.len() <= 32, "SXCU request map exceeds 32 entries");
                for (key, value) in map {
                    ensure!(
                        !key.is_empty()
                            && key.len() <= 128
                            && (value.is_string() || value.is_null()),
                        "Invalid SXCU request map entry"
                    );
                    let query_name = if map_kind == "parameters" {
                        Some(delivered_query_name(key)?)
                    } else {
                        None
                    };
                    if sensitive(query_name.as_deref().map(String::as_str).unwrap_or(key))
                        && value.as_str().is_some_and(|value| !value.is_empty())
                    {
                        literal_auth = true;
                    }
                }
            }
        }
        if matches!(body.as_str(), "JSON" | "XML") && !body_credentials(string("data")?)?.is_empty()
        {
            literal_auth = true;
        }
        let prefixed = if request
            .get(..8)
            .is_some_and(|value| value.eq_ignore_ascii_case("https://"))
            || request
                .get(..7)
                .is_some_and(|value| value.eq_ignore_ascii_case("http://"))
        {
            request.to_owned()
        } else {
            format!("https://{request}")
        };
        let authority = prefixed
            .split_once("://")
            .map(|(_, rest)| rest.split(['/', '?', '#']).next().unwrap_or_default())
            .unwrap_or_default();
        let original_origin = if !authority.contains(['{', '}', '\\', '@']) {
            reqwest::Url::parse(&prefixed)
                .ok()
                .filter(|url| {
                    url.scheme() == "https"
                        && url.username().is_empty()
                        && url.password().is_none()
                        && url.host_str().is_some()
                })
                .map(|url| url.origin().ascii_serialization())
        } else {
            None
        };
        if let Some((_, query)) = prefixed.split_once('?') {
            for pair in query.split('&') {
                let (key, value) = pair.split_once('=').unwrap_or((pair, ""));
                let extracted = delivered_query_name(key)?;
                let delivered = delivered_query_name(&extracted)?;
                if sensitive(&delivered) && !value.is_empty() {
                    literal_auth = true;
                }
            }
        }
        ensure!(
            !(literal_auth
                && prefixed
                    .get(..7)
                    .is_some_and(|scheme| scheme.eq_ignore_ascii_case("http://"))),
            "SXCU authentication material requires HTTPS before interpretation or credential lookup"
        );
        Ok(Self {
            method,
            body,
            destinations,
            field: string("fileformname")?.to_owned(),
            original_origin,
            literal_auth,
            has_url: !string("url")?.is_empty(),
        })
    }
    fn admit(&self, kind: ShareXInputKind) -> Result<()> {
        ensure!(
            self.destinations & kind.flag() != 0,
            "SXCU destination does not support this input kind"
        );
        ensure!(
            !kind.is_file() || matches!(self.body.as_str(), "MultipartFormData" | "Binary"),
            "SXCU image/file input supports multipart or binary bodies only"
        );
        ensure!(
            !matches!(kind, ShareXInputKind::Url | ShareXInputKind::UrlSharing)
                || self.body != "Binary",
            "SXCU URL input does not support binary bodies"
        );
        ensure!(
            !kind.is_file()
                || self.body != "MultipartFormData"
                || (!self.field.is_empty()
                    && self.field.len() <= 128
                    && !self.field.chars().any(char::is_control)),
            "SXCU multipart file input requires FileFormName"
        );
        Ok(())
    }
}
fn may_return_link(kind: Option<ShareXInputKind>, configured: bool) -> bool {
    kind != Some(ShareXInputKind::UrlSharing) || configured
}
struct Prepared {
    raw: Zeroizing<String>,
    metadata: Metadata,
    options: Options,
    kind: Option<ShareXInputKind>,
    credential: Option<crate::UploadDestination>,
    origin: Option<String>,
}
#[derive(Clone)]
pub struct ShareXPrepared(Arc<Prepared>);
impl ShareXPrepared {
    /// Digest the exact admitted configuration, excluding credential-store token values.
    pub fn config_digest(&self) -> Result<[u8; 32]> {
        use sha2_10::{Digest, Sha256};
        let options = Zeroizing::new(serde_json::to_vec(&self.0.options)?);
        let credential = Zeroizing::new(serde_json::to_vec(&self.0.credential)?);
        let mut hash = Sha256::new();
        hash.update(b"screenshots-sxcu-config-v1\0");
        for bytes in [
            self.0.raw.as_bytes(),
            options.as_slice(),
            credential.as_slice(),
        ] {
            hash.update((bytes.len() as u64).to_le_bytes());
            hash.update(bytes);
        }
        hash.update(serde_json::to_vec(&self.0.kind)?);
        hash.update(b"in-process-interpreter-v1");
        Ok(hash.finalize().into())
    }
    pub fn read(profile: &Path, kind: ShareXInputKind, options: Option<&Path>) -> Result<Self> {
        Self::load(profile, Some(kind), options)
    }
    fn load(profile: &Path, kind: Option<ShareXInputKind>, options: Option<&Path>) -> Result<Self> {
        let bytes = read(profile, 16384, "SXCU profile")?;
        Self::from_snapshot(bytes, kind, options)
    }
    fn from_snapshot(
        bytes: Zeroizing<Vec<u8>>,
        kind: Option<ShareXInputKind>,
        options: Option<&Path>,
    ) -> Result<Self> {
        let value = json(&bytes)?;
        let metadata = Metadata::read(&value.0)?;
        if let Some(kind) = kind {
            metadata.admit(kind)?;
        }
        let options = Options::load(options)?;
        let credential = options
            .credential_profile
            .as_deref()
            .map(crate::UploadDestination::load)
            .transpose()?;
        let native_origin = if let Some(credential) = &credential {
            let target = credential
                .credential_target()
                .context("SXCU credential profile must contain a native credential reference")?;
            let rest = target
                .strip_prefix("Screenshots/upload/")
                .context("Invalid SXCU native credential target")?;
            Some(
                rest.rsplit_once('/')
                    .context("Invalid SXCU native credential target")?
                    .0
                    .to_owned(),
            )
        } else {
            None
        };
        let origin = options
            .credential_origin
            .clone()
            .or(native_origin.clone())
            .or(metadata.original_origin.clone());
        ensure!(
            native_origin
                .as_ref()
                .is_none_or(|native| origin.as_ref() == Some(native)),
            "SXCU explicit and native credential origins conflict"
        );
        ensure!(
            !metadata.literal_auth || origin.is_some(),
            "SXCU authentication requires a fixed HTTPS origin or an explicit credential_origin binding"
        );
        let raw = Zeroizing::new(
            std::str::from_utf8(&bytes)
                .map_err(|_| anyhow::anyhow!("SXCU profile must be UTF-8"))?
                .to_owned(),
        );
        Ok(Self(Arc::new(Prepared {
            raw,
            metadata,
            options,
            kind,
            credential,
            origin,
        })))
    }
    pub fn inspect(profile: &Path, options: Option<&Path>) -> Result<Value> {
        Self::inspect_with_cancel(profile, options, crate::UploadCancellation::default())
    }
    pub fn inspect_with_cancel(
        profile: &Path,
        options: Option<&Path>,
        cancel: crate::UploadCancellation,
    ) -> Result<Value> {
        cancel.check()?;
        let _permit = Permit::acquire()?;
        let prepared = Self::load(profile, None, options)?;
        cancel.check()?;
        prepared.interpret::<Value>("inspect", "", "", None, &cancel)
    }
    fn interpret<T: for<'de> Deserialize<'de>>(
        &self,
        stage: &str,
        filename: &str,
        input: &str,
        response: Option<Value>,
        cancel: &crate::UploadCancellation,
    ) -> Result<T> {
        let request = SecretJson(
            serde_json::json!({"version":1,"stage":stage,"profile_json":self.0.raw.as_str(),"kind":self.0.kind.unwrap_or(ShareXInputKind::File).name(),"filename":filename,"input":input,"answers":self.0.options.answers,"answer_indices":self.0.options.answer_indices,"function_names":self.0.options.function_names,"name_values":self.0.options.name_values,"allow_environment_names":self.0.options.allow_environment_names,"response":response}),
        );
        let request = Zeroizing::new(
            serde_json::to_vec(&request.0)
                .map_err(|_| anyhow::anyhow!("Could not encode private SXCU request"))?,
        );
        let bytes = crate::sharex_interpreter::interpret(&request, cancel)?;
        #[derive(Deserialize)]
        #[serde(deny_unknown_fields)]
        struct Envelope<T> {
            ok: bool,
            result: Option<T>,
            code: Option<String>,
        }
        let envelope: Envelope<T> = serde_json::from_slice(&bytes)
            .map_err(|_| anyhow::anyhow!("Invalid private SXCU interpreter result"))?;
        if !envelope.ok {
            let code = envelope
                .code
                .filter(|code| {
                    code.len() <= 64
                        && code
                            .bytes()
                            .all(|byte| byte.is_ascii_lowercase() || byte == b'_')
                })
                .unwrap_or_else(|| "invalid_expression".into());
            anyhow::bail!("Private SXCU interpreter refused the operation ({code})");
        }
        envelope
            .result
            .context("Private SXCU interpreter returned no result")
    }
    /// Empty URL uses the raw response for image/file/text/shortener profiles;
    /// URLSharing without a configured link deliberately has no link result.
    pub fn may_return_link(&self) -> bool {
        may_return_link(self.0.kind, self.0.metadata.has_url)
    }
    pub fn upload_file(
        &self,
        path: &Path,
        cancel: crate::UploadCancellation,
    ) -> Result<ShareXOutcome> {
        self.execute(path, cancel)
    }
}

#[derive(Default)]
struct Pairs(Vec<(String, String)>);
impl<'de> Deserialize<'de> for Pairs {
    fn deserialize<D: serde::Deserializer<'de>>(
        deserializer: D,
    ) -> std::result::Result<Self, D::Error> {
        struct V;
        impl<'de> Visitor<'de> for V {
            type Value = Pairs;
            fn expecting(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
                f.write_str("ordered string fields")
            }
            fn visit_map<A: MapAccess<'de>>(
                self,
                mut map: A,
            ) -> std::result::Result<Pairs, A::Error> {
                let mut entries = Vec::new();
                let mut seen = HashSet::new();
                while let Some((key, value)) = map.next_entry::<String, String>()? {
                    if entries.len() >= 32 || !seen.insert(key.clone()) {
                        return Err(de::Error::custom("Duplicate/oversized helper map"));
                    }
                    entries.push((key, value));
                }
                Ok(Pairs(entries))
            }
        }
        deserializer.deserialize_map(V)
    }
}
impl Drop for Pairs {
    fn drop(&mut self) {
        for (_, value) in &mut self.0 {
            value.zeroize();
        }
    }
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawEvent {
    title: String,
    text: String,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Request {
    url: String,
    method: String,
    body: String,
    file_field: String,
    attach_file: bool,
    headers: Pairs,
    arguments: Pairs,
    data: String,
    events: Vec<RawEvent>,
    secrets: Vec<String>,
}
impl Drop for Request {
    fn drop(&mut self) {
        self.url.zeroize();
        self.data.zeroize();
        for value in &mut self.secrets {
            value.zeroize();
        }
        for event in &mut self.events {
            event.text.zeroize();
            event.title.zeroize();
        }
    }
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Response {
    success: bool,
    status: u16,
    url: String,
    thumbnail: String,
    deletion: String,
    error_message: String,
    events: Vec<RawEvent>,
    secrets: Vec<String>,
}
impl Drop for Response {
    fn drop(&mut self) {
        for value in [
            &mut self.url,
            &mut self.thumbnail,
            &mut self.deletion,
            &mut self.error_message,
        ] {
            value.zeroize();
        }
        for value in &mut self.secrets {
            value.zeroize();
        }
        for event in &mut self.events {
            event.text.zeroize();
            event.title.zeroize();
        }
    }
}
static ACTIVE: AtomicUsize = AtomicUsize::new(0);
struct Permit;
impl Permit {
    fn acquire() -> Result<Self> {
        let mut current = ACTIVE.load(Ordering::Acquire);
        loop {
            ensure!(current < 2, "Two SXCU uploads are already active");
            match ACTIVE.compare_exchange(current, current + 1, Ordering::AcqRel, Ordering::Acquire)
            {
                Ok(_) => break,
                Err(actual) => current = actual,
            }
        }
        Ok(Self)
    }
}
impl Drop for Permit {
    fn drop(&mut self) {
        ACTIVE.fetch_sub(1, Ordering::AcqRel);
    }
}
fn media(name: &str) -> &'static str {
    match name
        .rsplit('.')
        .next()
        .unwrap_or_default()
        .to_ascii_lowercase()
        .as_str()
    {
        "png" => "image/png",
        "jpg" | "jpeg" => "image/jpeg",
        "webp" => "image/webp",
        "gif" => "image/gif",
        "bmp" => "image/bmp",
        "tif" | "tiff" => "image/tiff",
        "txt" => "text/plain",
        "json" => "application/json",
        "xml" => "application/xml",
        _ => "application/octet-stream",
    }
}
struct SecretMask(Option<regex::bytes::Regex>);
impl SecretMask {
    fn new(secrets: &[Zeroizing<String>]) -> Result<Self> {
        let mut values: Vec<_> = secrets
            .iter()
            .filter(|value| !value.is_empty())
            .map(|value| value.as_str())
            .collect();
        values.sort_unstable_by_key(|value| std::cmp::Reverse(value.len()));
        values.dedup();
        ensure!(
            values.iter().map(|value| value.len()).sum::<usize>() <= 256 * 1024,
            "SXCU authentication redaction exceeds its byte bound"
        );
        if values.is_empty() {
            return Ok(Self(None));
        }
        let pattern = Zeroizing::new(
            values
                .into_iter()
                .map(regex::escape)
                .collect::<Vec<_>>()
                .join("|"),
        );
        let regex = regex::bytes::RegexBuilder::new(&pattern)
            .size_limit(4 * 1024 * 1024)
            .dfa_size_limit(1024 * 1024)
            .nest_limit(16)
            .build()
            .map_err(|_| {
                anyhow::anyhow!("SXCU authentication redaction exceeded its expression bound")
            })?;
        Ok(Self(Some(regex)))
    }
    fn contains_url(&self, text: &str) -> Result<bool> {
        let Some(regex) = &self.0 else {
            return Ok(false);
        };
        if regex.is_match(text.as_bytes()) {
            return Ok(true);
        }
        let url =
            reqwest::Url::parse(text).map_err(|_| anyhow::anyhow!("Invalid SXCU result URL"))?;
        let mut decoded = Zeroizing::new(url.origin().ascii_serialization().into_bytes());
        decoded.extend_from_slice(&projected(url.path(), false).bytes);
        if let Some(query) = url.query() {
            decoded.push(b'?');
            decoded.extend_from_slice(&projected(query, true).bytes);
        }
        if let Some(fragment) = url.fragment() {
            decoded.push(b'#');
            decoded.extend_from_slice(&projected(fragment, false).bytes);
        }
        Ok(regex.is_match(&decoded))
    }
    fn mask(&self, text: &str, limit: usize) -> String {
        let mut spans = Vec::new();
        if let Some(regex) = &self.0 {
            spans.extend(
                regex
                    .find_iter(text.as_bytes())
                    .map(|value| (value.start(), value.end())),
            );
            for form in [false, true] {
                let view = projected(text, form);
                for value in regex.find_iter(&view.bytes) {
                    spans.push((view.spans[value.start()].0, view.spans[value.end() - 1].1));
                }
            }
        }
        spans.sort_unstable();
        let mut merged: Vec<(usize, usize)> = Vec::new();
        for (start, end) in spans {
            if let Some(last) = merged.last_mut()
                && start <= last.1
            {
                last.1 = last.1.max(end);
            } else {
                merged.push((start, end));
            }
        }
        fn append(output: &mut String, text: &str, limit: usize) {
            for value in text.chars() {
                let value = if value.is_control() && value != '\n' && value != '\t' {
                    ' '
                } else {
                    value
                };
                if output.len() + value.len_utf8() > limit {
                    break;
                }
                output.push(value);
            }
        }
        let mut output = String::new();
        let mut previous = 0;
        for (start, end) in merged {
            append(&mut output, &text[previous..start], limit);
            append(&mut output, "[redacted]", limit);
            previous = end;
            if output.len() >= limit {
                return output;
            }
        }
        append(&mut output, &text[previous..], limit);
        output
    }
}
fn link(text: &str, secrets: &SecretMask) -> Result<Option<String>> {
    if text.is_empty() {
        return Ok(None);
    }
    ensure!(text.len() <= 2048, "SXCU result exceeded its link bound");
    let outcome = crate::UploadOutcome {
        url: text.to_owned(),
        thumbnail: None,
        deletion: None,
    }
    .validate_links()?;
    ensure!(
        !secrets.contains_url(&outcome.url)?,
        "SXCU result contained authentication material"
    );
    Ok(Some(outcome.url))
}

enum Payload {
    File(File, u64),
    Bytes(Vec<u8>),
}
impl Payload {
    fn length(&self) -> u64 {
        match self {
            Self::File(_, length) => *length,
            Self::Bytes(bytes) => bytes.len() as u64,
        }
    }
    fn body(
        self,
        count: Arc<std::sync::atomic::AtomicU64>,
        cancel: crate::UploadCancellation,
    ) -> reqwest::Body {
        let length = self.length();
        let reader: Box<dyn tokio::io::AsyncRead + Unpin + Send> = match self {
            Self::File(file, length) => {
                let mut file = tokio::fs::File::from_std(file);
                file.set_max_buf_size(65536);
                Box::new(file.take(length))
            }
            Self::Bytes(bytes) => Box::new(std::io::Cursor::new(bytes)),
        };
        reqwest::Body::wrap_stream(tokio_util::io::ReaderStream::with_capacity(
            TrackedUploadReader::new(reader, length, count, cancel),
            length.clamp(1, 65536) as usize,
        ))
    }
}
impl ShareXPrepared {
    /// Send an already finalized in-memory PNG without a save or disk round trip.
    pub fn upload_png_bytes(
        &self,
        png: Vec<u8>,
        cancel: crate::UploadCancellation,
    ) -> Result<ShareXOutcome> {
        ensure!(
            self.0.kind == Some(ShareXInputKind::Image),
            "In-memory PNG SXCU input requires KindImage"
        );
        ensure!(
            png.starts_with(b"\x89PNG\r\n\x1a\n")
                && png.len() as u64 <= crate::MAX_CAPTURE_BYTES + 16 * 1024 * 1024,
            "Invalid or oversized finalized SXCU PNG input"
        );
        cancel.check()?;
        let permit = Permit::acquire()?;
        let name = self
            .0
            .options
            .filename
            .clone()
            .unwrap_or_else(|| format!("{}.png", uuid::Uuid::new_v4().simple()));
        self.run(
            Some(Payload::Bytes(png)),
            Zeroizing::new(String::new()),
            name,
            None,
            cancel,
            permit,
        )
    }
    /// Explicit private UTF-8 input; no plaintext file or diagnostic payload is retained.
    pub fn upload_text(
        &self,
        text: &str,
        cancel: crate::UploadCancellation,
    ) -> Result<ShareXOutcome> {
        ensure!(
            self.0.kind == Some(ShareXInputKind::Text)
                && text.len() <= 65536
                && !text.contains('\0'),
            "In-memory SXCU text requires KindText and at most 64 KiB without NUL"
        );
        cancel.check()?;
        let permit = Permit::acquire()?;
        let name = self
            .0
            .options
            .filename
            .clone()
            .unwrap_or_else(|| format!("{}.txt", uuid::Uuid::new_v4().simple()));
        let mut outcome = self
            .run(
                None,
                Zeroizing::new(text.to_owned()),
                name,
                None,
                cancel,
                permit,
            )
            .map_err(|_| {
                anyhow::anyhow!(
                    "Private SXCU text upload failed; a remote object may exist; no retry"
                )
            })?;
        // Imported profiles may reflect input into diagnostic events and error messages.
        for event in &mut outcome.events {
            event.title.zeroize();
            event.text.zeroize();
        }
        outcome.events.clear();
        if let Some(message) = outcome.error_message.as_mut() {
            message.zeroize();
        }
        outcome.error_message = None;
        Ok(outcome)
    }
    fn execute(&self, path: &Path, cancel: crate::UploadCancellation) -> Result<ShareXOutcome> {
        cancel.check()?;
        let permit = Permit::acquire()?;
        let kind = self
            .0
            .kind
            .context("SXCU inspection snapshot cannot upload")?;
        let limit = if kind == ShareXInputKind::Image {
            crate::MAX_CAPTURE_BYTES + 16 * 1024 * 1024
        } else if kind.is_file() {
            64 * 1024 * 1024
        } else {
            65536
        };
        let (file, length) = opened(path, limit, "SXCU input")?;
        let admitted = crate::FileIdentity::from_file(&file)?;
        cancel.verify_source_file(&file)?;
        let source_guard = UploadSourceGuard::new(&file, admitted, length)?;
        let name = self.0.options.filename.clone().unwrap_or_else(|| {
            if matches!(kind, ShareXInputKind::Url | ShareXInputKind::UrlSharing) {
                return String::new();
            }
            let extension = if kind == ShareXInputKind::Text {
                "txt"
            } else {
                path.extension()
                    .and_then(|value| value.to_str())
                    .filter(|value| {
                        value.len() <= 16 && value.bytes().all(|byte| byte.is_ascii_alphanumeric())
                    })
                    .unwrap_or("bin")
            };
            format!("{}.{}", uuid::Uuid::new_v4().simple(), extension)
        });
        if kind.is_file() {
            self.run(
                Some(Payload::File(file, length)),
                Zeroizing::new(String::new()),
                name,
                Some(source_guard),
                cancel,
                permit,
            )
        } else {
            let mut bytes = Zeroizing::new(Vec::with_capacity(length as usize));
            file.take(length + 1).read_to_end(&mut bytes)?;
            ensure!(
                bytes.len() as u64 == length,
                "SXCU text input changed while reading"
            );
            let text = std::str::from_utf8(&bytes)
                .map_err(|_| anyhow::anyhow!("SXCU text/URL input must be UTF-8"))?;
            let input = Zeroizing::new(
                if matches!(kind, ShareXInputKind::Url | ShareXInputKind::UrlSharing) {
                    let text = text.trim();
                    let url = reqwest::Url::parse(text)
                        .map_err(|_| anyhow::anyhow!("Invalid URL-shortener input"))?;
                    ensure!(
                        matches!(url.scheme(), "http" | "https")
                            && url.host_str().is_some()
                            && url.username().is_empty()
                            && url.password().is_none()
                            && !text.chars().any(char::is_control),
                        "URL-shortener input must be an HTTP(S) URL without credentials or control characters"
                    );
                    text.to_owned()
                } else {
                    text.to_owned()
                },
            );
            self.run(None, input, name, Some(source_guard), cancel, permit)
        }
    }
    fn run(
        &self,
        mut payload: Option<Payload>,
        input: Zeroizing<String>,
        name: String,
        source_guard: Option<UploadSourceGuard>,
        cancel: crate::UploadCancellation,
        _permit: Permit,
    ) -> Result<ShareXOutcome> {
        cancel.check()?;
        cancel.check()?;
        let kind = self
            .0
            .kind
            .context("SXCU inspection snapshot cannot upload")?;
        let request: Request = self.interpret("prepare", &name, &input, None, &cancel)?;
        ensure!(
            request.method == self.0.metadata.method
                && request.body == self.0.metadata.body
                && request.file_field == self.0.metadata.field,
            "Private SXCU request disagrees with admitted profile"
        );
        ensure!(
            request.url.len() <= 8192 && !request.url.chars().any(char::is_control),
            "SXCU request URL exceeds its limit or contains controls"
        );
        let url = reqwest::Url::parse(&request.url)
            .map_err(|_| anyhow::anyhow!("Invalid interpreted SXCU request URL"))?;
        ensure!(
            (url.scheme() == "https" || (self.0.options.allow_http && url.scheme() == "http"))
                && url.host_str().is_some()
                && url.username().is_empty()
                && url.password().is_none()
                && url.fragment().is_none(),
            "SXCU request requires HTTPS by default, a host and no embedded credentials/fragment"
        );
        let data = Zeroizing::new(if request.data.is_empty() {
            Vec::new()
        } else {
            ensure!(
                request.data.len() <= 699052,
                "SXCU encoded text body exceeds its limit"
            );
            base64::engine::general_purpose::STANDARD
                .decode(&request.data)
                .map_err(|_| anyhow::anyhow!("Invalid private SXCU text body"))?
        });
        ensure!(data.len() <= 512 * 1024, "SXCU text body exceeds 512 KiB");
        let body_secrets = if !data.is_empty() {
            body_credentials(
                std::str::from_utf8(&data)
                    .map_err(|_| anyhow::anyhow!("Configured SXCU body must be UTF-8"))?,
            )?
        } else if request.body == "Binary" && kind == ShareXInputKind::Text {
            body_credentials(&input)?
        } else {
            Vec::new()
        };
        let query_secrets = query_credentials(&url)?;
        let actual_origin = url.origin().ascii_serialization();
        if !body_secrets.is_empty()
            || !query_secrets.is_empty()
            || self.0.metadata.literal_auth
            || request.secrets.iter().any(|value| !value.is_empty())
            || self.0.credential.is_some()
        {
            ensure!(
                url.scheme() == "https" && self.0.origin.as_ref() == Some(&actual_origin),
                "Interpreted SXCU request origin does not match its admitted authentication origin"
            );
        }
        let expected_file = self.0.metadata.body == "MultipartFormData"
            && (kind.is_file()
                || (kind == ShareXInputKind::Text && !self.0.metadata.field.is_empty()));
        ensure!(
            request.attach_file == expected_file,
            "Invalid private SXCU file attachment mode"
        );
        let mut secrets: Vec<Zeroizing<String>> = request
            .secrets
            .iter()
            .filter(|value| !value.is_empty())
            .map(|value| Zeroizing::new(value.clone()))
            .collect();
        for value in body_secrets.into_iter().chain(query_secrets) {
            add_secret(&mut secrets, &value);
        }
        let mut headers = reqwest::header::HeaderMap::new();
        let mut header_bytes = 0usize;
        for (key, value) in &request.headers.0 {
            ensure!(
                ![
                    "host",
                    "content-length",
                    "transfer-encoding",
                    "connection",
                    "proxy-connection",
                    "trailer",
                    "upgrade",
                    "te",
                    "proxy-authorization"
                ]
                .iter()
                .any(|reserved| key.eq_ignore_ascii_case(reserved)),
                "SXCU framing, Host and proxy header overrides are forbidden"
            );
            header_bytes = header_bytes.saturating_add(key.len() + value.len());
            ensure!(header_bytes <= 65536, "SXCU headers exceed 64 KiB");
            let key = reqwest::header::HeaderName::from_bytes(key.as_bytes())
                .map_err(|_| anyhow::anyhow!("Invalid SXCU request header name"))?;
            let mut value = reqwest::header::HeaderValue::from_str(value)
                .map_err(|_| anyhow::anyhow!("Invalid SXCU request header value"))?;
            if sensitive(key.as_str()) {
                value.set_sensitive(true);
                let raw = String::from_utf8_lossy(value.as_bytes());
                add_auth_secret(&mut secrets, key.as_str(), &raw)?;
            }
            headers.append(key, value);
        }
        let mut argument_bytes = 0usize;
        for (key, value) in &request.arguments.0 {
            argument_bytes = argument_bytes.saturating_add(key.len() + value.len());
            ensure!(
                !key.is_empty()
                    && key.len() <= 128
                    && argument_bytes <= 65536
                    && !key.chars().any(char::is_control),
                "Invalid or oversized SXCU form arguments"
            );
        }
        ensure!(
            request.events.len() <= 16 && request.secrets.len() <= 128,
            "Private SXCU output event/secret bound exceeded"
        );
        cancel.check()?;
        if let Some(credential) = &self.0.credential {
            let authentication = credential.authentication().unwrap_or_default();
            let name = authentication.header_name();
            ensure!(
                headers.get_all(name).iter().all(|value| value.is_empty()),
                "SXCU literal and native authentication headers conflict"
            );
            headers.remove(name);
            let target = credential
                .credential_target()
                .context("Frozen SXCU credential target is missing")?;
            #[cfg(any(target_os = "linux", target_os = "macos"))]
            let value = crate::upload_credentials::read_header_with_check(
                &target,
                authentication,
                &|| {
                    if cancel.is_canceled() {
                        Err(crate::CaptureError::NativeCanceled)
                    } else {
                        Ok(())
                    }
                },
            )?;
            #[cfg(not(any(target_os = "linux", target_os = "macos")))]
            let value = crate::upload_credentials::read_header(&target, authentication)?;
            cancel.check()?;
            add_secret(
                &mut secrets,
                std::str::from_utf8(value.as_bytes())
                    .map_err(|_| anyhow::anyhow!("Invalid native SXCU authentication header"))?,
            );
            headers.insert(name, value);
        }
        if (request.body == "Binary" || request.attach_file) && payload.is_none() {
            payload = Some(Payload::Bytes(input.as_bytes().to_vec()));
        }
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .max_blocking_threads(1)
            .build()?;
        let response=runtime.block_on(async{
            tokio::select!{biased;
                _=cancel.wait()=>anyhow::bail!("SXCU upload canceled; a remote object may remain and no retry was attempted"),
                value=tokio::time::timeout(Duration::from_secs(60),http(&request,url,headers,payload,data,&name,source_guard,cancel.clone()))=>value.context("SXCU request exceeded sixty seconds; a remote object may remain and no retry was attempted")?,
            }
        }).context("SXCU transfer could not be confirmed; a remote object may remain and no retry was attempted")?;
        drop(runtime);
        cancel.check().context(
            "SXCU transfer finished; a remote object may remain and no retry was attempted",
        )?;
        for (name, values) in &response.headers {
            if sensitive(name) {
                for value in values {
                    add_auth_secret(&mut secrets, name, value).context("SXCU response authentication validation failed; a remote object may remain and no retry was attempted")?;
                }
            }
        }
        let frame = serde_json::json!({"status":response.status,"url":response.url,"content_type":response.content_type,"headers":response.headers,"bytes":base64::engine::general_purpose::STANDARD.encode(&response.bytes)});
        let interpreted:Response=self.interpret("response",&name,&input,Some(frame),&cancel).context("SXCU response interpretation failed; the server may already have received the upload and no retry was attempted")?;
        let outcome=(||->Result<ShareXOutcome>{
        ensure!(
            interpreted.status == response.status
                && interpreted.success == (200..=299).contains(&response.status)
                && interpreted.events.len() + request.events.len() <= 16
                && interpreted.secrets.len() <= 128,
            "Invalid private SXCU response result"
        );
        for value in &interpreted.secrets {
            add_secret(&mut secrets, value);
        }
        let secrets = SecretMask::new(&secrets).context(
            "SXCU output redaction failed; the server may already have received the upload",
        )?;
        let events = request
            .events
            .iter()
            .chain(&interpreted.events)
            .map(|event| ShareXEvent {
                title: secrets.mask(&event.title, 256),
                text: secrets.mask(&event.text, 4096),
            })
            .collect();
        let url = if kind == ShareXInputKind::UrlSharing && !self.0.metadata.has_url {
            None
        } else {
            link(&interpreted.url, &secrets)?
        };
        let outcome = ShareXOutcome {
            success: interpreted.success,
            status: response.status,
            url,
            thumbnail: link(&interpreted.thumbnail, &secrets)?,
            deletion: link(&interpreted.deletion, &secrets)?,
            error_message: (!interpreted.error_message.is_empty())
                .then(|| secrets.mask(&interpreted.error_message, 4096)),
            events,
        };
        Ok(outcome)
        })().context("SXCU response/output validation failed; the server may already have received the upload and no retry was attempted")?;
        cancel.check().context(
            "SXCU transfer finished; a remote object may remain and no retry was attempted",
        )?;
        Ok(outcome)
    }
}
fn add_auth_secret(secrets: &mut Vec<Zeroizing<String>>, name: &str, value: &str) -> Result<()> {
    add_secret(secrets, value);
    if name.eq_ignore_ascii_case("cookie") || name.eq_ignore_ascii_case("set-cookie") {
        for (index, pair) in value.split(';').enumerate() {
            if name.eq_ignore_ascii_case("set-cookie") && index > 0 {
                break;
            }
            ensure!(
                index < 128,
                "SXCU Cookie header exceeds its bounded pair count"
            );
            if let Some((key, value)) = pair.split_once('=')
                && !key.trim().is_empty()
            {
                let value = value.trim();
                add_secret(secrets, value);
                if let Some(value) = value
                    .strip_prefix('"')
                    .and_then(|value| value.strip_suffix('"'))
                {
                    add_secret(secrets, value);
                }
            }
        }
    }
    Ok(())
}
fn add_secret(secrets: &mut Vec<Zeroizing<String>>, value: &str) {
    if !value.is_empty() && !secrets.iter().any(|old| old.as_str() == value) {
        secrets.push(Zeroizing::new(value.to_owned()));
    }
    if let Some((scheme, token)) = value.split_once(' ')
        && matches!(scheme.to_ascii_lowercase().as_str(), "bearer" | "basic")
        && !token.is_empty()
        && !secrets.iter().any(|old| old.as_str() == token)
    {
        secrets.push(Zeroizing::new(token.to_owned()));
    }
}
struct HttpResponse {
    status: u16,
    url: String,
    content_type: String,
    headers: BTreeMap<String, Vec<String>>,
    bytes: Zeroizing<Vec<u8>>,
}
// Keep the admitted request and retained source proof explicit without another wrapper.
#[allow(clippy::too_many_arguments)]
async fn http(
    request: &Request,
    url: reqwest::Url,
    headers: reqwest::header::HeaderMap,
    mut payload: Option<Payload>,
    data: Zeroizing<Vec<u8>>,
    filename: &str,
    source_guard: Option<UploadSourceGuard>,
    cancel: crate::UploadCancellation,
) -> Result<HttpResponse> {
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
        .tls_sslkeylogfile(false)
        .build()
        .map_err(|_| anyhow::anyhow!("Could not initialize SXCU HTTPS transport"))?;
    let method = reqwest::Method::from_bytes(request.method.as_bytes())
        .map_err(|_| anyhow::anyhow!("Invalid admitted SXCU HTTP method"))?;
    let mut builder = client.request(method, url);
    let transferred = Arc::new(std::sync::atomic::AtomicU64::new(0));
    let mut expected = None;
    let mut argument_counts = Vec::with_capacity(request.arguments.0.len());
    match request.body.as_str() {
        "None" => {}
        "MultipartFormData" => {
            let mut form = reqwest::multipart::Form::new();
            for (key, value) in &request.arguments.0 {
                let length = value.len() as u64;
                let consumed = Arc::new(std::sync::atomic::AtomicU64::new(0));
                let body = Payload::Bytes(value.as_bytes().to_vec())
                    .body(consumed.clone(), cancel.clone());
                form = form.part(
                    key.clone(),
                    reqwest::multipart::Part::stream_with_length(body, length),
                );
                argument_counts.push((consumed, length));
            }
            if request.attach_file {
                let payload = payload
                    .take()
                    .context("SXCU file attachment has no admitted source")?;
                let length = payload.length();
                expected = Some(length);
                let part = reqwest::multipart::Part::stream_with_length(
                    payload.body(transferred.clone(), cancel.clone()),
                    length,
                )
                .file_name(filename.to_owned())
                .mime_str(media(filename))?;
                form = form.part(request.file_field.clone(), part);
            }
            builder = builder.multipart(form);
        }
        "FormURLEncoded" => {
            expected = Some(data.len() as u64);
            builder = builder
                .header(
                    reqwest::header::CONTENT_TYPE,
                    "application/x-www-form-urlencoded",
                )
                .header(reqwest::header::CONTENT_LENGTH, data.len())
                .body(Payload::Bytes(data.to_vec()).body(transferred.clone(), cancel.clone()))
        }
        "JSON" | "XML" => {
            expected = Some(data.len() as u64);
            builder = builder
                .header(
                    reqwest::header::CONTENT_TYPE,
                    if request.body == "JSON" {
                        "application/json"
                    } else {
                        "application/xml"
                    },
                )
                .header(reqwest::header::CONTENT_LENGTH, data.len())
                .body(Payload::Bytes(data.to_vec()).body(transferred.clone(), cancel.clone()))
        }
        "Binary" => {
            let payload = payload
                .take()
                .context("SXCU binary body has no admitted source")?;
            let length = payload.length();
            expected = Some(length);
            builder = builder
                .header(reqwest::header::CONTENT_TYPE, media(filename))
                .header(reqwest::header::CONTENT_LENGTH, length)
                .body(payload.body(transferred.clone(), cancel.clone()));
        }
        _ => anyhow::bail!("Invalid admitted SXCU body mode"),
    }
    let mut response=builder.headers(headers).header(reqwest::header::ACCEPT_ENCODING,"identity").send().await.map_err(|_|anyhow::anyhow!("SXCU request failed; the server may have received the upload and no retry was attempted"))?;
    ensure!(
        response
            .content_length()
            .is_none_or(|length| length <= 1024 * 1024),
        "SXCU response exceeds one MiB; a remote object may remain"
    );
    let status = response.status().as_u16();
    let url = response.url().as_str().to_owned();
    let content_type = response
        .headers()
        .get(reqwest::header::CONTENT_TYPE)
        .and_then(|value| value.to_str().ok())
        .unwrap_or_default()
        .to_owned();
    let mut captured: BTreeMap<String, Vec<String>> = BTreeMap::new();
    let mut header_bytes = 0usize;
    let mut count = 0usize;
    for (name, value) in response.headers() {
        count += 1;
        header_bytes = header_bytes.saturating_add(name.as_str().len() + value.as_bytes().len());
        ensure!(
            count <= 128 && header_bytes <= 65536 && value.as_bytes().len() <= 8192,
            "SXCU response headers exceed their bounds; a remote object may remain and no retry was attempted"
        );
        captured.entry(name.as_str().to_owned()).or_default().push(
            value
                .as_bytes()
                .iter()
                .map(|byte| char::from(*byte))
                .collect(),
        );
    }
    let mut bytes = Zeroizing::new(Vec::new());
    while let Some(chunk) = response
        .chunk()
        .await
        .map_err(|_| anyhow::anyhow!("SXCU response read failed; a remote object may remain"))?
    {
        ensure!(
            bytes.len().saturating_add(chunk.len()) <= 1024 * 1024,
            "SXCU response exceeds one MiB; a remote object may remain"
        );
        bytes.extend_from_slice(&chunk);
    }
    if (200..=299).contains(&status) {
        let expected_arguments: u64 = argument_counts.iter().map(|(_, length)| *length).sum();
        let consumed_arguments: u64 = argument_counts
            .iter()
            .map(|(count, _)| count.load(Ordering::Acquire))
            .sum();
        ensure!(
            consumed_arguments == expected_arguments,
            "SXCU multipart arguments were not fully consumed; a remote object may remain and no retry was attempted"
        );
        verify_transfer(source_guard, transferred, expected.unwrap_or(0), &cancel).await?;
    }
    Ok(HttpResponse {
        status,
        url,
        content_type,
        headers: captured,
        bytes,
    })
}

#[derive(Clone)]
pub enum ClassifiedUploadProfile {
    Native(Box<crate::UploadDestination>),
    ShareX(ShareXPrepared),
}
/// Read once and dispatch the frozen bytes; no second file read changes the selected semantics.
pub fn classify_upload_profile(
    path: &Path,
    kind: ShareXInputKind,
    options: Option<&Path>,
) -> Result<ClassifiedUploadProfile> {
    let bytes = read(path, 16384, "Upload profile")?;
    let expects_sharex = path
        .extension()
        .is_some_and(|extension| extension.eq_ignore_ascii_case("sxcu"));
    classify_snapshot_format(bytes, kind, options, Some(expects_sharex))
}
#[cfg(test)]
fn classify_snapshot(
    bytes: Zeroizing<Vec<u8>>,
    kind: ShareXInputKind,
    options: Option<&Path>,
) -> Result<ClassifiedUploadProfile> {
    classify_snapshot_format(bytes, kind, options, None)
}
fn classify_snapshot_format(
    bytes: Zeroizing<Vec<u8>>,
    kind: ShareXInputKind,
    options: Option<&Path>,
    expects_sharex: Option<bool>,
) -> Result<ClassifiedUploadProfile> {
    let value = json(&bytes)?;
    let object = value
        .0
        .as_object()
        .context("Upload profile must be an object")?;
    let sxcu = object.keys().any(|key| {
        key.eq_ignore_ascii_case("RequestURL") || key.eq_ignore_ascii_case("DestinationType")
    });
    if expects_sharex == Some(true) {
        ensure!(
            sxcu,
            "Invalid ShareX profile: the .sxcu extension requires published SXCU fields"
        );
    }
    if expects_sharex == Some(false) {
        ensure!(
            options.is_none(),
            "Full SXCU options cannot be applied to a native upload profile"
        );
        return Ok(ClassifiedUploadProfile::Native(Box::new(
            crate::UploadDestination::from_json_snapshot(&bytes)?,
        )));
    }
    if !sxcu {
        ensure!(
            options.is_none(),
            "Full SXCU options cannot be applied to a native upload profile"
        );
        return Ok(ClassifiedUploadProfile::Native(Box::new(
            crate::UploadDestination::from_json_snapshot(&bytes)?,
        )));
    }
    let legacy = object
        .iter()
        .find(|(key, _)| key.eq_ignore_ascii_case("Data"))
        .is_some_and(|(_, value)| value.is_object())
        || object
            .iter()
            .find(|(key, _)| key.eq_ignore_ascii_case("Version"))
            .is_none_or(|(_, value)| value.as_str().is_none_or(str::is_empty));
    if legacy {
        ensure!(
            options.is_none() && kind.is_file(),
            "Legacy SXCU-like profiles support only their original image/file importer, without full execution options"
        );
        return Ok(ClassifiedUploadProfile::Native(Box::new(
            crate::UploadDestination::from_sharex_snapshot(&bytes, kind == ShareXInputKind::File)?
                .0,
        )));
    }
    drop(value);
    Ok(ClassifiedUploadProfile::ShareX(
        ShareXPrepared::from_snapshot(bytes, Some(kind), options)?,
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;
    struct Fixture(PathBuf);
    impl Fixture {
        fn new() -> Self {
            let path = std::env::temp_dir()
                .join(format!("screenshots-sxcu-unit-{}", uuid::Uuid::new_v4()));
            std::fs::create_dir(&path).unwrap();
            Self(path)
        }
        fn path(&self, name: &str) -> PathBuf {
            self.0.join(name)
        }
    }
    impl Drop for Fixture {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }
    fn profile() -> Value {
        serde_json::json!({"Version":"17.0.0","DestinationType":"ImageUploader,TextUploader,FileUploader,URLShortener,URLSharingService","RequestURL":"https://example.invalid/upload","RequestMethod":"POST","Body":"MultipartFormData","FileFormName":"file","URL":"{json:url}"})
    }
    fn metadata(value: &Value) -> Result<Metadata> {
        Metadata::read(value)
    }
    #[test]
    fn official_fields_flags_methods_and_body_input_matrix_are_complete() {
        let kinds = [
            ShareXInputKind::Image,
            ShareXInputKind::File,
            ShareXInputKind::Text,
            ShareXInputKind::Url,
            ShareXInputKind::UrlSharing,
        ];
        for (method_index, method) in ["GET", "POST", "PUT", "PATCH", "DELETE"].iter().enumerate() {
            for value in [(*method).into(), method_index.into()] {
                let mut p = profile();
                p["RequestMethod"] = value;
                assert_eq!(metadata(&p).unwrap().method, *method);
            }
        }
        for (body_index, body) in [
            "None",
            "MultipartFormData",
            "FormURLEncoded",
            "JSON",
            "XML",
            "Binary",
        ]
        .iter()
        .enumerate()
        {
            for value in [(*body).into(), body_index.into()] {
                let mut p = profile();
                p["Body"] = value;
                let m = metadata(&p).unwrap();
                for kind in kinds {
                    let allowed = if kind.is_file() {
                        matches!(*body, "MultipartFormData" | "Binary")
                    } else if matches!(kind, ShareXInputKind::Url | ShareXInputKind::UrlSharing) {
                        *body != "Binary"
                    } else {
                        true
                    };
                    assert_eq!(m.admit(kind).is_ok(), allowed, "{} {body}", kind.name());
                }
            }
        }
        let mut p = profile();
        p["DestinationType"] = 31.into();
        assert_eq!(metadata(&p).unwrap().destinations, 31);
        p["DestinationType"] = 32.into();
        assert!(metadata(&p).is_err());
        p["DestinationType"] = "UnknownUploader".into();
        assert!(metadata(&p).is_err());
    }
    #[test]
    fn duplicate_unknown_deep_legacy_and_version_admission_does_not_echo_secrets() {
        assert!(json(br#"{"Version":"17.0.0","Version":"MUST_NOT_ECHO"}"#).is_err());
        let deep = format!("{}0{}", "[".repeat(65), "]".repeat(65));
        assert!(json(deep.as_bytes()).is_err());
        for (key, bad) in [
            ("Version", "".into()),
            ("Version", "12.3.1".into()),
            ("Version", "12.3.1.0".into()),
            ("Version", "MUST_NOT_ECHO".into()),
            ("Unknown", "MUST_NOT_ECHO".into()),
            ("Data", serde_json::json!({"image":"{base64}"})),
            ("RequestMethod", "CONNECT".into()),
            ("version", "17.0.0".into()),
        ] {
            let mut p = profile();
            p[key] = bad;
            let error = metadata(&p).err().unwrap();
            assert!(!format!("{error:#}").contains("MUST_NOT_ECHO"), "{key}");
        }
        let mut p = profile();
        p["Version"] = "12.3.2".into();
        assert!(metadata(&p).is_ok());
        p["Headers"] = serde_json::json!({"x-test":"a","X-Test":"b"});
        assert!(metadata(&p).is_ok());
        p["FileFormName"] = "bad\r\nfield".into();
        assert!(metadata(&p).unwrap().admit(ShareXInputKind::Image).is_err());
    }
    #[test]
    fn raw_response_link_fallback_and_url_sharing_optional_links_are_distinct() {
        for kind in [
            ShareXInputKind::Image,
            ShareXInputKind::File,
            ShareXInputKind::Text,
            ShareXInputKind::Url,
        ] {
            assert!(may_return_link(Some(kind), false));
            assert!(may_return_link(Some(kind), true));
        }
        assert!(!may_return_link(Some(ShareXInputKind::UrlSharing), false));
        assert!(may_return_link(Some(ShareXInputKind::UrlSharing), true));
    }
    #[test]
    fn literal_authentication_is_bound_to_the_static_https_origin() {
        let mut p = profile();
        p["Headers"] = serde_json::json!({"Authorization":"Bearer synthetic-only"});
        let m = metadata(&p).unwrap();
        assert!(m.literal_auth);
        assert_eq!(
            m.original_origin.as_deref(),
            Some("https://example.invalid")
        );
        p["RequestURL"] = "HTTPS://EXAMPLE.INVALID/{filename}".into();
        assert_eq!(
            metadata(&p).unwrap().original_origin.as_deref(),
            Some("https://example.invalid")
        );
        p["RequestURL"] = "https://{input}/upload".into();
        assert!(metadata(&p).unwrap().original_origin.is_none());
        p["RequestURL"] = "http://example.invalid/upload".into();
        assert!(metadata(&p).is_err());
        p["Headers"] = serde_json::json!({});
        p["RequestURL"] = "https://example.invalid/upload?api_key=synthetic-only".into();
        assert!(metadata(&p).unwrap().literal_auth);
    }
    #[test]
    fn canonical_native_auth_names_and_obvious_variants_fail_closed_on_http() {
        for name in crate::upload::RESERVED_QUERY_NAMES.iter().copied().chain([
            "X-Auth",
            "xAuth",
            "access_key",
            "AccessKey",
            "access_key_id",
            "aws_access_key_id",
            "secret_access_key",
        ]) {
            assert!(sensitive(name), "{name}");
        }
        for name in ["author", "monkey", "keyboard", "filename", "index"] {
            assert!(!sensitive(name), "{name}");
        }
        for key in ["auth", "key", "access_key", "access_key_id"] {
            let mut value = profile();
            value["RequestURL"] =
                format!("http://127.0.0.1:1/upload?{key}=SYNTHETIC_PRIVATE").into();
            let error = metadata(&value).err().unwrap();
            assert!(error.to_string().contains("requires HTTPS"));
            assert!(!error.to_string().contains("SYNTHETIC_PRIVATE"));
            value["RequestURL"] = "http://127.0.0.1:1/upload".into();
            value["Parameters"] = serde_json::json!({key:"SYNTHETIC_PRIVATE"});
            assert!(metadata(&value).is_err());
        }
        let mut value = profile();
        value["RequestURL"] = "http://127.0.0.1:1/upload".into();
        value["Headers"] = serde_json::json!({"X-Auth":"SYNTHETIC_PRIVATE"});
        assert!(metadata(&value).is_err());
    }
    #[test]
    fn delivered_query_names_are_projected_without_changing_literal_wire_keys() {
        for key in [
            "a%75th",
            "%61uth",
            "k%65y",
            "access%5fkey",
            "%61ccess%5Fkey%5Fid",
        ] {
            let mut value = profile();
            value["RequestURL"] = "http://127.0.0.1:1/upload".into();
            value["Parameters"] = serde_json::json!({key:"SYNTHETIC_PRIVATE"});
            assert!(metadata(&value).is_err(), "{key}");
        }
        let mut value = profile();
        value["RequestURL"] = "http://127.0.0.1:1/upload?a%2575th=SYNTHETIC_PRIVATE".into();
        assert!(metadata(&value).is_err());
        assert_eq!(
            delivered_query_name("public%2Bname").unwrap().as_str(),
            "public+name"
        );
        assert_eq!(
            delivered_query_name("public+name").unwrap().as_str(),
            "public name"
        );
        assert!(delivered_query_name("invalid%FF").is_err());
        let url = reqwest::Url::parse("https://example.invalid/upload?a%75th=synthetic%2Bprivate")
            .unwrap();
        assert_eq!(
            query_credentials(&url).unwrap()[0].as_str(),
            "synthetic+private"
        );
    }
    #[test]
    fn failed_json_xml_sniffing_cannot_hide_later_raw_form_credentials() {
        for prefix in ["{caption}", "[caption]", "<caption>", "{\"public\":true}"] {
            let text = format!("{prefix}=public&password=opaque%2Bprefix&Cookie=sid%3Dbare-cookie");
            let values = body_credentials(&text).unwrap();
            let mask = SecretMask::new(&values).unwrap();
            assert_eq!(
                mask.mask("opaque+prefix bare-cookie", 4096),
                "[redacted] [redacted]",
                "{prefix}"
            );
        }
        assert!(
            body_credentials(r#"{"caption":"public &password=not-auth"}"#)
                .unwrap()
                .is_empty()
        );
        assert!(
            body_credentials("<r><caption>public &amp;password=not-auth</caption></r>")
                .unwrap()
                .is_empty()
        );
    }

    #[test]
    fn known_secret_encoding_projections_are_redacted_with_original_utf8_spans() {
        let plus = SecretMask::new(&[Zeroizing::new("synthetic+private".into())]).unwrap();
        assert_eq!(
            plus.mask(
                "synthetic%2bprivate %73ynthetic%2Bprivate synthetic+private",
                4096
            ),
            "[redacted] [redacted] [redacted]"
        );
        assert!(link("https://example.invalid/x?key=synthetic%2bprivate", &plus).is_err());
        assert!(link("https://example.invalid/x?key=synthetic+private", &plus).is_err());
        assert!(link("https://example.invalid/synthetic+private", &plus).is_err());
        let space = SecretMask::new(&[Zeroizing::new("a b".into())]).unwrap();
        assert!(link("https://example.invalid/x?v=a+b", &space).is_err());
        assert!(link("https://example.invalid/a+b", &space).is_ok());
        assert!(link("https://example.invalid/a%20b", &space).is_err());
        assert_eq!(space.mask("a+b a%20b", 4096), "[redacted] [redacted]");
        let utf8 = SecretMask::new(&[Zeroizing::new("p%q&=/é".into())]).unwrap();
        assert_eq!(
            utf8.mask("préfixe%3a%20p%25q%26%3d%2f%c3%a9 suffix", 4096),
            "préfixe%3a%20[redacted] suffix"
        );
        assert!(link("https://example.invalid/x?v=p%25q%26%3D%2F%C3%A9", &utf8).is_err());
        assert_eq!(
            utf8.mask("%FF p%25q%26%3D%2F%C3%A9", 4096),
            "%FF [redacted]"
        );
        assert!(
            link(
                "https://example.invalid/share?token=unrecognized-public-token",
                &utf8
            )
            .is_ok()
        );
    }
    #[test]
    fn links_reject_raw_known_plus_and_literal_percent_as_well_as_decoded_forms() {
        for value in ["synthetic+private", "private%2Fcredential"] {
            let mask = SecretMask::new(&[Zeroizing::new(value.into())]).unwrap();
            assert!(link(&format!("https://example.invalid/x?key={value}"), &mask).is_err());
            assert!(link(&format!("https://example.invalid/{value}"), &mask).is_err());
        }
        let mask = SecretMask::new(&[Zeroizing::new("private%2Fcredential".into())]).unwrap();
        assert!(
            link(
                "https://example.invalid/x?key=private%252Fcredential",
                &mask
            )
            .is_err()
        );
        assert!(
            link(
                "https://example.invalid/share?token=unrelated-public-value",
                &mask
            )
            .is_ok()
        );
    }
    #[test]
    fn configured_body_authentication_covers_json_xml_attributes_and_form_without_mutating_data() {
        for (body, value, secret) in [
            (
                "XML",
                "<r><password>opaque-xml</password><caption>public</caption></r>",
                "opaque-xml",
            ),
            (
                "XML",
                "<r password='opaque-attribute'><caption>public</caption></r>",
                "opaque-attribute",
            ),
            (
                "JSON",
                r#"{"credentials":{"password":"opaque-json"},"caption":"public"}"#,
                "opaque-json",
            ),
            (
                "JSON",
                "pa%73sword=opaque%2Bform&caption=public",
                "opaque+form",
            ),
        ] {
            let values = body_credentials(value).unwrap();
            assert!(values.iter().any(|value| value.as_str() == secret));
            let mut p = profile();
            p["Body"] = body.into();
            p["DestinationType"] = "TextUploader".into();
            p["Data"] = value.into();
            p["RequestURL"] = "http://127.0.0.1:1/upload".into();
            assert!(metadata(&p).is_err());
            p["RequestURL"] = "{inputbox:Endpoint}".into();
            let m = metadata(&p).unwrap();
            assert!(m.literal_auth && m.original_origin.is_none());
            assert_eq!(p["Data"], value);
        }
        assert!(
            body_credentials(r#"{"Cookie":"sid=opaque-body-cookie=="}"#)
                .unwrap()
                .iter()
                .any(|value| value.as_str() == "opaque-body-cookie==")
        );
        let duplicates =
            body_credentials(r#"{"password":"first-private","password":"second-private"}"#)
                .unwrap();
        assert!(
            duplicates
                .iter()
                .any(|value| value.as_str() == "first-private")
                && duplicates
                    .iter()
                    .any(|value| value.as_str() == "second-private")
        );
        assert!(
            body_credentials(r#"{"caption":"password=public"}"#)
                .unwrap()
                .is_empty()
        );
        assert!(
            body_credentials("<r caption='public'><note>password=public</note></r>")
                .unwrap()
                .is_empty()
        );
        let mut ignored = profile();
        ignored["Body"] = "FormURLEncoded".into();
        ignored["Data"] = "password=unused".into();
        ignored["DestinationType"] = "TextUploader".into();
        ignored["RequestURL"] = "http://127.0.0.1:1/upload".into();
        assert!(!metadata(&ignored).unwrap().literal_auth);
        assert!(
            body_credentials("<!DOCTYPE r [<!ENTITY x SYSTEM 'file:///must-not-read'>]><r>&x;</r>")
                .is_err()
        );
    }
    #[test]
    fn numeric_body_authentication_masks_raw_and_canonical_tokens_within_one_bound() {
        for (raw, canonical) in [
            ("1e5", "100000"),
            ("1.25E+4", "12500"),
            ("-2.5e-3", "-0.0025"),
            ("1.000", "1"),
            ("-0", "-0"),
        ] {
            let body = format!("{{\"credentials\":{{\"values\":[{raw}]}}}}");
            let values = body_credentials(&body).unwrap();
            let mask = SecretMask::new(&values).unwrap();
            assert_eq!(mask.mask(raw, 4096), "[redacted]", "{raw}");
            assert_eq!(mask.mask(canonical, 4096), "[redacted]", "{canonical}");
            assert!(link(&format!("https://example.invalid/share?v={raw}"), &mask).is_err());
        }
        assert!(body_credentials(r#"{"caption":[1e5]}"#).unwrap().is_empty());
        let strings = body_credentials(r#"{"password":"100000","caption":1e5}"#).unwrap();
        assert!(!strings.iter().any(|value| value.as_str() == "1e5"));
        let numbers = (0..65)
            .map(|value| format!("{value}e0"))
            .collect::<Vec<_>>()
            .join(",");
        assert!(body_credentials(&format!("{{\"password\":[{numbers}]}}")).is_err());
    }
    #[test]
    fn numeric_authentication_projects_only_its_actual_sensitive_tokens() {
        let public = (1..=127)
            .map(|count| format!("100000.{}", "0".repeat(count)))
            .collect::<Vec<_>>()
            .join(",");
        let body = format!("{{\"password\":1e5,\"caption\":[{public}]}}");
        let values = body_credentials(&body).unwrap();
        assert_eq!(values.len(), 2);
        assert!(values.iter().any(|value| value.as_str() == "1e5"));
        assert!(values.iter().any(|value| value.as_str() == "100000"));
        assert!(!values.iter().any(|value| value.as_str() == "100000.00"));
        let mixed = body_credentials(r#"{"caption":100000.00,"password":"first","nested":{"id":20000.0},"password":1e5,"password":10e4,"credentials":[2e4]}"#).unwrap();
        for raw in ["1e5", "10e4", "2e4"] {
            assert!(mixed.iter().any(|value| value.as_str() == raw), "{raw}");
        }
        for public in ["100000.00", "20000.0"] {
            assert!(
                !mixed.iter().any(|value| value.as_str() == public),
                "{public}"
            );
        }
    }
    #[test]
    fn body_cookie_kind_survives_containers_and_explicit_descendants_override_it() {
        for (body, expected, ignored) in [
            (
                r#"{"Cookie":{"nested":{"value":"sid=opaque-cookie-private=="}},"caption":"sid=public-outside=="}"#,
                vec!["opaque-cookie-private=="],
                vec!["public-outside=="],
            ),
            (
                r#"{"Cookie":{"nested":[{"value":"sid=\"opaque+quoted==\"; second=secondary-private=="}]}}"#,
                vec!["opaque+quoted==", "secondary-private=="],
                vec![],
            ),
            (
                r#"{"Set-Cookie":{"nested":[{"value":"sid=\"opaque-set-cookie==\"; Path=/; SameSite=None"}]}}"#,
                vec!["opaque-set-cookie=="],
                vec!["/", "None"],
            ),
            (
                r#"{"Cookie":{"nested":{"Set-Cookie":{"value":"first=explicit-set==; next=public-attribute"},"sibling":{"value":"sid=parent-cookie==; second=parent-second=="}}}}"#,
                vec!["explicit-set==", "parent-cookie==", "parent-second=="],
                vec!["public-attribute"],
            ),
            (
                r#"{"Set-Cookie":{"nested":{"Cookie":[{"value":"first=explicit-cookie==; next=explicit-second=="}],"sibling":{"value":"sid=parent-set==; extra=public-set-attribute"}}}}"#,
                vec!["explicit-cookie==", "explicit-second==", "parent-set=="],
                vec!["public-set-attribute"],
            ),
            (
                r#"{"Authorization":{"nested":[{"value":"Bearer nested-bearer=="}]},"credentials":{"value":"Basic dXNlcjpwdw=="},"caption":"Bearer public-outside=="}"#,
                vec!["nested-bearer==", "dXNlcjpwdw=="],
                vec!["public-outside=="],
            ),
        ] {
            let values = body_credentials(body).unwrap();
            for value in expected {
                assert!(
                    values.iter().any(|secret| secret.as_str() == value),
                    "{value}"
                );
            }
            for value in ignored {
                assert!(
                    !values.iter().any(|secret| secret.as_str() == value),
                    "{value}"
                );
            }
        }
    }
    #[test]
    fn xml_cookie_kind_covers_attributes_text_overrides_and_siblings_without_joining_values() {
        for (body, expected, ignored) in [
            (
                r#"<Cookie><nested value="sid=opaque-xml-cookie-private=="/></Cookie>"#,
                vec!["opaque-xml-cookie-private=="],
                vec![],
            ),
            (
                r#"<Cookie><first>sid=xml-first==</first><second>other=xml-second==</second></Cookie>"#,
                vec!["xml-first==", "xml-second=="],
                vec![
                    "xml-first==other=xml-second==",
                    "sid=xml-first==other=xml-second==",
                ],
            ),
            (
                r#"<Set-Cookie><nested value='sid="xml-quoted=="; Path=/; SameSite=None'/></Set-Cookie>"#,
                vec!["xml-quoted=="],
                vec!["/", "None"],
            ),
            (
                r#"<Cookie><Set-Cookie><nested value="sid=xml-explicit-set==; next=public-attribute"/></Set-Cookie><sibling value="sid=xml-parent-cookie==; second=xml-parent-second=="/></Cookie>"#,
                vec![
                    "xml-explicit-set==",
                    "xml-parent-cookie==",
                    "xml-parent-second==",
                ],
                vec!["public-attribute"],
            ),
            (
                r#"<Set-Cookie><Cookie><nested>sid=xml-explicit-cookie==; second=xml-explicit-second==</nested></Cookie><sibling value="sid=xml-parent-set==; next=public-attribute"/></Set-Cookie>"#,
                vec![
                    "xml-explicit-cookie==",
                    "xml-explicit-second==",
                    "xml-parent-set==",
                ],
                vec!["public-attribute"],
            ),
            (
                r#"<r><nested Cookie="sid=xml-attribute=="/><caption value="sid=public-outside=="/></r>"#,
                vec!["xml-attribute=="],
                vec!["public-outside=="],
            ),
            (
                r#"<Cookie><nested Set-Cookie="sid=xml-explicit-attribute==; next=public-attribute" value="sid=xml-sibling-cookie==; second=xml-sibling-second=="/></Cookie>"#,
                vec![
                    "xml-explicit-attribute==",
                    "xml-sibling-cookie==",
                    "xml-sibling-second==",
                ],
                vec!["public-attribute"],
            ),
            (
                r#"<Set-Cookie><nested Cookie="sid=xml-explicit-attr-cookie==; next=xml-explicit-attr-second==" value="sid=xml-sibling-set==; next=public-attribute"/></Set-Cookie>"#,
                vec![
                    "xml-explicit-attr-cookie==",
                    "xml-explicit-attr-second==",
                    "xml-sibling-set==",
                ],
                vec!["public-attribute"],
            ),
            (
                r#"<r><credentials><Authorization><value>Bearer xml-bearer==</value></Authorization><nested value="Basic eG1sOnB3=="/></credentials><caption value="Bearer public-outside=="/></r>"#,
                vec!["xml-bearer==", "eG1sOnB3=="],
                vec!["public-outside=="],
            ),
        ] {
            let values = body_credentials(body).unwrap();
            let mask = SecretMask::new(&values).unwrap();
            for value in expected {
                assert!(
                    values.iter().any(|secret| secret.as_str() == value),
                    "{value}"
                );
                assert_eq!(mask.mask(value, 4096), "[redacted]");
                assert!(link(&format!("https://example.invalid/share?v={value}"), &mask).is_err());
            }
            for value in ignored {
                assert!(
                    !values.iter().any(|secret| secret.as_str() == value),
                    "{value}"
                );
            }
        }
    }
    #[test]
    fn repeated_numeric_and_cross_context_values_keep_unique_complete_masks() {
        let numbers = std::iter::repeat_n("1e5", 128)
            .collect::<Vec<_>>()
            .join(",");
        let values = body_credentials(&format!("{{\"password\":[{numbers}]}}")).unwrap();
        assert_eq!(values.len(), 2);
        for value in ["100000", "1e5"] {
            assert!(values.iter().any(|secret| secret.as_str() == value));
        }
        for body in [
            r#"{"credentials":"sid=context-first==; next=context-second==","Set-Cookie":{"nested":"sid=context-first==; next=context-second=="},"Cookie":{"nested":"sid=context-first==; next=context-second=="}}"#,
            r#"<r><credentials value="sid=context-first==; next=context-second=="/><Set-Cookie><nested value="sid=context-first==; next=context-second=="/></Set-Cookie><Cookie><nested value="sid=context-first==; next=context-second=="/></Cookie></r>"#,
        ] {
            let values = body_credentials(body).unwrap();
            for value in ["context-first==", "context-second=="] {
                assert!(values.iter().any(|secret| secret.as_str() == value));
            }
            let unique = values
                .iter()
                .map(|value| value.as_str())
                .collect::<HashSet<_>>();
            assert_eq!(values.len(), unique.len());
        }
    }
    #[test]
    fn outgoing_cookie_values_are_redacted_and_cannot_be_exposed_as_links() {
        let mut secrets = Vec::new();
        add_auth_secret(
            &mut secrets,
            "Cookie",
            "sid=secret==; theme=private-theme; quoted=\"a=b==\"",
        )
        .unwrap();
        let mask = SecretMask::new(&secrets).unwrap();
        assert_eq!(
            mask.mask("denied secret== private-theme a=b==", 4096),
            "denied [redacted] [redacted] [redacted]"
        );
        assert!(link("https://example.invalid/share?sid=secret==", &mask).is_err());
        assert!(link("https://example.invalid/share?v=a=b==", &mask).is_err());
        let mut response = Vec::new();
        add_auth_secret(
            &mut response,
            "Set-Cookie",
            "sid=another==; Path=/; SameSite=None",
        )
        .unwrap();
        assert!(response.iter().any(|value| value.as_str() == "another=="));
        assert!(
            !response
                .iter()
                .any(|value| matches!(value.as_str(), "/" | "None"))
        );
        assert!(
            add_auth_secret(
                &mut Vec::new(),
                "Cookie",
                &std::iter::repeat_n("a=b", 129)
                    .collect::<Vec<_>>()
                    .join("; ")
            )
            .is_err()
        );
    }
    #[test]
    fn options_are_bounded_and_cannot_select_a_runtime_or_secret_in_argv() {
        let fixture = Fixture::new();
        let path = fixture.path("options.json");
        std::fs::write(&path,br#"{"version":1,"answers":{"Headers.X-Token#0":"synthetic-answer"},"credential_origin":"https://example.invalid","filename":"public.png"}"#).unwrap();
        let options = Options::load(Some(&path)).unwrap();
        assert_eq!(options.answers["Headers.X-Token#0"], "synthetic-answer");
        for value in [
            serde_json::json!({"version":2}),
            serde_json::json!({"runtime":"/untrusted/program"}),
            serde_json::json!({"filename":"../private.png"}),
            serde_json::json!({"credential_origin":"https://example.invalid/path"}),
            serde_json::json!({"answers":{"a":"x".repeat(4097)}}),
            serde_json::json!({"answers":{"URL#0":"a"},"answer_indices":{"URL#0":0}}),
            serde_json::json!({"answer_indices":{"URL#0":16384}}),
            serde_json::json!({"function_names":{"URL#0":"execute"}}),
        ] {
            std::fs::write(&path, serde_json::to_vec(&value).unwrap()).unwrap();
            assert!(Options::load(Some(&path)).is_err());
        }
    }
    #[test]
    fn extension_selects_the_parser_on_the_already_read_snapshot() {
        let bytes =
            br#"{"url":"https://native.invalid/upload","method":"PUT","response":{"type":"text"}}"#;
        assert!(
            classify_snapshot_format(
                Zeroizing::new(bytes.to_vec()),
                ShareXInputKind::File,
                None,
                Some(true)
            )
            .err()
            .is_some_and(|error| error.to_string().contains("Invalid"))
        );
        assert!(matches!(
            classify_snapshot_format(
                Zeroizing::new(bytes.to_vec()),
                ShareXInputKind::File,
                None,
                Some(false)
            )
            .unwrap(),
            ClassifiedUploadProfile::Native(_)
        ));
        let sxcu = serde_json::json!({"RequestURL":"https://sharex.invalid/upload","DestinationType":"FileUploader","Body":"Binary","RequestMethod":"PUT","URL":"{response}"});
        assert!(
            classify_snapshot_format(
                Zeroizing::new(serde_json::to_vec(&sxcu).unwrap()),
                ShareXInputKind::File,
                None,
                Some(false)
            )
            .is_err()
        );
        assert!(matches!(
            classify_snapshot_format(
                Zeroizing::new(serde_json::to_vec(&sxcu).unwrap()),
                ShareXInputKind::File,
                None,
                Some(true)
            )
            .unwrap(),
            ClassifiedUploadProfile::Native(_)
        ));
    }
    #[test]
    fn native_and_legacy_dispatch_use_the_same_already_read_snapshot() {
        let fixture = Fixture::new();
        let path = fixture.path("profile.json");
        std::fs::write(&path,br#"{"url":"https://original.invalid/upload","method":"POST","field":"file","json_pointer":"/url"}"#).unwrap();
        let bytes = read(&path, 16384, "fixture").unwrap();
        std::fs::write(&path, b"changed after snapshot").unwrap();
        let ClassifiedUploadProfile::Native(native) =
            classify_snapshot(bytes, ShareXInputKind::File, None).unwrap()
        else {
            panic!("native profile changed type")
        };
        assert_eq!(native.origin(), "https://original.invalid");
        let legacy = serde_json::json!({"Version":"17.0.0","DestinationType":"ImageUploader","RequestURL":"https://legacy.invalid/upload","Body":"JSON","Data":{"image":"{base64}"},"URL":"{json:url}"});
        let ClassifiedUploadProfile::Native(native) = classify_snapshot(
            Zeroizing::new(serde_json::to_vec(&legacy).unwrap()),
            ShareXInputKind::Image,
            None,
        )
        .unwrap() else {
            panic!("legacy Data extension became official SXCU")
        };
        assert_eq!(native.origin(), "https://legacy.invalid");
    }
    #[test]
    fn literal_secret_redaction_never_reprocesses_its_own_placeholder() {
        let values = vec![
            Zeroizing::new("a".into()),
            Zeroizing::new("r".into()),
            Zeroizing::new("redacted".into()),
            Zeroizing::new("a.*(x)".into()),
        ];
        let secrets = SecretMask::new(&values).unwrap();
        assert_eq!(secrets.mask("a", 4096), "[redacted]");
        assert_eq!(secrets.mask("a.*(x)", 4096), "[redacted]");
        assert_eq!(SecretMask::new(&[]).unwrap().mask("ééé", 4), "éé");
        assert_eq!(secrets.mask("\x1b", 4096), " ");
        assert!(link("https://example.invalid/a", &secrets).is_err());
        assert!(link("", &secrets).unwrap().is_none());
    }
    #[test]
    fn source_extent_and_ordered_form_maps_are_preserved() {
        let fixture = Fixture::new();
        let path = fixture.path("input.bin");
        std::fs::write(&path, b"first").unwrap();
        let (file, length) = opened(&path, 64 * 1024 * 1024, "fixture").unwrap();
        std::fs::OpenOptions::new()
            .append(true)
            .open(&path)
            .unwrap()
            .write_all(b"later growth")
            .unwrap();
        let mut actual = Vec::new();
        std::io::Read::take(file, length)
            .read_to_end(&mut actual)
            .unwrap();
        assert_eq!(actual, b"first");
        let pairs: Pairs = serde_json::from_str(r#"{"z":"last","a":"first"}"#).unwrap();
        assert_eq!(pairs.0[0].0, "z");
        assert!(serde_json::from_str::<Pairs>(r#"{"z":"first","z":"second"}"#).is_err());
    }
    #[test]
    fn sxcu_http_rechecks_opened_versions_after_headers_for_binary_and_multipart() -> Result<()> {
        use std::{
            io::{Read, Write},
            net::TcpListener,
        };
        for body in ["Binary", "MultipartFormData"] {
            for sealed in [false, true] {
                for mutation in [0u8, 1, 2] {
                    let fixture = Fixture::new();
                    let path = fixture.path("source.bin");
                    let original = vec![b's'; 128 * 1024];
                    std::fs::write(&path, &original)?;
                    let (file, length) = opened(&path, 64 * 1024 * 1024, "fixture")?;
                    let admitted = crate::FileIdentity::from_file(&file)?;
                    let cancel = if sealed {
                        crate::UploadCancellation::default().with_source_identity(admitted.clone())
                    } else {
                        crate::UploadCancellation::default()
                    };
                    cancel.verify_source_file(&file)?;
                    let guard = UploadSourceGuard::new(&file, admitted, length)?;
                    let listener = TcpListener::bind("127.0.0.1:0")?;
                    listener.set_nonblocking(true)?;
                    let url = format!("http://{}/upload", listener.local_addr()?);
                    let changed = path.clone();
                    let worker = std::thread::spawn(move || {
                        let until = std::time::Instant::now() + Duration::from_secs(3);
                        let mut stream = loop {
                            match listener.accept() {
                                Ok((stream, _)) => break stream,
                                Err(error)
                                    if error.kind() == std::io::ErrorKind::WouldBlock
                                        && std::time::Instant::now() < until =>
                                {
                                    std::thread::sleep(Duration::from_millis(1))
                                }
                                error => panic!("Private SXCU accept failed: {error:?}"),
                            }
                        };
                        stream
                            .set_read_timeout(Some(Duration::from_secs(3)))
                            .unwrap();
                        stream
                            .set_write_timeout(Some(Duration::from_secs(3)))
                            .unwrap();
                        let mut headers = Vec::new();
                        let mut byte = [0];
                        while !headers.ends_with(b"\r\n\r\n") {
                            assert!(headers.len() < 8192);
                            stream.read_exact(&mut byte).unwrap();
                            headers.push(byte[0]);
                        }
                        let text = std::str::from_utf8(&headers).unwrap();
                        let mut remaining: usize = text
                            .lines()
                            .find_map(|line| {
                                let (name, value) = line.split_once(':')?;
                                name.eq_ignore_ascii_case("content-length")
                                    .then(|| value.trim().parse().unwrap())
                            })
                            .unwrap();
                        assert!(remaining < 512 * 1024);
                        // Native metadata/version changes occur after admission and before the 200.
                        if mutation != 0 {
                            let mut source = std::fs::OpenOptions::new()
                                .write(true)
                                .open(changed)
                                .unwrap();
                            source.write_all(b"x").unwrap();
                            if mutation == 2 {
                                source.set_len(length + 1).unwrap();
                            }
                            source.sync_all().unwrap();
                        }
                        let mut bytes = [0; 64 * 1024];
                        while remaining != 0 {
                            let count = remaining.min(bytes.len());
                            stream.read_exact(&mut bytes[..count]).unwrap();
                            remaining -= count;
                        }
                        let response = b"https://example.invalid/confirmed";
                        write!(
                            stream,
                            "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                            response.len()
                        )
                        .unwrap();
                        stream.write_all(response).unwrap();
                    });
                    let request = Request {
                        url: url.clone(),
                        method: "POST".into(),
                        body: body.into(),
                        file_field: "file".into(),
                        attach_file: body == "MultipartFormData",
                        headers: Pairs::default(),
                        arguments: Pairs::default(),
                        data: String::new(),
                        events: Vec::new(),
                        secrets: Vec::new(),
                    };
                    let runtime = tokio::runtime::Builder::new_current_thread()
                        .enable_all()
                        .max_blocking_threads(1)
                        .build()?;
                    let result = runtime.block_on(http(
                        &request,
                        reqwest::Url::parse(&url)?,
                        reqwest::header::HeaderMap::new(),
                        Some(Payload::File(file, length)),
                        Zeroizing::new(Vec::new()),
                        "public.bin",
                        Some(guard),
                        cancel,
                    ));
                    worker.join().unwrap();
                    if mutation == 0 {
                        assert_eq!(result?.status, 200);
                        assert_eq!(std::fs::read(&path)?, original);
                    } else {
                        let error = format!(
                            "{:#}",
                            result.err().expect("Changed SXCU source was confirmed")
                        );
                        assert!(
                            error.contains("changed") && error.contains("server may"),
                            "{error}"
                        );
                        assert_eq!(
                            std::fs::metadata(&path)?.len(),
                            length + u64::from(mutation == 2)
                        );
                    }
                }
            }
        }
        Ok(())
    }
    #[test]
    fn multipart_tracks_each_argument_and_file_with_exact_limits_and_refuses_early_2xx()
    -> Result<()> {
        use std::{
            io::{Read, Write},
            net::TcpListener,
        };
        for attach_file in [false, true] {
            for early in [false, true] {
                let listener = TcpListener::bind("127.0.0.1:0")?;
                listener.set_nonblocking(true)?;
                let url = format!("http://{}/upload", listener.local_addr()?);
                let worker = std::thread::spawn(move || {
                    let until = std::time::Instant::now() + Duration::from_secs(3);
                    let mut stream = loop {
                        match listener.accept() {
                            Ok((stream, _)) => break stream,
                            Err(error)
                                if error.kind() == std::io::ErrorKind::WouldBlock
                                    && std::time::Instant::now() < until =>
                            {
                                std::thread::sleep(Duration::from_millis(1))
                            }
                            error => panic!("Private multipart accept failed: {error:?}"),
                        }
                    };
                    stream
                        .set_read_timeout(Some(Duration::from_secs(3)))
                        .unwrap();
                    stream
                        .set_write_timeout(Some(Duration::from_secs(3)))
                        .unwrap();
                    let mut headers = Vec::new();
                    let mut byte = [0];
                    while !headers.ends_with(b"\r\n\r\n") {
                        assert!(headers.len() < 8192);
                        stream.read_exact(&mut byte).unwrap();
                        headers.push(byte[0]);
                    }
                    let text = std::str::from_utf8(&headers).unwrap();
                    let length: usize = text
                        .lines()
                        .find_map(|line| {
                            let (name, value) = line.split_once(':')?;
                            name.eq_ignore_ascii_case("content-length")
                                .then(|| value.trim().parse().unwrap())
                        })
                        .unwrap();
                    assert!(length < 65536);
                    let mut body = Vec::new();
                    if !early {
                        body.resize(length, 0);
                        stream.read_exact(&mut body).unwrap();
                    }
                    let response = b"https://example.invalid/confirmed";
                    write!(
                        stream,
                        "HTTP/1.1 201 Created\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                        response.len()
                    )
                    .unwrap();
                    stream.write_all(response).unwrap();
                    if early {
                        // Keep unread request bytes from resetting TCP before the client sees 201.
                        // This drains transport teardown only; the early response accepted no body.
                        stream.shutdown(std::net::Shutdown::Write).unwrap();
                        let until = std::time::Instant::now() + Duration::from_secs(3);
                        let mut discarded = [0; 1024];
                        let mut count = 0;
                        loop {
                            assert!(std::time::Instant::now() < until);
                            match stream.read(&mut discarded) {
                                Ok(0) => break,
                                Ok(length) => {
                                    count += length;
                                    assert!(count < 65536);
                                }
                                Err(error)
                                    if error.kind() == std::io::ErrorKind::ConnectionReset =>
                                {
                                    break;
                                }
                                Err(error) if error.kind() == std::io::ErrorKind::Interrupted => {
                                    continue;
                                }
                                Err(error) => {
                                    panic!("Private early-response teardown failed: {error}")
                                }
                            }
                        }
                    }
                    body
                });
                let request = Request {
                    url: url.clone(),
                    method: "POST".into(),
                    body: "MultipartFormData".into(),
                    file_field: if attach_file {
                        "file".into()
                    } else {
                        String::new()
                    },
                    attach_file,
                    headers: Pairs::default(),
                    arguments: Pairs(vec![
                        ("first".into(), "one".into()),
                        ("second".into(), "longer café".into()),
                        ("empty".into(), String::new()),
                    ]),
                    data: String::new(),
                    events: Vec::new(),
                    secrets: Vec::new(),
                };
                let _pause = early.then(crate::upload::upload_source::pause_next_body_for_test);
                let payload =
                    attach_file.then(|| Payload::Bytes(b"file-with-different-length".to_vec()));
                let runtime = tokio::runtime::Builder::new_current_thread()
                    .enable_all()
                    .max_blocking_threads(1)
                    .build()?;
                let result = runtime.block_on(http(
                    &request,
                    reqwest::Url::parse(&url)?,
                    reqwest::header::HeaderMap::new(),
                    payload,
                    Zeroizing::new(Vec::new()),
                    "public.bin",
                    None,
                    crate::UploadCancellation::default(),
                ));
                drop(runtime);
                let body = worker.join().unwrap();
                if early {
                    let error = format!(
                        "{:#}",
                        result
                            .err()
                            .expect("Paused multipart argument produced a successful response")
                    );
                    assert!(
                        error.contains("not fully consumed") && error.contains("remote object"),
                        "{error}"
                    );
                    assert!(body.is_empty());
                    // http returns no success to interpret/publish, so caller `?` cannot reach Link/QR.
                } else {
                    assert_eq!(result?.status, 201);
                    for bytes in [
                        b"name=\"first\"\r\n\r\none\r\n".as_slice(),
                        "name=\"second\"\r\n\r\nlonger café\r\n".as_bytes(),
                        b"name=\"empty\"\r\n\r\n\r\n".as_slice(),
                    ] {
                        assert!(body.windows(bytes.len()).any(|part| part == bytes));
                    }
                    assert_eq!(
                        body.windows(b"file-with-different-length".len())
                            .any(|part| part == b"file-with-different-length"),
                        attach_file
                    );
                }
            }
        }
        Ok(())
    }
    #[test]
    fn admitted_memory_body_counts_consumption_without_another_image_copy() {
        let count = Arc::new(std::sync::atomic::AtomicU64::new(0));
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        let bytes = b"finalized synthetic PNG bytes".to_vec();
        let length = bytes.len() as u64;
        let mut reader = TrackedUploadReader::new(
            std::io::Cursor::new(bytes),
            length,
            count.clone(),
            crate::UploadCancellation::default(),
        );
        let mut actual = Vec::new();
        runtime.block_on(async {
            reader.read_to_end(&mut actual).await.unwrap();
        });
        assert_eq!(count.load(Ordering::Relaxed), actual.len() as u64);
        assert_eq!(actual, b"finalized synthetic PNG bytes");
    }
}
