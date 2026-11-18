//! Discriminating cargo-deny controls on private copies of already checked inputs.
//! Port of the former `scripts/test-dependency-policy-engine.py`.
use crate::advisory::selected_ids;
use anyhow::{Context, Result, bail, ensure};
use screenshots_install::{TempDir, parse_json, regular_bytes, sha_bytes};
use serde_json::{Value, json};
use std::{
    collections::BTreeSet,
    fs,
    io::Read,
    net::TcpListener,
    path::{Path, PathBuf},
    process::{Command, Stdio},
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    time::{Duration, Instant},
};

const KNOWN: &str = "RUSTSEC-2024-0429";
const NEW: &str = "RUSTSEC-2026-9999";
const MAX_LOG: u64 = 8 * 1024 * 1024;

fn write_json(path: &Path, value: &Value) -> Result<()> {
    fs::write(path, serde_json::to_string_pretty(value)? + "\n")?;
    Ok(())
}

/// Bound the complete repository copy, including Git objects needed offline.
fn database_files(root: &Path) -> Result<Vec<(String, usize, String)>> {
    let (mut rows, mut total, mut entries) = (Vec::new(), 0usize, 0usize);
    let mut stack = vec![root.to_path_buf()];
    while let Some(directory) = stack.pop() {
        for entry in fs::read_dir(directory)? {
            let entry = entry?;
            entries += 1;
            ensure!(entries <= 32768, "Advisory database entry bound exceeded");
            let metadata = fs::symlink_metadata(entry.path())?;
            if metadata.is_dir() {
                stack.push(entry.path());
                continue;
            }
            ensure!(metadata.is_file(), "Advisory database contains a nonregular file");
            let body = regular_bytes(&entry.path(), 64 * 1024 * 1024)?;
            total += body.len();
            ensure!(total <= 256 * 1024 * 1024, "Advisory database byte bound exceeded");
            let name = entry.path().strip_prefix(root)?.components().map(|c| c.as_os_str().to_string_lossy()).collect::<Vec<_>>().join("/");
            rows.push((name, body.len(), sha_bytes(&body)));
        }
    }
    rows.sort();
    Ok(rows)
}

fn policy_copy(text: &str, db_path: &Path, db_urls: Option<&[String]>, drop_mitigation: bool) -> Result<String> {
    let mut text = text.to_owned();
    let mut replace = |pattern: &str, replacement: String| -> Result<()> {
        let regex = regex::Regex::new(pattern)?;
        ensure!(regex.find_iter(&text).count() == 1, "Control policy field was not unique");
        text = regex.replace(&text, regex::NoExpand(&replacement)).into_owned();
        Ok(())
    };
    replace(r"(?m)^db-path = .+$", format!("db-path = {}", Value::from(db_path.to_string_lossy().into_owned())))?;
    if let Some(urls) = db_urls {
        replace(r"(?m)^db-urls = .+$", format!("db-urls = {}", Value::from(urls.to_vec())))?;
    }
    if drop_mitigation {
        replace(r"(?ms)^ignore = \[\n.*?^\]$", "ignore = []".into())?;
    }
    Ok(text)
}

fn replace_identity(value: &Value, old: &str, new: &str) -> Value {
    match value {
        Value::Object(map) => Value::Object(map.iter().map(|(k, v)| (k.clone(), replace_identity(v, old, new))).collect()),
        Value::Array(items) => Value::Array(items.iter().map(|v| replace_identity(v, old, new)).collect()),
        Value::String(text) if text == old => Value::from(new),
        other => other.clone(),
    }
}

