# Pliron Stage 1: Scalar MIR-to-Native Vertical Slice

Outcome record for the roadmap section 4 "Pliron Stage 1" task (completed
2026-08-18). The backend lives in `src/backend/pliron/` behind the
`backend-pliron` feature; its gate is `scripts/check-pliron` (which chains the
Stage 0 spike gate). Pins are unchanged from Stage 0
(`docs/notes/pliron-stage0.md`): `pliron`/`pliron-llvm` `=0.17.0`, llvm-sys
221, LLVM 22, `LLVM_SYS_221_PREFIX=/usr/lib/llvm-22`, clang-22 for
object/executable emission.

All roadmap acceptance criteria pass:

- Straight-line, diamond, loop, multi-function, and recursive examples match
  the VM (`tests/pliron_backend_test.rs::differential_fixtures_match_vm` over
  the seven `assets/ok/pliron_*.mojo` fixtures, which also run in the normal
  VM corpus).
- Canonical Pliron text round-trips byte-stably
  (`canonical_text_round_trips`), repeated builds produce byte-identical
  canonical text and LLVM IR (`compilation_is_deterministic`).
- Invalid IR fails verification as a `PlironErrorKind::Verify` diagnostic,
  never a panic (`backend::pliron::tests::pliron_verify_rejects_invalid_ir`).
- The backend consumes only `MirProgram` facts (`crate::mir`, `crate::types`,
  `crate::ast` operator enums, `crate::token::SourceSpan`) — no AST/HIR/
  checker imports.
- `mojito compile [FILE] --backend pliron --emit plir|ll|bc|obj|exe [-o PATH]`
  is feature-gated; text kinds print to stdout unless `-o` is given, binary
  kinds require `-o`, and a feature-less build explains how to rebuild.

## Architecture

- **Entry**: `backend::pliron::compile(&MirProgram, &CompileOptions) ->
  NativeModule`. `CompileOptions.entries` names the roots; the backend
  compiles their transitive `Call` closure (program order preserved for
  determinism) and applies the supported-subset contract to that set only.
  The CLI passes `main` plus `__toplevel__` when present; the differential
  harness passes the pure scalar entry under test. Edges to names without MIR
  functions (builtins like `print`, unknowns) are rejected at the call site
  with a contextual diagnostic.
- **No `Backend` enum variant**: Stage 1 is compile-only.
  `BackendKind::instantiate` still refuses `pliron` for `run`/`exec`; the
  roadmap defers `run --backend pliron` to a later stage's conformance gate.
- **Lowering shape** (`lower.rs`): registers are block-local SSA values;
  cross-block dataflow arrives through variable slots, lowered as entry-block
  allocas with load/store at `UseVar`/`DefVar`; MIR blocks map 1:1 to pliron
  blocks (the entry block stays separate so MIR block 0 may have
  predecessors). pliron's `mem2reg` + `dce` then rebuild SSA; the module is
  verified before and after the pipeline.
- **Literals**: `Const IntLiteral` regs stay pending `BigInt`s; `UnOp Neg`
  folds into the pending value; `MaterializeLiteral` (and any direct consumer
  — shift amounts arrive unmaterialized) materializes one i64 constant with a
  range check (`LiteralOutOfRange` diagnostic otherwise). A literal reg may
  feed several consumers; reads are non-destructive.
- **Erased registers**: `InvalidateInteriors`/`EstablishLoans` markers and
  void-call dests emit nothing; a later read of one is an internal-invariant
  diagnostic, never a silent miscompile. `DropVar` on scalar slots and
  `KeepAlive` are no-ops. A value-less `Return` in a value-returning function
  is checker-guaranteed-unreachable fall-off scaffolding and lowers to
  `llvm.unreachable`.
- **Exhaustiveness**: the `MirInstr`/`MirTerm` matches name every variant (no
  wildcard), so new instruction forms force a lowering decision. The `Ty`
  match keeps a reject-by-default wildcard: unknown types safely fail as
  unsupported.
- **Locations**: `SpanTable` byte offsets convert to pliron
  `Location::SrcPos` (file/line/column) via per-source line tables; locations
  survive into the canonical text as outlined `!N` attributes and render in
  `PlironError` diagnostics.

