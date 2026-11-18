//! Create an advisory-only published-base view; never resolve or build crates.
use anyhow::{Context, Result, bail, ensure};
use screenshots_install::{archive, parse_json, regular_bytes, sha_bytes};
use serde_json::{Value, json};
use std::{
    collections::{BTreeMap, BTreeSet},
    fs,
    path::{Path, PathBuf},
};

const SPARSE_IDENTITY: &str = "sparse+https://index.crates.io/";
const TARGETS: [&str; 5] =
    ["x86_64-unknown-linux-gnu", "aarch64-unknown-linux-gnu", "x86_64-pc-windows-msvc", "aarch64-apple-darwin", "x86_64-apple-darwin"];
const MAX_ARCHIVE: usize = 8 * 1024 * 1024;
const MAX_UNPACKED: u64 = 64 * 1024 * 1024;

fn keys(value: &Value, expected: &[&str]) -> bool {
    value.as_object().is_some_and(|m| m.len() == expected.len() && expected.iter().all(|k| m.contains_key(*k)))
}
fn s<'a>(value: &'a Value, key: &str) -> &'a str {
    value[key].as_str().unwrap_or_default()
}
fn relative(value: &str) -> Result<PathBuf> {
    ensure!(!value.is_empty() && value.len() <= 4096, "Invalid source path");
    ensure!(!value.starts_with('/') && value.split('/').all(|p| !matches!(p, "" | "." | "..")), "Unsafe source path");
    Ok(PathBuf::from(value))
}

/// Hash regular source bytes, excluding only root build/VCS bookkeeping.
fn source_tree(folder: &Path) -> Result<Value> {
    ensure!(fs::symlink_metadata(folder).is_ok_and(|m| m.is_dir()), "Invalid vendor directory");
    let mut rows = Vec::new();
    let (mut total, mut visited) = (0usize, 0usize);
    let mut stack = vec![(folder.to_path_buf(), String::new())];
    while let Some((directory, prefix)) = stack.pop() {
        let mut entries: Vec<_> = fs::read_dir(&directory)?.collect::<std::io::Result<_>>()?;
        entries.sort_by_key(|e| e.file_name());
        for item in entries {
            visited += 1;
            ensure!(visited <= 8192, "Vendor source entry bound exceeded");
            let file_name = item.file_name().to_string_lossy().into_owned();
            if prefix.is_empty() && matches!(file_name.as_str(), ".git" | "target" | ".cargo-ok") {
                continue;
            }
            let name = format!("{prefix}{file_name}");
            let info = fs::symlink_metadata(item.path())?;
            ensure!(!info.file_type().is_symlink(), "Vendor source contains a symlink");
            if info.is_dir() {
                stack.push((item.path(), format!("{name}/")));
                continue;
            }
            ensure!(info.is_file(), "Vendor source contains a nonregular file");
            let body = regular_bytes(&item.path(), 16 * 1024 * 1024)?;
            total += body.len();
            rows.push((name, body.len(), sha_bytes(&body)));
            ensure!(rows.len() <= 4096 && total <= 32 * 1024 * 1024, "Vendor source exceeds fingerprint bounds");
        }
    }
    rows.sort();
    let encoded: String = rows.iter().map(|(name, size, sha)| format!("{sha}\t{size}\t{name}\n")).collect();
    Ok(json!({"sha256": sha_bytes(encoded.as_bytes()), "files": rows.len(), "bytes": total}))
}

pub(crate) fn selected_ids(metadata: &Value, selection: &Value) -> Result<BTreeSet<String>> {
    let selection = selection.as_object().filter(|m| !m.is_empty() && m.len() <= 4096).context("Invalid selected graph")?;
    let mut ids = BTreeMap::new();
    for package in metadata["packages"].as_array().into_iter().flatten() {
        let id = s(package, "id");
        ensure!(id.contains('#') && !id.contains(' '), "Unsupported metadata package ID format");
        let key = format!("{} {} {}", s(package, "name"), s(package, "version"), id.rsplit_once('#').map(|x| x.0).unwrap_or_default());
        ensure!(ids.insert(key, id.to_owned()).is_none(), "Ambiguous metadata package identity");
    }
    selection.keys().map(|key| ids.get(key).cloned().context("Selected graph contains an unknown package")).collect()
}

