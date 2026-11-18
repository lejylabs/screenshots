//! Install, upgrade, roll back or remove a verified user-owned Screenshots package.
//!
//! Online updates require an explicitly pinned Ed25519 public key and a detached
//! signature. No server, signing identity or periodic update task is assumed.
//! The on-disk layout and metadata are compatible with installations created by
//! the former `install-user.py`.
pub mod archive;

use anyhow::{Context, Result, anyhow};
use serde_json::{Map, Value, json};
use sha2::{Digest, Sha256};
use std::{
    collections::{BTreeMap, BTreeSet},
    fs::{self, File, OpenOptions},
    io::{Read, Write},
    path::{Path, PathBuf},
    time::{Duration, Instant},
};

pub const OWNER: &str = "screenshots-user-install-v1";
pub const MAX_ARCHIVE: u64 = 512 * 1024 * 1024;
pub const MAX_EXPANDED: u64 = 1024 * 1024 * 1024;
pub const MAX_FILES: usize = 10000;
pub const MAX_MANIFEST: u64 = 4 * 1024 * 1024;
const MAX_TRANSACTION: u64 = MAX_MANIFEST + 32768;
const MAX_VERSIONS: usize = 128;
const STATE: &str = ".screenshots-install.json";
const MANIFEST: &str = ".screenshots-version.json";
const TRANSACTION: &str = ".screenshots-transaction.json";
const PUBLIC_KEY: &str = "update-public-key.pem";

/// Ordered command result, printed like Python's `json.dumps`.
pub type Outcome = Vec<(&'static str, Value)>;

macro_rules! require {
    ($condition:expr, $message:expr) => {
        if !$condition {
            return Err(anyhow!($message));
        }
    };
}


// ---- JSON ----

struct Strict(usize);
impl<'de> serde::de::DeserializeSeed<'de> for Strict {
    type Value = Value;
    fn deserialize<D: serde::Deserializer<'de>>(self, deserializer: D) -> std::result::Result<Value, D::Error> {
        if self.0 > 128 {
            return Err(serde::de::Error::custom("JSON depth limit"));
        }
        deserializer.deserialize_any(self)
    }
}
impl<'de> serde::de::Visitor<'de> for Strict {
    type Value = Value;
    fn expecting(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        f.write_str("JSON without duplicate fields")
    }
    fn visit_bool<E>(self, v: bool) -> std::result::Result<Value, E> {
        Ok(v.into())
    }
    fn visit_i64<E>(self, v: i64) -> std::result::Result<Value, E> {
        Ok(v.into())
    }
    fn visit_u64<E>(self, v: u64) -> std::result::Result<Value, E> {
        Ok(v.into())
    }
    fn visit_f64<E: serde::de::Error>(self, v: f64) -> std::result::Result<Value, E> {
        serde_json::Number::from_f64(v).map(Value::Number).ok_or_else(|| E::custom("Invalid number"))
    }
    fn visit_str<E>(self, v: &str) -> std::result::Result<Value, E> {
        Ok(v.into())
    }
    fn visit_unit<E>(self) -> std::result::Result<Value, E> {
        Ok(Value::Null)
    }
    fn visit_seq<A: serde::de::SeqAccess<'de>>(self, mut seq: A) -> std::result::Result<Value, A::Error> {
        let mut values = Vec::new();
        while let Some(value) = seq.next_element_seed(Strict(self.0 + 1))? {
            values.push(value);
        }
        Ok(Value::Array(values))
    }
    fn visit_map<A: serde::de::MapAccess<'de>>(self, mut map: A) -> std::result::Result<Value, A::Error> {
        let mut values = Map::new();
        while let Some(key) = map.next_key::<String>()? {
            if values.contains_key(&key) {
                return Err(serde::de::Error::custom("Duplicate JSON field"));
            }
            let value = map.next_value_seed(Strict(self.0 + 1))?;
            values.insert(key, value);
        }
        Ok(Value::Object(values))
    }
}
/// Parse JSON, rejecting duplicate fields and trailing data.
pub fn parse_json(bytes: &[u8]) -> Result<Value> {
    use serde::de::DeserializeSeed;
    let mut decoder = serde_json::Deserializer::from_slice(bytes);
    let value = Strict(0).deserialize(&mut decoder).map_err(|_| anyhow!("Invalid or duplicate JSON"))?;
    decoder.end().map_err(|_| anyhow!("Trailing JSON data"))?;
    Ok(value)
}
pub fn read_json(path: &Path, limit: u64) -> Result<Value> {
    parse_json(&regular_bytes(path, limit)?)
}
fn escape_into(out: &mut String, text: &str) {
    out.push('"');
    for ch in text.chars() {
        match ch {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            '\u{8}' => out.push_str("\\b"),
            '\u{c}' => out.push_str("\\f"),
            ch if (ch as u32) < 0x20 || (ch as u32) > 0x7e && ch != '\u{7f}' => {
                let mut units = [0u16; 2];
                for unit in ch.encode_utf16(&mut units) {
                    out.push_str(&format!("\\u{unit:04x}"));
                }
            }
            ch => out.push(ch),
        }
    }
    out.push('"');
}
/// Python `json.dumps` with ASCII escaping; `compact` selects `(",", ":")` separators
/// and sorted keys, as the installer metadata files use.
pub fn dumps(value: &Value, compact: bool) -> String {
    fn write(out: &mut String, value: &Value, compact: bool) {
        let (item, key) = if compact { (",", ":") } else { (", ", ": ") };
        match value {
            Value::Null => out.push_str("null"),
            Value::Bool(b) => out.push_str(if *b { "true" } else { "false" }),
            Value::Number(n) => out.push_str(&n.to_string()),
            Value::String(s) => escape_into(out, s),
            Value::Array(items) => {
                out.push('[');
                for (index, entry) in items.iter().enumerate() {
                    if index > 0 {
                        out.push_str(item);
                    }
                    write(out, entry, compact);
                }
                out.push(']');
            }
            Value::Object(map) => {
                out.push('{');
                let mut entries: Vec<_> = map.iter().collect();
                if compact {
                    entries.sort_by(|a, b| a.0.cmp(b.0));
                }
                for (index, (name, entry)) in entries.into_iter().enumerate() {
                    if index > 0 {
                        out.push_str(item);
                    }
                    escape_into(out, name);
                    out.push_str(key);
                    write(out, entry, compact);
                }
                out.push('}');
            }
        }
    }
    let mut out = String::new();
    write(&mut out, value, compact);
    out
}
pub fn outcome_json(outcome: &Outcome) -> String {
    let mut out = String::from("{");
    for (index, (key, value)) in outcome.iter().enumerate() {
        if index > 0 {
            out.push_str(", ");
        }
        escape_into(&mut out, key);
        out.push_str(": ");
        out.push_str(&dumps(value, false));
    }
    out.push('}');
    out
}
fn json_bytes(value: &Value) -> Vec<u8> {
    (dumps(value, true) + "\n").into_bytes()
}
fn keys(value: &Value, expected: &[&str]) -> bool {
    value.as_object().is_some_and(|map| map.len() == expected.len() && expected.iter().all(|k| map.contains_key(*k)))
}
fn text<'a>(value: &'a Value, key: &str) -> Result<&'a str> {
    value.get(key).and_then(Value::as_str).with_context(|| format!("Missing text field {key}"))
}
fn opt_text<'a>(value: &'a Value, key: &str) -> Option<&'a str> {
    value.get(key).and_then(Value::as_str)
}
fn with(value: &Value, changes: &[(&str, Value)]) -> Value {
    let mut copy = value.clone();
    for (key, change) in changes {
        copy[*key] = change.clone();
    }
    copy
}

// ---- Validation ----

