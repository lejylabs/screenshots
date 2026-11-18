//! In-process interpreter for the published ShareX custom-uploader function contract.
//!
//! Original implementation; no ShareX code is included. It performs no upload,
//! credential lookup, process execution, file read or clipboard/browser access.
//! Rust callers keep source admission, HTTP transport and credential-origin checks.
//! The protocol is one JSON request and one JSON envelope, as the former helper used.
mod json;
mod names;
mod xml;

use base64::Engine;
use json::J;
use serde::Serialize;
use std::{
    collections::{HashMap, HashSet},
    rc::Rc,
    time::{Duration, Instant},
};
use zeroize::Zeroizing;

pub(super) enum Fail {
    Code(&'static str),
    Abort(anyhow::Error),
}
impl Fail {
    fn invalid() -> Self {
        Fail::Code("invalid_expression_or_request")
    }
}
type R<T> = Result<T, Fail>;

mod limits {
    use super::{Fail, R};
    pub(super) const INPUT: usize = 4 * 1024 * 1024;
    pub(super) const OUTPUT: usize = 2 * 1024 * 1024;
    pub(super) const TEXT: usize = 65536;
    pub(super) const BODY: usize = 512 * 1024;
    pub(super) const INTERMEDIATE: usize = 2 * 1024 * 1024;
    pub(super) fn require(value: bool, code: &'static str) -> R<()> {
        if value { Ok(()) } else { Err(Fail::Code(code)) }
    }
    pub(super) fn bounded(value: String, bytes: usize) -> R<String> {
        require(value.len() <= bytes, "expansion_limit")?;
        Ok(value)
    }
}

/// Field value: string or null, bounded in UTF-8 bytes.
fn string(value: Option<&J>, bytes: usize) -> R<String> {
    match value {
        None | Some(J::Null) => Ok(String::new()),
        Some(J::Str(text)) => limits::bounded(text.clone(), bytes),
        Some(_) => Err(Fail::Code("field_type")),
    }
}
fn integer(value: Option<&J>) -> Option<i64> {
    match value {
        Some(J::Num(raw)) if J::is_integer(raw) => raw.parse().ok(),
        _ => None,
    }
}
fn chars_len(text: &str) -> usize {
    text.encode_utf16().count()
}

/// Uniform index from the OS CSPRNG.
fn random_below(bound: usize) -> usize {
    debug_assert!(bound > 0);
    let bound = bound as u64;
    let zone = u64::MAX - u64::MAX % bound;
    loop {
        let mut bytes = [0u8; 8];
        getrandom::fill(&mut bytes).expect("operating system random source");
        let value = u64::from_le_bytes(bytes);
        if value < zone {
            return (value % bound) as usize;
        }
    }
}

fn regex(pattern: &str) -> R<fancy_regex::Regex> {
    fancy_regex::RegexBuilder::new(pattern)
        .backtrack_limit(1_000_000)
        .delegate_size_limit(8 * 1024 * 1024)
        .delegate_dfa_size_limit(8 * 1024 * 1024)
        .build()
        .map_err(|_| Fail::invalid())
}
fn regex_find<'t>(regex: &fancy_regex::Regex, text: &'t str) -> R<Option<fancy_regex::Captures<'t, str>>> {
    regex.captures(text).map_err(|error| match error {
        fancy_regex::Error::RuntimeError(_) => Fail::Code("regex_timeout"),
        _ => Fail::invalid(),
    })
}

/// Credential-like key names; mirrors the application's upload sensitivity policy.
pub(super) fn sensitive(key: &str) -> bool {
    let lower = key.to_lowercase();
    let normalized = lower.replace(['-', '_', ' '], "");
    const KNOWN: [&str; 21] = [
        "password", "passwd", "secret", "token", "accesstoken", "refreshtoken", "authtoken", "apikey",
        "authorization", "auth", "clientsecret", "key", "accesskey", "accesskeyid", "secretaccesskey", "credential",
        "credentials", "session", "sessionid", "xauth", "xkey",
    ];
    KNOWN.contains(&normalized.as_str())
        || lower
            .split(|ch: char| !(ch.is_ascii_lowercase() || ch.is_ascii_digit()))
            .any(|part| matches!(part, "auth" | "key" | "credential" | "credentials"))
        || ["authorization", "cookie", "apikey", "token", "password", "passwd", "secret", "signature"]
            .iter()
            .any(|part| normalized.contains(part))
}

fn hex(byte: u8) -> Option<u8> {
    match byte {
        b'0'..=b'9' => Some(byte - b'0'),
        b'a'..=b'f' => Some(byte - b'a' + 10),
        b'A'..=b'F' => Some(byte - b'A' + 10),
        _ => None,
    }
}
/// Percent-decode bytes; `form` turns `+` into a space.
fn percent_decode(value: &str, form: bool) -> Vec<u8> {
    let source = value.as_bytes();
    let mut out = Vec::with_capacity(source.len());
    let mut i = 0;
    while i < source.len() {
        if source[i] == b'%'
            && let (Some(high), Some(low)) =
                (source.get(i + 1).copied().and_then(hex), source.get(i + 2).copied().and_then(hex))
        {
            out.push(high * 16 + low);
            i += 3;
            continue;
        }
        out.push(if form && source[i] == b'+' { b' ' } else { source[i] });
        i += 1;
    }
    out
}
fn delivered_query_name(value: &str) -> R<String> {
    limits::bounded(value.to_owned(), 16384)?;
    String::from_utf8(percent_decode(value, true)).map_err(|_| Fail::invalid())
}
fn inspection_projection(value: &str, form: bool) -> String {
    String::from_utf8_lossy(&percent_decode(value, form)).into_owned()
}
/// `Uri.EscapeDataString`.
fn escape_data(value: &str) -> String {
    let mut out = String::with_capacity(value.len());
    for byte in value.bytes() {
        if byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'.' | b'_' | b'~') {
            out.push(byte as char);
        } else {
            out.push_str(&format!("%{byte:02X}"));
        }
    }
    out
}
/// `HttpUtility.UrlEncode`.
fn url_encode(value: &str) -> String {
    let mut out = String::with_capacity(value.len());
    for byte in value.bytes() {
        if byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.' | b'!' | b'*' | b'(' | b')') {
            out.push(byte as char);
        } else if byte == b' ' {
            out.push('+');
        } else {
            out.push_str(&format!("%{byte:02x}"));
        }
    }
    out
}
fn json_escape(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for ch in text.chars() {
        match ch {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            '\u{8}' => out.push_str("\\b"),
            '\u{c}' => out.push_str("\\f"),
            ch if (ch as u32) < 0x20 || matches!(ch, '\u{85}' | '\u{2028}' | '\u{2029}') => {
                out.push_str(&format!("\\u{:04x}", ch as u32))
            }
            ch => out.push(ch),
        }
    }
    out
}
fn xml_escape(text: &str) -> String {
    text.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;").replace('"', "&quot;").replace('\'', "&apos;")
}
/// ASCII case-insensitive replacement of an ASCII needle.
fn replace_ci(source: &str, needle: &str, replacement: &str) -> String {
    let lower = source.to_ascii_lowercase();
    let needle_lower = needle.to_ascii_lowercase();
    let mut out = String::with_capacity(source.len());
    let mut last = 0;
    for (index, _) in lower.match_indices(&needle_lower) {
        if index < last {
            continue;
        }
        out.push_str(&source[last..index]);
        out.push_str(replacement);
        last = index + needle.len();
    }
    out.push_str(&source[last..]);
    out
}
fn starts_ci(source: &[char], at: usize, needle: &str) -> bool {
    needle.chars().enumerate().all(|(n, ch)| source.get(at + n).is_some_and(|c| c.eq_ignore_ascii_case(&ch)))
}

