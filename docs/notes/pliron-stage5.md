# Pliron Stage 5: Supported-Language Native Parity

Stage 5 grows the feature-gated Pliron backend one vertical slice at a time
until the native path has **zero exclusions** across Mojito's advertised
runnable subset (`assets/ok` + `assets/ownership_ok` fixtures with a `main`
entry), with the VM remaining the semantic oracle and canonical `.mir`
artifacts behaving identically through both paths. This note records the
design decisions and divergences per slice; the normative ABI contract stays
[`docs/native-abi.md`](../native-abi.md).

## Harness generalization and the capability matrix

- The Stage 4 exe manifest is renamed to the stage-neutral
  `conformance/pliron-parity.tsv` (`parity_exe_manifest_and_differential`);
  its schema and oracles are unchanged. Alongside the existing
  eligible-coverage floors, the guard set now ratchets the `excluded` count
  downward: each landed slice tightens the ceiling toward zero, so a
  regression cannot hide behind unrelated progress.
- `conformance/pliron-capability.tsv` is the roadmap-mandated generated
  capability matrix (`backend::pliron::capability::matrix`), with three
  sections: one row per textual-MIR instruction mnemonic, one per
  checked-type constructor spelling, and one per exported runtime symbol
  (`since` ABI versions from the `rt_abi` contract table). The instruction
  and type tables are pinned against the canonical schema vocabulary —
  `mir::text::INSTRUCTION_MNEMONICS` and the new `mir::text::TYPE_SPELLINGS`
  inventory — so adding a MIR instruction or `Ty` constructor fails the pin
  until it receives an explicit `supported`/`partial`/`unsupported` decision.
  `partial` rows state the lowered condition; `supported` rows may still
  reject malformed artifacts (untyped registers, missing blocks), which are
  verifier-level anomalies rather than capability gaps.

## Narrow scalars and literal value types (S5.1)

Width-1 `Ty::Simd` scalar aliases (`Int8`…`Int64`, `UInt8`…`UInt64`,
`Float32`) and typed `IntLiteral`/`FloatLiteral`/`StringLiteral` storage now
lower. Design decisions and recorded divergences:

- **Representation.** Sized integers are native `iN` SSA values and `iN`
  storage (`lane_layout` in `src/native/layout.rs`); the checker admits only
  `+`/`-`/`*`/comparisons on them, and native `iN` add/sub/mul wraps exactly
  like the VM's `wrap`-after-i128 rule. Comparisons pick signed/unsigned
  predicates from the shared `runtime::integer_dtype_bits` table, so the
  width/signedness vocabulary cannot drift from the oracle.
- **Float32 is computed and printed through its f64 view.** The VM stores
  f32 lanes as f64 views and rounds each operation result to single
  precision (`round_f32`), so the lowering widens (`fpext`), operates at
  f64, and truncates (`fptrunc`) — never direct f32 arithmetic, whose single
  rounding differs from the VM's double rounding in edge cases. Printing
  widens to f64 and reuses `mjrt_fmt_f64`: the VM formats the f64 view, so
  the planned `mjrt_fmt_f32` (and an ABI bump) proved unnecessary — the
  runtime ABI stays at v3.
- **Cast saturation happens at i128.** The VM's float→int cast is Rust's
  `as i128` (saturating, NaN → 0) followed by a wrap to the lane width;
  the lowering uses `llvm.fptosi.sat.i128.f64` then truncates. Saturating
  at i64 or the target width instead wraps large magnitudes differently
  (e.g. `Float32(1e30).cast[DType.uint8]()` is 0, not 255).
- **Literal storage is exact; materialization wraps.** `MaterializeLiteral`
  into concrete scalar and width-1 SIMD targets wraps VM-exactly
  (`wrapping_signed/unsigned`, exact `to_f32` for Float32 — correctly
  rounded from the exact rational, never through an f64 intermediate). But
  an `IntLiteral`-typed register/var/field is i64 *storage*, and the VM
  keeps arbitrary precision there — a constant exceeding i64 rejects with
  `LiteralOutOfRange` (reject-never-wrap) instead of silently diverging.
  Recorded acceptance: runtime arithmetic on stored literal-typed values
  lowers at i64/f64, so an intermediate that overflows i64 after passing
  the storage checks would diverge from the VM's BigInt arithmetic; the
  CTFE fixtures that exercise literal vars fold their arithmetic at
  compile time, so no corpus case observes this.
- **Runtime FloatLiteral display rejects.** The VM prints a
  `Value::FloatLiteral` as its exact rational (`1/10` for `0.1`), which f64
  storage cannot reproduce; printing or `String(...)`-converting a runtime
  (non-constant) FloatLiteral value rejects contextually. Pending-literal
  prints keep the established default-materialization behavior.
- **StringLiteral storage is the borrowed `MjStrDesc` descriptor** (16
  bytes, already in the layout engine). A compile-time literal consumed as
  storage materializes on first use as a descriptor over its interned
  constant bytes (`reg_ptr`); descriptor copies are plain byte copies with
  no ownership, and `String(x)` over a runtime descriptor copies the bytes
  out. The owned-temp release rule is untouched (a descriptor register
  without an owned runtime string entry releases nothing).

## Backend-side monomorphization (S5.2)

The feature-independent `native::mono` pass now sits between the cached
post-drop MIR handoff and Pliron reachability. It borrows the canonical
`MirProgram`, builds an owned entry-rooted graph, and leaves VM execution and
serialized `.mir` bytes unchanged.

- Instance keys retain the template plus declaration-ordered concrete type and
  frozen value arguments; origins erase from native identity. `symbol` owns the
  deterministic instance spelling, while `native::mangle` remains the sole
  LLVM-name transform.
- One structural unifier combines concrete receiver, call-slot-bound runtime
  arguments, and result types. Duplicate solutions must agree; incomplete,
  non-constant, dependent-index, and missing associated-type facts reject as
  contextual unsupported monomorphization gaps.
- Substitution covers function/declaration types, register and variable tables,
  places, call metadata, instruction-owned types, and nested `try` regions.
  Concrete struct declarations and lifecycle edges are discovered transitively.
- Direct and method targets are rewritten through a recursion-safe instance
  worklist with a bounded polymorphic-recursion guard. VM and native dispatch
  now share `symbol::resolve_callable_symbol` and
  `symbol::resolve_method_symbol`, including abstract receiver retargeting and
  borrowed-iterator alternate probing.