fn fetch_archive(entry: &Value) -> Result<Vec<u8>> {
    let (name, version) = (s(entry, "name"), s(entry, "version"));
    ensure!(name.chars().all(|c| c.is_ascii_alphanumeric() || "_-".contains(c)), "Invalid published package name");
    ensure!(version.chars().all(|c| c.is_ascii_alphanumeric() || ".+-".contains(c)), "Invalid published package version");
    let url = format!("https://static.crates.io/crates/{name}/{name}-{version}.crate");
    let runtime = tokio::runtime::Builder::new_current_thread().enable_all().build()?;
    runtime.block_on(async {
        let client = reqwest::Client::builder().timeout(std::time::Duration::from_secs(30)).build()?;
        let mut response = client.get(url).send().await?.error_for_status()?;
        let mut body = Vec::new();
        while let Some(chunk) = response.chunk().await? {
            body.extend_from_slice(&chunk);
            ensure!(body.len() <= MAX_ARCHIVE, "Published archive exceeds limit");
        }
        Ok(body)
    })
}

/// Selected members of a verified published `.crate`.
struct Published {
    files: BTreeMap<String, Vec<u8>>,
    prefix: String,
}
fn published_archive(entry: &Value, body: &[u8], wanted: &[&str], unpacked: u64) -> Result<Published> {
    ensure!(body.len() <= MAX_ARCHIVE && sha_bytes(body) == s(entry, "published_archive_sha256"), "Published archive checksum mismatch");
    let prefix = format!("{}-{}/", s(entry, "name"), s(entry, "version"));
    let mut files = BTreeMap::new();
    let mut count = 0usize;
    let limits = archive::TarLimits { entries: 4096, member: unpacked, total: unpacked };
    archive::read_tar_gz(body, &limits, &mut |member| {
        count += 1;
        ensure!(count <= 4096, "Published archive member bound exceeded");
        if let Some(rest) = member.name.strip_prefix(&prefix)
            && (rest == "Cargo.toml" || wanted.contains(&rest))
        {
            ensure!(member.size <= 1024 * 1024, "Invalid published manifest member");
            let mut data = Vec::new();
            member.reader.read_to_end(&mut data)?;
            files.insert(rest.to_owned(), data);
        }
        Ok(())
    })?;
    let manifest = files.get("Cargo.toml").context("Invalid published manifest member")?;
    ensure!(manifest.len() <= 256 * 1024 && sha_bytes(manifest) == s(entry, "published_manifest_sha256"), "Published manifest mismatch");
    let parsed: toml::Table = std::str::from_utf8(manifest)?.parse()?;
    let package = parsed.get("package").context("Published base identity mismatch")?;
    ensure!(
        package.get("name").and_then(|v| v.as_str()) == Some(s(entry, "name"))
            && package.get("version").and_then(|v| v.as_str()) == Some(s(entry, "version")),
        "Published base identity mismatch"
    );
    Ok(Published { files, prefix })
}

fn glib_mitigation(proof: &Value, entry: &Value, package: &Value, metadata: &Value, published: &Published, root: &Path, db_root: &Path) -> Result<Value> {
    ensure!(
        (s(proof, "kind"), s(proof, "id"), s(proof, "package"), s(proof, "version"), s(proof, "source_file"), s(proof, "advisory_path"))
            == ("glib-variant-out-argument-v1", "RUSTSEC-2024-0429", "glib", "0.18.5", "src/variant_iter.rs", "crates/glib/RUSTSEC-2024-0429.md"),
        "Unknown mitigation contract"
    );
    // An ID ignore cannot hide another same-name package/version in this view.
    let glib: Vec<&Value> = metadata["packages"].as_array().into_iter().flatten().filter(|p| p["name"] == "glib").map(|p| &p["id"]).collect();
    ensure!(glib == vec![&package["id"]], "GLib mitigation identity is not unique");
    let original = published.files.get(s(proof, "source_file")).with_context(|| format!("{}{} missing", published.prefix, s(proof, "source_file")))?;
    ensure!(sha_bytes(original) == s(proof, "published_file_sha256"), "GLib published source mismatch");
    let text = std::str::from_utf8(original)?;
    let (old_pointer, old_argument) = ("let p: *mut libc::c_char = std::ptr::null_mut();", "                &p,\n");
    ensure!(text.matches(old_pointer).count() == 1 && text.matches(old_argument).count() == 1, "GLib original fix precondition changed");
    let corrected = text.replace(old_pointer, "let mut p: *mut libc::c_char = std::ptr::null_mut();").replace(old_argument, "                &mut p,\n");
    let actual = regular_bytes(&root.join(s(entry, "path")).join(s(proof, "source_file")), 1024 * 1024)?;
    ensure!(actual == corrected.as_bytes() && sha_bytes(&actual) == s(proof, "patched_file_sha256"), "GLib security fix is absent or differs");
    let mut advisories = Vec::new();
    for entry in fs::read_dir(db_root)? {
        let candidate = entry?.path().join(s(proof, "advisory_path"));
        if candidate.is_file() {
            advisories.push(candidate);
        }
    }
    ensure!(advisories.len() == 1, "Expected one fresh RustSec advisory body");
    ensure!(sha_bytes(&regular_bytes(&advisories[0], 256 * 1024)?) == s(proof, "advisory_sha256"), "GLib advisory changed; mitigation needs review");
    let logs = &proof["evidence"]["logs"];
    ensure!(
        logs.as_array().is_some_and(|l| !l.is_empty()) && proof["upstream_fix"]["sha256"].as_str().is_some_and(|v| !v.is_empty()),
        "Missing mitigation evidence"
    );
    Ok(json!({"id": proof["id"], "package_id": package["id"], "source_tree": entry["local_source"],
              "advisory_sha256": proof["advisory_sha256"], "patched_file_sha256": proof["patched_file_sha256"],
              "upstream_fix": proof["upstream_fix"], "prior_execution_evidence": proof["evidence"]}))
}

