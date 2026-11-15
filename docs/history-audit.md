# Capture file reference audit

Run `screenshots history-audit DATABASE [--cursor ID]` to inspect one page of
references in an existing capture history database. The command reads at most
20 capture/import references for filesystem inspection, newest ID first. The
database query reads and validates a 21st metadata row only as pagination
lookahead; that row's file is not inspected. It never starts the GUI, creates
or migrates a database, opens an image, or changes history entries, referenced files, settings,
clipboard contents or uploads.

The single JSON result contains `entries`, `has_more`, `next_cursor` and
`path_encoding: "utf8-lossy"`. Each entry contains its history `id`, a display
`path` and a `status`:

| Status | Meaning at the time of that entry's metadata check |
| --- | --- |
| `present` | The path resolves to a regular file. Its contents are not checked. |
| `missing` | The operating system reports that the path or its target is missing. |
| `not_file` | The path resolves to something other than a regular file, such as a directory. |
| `unreadable` | Metadata lookup failed for another reason, such as access denial or a symlink loop. |

File symlinks are followed: a link to a regular file is `present`, a broken link
is `missing`, and a link to a directory is `not_file`. A `present` result does
not establish that the file can be opened, decoded, or still exists after the
check; a damaged or non-PNG regular file remains `present`. This is a reference
audit, not an image integrity check. Error details are not included in entries.
The explicit result includes saved local paths; it is not a sanitized export.
Lossy display paths must not be used as exact filesystem identifiers.

To continue, pass `next_cursor` as `--cursor ID` in another invocation. The cursor
is an exclusive, positive decimal history ID. Omit it for the first page. When
`has_more` is false, `next_cursor` is null. Each invocation reads its own database
snapshot; concurrent imports or removals can change subsequent pages. Unknown,
duplicate, incomplete or extra options and invalid cursors fail before the
database is opened. A cursor beyond existing IDs is valid; it may return an
empty page.

The command reuses the read-only capture query, including its 250 ms SQLite
deadline, 100 ms lock wait, 1 GiB database limit and bounded metadata validation.
It performs sequential filesystem metadata checks for the returned page only.
Filesystem calls, including database path resolution, can block on the operating
system and are not interruptible by the SQLite deadline; there is no hard
end-to-end time limit. No background scan, image buffers, worker pool, cache,
polling timer or automatic repair is added. Memory is bounded to one metadata
page and its JSON result. Existing performance targets remain unchanged.
SQLite may create or update its normal WAL shared-memory sidecar while reading
a live database; read-only refers to application rows/schema, not a promise of
zero filesystem metadata changes by SQLite or the operating system.

An unavailable, symlinked, unsupported or malformed database fails with a
nonzero exit status and no JSON result. A missing database and its parent are
never created. Entry statuses are successful audit observations and do not
change the exit status. Nothing is automatically relinked or deleted.
