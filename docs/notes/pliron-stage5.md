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

## Iterator protocol (S5.3)

`GetIter`/`HasNext`/`Next`/`TryNext` now lower; raising value-yielding
iterators run over the tagged-outcome ABI. Design decisions and recorded
divergences:

- **Mono owns iterator normalization.** A dedicated pre-pass folds every
  `GetIter` before call rewriting: each `prepare` step resolves through the
  shared `symbol::resolve_method_symbol` (borrowed-alternate probing
  included), rewrites to its concrete instance, and the chain's final return
  type becomes the iterator variable's type — HIR leaves the split
  `$iterobjN` slot untyped, which was the first diagnostic on 13 fixtures.
  The pre-pass exists because block order does not put a `GetIter` before
  the advances that read its slot (comprehension loops interleave).
  Dynamic `__trait_dispatch.` steps unroll statically against the concrete
  receiver under the VM's budget of 8, rejecting on non-convergence; an
  untyped source (a compiler-private pack loop) passes through for the
  backend's own boundary. `HasNext.method` and `(Try)Next.call.target`
  retarget the same way, and reachability follows all iterator symbols.
  Reachability also exposed direct `List.__init__`-style constructor calls:
  their destination is the `out self`, not the declared `None` return —
  `infer_call` now binds it as the receiver.
- **Receiver conventions mirror the VM.** A borrowed (`ref`/`mut`) `__iter__`
  step aliases the current storage (step 0 aliases the source slot — the
  VM's reference-handle seam, so borrowing iterators root at the loop
  frame); a `read` step passes a plain byte copy (the VM's `current.clone()`
  runs no lifecycle copy); an owned (`var`) step consumes the storage in
  place, which lands the same user-visible destructor sequence as the VM's
  clone-then-consume. `__next__` passes the iterator variable's own storage
  as its `mut self`, so in-place advance is the write-back — and the VM's
  `rebase_iterator_result` needs no native counterpart: a yielded reference
  into `self` already points at caller-owned iterator storage. Superseded
  `read`-step intermediates release invisibly (or reject when they carry
  destructor work); raising `__iter__` steps and stepless heap-owning
  splits reject.
- **TryNext's error edge is statically the StopIteration edge.** MjError
  carries no type tag, and none is needed: verify pins
  `call.raises == Some(exhaustion)` and raise effects are single-typed, so
  any error out of the callee is exactly the caught exhaustion. The VM's
  "other raise propagates" arm is dynamically unreachable post-check. The
  exhausted edge frees the caught error's message (LSan-clean), zeroes the
  ok payload, and joins; `yielded` is the ok-tag comparison. The zeroed
  element flows through the unconditional `DefVar raw` — release-safe
  because null-free is a no-op — so elements whose type carries a user
  `__deinit__` reject until the Collections slice brings real per-field
  flags. Raising reference-yielding `__next__` (List/Span iterators) keeps
  the Stage-4 `declare_function` rejection; the residue moved to the S5.5
  Collections roadmap bullet.
- **`raise StopIteration()` lowers as a nullary error-struct raise.** A
  zero-sized error struct materializes an owned MjError spelling the VM's
  `Display` of the value (`Name()`), giving byte parity for unhandled
  propagation (`unhandled error: __module$std$iterable$StopIteration()`).
  The VM's lifecycle log records only `Value::Error` raises, so the native
  trace stays silent for struct raises.
- **Bounded protocol.** `HasNext` calls the nominal `__len__` on a plain
  byte-copied receiver and compares `> 0`; `Next` requires the non-raising
  concrete target. The compiler-private pack fallbacks (`method: absent`,
  `call: absent`) reject contextually. The `CopyIteratorReference` adapter
  consults the concrete signature: a reference-returning target reads
  through the returned pointer with the VM's lifecycle copy
  (`copy_aggregate`), a value-returning target passes through.
- Not pinned by a fixture: the dispatch-unrolling budget rejection (a
  nine-deep `Iterable` chain is not constructible in reasonable fixture
  size) and the non-raising concrete-reference `Next` path (every candidate
  fixture is blocked on List generics until S5.5; the VM oracle covers it
  when those unblock).