fn toml_json(value: &toml::Value) -> Result<Value> {
    Ok(serde_json::to_value(value)?)
}
fn advisory_policy(policy: &toml::Table, db_root: &Path, mitigations: &[Value]) -> Result<String> {
    let graph = toml_json(policy.get("graph").context("Unsupported graph policy")?)?;
    let advisories = toml_json(policy.get("advisories").context("Unsupported advisory policy field")?)?;
    ensure!(
        graph == json!({"targets": TARGETS, "all-features": false, "no-default-features": false, "features": [], "exclude-dev": true, "exclude-unpublished": false, "exclude": []}),
        "Unsupported graph policy"
    );
    ensure!(
        keys(&advisories, &["db-path", "db-urls", "unmaintained", "unsound", "yanked", "ignore", "unused-ignored-advisory"]),
        "Unsupported advisory policy field"
    );
    ensure!(
        advisories["ignore"] == json!([]) && advisories["unmaintained"] == "all" && advisories["unsound"] == "all" && advisories["yanked"] == "deny",
        "Primary policy must retain no-ignore coverage"
    );
    ensure!(advisories["db-urls"] == json!(["https://github.com/RustSec/advisory-db"]), "Unexpected advisory database");
    let mut lines = vec!["# CONSERVATIVE PUBLISHED-BASE ADVISORY VIEW ONLY; not build/license metadata.".to_owned(), "[graph]".to_owned()];
    for (key, value) in graph.as_object().into_iter().flatten() {
        lines.push(format!("{key} = {value}"));
    }
    lines.push("\n[advisories]".into());
    for (key, value) in advisories.as_object().into_iter().flatten() {
        match key.as_str() {
            "db-path" => lines.push(format!("db-path = {}", Value::from(db_root.canonicalize()?.to_string_lossy().into_owned()))),
            "ignore" => {
                lines.push("ignore = [".into());
                for proof in mitigations {
                    let reason = "Exact reviewed local GLib source/backport and advisory body verified; see upstream-view-report.json";
                    lines.push(format!("  {{ id = {}, reason = {} }},", proof["id"], Value::from(reason)));
                }
                lines.push("]".into());
            }
            _ => lines.push(format!("{key} = {value}")),
        }
    }
    Ok(lines.join("\n") + "\n")
}