// ---- Profile ----

const KEYS: [&str; 15] = [
    "Version", "Name", "DestinationType", "RequestMethod", "RequestURL", "Parameters", "Headers", "Body", "Arguments",
    "FileFormName", "Data", "URL", "ThumbnailURL", "DeletionURL", "ErrorMessage",
];
const METHODS: [&str; 5] = ["GET", "POST", "PUT", "PATCH", "DELETE"];
const BODIES: [&str; 6] = ["None", "MultipartFormData", "FormURLEncoded", "JSON", "XML", "Binary"];
const DESTINATIONS: [&str; 5] = ["ImageUploader", "TextUploader", "FileUploader", "URLShortener", "URLSharingService"];
const FUNCTIONS: [(&str, usize); 14] = [
    ("filename", 0), ("input", 0), ("response", 0), ("responseurl", 0), ("header", 1), ("base64", 1),
    ("random", 2), ("select", 1), ("inputbox", 0), ("prompt", 0), ("outputbox", 1), ("json", 1), ("regex", 1),
    ("xml", 1),
];
fn minimum_arity(name: &str) -> Option<usize> {
    FUNCTIONS.iter().find(|(n, _)| *n == name).map(|(_, a)| *a)
}

type Pairs = Vec<(String, String)>;
struct Profile {
    method: &'static str,
    url: String,
    body: &'static str,
    field: String,
    data: String,
    link: String,
    thumbnail: String,
    deletion: String,
    error: String,
    destinations: i64,
    parameters: Pairs,
    headers: Pairs,
    arguments: Pairs,
}
fn enum_value(token: Option<&J>, names: &[&'static str], fallback: &'static str) -> R<&'static str> {
    match token {
        None | Some(J::Null) => Ok(fallback),
        Some(J::Num(raw)) if J::is_integer(raw) => {
            let index: i64 = raw.parse().map_err(|_| Fail::Code("enum_value"))?;
            limits::require(index >= 0 && (index as usize) < names.len(), "enum_value")?;
            Ok(names[index as usize])
        }
        token => {
            let value = string(token, 128)?;
            let value = value.trim();
            names.iter().copied().find(|name| name.eq_ignore_ascii_case(value)).ok_or(Fail::Code("enum_value"))
        }
    }
}
fn map(token: Option<&J>) -> R<Pairs> {
    match token {
        None | Some(J::Null) => Ok(Vec::new()),
        Some(J::Obj(fields)) => {
            let mut out = Vec::new();
            for (name, value) in fields {
                limits::require(out.len() < 32 && (1..=128).contains(&chars_len(name)), "map_limit")?;
                out.push((name.clone(), string(Some(value), limits::TEXT)?));
            }
            Ok(out)
        }
        Some(_) => Err(Fail::Code("map_type")),
    }
}
fn migrate(source: &str) -> String {
    let mut out = String::new();
    let mut opening = true;
    let mut chars = source.chars();
    while let Some(ch) = chars.next() {
        match ch {
            '$' => {
                out.push(if opening { '{' } else { '}' });
                opening = !opening;
            }
            '\\' => {
                chars.next();
            }
            ch => {
                if matches!(ch, '{' | '}') {
                    out.push('\\');
                }
                out.push(ch);
            }
        }
    }
    out
}
/// `System.Version` with missing build/revision as zero.
fn version(text: &str) -> R<(i64, i64, i64, i64)> {
    let parts: Vec<&str> = text.split('.').collect();
    limits::require((2..=4).contains(&parts.len()), "unsupported_version")?;
    let mut values = [0i64; 4];
    for (slot, part) in values.iter_mut().zip(&parts) {
        let part = part.trim();
        limits::require(!part.is_empty() && !part.starts_with(['+', '-']), "unsupported_version")?;
        *slot = part.parse::<i32>().map_err(|_| Fail::Code("unsupported_version"))? as i64;
    }
    Ok((values[0], values[1], values[2], values[3]))
}
/// `HttpUtility.ParseQueryString`: keyless items become parameter names.
fn parse_query(text: &str, into: &mut Pairs) {
    let mut found: Vec<(Option<String>, Vec<String>)> = Vec::new();
    for item in text.split('&') {
        let (key, value) = match item.split_once('=') {
            Some((key, value)) => (Some(key), value),
            None => (None, item),
        };
        if key.is_none() && value.is_empty() {
            continue;
        }
        let decode = |v: &str| String::from_utf8_lossy(&percent_decode(v, true)).into_owned();
        let key = key.map(decode);
        let value = decode(value);
        match found.iter_mut().find(|(k, _)| *k == key) {
            Some((_, values)) => values.push(value),
            None => found.push((key, vec![value])),
        }
    }
    let mut add = |key: String, value: String| {
        if !into.iter().any(|(k, _)| *k == key) {
            into.push((key, value));
        }
    };
    for (key, values) in found {
        match key {
            None => values.into_iter().for_each(|value| add(value, String::new())),
            Some(key) => add(key, values.join(",")),
        }
    }
}
impl Profile {
    fn read(text: &str) -> R<Self> {
        limits::bounded(text.to_owned(), 16384)?;
        let J::Obj(fields) = json::parse(text, true)? else { return Err(Fail::invalid()) };
        let mut canonical: HashMap<&'static str, J> = HashMap::new();
        for (name, value) in fields {
            let key = KEYS.iter().copied().find(|key| key.eq_ignore_ascii_case(&name)).ok_or(Fail::Code("unknown_field"))?;
            limits::require(!canonical.contains_key(key), "duplicate_field")?;
            canonical.insert(key, value);
        }
        let get = |key: &str| canonical.get(key);
        let version_text = string(get("Version"), 64)?;
        string(get("Name"), limits::TEXT)?;
        let mut profile = Profile {
            url: string(get("RequestURL"), limits::TEXT)?,
            field: string(get("FileFormName"), 128)?,
            data: string(get("Data"), limits::TEXT)?,
            link: string(get("URL"), limits::TEXT)?,
            thumbnail: string(get("ThumbnailURL"), limits::TEXT)?,
            deletion: string(get("DeletionURL"), limits::TEXT)?,
            error: string(get("ErrorMessage"), limits::TEXT)?,
            parameters: map(get("Parameters"))?,
            headers: map(get("Headers"))?,
            arguments: map(get("Arguments"))?,
            method: enum_value(get("RequestMethod"), &METHODS, "POST")?,
            body: enum_value(get("Body"), &BODIES, "None")?,
            destinations: 0,
        };
        let version = version(&version_text)?;
        limits::require(version > (12, 3, 1, 0), "unsupported_version")?;
        match get("DestinationType") {
            Some(J::Num(raw)) if J::is_integer(raw) => {
                profile.destinations = raw.parse().map_err(|_| Fail::Code("destination_value"))?
            }
            token => {
                for name in string(token, 256)?.split(',').map(str::trim).filter(|n| !n.is_empty()) {
                    if name.eq_ignore_ascii_case("None") {
                        continue;
                    }
                    let index = DESTINATIONS
                        .iter()
                        .position(|value| value.eq_ignore_ascii_case(name))
                        .ok_or(Fail::Code("destination_value"))?;
                    profile.destinations |= 1 << index;
                }
            }
        }
        limits::require((0..=31).contains(&profile.destinations) && !profile.url.is_empty(), "destination_value")?;
        if let Some(query) = profile.url.find('?') {
            let rest = profile.url[query + 1..].to_owned();
            parse_query(&rest, &mut profile.parameters);
            profile.url.truncate(query);
        }
        if version <= (13, 7, 1, 0) {
            for value in [
                &mut profile.url,
                &mut profile.link,
                &mut profile.thumbnail,
                &mut profile.deletion,
                &mut profile.error,
            ] {
                *value = migrate(value);
            }
            for map in [&mut profile.parameters, &mut profile.headers, &mut profile.arguments] {
                for (_, value) in map.iter_mut() {
                    *value = migrate(value);
                }
            }
            profile.data = replace_ci(&replace_ci(&profile.data, "$input$", "{input}"), "$filename$", "{filename}");
        }
        Ok(profile)
    }
    fn admit_kind(&self, kind: &str) -> R<()> {
        let flag = match kind {
            "image" => 1,
            "text" => 2,
            "file" => 4,
            "url" => 8,
            "url_sharing" => 16,
            _ => return Err(Fail::Code("input_kind")),
        };
        let file = matches!(kind, "image" | "file");
        limits::require(self.destinations & flag != 0, "input_destination")?;
        limits::require(!file || matches!(self.body, "MultipartFormData" | "Binary"), "input_body")?;
        limits::require(!matches!(kind, "url" | "url_sharing") || self.body != "Binary", "input_body")?;
        limits::require(!file || self.body != "MultipartFormData" || !self.field.is_empty(), "file_form_name")
    }
}