fn digits(value: &str) -> bool {
    !value.is_empty() && value.bytes().all(|b| b.is_ascii_digit())
}
pub fn hex64(value: &str) -> bool {
    value.len() == 64 && value.bytes().all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f'))
}
/// Bounded numeric `major.minor.patch`.
pub fn version(value: &str) -> Result<(u64, u64, u64)> {
    let parts: Vec<&str> = value.split('.').collect();
    let valid = parts.len() == 3
        && parts.iter().all(|p| digits(p) && p.len() <= 9 && (p.len() == 1 || !p.starts_with('0')));
    require!(valid, "Package version must be a bounded numeric major.minor.patch");
    Ok((parts[0].parse()?, parts[1].parse()?, parts[2].parse()?))
}
pub fn target(value: &str) -> Result<&str> {
    let word = |w: &str| !w.is_empty() && w.bytes().all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'_');
    let parts: Vec<&str> = value.split('-').collect();
    require!(value.len() <= 96 && (3..=5).contains(&parts.len()) && parts.iter().all(|p| word(p)), "Invalid package target");
    Ok(value)
}
fn identifier_shape(value: &str) -> bool {
    match value.split_once('-') {
        Some((version, rest)) => {
            !version.is_empty()
                && version.bytes().all(|b| b.is_ascii_digit() || b == b'.')
                && !rest.is_empty()
                && rest.bytes().all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'_' || b == b'-')
        }
        None => false,
    }
}
pub fn local_target() -> Result<String> {
    let architecture = match std::env::consts::ARCH {
        "x86_64" => "x86_64",
        "aarch64" => "aarch64",
        _ => return Err(anyhow!("Unsupported install architecture")),
    };
    Ok(match std::env::consts::OS {
        "windows" => format!("{architecture}-pc-windows-msvc"),
        "macos" => format!("{architecture}-apple-darwin"),
        "linux" => format!("{architecture}-unknown-linux-gnu"),
        _ => return Err(anyhow!("Unsupported install platform")),
    })
}
pub fn executable_name(target: &str) -> &'static str {
    if target.contains("windows") { "screenshots.exe" } else { "screenshots" }
}
/// Relative archive path: at least a root folder plus one component, no traversal
/// or Windows-reserved names.
pub fn safe_name(name: &str) -> Result<Vec<String>> {
    require!(name.len() <= 4096 && !name.contains('\\') && !name.contains('\0'), "Invalid archive path");
    let parts: Vec<&str> = name.split('/').collect();
    let valid = !name.starts_with('/')
        && (2..=24).contains(&parts.len())
        && parts.iter().all(|p| !matches!(*p, "" | "." | "..") && !p.contains(':'));
    require!(valid, "Archive path escapes package root");
    let reserved = |part: &str| {
        let stem = part.split('.').next().unwrap_or_default().to_ascii_uppercase();
        matches!(stem.as_str(), "CON" | "PRN" | "AUX" | "NUL")
            || (stem.len() == 4 && (stem.starts_with("COM") || stem.starts_with("LPT")) && matches!(stem.as_bytes()[3], b'1'..=b'9'))
    };
    require!(parts.iter().all(|p| *p == p.trim_end_matches([' ', '.']) && !reserved(p)), "Archive contains a reserved native path");
    Ok(parts.into_iter().map(str::to_owned).collect())
}

// ---- Filesystem ----

fn exists_or_link(path: &Path) -> bool {
    fs::symlink_metadata(path).is_ok()
}
fn is_real_dir(path: &Path) -> bool {
    fs::symlink_metadata(path).is_ok_and(|m| m.is_dir())
}
fn is_real_file(path: &Path) -> bool {
    fs::symlink_metadata(path).is_ok_and(|m| m.is_file())
}
/// Open without following a final symlink and confirm the opened object is the
/// named regular file.
pub fn open_regular(path: &Path) -> Result<(File, fs::Metadata)> {
    let named = fs::symlink_metadata(path).context("Input metadata is unavailable")?;
    require!(named.file_type().is_file(), "Input must be a regular file, not a symlink");
    let mut options = OpenOptions::new();
    options.read(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK);
    }
    let file = options.open(path).context("Could not open input")?;
    let metadata = file.metadata()?;
    require!(metadata.is_file(), "Input must be a regular file");
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        require!(metadata.dev() == named.dev() && metadata.ino() == named.ino(), "Input path changed while opening");
    }
    Ok((file, metadata))
}
pub fn regular_bytes(path: &Path, limit: u64) -> Result<Vec<u8>> {
    let (file, metadata) = open_regular(path)?;
    require!(metadata.len() <= limit, "Input metadata is unavailable or excessive");
    let mut data = Vec::with_capacity(metadata.len() as usize);
    file.take(metadata.len() + 1).read_to_end(&mut data)?;
    require!(data.len() as u64 == metadata.len(), "Input was truncated");
    Ok(data)
}
fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}
pub fn sha_bytes(bytes: &[u8]) -> String {
    hex(&Sha256::digest(bytes))
}
/// SHA-256 of a bounded regular file.
pub fn sha(path: &Path) -> Result<String> {
    let (mut file, metadata) = open_regular(path)?;
    require!(metadata.len() <= MAX_EXPANDED, "Hash input must be a bounded regular file");
    let mut hasher = Sha256::new();
    let mut remaining = metadata.len();
    let mut buffer = vec![0u8; 65536];
    while remaining > 0 {
        let want = buffer.len().min(remaining as usize);
        let read = file.read(&mut buffer[..want])?;
        require!(read > 0, "Hash input was truncated");
        hasher.update(&buffer[..read]);
        remaining -= read as u64;
    }
    Ok(hex(&hasher.finalize()))
}
pub fn sync_directory(path: &Path) -> Result<()> {
    #[cfg(unix)]
    File::open(path)?.sync_all()?;
    #[cfg(not(unix))]
    let _ = path;
    Ok(())
}
pub fn set_mode(path: &Path, mode: u32) -> Result<()> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(path, fs::Permissions::from_mode(mode))?;
    }
    #[cfg(not(unix))]
    let _ = (path, mode);
    Ok(())
}
fn create_private_dir(path: &Path) -> Result<()> {
    fs::create_dir(path)?;
    set_mode(path, 0o700)
}
fn create_new(path: &Path) -> Result<File> {
    Ok(OpenOptions::new().write(true).create_new(true).open(path)?)
}
fn remove_if_present(path: &Path) {
    let _ = fs::remove_file(path);
}
fn unique(prefix: &str) -> String {
    format!("{prefix}{}", uuid::Uuid::new_v4())
}

/// Private temporary directory removed on drop.
pub struct TempDir(PathBuf);
impl TempDir {
    pub fn new(prefix: &str) -> Result<Self> {
        Self::new_in(&std::env::temp_dir(), prefix)
    }
    pub fn new_in(parent: &Path, prefix: &str) -> Result<Self> {
        let path = parent.join(unique(prefix));
        create_private_dir(&path)?;
        Ok(Self(path))
    }
    pub fn path(&self) -> &Path {
        &self.0
    }
}
impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

fn atomic_file(path: &Path, payload: &[u8], expected: Option<&Value>) -> Result<()> {
    // Unpublished bytes must not make a first install or recovered root unowned.
    let name = path.file_name().and_then(|n| n.to_str()).unwrap_or_default();
    let parent = path.parent().context("Managed path has no parent")?;
    let staging_parent = if matches!(name, STATE | TRANSACTION | PUBLIC_KEY) {
        parent.parent().context("Managed root has no parent")?
    } else {
        parent
    };
    let temporary = staging_parent.join(unique(".screenshots-write-"));
    let result = (|| {
        {
            let mut output = create_new(&temporary)?;
            set_mode(&temporary, 0o600)?;
            output.write_all(payload)?;
            output.sync_all()?;
        }
        match expected {
            None => {
                require!(!exists_or_link(path), "Managed metadata appeared before publication");
                fs::hard_link(&temporary, path)?;
            }
            Some(expected) => {
                let limit = if name == TRANSACTION { MAX_TRANSACTION } else { 16384 };
                require!(read_json(path, limit)? == *expected, "Managed metadata changed; existing installation preserved");
                fs::rename(&temporary, path)?;
            }
        }
        sync_directory(parent)
    })();
    remove_if_present(&temporary);
    result
}
fn atomic_json(path: &Path, value: &Value, expected: Option<&Value>) -> Result<()> {
    let payload = json_bytes(value);
    let limit = if path.file_name().is_some_and(|n| n == TRANSACTION) { MAX_TRANSACTION } else { MAX_MANIFEST };
    require!(payload.len() as u64 <= limit, "Managed manifest exceeds its byte limit");
    atomic_file(path, &payload, expected)
}

// ---- Lock ----