struct Controls {
    tool: PathBuf,
    output: PathBuf,
    results: Vec<Value>,
}
impl Controls {
    #[allow(clippy::too_many_arguments)]
    fn run(&mut self, name: &str, metadata: &Path, policy: &Path, checks: &[&str], expected: i32, required: &[&str], offline: bool, timeout: u64) -> Result<()> {
        let mut command = Command::new(&self.tool);
        if offline {
            command.arg("--offline");
        }
        command.args(["--locked", "--workspace", "--exclude-dev", "--config"]).arg(policy).arg("--metadata-path").arg(metadata).arg("check").args(checks);
        let printable: Vec<String> = std::iter::once(command.get_program()).chain(command.get_args()).map(|a| a.to_string_lossy().into_owned()).collect();
        let log = self.output.join(format!("{name}.log"));
        let file = fs::OpenOptions::new().write(true).create_new(true).open(&log)?;
        command.stdin(Stdio::null()).stdout(file.try_clone()?).stderr(file);
        #[cfg(unix)]
        std::os::unix::process::CommandExt::process_group(&mut command, 0);
        let mut child = command.spawn()?;
        let deadline = Instant::now() + Duration::from_secs(timeout);
        let kill = |child: &mut std::process::Child| {
            #[cfg(unix)]
            // SAFETY: signalling the process group we created for this control.
            unsafe {
                libc::killpg(child.id() as libc::pid_t, libc::SIGKILL);
            }
            let _ = child.kill();
            let _ = child.wait();
        };
        let code = loop {
            if let Some(status) = child.try_wait()? {
                break status.code().unwrap_or(-1);
            }
            if Instant::now() >= deadline {
                kill(&mut child);
                bail!("Control subprocess timed out: {name}");
            }
            if fs::metadata(&log)?.len() > MAX_LOG {
                kill(&mut child);
                bail!("Control log bound exceeded: {name}");
            }
            std::thread::sleep(Duration::from_millis(20));
        };
        let body = String::from_utf8_lossy(&regular_bytes(&log, MAX_LOG)?).into_owned();
        let matched = code == expected && required.iter().all(|token| body.contains(token));
        self.results.push(json!({"control": name, "command": printable, "exit": code, "expected_exit": expected,
            "required_diagnostics": required, "passed": matched, "log_sha256": sha_bytes(body.as_bytes())}));
        write_json(&self.output.join("results.json"), &Value::from(self.results.clone()))?;
        ensure!(matched, "Unexpected engine result: {name}; inspect {}", log.display());
        Ok(())
    }
}

