#!/usr/bin/env python3
"""Create an advisory-only published-base view; never resolve or build crates."""

import argparse
import copy
import gzip
import hashlib
import io
import json
import os
from pathlib import Path, PurePosixPath
import stat
import tarfile
import tomllib
import urllib.request


SPARSE_IDENTITY = "sparse+https://index.crates.io/"
TARGETS = ["x86_64-unknown-linux-gnu", "aarch64-unknown-linux-gnu",
           "x86_64-pc-windows-msvc", "aarch64-apple-darwin", "x86_64-apple-darwin"]
MAX_ARCHIVE = 8 * 1024 * 1024
MAX_UNPACKED = 64 * 1024 * 1024


def require(condition, message):
    if not condition:
        raise ValueError(message)


def digest(data):
    return hashlib.sha256(data).hexdigest()


def object_pairs(pairs):
    result = {}
    for key, value in pairs:
        require(key not in result, "Duplicate JSON field")
        result[key] = value
    return result


def load_json(path, limit):
    data = read_regular(path, limit)
    return json.loads(data, object_pairs_hook=object_pairs)


def read_regular(path, limit):
    flags = os.O_RDONLY | getattr(os, "O_NOFOLLOW", 0) | getattr(os, "O_NONBLOCK", 0)
    with os.fdopen(os.open(path, flags), "rb") as stream:
        before = os.fstat(stream.fileno())
        require(stat.S_ISREG(before.st_mode) and before.st_size <= limit, "Invalid or oversized input")
        result = stream.read(limit + 1)
        after = os.fstat(stream.fileno())
        stamp = lambda s: (s.st_dev, s.st_ino, s.st_size, s.st_mtime_ns, s.st_ctime_ns)
        require(stamp(before) == stamp(after) and len(result) == before.st_size, "Input changed during read")
        return result


def relative(value):
    require(isinstance(value, str) and 0 < len(value) <= 4096, "Invalid source path")
    path = PurePosixPath(value)
    require(not path.is_absolute() and all(p not in {"", ".", ".."} for p in value.split("/")), "Unsafe source path")
    return path


def source_tree(folder):
    """Hash regular source bytes, excluding only root build/VCS bookkeeping."""
    require(not folder.is_symlink() and folder.is_dir(), "Invalid vendor directory")
    require(folder.resolve() == folder, "Vendor directory has a symlink ancestor")
    rows, total, visited, stack = [], 0, 0, [(folder, "")]
    while stack:
        directory, prefix = stack.pop()
        for item in sorted(os.scandir(directory), key=lambda e: e.name):
            visited += 1
            require(visited <= 8192, "Vendor source entry bound exceeded")
            name = prefix + item.name
            if not prefix and item.name in {".git", "target", ".cargo-ok"}:
                continue
            info = item.stat(follow_symlinks=False)
            require(not stat.S_ISLNK(info.st_mode), "Vendor source contains a symlink")
            if stat.S_ISDIR(info.st_mode):
                stack.append((Path(item.path), name + "/"))
                continue
            require(stat.S_ISREG(info.st_mode), "Vendor source contains a nonregular file")
            body = read_regular(item.path, 16 * 1024 * 1024)
            total += len(body)
            rows.append((name, len(body), digest(body)))
            require(len(rows) <= 4096 and total <= 32 * 1024 * 1024, "Vendor source exceeds fingerprint bounds")
    rows.sort()
    encoded = "".join(f"{sha}\t{size}\t{name}\n" for name, size, sha in rows).encode()
    return {"sha256": digest(encoded), "files": len(rows), "bytes": total}


def selected_ids(metadata, selection):
    require(isinstance(selection, dict) and 0 < len(selection) <= 4096, "Invalid selected graph")
    keys = {}
    for package in metadata["packages"]:
        identifier = package["id"]
        require("#" in identifier and " " not in identifier, "Unsupported metadata package ID format")
        key = f"{package['name']} {package['version']} {identifier.rsplit('#', 1)[0]}"
        require(key not in keys, "Ambiguous metadata package identity")
        keys[key] = identifier
    require(set(selection) <= set(keys), "Selected graph contains an unknown package")
    return {keys[key] for key in selection}