/// One OS lock serializes publication, trust changes, rollback and removal.
struct InstallationLock {
    file: File,
}
fn strip_verbatim(path: PathBuf) -> PathBuf {
    #[cfg(windows)]
    {
        let text = path.to_string_lossy();
        if let Some(rest) = text.strip_prefix(r"\\?\")
            && !rest.starts_with("UNC\\")
        {
            return PathBuf::from(rest.to_owned());
        }
    }
    path
}
impl InstallationLock {
    fn acquire(root: &Path, create_parent: bool) -> Result<Self> {
        let requested = std::path::absolute(root)?;
        let parent = requested.parent().context("Install root has no parent")?;
        if create_parent {
            fs::create_dir_all(parent)?;
        }
        let parent = strip_verbatim(parent.canonicalize()?);
        let name = requested.file_name().context("Install root has no name")?;
        let root = parent.join(name);
        let text = root.to_string_lossy().into_owned();
        let normalized = if cfg!(any(windows, target_os = "macos")) { text.to_lowercase() } else { text };
        let identity = &sha_bytes(normalized.as_bytes())[..32];
        let path = parent.join(format!(".screenshots-install-lock-{identity}"));
        let mut options = OpenOptions::new();
        options.read(true).write(true).create(true).truncate(false);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600).custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK);
        }
        let mut file = options.open(&path).context("Could not open installation lock")?;
        let metadata = file.metadata()?;
        let named = fs::symlink_metadata(&path)?;
        require!(metadata.is_file() && metadata.len() <= 8192, "Installation lock is not a bounded private regular file");
        require!(named.is_file(), "Installation lock path changed or is a symlink");
        #[cfg(unix)]
        {
            use std::os::unix::fs::MetadataExt;
            require!(metadata.nlink() == 1, "Installation lock is not a bounded private regular file");
            require!(metadata.dev() == named.dev() && metadata.ino() == named.ino(), "Installation lock path changed or is a symlink");
            // SAFETY: getuid has no preconditions.
            require!(metadata.uid() == unsafe { libc::getuid() }, "Installation lock has a different owner");
        }
        let deadline = Instant::now() + Duration::from_secs(10);
        loop {
            match file.try_lock() {
                Ok(()) => break,
                Err(fs::TryLockError::WouldBlock) => {
                    require!(Instant::now() < deadline, "Another installation operation holds the lock");
                    std::thread::sleep(Duration::from_millis(25));
                }
                Err(fs::TryLockError::Error(error)) => return Err(error.into()),
            }
        }
        let mut marker = format!("{OWNER}\n").into_bytes();
        marker.extend_from_slice(normalized.as_bytes());
        marker.push(b'\n');
        require!(marker.len() <= 8192, "Installation root exceeds the lock identity limit");
        let mut existing = Vec::new();
        (&mut file).take(8193).read_to_end(&mut existing)?;
        require!(existing.is_empty() || existing == marker, "Installation lock contains unrelated data");
        if existing.is_empty() {
            file.write_all(&marker)?;
            file.sync_all()?;
        }
        Ok(Self { file })
    }
}
impl Drop for InstallationLock {
    fn drop(&mut self) {
        // Keep the tiny lock file: unlinking it lets a waiter lock an obsolete inode.
        let _ = self.file.unlock();
    }
}

// ---- Package extraction ----

pub struct Extracted {
    pub info: Value,
    pub files: Vec<Value>,
    pub executable: &'static str,
}
pub fn extract(package: &Path, destination: &Path) -> Result<Extracted> {
    let (file, metadata) = open_regular(package)?;
    require!(metadata.len() <= MAX_ARCHIVE, "Package must be a regular archive of at most 512 MiB");
    let mut seen = BTreeSet::new();
    let mut roots = BTreeSet::new();
    let (mut total, mut path_bytes) = (0u64, 0usize);
    let mut files = Vec::new();
    let mut copy = |member: archive::Member<'_>| -> Result<()> {
        let parts = safe_name(&member.name)?;
        require!(
            !seen.contains(&member.name) && seen.len() < MAX_FILES && member.size <= MAX_ARCHIVE,
            "Duplicate or excessive archive entry"
        );
        seen.insert(member.name.clone());
        path_bytes += member.name.len();
        require!(path_bytes <= 1024 * 1024, "Package path metadata exceeds its limit");
        roots.insert(parts[0].clone());
        total += member.size;
        require!(roots.len() == 1 && total <= MAX_EXPANDED, "Package exceeds expanded limit or has multiple roots");
        let relative = parts[1..].join("/");
        let last = parts.last().map(String::as_str).unwrap_or_default();
        require!(last != STATE && last != MANIFEST, "Package collides with installer metadata");
        let output = parts[1..].iter().fold(destination.to_path_buf(), |path, part| path.join(part));
        fs::create_dir_all(output.parent().context("Archive member has no parent")?)?;
        let mut stream = create_new(&output)?;
        let mut hasher = Sha256::new();
        let mut remaining = member.size;
        let mut buffer = vec![0u8; 65536];
        while remaining > 0 {
            let want = buffer.len().min(remaining as usize);
            let read = member.reader.read(&mut buffer[..want])?;
            require!(read > 0, "Truncated archive member");
            hasher.update(&buffer[..read]);
            stream.write_all(&buffer[..read])?;
            remaining -= read as u64;
        }
        require!(member.reader.read(&mut [0u8; 1])? == 0, "Archive member exceeded its declared size");
        stream.sync_all()?;
        set_mode(&output, if member.mode & 0o111 != 0 { 0o755 } else { 0o644 })?;
        files.push(json!({"path": relative, "sha256": hex(&hasher.finalize()), "size": member.size}));
        Ok(())
    };
    let zip = package.extension().is_some_and(|e| e.eq_ignore_ascii_case("zip"));
    if zip {
        archive::read_zip(file, &archive::ZipLimits { entries: MAX_FILES, member: MAX_ARCHIVE }, &mut copy)?;
    } else {
        let limits = archive::TarLimits { entries: MAX_FILES, member: MAX_ARCHIVE, total: MAX_EXPANDED + 16 * 1024 * 1024 };
        archive::read_tar_gz(file, &limits, &mut copy)?;
    }
    require!(!files.is_empty(), "Package contains no files");
    let info = read_json(&destination.join("BUILD-INFO.json"), 16384)?;
    require!(["version", "target", "binary_sha256"].iter().all(|k| info.get(*k).is_some()), "Package build information is incomplete");
    version(text(&info, "version")?)?;
    target(text(&info, "target")?)?;
    require!(text(&info, "target")? == local_target()?, "Package target does not match this machine");
    let executable = executable_name(text(&info, "target")?);
    let binary = destination.join(executable);
    require!(is_real_file(&binary) && sha(&binary)? == text(&info, "binary_sha256")?, "Package executable hash does not match build information");
    for required in ["LICENSE", "README.md", "Cargo.lock", "THIRD_PARTY_NOTICES.md"] {
        require!(is_real_file(&destination.join(required)), "Package is missing required notices or metadata");
    }
    Ok(Extracted { info, files, executable })
}

// ---- Managed state ----

fn root_path(value: &Path, create: bool) -> Result<PathBuf> {
    let path = std::path::absolute(value)?;
    require!(!fs::symlink_metadata(&path).is_ok_and(|m| m.file_type().is_symlink()), "Install root must not be a symlink");
    if create && !exists_or_link(&path) {
        fs::create_dir_all(&path)?;
        set_mode(&path, 0o700)?;
    }
    require!(is_real_dir(&path), "Install root must be a directory");
    Ok(path)
}
fn installation_absent(value: &Path) -> Result<bool> {
    let path = std::path::absolute(value)?;
    let Ok(metadata) = fs::symlink_metadata(&path) else { return Ok(true) };
    require!(!metadata.file_type().is_symlink(), "Install root must not be a symlink");
    require!(metadata.is_dir(), "Install root must be a directory");
    Ok(fs::read_dir(&path)?.next().is_none())
}
fn entry_names(root: &Path) -> Result<BTreeSet<String>> {
    fs::read_dir(root)?
        .map(|entry| Ok(entry?.file_name().to_string_lossy().into_owned()))
        .collect()
}
fn state(root: &Path, optional: bool) -> Result<Option<Value>> {
    let path = root.join(STATE);
    if optional && !path.exists() {
        require!(!exists_or_link(&path) && fs::read_dir(root)?.next().is_none(), "Existing nonempty directory is not an owned installation");
        return Ok(None);
    }
    Ok(Some(validate_state(read_json(&path, 16384)?)?))
}
fn required_state(root: &Path) -> Result<Value> {
    state(root, false)?.context("Installation has no current version")
}
fn validate_state(value: Value) -> Result<Value> {
    require!(value.is_object(), "Invalid install metadata object");
    require!(
        keys(&value, &["owner", "current", "previous", "target", "key_sha256"]) && opt_text(&value, "owner") == Some(OWNER),
        "Install metadata is not owned by Screenshots"
    );
    let target_name = text(&value, "target")?.to_owned();
    target(&target_name)?;
    for key in ["current", "previous"] {
        match &value[key] {
            Value::Null => {}
            Value::String(identifier) => {
                require!(identifier_shape(identifier), "Invalid managed version path");
                let suffix = format!("-{target_name}");
                require!(identifier.ends_with(&suffix), "Managed version target mismatch");
                version(&identifier[..identifier.len() - suffix.len()])?;
            }
            _ => return Err(anyhow!("Invalid managed version path")),
        }
    }
    require!(!value["current"].is_null(), "Installation has no current version");
    require!(value["key_sha256"].is_null() || opt_text(&value, "key_sha256").is_some_and(hex64), "Invalid pinned update key hash");
    Ok(value)
}
fn transaction(root: &Path) -> Result<Option<Value>> {
    let path = root.join(TRANSACTION);
    if !exists_or_link(&path) {
        return Ok(None);
    }
    let value = read_json(&path, MAX_TRANSACTION)?;
    let kind = opt_text(&value, "kind").unwrap_or_default().to_owned();
    require!(
        value.is_object() && opt_text(&value, "owner") == Some(OWNER) && matches!(kind.as_str(), "install" | "pin" | "uninstall"),
        "Recovery intent is not owned by Screenshots"
    );
    let mut expected = vec!["owner", "kind", "prior", "new"];
    expected.extend(match kind.as_str() {
        "install" => &["stage", "manifest_sha256"][..],
        "pin" => &["public_key"][..],
        _ => &["remaining", "active", "launcher"][..],
    });
    require!(keys(&value, &expected), "Recovery intent has unknown or missing fields");
    if !value["prior"].is_null() {
        validate_state(value["prior"].clone())?;
    }
    validate_state(value["new"].clone())?;
    match kind.as_str() {
        "install" => {
            let stage = opt_text(&value, "stage").unwrap_or_default();
            let valid_stage = stage.strip_prefix(".screenshots-stage-").is_some_and(|rest| {
                rest.len() == 36 && rest.bytes().all(|b| b.is_ascii_digit() || matches!(b, b'a'..=b'f' | b'-'))
            });
            require!(valid_stage && opt_text(&value, "manifest_sha256").is_some_and(hex64), "Invalid install recovery intent");
        }
        "pin" => {
            require!(!value["prior"].is_null() && value["prior"]["key_sha256"].is_null(), "Invalid trust-pin recovery intent");
            require!(
                ["owner", "current", "previous", "target"].iter().all(|k| value["new"][*k] == value["prior"][*k]),
                "Trust-pin intent cannot change installed versions"
            );
            let public = decode_key(&value)?;
            require!(
                public.len() <= 4096 && Some(sha_bytes(&public).as_str()) == opt_text(&value["new"], "key_sha256"),
                "Pinned-key intent does not match its public key"
            );
        }
        _ => {
            let remaining = value["remaining"].as_array();
            require!(
                value["prior"] == value["new"] && remaining.is_some_and(|r| r.len() <= MAX_VERSIONS),
                "Invalid uninstall recovery intent"
            );
            let remaining = remaining.cloned().unwrap_or_default();
            let unique: BTreeSet<_> = remaining.iter().filter_map(Value::as_str).collect();
            require!(
                remaining.iter().all(|i| i.as_str().is_some_and(identifier_shape)) && unique.len() == remaining.len(),
                "Invalid uninstall version list"
            );
            let active = &value["active"];
            require!(
                active.is_null()
                    || keys(active, &["identifier", "manifest"]) && !remaining.is_empty() && active["identifier"] == remaining[0],
                "Invalid active uninstall version"
            );
            let launcher = &value["launcher"];
            require!(
                keys(launcher, &["name", "sha256", "size"])
                    && opt_text(launcher, "name") == Some(executable_name(text(&value["prior"], "target")?))
                    && opt_text(launcher, "sha256").is_some_and(hex64)
                    && launcher["size"].as_u64().is_some_and(|s| s <= MAX_ARCHIVE),
                "Invalid uninstall launcher intent"
            );
        }
    }
    Ok(Some(value))
}
fn decode_key(intent: &Value) -> Result<Vec<u8>> {
    use base64::Engine;
    base64::engine::general_purpose::STANDARD
        .decode(text(intent, "public_key")?)
        .map_err(|_| anyhow!("Invalid pinned public key encoding"))
}
fn finish_transaction(root: &Path) -> Result<()> {
    fs::remove_file(root.join(TRANSACTION))?;
    sync_directory(root)
}
fn replace_state(root: &Path, value: &Value) -> Result<()> {
    let path = root.join(STATE);
    let expected = if exists_or_link(&path) { Some(read_json(&path, 16384)?) } else { None };
    atomic_json(&path, value, expected.as_ref())
}
fn read_state_file(root: &Path) -> Result<Option<Value>> {
    let path = root.join(STATE);
    if path.exists() { Ok(Some(read_json(&path, 16384)?)) } else { Ok(None) }
}

