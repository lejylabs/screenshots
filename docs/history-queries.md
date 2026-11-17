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

Supported options are `--order newest|oldest|date-newest|date-oldest`, `--cursor ID`, `--from DATE`,
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
time arbitrary search/sort is made. Explicit chronological sorting is described
below; it adds no persistent index or migration. No full integrity scan occurs
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

## Explicit chronological sorting

`query_capture_history_by_date` and `query_upload_history_by_date` accept the
existing `HistoryQuery` for direction/filters plus an optional `HistoryDateCursor`.
`Newest`/`Oldest` select descending/ascending timestamp order in these two new
entry points; the existing functions, types and insertion-ID orders are unchanged.
`HistoryQuery.cursor` must be absent for the date functions. Results use the
separate `HistoryDatePage` with at most 20 entries, `has_more` and a composite
`next_cursor`. UTC dates mean midnight. Valid RFC3339 UTC representations such as
`Z`, `z`, `+00:00` and `-00:00` compare by their instant. All stored fractional
second digits up to the existing 64-byte timestamp limit participate in ordering;
there is no floating-point, millisecond or nanosecond rounding. Equivalent
representations (including trailing fractional zeros and date-only midnight) tie
by ID in the selected direction. Accepted leap seconds sort after second 59 and
before the following minute.

```sh
screenshots history-query captures /path/history.sqlite3 --order date-oldest
screenshots history-query uploads /path/history.sqlite3 --order date-newest --cursor '2024-03-01T12:34:56.1234567891Z/120'
```

For the new CLI orders `next_cursor` is a string `TIMESTAMP/ID` (or null at the
end), passed back unchanged through `--cursor`. The existing orders still return
numeric ID cursors with the existing JSON shape. Tokens are bounded before
parsing, validate a positive ID and valid UTC timestamp, and carry the original
anchor timestamp. They never look up the anchor ID: deleting or retimestamping
that row cannot silently change the page boundary. Keep kind, direction and
filters unchanged between pages. Pages remain independent SQLite snapshots;
concurrent inserts, deletes or timestamp changes to other rows can make those
rows enter or leave later pages. A moved row may appear twice or be skipped across
snapshots; this is not a pinned multi-page transaction.

