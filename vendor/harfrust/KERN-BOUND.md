# Legacy kern preprocessing admission

`kern_admission.rs` is an opt-in observer added to the published HarfRust 0.15.0
source. It does not alter upstream default shaping or cache construction. The
editor calls it during font admission, before any ShaperFont or legacy cache is
created, and adds its returned peak bytes to the separately charged OpenType
allowance. An unavailable bound is an explicit admission error. Other clients
retain upstream behavior and need to opt in themselves.

The preflight uses the engine's locked read-fonts 0.46.0 `FontRef`, `Kern`,
`SubtableKind` and state-table accessors. It allocates no tables, sets, vectors,
fonts, caches, threads or native runtime. Only borrowed metadata, scalar counters
and the existing stack-owned atomic Budget are used. Two independent ceilings
are 4 MiB potential peak heap and 4 Mi record/state/range-cell visits per face.
These preserve the existing four-MiB minimum preprocessing allowance rather than
introducing an unproven input-byte multiplier. The caller checks cancellation
immediately before and after this finite preflight.

## Byte proof

The following private-layout assumptions must be re-audited when changing the
locked read-fonts version or Rust 1.100 toolchain. Supported native targets are
64-bit, but the size expressions use the actual compiled target layouts.

- `aat::KernSubtableCache` and its boxed `common::ClassCache` are charged with
  their actual `size_of` values. ClassCache currently occupies 512 bytes.
  The cache entry includes both U32Set inline headers, parsed views, descriptors
  and inline digest. The exact boxed `Yoke<KernTableCache, Arc<Tables>>` is added.
- read-fonts `collections/int_set/bitpage.rs` stores each BitPage as eight u64s
  and a u32 length: 72 bytes on native targets. Its `bitset.rs` PageInfo contains
  two u32s: 8 bytes. Both backing vectors grow by one push/insert from empty.
  Rust 1.100 `RawVec::grow_amortized` doubles from a minimum of four elements.
  Thus `capacity(n) = max(4, next_power_of_two(n))` (zero stays zero) bounds
  each vector. Their combined charge is `capacity(pages) * (72 + 8)`.
- The table's cache-entry vector also grows by one push and uses that same
  capacity bound times the actual entry size. The formulas do not count only
  live lengths or ignore spare capacity.
- Format0 glyph IDs are u16; each first/second set has at most
  `min(number_of_pairs, 128)` occupied 512-bit pages.
- Format2 inserts each first-glyph + u16-count range. Format3 inserts the
  range from zero through glyph_count minus one. Their page counts are computed
  from the exact range endpoints. The upstream saturating count-zero behavior
  inserts one glyph, which is conservatively retained here.
- Format1's class mappings have a u16 first glyph and count. The occupied-page
  bound covers their whole range plus a separate deleted-glyph sentinel page.
  Temporary class-index sets cover indices zero through state_size minus one.
  Their maximum heap is included while the target glyph sets remain live.
- Format1's SafeToBreak vectors accumulate at most
  `min(max(state_size, OUT_OF_BOUNDS + 1), 256)` u16 class entries and
  `ceil(max(min(ceil(state_array_bytes/state_size), 65536) - 64, 0)/64)` u64 tail
  words per subtable, exactly as the upstream constructor. Chunk reserve/resize
  can leave capacity at most twice final length (or the initial four slots).
  The bound charges that spare capacity for the aggregate vectors.
- Peak is `2 * (all_retained_heap_bounds + largest_temporary_class_set_heap)`
  plus the exact boxed yoke. The extra retained copy covers any old allocation
  coexisting with its replacement during vector growth. It overcounts multiple
  simultaneous reallocations, since construction is sequential. This factor is
  an explicit peak-overlap proof, not an empirical amplification estimate.

## Work and malformed input

A stack Budget charges one visit per physical subtable header, each format0 pair
into both sets, the format1 fixed 64 states, class entries, tail states and class
mapping cells, and range-page visits in formats2/3. The first set's range digest
is charged every bit of every occupied page, covering alternating singleton
ranges too. Sparse page storage domains and their search/movement costs are
bounded by the proved page counts; these work units, like upstream's OT budget,
are semantic record/state visits rather than a CPU-time guarantee.

All arithmetic is checked before use. Each iteration rechecks the prospective
peak and charges only its nonnegative delta. The engine's physical iterator
stops on zero length or missing data; the declared count must also equal the
number of consumed valid headers. Unknown/unreadable kinds and count/progress
mismatches refuse. The existing OT-last-subtable length convention is retained
by using that exact iterator. No loop is driven solely by a declared huge
count, and no legacy cache is built while evaluating admission.

## Verification

The isolated `app-core/tests/editor_font_budget.rs` test installs a thread-scoped
counting wrapper around the system allocator. It calls the actual public
preflight on all four admitted formats, no-kern fonts, malformed data and the
80,000-subtable attack, and requires zero allocation/reallocation calls.

Core tests construct a checksum-valid font by replacing the existing OFL Fira
Mono fixture's DSIG table with `kern` and disabling GPOS in that fixture. Both
normal OpenType and Apple format0 controls change actual AV total advances by
-120 font units. Formats1/2/3 are also admitted and shaped. The actual 80,000
Apple format0 one-pair subtables occupy 1,760,008 table bytes and keep the whole
font below 2 MiB; admission refuses before any cache can allocate its 40,960,000
bytes of ClassCaches alone. A separate tiny format1 example with 65 subtables
and 65,535 declared class columns exceeds the work ceiling while remaining
small in bytes. Zero/truncated/huge declared counts and unknown kinds refuse.
The executable fixture verifies OT/Apple kerning pixel differences and refuses
the real large font without final output or `.screenshots-*.part` files.

These are transient synthetic test fonts, never installed or distributed as
modified font assets. The existing OFL source fixture/license is retained.

The audited read-fonts crate records source revision
`47a301615fd95acf0da9fb7dbcd0ea7c7df36943`, path `read-fonts`, in its published
`.cargo_vcs_info.json`. Primary definitions are
`src/collections/int_set/bitpage.rs`, `bitset.rs`, `src/tables/aat.rs` and
`src/tables/kern.rs`. The pinned compiler is Rust1.100.0 (`0ef1bc386`); its
official `library/alloc/src/raw_vec/mod.rs` at full commit
`0ef1bc386059596dada8fb65b50425aab2ea637a` was retrieved and read to verify
`min_non_zero_cap` and `grow_amortized`. The fetched file is retained only in
ignored `target/verification/rust-1.100-raw-vec.rs`. The exact external source hashes are
recorded with the repository source in the round4 verification checkpoint.
