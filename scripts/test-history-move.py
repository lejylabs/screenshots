#!/usr/bin/env python3
"""Verify Linux history file moves and explicit recovery with synthetic files.

Only a private temporary database and PNG are modified. No GUI, user history,
clipboard, credentials, external requests or production fault flags are used.
"""

import argparse
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
import time
import zlib

REPO = Path(__file__).resolve().parents[1]
STATE = ("HOME", "USERPROFILE", "APPDATA", "LOCALAPPDATA", "XDG_CONFIG_HOME",
         "XDG_DATA_HOME", "XDG_CACHE_HOME")
FAULT_TRIGGER = """CREATE TRIGGER fixture_abort_move BEFORE UPDATE OF file_path ON captures
BEGIN SELECT RAISE(ABORT,'synthetic finalization failure'); END"""


def require(condition, message):
    if not condition:
        raise RuntimeError(message)


def digest(path):
    value = hashlib.sha256()
    with path.open("rb") as source:
        for chunk in iter(lambda: source.read(1024 * 1024), b""):
            value.update(chunk)
    return value.hexdigest()


def tiny_png():
    def chunk(kind, payload):
        return (struct.pack(">I", len(payload)) + kind + payload
                + struct.pack(">I", zlib.crc32(kind + payload) & 0xFFFFFFFF))
    rgba = bytes((3, 19, 79, 0, 31, 47, 63, 128, 89, 107, 131, 255))
    return (b"\x89PNG\r\n\x1a\n"
            + chunk(b"IHDR", struct.pack(">IIBBBBB", 3, 1, 8, 6, 0, 0, 0))
            + chunk(b"IDAT", zlib.compress(b"\0" + rgba)) + chunk(b"IEND", b""))


def run(binary, env, action, database, *arguments, expect_success=True):
    result = subprocess.run([binary, "history-manage", action, str(database),
                             *map(str, arguments)], capture_output=True, env=env, timeout=15)
    require(len(result.stdout) <= 65536 and len(result.stderr) <= 65536,
            "CLI output exceeded test limits")
    if expect_success:
        require(result.returncode == 0, f"History {action} failed")
        require(not result.stderr, f"History {action} emitted diagnostics")
        return json.loads(result.stdout)
    require(result.returncode != 0 and not result.stdout, f"History {action} falsely succeeded")
    return result


def row(database, capture_id):
    with sqlite3.connect(database) as connection:
        value = connection.execute("SELECT id,file_path,thumbnail_path,mime_type,width,height,"
                                   "file_size,created_at,source FROM captures WHERE id=?",
                                   (capture_id,)).fetchone()
    require(value is not None, "Capture row disappeared")
    return value


def sql(database, command):
    with sqlite3.connect(database) as connection:
        connection.execute(command)