// ---- Context ----

#[derive(Clone)]
pub(super) struct Context {
    filename: String,
    input: String,
    response: String,
    response_url: String,
    kind: String,
    status: i64,
    allow_environment: bool,
    answer_indices: HashMap<String, u32>,
    function_names: HashMap<String, String>,
    answers: HashMap<String, String>,
    names: HashMap<String, String>,
    /// Lowercased name -> joined values.
    headers: HashMap<String, String>,
    events: Vec<Event>,
    marker: std::cell::OnceCell<String>,
    cancel: Option<crate::UploadCancellation>,
    deadline: Instant,
}
#[derive(Clone, Serialize)]
struct Event {
    title: String,
    text: String,
}
impl Context {
    fn unknown_marker(&self) -> &str {
        self.marker.get_or_init(|| {
            let mut marker = String::from("9876543210");
            for _ in 0..40 {
                marker.push(char::from(b'0' + random_below(10) as u8));
            }
            marker
        })
    }
    fn tick(&self) -> R<()> {
        if let Some(cancel) = &self.cancel {
            cancel.check().map_err(Fail::Abort)?;
        }
        if Instant::now() >= self.deadline {
            return Err(Fail::Abort(anyhow::anyhow!("Private SXCU interpreter exceeded five seconds")));
        }
        Ok(())
    }
    fn answer(&self, id: &str, choices: Option<&[String]>) -> R<String> {
        if let (Some(choices), Some(index)) = (choices, self.answer_indices.get(id)) {
            limits::require((*index as usize) < choices.len() && !self.answers.contains_key(id), "selection_index")?;
            return Ok(choices[*index as usize].clone());
        }
        let answer = self.answers.get(id).ok_or(Fail::Code("answer_required"))?;
        limits::bounded(answer.clone(), 4096)?;
        limits::require(choices.is_none_or(|c| c.iter().any(|v| v == answer)), "selection_answer")?;
        Ok(answer.clone())
    }
    fn read(root: &J, cancel: Option<crate::UploadCancellation>, deadline: Instant) -> R<Self> {
        let mut context = Context {
            filename: string(root.get("filename"), 128)?,
            input: string(root.get("input"), 65536)?,
            kind: string(root.get("kind"), 32)?,
            allow_environment: matches!(root.get("allow_environment_names"), Some(J::Bool(true))),
            response: String::new(),
            response_url: String::new(),
            status: 200,
            answer_indices: HashMap::new(),
            function_names: HashMap::new(),
            answers: HashMap::new(),
            names: HashMap::new(),
            headers: HashMap::new(),
            events: Vec::new(),
            marker: std::cell::OnceCell::new(),
            cancel,
            deadline,
        };
        for (field, target) in [
            ("answers", &mut context.answers),
            ("name_values", &mut context.names),
            ("function_names", &mut context.function_names),
        ] {
            if let Some(J::Obj(fields)) = root.get(field) {
                for (name, value) in fields {
                    limits::require(target.len() < 128, "context_limit")?;
                    target.insert(name.clone(), string(Some(value), 4096)?);
                }
            }
        }
        if let Some(J::Obj(fields)) = root.get("answer_indices") {
            for (name, value) in fields {
                let index = integer(Some(value));
                limits::require(
                    context.answer_indices.len() + context.answers.len() < 128
                        && (1..=256).contains(&chars_len(name))
                        && index.is_some_and(|i| (0..16384).contains(&i))
                        && !context.answers.contains_key(name),
                    "selection_index",
                )?;
                context.answer_indices.insert(name.clone(), index.unwrap_or_default() as u32);
            }
        }
        for name in context.function_names.values() {
            limits::require(minimum_arity(&name.to_lowercase()).is_some(), "function_binding")?;
        }
        if let Some(response @ J::Obj(_)) = root.get("response") {
            context.status = integer(response.get("status")).unwrap_or(0);
            context.response_url = string(response.get("url"), 8192)?;
            let encoded = string(response.get("bytes"), 1_400_000)?;
            let bytes = Zeroizing::new(
                base64::engine::general_purpose::STANDARD.decode(encoded.as_bytes()).map_err(|_| Fail::invalid())?,
            );
            limits::require(bytes.len() <= 1_048_576, "response_limit")?;
            let content_type = string(response.get("content_type"), 4096)?;
            let charset = content_type.split(';').skip(1).find_map(|parameter| {
                let (name, value) = parameter.split_once('=')?;
                name.trim().eq_ignore_ascii_case("charset").then(|| value.trim().trim_matches('"').to_owned())
            });
            let encoding = match charset.filter(|c| !c.trim().is_empty()) {
                Some(label) => encoding_rs::Encoding::for_label(label.as_bytes()).ok_or_else(Fail::invalid)?,
                None => encoding_rs::UTF_8,
            };
            context.response = encoding.decode(&bytes).0.into_owned();
            if let Some(J::Obj(fields)) = response.get("headers") {
                for (name, value) in fields {
                    let J::Arr(values) = value else { return Err(Fail::Code("response_headers")) };
                    limits::require(context.headers.len() < 128, "response_headers")?;
                    let joined = values.iter().map(|v| string(Some(v), 8192)).collect::<R<Vec<_>>>()?.join(",");
                    limits::require(context.headers.insert(name.to_lowercase(), joined).is_none(), "invalid_expression_or_request")?;
                }
            }
        }
        Ok(context)
    }
}

// ---- Template syntax ----

