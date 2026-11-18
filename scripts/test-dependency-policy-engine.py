#!/usr/bin/env python3
"""Discriminating cargo-deny controls on private copies of already checked inputs."""

import argparse
import copy
import importlib.util
import json
import os
from pathlib import Path
import re
import selectors
import signal
import socket
import stat
import subprocess
import tempfile
import threading
import time
import tomllib


spec = importlib.util.spec_from_file_location("view", Path(__file__).with_name("vendored-advisory-view.py"))
view = importlib.util.module_from_spec(spec)
spec.loader.exec_module(view)
KNOWN = "RUSTSEC-2024-0429"
NEW = "RUSTSEC-2026-9999"
MAX_LOG = 8 * 1024 * 1024


def write_json(path, value):
    path.write_text(json.dumps(value, indent=2) + "\n")


def database_files(root):
    """Bound the complete repository copy, including Git objects needed offline."""
    rows, total, entries, stack = [], 0, 0, [root]
    while stack:
        with os.scandir(stack.pop()) as directory:
            for entry in directory:
                entries += 1
                view.require(entries <= 32768, "Advisory database entry bound exceeded")
                path = Path(entry.path)
                mode = entry.stat(follow_symlinks=False).st_mode
                if stat.S_ISDIR(mode):
                    stack.append(path)
                    continue
                view.require(stat.S_ISREG(mode), "Advisory database contains a nonregular file")
                body = view.read_regular(path, 64 * 1024 * 1024)
                total += len(body)
                view.require(total <= 256 * 1024 * 1024, "Advisory database byte bound exceeded")
                rows.append((path.relative_to(root).as_posix(), len(body), view.digest(body)))
    return sorted(rows)


def policy_copy(text, db_path, db_urls=None, drop_mitigation=False):
    def replace(pattern, replacement):
        nonlocal text
        text, count = re.subn(pattern, lambda _: replacement, text)
        view.require(count == 1, "Control policy field was not unique")
    replace(r"(?m)^db-path = .+$", "db-path = " + json.dumps(str(db_path)))
    if db_urls is not None:
        replace(r"(?m)^db-urls = .+$", "db-urls = " + json.dumps(db_urls))
    if drop_mitigation:
        replace(r"(?ms)^ignore = \[\n.*?^\]$", "ignore = []")
    return text


