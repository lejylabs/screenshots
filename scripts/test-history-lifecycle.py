#!/usr/bin/env python3
"""Exercise real Library CLI/filesystem/SQLite crash boundaries in private fixtures."""

import argparse
import contextlib
import hashlib
import json
import os
from pathlib import Path
import shutil
import sqlite3
import struct
import subprocess
import sys
import tempfile
import unicodedata
import zlib


def require(value, message):
    if not value:
        raise AssertionError(message)


def png():
    def chunk(kind, data):
        return struct.pack(">I", len(data)) + kind + data + struct.pack(">I", zlib.crc32(kind + data) & 0xffffffff)
    pixels = bytes([0, 1, 12, 123, 0, 9, 8, 7, 255, 4, 3, 2, 127]) * 2
    return (b"\x89PNG\r\n\x1a\n" + chunk(b"IHDR", struct.pack(">IIBBBBB", 3, 2, 8, 6, 0, 0, 0))
            + chunk(b"IDAT", zlib.compress(pixels)) + chunk(b"IEND", b""))


def run(binary, root, action, database, *arguments, fail=False, extra_env=None):
    env = dict(os.environ)
    for name in ["HOME", "USERPROFILE", "APPDATA", "LOCALAPPDATA", "XDG_CONFIG_HOME", "XDG_DATA_HOME", "XDG_CACHE_HOME"]:
        env[name] = str(root / ("unused-" + name))
    for name in ["DISPLAY", "WAYLAND_DISPLAY"]:
        env.pop(name, None)
    if extra_env:
        env.update(extra_env)
    result = subprocess.run([str(binary), "history-manage", action, str(database), *map(str, arguments)],
                            cwd=root, env=env, capture_output=True, timeout=45)
    require(len(result.stdout) <= 2 * 1024 * 1024 and len(result.stderr) <= 65536, "Excessive fixture command result")
    require((result.returncode != 0) if fail else (result.returncode == 0),
            f"Unexpected result for {action}: {result.returncode}, {result.stderr.decode(errors='replace')[:2048]}")
    values = [json.loads(line) for line in result.stdout.splitlines()]
    return values


def rows(database, sql, values=()):
    with contextlib.closing(sqlite3.connect(database)) as connection:
        return connection.execute(sql, values).fetchall()


def fixture_connection(database):
    connection = sqlite3.connect(database)
    def stamp_identity(payload):
        stamp = json.loads(payload)["stamp"]
        if stamp.get("file_id") is not None:
            return f"w:{stamp['device']:016x}:" + bytes(stamp["file_id"]).hex()
        return f"u:{stamp['device']:016x}:{stamp['inode']:016x}"
    connection.create_function("screenshots_stamp_identity", 1, stamp_identity, deterministic=True)
    return connection


def mutate(database, sql, values=()):
    with contextlib.closing(fixture_connection(database)) as connection:
        with connection:
            connection.execute(sql, values)


def pending(binary, root, database):
    return run(binary, root, "pending-move", database)[0]["pending"]