enum Node {
    Literal(String),
    Call { name: Vec<Node>, id: String, arguments: Vec<Vec<Node>> },
}
fn literal_value(tree: &[Node]) -> Option<String> {
    tree.iter()
        .map(|node| match node {
            Node::Literal(text) => Some(text.as_str()),
            Node::Call { .. } => None,
        })
        .collect()
}
fn response_field(field: &str) -> bool {
    matches!(field, "URL" | "ThumbnailURL" | "DeletionURL" | "ErrorMessage")
}
fn url_field(field: &str) -> bool {
    field == "RequestURL" || response_field(field)
}

struct Budget {
    calls: usize,
    materialized: usize,
    inspection: Vec<serde_json::Value>,
}
struct Syntax<'a> {
    source: Vec<char>,
    field: &'a str,
    offset: usize,
    ordinal: usize,
    budget: &'a mut Budget,
}
impl Syntax<'_> {
    fn parse(mut self) -> R<Vec<Node>> {
        let result = self.sequence(0, false, false)?;
        limits::require(self.offset == self.source.len(), "template_delimiter")?;
        Ok(result)
    }
    fn sequence(&mut self, depth: usize, argument: bool, name_mode: bool) -> R<Vec<Node>> {
        limits::require(depth <= 16, "template_depth")?;
        let mut result = Vec::new();
        let mut text = String::new();
        while self.offset < self.source.len() {
            let c = self.source[self.offset];
            if c == '\\' {
                self.offset += 1;
                if self.offset < self.source.len() {
                    text.push(self.source[self.offset]);
                    self.offset += 1;
                }
                continue;
            }
            if c == '}' || c == '|' || name_mode && c == ':' {
                limits::require(argument, "template_delimiter")?;
                break;
            }
            if c != '{' {
                text.push(c);
                self.offset += 1;
                continue;
            }
            if !text.is_empty() {
                result.push(Node::Literal(std::mem::take(&mut text)));
            }
            self.offset += 1;
            self.budget.calls += 1;
            limits::require(self.budget.calls <= 128, "template_calls")?;
            let id = format!("{}#{}", self.field, self.ordinal);
            self.ordinal += 1;
            let name = self.sequence(depth + 1, true, true)?;
            let function = literal_value(&name).map(|n| n.to_lowercase());
            if let Some(function) = &function {
                limits::require(minimum_arity(function).is_some(), "function_name")?;
            }
            let mut args: Vec<Vec<Node>> = Vec::new();
            if self.source.get(self.offset) == Some(&':') {
                loop {
                    self.offset += 1;
                    args.push(self.sequence(depth + 1, true, false)?);
                    if self.source.get(self.offset) != Some(&'|') {
                        break;
                    }
                }
            }
            limits::require(self.source.get(self.offset) == Some(&'}'), "template_unclosed")?;
            self.offset += 1;
            if let Some(function) = &function {
                limits::require(args.len() >= minimum_arity(function).unwrap_or(0), "function_arity")?;
            }
            let stage = if response_field(self.field) { "response" } else { "request" };
            match function.as_deref() {
                Some(function @ ("inputbox" | "prompt" | "select")) => {
                    let title = if function == "select" {
                        Some("Select".to_owned())
                    } else if args.is_empty() {
                        Some("Input".to_owned())
                    } else {
                        literal_value(&args[0])
                    };
                    let redacted = sensitive_field(self.field) || title.as_deref().is_some_and(sensitive);
                    let default_text = if args.len() > 1 { literal_value(&args[1]) } else { Some(String::new()) };
                    let choices: Option<Vec<String>> = (function == "select")
                        .then(|| args.iter().map(|a| literal_value(a)).collect::<Option<Vec<_>>>())
                        .flatten()
                        .map(|values| values.into_iter().filter(|v| !v.is_empty()).collect());
                    let answer_kind = if function == "select" {
                        if choices.is_none() { "index_unless_frozen_context" } else { "value_or_index" }
                    } else {
                        "value"
                    };
                    let requires = title.is_none() || function == "select" && choices.is_none();
                    self.budget.inspection.push(serde_json::json!({
                        "id": id, "function": function, "parameters": args.len(), "stage": stage,
                        "answer_kind": answer_kind,
                        "title": if redacted { None } else { title },
                        "default_text": if redacted { None } else { default_text },
                        "choices": if redacted { None } else { choices },
                        "redacted": redacted, "requires_runtime_context": requires,
                    }));
                }
                None => self.budget.inspection.push(serde_json::json!({
                    "id": id, "function": "computed", "parameters": args.len(), "stage": stage,
                    "answer_kind": "function_name_if_not_frozen_context",
                })),
                Some(_) => {}
            }
            result.push(Node::Call { name, id, arguments: args });
        }
        if !text.is_empty() {
            result.push(Node::Literal(text));
        }
        Ok(result)
    }
}

fn sensitive_field(field: &str) -> bool {
    let name = field.split_once('.').map_or(field, |(_, rest)| rest);
    if field.starts_with("Parameters.") {
        delivered_query_name(name).is_ok_and(|decoded| sensitive(&decoded))
    } else {
        sensitive(name)
    }
}

// ---- Body authentication ----

fn dotnet_number(raw: &str) -> R<String> {
    json::token_string(&J::Num(raw.to_owned()))
}
/// Shared configured/resolved-body credential policy. It never rewrites the transmitted body.
fn body_authentication(body: &str, marker: Option<&str>, emit: &mut dyn FnMut(&str, &str) -> R<()>) -> R<()> {
    limits::bounded(body.to_owned(), limits::BODY)?;
    let trimmed = body.trim_start();
    struct Found<'m, 'e> {
        known: HashSet<String>,
        bytes: usize,
        marker: Option<&'m str>,
        emit: &'e mut dyn FnMut(&str, &str) -> R<()>,
    }
    impl Found<'_, '_> {
        fn add(&mut self, name: &str, value: &str) -> R<()> {
            if value.is_empty() || self.marker.is_some_and(|m| value.contains(m)) {
                return Ok(());
            }
            if self.known.insert(value.to_owned()) {
                self.bytes += value.len();
                limits::require(self.known.len() <= 128 && self.bytes <= 256 * 1024, "body_auth_limit")?;
            }
            (self.emit)(name, value)
        }
    }
    let mut found = Found { known: HashSet::new(), bytes: 0, marker, emit };
    if trimmed.starts_with(['{', '[']) {
        let mut context = vec![false];
        let mut cookies = vec![0u8];
        let mut pending = 0u8;
        let mut property = false;
        let result = json::events(body, true, &mut |event| {
            use json::Ev;
            match event {
                Ev::Key(name) => {
                    property = *context.last().unwrap_or(&false) || sensitive(&name);
                    pending = if name.eq_ignore_ascii_case("Cookie") {
                        1
                    } else if name.eq_ignore_ascii_case("Set-Cookie") {
                        2
                    } else {
                        *cookies.last().unwrap_or(&0)
                    };
                }
                Ev::StartObj | Ev::StartArr => {
                    let inherited = *context.last().unwrap_or(&false) || property;
                    context.push(inherited);
                    let cookie = if pending == 0 { *cookies.last().unwrap_or(&0) } else { pending };
                    cookies.push(cookie);
                    property = false;
                    pending = 0;
                }
                Ev::EndObj | Ev::EndArr => {
                    if context.len() > 1 {
                        context.pop();
                        cookies.pop();
                    }
                    property = false;
                    pending = 0;
                }
                Ev::Null => {
                    property = false;
                    pending = 0;
                }
                value => {
                    if *context.last().unwrap_or(&false) || property {
                        let cookie = if pending == 0 { *cookies.last().unwrap_or(&0) } else { pending };
                        let scope = match cookie {
                            1 => "Cookie",
                            2 => "Set-Cookie",
                            _ => "body",
                        };
                        match value {
                            Ev::Str(text) => found.add(scope, &text)?,
                            Ev::Bool(b) => found.add(scope, if b { "true" } else { "false" })?,
                            Ev::Num(raw) => {
                                found.add(scope, &dotnet_number(raw)?)?;
                                if !found.known.contains(raw) {
                                    found.add("body", raw)?;
                                }
                            }
                            _ => {}
                        }
                    }
                    property = false;
                    pending = 0;
                }
            }
            Ok(())
        });
        match result {
            Ok(()) => return Ok(()),
            Err(json::JErr::Fail(fail)) => return Err(fail),
            Err(json::JErr::Depth) => return Err(Fail::Code("body_depth")),
            Err(json::JErr::Constructor) => return Err(Fail::Code("body_json_constructor")),
            Err(json::JErr::Syntax) => {}
        }
    }
    if trimmed.starts_with('<') {
        let lower = body.to_ascii_lowercase();
        limits::require(!lower.contains("<!doctype") && !lower.contains("<!entity"), "body_dtd_forbidden")?;
        if let Ok(document) = xml::parse(body) {
            xml::require_depth(&document, "body_depth")?;
            return xml::scan_body(&document, &mut |name, value| found.add(name, value));
        }
    }
    // Raw form data is classified even under a custom Content-Type. Unrelated keys stay data.
    for pair in body.split('&') {
        let Some((name, value)) = pair.split_once('=') else { continue };
        let name = delivered_query_name(name)?;
        if sensitive(&name) {
            found.add(&name, &delivered_query_name(value)?)?;
        }
    }
    Ok(())
}