fn verify_version(root: &Path, identifier: &str) -> Result<Value> {
    require!(is_real_dir(&root.join("versions")), "Version directory must not be a symlink");
    verify_folder(&root.join("versions").join(identifier))
}
fn parents_of(path: &str) -> impl Iterator<Item = String> + '_ {
    path.match_indices('/').map(move |(index, _)| path[..index].to_owned())
}
/// Every regular file and directory below `folder`, relative with `/` separators.
fn walk(folder: &Path) -> Result<(BTreeSet<String>, BTreeSet<String>)> {
    let (mut files, mut directories) = (BTreeSet::new(), BTreeSet::new());
    let mut stack = vec![(folder.to_path_buf(), String::new())];
    while let Some((directory, prefix)) = stack.pop() {
        for entry in fs::read_dir(&directory)? {
            let entry = entry?;
            let name = format!("{prefix}{}", entry.file_name().to_string_lossy());
            let metadata = fs::symlink_metadata(entry.path())?;
            if metadata.is_dir() {
                directories.insert(name.clone());
                stack.push((entry.path(), format!("{name}/")));
            } else {
                require!(metadata.is_file(), "Unexpected special file or symlink inside installed version");
                files.insert(name);
            }
            require!(files.len() + directories.len() <= MAX_FILES * 4, "Installed version contains too many entries");
        }
    }
    Ok((files, directories))
}
fn verify_folder(folder: &Path) -> Result<Value> {
    require!(is_real_dir(folder), "Managed version is unavailable");
    let manifest = read_json(&folder.join(MANIFEST), MAX_MANIFEST)?;
    require!(
        keys(&manifest, &["owner", "version", "target", "executable", "files", "archive_sha256"])
            && opt_text(&manifest, "owner") == Some(OWNER),
        "Version metadata is not owned by Screenshots"
    );
    let members = manifest["files"].as_array().context("Invalid installed member metadata")?;
    require!(members.len() <= MAX_FILES, "Excessive installed manifest");
    let mut expected = BTreeSet::from([MANIFEST.to_owned()]);
    let mut expected_directories = BTreeSet::new();
    for item in members {
        require!(keys(item, &["path", "sha256", "size"]), "Invalid installed member metadata");
        let path = text(item, "path")?;
        let parts = safe_name(&format!("root/{path}"))?;
        let mut parent = folder.to_path_buf();
        for component in &parts[1..parts.len() - 1] {
            parent.push(component);
            require!(is_real_dir(&parent), "Installed parent path changed; preserve the installation");
        }
        let file = parent.join(&parts[parts.len() - 1]);
        let size = item["size"].as_u64();
        require!(
            is_real_file(&file) && fs::metadata(&file)?.len() == size.unwrap_or(u64::MAX) && sha(&file)? == text(item, "sha256")?,
            "Installed file changed; preserve it and inspect the installation"
        );
        expected.insert(path.to_owned());
        expected_directories.extend(parents_of(path));
    }
    let (actual, actual_directories) = walk(folder)?;
    require!(actual == expected && actual_directories == expected_directories, "Installed version contains unowned files or directories; preserved");
    require!(expected.contains(text(&manifest, "executable")?), "Invalid installed executable");
    Ok(manifest)
}
fn version_names(root: &Path) -> Result<Vec<String>> {
    let versions = root.join("versions");
    require!(is_real_dir(&versions), "Version directory is unavailable");
    let mut names = Vec::new();
    for entry in fs::read_dir(&versions)? {
        require!(names.len() < MAX_VERSIONS, "Retained version count exceeds its limit");
        let name = entry?.file_name().to_string_lossy().into_owned();
        require!(identifier_shape(&name), "Unknown entry in managed version directory");
        names.push(name);
    }
    names.sort();
    Ok(names)
}
fn publish_entry(root: &Path, folder: &Path, executable: &str, previous: Option<&str>) -> Result<()> {
    let entry = root.join(executable);
    if exists_or_link(&entry) {
        require!(previous.is_some() && is_real_file(&entry), "Launcher conflicts with existing file");
        let previous = previous.unwrap_or_default();
        let old = verify_version(root, previous)?;
        let old_launcher = root.join("versions").join(previous).join(text(&old, "executable")?);
        require!(sha(&entry)? == sha(&old_launcher)?, "Installed launcher changed; preserved");
    }
    let parent = root.parent().context("Install root has no parent")?;
    let temporary = parent.join(unique(".screenshots-launch-"));
    let result = (|| {
        // Prefer one filesystem link; copying remains streamed for filesystems without links.
        if fs::hard_link(folder.join(executable), &temporary).is_err() {
            let (mut source, _) = open_regular(&folder.join(executable))?;
            let mut output = create_new(&temporary)?;
            std::io::copy(&mut source, &mut output)?;
            output.sync_all()?;
            set_mode(&temporary, 0o755)?;
        }
        if previous.is_none() {
            fs::hard_link(&temporary, &entry)?;
        } else {
            fs::rename(&temporary, &entry)?;
        }
        sync_directory(root)
    })();
    remove_if_present(&temporary);
    result
}

// ---- Operations ----