def execute(binary, root):
    database = root / "history.sqlite3"
    inputs, exported, trash = root / "inputs", root / "exports", root / "trash"
    for path in [inputs, exported, trash]:
        path.mkdir()
    original = png()
    files = [inputs / f"source-{index:02d}.png" for index in range(1, 26)]
    for path in files:
        path.write_bytes(original)
    require(run(binary, root, "import", database, *files)[0]["added"] == 25, "Fixture import failed")
    require(rows(database, "PRAGMA user_version")[0][0] == 9, "Lifecycle schema was not migrated")
    indexed = run(binary, root, "index-identities", database)[0]
    require(indexed["complete"] and indexed["checked"] == 25 and indexed["remaining"] == 0
            and all(path.read_bytes() == original for path in files), "Purposeful identity refresh did not finish without changing images")
    ids = [value[0] for value in rows(database, "SELECT id FROM captures ORDER BY id")]
    require(run(binary, root, "export", database, exported, "original", *ids)[0]["exported"] == 25, "Cross-page export failed")
    require(all((exported / path.name).read_bytes() == original for path in files), "Export changed pixels or metadata")
    renamed = inputs / "renamed.png"
    require(run(binary, root, "move", database, ids[0], renamed)[0]["status"] == "moved", "Native rename failed")
    require(not files[0].exists() and renamed.read_bytes() == original, "Rename bytes differ")
    cross_volume = False
    with contextlib.ExitStack() as stack:
        secondary = root / "other-volume"
        secondary.mkdir()
        if sys.platform.startswith("linux") and Path("/dev/shm").is_dir() and os.stat(root).st_dev != os.stat("/dev/shm").st_dev:
            secondary = Path(stack.enter_context(tempfile.TemporaryDirectory(prefix="screenshots-library-transfer-", dir="/dev/shm")))
            cross_volume = True
        destination = secondary / "transferred.png"
        require(run(binary, root, "transfer", database, ids[0], destination)[0]["status"] == "transferred", "Transfer failed")
        require(not renamed.exists() and destination.read_bytes() == original, "Transfer source/delete ordering failed")
        require(run(binary, root, "transfer", database, ids[0], files[0])[0]["status"] == "transferred", "Transfer back failed")
        require(not destination.exists() and files[0].read_bytes() == original, "Transfer back bytes differ")

    result = run(binary, root, "trash", database, trash, *ids)
    require(len(result) == 26 and result[-1]["trashed"] == 25, "Cross-page trash outcomes are incomplete")
    require(all(not path.exists() for path in files), "A trashed source still exists")
    first = run(binary, root, "trash-list", database)[0]
    second = run(binary, root, "trash-list", database, first["next_cursor"])[0]
    require(len(first["entries"]) == 20 and first["has_more"] and len(second["entries"]) == 5 and not second["has_more"],
            "Trash page boundaries differ")
    entries = first["entries"] + second["entries"]
    require(all(Path(value["trash_path"]).read_bytes() == original for value in entries), "Trash bytes differ")
    native_aliases = {}
    owned = Path(entries[0]["trash_path"])
    for label, alias in [("uppercase", owned.with_name(owned.name.upper())),
                         ("long_s", owned.with_name(owned.name.replace("s", "\u017f", 1))),
                         ("parent_case", Path(str(owned.parent).upper()) / owned.name)]:
        same = alias.exists() and alias.samefile(owned)
        native_aliases[label] = same
        if same:
            run(binary, root, "import", database, alias, fail=True)
        elif label != "parent_case":
            alias.write_bytes(original)
            run(binary, root, "import", database, alias)
            alias_id = rows(database, "SELECT id FROM captures")[0][0]
            run(binary, root, "remove", database, alias_id)
            require(alias.read_bytes() == original, "Distinct case-sensitive file was modified")
    restored = run(binary, root, "restore", database, entries[0]["trash_id"])[0]
    restored_path = Path(restored["destination_path"])
    require(restored_path.read_bytes() == original, "Restored bytes differ")

    reserved = Path(entries[2]["trash_path"])
    case_variant = reserved.with_name(reserved.name.upper())
    case_insensitive = case_variant.exists() and case_variant.samefile(reserved)
    parked = reserved.with_name(".fixture-preserved-trash.png")
    reserved.rename(parked)
    for destination in [reserved, *([case_variant] if case_insensitive else [])]:
        for action in ["move", "transfer"]:
            run(binary, root, action, database, restored["capture_id"], destination, fail=True)
            require(restored_path.read_bytes() == original and not reserved.exists() and pending(binary, root, database) is None,
                    "Missing trash metadata did not reserve the move/transfer destination")
        run(binary, root, "restore", database, entries[3]["trash_id"], destination, fail=True)
    require(Path(entries[3]["trash_path"]).read_bytes() == original and not reserved.exists()
            and pending(binary, root, database) is None, "Missing trash metadata did not reserve the restoration destination")
    parked.rename(reserved)

    victim = entries[1]
    Path(victim["trash_path"]).write_bytes(b"preserve user modification")
    run(binary, root, "purge", database, victim["trash_id"], fail=True)
    require(Path(victim["trash_path"]).read_bytes() == b"preserve user modification" and pending(binary, root, database) is None,
            "Changed trash was deleted or published intent")
    Path(victim["trash_path"]).write_bytes(original)
    require(run(binary, root, "purge", database, victim["trash_id"])[-1]["purged"] == 1, "Explicit purge failed")
    require(not Path(victim["trash_path"]).exists(), "Purged file remains")

    mutate(database, "CREATE TRIGGER fixture_interrupt_trash BEFORE INSERT ON history_trash BEGIN SELECT RAISE(ABORT,'fixture trash commit'); END")
    run(binary, root, "trash", database, trash, restored["capture_id"], fail=True)
    operation = pending(binary, root, database)
    require(operation["kind"] == "trash" and not Path(operation["source_path"]).exists()
            and Path(operation["destination_path"]).read_bytes() == original, "Trash crash boundary is not recoverable")
    mutate(database, "DROP TRIGGER fixture_interrupt_trash")
    require(run(binary, root, "recover-file", database, operation["operation_id"], "finish")[0]["status"] == "trashed", "Trash recovery failed")

    target_id = rows(database, "SELECT trash_id FROM history_trash ORDER BY sequence LIMIT 1")[0][0]
    mutate(database, f"CREATE TRIGGER fixture_interrupt_purge BEFORE DELETE ON history_trash WHEN old.trash_id='{target_id}' BEGIN SELECT RAISE(ABORT,'fixture purge commit'); END")
    run(binary, root, "purge", database, target_id, fail=True)
    operation = pending(binary, root, database)
    require(operation["kind"] == "purge" and not Path(operation["source_path"]).exists()
            and not Path(operation["destination_path"]).exists(), "Purge crash boundary is not recoverable")
    mutate(database, "DROP TRIGGER fixture_interrupt_purge")
    require(run(binary, root, "recover-file", database, operation["operation_id"], "finish")[0]["status"] == "purged", "Purge recovery failed")

    target_id = rows(database, "SELECT trash_id FROM history_trash ORDER BY sequence LIMIT 1")[0][0]
    restored = run(binary, root, "restore", database, target_id)[0]
    restored_path = Path(restored["destination_path"])
    destination = inputs / "interrupted-transfer.png"
    mutate(database, "CREATE TRIGGER fixture_interrupt_copy BEFORE UPDATE ON pending_file_move WHEN json_extract(new.payload,'$.destination_stamp') IS NOT NULL BEGIN SELECT RAISE(ABORT,'fixture copy commit'); END")
    run(binary, root, "transfer", database, restored["capture_id"], destination, fail=True)
    operation = pending(binary, root, database)
    value = json.loads(rows(database, "SELECT payload FROM pending_file_move")[0][0])
    stage = Path(value["action"]["stage_path"])
    require(operation["kind"] == "transfer" and restored_path.read_bytes() == original and stage.read_bytes() == original
            and not destination.exists(), "Copy fingerprint crash damaged source")
    mutate(database, "DROP TRIGGER fixture_interrupt_copy")
    canceled = run(binary, root, "recover-file", database, operation["operation_id"], "cancel")[0]
    require(Path(canceled["preserved_path"]).samefile(stage) and stage.read_bytes() == original
            and restored_path.read_bytes() == original, "Unverified copy was not preserved on cancel")

    mutate(database, "UPDATE captures SET source='capture',created_at='2020-01-01T00:00:00Z'")
    policy_path = root / "retention.json"
    policy_path.write_text(json.dumps({"version": 1, "trash_directory": str(trash), "capture_days": 30, "limit": 100}))
    preview = run(binary, root, "retention", database, policy_path, "preview")[0]
    require(preview["capture_ids"] == [restored["capture_id"]] and restored_path.exists(), "Retention preview changed files")
    report = run(binary, root, "retention", database, policy_path, "apply")[0]
    require(len(report["outcomes"]) == 1 and report["failure"] is None and not restored_path.exists(), "Capture retention failed")
    with contextlib.closing(fixture_connection(database)) as connection:
        with connection:
            for identifier, data in connection.execute("SELECT trash_id,payload FROM history_trash").fetchall():
                value = json.loads(data)
                value["trashed_at"] = "2020-01-01T00:00:00Z"
                connection.execute("UPDATE history_trash SET trashed_at=?,payload=? WHERE trash_id=?",
                                   (value["trashed_at"], json.dumps(value), identifier))
    policy_path.write_text(json.dumps({"version": 1, "purge_days": 7, "limit": 100}))
    preview = run(binary, root, "retention", database, policy_path, "preview")[0]
    victim_id = preview["trash_ids"][0]
    victim_data = json.loads(rows(database, "SELECT payload FROM history_trash WHERE trash_id=?", (victim_id,))[0][0])
    victim_path = Path(victim_data["trash_path"])
    victim_path.write_bytes(b"preserve changed retention file")
    report = run(binary, root, "retention", database, policy_path, "apply", fail=True)[0]
    require(report["failure"] and not report["outcomes"] and victim_path.read_bytes() == b"preserve changed retention file",
            "Retention deleted a changed file")
    victim_path.write_bytes(original)
    report = run(binary, root, "retention", database, policy_path, "apply")[0]
    require(report["failure"] is None and len(report["outcomes"]) == len(preview["trash_ids"]), "Purge retention outcomes differ")
    require(rows(database, "SELECT count(*) FROM history_trash")[0][0] == 0 and pending(binary, root, database) is None,
            "Retention did not finalize all metadata")

    missing = [inputs / f"missing-{number:02d}.png" for number in range(25)]
    for path in missing:
        path.write_bytes(original)
    run(binary, root, "import", database, *missing)
    selected = [value[0] for value in rows(database, "SELECT id FROM captures ORDER BY id")]
    for path in missing[1:]:
        path.unlink()
    run(binary, root, "remove-missing", database, *selected, fail=True)
    require(rows(database, "SELECT count(*) FROM captures")[0][0] == 25, "Ambiguous missing reconciliation was not atomic")
    require(run(binary, root, "remove-missing", database, *selected[1:])[0]["removed"] == 24, "Cross-page missing reconciliation failed")
    require(run(binary, root, "remove", database, selected[0])[0]["removed"] == 1 and missing[0].read_bytes() == original,
            "Metadata removal modified an image")
    require(all((exported / path.name).read_bytes() == original for path in files), "Unrelated export bytes changed")
    for name in ["HOME", "USERPROFILE", "APPDATA", "LOCALAPPDATA", "XDG_CONFIG_HOME", "XDG_DATA_HOME", "XDG_CACHE_HOME"]:
        require(not (root / ("unused-" + name)).exists(), "Unrequested user state was created")
    return {"status": "passed", "real_cross_volume_transfer": cross_volume, "cross_page_items": 25,
            "crash_boundaries": ["trash finalization", "purge finalization", "copy fingerprint commit"],
            "user_state": "unchanged", "desktop_services": "not opened", "native_aliases_resolved": native_aliases}