- **Two latent S5.2 unification gaps surfaced and fixed.** The dest-unify in
  `infer_call` compared a reference-returning callee's declared referent
  against the caller's `ref`-typed destination (both spell `Int` — the
  manifest committed with S5.2 was stale and hid the break); `unify_result`
  now strips `ref` layers on both sides for every result unification. And a
  literal-typed argument register (`IntLiteral` displays as `Int`, making
  the mismatch invisible) failed against the concrete parameter the checker
  had admitted; `unify` now accepts literal-typed actuals without binding —
  `MaterializeLiteral` converts the value at the lowering boundary.

## Pointer/uninit storage intrinsics and builtins (S5.4)

The five storage instructions (`PointerStorageTake`/`PointerStorageDestroy`,
`UninitStorage`/`UninitStorageTake`/`UninitStorageDestroy`), the
`UnsafePointer` allocation family, and the `len`/`abs`/`min`/`max`/`round`/
`divmod`/`input` builtins plus the scalar
`__floor__`/`__ceil__`/`__trunc__`/`__ceildiv__` intrinsics now lower.
Runtime ABI v4 adds `mjrt_read_line` and the stdin-failure trap category (6).
Design decisions and recorded divergences:

- **Storage is payload-only; no tombstones, no init flags.** The VM tracks
  slot initialization structurally (`Value::Moved` tombstones in heap
  slots, the `Option` payload of `Value::UninitStorage`), but ownership
  verification guarantees the runnable subset never takes or destroys an
  uninitialized slot and never double-takes — so a native take is a raw
  byte move (`heap_take`'s `mem::replace`, no `__copyinit__`) and a destroy
  reuses `emit_drop_value` at the element address (compiled-`__deinit__`
  dispatch, droppable-field/raising rejections, and lifecycle-trace events
  included). The misuse traps live in off-gate `runtime_error` fixtures.
  `__UninitStorage[T]` instances lay out as their bare payload
  (`layout_of` resolves them through a `$mono`-aware
  `types::uninit_storage_element`) with deliberately **no synthesized MIR
  declaration**: a nominal struct without declared fields drops as a no-op,
  which is exactly the VM's leak-by-design `Value::UninitStorage` drop.
  `Proj::UninitPayload` is an identity projection under that layout, so
  `unsafe_write` stays the raw no-drop overwrite. Leaked payloads are
  inline, so LSan stays clean as long as leak-path payloads own no heap —
  a constraint on fixture design, not on the lowering.
- **Builtins split between mono and the backend by the VM's own split.**
  Nominal-receiver `len`/`abs`/`round` are the VM's by-name
  `call_dunder` dispatches, so mono rewrites them into unresolved
  `MethodCall`s and the existing rewrite arm resolves them through the
  shared `symbol::resolve_method_symbol` (invariant-6 anti-divergence);
  the same rewrite turns a struct-lhs `BinOp` into its operator method
  (`String.__add__` runs the compiled stdlib byte loop natively —
  `apply_binop`'s struct arm). The scalar forms intercept in `lower_call`:
  `abs` is `select`-based `wrapping_abs` on Int (`abs(i64::MIN) ==
  i64::MIN`), identity on UInt, `llvm.fabs.f64`; `min`/`max` promote
  statically by the VM's rank (Int < UInt < Float64) and pick by ordered
  `<=` (left-biased ties, NaN loses either side); mixed **concrete**
  Int/UInt rejects — the VM compares those exactly, which one unsigned
  compare cannot reproduce; `round` is `llvm.round.f64` (ties away, always
  Float64); `divmod` reuses the operators' flooring expansions (zero trap,
  `i64::MIN` divisor sanitizing) and stores `(q, r)` into the checker's
  nominal Tuple layout; `len` covers string bytes and static pack counts.
  Scalar `__floor__`/`__ceil__`/`__trunc__` are integer identity /
  `llvm.*.f64`; `__ceildiv__` is the negated flooring division (Int; the
  VM's non-wrapping negate would panic on `-i64::MIN` — an unexercised
  recorded divergence, native wraps), remainder-carry (UInt), or
  `ceil(a / b)` (Float64).
- **`input()` = prompt bytes + `mjrt_read_line` (ABI v4).** The prompt
  writes through the string machinery (no newline; `mjrt_write_stdout`
  flushes per call, so ordering holds even piped). The runtime fills a
  caller-owned 24-byte `MjString` whose first words double as the
  `MjStrDesc` the checker's `StringLiteral`-typed result reads —
  the nominal wrap is a separate constructor conversion. EOF yields the
  empty string (never blocks); a read *error* traps with the new category 6
  (the VM raises `RuntimeError::Unsupported` — unobservable in the corpus,
  recorded). Differential testing injects identical bytes into a test-only
  `VmBackend::set_input_override` (prompts append to the captured output;
  default behavior untouched) and the executables' piped stdin, so
  `input.mojo` is a true exe-differential row. `run --backend pliron` now
  inherits the CLI's stdin (`Command::output()` silently nulls it).
- **Pointer vocabulary.** `unsafe_offset` is MIR pointer `+`: a
  size-scaled byte GEP (the VM adds to its element-counted offset with an
  overflow check natively elided — off-gate); pointer `-` keeps a
  contextual rejection. `UnsafePointer.alloc`/`alloc_aligned` share the
  `unsafe_alloc` core (`mjrt_alloc`, count-overflow trap);
  `unsafe_dangling` is `ptr null` — the VM errors on `free` of a dangling
  pointer while `mjrt_free(null)` is a no-op, a recorded off-gate
  divergence. `Const::None` lowers as a zero-sized erased register
  (consumers read nothing), which the statement-position pointer intrinsics
  produce.
- **Direct `__init__` calls bind their destination as `out self`.** The
  checker's specialized constructor symbols (and their `$mono` instances)
  arrive as plain `Call`s whose declaration facts exclude the receiver;
  `lower_call` now allocates the result storage and binds the remaining
  arguments past the receiver — the struct-name constructor path's exact
  contract. This unblocked every `$ov$$mono$` constructor arity failure.
- **Mono hardening.** `bind_type` merges literal-typed actuals with
  concrete bindings order-independently (receiver-first and result-last
  shapes both bound `T` twice; `Int` and `IntLiteral` display identically,
  producing the absurd "conflicting solutions for `T`: `Int` and `Int`" —
  conflict messages now carry the structural forms). `discover_structs`
  seeds destroy/take `element` types so element lifecycle methods always
  join the walk, and **rejects instance-identity collisions**: output
  declarations dedupe by name, and a checker-concrete generic application
  keeps its template name, so two instantiations of one template
  (`UnsafeMaybeUninit[Int]` + `UnsafeMaybeUninit[Recorder]`) would
  silently share whichever declaration was discovered first. Sharing is
  tolerated only when the field substitutions are equivalent **modulo
  pointer element types** — every pointer is one opaque target word and
  drops inertly, which keeps the ubiquitous `_RawAlloc`/`List`/`Array`
  shapes (fields differing only behind `UnsafePointer[T]`) compiling as
  before; payload-carrying differences reject — with distinct layouts or
  per-instance destructor identities, sharing would be wrong, not merely
  imprecise. Renaming concrete applications (and bare in-body `Self`
  references) to instance symbols is the Collections slice's
  canonicalization prerequisite; until then those fixtures stay excluded
  with a contextual diagnostic.
- **Scope shifts.** `Slice` descriptor construction moved to S5.5 beside
  its consumers (no manifest row blocks on the instruction alone; the
  `slice.get` capability row already said Collections). The S5.3 rejection
  pins that S5.4 made compile (nominal `String +`, discarded `divmod`)
  were replaced with the generic-constructor-instance and pointer-`Sub`
  rejections.
- Not pinned by a fixture: the mixed concrete-Int/UInt `min`/`max`
  rejection and the stdin-failure trap (neither shape is constructible as
  a runnable `ok` fixture; the capability matrix and unit paths cover the
  decisions).