/// Install or upgrade from a local archive with its `.sha256` sidecar.
pub fn install(package: &Path, root: &Path) -> Result<Outcome> {
    let _lock = InstallationLock::acquire(root, true)?;
    install_package(package, root, None)
}
fn install_package(package: &Path, root: &Path, verified: Option<&str>) -> Result<Outcome> {
    let package = std::path::absolute(package)?;
    let (mut source, metadata) = open_regular(&package)?;
    require!(metadata.len() <= MAX_ARCHIVE, "Package must be a bounded regular archive");
    let digest = match verified {
        Some(digest) => digest.to_owned(),
        None => {
            let name = package.file_name().context("Package has no name")?.to_string_lossy().into_owned();
            let checksum = package.with_file_name(format!("{name}.sha256"));
            require!(is_real_file(&checksum) && fs::metadata(&checksum)?.len() <= 1024, "Local package checksum sidecar is required");
            let text = String::from_utf8(regular_bytes(&checksum, 1024)?)?;
            let text = text.trim();
            let valid = text.len() == 66 + name.len() && hex64(&text[..64]) && text[64..66] == *"  " && text[66..] == name;
            require!(valid, "Invalid local package checksum");
            text[..64].to_owned()
        }
    };
    let temporary = TempDir::new("screenshots-package-")?;
    let zip = package.extension().is_some_and(|e| e.eq_ignore_ascii_case("zip"));
    let snapshot = temporary.path().join(if zip { "package.zip" } else { "package.tar.gz" });
    let mut hasher = Sha256::new();
    {
        let mut output = create_new(&snapshot)?;
        let mut remaining = metadata.len();
        let mut buffer = vec![0u8; 65536];
        while remaining > 0 {
            let want = buffer.len().min(remaining as usize);
            let read = source.read(&mut buffer[..want])?;
            require!(read > 0, "Package was truncated during snapshot");
            hasher.update(&buffer[..read]);
            output.write_all(&buffer[..read])?;
            remaining -= read as u64;
        }
        output.sync_all()?;
    }
    require!(hex(&hasher.finalize()) == digest, "Package checksum mismatch");
    install_snapshot(&snapshot, root, &digest)
}
fn same_device(root: &Path) -> Result<bool> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        let parent = root.parent().context("Install root has no parent")?;
        Ok(fs::metadata(root)?.dev() == fs::metadata(parent)?.dev())
    }
    #[cfg(not(unix))]
    {
        let _ = root;
        Ok(true)
    }
}
fn install_snapshot(package: &Path, root_value: &Path, digest: &str) -> Result<Outcome> {
    let root = root_path(root_value, true)?;
    require!(same_device(&root)?, "Install root must share its parent filesystem for recoverable publication");
    require!(!exists_or_link(&root.join(TRANSACTION)), "An interrupted operation requires explicit recovery");
    let prior = state(&root, true)?;
    if let Some(prior) = &prior {
        verify_version(&root, text(prior, "current")?)?;
    }
    let parent = root.parent().context("Install root has no parent")?.to_path_buf();
    let staging = parent.join(unique(".screenshots-stage-"));
    create_private_dir(&staging)?;
    let result = (|| -> Result<Outcome> {
        let extracted = extract(package, &staging)?;
        let info = &extracted.info;
        let (info_version, info_target) = (text(info, "version")?.to_owned(), text(info, "target")?.to_owned());
        let identifier = format!("{info_version}-{info_target}");
        if let Some(prior) = &prior
            && text(prior, "current")? == identifier
        {
            let existing = verify_version(&root, &identifier)?;
            require!(text(&existing, "archive_sha256")? == digest, "Same installed version has different contents");
            return Ok(vec![("status", "unchanged".into()), ("version", info_version.into()), ("target", info_target.into())]);
        }
        if let Some(prior) = &prior {
            require!(version_names(&root)?.len() < MAX_VERSIONS, "Retained version limit reached; remove an older owned installation explicitly");
            let current = verify_version(&root, text(prior, "current")?)?;
            require!(
                info_target == text(prior, "target")? && version(&info_version)? > version(text(&current, "version")?)?,
                "Upgrade must use a newer version of the same target; use rollback for the previous version"
            );
        }
        let manifest = json!({"owner": OWNER, "version": info_version, "target": info_target, "executable": extracted.executable,
                              "files": extracted.files, "archive_sha256": digest});
        atomic_json(&staging.join(MANIFEST), &manifest, None)?;
        let versions = root.join("versions");
        require!(!fs::symlink_metadata(&versions).is_ok_and(|m| m.file_type().is_symlink()), "Version directory must not be a symlink");
        let folder = versions.join(&identifier);
        require!(!exists_or_link(&folder), "Version destination already exists; inspect it");
        let previous = prior.as_ref().map(|p| p["current"].clone()).unwrap_or(Value::Null);
        let key = prior.as_ref().map(|p| p["key_sha256"].clone()).unwrap_or(Value::Null);
        let value = json!({"owner": OWNER, "current": identifier, "previous": previous, "target": info_target, "key_sha256": key});
        let stage_name = staging.file_name().context("Stage has no name")?.to_string_lossy().into_owned();
        let intent = json!({"owner": OWNER, "kind": "install", "prior": prior.clone().unwrap_or(Value::Null), "new": value,
                            "stage": stage_name, "manifest_sha256": sha(&staging.join(MANIFEST))?});
        let (_, directories) = walk(&staging)?;
        let mut ordered: Vec<String> = directories.into_iter().collect();
        ordered.sort_by_key(|d| std::cmp::Reverse(d.matches('/').count()));
        for directory in ordered {
            sync_directory(&staging.join(directory))?;
        }
        sync_directory(&staging)?;
        sync_directory(&parent)?;
        atomic_json(&root.join(TRANSACTION), &intent, None)?;
        if !exists_or_link(&versions) {
            create_private_dir(&versions)?;
        }
        require!(is_real_dir(&versions), "Version directory must not be a symlink");
        sync_directory(&root)?;
        fs::rename(&staging, &folder)?;
        sync_directory(&versions)?;
        sync_directory(&parent)?;
        atomic_json(&root.join(STATE), &value, prior.as_ref())?;
        publish_entry(&root, &folder, extracted.executable, prior.as_ref().and_then(|p| opt_text(p, "current")))?;
        finish_transaction(&root)?;
        let status = if prior.is_none() { "installed" } else { "upgraded" };
        Ok(vec![
            ("status", status.into()),
            ("version", info_version.into()),
            ("target", info_target.into()),
            ("previous_retained", prior.is_some().into()),
        ])
    })();
    if staging.exists() && !root.join(TRANSACTION).exists() {
        let _ = fs::remove_dir_all(&staging);
    }
    // An owned intent retains staging/published versions for explicit recovery.
    result
}

pub fn status(root: &Path) -> Result<Outcome> {
    let _lock = InstallationLock::acquire(root, false)?;
    status_unlocked(root)
}
fn status_unlocked(root_value: &Path) -> Result<Outcome> {
    if installation_absent(root_value)? {
        return Ok(vec![("status", "not_installed".into())]);
    }
    let root = root_path(root_value, false)?;
    if let Some(pending) = transaction(&root)? {
        let current = text(&pending["new"], "current")?;
        return Ok(vec![
            ("status", "recovery_required".into()),
            ("operation", pending["kind"].clone()),
            ("version", current.split('-').next().unwrap_or_default().into()),
            ("state_published", root.join(STATE).exists().into()),
        ]);
    }
    let value = required_state(&root)?;
    let current_id = text(&value, "current")?;
    let current = verify_version(&root, current_id)?;
    let executable = text(&current, "executable")?;
    let entry = root.join(executable);
    require!(
        is_real_file(&entry) && sha(&entry)? == sha(&root.join("versions").join(current_id).join(executable))?,
        "Launcher and current-version metadata differ; inspect interrupted upgrade"
    );
    let interrupted: Vec<Value> = entry_names(&root)?
        .into_iter()
        .filter(|n| n.starts_with(".screenshots-stage-") || n.starts_with(".screenshots-write-") || n.starts_with(".screenshots-launch-"))
        .map(Value::from)
        .collect();
    Ok(vec![
        ("status", "installed".into()),
        ("version", current["version"].clone()),
        ("target", current["target"].clone()),
        ("previous_retained", (!value["previous"].is_null()).into()),
        ("update_key_pinned", (!value["key_sha256"].is_null()).into()),
        ("unfinished", Value::Array(interrupted)),
    ])
}

pub fn rollback(root_value: &Path) -> Result<Outcome> {
    let _lock = InstallationLock::acquire(root_value, false)?;
    let root = root_path(root_value, false)?;
    require!(transaction(&root)?.is_none(), "An interrupted operation requires explicit recovery");
    let value = required_state(&root)?;
    let previous_id = opt_text(&value, "previous").context("No previous version is retained")?;
    let current_id = text(&value, "current")?;
    let previous = verify_version(&root, previous_id)?;
    verify_version(&root, current_id)?;
    publish_entry(&root, &root.join("versions").join(previous_id), text(&previous, "executable")?, Some(current_id))?;
    let changed = with(&value, &[("current", previous_id.into()), ("previous", current_id.into())]);
    atomic_json(&root.join(STATE), &changed, Some(&value))?;
    Ok(vec![("status", "rolled_back".into()), ("version", previous["version"].clone()), ("target", previous["target"].clone())])
}

