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

Legacy `kern` is outside the upstream OT preprocessing budget. The editor's
additional opt-in, allocation-free `kern_preprocessing_size` admission observer
now bounds all legacy formats0/1/2/3 before any cache construction. It preserves
upstream behavior for other callers. Exact private-layout/capacity/work formulas,
assumptions, the independent4MiB legacy limits and real synthetic proofs are
documented in [KERN-BOUND.md](KERN-BOUND.md). Its returned peak is accounted
separately from OT and glyph/outline scratch; refusal is explicit.

The editor also opts into `Buffer::set_fail_on_runtime_budget_exhaustion(true)`.
Published legacy-kern state machines can force advancement when their operation
allowance runs out, and OpenType contextual lookup application can skip required
recursion at zero without marking the buffer unsuccessful. In strict mode these
paths instead retain an unsuccessful status before glyphs can be accepted.
Natural advancing transitions that use the last operation successfully remain
valid; a further operation, or zero-budget forced advancement, fails and the
legacy loop does not enter later subtables. The fast advancing branch is checked
too. `clear` clears the failure and retains the option; `reset` also disables it.
Default callers retain the published control flow, glyphs and status. The patch
adds one boolean to each existing Buffer and no allocations or idle work.

Actual private-engine boundary tests are in `src/runtime_budget_tests.rs`.
The application and executable fixtures build a valid format1 PUSH/DONT_ADVANCE
self-loop from the existing OFL font and require explicit refusal with no final
or staging output. Default shaping and naturally advancing controls remain
unchanged. The editor rejects `morx`, `mort` and `kerx` at admission; this opt-in
does not claim to harden those unrelated upstream state-machine paths.

The root `.gitattributes` preserves this vendored tree's published line endings,
including CRLF generated tables, and treats carriage returns as line endings for
whitespace checks. Source/provenance hashes refer to those retained bytes.