// ---- Interpreter ----

struct Interpreter {
    profile: Profile,
    context: Context,
    budget: Budget,
    trees: Vec<(String, Rc<Vec<Node>>)>,
    secrets: Vec<String>,
    secret_set: HashSet<String>,
    secret_bytes: usize,
    resolved_data: Option<String>,
}
impl Interpreter {
    fn new(profile: Profile, context: Context) -> R<Self> {
        let mut this = Interpreter {
            profile,
            context,
            budget: Budget { calls: 0, materialized: 0, inspection: Vec::new() },
            trees: Vec::new(),
            secrets: Vec::new(),
            secret_set: HashSet::new(),
            secret_bytes: 0,
            resolved_data: None,
        };
        let mut fields: Vec<(String, String, bool)> = vec![("RequestURL".into(), this.profile.url.clone(), false)];
        for (prefix, map) in
            [("Parameters.", &this.profile.parameters), ("Headers.", &this.profile.headers), ("Arguments.", &this.profile.arguments)]
        {
            fields.extend(map.iter().map(|(k, v)| (format!("{prefix}{k}"), v.clone(), true)));
        }
        fields.push(("URL".into(), this.profile.link.clone(), false));
        fields.push(("ThumbnailURL".into(), this.profile.thumbnail.clone(), false));
        fields.push(("DeletionURL".into(), this.profile.deletion.clone(), false));
        fields.push(("ErrorMessage".into(), this.profile.error.clone(), false));
        for (field, value, names) in fields {
            let source = if names { names::expand(&value, &this.context, true, false)? } else { value };
            let tree = Syntax { source: source.chars().collect(), field: &field, offset: 0, ordinal: 0, budget: &mut this.budget }
                .parse()?;
            limits::require(!this.trees.iter().any(|(name, _)| *name == field), "invalid_expression_or_request")?;
            this.trees.push((field, Rc::new(tree)));
        }
        Ok(this)
    }
    fn tree(&self, field: &str) -> Rc<Vec<Node>> {
        self.trees.iter().find(|(name, _)| name == field).map(|(_, tree)| tree.clone()).unwrap_or_default()
    }
    /// Visit every call in post-order (name and arguments before the call itself).
    fn for_each_call(&mut self, mut visit: impl FnMut(&mut Self, &str, &Node) -> R<()>) -> R<()> {
        fn walk<S>(
            this: &mut S,
            field: &str,
            tree: &[Node],
            visit: &mut impl FnMut(&mut S, &str, &Node) -> R<()>,
        ) -> R<()> {
            for node in tree {
                if let Node::Call { name, arguments, .. } = node {
                    walk(this, field, name, visit)?;
                    for argument in arguments {
                        walk(this, field, argument, visit)?;
                    }
                    visit(this, field, node)?;
                }
            }
            Ok(())
        }
        for (field, tree) in self.trees.clone() {
            walk(self, &field, &tree, &mut visit)?;
        }
        Ok(())
    }
    fn materialize(&mut self, value: &str) -> R<()> {
        self.budget.materialized += value.len();
        limits::require(self.budget.materialized <= 16 * 1024 * 1024, "materialization_limit")
    }
    fn static_value(&mut self, tree: &[Node], url: bool, depth: usize) -> R<Option<String>> {
        limits::require(depth <= 16, "template_depth")?;
        let mut output = String::new();
        for node in tree {
            let value = match node {
                Node::Literal(text) => text.clone(),
                Node::Call { name, id, arguments } => {
                    let Some(function) = self.static_value(name, url, depth + 1)?.map(|n| n.to_lowercase()) else {
                        return Ok(None);
                    };
                    let value = match function.as_str() {
                        "response" | "responseurl" | "header" => return Ok(None),
                        "inputbox" | "prompt" => self.context.answers.get(id).cloned(),
                        "outputbox" => Some(String::new()),
                        "filename" => Some(if url { escape_data(&self.context.filename) } else { self.context.filename.clone() }),
                        "input" => Some(if url { escape_data(&self.context.input) } else { self.context.input.clone() }),
                        _ => {
                            let mut known = Vec::with_capacity(arguments.len());
                            for argument in arguments {
                                match self.static_value(argument, url, depth + 1)? {
                                    Some(value) => known.push(value),
                                    None => return Ok(None),
                                }
                            }
                            match function.as_str() {
                                "random" => (known.len() >= 2 && known.iter().all(|v| *v == known[0])).then(|| known[0].clone()),
                                "select" => {
                                    let choices: Vec<String> = known.into_iter().filter(|v| !v.is_empty()).collect();
                                    if choices.is_empty() {
                                        Some(String::new())
                                    } else if self.context.answer_indices.contains_key(id) || self.context.answers.contains_key(id) {
                                        Some(self.context.answer(id, Some(&choices))?)
                                    } else {
                                        None
                                    }
                                }
                                "json" | "xml" if known.len() <= 1 => return Ok(None),
                                "regex" if known.len() <= 2 => return Ok(None),
                                "base64" | "json" | "regex" | "xml" => Some(self.invoke(&function, id, &known, url)?),
                                _ => return Ok(None),
                            }
                        }
                    };
                    match value {
                        Some(value) => value,
                        None => return Ok(None),
                    }
                }
            };
            limits::require(output.len() + value.len() <= limits::INTERMEDIATE, "expansion_limit")?;
            output.push_str(&value);
        }
        let result = limits::bounded(output, limits::INTERMEDIATE)?;
        self.materialize(&result)?;
        Ok(Some(result))
    }
    fn preflight_answers(&mut self) -> R<()> {
        self.for_each_call(|this, field, node| this.preflight_call(field, node))
    }
    fn preflight_call(&mut self, field: &str, node: &Node) -> R<()> {
        {
            let Node::Call { name, id, arguments } = node else { return Ok(()) };
            let url = url_field(field);
            let mut function = self.static_value(name, url, 0)?.map(|n| n.to_lowercase());
            if function.is_none() {
                let bound = self.context.function_names.get(id).ok_or(Fail::Code("function_binding_required"))?;
                function = Some(bound.to_lowercase());
            }
            let function = function.unwrap_or_default();
            if let Some(binding) = self.context.function_names.get(id) {
                limits::require(function.eq_ignore_ascii_case(binding), "function_binding_mismatch")?;
            }
            limits::require(
                minimum_arity(&function).is_some_and(|minimum| arguments.len() >= minimum),
                "function_name_or_arity",
            )?;
            match function.as_str() {
                "inputbox" | "prompt" => limits::require(
                    self.context.answers.contains_key(id) && !self.context.answer_indices.contains_key(id),
                    "answer_required",
                )?,
                "select" => {
                    let mut known = Vec::new();
                    let mut dynamic = false;
                    for argument in arguments {
                        match self.static_value(argument, url, 0)? {
                            Some(value) => known.push(value),
                            None => dynamic = true,
                        }
                    }
                    if dynamic {
                        limits::require(
                            self.context.answer_indices.contains_key(id) && !self.context.answers.contains_key(id),
                            "selection_index_required",
                        )?;
                    } else {
                        let choices: Vec<String> = known.into_iter().filter(|v| !v.is_empty()).collect();
                        if !choices.is_empty() {
                            self.context.answer(id, Some(&choices))?;
                        }
                    }
                }
                _ => {}
            }
        }
        Ok(())
    }
    fn add_secret(&mut self, value: &str) -> R<()> {
        if value.is_empty() || !self.secret_set.insert(value.to_owned()) {
            return Ok(());
        }
        self.secrets.push(value.to_owned());
        self.secret_bytes += value.len();
        limits::require(self.secrets.len() <= 128 && self.secret_bytes <= 256 * 1024, "authentication_limit")
    }
    fn add_known_authentication(&mut self, name: &str, value: &str) -> R<()> {
        if value.is_empty() {
            return Ok(());
        }
        self.add_secret(value)?;
        if let Some(space) = value.find(' ').filter(|s| *s > 0) {
            let scheme = &value[..space];
            if (scheme.eq_ignore_ascii_case("Bearer") || scheme.eq_ignore_ascii_case("Basic")) && space + 1 < value.len() {
                self.add_secret(&value[space + 1..])?;
            }
        }
        let set_cookie = name.eq_ignore_ascii_case("Set-Cookie");
        if name.eq_ignore_ascii_case("Cookie") || set_cookie {
            for (count, pair) in value.split(';').enumerate() {
                if set_cookie && count > 0 {
                    break;
                }
                limits::require(count < 128, "cookie_pair_limit")?;
                if let Some(equals) = pair.find('=').filter(|e| *e > 0) {
                    let token = pair[equals + 1..].trim();
                    if !token.is_empty() {
                        self.add_secret(token)?;
                        if token.len() >= 2 && token.starts_with('"') && token.ends_with('"') {
                            self.add_secret(&token[1..token.len() - 1])?;
                        }
                    }
                }
            }
        }
        Ok(())
    }
    fn scan_body(&mut self, body: &str, marker: Option<&str>) -> R<()> {
        let mut found = Vec::new();
        body_authentication(body, marker, &mut |name, value| {
            found.push((name.to_owned(), value.to_owned()));
            Ok(())
        })?;
        for (name, value) in found {
            self.add_known_authentication(&name, &value)?;
        }
        Ok(())
    }
    fn collect_inspection_authentication(&mut self) -> R<()> {
        let fields: Vec<String> = self
            .trees
            .iter()
            .map(|(field, _)| field.clone())
            .filter(|f| f.starts_with("Headers.") || f.starts_with("Parameters.") || f.starts_with("Arguments."))
            .collect();
        for field in fields {
            if !sensitive_field(&field) {
                continue;
            }
            let tree = self.tree(&field);
            if let Some(value) = self.static_value(&tree, false, 0)? {
                let name = &field[field.find('.').unwrap_or(0) + 1..];
                let name = if field.starts_with("Parameters.") { delivered_query_name(name)? } else { name.to_owned() };
                self.add_known_authentication(&name, &value)?;
            }
        }
        if matches!(self.profile.body, "JSON" | "XML") {
            let data = self.data(true)?;
            let marker = self.context.unknown_marker().to_owned();
            self.scan_body(&data, Some(&marker))?;
        }
        limits::require(self.secret_bytes <= 256 * 1024, "inspection_auth_limit")
    }
    fn sanitized_inspection_answers(&mut self) -> R<serde_json::Value> {
        self.collect_inspection_authentication()?;
        let secrets = &self.secrets;
        let contains = |value: &str| {
            let (uri, form) = (inspection_projection(value, false), inspection_projection(value, true));
            secrets.iter().any(|s| !s.is_empty() && (value.contains(s) || uri.contains(s) || form.contains(s)))
        };
        let mut answers = std::mem::take(&mut self.budget.inspection);
        for answer in &mut answers {
            let Some(object) = answer.as_object_mut() else { continue };
            let mut redacted = object.get("redacted").and_then(serde_json::Value::as_bool).unwrap_or(false);
            for key in ["title", "default_text"] {
                if object.get(key).and_then(serde_json::Value::as_str).is_some_and(contains) {
                    object.insert(key.into(), "[redacted]".into());
                    redacted = true;
                }
            }
            if let Some(serde_json::Value::Array(choices)) = object.get_mut("choices") {
                for choice in choices.iter_mut() {
                    if choice.as_str().is_some_and(contains) {
                        *choice = "[redacted]".into();
                        redacted = true;
                    }
                }
            }
            if object.contains_key("redacted") {
                object.insert("redacted".into(), redacted.into());
            }
        }
        Ok(serde_json::Value::Array(answers))
    }
    fn inspect(&mut self) -> R<serde_json::Value> {
        let mut contracts = Vec::new();
        self.for_each_call(|this, field, node| {
            if let Some(contract) = this.contract(field, node)? {
                contracts.push(contract);
            }
            Ok(())
        })?;
        let calls = self.budget.calls;
        let answers = self.sanitized_inspection_answers()?;
        Ok(serde_json::json!({
            "calls": calls,
            "answers": answers,
            "answer_contracts": contracts,
            "functions": FUNCTIONS.iter().map(|(n, _)| *n).collect::<Vec<_>>(),
        }))
    }
    fn contract(&mut self, field: &str, node: &Node) -> R<Option<serde_json::Value>> {
        {
            let Node::Call { name, id, arguments } = node else { return Ok(None) };
            let url = url_field(field);
            let mut function = self.static_value(name, url, 0)?.map(|n| n.to_lowercase());
            let computed = literal_value(name).is_none();
            if function.is_none() {
                function = self.context.function_names.get(id).map(|b| b.to_lowercase());
            }
            let answer_kind = match function.as_deref() {
                Some("inputbox" | "prompt") => Some("value"),
                Some("select") => {
                    let mut dynamic = false;
                    for argument in arguments {
                        dynamic |= self.static_value(argument, url, 0)?.is_none();
                    }
                    Some(if dynamic { "index" } else { "value_or_index" })
                }
                _ => None,
            };
            Ok((answer_kind.is_some() || computed).then(|| {
                serde_json::json!({
                    "id": id,
                    "stage": if response_field(field) { "response" } else { "request" },
                    "function": function.as_deref().filter(|f| minimum_arity(f).is_some()),
                    "answer_kind": answer_kind,
                    "function_name_binding_required": computed && function.is_none(),
                })
            }))
        }
    }
    fn evaluate(&mut self, tree: &[Node], url: bool, limit: usize) -> R<String> {
        self.context.tick()?;
        let mut text = String::new();
        for node in tree {
            let value = match node {
                Node::Literal(literal) => literal.clone(),
                Node::Call { name, id, arguments } => {
                    let function = self.evaluate(name, url, 128)?.to_lowercase();
                    let mut args = Vec::with_capacity(arguments.len());
                    for argument in arguments {
                        args.push(self.evaluate(argument, url, limits::INTERMEDIATE)?);
                    }
                    self.invoke(&function, id, &args, url)?
                }
            };
            limits::require(text.len() + value.len() <= limit, "expansion_limit")?;
            text.push_str(&value);
        }
        let result = limits::bounded(text, limit)?;
        self.materialize(&result)?;
        Ok(result)
    }
    fn field(&mut self, field: &str, url: bool, limit: usize) -> R<String> {
        let tree = self.tree(field);
        self.evaluate(&tree, url, limit)
    }
    fn invoke(&mut self, name: &str, id: &str, args: &[String], url: bool) -> R<String> {
        self.context.tick()?;
        if let Some(binding) = self.context.function_names.get(id) {
            limits::require(name.eq_ignore_ascii_case(binding), "function_binding_mismatch")?;
        }
        limits::require(minimum_arity(name).is_some_and(|m| args.len() >= m), "function_name_or_arity")?;
        let first = args.first().cloned().unwrap_or_default();
        let context = &self.context;
        Ok(match name {
            "filename" => if url { escape_data(&context.filename) } else { context.filename.clone() },
            "input" => if url { escape_data(&context.input) } else { context.input.clone() },
            "response" => context.response.clone(),
            "responseurl" => context.response_url.clone(),
            "header" => context.headers.get(&first.to_lowercase()).cloned().unwrap_or_default(),
            "base64" => base64::engine::general_purpose::STANDARD.encode(first.as_bytes()),
            "random" => args[random_below(args.len())].clone(),
            "select" => {
                let choices: Vec<String> = args.iter().filter(|v| !v.is_empty()).cloned().collect();
                if choices.is_empty() { String::new() } else { context.answer(id, Some(&choices))? }
            }
            "prompt" | "inputbox" => {
                let title = if args.is_empty() { "Input" } else { first.as_str() };
                let answer = context.answer(id, None)?;
                let field = &id[..id.rfind('#').unwrap_or(id.len())];
                if !answer.is_empty() && (sensitive(title) || sensitive_field(field)) {
                    self.add_secret(&answer)?;
                }
                answer
            }
            "outputbox" => {
                let message = if args.len() > 1 { args[1].clone() } else { first.clone() };
                if !message.is_empty() {
                    limits::require(self.context.events.len() < 16, "output_event_limit")?;
                    let title = if args.len() > 1 && !first.is_empty() { first } else { "Output".into() };
                    self.context.events.push(Event {
                        title: limits::bounded(title, 256)?,
                        text: limits::bounded(message, 4096)?,
                    });
                }
                String::new()
            }
            "json" => {
                let (input, path) = if args.len() > 1 { (first, args[1].clone()) } else { (context.response.clone(), first) };
                if input.is_empty() || path.is_empty() {
                    return Ok(String::new());
                }
                limits::bounded(input.clone(), limits::INTERMEDIATE)?;
                limits::bounded(path.clone(), 8192)?;
                let path = if path.starts_with("$.") { path } else { format!("$.{path}") };
                let document = json::parse(&input, false)?;
                match json::select_token(&document, &path)? {
                    Some(value) => json::token_string(&value)?,
                    None => String::new(),
                }
            }
            "regex" => {
                let (input, pattern, group) = if args.len() > 2 {
                    (first, args[1].clone(), args[2].clone())
                } else {
                    (context.response.clone(), first, args.get(1).cloned().unwrap_or_default())
                };
                if input.is_empty() || pattern.is_empty() {
                    return Ok(String::new());
                }
                limits::bounded(input.clone(), limits::INTERMEDIATE)?;
                limits::bounded(pattern.clone(), 8192)?;
                let regex = regex(&pattern)?;
                let Some(captures) = regex_find(&regex, &input)? else { return Ok(String::new()) };
                let found = if group.is_empty() {
                    captures.get(0)
                } else if let Ok(index) = group.trim().parse::<i64>() {
                    usize::try_from(index).ok().and_then(|i| captures.get(i))
                } else {
                    captures.name(&group)
                };
                found.map(|m| m.as_str().to_owned()).unwrap_or_default()
            }
            "xml" => {
                let (input, path) = if args.len() > 1 { (first, args[1].clone()) } else { (context.response.clone(), first) };
                if input.is_empty() || path.is_empty() {
                    return Ok(String::new());
                }
                limits::bounded(input.clone(), limits::INTERMEDIATE)?;
                limits::bounded(path.clone(), 8192)?;
                let document = xml::parse(&input)?;
                xml::require_depth(&document, "xml_depth")?;
                xml::select_single(&document, &path)?.unwrap_or_default()
            }
            _ => return Err(Fail::Code("function_name")),
        })
    }
    fn map(&mut self, prefix: &str) -> R<Pairs> {
        let keys: Vec<String> = match prefix {
            "Headers." => &self.profile.headers,
            "Arguments." => &self.profile.arguments,
            _ => &self.profile.parameters,
        }
        .iter()
        .map(|(k, _)| k.clone())
        .collect();
        let mut output = Vec::new();
        let mut bytes = 0;
        for key in keys {
            let value = self.field(&format!("{prefix}{key}"), false, limits::TEXT)?;
            bytes += key.len() + value.len();
            limits::require(bytes <= 65536, "map_expansion_limit")?;
            output.push((key, value));
        }
        Ok(output)
    }
    fn data(&mut self, frozen_only: bool) -> R<String> {
        if !frozen_only && let Some(data) = &self.resolved_data {
            return Ok(data.clone());
        }
        let source = names::expand(&self.profile.data, &self.context, false, frozen_only)?;
        if frozen_only {
            let marker = self.context.unknown_marker().to_owned();
            let mut source = replace_ci(&replace_ci(&source, "{input}", &marker), "{filename}", &marker);
            if source.trim_start().starts_with('<') {
                source = source.replace(&marker, &format!("x{marker}"));
            }
            return Ok(source);
        }
        let encode = |text: &str| if self.profile.body == "JSON" { json_escape(text) } else { xml_escape(text) };
        let (input, filename) = (encode(&self.context.input), encode(&self.context.filename));
        let chars: Vec<char> = source.chars().collect();
        let mut result = String::new();
        let mut i = 0;
        while i < chars.len() {
            let (replacement, consumed) = if starts_ci(&chars, i, "{input}") {
                (input.as_str(), 7)
            } else if starts_ci(&chars, i, "{filename}") {
                (filename.as_str(), 10)
            } else {
                let mut buffer = [0; 4];
                result.push_str(chars[i].encode_utf8(&mut buffer));
                limits::require(result.len() <= limits::BODY, "body_limit")?;
                i += 1;
                continue;
            };
            limits::require(result.len() + replacement.len() <= limits::BODY, "body_limit")?;
            result.push_str(replacement);
            i += consumed;
        }
        let result = limits::bounded(result, limits::BODY)?;
        self.resolved_data = Some(result.clone());
        Ok(result)
    }
    fn prepare(&mut self) -> R<Prepared> {
        self.profile.admit_kind(&self.context.kind)?;
        self.preflight_answers()?;
        let mut url = self.field("RequestURL", true, 8192)?;
        let lower = url.to_ascii_lowercase();
        if !lower.starts_with("http://") && !lower.starts_with("https://") {
            url = format!("https://{url}");
        }
        let headers = self.map("Headers.")?;
        let arguments = self.map("Arguments.")?;
        let parameters = self.map("Parameters.")?;
        let suffix = parameters
            .iter()
            .map(|(k, v)| if v.is_empty() { k.clone() } else { format!("{k}={}", url_encode(v)) })
            .collect::<Vec<_>>()
            .join("&");
        if !suffix.is_empty() {
            url.push(if url.contains('?') { '&' } else { '?' });
            url.push_str(&suffix);
        }
        let url = limits::bounded(url, 8192)?;
        for (key, value) in headers.iter().chain(arguments.iter()) {
            if sensitive(key) && !value.is_empty() {
                self.add_known_authentication(key, value)?;
            }
        }
        for (key, value) in &parameters {
            let name = delivered_query_name(key)?;
            if sensitive(&name) && !value.is_empty() {
                self.add_known_authentication(&name, value)?;
            }
        }
        let kind = self.context.kind.clone();
        let attach = self.profile.body == "MultipartFormData"
            && (matches!(kind.as_str(), "image" | "file") || kind == "text" && !self.profile.field.is_empty());
        let mut data = Vec::new();
        if matches!(self.profile.body, "JSON" | "XML") {
            data = self.data(false)?.into_bytes();
        }
        if self.profile.body == "FormURLEncoded" {
            data = arguments
                .iter()
                .map(|(k, v)| format!("{}={}", escape_data(k).replace("%20", "+"), escape_data(v).replace("%20", "+")))
                .collect::<Vec<_>>()
                .join("&")
                .into_bytes();
        }
        limits::require(data.len() <= limits::BODY, "body_limit")?;
        if !data.is_empty() {
            let text = String::from_utf8_lossy(&data).into_owned();
            self.scan_body(&text, None)?;
        } else if self.profile.body == "Binary" && kind == "text" {
            let input = self.context.input.clone();
            self.scan_body(&input, None)?;
        }
        Ok(Prepared {
            url,
            method: self.profile.method,
            body: self.profile.body,
            file_field: self.profile.field.clone(),
            attach_file: attach,
            headers: Ordered(headers),
            arguments: Ordered(arguments),
            data: base64::engine::general_purpose::STANDARD.encode(&data),
            events: std::mem::take(&mut self.context.events),
            secrets: std::mem::take(&mut self.secrets),
        })
    }
    fn response(&mut self) -> R<ResponseResult> {
        let success = (200..=299).contains(&self.context.status);
        let (mut link, mut thumbnail, mut deletion, mut error) = Default::default();
        if success {
            link = if self.profile.link.is_empty() { self.context.response.clone() } else { self.field("URL", true, limits::TEXT)? };
            thumbnail = self.field("ThumbnailURL", true, limits::TEXT)?;
            deletion = self.field("DeletionURL", true, limits::TEXT)?;
        } else if !self.profile.error.is_empty() {
            error = self.field("ErrorMessage", true, 4096)?;
        }
        Ok(ResponseResult {
            success,
            status: self.context.status,
            url: limits::bounded(link, 2048)?,
            thumbnail: limits::bounded(thumbnail, 2048)?,
            deletion: limits::bounded(deletion, 2048)?,
            error_message: error,
            events: std::mem::take(&mut self.context.events),
            secrets: std::mem::take(&mut self.secrets),
        })
    }
}