/// Reconcile the stable executable after interruption, preserving both versions.
pub fn recover(root_value: &Path, choice: &str) -> Result<Outcome> {
    require!(matches!(choice, "current" | "previous"), "Recovery choice must be current or previous");
    let _lock = InstallationLock::acquire(root_value, false)?;
    if installation_absent(root_value)? {
        require!(choice == "current", "No previous installation is available");
        return Ok(uninstalled());
    }
    let root = root_path(root_value, false)?;
    if let Some(pending) = transaction(&root)? {
        return recover_transaction(&root, pending, choice);
    }
    let value = required_state(&root)?;
    let selected = opt_text(&value, choice).context("Requested recovery version is unavailable")?.to_owned();
    let chosen = verify_version(&root, &selected)?;
    let entry = root.join(text(&chosen, "executable")?);
    let mut old_identifier = None;
    if exists_or_link(&entry) {
        require!(is_real_file(&entry), "Recovery launcher is not a regular file");
        let digest = sha(&entry)?;
        for identifier in [opt_text(&value, "current"), opt_text(&value, "previous")].into_iter().flatten() {
            let old = verify_version(&root, identifier)?;
            if digest == sha(&root.join("versions").join(identifier).join(text(&old, "executable")?))? {
                old_identifier = Some(identifier.to_owned());
                break;
            }
        }
        require!(old_identifier.is_some(), "Recovery found an unowned launcher; preserved");
    }
    publish_entry(&root, &root.join("versions").join(&selected), text(&chosen, "executable")?, old_identifier.as_deref())?;
    if choice == "previous" {
        let changed = with(&value, &[("current", value["previous"].clone()), ("previous", value["current"].clone())]);
        atomic_json(&root.join(STATE), &changed, Some(&value))?;
    }
    Ok(vec![("status", "recovered".into()), ("version", chosen["version"].clone()), ("target", chosen["target"].clone())])
}
fn uninstalled() -> Outcome {
    vec![("status", "uninstalled".into()), ("user_data", "preserved".into())]
}
fn recover_transaction(root: &Path, pending: Value, choice: &str) -> Result<Outcome> {
    let current_state = read_state_file(root)?;
    let kind = text(&pending, "kind")?.to_owned();
    if kind == "uninstall" {
        require!(current_state.is_none() || current_state.as_ref() == Some(&pending["prior"]), "Installation state differs from uninstall intent; preserved");
        require!(choice == "current", "An interrupted uninstall can only finish; deleted owned files cannot be restored");
        return finish_uninstall(root, pending);
    }
    let as_state = |value: &Value| if value.is_null() { None } else { Some(value.clone()) };
    let mut accepted = vec![as_state(&pending["prior"]), as_state(&pending["new"])];
    if kind == "install" && !pending["prior"].is_null() {
        accepted.push(Some(with(&pending["prior"], &[("previous", pending["new"]["current"].clone())])));
    }
    require!(accepted.contains(&current_state), "Installation state differs from recovery intent; preserved");
    if kind == "pin" {
        let public = decode_key(&pending)?;
        let key = root.join(PUBLIC_KEY);
        if exists_or_link(&key) {
            require!(regular_bytes(&key, 4096)? == public, "Pinned key differs from recovery intent; preserved");
        }
        if choice == "current" {
            if !key.exists() {
                atomic_file(&key, &public, None)?;
            }
            replace_state(root, &pending["new"])?;
        } else {
            if key.exists() {
                fs::remove_file(&key)?;
                sync_directory(root)?;
            }
            replace_state(root, &pending["prior"])?;
        }
        finish_transaction(root)?;
        return Ok(vec![("status", "recovered".into()), ("operation", "pin".into()), ("update_key_pinned", (choice == "current").into())]);
    }
    let new_current = text(&pending["new"], "current")?.to_owned();
    let mut selected_state = if choice == "current" { pending["new"].clone() } else { pending["prior"].clone() };
    require!(!selected_state.is_null(), "First installation has no previous version; recover current");
    if choice == "previous" {
        selected_state = with(&selected_state, &[("previous", new_current.clone().into())]);
    }
    let versions = root.join("versions");
    if !exists_or_link(&versions) {
        create_private_dir(&versions)?;
    }
    require!(is_real_dir(&versions), "Recovery version directory is unavailable");
    sync_directory(root)?;
    let published = versions.join(&new_current);
    let stage = text(&pending, "stage")?;
    let mut staging = root.parent().context("Install root has no parent")?.join(stage);
    let old_staging = root.join(stage);
    require!(!(exists_or_link(&staging) && exists_or_link(&old_staging)), "Multiple staged versions exist; preserved");
    if exists_or_link(&old_staging) {
        staging = old_staging;
    }
    require!(!(published.exists() && staging.exists()), "Both staged and published version exist; preserved");
    let manifest_sha = text(&pending, "manifest_sha256")?;
    if !published.exists() {
        verify_folder(&staging)?;
        require!(sha(&staging.join(MANIFEST))? == manifest_sha, "Staged version differs from recovery intent");
        fs::rename(&staging, &published)?;
        sync_directory(&versions)?;
        sync_directory(staging.parent().context("Stage has no parent")?)?;
    }
    let manifest = verify_version(root, &new_current)?;
    require!(
        sha(&published.join(MANIFEST))? == manifest_sha
            && format!("{}-{}", text(&manifest, "version")?, text(&manifest, "target")?) == new_current,
        "Published version differs from recovery intent"
    );
    let selected_id = text(&selected_state, "current")?.to_owned();
    let selected = verify_version(root, &selected_id)?;
    let entry = root.join(text(&selected, "executable")?);
    let mut previous_identifier = None;
    if exists_or_link(&entry) {
        require!(is_real_file(&entry), "Recovery launcher is not a regular file");
        let digest = sha(&entry)?;
        let mut candidates = BTreeSet::from([new_current]);
        if !pending["prior"].is_null() {
            candidates.extend(["current", "previous"].iter().filter_map(|k| opt_text(&pending["prior"], k)).map(str::to_owned));
        }
        for identifier in candidates {
            let candidate = verify_version(root, &identifier)?;
            if sha(&root.join("versions").join(&identifier).join(text(&candidate, "executable")?))? == digest {
                previous_identifier = Some(identifier);
                break;
            }
        }
        require!(previous_identifier.is_some(), "Recovery found an unowned launcher; preserved");
    }
    replace_state(root, &selected_state)?;
    publish_entry(root, &root.join("versions").join(&selected_id), text(&selected, "executable")?, previous_identifier.as_deref())?;
    finish_transaction(root)?;
    Ok(vec![("status", "recovered".into()), ("version", selected["version"].clone()), ("target", selected["target"].clone())])
}