## Symbol mangling (`mangle.rs`)

MIR names are already unique and deterministic (`src/symbol.rs` owns
identity); the backend applies a purely mechanical, injective, C-safe escape:
prefix `mj_`; `[A-Za-z0-9]` pass through; `_` becomes `_u`; any other byte
becomes `_hh` (two lowercase hex digits). Injective because every `_` in the
output starts an escape and `u` is not a hex digit. The synthesized
executable wrapper is plain `main`, which no mangled name can collide with.

## Recorded VM/native divergence policies

- **Int overflow** (add/sub/mul): the VM uses plain Rust arithmetic — a
  debug-build panic, with no defined language semantics. Native lowers to
  wrapping LLVM `add`/`sub`/`mul`. Differential fixtures avoid overflow;
  revisit when Stage 2 defines trap categories.
- **Division by zero** (`//`, `%`): the VM raises a `RuntimeError`; native
  `sdiv`/`srem` by zero is UB (in practice a trap). No guard is emitted in
  Stage 1; fixtures avoid zero divisors.
- **FloorDiv/Mod**: exact match. Branch-free select expansions reproduce
  `runtime.rs::floor_div`/`floor_mod` (quotient floors toward negative
  infinity; remainder takes the divisor's sign), pinned by the
  `pliron_floor_signs` sign-matrix fixture.
- **Shifts**: exact match. The VM's `wrapping_shl`/`wrapping_shr` mask the
  amount mod 64; the lowering emits `and amt, 63` before `shl`/`ashr`
  (`Shr` on `Int` is arithmetic), pinned by `pliron_straightline`'s
  `<< 65` / `>> 64` cases.
- **`Pow` and true division `/`** are rejected in Stage 1 (`/` produces
  `Float64`, outside the subset; `Pow`'s VM contract involves `i64::pow`
  panics and a guarded exponent — deferred to Stage 2).

## Differential harness design

Fixtures are valid printing programs: a pure scalar `def compute() -> Int`
plus `def main(): print(compute())`. The VM runs the whole program and its
single printed line parses as the expected i64; the backend compiles only the
closure reachable from `compute` and executes it through ORC LLJIT
(`NativeModule::jit_i64`). The fixtures double as ordinary `assets/ok` corpus
cases, and the harness guards against corpus shrink (>= 7 fixtures). The
executable path is checked separately: a print-free `main` fixture must build
via bitcode + clang, exit 0, and print nothing.

## Findings

- MIR `BinOp` operands can be **unmaterialized `IntLiteral` registers**
  (shift amounts): `MaterializeLiteral` is not guaranteed to precede every
  literal consumer, and one literal reg can feed several consumers.
- MIR emits a **value-less `Return` fall-off block even in Int-returning
  functions** (unreachable by checking); pliron-llvm's `ReturnOp` verifier
  rejects it, hence the `llvm.unreachable` lowering.
- clang infers input kinds from extensions: intermediate bitcode files must
  end in `.bc` or clang silently produces nothing (exit 0).
- The `Cargo.lock` now contains `pliron`/`pliron-llvm`/`llvm-sys` entries as
  optional dependencies; `tests/backend_isolation_test.rs` therefore guards
  the resolved default graph via `cargo tree --no-default-features` instead
  of scanning the lockfile.
- Reachability through collections drags self-hosted stdlib helpers into the
  compiled set, so rejection diagnostics may name the deepest unsupported
  stdlib callee (e.g. `Pointer` types under `List`) rather than the user
  function — accurate, though Stage 2 may want a reachability chain in the
  message.

## Stage 2 pointers

- The `compile`-vs-`run` split: promoting `run --backend pliron` requires the
  Stage 2 conformance-eligibility gate (every eligible scalar `run` case, at
  `O0` and an optimized level, plus a capability manifest).
- The pass pipeline is deliberately minimal (`mem2reg` + `dce` per function);
  `simplify_cfg` and constant folding are Stage 2 optimization-pipeline
  decisions. Dead `llvm.constant`s survive DCE upstream (Stage 0 finding);
  `to_llvm_ir` folds constant chains at emission, so emitted LLVM IR is
  already clean.
- `UInt`, `Float64`, `Pow`, true division, and raising calls are the nearest
  subset boundaries.
