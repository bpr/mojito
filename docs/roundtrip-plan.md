# Lossless Textual MIR Round Trips Plan

## Objective

Close roadmap §3's "Lossless round trips": complete `mir::text::parse_artifact`
from the seed instruction subset to the full normative grammar of
[`mir-text-format.md`](mir-text-format.md), and require MIR → text → MIR
equivalence across the full executable test corpus.

This milestone owns full-schema decoding (instructions, terminators, the
complete type/origin/compile-time-value grammar, and the `structs:`/`decls:`
declaration-metadata sections), the exact `FloatLiteral` inverse, and the
corpus-wide round-trip enforcement group. It does **not** own CLI artifact
execution or compiler dump flags; those remain the later roadmap §3 tasks.

## Deliverables

- `FloatLiteral::parse_exact` (`src/literal.rs`): the declared inverse of the
  literal's `Display` spellings — `-0.0`, `{n}.0`, and the reduced
  `{numer}/{denom}` rational — accepting a non-reduced rational and reprinting
  it reduced.
- `src/mir/text/parse.rs` extended to the complete schema, mirroring
  `src/mir/text/write.rs` printer-for-printer: leaf enum inverses, constants
  (`float_literal` included), `CheckedConst`/`CtValue`/`CtExpr` trees, the
  full `Ty` grammar (callable types with transfer effects, parameter
  declarations, constraints), origins/signature origins/capture sets, all
  instruction payload records (places, loans, call contracts, subscript and
  iterator calls), both remaining terminators, nested `try` regions, and the
  `struct`/`decl` metadata records.
- The `roundtrip::*` group of `tests/corpus_test.rs`: per executable fixture
  (`ok`, `origin_ok`, `ownership_ok`), disassemble the drop-elaborated MIR,
  parse the text, re-disassemble, and require byte equality, with a
  first-divergence diff hint and a corpus-size guard.
- The `tests/snapshots/mir/metadata.mir` snapshot pinning a
  declaration-metadata artifact through disassembly, parse-and-reprint, and
  verified loading.

## Decisions

- **Equivalence is canonical-text byte equality** (print → parse → print), not
  structural Rust equality. The writer canonicalizes unordered tables (sorted
  structs/decls, sorted set/map views), so `parse(print(M))` legitimately
  differs from `M` in container order while meaning the same program;
  `MirProgram` deliberately has no `PartialEq`. Deterministic printing makes
  text equality a sound equivalence proxy.
- **Ordering rules bind the emitter, not the parser.** The parser enforces
  only the identity constraints (dense `fileN`/`bbN`, `start <= end`,
  duplicate keys/fields) and accepts record fields and unordered tables in
  any order; reprinting restores canonical order. Origin unions and concrete
  capture sets decode through their canonicalizing constructors.
- **`FloatLiteral`'s canonical spelling is the exact rational family, not
  exact decimal.** Compile-time folding produces exact non-decimal rationals
  (`1/3`), so the format doc's original "exact decimal" wording was
  unimplementable; the doc now blesses the implemented `-0.0` / `{n}.0` /
  `{numer}/{denom}` spellings. The never-round-through-`f64` guarantee is
  unchanged.
- **Nested `try` regions stay out of the artifact source map.** Each region
  decodes its own dense `bb0..bbN` namespace, but region-local paths would
  collide with the function-level namespace and the canonical verifier only
  resolves `function/<name>/bb<n>` paths; the enclosing
  `.../instruction/<k>` mark already brackets the region text.
- **The round-trip trial's second disassembly is the artifact-side semantic
  gate.** `disassemble` re-runs invariant checks, `mir::verify`, and schema
  findings on the parsed program, so the trial does not call
  `verify_artifact` separately; span-mapped verification stays covered by
  `tests/mir_assembler_test.rs`.
- **Missing required fields are diagnostics, never silent defaults.** The
  seed parser silently dropped an instruction (and its block) when a field
  was absent; the full decoder reports `missing required field` and fails the
  parse, and out-of-vocabulary instruction/terminator tags are now fatal
  (`unknown instruction`), matching the major-version compatibility rule.
- **`# requires: discovery` fixtures round-trip through the `Compiler`.** The
  raw phase seam is non-authoritative for the whole-program
  discovery/specialization handoff, so those trials re-lower from
  `CompiledProgram::checked()` before drop elaboration. Caching the verified
  `MirProgram` in `CompiledProgram` stays deferred to the VM-artifact-execution
  task, where the `Backend` contract is already open.

## Known corpus finding

`assets/ok/nested_comprehensions.mojo` fails nondeterministically across the
pre-existing `assets_ok`/`vm_ok`/`verify` groups and therefore also the new
`roundtrip` group, with `mir::verify` findings of the
`interior loan origin roots at slot N, but its executable place roots at slot
M` family. Bisected to commit `fbf78da` ("Close the backend-ready MIR
checkpoint"), which added the interior-loan root/place consistency rule; the
failure predates and is independent of the round-trip work. Fixing the
comprehension lowering (or the rule) is follow-up work outside this
milestone.