def native_faults(binary, root):
    require(sys.platform.startswith("linux") and shutil.which("cc"), "Native fsync fault fixture requires Linux and a C compiler")
    root.mkdir()
    library = root / "history-fault.so"
    source = Path(__file__).with_name("history-fault-preload.c").resolve()
    subprocess.run(["cc", "-shared", "-fPIC", "-O2", str(source), "-ldl", "-o", str(library)],
                   check=True, capture_output=True, timeout=30)
    inputs, trash, output = root / "inputs", root / "trash", root / "output"
    for path in [inputs, trash, output]:
        path.mkdir()
    database = root / "history.sqlite3"
    original = png()
    file = inputs / "source.png"
    file.write_bytes(original)
    run(binary, root, "import", database, file)
    identifier = run(binary, root, "trash", database, trash, 1)[0]["trash_id"]
    log = root / "purge-fault.log"
    env = {"LD_PRELOAD": str(library), "SCREENSHOTS_FIXTURE_FAULT_MODE": "after_unlink",
           "SCREENSHOTS_FIXTURE_UNLINK_PREFIX": ".screenshots-purge-", "SCREENSHOTS_FIXTURE_FAULT_LOG": str(log)}
    run(binary, root, "purge", database, identifier, fail=True, extra_env=env)
    require(log.read_text().splitlines() == ["native unlink completed", "native parent fsync rejected"], "Purge did not fail after real unlink and before parent sync")
    operation = pending(binary, root, database)
    require(operation["kind"] == "purge" and not Path(operation["source_path"]).exists()
            and not Path(operation["destination_path"]).exists(), "Purge native fault left unexpected paths")
    retry_log = root / "purge-retry.log"
    retry = {"LD_PRELOAD": str(library), "SCREENSHOTS_FIXTURE_FAULT_MODE": "parent_barrier",
             "SCREENSHOTS_FIXTURE_PARENT": str(trash), "SCREENSHOTS_FIXTURE_FAULT_LOG": str(retry_log)}
    run(binary, root, "recover-file", database, operation["operation_id"], "finish", fail=True, extra_env=retry)
    require("native parent fsync rejected" in retry_log.read_text() and pending(binary, root, database)["operation_id"] == operation["operation_id"],
            "Purge recovery forgot intent before its missing-path parent barrier")
    require(rows(database, "SELECT count(*) FROM history_trash")[0][0] == 1, "Purge recovery lost trash metadata before synchronization")
    run(binary, root, "recover-file", database, operation["operation_id"], "finish")

    file.write_bytes(original)
    run(binary, root, "import", database, file)
    restore_id = run(binary, root, "trash", database, trash, 1)[0]["trash_id"]
    mutate(database, "CREATE TRIGGER fixture_interrupt_restore BEFORE INSERT ON captures WHEN new.id>0 BEGIN SELECT RAISE(ABORT,'fixture restore commit'); END")
    run(binary, root, "restore", database, restore_id, fail=True)
    operation = pending(binary, root, database)
    require(operation["kind"] == "restore", "Restore interruption did not retain intent")
    mutate(database, "DROP TRIGGER fixture_interrupt_restore")
    next_file = inputs / "next-capture.png"
    next_file.write_bytes(original)
    run(binary, root, "import", database, next_file, fail=True)
    require(rows(database, "SELECT count(*) FROM captures")[0][0] == 0 and next_file.read_bytes() == original
            and pending(binary, root, database)["operation_id"] == operation["operation_id"],
            "Normal automatic ID allocation claimed the reserved restore ID")
    run(binary, root, "recover-file", database, operation["operation_id"], "finish")
    run(binary, root, "import", database, next_file)

    file.write_bytes(original)
    run(binary, root, "import", database, file)
    destination = output / "transferred.png"
    log = root / "transfer-fault.log"
    env.update(SCREENSHOTS_FIXTURE_UNLINK_PREFIX="source.png", SCREENSHOTS_FIXTURE_FAULT_LOG=str(log))
    run(binary, root, "transfer", database, 1, destination, fail=True, extra_env=env)
    require(log.read_text().splitlines() == ["native unlink completed", "native parent fsync rejected"], "Transfer did not fail after real source unlink and before parent sync")
    operation = pending(binary, root, database)
    require(operation["kind"] == "transfer" and not file.exists() and destination.read_bytes() == original, "Transfer native fault damaged verified copy")
    retry_log = root / "transfer-retry.log"
    retry.update(SCREENSHOTS_FIXTURE_PARENT=str(inputs), SCREENSHOTS_FIXTURE_FAULT_LOG=str(retry_log))
    run(binary, root, "recover-file", database, operation["operation_id"], "finish", fail=True, extra_env=retry)
    require("native parent fsync rejected" in retry_log.read_text() and pending(binary, root, database)["operation_id"] == operation["operation_id"],
            "Transfer recovery forgot intent before the source-parent barrier")
    run(binary, root, "recover-file", database, operation["operation_id"], "finish")
    require(pending(binary, root, database) is None and destination.read_bytes() == original, "Transfer recovery did not finish after barrier success")
    for choice in ["finish", "cancel"]:
        stage_database = root / f"stage-{choice}.sqlite3"
        stage_source, other = inputs / f"stage-{choice}.png", inputs / f"other-{choice}.png"
        stage_source.write_bytes(original)
        other.write_bytes(original)
        run(binary, root, "import", stage_database, stage_source, other)
        stage_destination = output / f"stage-final-{choice}.png"
        stage_log = root / f"stage-{choice}.log"
        stage_env = {"LD_PRELOAD": str(library), "SCREENSHOTS_FIXTURE_FAULT_MODE": "stage_publish",
                     "SCREENSHOTS_FIXTURE_FAULT_LOG": str(stage_log)}
        run(binary, root, "transfer", stage_database, 1, stage_destination, fail=True, extra_env=stage_env)
        require(stage_log.read_text().splitlines() == ["verified staging publish rejected"], "Verified stage publication fault was not reached")
        operation = pending(binary, root, stage_database)
        payload = json.loads(rows(stage_database, "SELECT payload FROM pending_file_move")[0][0])
        stage = Path(payload["action"]["stage_path"])
        require(payload.get("destination_stamp") and stage.read_bytes() == original, "Stage fingerprint was not committed before fault")
        run(binary, root, "import", stage_database, stage, fail=True)
        run(binary, root, "relink", stage_database, 2, stage, fail=True)
        require(stage.read_bytes() == original and other.read_bytes() == original, "Stage guards changed a referenced image")

        # Reproduce a legacy adopter in this private fixture, bypassing only insert guards.
        trigger_names = ["captures_native_reserve_insert", "captures_reservation_keys_insert"]
        triggers = rows(stage_database, "SELECT name,sql FROM sqlite_schema WHERE type='trigger' AND name IN (?,?)", trigger_names)
        for name in trigger_names:
            mutate(stage_database, "DROP TRIGGER " + name)
        adopted = stage
        if choice == "finish":
            adopted = inputs / "arbitrary-legacy-reference.png"
            adopted.symlink_to(stage)
        projected = unicodedata.normalize("NFKD", unicodedata.normalize("NFKD", adopted.name).upper())
        projected = "".join(character for character in projected if character.isascii()).rstrip(" .")
        key = "v1:" + hashlib.sha256(b"screenshots-reservation-name-v1\0" + projected.encode()).hexdigest()
        mutate(stage_database, "INSERT INTO captures(id,file_path,mime_type,width,height,file_size,created_at,source,reservation_name) VALUES(3,?,'image/png',3,2,?,'2020-01-01T00:00:00Z','import',?)",
               (str(adopted), stage.stat().st_size, key))
        for _, sql in triggers:
            mutate(stage_database, sql)
        run(binary, root, "recover-file", stage_database, operation["operation_id"], choice, fail=True)
        require(adopted.read_bytes() == original and stage.read_bytes() == original and stage_source.read_bytes() == original
                and pending(binary, root, stage_database)["operation_id"] == operation["operation_id"], "Recovery moved/deleted an adopted stage")
        run(binary, root, "remove", stage_database, 3)
        run(binary, root, "recover-file", stage_database, operation["operation_id"], choice)
        require((stage_destination if choice == "finish" else stage_source).read_bytes() == original,
                "Unadopted stage did not recover after explicit metadata resolution")
    return {"status": "passed", "mechanism": "private Linux LD_PRELOAD fixture, actual unlinkat followed by injected fsync EIO",
            "recovered_boundaries": ["purge unlink before parent sync", "transfer source unlink before parent sync"],
            "failed_recovery_kept_intent": True, "automatic_capture_id_reservation": "passed",
            "verified_stage_reservations_and_legacy_adoption": ["finish", "cancel"]}


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--binary", type=Path, required=True)
    parser.add_argument("--report", type=Path)
    parser.add_argument("--native-faults", action="store_true", help="Linux-only actual unlink/fsync failure injection using a private C fixture")
    args = parser.parse_args()
    binary = args.binary.resolve(strict=True)
    with tempfile.TemporaryDirectory(prefix="screenshots-history-lifecycle-") as temporary:
        root = Path(temporary).resolve()
        report = execute(binary, root)
        if args.native_faults:
            report["native_faults"] = native_faults(binary, root / "native-faults")
    with binary.open("rb") as source:
        digest = hashlib.file_digest(source, "sha256").hexdigest()
    report.update(binary_sha256=digest, platform=sys.platform)
    payload = json.dumps(report, indent=2) + "\n"
    if args.report:
        args.report.parent.mkdir(parents=True, exist_ok=True)
        args.report.write_text(payload)
    print(payload, end="")


if __name__ == "__main__":
    main()