def fetch_archive(entry):
    name, version = entry["name"], entry["version"]
    require(all(c.isalnum() or c in "_-" for c in name), "Invalid published package name")
    require(all(c.isalnum() or c in ".+-" for c in version), "Invalid published package version")
    url = f"https://static.crates.io/crates/{name}/{name}-{version}.crate"
    with urllib.request.urlopen(url, timeout=30) as response:
        body = response.read(MAX_ARCHIVE + 1)
    require(len(body) <= MAX_ARCHIVE, "Published archive exceeds limit")
    return body


def published_archive(entry, loader):
    body = loader(entry)
    require(len(body) <= MAX_ARCHIVE and digest(body) == entry["published_archive_sha256"], "Published archive checksum mismatch")
    with gzip.GzipFile(fileobj=io.BytesIO(body)) as compressed:
        unpacked = compressed.read(MAX_UNPACKED + 1)
    require(len(unpacked) <= MAX_UNPACKED, "Published archive unpacked extent exceeds limit")
    archive = tarfile.open(fileobj=io.BytesIO(unpacked), mode="r:")
    for count, member in enumerate(archive, 1):
        require(count <= 4096 and member.size <= MAX_UNPACKED, "Published archive member bound exceeded")
    prefix = f"{entry['name']}-{entry['version']}/"
    member = archive.getmember(prefix + "Cargo.toml")
    require(member.isfile() and member.size <= 256 * 1024, "Invalid published manifest member")
    manifest = archive.extractfile(member).read(256 * 1024 + 1)
    require(len(manifest) <= 256 * 1024 and digest(manifest) == entry["published_manifest_sha256"], "Published manifest mismatch")
    package = tomllib.loads(manifest.decode())["package"]
    require((package["name"], package["version"]) == (entry["name"], entry["version"]), "Published base identity mismatch")
    return archive, prefix


def glib_mitigation(proof, entry, package, metadata, archive, prefix, root, db_root):
    require((proof["kind"], proof["id"], proof["package"], proof["version"], proof["source_file"], proof["advisory_path"]) ==
            ("glib-variant-out-argument-v1", "RUSTSEC-2024-0429", "glib", "0.18.5", "src/variant_iter.rs", "crates/glib/RUSTSEC-2024-0429.md"),
            "Unknown mitigation contract")
    # An ID ignore cannot hide another same-name package/version in this view.
    require([p["id"] for p in metadata["packages"] if p["name"] == "glib"] == [package["id"]], "GLib mitigation identity is not unique")
    original = archive.extractfile(prefix + proof["source_file"]).read(1024 * 1024 + 1)
    require(len(original) <= 1024 * 1024 and digest(original) == proof["published_file_sha256"], "GLib published source mismatch")
    old_pointer = b"let p: *mut libc::c_char = std::ptr::null_mut();"
    old_argument = b"                &p,\n"
    require(original.count(old_pointer) == original.count(old_argument) == 1, "GLib original fix precondition changed")
    corrected = original.replace(old_pointer, b"let mut p: *mut libc::c_char = std::ptr::null_mut();").replace(old_argument, b"                &mut p,\n")
    actual = read_regular(root / entry["path"] / proof["source_file"], 1024 * 1024)
    require(actual == corrected and digest(actual) == proof["patched_file_sha256"], "GLib security fix is absent or differs")
    advisories = list(db_root.glob("*/" + proof["advisory_path"]))
    require(len(advisories) == 1, "Expected one fresh RustSec advisory body")
    require(digest(read_regular(advisories[0], 256 * 1024)) == proof["advisory_sha256"], "GLib advisory changed; mitigation needs review")
    require(proof["evidence"]["logs"] and proof["upstream_fix"]["sha256"], "Missing mitigation evidence")
    return {"id": proof["id"], "package_id": package["id"], "source_tree": entry["local_source"],
            "advisory_sha256": proof["advisory_sha256"], "patched_file_sha256": proof["patched_file_sha256"],
            "upstream_fix": proof["upstream_fix"], "prior_execution_evidence": proof["evidence"]}


