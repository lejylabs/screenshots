# Read-only history queries

Status: implementation contract; native UI/library workflows are unchanged.

## Scope and behavior

`query_capture_history` and `query_upload_history` read an existing application
database without creation, migrations, image decoding, file enumeration or network
requests. The caller runs them on a worker. They return at most 20 metadata rows,
`has_more`, and `next_cursor`; SQLite reads one additional row as lookahead.

`HistoryQuery` accepts a positive exclusive ID cursor, `Newest`/`Oldest` order,
capture source filter, text, and inclusive `from`/`through` UTC dates in strict
`YYYY-MM-DD` form. Newest means descending insertion ID, oldest means ascending
ID, even when timestamps disagree. A cursor is meaningful only with the same
kind, sort and filters. Pages are independently consistent SQLite snapshots;
concurrent inserts/deletes can change later pages. There is no offset pagination.
Upload queries reject a capture-only source filter.

The headless CLI returns one JSON object with these page fields:

```sh
screenshots history-query captures /path/history.sqlite3 --order oldest --source imports --from 2024-02-29 --through 2024-03-01 --search invoice
screenshots history-query uploads /path/history.sqlite3 --order newest --cursor 120 --search example.invalid
```

Supported options are `--order newest|oldest`, `--cursor ID`, `--from DATE`,
`--through DATE`, `--search TEXT` and `--source all|captures|imports`. Each option
may occur once; `--source captures|imports` applies only to capture queries.
The existing `history` command and UI queries retain their prior behavior.

Capture text retains the existing literal word-prefix FTS semantics: at most
512 UTF-8 bytes and 16 alphanumeric words joined with AND. Punctuation has no
query-language meaning. Blank text means no text filter; nonblank text without
words matches nothing. Upload text is a literal case-insensitive ASCII substring
across filename, destination origin and returned URL (512 UTF-8 bytes maximum).
SQLite's built-in ASCII case folding does not provide Unicode case equivalence.
URLs are returned as local metadata, never opened, resolved or fetched.

Dates compare against the stored UTC calendar-date prefix. The application
writes RFC3339 UTC timestamps; legacy date-only values are also accepted.
Returned timestamps must be valid UTC RFC3339 or strict dates. Arbitrary non-UTC
timestamp formats are unsupported. Inclusive calendar-prefix comparison includes
the complete final day without requiring a next-day value beyond year 9999.
IDs, dimensions, paths, filenames, origins, URLs and timestamps are validated
before return. Oversized selected
fields fail the query; they are not truncated or silently skipped. SQL bounds
text bytes before Rust allocates a field. Paths need not exist and are never
dereferenced. Explicit queries can reveal locally retained signed sharing URLs.

## Resource contract and limitations

Each request owns one read-only SQLite connection, a 512 KiB page-cache target,
one prepared query and at most 21 metadata rows. A capture path is at most 4,096
UTF-8 bytes, upload filenames 1,024, upload origin/URL 2,048 each, and timestamps
64, giving at most 108 KiB of retained text plus object/allocator overhead for a
21-row upload page. The database main file and declared database pages have the
shared 1 GiB maintenance admission cap. This is not a hard cap on all SQLite/transient/OS
allocations. No thread, runtime, timer, cache, index or schema migration is added.
The caller provides an `Arc<AtomicBool>` cancellation flag; SQLite checks it and
the 250 ms request deadline every 1,000 virtual-machine instructions and Rust
checks between rows. Failure discards the partial page. Blocking filesystem/OS
calls and SQLite's existing 100 ms busy wait cannot be interrupted mid-call, so
the deadline is cooperative, not a hard wall-clock guarantee. SQLite read-only
WAL access can create/update its shared-memory coordination sidecar; query work
does not modify main database content or schema.

Unfiltered/source-filtered capture and upload pagination use existing primary
key/source indexes (legacy version-1 source filtering may scan because that
index does not exist yet). Capture text requires schema version 3 or later and
uses the existing FTS index; upload queries require version 4. Date filters and
upload substring filters have no matching index: they can scan many metadata
rows before finding 21 matches and may hit the deadline. No claim of constant
time arbitrary search/sort is made. Actual timestamp sorting and new indexes
require a separate migration and resource design. No full integrity scan occurs
on each page; schema/required-column checks are bounded, and corrupt storage can
still fail during the query.

## Verification

Isolated fixtures cover 10,000 metadata entries without image files, stable
forward/reverse cursors, combined source/date/text filters, leap days, literal
substring punctuation, malformed inputs/fields, cancellation, SQLite progress
interruption, read-only legacy schemas, absent databases and indexed query plans.
Optimized test commands and observations are recorded after execution; targets
above are not measured acceptance results. Full UI, real-disk latency, memory,
cross-platform and reference-hardware acceptance remain open.

## Optimized development observation (2026-11-15)

The explicit `history_query_10k_cost` fixture used 10,000 metadata rows and no
image files. New capture-ID paging observed p95 0.111 ms versus 0.080 ms for the
existing query; the new connection performs read-only validation rather than
migrations. Capture FTS p95 was 0.215 ms; upload substring p95 was 2.283 ms and a
full no-match upload scan p95 was 2.708 ms. These are warm local SQLite observations,
not UI latency, reference-hardware acceptance or guarantees for arbitrary data.
The existing UI queries remain unchanged; no new index/cache/idle task was added.
