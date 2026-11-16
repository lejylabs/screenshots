# Observable OpenType preprocessing exhaustion

Based on the published `harfrust 0.15.0` crate (MIT), upstream repository
<https://github.com/harfbuzz/harfrust>, revision
`71d965130ea93905f81b8f03a03d7ecc794fbc47`, path `harfrust`.
The original crate manifest, source, README and MIT license are retained. The
Cargo registry checksum remains recorded in this file for provenance:
`865dcc4105d51c323d8fedbb2d75b5e9c8ecddf5613f74c82f851d0afbc97114`.

The upstream `Budget` skips and memoizes lookups when work/allocation
allowances are insufficient. The editor must explicitly refuse such a result.
This patch adds a sticky atomic exhaustion flag (including checked allocation
multiplication overflow), retains that same budget independently of lookup
construction success in `OtCache`, and exposes the observational
`ShaperFont::preprocessing_budget_exhausted()` query. Limits, lazy lookup
construction, shaping output and default behavior for other callers are
unchanged. The editor checks the flag immediately after shaper construction and
after every shape, before accepting any glyphs.

Resource cost is one atomic boolean per existing per-font preprocessing budget
and one optional shared-budget reference per existing OpenType cache. No new
runtime, thread, font scan, retained glyph bitmap or idle work is introduced.

`src/budget.rs` contains independent work/allocation underflow, multiplication
overflow, exact-boundary, sticky-state and concurrent-budget tests. It can be
run without the upstream crate's separately distributed font test corpus:

```
rustc --edition=2021 --test --cfg 'feature="std"' vendor/harfrust/src/budget.rs -o target/verification/harfrust-budget-tests
target/verification/harfrust-budget-tests
```

The editor's real shaping regression constructs a structurally valid GPOS table
with repeated aliased PairPos1 pair-set offsets and a default kern feature,
inspired by upstream `src/ot/lookup/gpos.rs::tests::repeated_pair_sets_charge_every_record_visit`.
It overlays that table on the existing SIL-OFL Fira Mono fixture in memory and
checks the public status and editor refusal after normal shaping. No altered
font is distributed. The executable E2E creates the same bounded synthetic
font only in its temporary fixture directory, checks refusal/no published
output, and preserves the original font fixture and its existing license.

The original crate excludes the upstream font corpus from its published
archive. Its entire unrelated unit-test suite is therefore not claimed run;
we run the standalone budget tests and the application's real font, shaping
and rendering regressions against this exact patched dependency.