pub fn uninstall(root_value: &Path) -> Result<Outcome> {
    let _lock = InstallationLock::acquire(root_value, false)?;
    if installation_absent(root_value)? {
        return Ok(uninstalled());
    }
    let root = root_path(root_value, false)?;
    if let Some(pending) = transaction(&root)? {
        require!(opt_text(&pending, "kind") == Some("uninstall"), "A different interrupted operation requires explicit recovery");
        return finish_uninstall(&root, pending);
    }
    let value = required_state(&root)?;
    status_unlocked(&root)?;
    let identifiers = version_names(&root)?;
    for identifier in &identifiers {
        verify_version(&root, identifier)?;
    }
    let launcher = executable_name(text(&value, "target")?);
    let mut allowed = BTreeSet::from([STATE.to_owned(), "versions".to_owned(), launcher.to_owned()]);
    if let Some(key) = opt_text(&value, "key_sha256") {
        require!(sha(&root.join(PUBLIC_KEY))? == key, "Pinned key changed; installation preserved");
        allowed.insert(PUBLIC_KEY.to_owned());
    }
    require!(entry_names(&root)? == allowed, "Install root contains unowned or unfinished files; preserved");
    let entry = root.join(launcher);
    let pending = json!({"owner": OWNER, "kind": "uninstall", "prior": value, "new": value,
                         "remaining": identifiers, "active": null,
                         "launcher": {"name": launcher, "size": fs::metadata(&entry)?.len(), "sha256": sha(&entry)?}});
    atomic_json(&root.join(TRANSACTION), &pending, None)?;
    finish_uninstall(&root, pending)
}
type Members = (BTreeMap<String, Value>, BTreeSet<String>);
fn removal_members(manifest: &Value) -> Result<Members> {
    let files_value = manifest["files"].as_array();
    require!(
        keys(manifest, &["owner", "version", "target", "executable", "files", "archive_sha256"])
            && opt_text(manifest, "owner") == Some(OWNER)
            && files_value.is_some_and(|f| f.len() <= MAX_FILES),
        "Uninstall manifest is not owned or bounded"
    );
    let (mut files, mut directories) = (BTreeMap::new(), BTreeSet::new());
    for item in files_value.into_iter().flatten() {
        require!(keys(item, &["path", "sha256", "size"]), "Invalid uninstall member");
        let path = text(item, "path")?.to_owned();
        safe_name(&format!("root/{path}"))?;
        require!(
            !files.contains_key(&path)
                && item["size"].as_u64().is_some_and(|s| s <= MAX_ARCHIVE)
                && opt_text(item, "sha256").is_some_and(hex64),
            "Invalid uninstall member admission"
        );
        directories.extend(parents_of(&path));
        files.insert(path, item.clone());
    }
    require!(files.contains_key(text(manifest, "executable")?), "Uninstall executable is not owned");
    Ok((files, directories))
}
fn verify_removal_tree(folder: &Path, manifest: &Value) -> Result<Members> {
    let (files, directories) = removal_members(manifest)?;
    if !exists_or_link(folder) {
        return Ok((files, directories));
    }
    require!(is_real_dir(folder), "Uninstall version path changed");
    let mut stack = vec![(folder.to_path_buf(), String::new())];
    while let Some((directory, prefix)) = stack.pop() {
        for entry in fs::read_dir(&directory)? {
            let entry = entry?;
            let relative = format!("{prefix}{}", entry.file_name().to_string_lossy());
            let metadata = fs::symlink_metadata(entry.path())?;
            if metadata.is_dir() {
                require!(directories.contains(&relative), "Uninstall found an unowned directory; preserved");
                stack.push((entry.path(), format!("{relative}/")));
            } else {
                require!(metadata.is_file(), "Uninstall found a special file or symlink; preserved");
                if relative == MANIFEST {
                    require!(read_json(&entry.path(), MAX_MANIFEST)? == *manifest, "Version metadata changed during uninstall; preserved");
                } else {
                    let item = files.get(&relative);
                    require!(
                        item.is_some_and(|i| i["size"].as_u64() == Some(metadata.len()))
                            && Some(sha(&entry.path())?.as_str()) == item.and_then(|i| opt_text(i, "sha256")),
                        "Uninstall found an unowned or modified file; preserved"
                    );
                }
            }
        }
    }
    Ok((files, directories))
}
fn stamp(metadata: &fs::Metadata) -> (u64, Option<std::time::SystemTime>, u64, u64) {
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        (metadata.len(), metadata.modified().ok(), metadata.ino(), metadata.ctime() as u64)
    }
    #[cfg(not(unix))]
    {
        (metadata.len(), metadata.modified().ok(), 0, 0)
    }
}
fn delete_owned_file(path: &Path, item: &Value) -> Result<()> {
    let Ok(metadata) = fs::symlink_metadata(path) else { return Ok(()) };
    require!(
        metadata.is_file() && Some(metadata.len()) == item["size"].as_u64() && Some(sha(path)?.as_str()) == opt_text(item, "sha256"),
        "Uninstall member changed before removal; preserved"
    );
    let current = fs::symlink_metadata(path)?;
    require!(stamp(&metadata) == stamp(&current), "Uninstall member changed during verification; preserved");
    fs::remove_file(path)?;
    Ok(())
}
fn finish_uninstall(root: &Path, mut pending: Value) -> Result<Outcome> {
    let value = pending["prior"].clone();
    let state_file = root.join(STATE);
    if exists_or_link(&state_file) {
        require!(read_json(&state_file, 16384)? == value, "Install state differs from uninstall intent; preserved");
    }
    let launcher_name = text(&pending["launcher"], "name")?.to_owned();
    let mut allowed = BTreeSet::from([STATE.to_owned(), TRANSACTION.to_owned(), "versions".to_owned(), launcher_name.clone()]);
    if !value["key_sha256"].is_null() {
        allowed.insert(PUBLIC_KEY.to_owned());
    }
    require!(entry_names(root)?.is_subset(&allowed), "Uninstall found unowned root entries; preserved");
    let versions = root.join("versions");
    let remaining = |p: &Value| p["remaining"].as_array().cloned().unwrap_or_default();
    if !remaining(&pending).is_empty() {
        require!(is_real_dir(&versions), "Uninstall version directory changed");
    }
    if exists_or_link(&versions) {
        require!(is_real_dir(&versions), "Uninstall version directory changed; preserved");
        let listed: BTreeSet<String> = remaining(&pending).iter().filter_map(Value::as_str).map(str::to_owned).collect();
        require!(version_names(root)?.into_iter().all(|n| listed.contains(&n)), "Uninstall found an unowned retained version; preserved");
    }
    while let Some(identifier) = remaining(&pending).first().and_then(Value::as_str).map(str::to_owned) {
        let folder = versions.join(&identifier);
        if pending["active"].is_null() {
            let manifest = verify_version(root, &identifier)?;
            let changed = with(&pending, &[("active", json!({"identifier": identifier, "manifest": manifest}))]);
            atomic_json(&root.join(TRANSACTION), &changed, Some(&pending))?;
            pending = changed;
        }
        let manifest = pending["active"]["manifest"].clone();
        let (files, directories) = verify_removal_tree(&folder, &manifest)?;
        for (name, item) in &files {
            delete_owned_file(&folder.join(name), item)?;
        }
        let mut ordered: Vec<&String> = directories.iter().collect();
        ordered.sort_by_key(|d| std::cmp::Reverse(d.matches('/').count()));
        for name in ordered {
            let directory = folder.join(name);
            if exists_or_link(&directory) {
                require!(is_real_dir(&directory), "Uninstall directory changed; preserved");
                fs::remove_dir(&directory)?;
            }
        }
        let manifest_file = folder.join(MANIFEST);
        if exists_or_link(&manifest_file) {
            require!(read_json(&manifest_file, MAX_MANIFEST)? == manifest, "Uninstall version metadata changed; preserved");
            fs::remove_file(&manifest_file)?;
        }
        if folder.exists() {
            fs::remove_dir(&folder)?;
            sync_directory(&versions)?;
        }
        let rest: Vec<Value> = remaining(&pending).into_iter().skip(1).collect();
        let changed = with(&pending, &[("remaining", Value::Array(rest)), ("active", Value::Null)]);
        atomic_json(&root.join(TRANSACTION), &changed, Some(&pending))?;
        pending = changed;
    }
    if versions.exists() {
        fs::remove_dir(&versions)?;
    }
    delete_owned_file(&root.join(&launcher_name), &pending["launcher"])?;
    let key = root.join(PUBLIC_KEY);
    if let Some(expected) = opt_text(&value, "key_sha256")
        && exists_or_link(&key)
    {
        require!(sha(&key)? == expected, "Pinned key changed during uninstall; preserved");
        fs::remove_file(&key)?;
    }
    if exists_or_link(&state_file) {
        require!(read_json(&state_file, 16384)? == value, "Install state changed during uninstall; preserved");
        fs::remove_file(&state_file)?;
    }
    finish_transaction(root)?;
    fs::remove_dir(root)?;
    sync_directory(root.parent().context("Install root has no parent")?)?;
    Ok(uninstalled())
}

// ---- Signed updates ----

/// Ed25519 verification through the system `openssl` tool, bounded to ten seconds.
fn openssl(arguments: &[&std::ffi::OsStr]) -> Result<()> {
    let mut child = std::process::Command::new("openssl")
        .args(arguments)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
        .map_err(|_| anyhow!("Ed25519 verification helper failed"))?;
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        if let Some(status) = child.try_wait()? {
            require!(status.success(), "Ed25519 verification helper failed");
            return Ok(());
        }
        if Instant::now() >= deadline {
            let _ = child.kill();
            let _ = child.wait();
            return Err(anyhow!("Ed25519 verification helper failed"));
        }
        std::thread::sleep(Duration::from_millis(10));
    }
}

pub fn pin_key(root_value: &Path, public_key: &Path) -> Result<Outcome> {
    use base64::Engine;
    let _lock = InstallationLock::acquire(root_value, false)?;
    let root = root_path(root_value, false)?;
    let value = required_state(&root)?;
    require!(is_real_file(public_key) && (1..=4096).contains(&fs::metadata(public_key)?.len()), "Public key must be a bounded regular PEM file");
    let public = regular_bytes(public_key, 4096)?;
    let pem = std::str::from_utf8(&public).map_err(|_| anyhow!("Update trust key must be a public-key PEM"))?.trim();
    let body = pem
        .strip_prefix("-----BEGIN PUBLIC KEY-----")
        .and_then(|rest| rest.strip_suffix("-----END PUBLIC KEY-----"))
        .filter(|body| !body.trim().is_empty() && body.bytes().all(|b| b.is_ascii_alphanumeric() || b"+/=\r\n \t".contains(&b)))
        .context("Update trust key must be a public-key PEM")?;
    let compact: String = body.chars().filter(|c| !c.is_whitespace()).collect();
    let der = base64::engine::general_purpose::STANDARD.decode(compact).map_err(|_| anyhow!("Update trust key must be a public-key PEM"))?;
    const ED25519_SPKI: [u8; 12] = [0x30, 0x2a, 0x30, 0x05, 0x06, 0x03, 0x2b, 0x65, 0x70, 0x03, 0x21, 0x00];
    require!(der.len() == 44 && der[..12] == ED25519_SPKI, "Update trust key must be Ed25519");
    let encoded = base64::engine::general_purpose::STANDARD.encode(&public);
    let digest = sha_bytes(&public);
    if let Some(pending) = transaction(&root)? {
        require!(
            opt_text(&pending, "kind") == Some("pin") && opt_text(&pending, "public_key") == Some(encoded.as_str()),
            "A different interrupted operation requires explicit recovery"
        );
        recover_transaction(&root, pending, "current")?;
        return Ok(vec![("status", "update_key_pinned".into()), ("public_key_sha256", digest.into()), ("recovered", true.into())]);
    }
    require!(value["key_sha256"].is_null(), "An update key is already pinned; changing trust requires an explicit new installation");
    require!(!exists_or_link(&root.join(PUBLIC_KEY)), "Public key destination contains an unowned file");
    let changed = with(&value, &[("key_sha256", digest.clone().into())]);
    let pending = json!({"owner": OWNER, "kind": "pin", "prior": value, "new": changed, "public_key": encoded});
    atomic_json(&root.join(TRANSACTION), &pending, None)?;
    recover_transaction(&root, pending, "current")?;
    Ok(vec![("status", "update_key_pinned".into()), ("public_key_sha256", digest.into())])
}