def journal_snapshot(database):
    with sqlite3.connect(database) as connection:
        return connection.execute("SELECT * FROM pending_file_move").fetchall()


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--binary", default=str(REPO / "target/release/screenshots"))
    parser.add_argument("--report", type=Path)
    args = parser.parse_args()
    require(sys.platform.startswith("linux"), "History move fixture requires Linux")
    binary = shutil.which(args.binary)
    require(binary is not None, "screenshots binary is unavailable")
    binary = str(Path(binary).resolve())
    started = time.monotonic()
    with tempfile.TemporaryDirectory(prefix="screenshots-history-move-") as temporary:
        root = Path(temporary).resolve()
        env = dict(os.environ)
        env.update({name: str(root / name.lower()) for name in STATE})
        for name in ("DISPLAY", "WAYLAND_DISPLAY", "DBUS_SESSION_BUS_ADDRESS"):
            env.pop(name, None)
        source = root / "source.png"
        source.write_bytes(tiny_png())
        original_bytes = source.read_bytes()
        original_identity = (source.stat().st_dev, source.stat().st_ino)
        database = root / "history.sqlite3"
        imported = run(binary, env, "import", database, source)
        require(imported["added"] == 1 and imported["failed"] == 0, "Import failed")
        with sqlite3.connect(database) as connection:
            capture_id = connection.execute("SELECT id FROM captures").fetchone()[0]
        original_row = row(database, capture_id)
        destination = root / "renamed.png"
        moved = run(binary, env, "move", database, capture_id, destination)
        require(moved["status"] == "moved" and moved["capture_id"] == capture_id,
                "Normal move returned incorrect outcome")
        require(not source.exists() and destination.read_bytes() == original_bytes,
                "Normal move did not preserve bytes")
        require((destination.stat().st_dev, destination.stat().st_ino) == original_identity,
                "Normal move copied or replaced the original file")
        expected = list(original_row)
        expected[1] = str(destination)
        expected[2] = None
        require(row(database, capture_id) == tuple(expected), "Normal move changed capture metadata")
        require(not journal_snapshot(database), "Normal move left a pending journal")
        with sqlite3.connect(database) as connection:
            matches = connection.execute("SELECT rowid FROM captures_search WHERE captures_search MATCH 'renamed'").fetchall()
        require(matches == [(capture_id,)], "FTS did not update the moved path")

        collision = root / "collision.png"
        collision.write_bytes(b"existing destination")
        run(binary, env, "move", database, capture_id, collision, expect_success=False)
        require(collision.read_bytes() == b"existing destination"
                and destination.read_bytes() == original_bytes and not journal_snapshot(database),
                "Collision modified files or prepared a journal")

        for action in ("finish", "cancel"):
            before = row(database, capture_id)
            old = Path(before[1])
            new = root / f"recover-{action}.png"
            sql(database, FAULT_TRIGGER)
            failed = run(binary, env, "move", database, capture_id, new, expect_success=False)
            require(not old.exists() and new.read_bytes() == original_bytes,
                    "Injected finalization failure did not leave a renamed file")
            require(row(database, capture_id) == before, "Failed finalization partially updated the row")
            before_journal = journal_snapshot(database)
            pending = run(binary, env, "pending-move", database)["pending"]
            require(pending is not None and pending["capture_id"] == capture_id,
                    "Failed finalization lost its pending journal")
            require(pending["operation_id"].encode() in failed.stderr,
                    "Failure omitted the recovery operation ID")
            require(run(binary, env, "pending-move", database)["pending"] == pending
                    and journal_snapshot(database) == before_journal and row(database, capture_id) == before,
                    "Pending inspection changed the database")
            sql(database, "DROP TRIGGER fixture_abort_move")
            if action == "cancel":
                # Private fixture only: restore the pre-rename filesystem state.
                # Production recovery itself must never perform this rename.
                os.rename(new, old)
            recovered = run(binary, env, "recover-move", database, pending["operation_id"], action)
            require(recovered["status"] == ("moved" if action == "finish" else "canceled"),
                    "Recovery returned incorrect outcome")
            require(not journal_snapshot(database), "Recovery left the journal pending")
            expected = list(before)
            if action == "finish":
                expected[1] = str(new)
                expected[2] = None
            require(row(database, capture_id) == tuple(expected), "Recovery changed unrelated row fields")
            active = new if action == "finish" else old
            absent = old if action == "finish" else new
            require(active.read_bytes() == original_bytes and not absent.exists(),
                    "Recovery moved, deleted or corrupted a file")
            require((active.stat().st_dev, active.stat().st_ino) == original_identity,
                    "Recovery changed the original inode")
            run(binary, env, "recover-move", database, pending["operation_id"], action,
                expect_success=False)
            require(active.read_bytes() == original_bytes, "Repeated recovery changed the file")
        for name in STATE:
            require(not Path(env[name]).exists(), "Fixture created unrelated user state")
        require(not any(path.suffix == ".part" for path in root.iterdir()),
                "Fixture left an unexpected staging file")
    report = {"passed": True, "binary_sha256": digest(Path(binary)),
              "normal_move_same_bytes_inode": True, "row_id_metadata_preserved": True,
              "fts_updated": True, "collision_preserved": True,
              "failed_finalize_retained_journal": True, "pending_read_only": True,
              "finish_recovery": True, "cancel_recovery": True,
              "source_user_state_unchanged": True,
              "elapsed_seconds": round(time.monotonic() - started, 3)}
    if args.report:
        args.report.parent.mkdir(parents=True, exist_ok=True)
        args.report.write_text(json.dumps(report, indent=2) + "\n")
    print(json.dumps(report))


if __name__ == "__main__":
    try:
        main()
    except (RuntimeError, OSError, ValueError, sqlite3.Error, subprocess.SubprocessError) as error:
        print(f"History move fixture failed: {error}", file=sys.stderr)
        raise SystemExit(1)