def advisory_policy(policy, db_root, mitigations):
    graph, advisories = policy["graph"], policy["advisories"]
    require(graph == {"targets": TARGETS, "all-features": False, "no-default-features": False,
                      "features": [], "exclude-dev": True, "exclude-unpublished": False, "exclude": []}, "Unsupported graph policy")
    require(set(advisories) == {"db-path", "db-urls", "unmaintained", "unsound", "yanked", "ignore", "unused-ignored-advisory"}, "Unsupported advisory policy field")
    require(advisories["ignore"] == [] and advisories["unmaintained"] == advisories["unsound"] == "all" and advisories["yanked"] == "deny", "Primary policy must retain no-ignore coverage")
    require(advisories["db-urls"] == ["https://github.com/RustSec/advisory-db"], "Unexpected advisory database")
    lines = ["# CONSERVATIVE PUBLISHED-BASE ADVISORY VIEW ONLY; not build/license metadata.", "[graph]"]
    for key, value in graph.items():
        lines.append(f"{key} = {json.dumps(value)}")
    lines.append("\n[advisories]")
    for key, value in advisories.items():
        if key == "db-path":
            value = str(db_root.resolve())
        if key == "ignore":
            lines.append("ignore = [")
            for proof in mitigations:
                reason = "Exact reviewed local GLib source/backport and advisory body verified; see upstream-view-report.json"
                lines.append(f"  {{ id = {json.dumps(proof['id'])}, reason = {json.dumps(reason)} }},")
            lines.append("]")
        else:
            lines.append(f"{key} = {json.dumps(value)}")
    return "\n".join(lines) + "\n"


def prepare(metadata, selection, provenance, root, policy, db_root, loader=fetch_archive):
    require(set(provenance) == {"version", "scope", "first_party", "packages", "mitigations"}, "Unknown provenance field")
    require(provenance["version"] == 1 and len(provenance["packages"]) <= 64 and len(metadata["packages"]) <= 4096, "Invalid provenance/metadata bounds")
    require(len(provenance["first_party"]) <= 16 and len(provenance["mitigations"]) <= 1, "Provenance entry bound exceeded")
    for entry in provenance["packages"]:
        require(set(entry) == {"name", "version", "path", "published_archive_sha256", "published_manifest_sha256", "published_checksum_api", "local_source"}, "Unknown published-base field")
        require(set(entry["local_source"]) == {"sha256", "files", "bytes"}, "Unknown source fingerprint field")
    for entry in provenance["first_party"]:
        require(set(entry) == {"name", "manifest"}, "Unknown first-party field")
        relative(entry["manifest"])
    for proof in provenance["mitigations"]:
        require(set(proof) == {"id", "package", "version", "kind", "advisory_path", "advisory_sha256", "source_file", "published_file_sha256", "patched_file_sha256", "upstream_fix", "evidence"}, "Unknown mitigation field")
        require((proof["id"], proof["package"], proof["version"], proof["kind"]) == ("RUSTSEC-2024-0429", "glib", "0.18.5", "glib-variant-out-argument-v1"), "Unreviewed mitigation identity")
    require(Path(metadata["workspace_root"]).resolve() == root.resolve(), "Metadata workspace does not match source root")
    selected = selected_ids(metadata, selection)
    first = {p["manifest"]: p["name"] for p in provenance["first_party"]}
    workspace = {p["id"]: p for p in metadata["packages"] if p["id"] in metadata["workspace_members"]}
    require(set(workspace) <= selected and len(workspace) == len(first), "Missing selected first-party roots")
    for package in workspace.values():
        manifest = str(Path(package["manifest_path"]).relative_to(root))
        require(first.get(manifest) == package["name"] and package["source"] is None, "Unknown first-party identity")
    entries = {entry["name"]: entry for entry in provenance["packages"]}
    require(len(entries) == len(provenance["packages"]), "Duplicate published-base mapping")
    projected, mapped, mitigated = copy.deepcopy(metadata), [], []
    archived_bytes, source_bytes = 0, 0
    for package in projected["packages"]:
        if package["id"] not in selected or package["source"] is not None or package["id"] in workspace:
            continue
        entry = entries.get(package["name"])
        require(entry is not None and entry["version"] == package["version"], "Selected local package has no verified published base")
        folder = root / relative(entry["path"])
        require(entry["path"].startswith("vendor/") and Path(package["manifest_path"]) == folder / "Cargo.toml", "Vendor manifest does not match mapping")
        actual = source_tree(folder)
        require(actual == entry["local_source"], f"Vendor source fingerprint changed: {entry['name']}; provenance review required")
        source_bytes += actual["bytes"]
        require(source_bytes <= 256 * 1024 * 1024, "Aggregate vendor fingerprint bound exceeded")
        archive_body = loader(entry)
        archived_bytes += len(archive_body)
        require(archived_bytes <= 64 * 1024 * 1024, "Aggregate published archive bound exceeded")
        archive, prefix = published_archive(entry, lambda _: archive_body)
        with archive:
            for proof in provenance["mitigations"]:
                if proof["package"] == package["name"]:
                    mitigated.append(glib_mitigation(proof, entry, package, metadata, archive, prefix, root, db_root))
        del archive, archive_body
        # Pinned krates0.21.2 repairs features only for the legacy registry spelling.
        # Sparse identifies the same crates.io base to cargo-deny without replacing
        # path-patched feature definitions with published registry features.
        package["source"] = SPARSE_IDENTITY
        mapped.append({"package_id": package["id"], "name": package["name"], "version": package["version"],
                       "actual_source": None, "advisory_only_source": SPARSE_IDENTITY,
                       "published_archive_sha256": entry["published_archive_sha256"], "local_source": actual})
    require(len({p["id"] for p in mitigated}) == len(mitigated), "Duplicate conditional mitigation")
    # No new IDs, versions, checksums, edges, feature maps or target predicates.
    restored = copy.deepcopy(projected)
    for package in restored["packages"]:
        if any(p["package_id"] == package["id"] for p in mapped):
            package["source"] = None
    require(restored == metadata, "Projection changed actual graph data")
    report = {"scope": "Conservative verified published-base advisory check; not actual patched-source/build/license metadata.",
              "selected_packages": len(selected), "selected_first_party": list(workspace), "mapped_vendor_packages": mapped,
              "conditional_source_mitigations": mitigated, "actual_graph_preserved_except_advisory_source": True,
              "source_bytes_hashed": source_bytes, "published_archive_bytes": archived_bytes}
    return projected, advisory_policy(policy, db_root, mitigated), report


