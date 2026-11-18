#!/usr/bin/env python3
"""Synthetic provenance controls; no Cargo, network, native API or account."""

import copy
import importlib.util
import io
import json
import os
from pathlib import Path
import tarfile
import tempfile
import unittest
from unittest import mock


spec = importlib.util.spec_from_file_location("view", Path(__file__).with_name("vendored-advisory-view.py"))
view = importlib.util.module_from_spec(spec)
spec.loader.exec_module(view)


class Controls(unittest.TestCase):
    def setUp(self):
        parent = Path(os.environ["SCREENSHOTS_POLICY_TEST_ROOT"])
        parent.mkdir(parents=True, exist_ok=True)
        self.owned = tempfile.TemporaryDirectory(dir=parent)
        self.addCleanup(self.owned.cleanup)
        self.root = Path(self.owned.name).resolve()
        self.folder = self.root / "vendor/glib"
        (self.folder / "src").mkdir(parents=True)
        self.original = (b"let p: *mut libc::c_char = std::ptr::null_mut();\n"
                         b"                &p,\n")
        self.fixed = self.original.replace(b"let p:", b"let mut p:").replace(b"&p,", b"&mut p,")
        self.manifest = b'[package]\nname="glib"\nversion="0.18.5"\n'
        (self.folder / "Cargo.toml").write_bytes(self.manifest)
        (self.folder / "src/variant_iter.rs").write_bytes(self.fixed)
        memory = io.BytesIO()
        with tarfile.open(fileobj=memory, mode="w:gz") as archive:
            for name, body in [("Cargo.toml", self.manifest), ("src/variant_iter.rs", self.original)]:
                member = tarfile.TarInfo("glib-0.18.5/" + name)
                member.size = len(body)
                archive.addfile(member, io.BytesIO(body))
        self.archive = memory.getvalue()
        self.app = "path+file://" + str(self.root / "app") + "#app@0.1.0"
        self.vendor = "path+file://" + str(self.folder) + "#glib@0.18.5"
        self.metadata = {"workspace_root": str(self.root), "workspace_members": [self.app],
                         "packages": [{"id": self.app, "name": "app", "version": "0.1.0", "source": None,
                                       "manifest_path": str(self.root / "app/Cargo.toml"), "features": {}},
                                      {"id": self.vendor, "name": "glib", "version": "0.18.5", "source": None,
                                       "manifest_path": str(self.folder / "Cargo.toml"), "features": {"native": []}}],
                         "resolve": {"nodes": [{"id": self.app, "deps": [{"pkg": self.vendor}], "features": []},
                                               {"id": self.vendor, "deps": [], "features": ["native"]}]}}
        self.selection = {f"{p['name']} {p['version']} {p['id'].rsplit('#', 1)[0]}": {"licenses": ["MIT"]}
                          for p in self.metadata["packages"]}
        self.entry = {"name": "glib", "version": "0.18.5", "path": "vendor/glib",
                      "published_archive_sha256": view.digest(self.archive), "published_manifest_sha256": view.digest(self.manifest),
                      "published_checksum_api": "https://crates.io/api/v1/crates/glib/0.18.5", "local_source": view.source_tree(self.folder)}
        self.provenance = {"version": 1, "scope": "synthetic", "first_party": [{"name": "app", "manifest": "app/Cargo.toml"}],
                           "packages": [self.entry], "mitigations": []}
        self.policy = {"graph": {"targets": view.TARGETS, "all-features": False, "no-default-features": False,
                                 "features": [], "exclude-dev": True, "exclude-unpublished": False, "exclude": []},
                       "advisories": {"db-path": "unused", "db-urls": ["https://github.com/RustSec/advisory-db"],
                                      "unmaintained": "all", "unsound": "all", "yanked": "deny", "ignore": [],
                                      "unused-ignored-advisory": "deny"}}
        self.database = self.root / "databases"
        advisory = self.database / "owned/crates/glib/RUSTSEC-2024-0429.md"
        advisory.parent.mkdir(parents=True)
        advisory.write_text("synthetic reviewed advisory body")
        self.advisory = advisory

    def prepare(self):
        return view.prepare(self.metadata, self.selection, self.provenance, self.root, self.policy,
                            self.database, lambda entry: self.archive)

    def mitigation(self):
        self.provenance["mitigations"] = [{"id": "RUSTSEC-2024-0429", "package": "glib", "version": "0.18.5",
            "kind": "glib-variant-out-argument-v1", "advisory_path": "crates/glib/RUSTSEC-2024-0429.md",
            "advisory_sha256": view.digest(self.advisory.read_bytes()), "source_file": "src/variant_iter.rs",
            "published_file_sha256": view.digest(self.original), "patched_file_sha256": view.digest(self.fixed),
            "upstream_fix": {"url": "https://example.invalid/synthetic", "sha256": "f" * 64},
            "evidence": {"scope": "synthetic", "logs": [{"sha256": "a" * 64}]}}]

    def test_only_advisory_identity_changes(self):
        snapshot = copy.deepcopy(self.metadata)
        projected, policy, report = self.prepare()
        self.assertEqual(self.metadata, snapshot)
        self.assertIsNone(projected["packages"][0]["source"])
        self.assertEqual(projected["packages"][1]["source"], view.SPARSE_IDENTITY)
        projected["packages"][1]["source"] = None
        self.assertEqual(projected, snapshot)
        self.assertEqual(report["selected_packages"], 2)
        self.assertIn("ignore = [\n]", policy)

    def test_missing_mapping_is_failure(self):
        self.provenance["packages"] = []
        with self.assertRaisesRegex(ValueError, "no verified published base"):
            self.prepare()

    def test_wrong_archive_fails_before_projection(self):
        self.archive += b"changed"
        with self.assertRaisesRegex(ValueError, "archive checksum"):
            self.prepare()

    def test_unpacked_archive_extent_is_bounded(self):
        with mock.patch.object(view, "MAX_UNPACKED", 1024):
            with self.assertRaisesRegex(ValueError, "unpacked extent"):
                self.prepare()

    def test_changed_local_source_is_failure(self):
        (self.folder / "src/variant_iter.rs").write_bytes(self.original)
        with self.assertRaisesRegex(ValueError, "fingerprint changed"):
            self.prepare()

    def test_unfixed_source_cannot_receive_mitigation(self):
        self.mitigation()
        (self.folder / "src/variant_iter.rs").write_bytes(self.original)
        self.entry["local_source"] = view.source_tree(self.folder)
        with self.assertRaisesRegex(ValueError, "security fix is absent"):
            self.prepare()

    def test_exact_source_mitigation_is_explicit(self):
        self.mitigation()
        _, policy, report = self.prepare()
        self.assertIn('id = "RUSTSEC-2024-0429"', policy)
        self.assertEqual(len(report["conditional_source_mitigations"]), 1)

    def test_changed_advisory_reopens_mitigation(self):
        self.mitigation()
        self.advisory.write_text("new advisory scope")
        with self.assertRaisesRegex(ValueError, "advisory changed"):
            self.prepare()

    def test_other_same_name_package_prevents_id_mitigation(self):
        self.mitigation()
        duplicate = dict(self.metadata["packages"][1], id="registry+https://example.invalid#glib@0.18.4", version="0.18.4", source="registry+https://example.invalid")
        self.metadata["packages"].append(duplicate)
        with self.assertRaisesRegex(ValueError, "not unique"):
            self.prepare()

    def test_unknown_advisory_cannot_be_added_to_mitigation(self):
        self.mitigation()
        self.provenance["mitigations"][0]["id"] = "RUSTSEC-2099-9999"
        with self.assertRaisesRegex(ValueError, "Unreviewed mitigation"):
            self.prepare()

    def test_selection_cannot_drop_first_party(self):
        self.selection.pop(next(k for k in self.selection if k.startswith("app ")))
        with self.assertRaisesRegex(ValueError, "first-party roots"):
            self.prepare()

    def test_changed_selected_listing_is_failure(self):
        after = copy.deepcopy(self.selection)
        after.pop(next(iter(after)))
        with self.assertRaisesRegex(ValueError, "listing differs"):
            view.verify_selection(self.selection, after)
        view.verify_selection(self.selection, self.selection)

    def test_unknown_provenance_fields_fail(self):
        self.provenance["skip"] = True
        with self.assertRaisesRegex(ValueError, "Unknown provenance"):
            self.prepare()

    def test_symlink_source_is_failure(self):
        (self.folder / "link").symlink_to(self.folder / "Cargo.toml")
        with self.assertRaisesRegex(ValueError, "symlink"):
            self.prepare()

    def test_duplicate_json_is_failure(self):
        with self.assertRaisesRegex(ValueError, "Duplicate JSON"):
            json.loads('{"version":1,"version":2}', object_pairs_hook=view.object_pairs)


if __name__ == "__main__":
    unittest.main()