pub fn main(args: &[String]) -> Result<()> {
    let mut options = std::collections::BTreeMap::new();
    let mut iter = args.iter();
    while let Some(flag) = iter.next() {
        let name = flag.strip_prefix("--").filter(|n| matches!(*n, "tool" | "evidence" | "policy" | "advisory-db-root"));
        let name = name.context("usage: dependency-policy-controls --tool .. --evidence .. --policy .. --advisory-db-root ..")?;
        options.insert(name.to_owned(), PathBuf::from(iter.next().context("Missing option value")?).canonicalize()?);
    }
    let get = |name: &str| options.get(name).cloned().with_context(|| format!("--{name} is required"));
    let (tool, evidence, primary, db_root) = (get("tool")?, get("evidence")?, get("policy")?, get("advisory-db-root")?);
    let output = evidence.join("engine-controls");
    fs::create_dir(&output)?;
    let mut controls = Controls { tool: tool.clone(), output: output.clone(), results: Vec::new() };

    let actual_path = evidence.join("actual-metadata.json");
    let projected_path = evidence.join("upstream-view/upstream-advisory-only-view.json");
    let generated_path = evidence.join("upstream-view/upstream-advisory-policy.toml");
    let actual_bytes = regular_bytes(&actual_path, 16 << 20)?;
    let actual = parse_json(&actual_bytes)?;
    let projected_bytes = regular_bytes(&projected_path, 16 << 20)?;
    let projected = parse_json(&projected_bytes)?;
    let report = parse_json(&regular_bytes(&evidence.join("upstream-view/upstream-view-report.json"), 1 << 20)?)?;
    let primary_bytes = regular_bytes(&primary, 64 << 10)?;
    let primary_text = String::from_utf8(primary_bytes.clone())?;
    let generated_text = String::from_utf8(regular_bytes(&generated_path, 64 << 10)?)?;
    let main_policy: toml::Table = primary_text.parse()?;
    let generated_policy: toml::Table = generated_text.parse()?;
    let as_json = |t: &toml::Table, key: &str| serde_json::to_value(t.get(key)).unwrap_or(Value::Null);
    let (main_advisories, generated_advisories) = (as_json(&main_policy, "advisories"), as_json(&generated_policy, "advisories"));
    ensure!(main_advisories["ignore"] == json!([]), "Primary policy must stay ignore-free");
    ensure!(
        report["actual_metadata_sha256"] == sha_bytes(&actual_bytes) && report["primary_policy_sha256"] == sha_bytes(&primary_bytes),
        "Stale checked inputs"
    );
    let mitigations = report["conditional_source_mitigations"].as_array().cloned().unwrap_or_default();
    let ignores = generated_advisories["ignore"].as_array().cloned().unwrap_or_default();
    ensure!(
        mitigations.len() == 1 && ignores.len() == 1 && mitigations[0]["id"] == KNOWN && ignores[0]["id"] == KNOWN,
        "Expected only the verified GLib mitigation"
    );
    let generated_db = PathBuf::from(generated_advisories["db-path"].as_str().unwrap_or_default());
    ensure!(
        as_json(&generated_policy, "graph") == as_json(&main_policy, "graph") && generated_db.canonicalize().ok().as_deref() == Some(db_root.as_path()),
        "Generated policy does not use the checked graph/database"
    );
    let mapped: BTreeSet<&str> = report["mapped_vendor_packages"].as_array().into_iter().flatten().filter_map(|m| m["package_id"].as_str()).collect();
    let mut restored = projected;
    for package in restored["packages"].as_array_mut().into_iter().flatten() {
        if package["id"].as_str().is_some_and(|id| mapped.contains(id)) {
            package["source"] = Value::Null;
        }
    }
    ensure!(restored == actual, "Projected metadata differs beyond verified advisory sources");
    let selected = selected_ids(&actual, &parse_json(&regular_bytes(&evidence.join("actual-selected.json"), 4 << 20)?)?)?;
    let mut repositories = Vec::new();
    for (count, entry) in fs::read_dir(&db_root)?.enumerate() {
        ensure!(count < 32768, "Advisory database root entry bound exceeded");
        let entry = entry?;
        let metadata = fs::symlink_metadata(entry.path())?;
        ensure!(!metadata.file_type().is_symlink(), "Advisory database root contains a symlink");
        if metadata.is_dir() {
            repositories.push(entry.path());
            ensure!(repositories.len() <= 1, "Expected one fetched database");
        }
    }
    ensure!(repositories.len() == 1 && repositories[0].join(".git").is_dir(), "Expected one fetched database");
    let repository = repositories.remove(0);
    let original_db = database_files(&repository)?;
    ensure!(!original_db.iter().any(|(name, ..)| name.ends_with(&format!("/{NEW}.md"))), "Control advisory ID collision");
    write_json(&output.join("inputs.json"), &json!({"actual_metadata_sha256": sha_bytes(&actual_bytes),
        "projected_metadata_sha256": sha_bytes(&projected_bytes), "primary_policy_sha256": sha_bytes(&primary_bytes),
        "generated_policy_sha256": sha_bytes(generated_text.as_bytes()),
        "original_database_tree_sha256": sha_bytes(serde_json::to_string(&original_db)?.as_bytes()),
        "tool_sha256": sha_bytes(&regular_bytes(&tool, 128 << 20)?), "new_advisory": NEW,
        "fixture_scope": "Private control copies only; never accepted production metadata"}))?;

    {
        let owned = TempDir::new_in(&output, "owned-")?;
        let owned = owned.path();
        let private_db = owned.join("advisory-dbs");
        let private_repo = private_db.join(repository.file_name().context("repository name")?);
        fs::create_dir_all(&private_repo)?;
        for (name, size, digest) in &original_db {
            let body = regular_bytes(&repository.join(name), 64 << 20)?;
            ensure!(body.len() == *size && sha_bytes(&body) == *digest, "Database changed during copy");
            let destination = private_repo.join(name);
            fs::create_dir_all(destination.parent().context("parent")?)?;
            fs::write(destination, body)?;
        }
        let (primary_copy, generated_copy) = (owned.join("primary.toml"), owned.join("generated.toml"));
        fs::write(&primary_copy, policy_copy(&primary_text, &private_db, None, false)?)?;
        fs::write(&generated_copy, policy_copy(&generated_text, &private_db, None, false)?)?;
        controls.run("actual-baseline", &actual_path, &primary_copy, &["advisories", "licenses"], 0, &[], true, 180)?;
        controls.run("projected-baseline", &projected_path, &generated_copy, &["advisories"], 0, &[], true, 180)?;

        let no_mitigation = owned.join("without-glib-mitigation.toml");
        fs::write(&no_mitigation, policy_copy(&generated_text, &private_db, None, true)?)?;
        let known_id = format!("ID: {KNOWN}");
        controls.run("removed-glib-mitigation", &projected_path, &no_mitigation, &["advisories"], 1, &["error[unsound]", &known_id, "glib v0.18.5"], true, 180)?;
        let advisory = private_repo.join(format!("crates/glib/{NEW}.md"));
        fs::write(&advisory, format!("```toml\n[advisory]\nid = \"{NEW}\"\npackage = \"glib\"\ndate = \"2026-01-01\"\n[versions]\npatched = []\nunaffected = []\n```\n\n# Synthetic CI control: an additional GLib vulnerability\n\nPrivate test advisory only; it must not inherit the reviewed mitigation.\n"))?;
        let new_id = format!("ID: {NEW}");
        let result = controls.run("new-glib-advisory", &projected_path, &generated_copy, &["advisories"], 1, &["error[vulnerability]", &new_id, "glib v0.18.5"], true, 180);
        fs::remove_file(&advisory)?;
        result?;

        let mut gpl = actual.clone();
        let mut found = 0;
        for package in gpl["packages"].as_array_mut().into_iter().flatten() {
            if package["name"] == "app-core" && package["id"].as_str().is_some_and(|id| selected.contains(id)) {
                ensure!(package["license"] == "MIT", "GPL control package changed");
                package["license"] = "GPL-3.0-only".into();
                found += 1;
            }
        }
        ensure!(found == 1, "GPL control package changed");
        let gpl_path = owned.join("gpl-source-license.json");
        write_json(&gpl_path, &gpl)?;
        controls.run("gpl-source-license", &gpl_path, &primary_copy, &["licenses"], 4, &["error[rejected]", "GPL-3.0-only", "app-core v0.1.0", "license is not explicitly allowed"], true, 180)?;

        let candidates: Vec<&Value> = actual["packages"].as_array().into_iter().flatten()
            .filter(|p| p["name"] == "option-ext" && p["id"].as_str().is_some_and(|id| selected.contains(id))).collect();
        ensure!(candidates.len() == 1 && candidates[0]["version"] == "0.2.0" && candidates[0]["license"] == "MPL-2.0", "Exact-version control package changed");
        let old = candidates[0]["id"].as_str().unwrap_or_default().to_owned();
        ensure!(old.ends_with("#option-ext@0.2.0"), "Unexpected option-ext package identity");
        let new = format!("{}0.2.1", &old[..old.len() - 5]);
        let mut mpl = replace_identity(&actual, &old, &new);
        for package in mpl["packages"].as_array_mut().into_iter().flatten() {
            if package["id"] == new.as_str() {
                package["version"] = "0.2.1".into();
            }
        }
        let mpl_path = owned.join("mpl-other-version.json");
        write_json(&mpl_path, &mpl)?;
        controls.run("mpl-exact-version", &mpl_path, &primary_copy, &["licenses"], 4, &["error[rejected]", "MPL-2.0", "option-ext v0.2.1", "license is not explicitly allowed"], true, 180)?;

        let listener = TcpListener::bind("127.0.0.1:0")?;
        let endpoint = format!("https://localhost:{}/owned-advisory-db", listener.local_addr()?.port());
        let contacted = Arc::new(AtomicBool::new(false));
        listener.set_nonblocking(true)?;
        let stop = Arc::new(AtomicBool::new(false));
        let worker = {
            let (contacted, stop) = (contacted.clone(), stop.clone());
            std::thread::spawn(move || {
                let deadline = Instant::now() + Duration::from_secs(45);
                while !stop.load(Ordering::SeqCst) && Instant::now() < deadline {
                    if let Ok((mut connection, _)) = listener.accept() {
                        contacted.store(true, Ordering::SeqCst);
                        let _ = connection.read(&mut [0u8; 1]);
                        return;
                    }
                    std::thread::sleep(Duration::from_millis(20));
                }
            })
        };
        let failure_policy = owned.join("fetch-failure.toml");
        fs::write(&failure_policy, policy_copy(&primary_text, &owned.join("empty-fetch-root"), Some(std::slice::from_ref(&endpoint)), false)?)?;
        let message = format!("failed to fetch advisory database {endpoint}");
        let result = controls.run("fresh-fetch-failure", &actual_path, &failure_policy, &["advisories"], 1, &[&message], false, 40);
        stop.store(true, Ordering::SeqCst);
        let _ = worker.join();
        result?;
        let reached = contacted.load(Ordering::SeqCst);
        if let Some(last) = controls.results.last_mut() {
            last["owned_endpoint_contacted"] = reached.into();
            last["passed"] = reached.into();
        }
        write_json(&output.join("results.json"), &Value::from(controls.results.clone()))?;
        ensure!(reached, "Fetch control did not contact its owned endpoint");
    }

    ensure!(
        regular_bytes(&actual_path, 16 << 20)? == actual_bytes
            && regular_bytes(&projected_path, 16 << 20)? == projected_bytes
            && regular_bytes(&primary, 64 << 10)? == primary_bytes
            && String::from_utf8(regular_bytes(&generated_path, 64 << 10)?)? == generated_text
            && database_files(&repository)? == original_db,
        "Production input changed during controls"
    );
    write_json(&output.join("passed.json"), &json!({"controls": controls.results.len(), "production_inputs_unchanged": true, "owned_fixture_cleanup": true}))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn policy_copy_replaces_unique_fields_and_drops_mitigation() {
        let text = "db-path = \"x\"\ndb-urls = [\"a\"]\nignore = [\n  { id = \"R\" },\n]\n";
        let out = policy_copy(text, Path::new("/db"), Some(&["https://h".into()]), true).unwrap();
        assert_eq!(out, "db-path = \"/db\"\ndb-urls = [\"https://h\"]\nignore = []\n");
        assert!(policy_copy("x = 1\n", Path::new("/db"), None, false).is_err());
        assert_eq!(replace_identity(&json!({"a": ["o", "p"]}), "o", "n"), json!({"a": ["n", "p"]}));
    }
}
