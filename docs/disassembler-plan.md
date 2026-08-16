# Textual MIR Disassembler Implementation Plan

## Objective

Implement deterministic `MirProgram` → Mojito MIR 1.0 text emission according
to [`mir-text-format.md`](mir-text-format.md). The disassembler must print every
field needed to reconstruct and verify the program, must never consult source
AST syntax, and must produce byte-stable output for equivalent in-memory MIR.

This task does not parse artifacts, expose a CLI command, execute text, or claim
round-trip support. Those remain separate roadmap tasks.

## Public API and ownership

1. Extend `src/mir/text.rs` as the public schema boundary.
2. Add `disassemble(program: &MirProgram) -> Result<String,
   DisassembleError>`.
3. Make `DisassembleError` cover pre-existing invariant errors, fresh
   `mir::verify` findings, duplicate identities, malformed locations/counts,
   and schema invariants not already diagnosed by verification.
4. Put private implementation under `src/mir/text/` (`write.rs`, with focused
   type/value children if needed). Keep `text.rs` public-first.
5. Do not add Serde or a generic serialization layer; emit the Mojito grammar
   directly.

## Preconditions and failure behavior

- Accept only typed, semantically verified MIR: reject nonempty
  `invariant_errors`, then run `mir::verify::verify` before writing.
- Do not rerun ownership or drop elaboration. Both verified pre-drop and
  drop-elaborated programs are printable.
- Never use Rust `Debug`, source AST/annotations, or VM metadata as a fallback.
- Fail atomically with `Err`; never return partial text.

## Canonical writer

### Writer primitive

Create a private indentation-aware writer over `String` for fixed-order
records, ordered lists, options, symbols/quoting, `%rN`/`$vN`/`bbN`/`fileN`,
booleans, and integers. Always emit LF and exactly one trailing LF. Formatting
must not depend on locale, terminal width, or environment.

### Source-file normalization

- Collect distinct `(path, module)` pairs from all `SpanTable`s.
- Sort by serialized path then module, assign dense `fileN` IDs, and reuse them.
- Print register locations by register number; missing locations are `absent`.
- Preserve byte offsets and optional origin slots; omit `SyntaxId`.
- Reject reversed spans and invalid origin slots.

### Deterministic ordering

- Sort struct and function declarations by canonical symbol bytes.
- Preserve semantically ordered fields, parameters, functions, blocks,
  instructions, and unions.
- Sort `explicit_destructors`, `var_tys`, `reg_types`, and spans by key.
- Sort `mut_self_methods` lexically.
- Never reorder the input program itself; sort only borrowed views while writing.

## Complete serialization coverage

Implement exhaustive, wildcard-free `write_*` owners for:

1. Envelope, files, declarations, structs, and function declarations.
2. Functions, blocks, nested `try` namespaces, slot/register tables, masks, and
   locations.
3. Every `MirInstr` and `MirTerm`, using the shared mnemonic vocabulary while
   emitting all operands and metadata.
4. Places, typed projections, loans, interiors, captures, and invalidations.
5. Direct/indirect/method/subscript/iterator call contracts, including places,
   conventions, effects, reference ABI, adapters, generic witnesses, and
   capture accesses.
6. `Const`, `CheckedConst`, exact literals, `CtValue`, `CtExpr`, and callable
   defaults.
7. Every `Ty`, `TyArg`, `ParamDecl`, dependent type, constraint, operand,
   DType/operator/convention, slice kind, and intrinsic enum.
8. Origins, pointer origins, mutability, reference/signature types, callable
   environments/capture sets, and transfer effects.

Adding a Rust variant must fail compilation until its schema writer is added.

## Exact values and escaping

- Emit concrete floats by exact lowercase IEEE bits where required.
- Use canonical arbitrary-precision `IntLiteral` and exact `FloatLiteral`
  representations without conversion through `i64`/`f64`; preserve negative
  zero.
- Route every string and symbol through schema quoting: paths, modules, fields,
  keywords, messages, and mangled names included.
- Test controls, Unicode, quotes, backslashes, mangled symbols, and reserved
  words.

## Snapshot strategy

Add `tests/mir_disassembler_test.rs` and snapshots under
`tests/snapshots/mir/`. Use `include_str!` comparisons rather than a dependency.
Snapshot a compact representative matrix:

1. `minimal.mir`: declarations, typed slots/registers, constants, call, return.
2. `places_and_loans.mir`: projections, `through`, loans, interiors,
   invalidation, drops.
3. `abstract_dispatch.mir`: subscript/method/iterator residue,
   `CopyIteratorReference`, `ref`-to-`read` facts.
4. `generic_callable.mir`: `GenericFunc`, defaults, indirect call,
   instantiation, variadic/keyword collectors.
5. `try_regions.mir`: structured regions, raising metadata, cleanup return and
   escape.
6. `types_and_literals.mir`: exact values, SIMD, variants, packs,
   pointer/reference origins, multiple files, Unicode/escaping.

Prefer existing small fixtures or inline sources. Hand-build MIR only for
unreachable schema forms, and require it to pass `mir::verify`.

## Focused tests

Positive coverage:

- repeat printing is byte-identical;
- reordered map/set insertion yields identical text;
- header/UTF-8/LF/trailing-LF rules hold;
- snapshots match;
- drop-elaborated MIR prints explicit cleanup;
- every instruction/terminator is exercised by a snapshot or unit-format test.

Negative coverage:

- invariant errors and verifier corruption are rejected;
- duplicate function/declaration names are rejected;
- malformed locations/counts are rejected;
- errors return no partial output.

Keep `mir_text_schema_test.rs` vocabulary guards and add tests that every
emitted tag belongs to the schema vocabulary and reserved symbols are quoted.

## Explicitly skipped unrelated tests

Validation is intentionally limited to the disassembler and direct MIR/schema
dependencies. Do **not** run the repository-wide corpus, stdlib, evaluator/VM,
checker, parser, module-linking, conformance, or unrelated compiler suites
unless a focused failure identifies one as the owner.

Run only:

```text
cargo fmt --check
cargo nextest run --test mir_text_schema_test
cargo nextest run --test mir_disassembler_test
cargo nextest run --test mir_test <relevant_existing_test_name>
cargo clippy --all-targets --all-features -- -D warnings
git diff --check
```

If `scripts/check` cannot exclude unrelated suites, do not run it for this task.
Run its non-test parity/format/Clippy components separately where available and
record the focused-test policy in the handoff. A focused failure authorizes only
the narrow owning test target needed for diagnosis, not a fallback full-suite
run.

## Documentation and lifecycle

- Update the schema only if implementation exposes ambiguity; synchronize
  vocabulary and snapshots with any clarification.
- Update architecture, VM manual, symbol map, and feature matrix for
  disassembly-only support; assembly and execution remain unsupported.
- Remove the completed **Disassembler** roadmap entry rather than checking it.
- Update `commit_msg.txt` on completion.

## Acceptance criteria

- Every verified current `MirProgram` form prints deterministically as schema
  1.0 with no AST reconstruction or `Debug` fallback.
- Every required field, including ownership/analysis-only metadata, is emitted.
- Map/set insertion order, locale, and repeated runs cannot affect output.
- Invalid MIR fails atomically with actionable errors.
- Snapshots cover declarations, types, calls, control flow, places/loans, exact
  literals, and locations.
- Focused schema/disassembler and relevant MIR tests, formatting, Clippy with
  warnings denied, and `git diff --check` pass; unrelated slow suites are
  explicitly skipped.
