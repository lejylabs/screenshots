# Explicit history maintenance

History maintenance is an explicit worker/headless operation. It adds no startup
scan, background cleanup, timer, cache or new runtime. The public core functions
are `check_history_database`, `backup_history_database` and `inspect_part_files`.
They never delete capture images or history records.

## Inspect a database

`check_history_database(path)` opens an existing regular file with SQLite's
read-only and no-follow flags. Existing parent directories are canonicalized
before opening, so operating-system aliases such as macOS `/var` and a selected
folder reached through a symlink work. The final database component is checked
without following it and must be a regular file; final symlinks remain rejected.
Missing paths are not created. No application
migrations, WAL-mode changes or history writes run. It accepts the known history
schema versions 1–9, checks the expected columns and runs `quick_check(1)`.
Version 5 also requires the pending-file-move table and reservation triggers.
Version 6 additionally checks the indexed trash table and capture/trash guards
for the explicit [physical Library lifecycle](history-lifecycle.md). Version 7
also checks the after-insert guard that reserves SQLite-assigned capture IDs.
Version 8 checks the indexed native identity/name metadata and capture reservation
triggers, including transfer staging paths. Read-only checks need no registration
of the mutation-only filesystem functions. Version 9 also requires the partial
unresolved-identity index used by bounded physical-operation preparation.
Unknown versions, foreign databases without the expected schema and corruption
fail explicitly. A successful result gives the schema version and `ok`; it is
not a complete filesystem or power-loss durability guarantee.

Maintenance currently admits at most 1 GiB of database pages. Inspection uses
SQLite's instruction progress hook with a five-second deadline and a 100 ms
busy timeout. Filesystem/native SQLite calls cannot be forcibly interrupted;
the deadline is observed at SQLite progress callbacks. SQLite may need its
normal shared-memory support to read an active WAL database. Never replace this
with `immutable=1`: doing so can ignore committed WAL data.

## Back up or recover into a new path

`backup_history_database(source, destination, cancelled)` uses SQLite's online
backup API while holding a consistent read snapshot of the source. This includes
committed records still in its WAL. It checks the source, copies at most 64 pages
per step and checks cancellation between steps. Busy/locked sources fail with a
retry message rather than spinning. The operation has a 30-second deadline,
observed between native calls. It verifies the copied database before publication.

The destination directory must already exist and is canonicalized before
staging and publication. The destination and its `-wal`,
`-shm` and `-journal` siblings must be absent. Existing files, including dangling
symlinks, are never replaced. A uniquely named staging database is converted to
standalone journal mode, closed and synced, then published without replacement.
Normal failure removes only this job's staging file and sidecars. Crash/power
loss can leave a staging candidate for explicit inspection. Filesystems without
supported no-replace publication fail; there is no partial-copy fallback.

Recovery uses the same operation with a known backup as the source and a **new**
destination. There is deliberately no in-place restore API: replacing an open
database or leaving old WAL/SHM files next to a replacement can lose data.
Close the application before manually choosing which verified database to use;
keep the old database and its sidecars together until recovery is confirmed.
This feature does not automatically reconcile orphan images/history rows or
replace migration backup/recovery acceptance on every supported platform.

## Inspect possible unfinished files

`inspect_part_files(directory, cancelled)` inspects one regular directory without
recursion. It visits at most 10,000 entries, returns at most 100 candidates and
checks cancellation/deadline between entries. Directory access can itself block
inside the operating system. Only regular files named `.screenshots-<UUID>.part`
or `settings-<UUID>.part` with a canonical lowercase UUID are returned. Symlink
entries and nested directories are not followed. The result reports the number
of visited entries and whether a limit truncated inspection.

Inspection stops after five seconds between entries and sorts returned paths.
Cancellation and I/O errors fail the operation instead of returning a partial
success. CLI candidate paths are explicitly labeled as lossy UTF-8 display text;
invalid native path bytes become replacement characters. Use the native core API
when exact path identity is required. This command never acts on displayed paths.

These are **candidates**, not proof of abandonment: another application process
may still be writing one. Inspection never opens, decodes, modifies or deletes
their contents. Unknown filenames are left alone. No age threshold or filename
can establish that a file is safe to delete, so automatic cleanup is not offered.

## Verification scope

Deterministic core fixtures cover WAL-aware standalone backups, reopening the
copy, source-byte preservation, no overwrite, sidecar rejection, cancellation
cleanup, missing/foreign/future databases and symlink handling. Candidate fixtures
cover nonrecursive bounded discovery and preservation of every file. These
fixtures use temporary databases and no user settings or capture images. Actual
test commands/results belong to the delivery report; this document does not
claim that an unrun test passed.

Power loss, unavailable network/removable drives, hostile concurrent directory
replacement and platform-specific filesystem behavior remain native acceptance
cases. Paths should be chosen in user-controlled storage, not directories writable
by another user. No claim is made that standard path-based APIs isolate a job
from every privileged or same-user filesystem race.

The [durable scheduler](capture-scheduling.md) uses its own private SQLite
database and singleton run-intent recovery, separate from capture History.
History `storage check`/backup schema admission does not treat a schedule
database as a History database. Inspect and resolve uncertain run IDs explicitly;
maintenance never replays scheduled Tasks or uploads.