def verify_selection(before, after):
    require(isinstance(before, dict) and before and before == after, "Actual/projected selected package or license listing differs")


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    sub = parser.add_subparsers(dest="operation", required=True)
    prepare_parser = sub.add_parser("prepare")
    for name in ["metadata", "selected", "provenance", "root", "policy", "advisory-db-root", "output"]:
        prepare_parser.add_argument("--" + name, type=Path, required=True)
    compare = sub.add_parser("verify-selection")
    compare.add_argument("before", type=Path)
    compare.add_argument("after", type=Path)
    args = parser.parse_args()
    if args.operation == "verify-selection":
        verify_selection(load_json(args.before, 4 * 1024 * 1024), load_json(args.after, 4 * 1024 * 1024))
        return
    actual_bytes = read_regular(args.metadata, 16 * 1024 * 1024)
    metadata = json.loads(actual_bytes, object_pairs_hook=object_pairs)
    provenance_bytes = read_regular(args.provenance, 1024 * 1024)
    provenance = json.loads(provenance_bytes, object_pairs_hook=object_pairs)
    policy = tomllib.loads(read_regular(args.policy, 64 * 1024).decode())
    projected, configuration, report = prepare(metadata, load_json(args.selected, 4 * 1024 * 1024), provenance,
                                                args.root.resolve(), policy, args.advisory_db_root)
    report.update({"actual_metadata_sha256": digest(actual_bytes), "provenance_sha256": digest(provenance_bytes),
                   "primary_policy_sha256": digest(read_regular(args.policy, 64 * 1024))})
    args.output.mkdir(parents=True, exist_ok=False)
    for name, content in [("upstream-advisory-only-view.json", json.dumps(projected)),
                          ("upstream-advisory-policy.toml", configuration),
                          ("upstream-view-report.json", json.dumps(report, indent=2))]:
        with (args.output / name).open("x") as stream:
            stream.write(content + "\n")


if __name__ == "__main__":
    main()