/// Ordered string map preserving profile order in JSON output.
struct Ordered(Pairs);
impl Serialize for Ordered {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        use serde::ser::SerializeMap;
        let mut map = serializer.serialize_map(Some(self.0.len()))?;
        for (key, value) in &self.0 {
            map.serialize_entry(key, value)?;
        }
        map.end()
    }
}
#[derive(Serialize)]
struct Prepared {
    url: String,
    method: &'static str,
    body: &'static str,
    file_field: String,
    attach_file: bool,
    headers: Ordered,
    arguments: Ordered,
    data: String,
    events: Vec<Event>,
    secrets: Vec<String>,
}
#[derive(Serialize)]
struct ResponseResult {
    success: bool,
    status: i64,
    url: String,
    thumbnail: String,
    deletion: String,
    error_message: String,
    events: Vec<Event>,
    secrets: Vec<String>,
}

fn run(request: &[u8], cancel: Option<crate::UploadCancellation>, deadline: Instant) -> R<Vec<u8>> {
    limits::require(request.len() <= limits::INPUT, "request_limit")?;
    let text = std::str::from_utf8(request).map_err(|_| Fail::invalid())?;
    let root = json::parse(text, true)?;
    limits::require(integer(root.get("version")) == Some(1), "protocol_version")?;
    let stage = string(root.get("stage"), 16)?;
    let profile = Profile::read(&string(root.get("profile_json"), 16384)?)?;
    let context = Context::read(&root, cancel, deadline)?;
    let mut interpreter = Interpreter::new(profile, context)?;
    fn ok<T: Serialize>(result: &T) -> R<Vec<u8>> {
        #[derive(Serialize)]
        struct Envelope<'a, T> {
            ok: bool,
            result: &'a T,
        }
        serde_json::to_vec(&Envelope { ok: true, result }).map_err(|_| Fail::invalid())
    }
    match stage.as_str() {
        "inspect" => ok(&interpreter.inspect()?),
        "prepare" => ok(&interpreter.prepare()?),
        "response" => ok(&interpreter.response()?),
        _ => Err(Fail::Code("stage")),
    }
}

/// Interpret one request. Policy refusals become `{ok:false, code}` envelopes;
/// cancellation and the five-second deadline are returned as errors.
pub(crate) fn interpret(
    request: &[u8],
    cancel: &crate::UploadCancellation,
) -> anyhow::Result<Zeroizing<Vec<u8>>> {
    cancel.check()?;
    let deadline = Instant::now() + Duration::from_secs(5);
    let envelope = |code: &str| serde_json::to_vec(&serde_json::json!({"ok": false, "code": code})).unwrap_or_default();
    let output = match run(request, Some(cancel.clone()), deadline) {
        Ok(bytes) if bytes.len() <= limits::OUTPUT => bytes,
        Ok(_) => envelope("output_limit"),
        Err(Fail::Code(code)) => envelope(code),
        Err(Fail::Abort(error)) => return Err(error),
    };
    cancel.check()?;
    Ok(Zeroizing::new(output))
}

#[cfg(test)]
#[path = "tests.rs"]
mod tests;