type Loader<'a> = &'a dyn Fn(&Value) -> Result<Vec<u8>>;
fn prepare(metadata: &Value, selection: &Value, provenance: &Value, root: &Path, policy: &toml::Table, db_root: &Path, loader: Loader) -> Result<(Value, String, Value)> {
    prepare_bounded(metadata, selection, provenance, root, policy, db_root, loader, MAX_UNPACKED)
}
fn prepare_bounded(metadata: &Value, selection: &Value, provenance: &Value, root: &Path, policy: &toml::Table, db_root: &Path, loader: Loader, unpacked: u64) -> Result<(Value, String, Value)> {
    ensure!(keys(provenance, &["version", "scope", "first_party", "packages", "mitigations"]), "Unknown provenance field");
    let list = |key: &str| provenance[key].as_array().cloned().unwrap_or_default();
    let (packages, first_party, mitigations) = (list("packages"), list("first_party"), list("mitigations"));
    ensure!(
        provenance["version"] == 1 && packages.len() <= 64 && metadata["packages"].as_array().is_some_and(|p| p.len() <= 4096),
        "Invalid provenance/metadata bounds"
    );
    ensure!(first_party.len() <= 16 && mitigations.len() <= 1, "Provenance entry bound exceeded");
    for entry in &packages {
        ensure!(
            keys(entry, &["name", "version", "path", "published_archive_sha256", "published_manifest_sha256", "published_checksum_api", "local_source"]),
            "Unknown published-base field"
        );
        ensure!(keys(&entry["local_source"], &["sha256", "files", "bytes"]), "Unknown source fingerprint field");
    }
    for entry in &first_party {
        ensure!(keys(entry, &["name", "manifest"]), "Unknown first-party field");
        relative(s(entry, "manifest"))?;
    }
    for proof in &mitigations {
        ensure!(
            keys(proof, &["id", "package", "version", "kind", "advisory_path", "advisory_sha256", "source_file", "published_file_sha256", "patched_file_sha256", "upstream_fix", "evidence"]),
            "Unknown mitigation field"
        );
        ensure!(
            (s(proof, "id"), s(proof, "package"), s(proof, "version"), s(proof, "kind")) == ("RUSTSEC-2024-0429", "glib", "0.18.5", "glib-variant-out-argument-v1"),
            "Unreviewed mitigation identity"
        );
    }
    ensure!(Path::new(s(metadata, "workspace_root")).canonicalize()? == root.canonicalize()?, "Metadata workspace does not match source root");
    let selected = selected_ids(metadata, selection)?;
    let first: BTreeMap<&str, &str> = first_party.iter().map(|p| (s(p, "manifest"), s(p, "name"))).collect();
    let members: BTreeSet<&str> = metadata["workspace_members"].as_array().into_iter().flatten().filter_map(Value::as_str).collect();
    let workspace: BTreeMap<String, &Value> = metadata["packages"].as_array().into_iter().flatten().filter(|p| members.contains(s(p, "id"))).map(|p| (s(p, "id").to_owned(), p)).collect();
    ensure!(workspace.keys().all(|id| selected.contains(id)) && workspace.len() == first.len(), "Missing selected first-party roots");
    for package in workspace.values() {
        let manifest = Path::new(s(package, "manifest_path")).strip_prefix(root)?.components().map(|c| c.as_os_str().to_string_lossy()).collect::<Vec<_>>().join("/");
        ensure!(first.get(manifest.as_str()) == Some(&s(package, "name")) && package["source"].is_null(), "Unknown first-party identity");
    }
    let entries: BTreeMap<&str, &Value> = packages.iter().map(|e| (s(e, "name"), e)).collect();
    ensure!(entries.len() == packages.len(), "Duplicate published-base mapping");
    let mut projected = metadata.clone();
    let (mut mapped, mut mitigated) = (Vec::new(), Vec::new());
    let (mut archived_bytes, mut source_bytes) = (0usize, 0u64);
    let wanted: Vec<&str> = mitigations.iter().map(|p| s(p, "source_file")).collect();
    for package in projected["packages"].as_array_mut().context("metadata packages")? {
        let id = s(package, "id").to_owned();
        if !selected.contains(&id) || !package["source"].is_null() || workspace.contains_key(&id) {
            continue;
        }
        let entry = entries.get(s(package, "name")).filter(|e| e["version"] == package["version"]).context("Selected local package has no verified published base")?;
        let folder = root.join(relative(s(entry, "path"))?);
        ensure!(s(entry, "path").starts_with("vendor/") && Path::new(s(package, "manifest_path")) == folder.join("Cargo.toml"), "Vendor manifest does not match mapping");
        let actual = source_tree(&folder)?;
        ensure!(actual == entry["local_source"], "Vendor source fingerprint changed: {}; provenance review required", s(entry, "name"));
        source_bytes += actual["bytes"].as_u64().unwrap_or_default();
        ensure!(source_bytes <= 256 * 1024 * 1024, "Aggregate vendor fingerprint bound exceeded");
        let body = loader(entry)?;
        archived_bytes += body.len();
        ensure!(archived_bytes <= 64 * 1024 * 1024, "Aggregate published archive bound exceeded");
        let published = published_archive(entry, &body, &wanted, unpacked)?;
        for proof in mitigations.iter().filter(|p| p["package"] == package["name"]) {
            mitigated.push(glib_mitigation(proof, entry, package, metadata, &published, root, db_root)?);
        }
        // Sparse identifies the same crates.io base to cargo-deny without replacing
        // path-patched feature definitions with published registry features.
        package["source"] = SPARSE_IDENTITY.into();
        mapped.push(json!({"package_id": id, "name": package["name"], "version": package["version"], "actual_source": null,
                           "advisory_only_source": SPARSE_IDENTITY, "published_archive_sha256": entry["published_archive_sha256"], "local_source": actual}));
    }
    let unique: BTreeSet<&str> = mitigated.iter().map(|p| s(p, "package_id")).collect();
    ensure!(unique.len() == mitigated.len(), "Duplicate conditional mitigation");
    // No new IDs, versions, checksums, edges, feature maps or target predicates.
    let mut restored = projected.clone();
    let mapped_ids: BTreeSet<&str> = mapped.iter().map(|m| s(m, "package_id")).collect();
    for package in restored["packages"].as_array_mut().into_iter().flatten() {
        if mapped_ids.contains(s(package, "id")) {
            package["source"] = Value::Null;
        }
    }
    ensure!(restored == *metadata, "Projection changed actual graph data");
    let report = json!({"scope": "Conservative verified published-base advisory check; not actual patched-source/build/license metadata.",
        "selected_packages": selected.len(), "selected_first_party": workspace.keys().collect::<Vec<_>>(), "mapped_vendor_packages": mapped,
        "conditional_source_mitigations": mitigated, "actual_graph_preserved_except_advisory_source": true,
        "source_bytes_hashed": source_bytes, "published_archive_bytes": archived_bytes});
    let configuration = advisory_policy(policy, db_root, &mitigated)?;
    Ok((projected, configuration, report))
}