fn update_url(value: &str) -> Result<reqwest::Url> {
    require!(value.len() <= 2048 && !value.chars().any(|c| (c as u32) < 32), "Invalid update URL");
    let parsed = reqwest::Url::parse(value).map_err(|_| anyhow!("Invalid update URL"))?;
    require!(
        parsed.scheme() == "https"
            && parsed.host_str().is_some_and(|h| !h.is_empty())
            && parsed.username().is_empty()
            && parsed.password().is_none()
            && parsed.fragment().is_none(),
        "Updates require HTTPS without credentials or fragments"
    );
    Ok(parsed)
}
fn download(address: &str, destination: &Path, limit: u64, expected: Option<u64>) -> Result<()> {
    let url = update_url(address)?;
    let runtime = tokio::runtime::Builder::new_current_thread().enable_all().build()?;
    let deadline = Instant::now() + Duration::from_secs(60);
    let mut output = create_new(destination)?;
    runtime.block_on(async {
        let client = reqwest::Client::builder()
            .no_proxy()
            .redirect(reqwest::redirect::Policy::none())
            .connect_timeout(Duration::from_secs(15))
            .read_timeout(Duration::from_secs(15))
            .user_agent("screenshots-updater/1")
            .build()?;
        let mut response = client.get(url).send().await?;
        require!(response.status() == reqwest::StatusCode::OK, "Update server did not return a complete resource");
        if let Some(declared) = response.content_length() {
            require!(declared <= limit && expected.is_none_or(|e| declared == e), "Update size admission failed");
        }
        let mut total = 0u64;
        loop {
            require!(Instant::now() < deadline, "Update download exceeded its deadline");
            let remaining = deadline.saturating_duration_since(Instant::now());
            let chunk = tokio::time::timeout(remaining, response.chunk())
                .await
                .map_err(|_| anyhow!("Update download exceeded its deadline"))??;
            let Some(chunk) = chunk else { break };
            total += chunk.len() as u64;
            require!(total <= limit, "Update resource exceeded its byte limit");
            output.write_all(&chunk)?;
        }
        require!(expected.is_none_or(|e| total == e), "Update resource was truncated");
        output.sync_all()?;
        Ok(())
    })
}

pub fn apply_signed(root_value: &Path, manifest: &Path, signature: &Path, artifact: &Path) -> Result<Outcome> {
    let _lock = InstallationLock::acquire(root_value, false)?;
    apply_signed_unlocked(root_value, manifest, signature, artifact)
}
fn apply_signed_unlocked(root_value: &Path, manifest_path: &Path, signature_path: &Path, artifact: &Path) -> Result<Outcome> {
    let root = root_path(root_value, false)?;
    require!(transaction(&root)?.is_none(), "An interrupted operation requires explicit recovery");
    let installed = required_state(&root)?;
    let manifest = authenticated_manifest(&root, &installed, manifest_path, signature_path)?;
    validate_update_manifest(&root, &installed, &manifest)?;
    let size = manifest["size"].as_u64().unwrap_or_default();
    let digest = text(&manifest, "sha256")?;
    require!(
        is_real_file(artifact) && fs::metadata(artifact)?.len() == size && sha(artifact)? == digest,
        "Artifact does not match authenticated manifest"
    );
    // Freeze the authenticated input before parsing; later source edits cannot change it.
    let temporary = TempDir::new("screenshots-update-inspect-")?;
    let zip = artifact.extension().is_some_and(|e| e.eq_ignore_ascii_case("zip"));
    let snapshot = temporary.path().join(if zip { "artifact.zip" } else { "artifact.tar.gz" });
    {
        let (mut source, opened) = open_regular(artifact)?;
        require!(opened.len() == size, "Artifact changed before opening");
        let mut output = create_new(&snapshot)?;
        let copied = std::io::copy(&mut (&mut source).take(size), &mut output)?;
        require!(copied == size, "Artifact was truncated during snapshot");
        output.sync_all()?;
    }
    require!(sha(&snapshot)? == digest, "Artifact changed before authenticated parsing");
    let inspected = temporary.path().join("expanded");
    fs::create_dir(&inspected)?;
    let extracted = extract(&snapshot, &inspected)?;
    require!(
        extracted.info["version"] == manifest["version"] && extracted.info["target"] == manifest["target"],
        "Artifact build information differs from signed manifest"
    );
    install_snapshot(&snapshot, &root, digest)
}
fn authenticated_manifest(root: &Path, installed: &Value, manifest_path: &Path, signature_path: &Path) -> Result<Value> {
    let pinned = opt_text(installed, "key_sha256").context("No update public key is pinned")?;
    let key_bytes = regular_bytes(&root.join(PUBLIC_KEY), 4096)?;
    require!(sha_bytes(&key_bytes) == pinned, "Pinned update public key changed");
    let data = regular_bytes(manifest_path, 16384)?;
    let signature = regular_bytes(signature_path, 64)?;
    require!(signature.len() == 64, "Ed25519 signature must be exactly 64 bytes");
    let temporary = TempDir::new("screenshots-signature-")?;
    let (key, content, detached) =
        (temporary.path().join("key.pem"), temporary.path().join("manifest.json"), temporary.path().join("manifest.sig"));
    fs::write(&key, &key_bytes)?;
    fs::write(&content, &data)?;
    fs::write(&detached, &signature)?;
    openssl(&[
        "pkeyutl".as_ref(),
        "-verify".as_ref(),
        "-pubin".as_ref(),
        "-inkey".as_ref(),
        key.as_os_str(),
        "-rawin".as_ref(),
        "-in".as_ref(),
        content.as_os_str(),
        "-sigfile".as_ref(),
        detached.as_os_str(),
    ])?;
    parse_json(&data)
}
fn validate_update_manifest(root: &Path, installed: &Value, manifest: &Value) -> Result<()> {
    require!(keys(manifest, &["version", "target", "url", "sha256", "size"]), "Unknown or missing signed update fields");
    let update_version = version(text(manifest, "version")?)?;
    require!(manifest["target"] == installed["target"], "Signed update target does not match installation");
    let current = verify_version(root, text(installed, "current")?)?;
    require!(update_version > version(text(&current, "version")?)?, "Signed update is not newer than installed version");
    update_url(text(manifest, "url")?)?;
    require!(opt_text(manifest, "sha256").is_some_and(hex64), "Invalid signed artifact digest");
    require!(manifest["size"].as_u64().is_some_and(|s| (1..=MAX_ARCHIVE).contains(&s)), "Invalid signed artifact size");
    Ok(())
}
pub fn update(root_value: &Path, manifest_url: &str) -> Result<Outcome> {
    let _lock = InstallationLock::acquire(root_value, false)?;
    let root = root_path(root_value, false)?;
    require!(transaction(&root)?.is_none(), "An interrupted operation requires explicit recovery");
    let installed = required_state(&root)?;
    require!(!installed["key_sha256"].is_null(), "No update public key is pinned");
    require!(update_url(manifest_url)?.query().is_none(), "Update manifest URL must not contain a query");
    let temporary = TempDir::new("screenshots-update-")?;
    let (manifest, signature) = (temporary.path().join("manifest.json"), temporary.path().join("manifest.sig"));
    download(manifest_url, &manifest, 16384, None)?;
    download(&format!("{manifest_url}.sig"), &signature, 64, Some(64))?;
    // Authenticate before trusting the artifact URL or allocating the artifact download.
    let data = authenticated_manifest(&root, &installed, &manifest, &signature)?;
    validate_update_manifest(&root, &installed, &data)?;
    let address = text(&data, "url")?;
    let artifact = temporary.path().join(if address.to_lowercase().ends_with(".zip") { "release.zip" } else { "release.tar.gz" });
    download(address, &artifact, MAX_ARCHIVE, data["size"].as_u64())?;
    apply_signed_unlocked(&root, &manifest, &signature, &artifact)
}

#[cfg(test)]
mod tests;