def replace_identity(value, old, new):
    if isinstance(value, dict):
        return {key: replace_identity(item, old, new) for key, item in value.items()}
    if isinstance(value, list):
        return [replace_identity(item, old, new) for item in value]
    return new if value == old else value


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    for name in ["tool", "evidence", "policy", "advisory-db-root"]:
        parser.add_argument("--" + name, type=Path, required=True)
    args = parser.parse_args()
    tool, evidence, primary, db_root = (p.resolve() for p in
                                      [args.tool, args.evidence, args.policy, args.advisory_db_root])
    output = evidence / "engine-controls"
    output.mkdir(exist_ok=False)
    results = []

    def run(name, metadata, policy, checks, expected, required, offline=True, timeout=180):
        command = [str(tool), *(["--offline"] if offline else []), "--locked", "--workspace",
                   "--exclude-dev", "--config", str(policy), "--metadata-path", str(metadata),
                   "check", *checks]
        log = output / (name + ".log")
        deadline, size = time.monotonic() + timeout, 0
        with log.open("xb") as stream, subprocess.Popen(command, stdout=subprocess.PIPE,
                stderr=subprocess.STDOUT, start_new_session=True) as process:
            try:
                with selectors.DefaultSelector() as selector:
                    selector.register(process.stdout, selectors.EVENT_READ)
                    while selector.get_map():
                        remaining = deadline - time.monotonic()
                        view.require(remaining > 0, "Control subprocess timed out: " + name)
                        for key, _ in selector.select(remaining):
                            body = os.read(key.fd, 65536)
                            if not body:
                                selector.unregister(key.fd)
                                continue
                            size += len(body)
                            view.require(size <= MAX_LOG, "Control log bound exceeded: " + name)
                            stream.write(body)
                    code = process.wait(timeout=max(0.001, deadline - time.monotonic()))
            except BaseException:
                try:
                    os.killpg(process.pid, signal.SIGKILL)
                except ProcessLookupError:
                    pass
                process.wait()
                raise
        body = view.read_regular(log, MAX_LOG).decode(errors="replace")
        matched = code == expected and all(token in body for token in required)
        results.append({"control": name, "command": command, "exit": code, "expected_exit": expected,
                        "required_diagnostics": required, "passed": matched,
                        "log_sha256": view.digest(log.read_bytes())})
        write_json(output / "results.json", results)
        view.require(matched, "Unexpected engine result: " + name + "; inspect " + str(log))

    actual_path = evidence / "actual-metadata.json"
    projected_path = evidence / "upstream-view/upstream-advisory-only-view.json"
    generated_path = evidence / "upstream-view/upstream-advisory-policy.toml"
    actual_bytes = view.read_regular(actual_path, 16 * 1024 * 1024)
    actual = json.loads(actual_bytes, object_pairs_hook=view.object_pairs)
    projected_bytes = view.read_regular(projected_path, 16 * 1024 * 1024)
    projected = json.loads(projected_bytes, object_pairs_hook=view.object_pairs)
    report = view.load_json(evidence / "upstream-view/upstream-view-report.json", 1024 * 1024)
    primary_bytes = view.read_regular(primary, 64 * 1024)
    primary_text = primary_bytes.decode()
    generated_text = view.read_regular(generated_path, 64 * 1024).decode()
    main_policy, generated_policy = (tomllib.loads(text) for text in [primary_text, generated_text])
    view.require(main_policy["advisories"]["ignore"] == [], "Primary policy must stay ignore-free")
    view.require(report["actual_metadata_sha256"] == view.digest(actual_bytes) and
                 report["primary_policy_sha256"] == view.digest(primary_bytes), "Stale checked inputs")
    mitigations = report["conditional_source_mitigations"]
    ignores = generated_policy["advisories"]["ignore"]
    view.require(len(mitigations) == len(ignores) == 1 and mitigations[0]["id"] == ignores[0]["id"] == KNOWN,
                 "Expected only the verified GLib mitigation")
    view.require(generated_policy["graph"] == main_policy["graph"] and
                 Path(generated_policy["advisories"]["db-path"]).resolve() == db_root,
                 "Generated policy does not use the checked graph/database")
    restored = copy.deepcopy(projected)
    for package in restored["packages"]:
        if package["id"] in {item["package_id"] for item in report["mapped_vendor_packages"]}:
            package["source"] = None
    view.require(restored == actual, "Projected metadata differs beyond verified advisory sources")
    selected = view.selected_ids(actual, view.load_json(evidence / "actual-selected.json", 4 * 1024 * 1024))
    repositories = []
    root_entries = 0
    with os.scandir(db_root) as directory:
        for entry in directory:
            root_entries += 1
            view.require(root_entries <= 32768, "Advisory database root entry bound exceeded")
            view.require(not entry.is_symlink(), "Advisory database root contains a symlink")
            if entry.is_dir(follow_symlinks=False):
                repositories.append(Path(entry.path))
                view.require(len(repositories) <= 1, "Expected one fetched database")
    view.require(len(repositories) == 1 and (repositories[0] / ".git").is_dir(), "Expected one fetched database")
    original_db = database_files(repositories[0])
    view.require(not any(Path(name).name == NEW + ".md" for name, _, _ in original_db), "Control advisory ID collision")
    write_json(output / "inputs.json", {"actual_metadata_sha256": view.digest(actual_bytes),
               "projected_metadata_sha256": view.digest(projected_bytes),
               "primary_policy_sha256": view.digest(primary_bytes),
               "generated_policy_sha256": view.digest(generated_text.encode()),
               "original_database_tree_sha256": view.digest(json.dumps(original_db).encode()),
               "tool_sha256": view.digest(view.read_regular(tool, 128 * 1024 * 1024)),
               "new_advisory": NEW, "fixture_scope": "Private control copies only; never accepted production metadata"})

    with tempfile.TemporaryDirectory(prefix="owned-", dir=output) as temporary:
        owned = Path(temporary)
        private_db = owned / "advisory-dbs"
        private_repo = private_db / repositories[0].name
        private_repo.mkdir(parents=True)
        for name, size, digest in original_db:
            body = view.read_regular(repositories[0] / name, 64 * 1024 * 1024)
            view.require(len(body) == size and view.digest(body) == digest, "Database changed during copy")
            destination = private_repo / name
            destination.parent.mkdir(parents=True, exist_ok=True)
            destination.write_bytes(body)
        primary_copy, generated_copy = owned / "primary.toml", owned / "generated.toml"
        primary_copy.write_text(policy_copy(primary_text, private_db))
        generated_copy.write_text(policy_copy(generated_text, private_db))
        run("actual-baseline", actual_path, primary_copy, ["advisories", "licenses"], 0, [])
        run("projected-baseline", projected_path, generated_copy, ["advisories"], 0, [])

        no_mitigation = owned / "without-glib-mitigation.toml"
        no_mitigation.write_text(policy_copy(generated_text, private_db, drop_mitigation=True))
        run("removed-glib-mitigation", projected_path, no_mitigation, ["advisories"], 1,
            ["error[unsound]", "ID: " + KNOWN, "glib v0.18.5"])
        advisory = private_repo / ("crates/glib/" + NEW + ".md")
        advisory.write_text('```toml\n[advisory]\nid = "' + NEW + '"\npackage = "glib"\n'
                            'date = "2026-01-01"\n[versions]\npatched = []\nunaffected = []\n```\n\n'
                            '# Synthetic CI control: an additional GLib vulnerability\n\n'
                            'Private test advisory only; it must not inherit the reviewed mitigation.\n')
        try:
            run("new-glib-advisory", projected_path, generated_copy, ["advisories"], 1,
                ["error[vulnerability]", "ID: " + NEW, "glib v0.18.5"])
        finally:
            advisory.unlink()

        gpl = copy.deepcopy(actual)
        packages = [p for p in gpl["packages"] if p["name"] == "app-core" and p["id"] in selected]
        view.require(len(packages) == 1 and packages[0]["license"] == "MIT", "GPL control package changed")
        packages[0]["license"] = "GPL-3.0-only"
        gpl_path = owned / "gpl-source-license.json"
        write_json(gpl_path, gpl)
        run("gpl-source-license", gpl_path, primary_copy, ["licenses"], 4,
            ["error[rejected]", "GPL-3.0-only", "app-core v0.1.0", "license is not explicitly allowed"])
        packages = [p for p in actual["packages"] if p["name"] == "option-ext" and p["id"] in selected]
        view.require(len(packages) == 1 and packages[0]["version"] == "0.2.0" and
                     packages[0]["license"] == "MPL-2.0", "Exact-version control package changed")
        old = packages[0]["id"]
        view.require(old.endswith("#option-ext@0.2.0"), "Unexpected option-ext package identity")
        new = old[:-5] + "0.2.1"
        mpl = replace_identity(actual, old, new)
        next(p for p in mpl["packages"] if p["id"] == new)["version"] = "0.2.1"
        mpl_path = owned / "mpl-other-version.json"
        write_json(mpl_path, mpl)
        run("mpl-exact-version", mpl_path, primary_copy, ["licenses"], 4,
            ["error[rejected]", "MPL-2.0", "option-ext v0.2.1", "license is not explicitly allowed"])

        contacted = threading.Event()
        with socket.socket() as listener:
            listener.bind(("127.0.0.1", 0))
            listener.listen(1)
            listener.settimeout(45)
            endpoint = f"https://localhost:{listener.getsockname()[1]}/owned-advisory-db"

            def refuse_fetch():
                try:
                    connection, _ = listener.accept()
                    with connection:
                        contacted.set()
                except OSError:
                    pass

            worker = threading.Thread(target=refuse_fetch, daemon=True)
            worker.start()
            failure_policy = owned / "fetch-failure.toml"
            failure_policy.write_text(policy_copy(primary_text, owned / "empty-fetch-root", [endpoint]))
            try:
                run("fresh-fetch-failure", actual_path, failure_policy, ["advisories"], 1,
                    ["failed to fetch advisory database " + endpoint], offline=False, timeout=40)
                results[-1]["owned_endpoint_contacted"] = contacted.is_set()
                results[-1]["passed"] = contacted.is_set()
                write_json(output / "results.json", results)
                view.require(contacted.is_set(), "Fetch control did not contact its owned endpoint")
            finally:
                listener.close()
                worker.join(timeout=1)

    view.require(view.read_regular(actual_path, 16 * 1024 * 1024) == actual_bytes and
                 view.read_regular(projected_path, 16 * 1024 * 1024) == projected_bytes and
                 view.read_regular(primary, 64 * 1024) == primary_bytes and
                 view.read_regular(generated_path, 64 * 1024).decode() == generated_text and
                 database_files(repositories[0]) == original_db, "Production input changed during controls")
    write_json(output / "passed.json", {"controls": len(results), "production_inputs_unchanged": True,
                                         "owned_fixture_cleanup": True})


if __name__ == "__main__":
    main()