fn load(path: &Path, limit: u64) -> Result<Value> {
    parse_json(&regular_bytes(path, limit)?)
}

pub fn main(args: &[String]) -> Result<()> {
    match args.first().map(String::as_str) {
        Some("verify-selection") => {
            let [_, before, after] = args else { bail!("usage: verify-selection <before> <after>") };
            let (before, after) = (load(Path::new(before), 4 << 20)?, load(Path::new(after), 4 << 20)?);
            verify_selection(&before, &after)?;
            Ok(())
        }
        Some("prepare") => {
            let mut options = BTreeMap::new();
            let mut iter = args[1..].iter();
            while let Some(flag) = iter.next() {
                let name = flag.strip_prefix("--").context("Expected --option")?;
                ensure!(
                    matches!(name, "metadata" | "selected" | "provenance" | "root" | "policy" | "advisory-db-root" | "output"),
                    "Unknown option --{name}"
                );
                options.insert(name.to_owned(), PathBuf::from(iter.next().context("Missing option value")?));
            }
            let option = |name: &str| options.get(name).with_context(|| format!("--{name} is required"));
            let actual_bytes = regular_bytes(option("metadata")?, 16 << 20)?;
            let metadata = parse_json(&actual_bytes)?;
            let provenance_bytes = regular_bytes(option("provenance")?, 1 << 20)?;
            let provenance = parse_json(&provenance_bytes)?;
            let policy_bytes = regular_bytes(option("policy")?, 64 << 10)?;
            let policy: toml::Table = std::str::from_utf8(&policy_bytes)?.parse()?;
            let root = option("root")?.canonicalize()?;
            let selection = load(option("selected")?, 4 << 20)?;
            let (projected, configuration, mut report) =
                prepare(&metadata, &selection, &provenance, &root, &policy, option("advisory-db-root")?, &fetch_archive)?;
            report["actual_metadata_sha256"] = sha_bytes(&actual_bytes).into();
            report["provenance_sha256"] = sha_bytes(&provenance_bytes).into();
            report["primary_policy_sha256"] = sha_bytes(&policy_bytes).into();
            let output = option("output")?;
            ensure!(!output.exists(), "Output directory already exists");
            fs::create_dir_all(output)?;
            for (name, content) in [
                ("upstream-advisory-only-view.json", serde_json::to_string(&projected)?),
                ("upstream-advisory-policy.toml", configuration),
                ("upstream-view-report.json", serde_json::to_string_pretty(&report)?),
            ] {
                let mut file = fs::OpenOptions::new().write(true).create_new(true).open(output.join(name))?;
                std::io::Write::write_all(&mut file, (content + "\n").as_bytes())?;
            }
            Ok(())
        }
        _ => bail!("usage: cargo xtask vendored-advisory-view prepare|verify-selection ..."),
    }
}

fn verify_selection(before: &Value, after: &Value) -> Result<()> {
    ensure!(before.as_object().is_some_and(|m| !m.is_empty()) && before == after, "Actual/projected selected package or license listing differs");
    Ok(())
}

#[cfg(test)]
#[path = "advisory_tests.rs"]
mod tests;