Date admission verifies that the selected table has one declared INTEGER primary
key which aliases SQLite's rowid (including rejection of affinity-only `PRIMARY
KEY DESC` and `WITHOUT ROWID` variants). This proves uniqueness and fixed integer
storage for the raw ID used by the FTS join; malformed/nonunique foreign schemas
are rejected. Genuine released schema versions retain their existing support.
The chronological projection, secondary ordering and cursor comparisons also use
type/range-guarded IDs, and the timestamp scalar validates the bounded positive ID
before keyset comparison can exclude an invalid tie.

Each explicit date request registers one connection-local deterministic,
DIRECTONLY scalar function using the already-enabled rusqlite functions feature.
It borrows SQLite timestamp bytes, rejects non-text/over-64-byte timestamps
before Rust allocation and checks cancellation/deadline on every invocation.
SQL also bounds its timestamp input before invocation. It returns a canonical
text key preserving every fractional digit. The query orders that computed key and ID with `LIMIT 21`; SQLite maintains a top-21 ephemeral tree
rather than materializing every matching row. Selected text projections retain
the existing field limits before becoming sort payload. Capture width/height
projections require SQLite INTEGER storage in `1..=u32::MAX`; otherwise they yield
NULL before entering the candidate tree. This is necessary because released
non-STRICT INTEGER/NOT NULL columns can store large BLOBs, text and reals. The
query does not filter invalid dimensions away: selected NULLs fail row decoding,
while range-valid dimensions still receive the existing decoded-pixel budget
check. All numeric sort payloads are fixed-width integers or NULL. The ephemeral
tree can retain 21 bounded metadata payloads (at most the existing 108 KiB upload-page text
bound) and 21 timestamp keys (64 bytes each, about 1.4 KiB), while Rust collects
another page. Allow about 218 KiB of combined retained text/key bytes, plus
SQLite/object/allocator overhead and one transient key. This is added active
sorting work rather than a persistent cache. The 512 KiB cache target is
unchanged and is not a cap on total SQLite/process/OS memory. Schema admission, the 1 GiB main/declared-page limit, 100 ms busy timeout,
250 ms cooperative deadline and failure-without-partial-page behavior are reused.
A malformed timestamp evaluated while ranking fails the whole query, even if
that record would fall outside the visible page. Other metadata is validated
when selected, as in the existing query.

Date ordering has no persistent matching index. A page scans matching metadata,
computes bounded keys and performs O(matches × log(21)) top-N work; it may hit the
deadline on large histories, including a page after a cursor. It opens no image
files, enumerates no directories and fetches no URLs. No new dependency, schema,
migration, backup requirement, persistent index, worker, runtime, cache, thread,
UI control, polling, startup work or idle activity is introduced. Existing
insertion-ID pages retain their indexed path and scalar-free SQL.

Executed optimized verification fixtures cover 10,000 metadata-only capture/upload entries
whose dates disagree with ID order; independent expected ranks, exact fractions
past nanoseconds, equivalent UTC representations, midnight/date-only ties,
source/FTS/date/upload-substring filters, both directions and all static pages,
self-contained cursors after anchor changes/deletion, inserts on the visited
side, bounded invalid tokens, foreign field limits, cancellation/deadline inside
the scalar, and read-only compatible reduced schema-1 capture/schema-4 upload
databases. Additional real-migration and reduced legacy fixtures cover 8 MiB BLOBs, text, reals, negatives, zero and
integer overflow in numeric fields, invalid IDs and primary-key shapes. A new
executable fixture uses its own temporary DB and redirected user-data paths,
checks both new sorts and old numeric cursor output, and never opens a native
display or image. Bundled-SQLite EXPLAIN assertions require top-N eviction and
reject `SorterOpen`. The ignored optimized fixture compares ID and chronological
queries. The targeted core suite passed 17 regular cases with three opt-ins
excluded; both actual CLI integration cases pass. The two new optimized opt-in
timing/allocator probes were also explicitly executed and pass. The CLI fixture
uses the genuine migrated schema, disables triggers only on its synthetic
metadata-seeding connections and rebuilds FTS explicitly; production query
connections retain normal behavior. Initial fixture setup failures and their
corrections are preserved separately from passing results. No production writer
constraint or query admission was relaxed.

### Numeric-payload laboratory observation (2026-11-17)

A private in-memory system SQLite 3.54.0 laboratory used 10,000 metadata rows,
with one 8 MiB BLOB stored in width and one in height. The raw dimension projection
reproduced 52,523,776 bytes of SQLite query allocator peak growth; the guarded SQL
measured 132,720 bytes. Database residency was measured before each query and
excluded from that delta. This observes SQLite allocator growth only: it excludes
Python allocations, process RSS/private memory, Rust row decoding, application
latency and platform acceptance. Fourteen adversarial dimension checks projected
NULL in both directions; six primary-key shapes checked the rowid admission
rule. Twelve exact plain/cursor/filter query forms retained one top-21 ephemeral
tree each and no `SorterOpen`; capture FTS retained its integer-rowid lookup.
The explicitly executed application-linked bundled-SQLite probe measured
52,967,296 bytes with raw dimensions and 583,312 bytes with guarded dimensions,
including SQLite/cache overhead. Its isolated serial run passed the raw-case
reproduction and below-4-MiB guarded-growth assertions. The roughly 218 KiB
retained text/key calculation remains separate from total allocator and
process-memory acceptance.


### Optimized local observations (2026-11-17)

Twenty-five warm in-process queries per case used 10,000 metadata entries:

| Query | p50 | p95 | Maximum |
|---|---:|---:|---:|
| Existing capture ID | 0.157 ms | 0.245 ms | 0.642 ms |
| Capture date | 4.690 ms | 4.754 ms | 4.757 ms |
| Capture date source/text | 3.234 ms | 3.266 ms | 3.278 ms |
| Upload date | 4.667 ms | 4.764 ms | 4.788 ms |
| Upload date substring | 3.519 ms | 3.636 ms | 3.672 ms |

A separate actual executable probe migrated the exact initial production schema
through the CLI, then seeded 10,000 capture plus 10,000 upload metadata rows with
writer triggers disabled only during private seeding and an explicit FTS rebuild.
The 6,684,672-byte database remained byte-identical after 150 timed requests and
six separate memory-sampled requests. It created no image files and redirected
user-data locations. Twenty-five launches per case, including the GNU-time
wrapper/process startup/JSON, measured date-page p95 values of 6.762 ms for
captures, 9.238 ms in reverse and 6.651 ms for uploads; existing capture-ID p95
was 3.653 ms. Maximum GNU-time RSS across all six cases was 23,028 KiB.
Separate brief smaps samples observed at most 16,460 KiB private-resident,
17,157 KiB PSS and 20,868 KiB RSS, with no observed swap. Those samples are not
guaranteed peaks or total private mapping footprints, and do not accept the
visible-shell memory budget. No GUI or native capture was started.

Executable SHA256:
`bcd3da98b69904199bb1fd5a6b88d69b4859117a2fac5676da0e9fcadcbc276b`.
Exact commands, initial failures, corrected fixture hashes and full results are
under `target/verification/history-chronological-integration/`. These are warm
local development observations, separate from native cross-platform and
reference-hardware acceptance. The feature adds no idle work.
