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

## Collections, part A: canonicalization, call contracts, subscripts (S5.5)

The Collections slice's first half: mono instance-identity canonicalization,
the keyword/default call-contract gaps, and all four subscript instructions
(`Index`, `Slice`, `MultiIndex`, `MultiSet`) with slice-descriptor
construction. This unblocked the List/Dict/Span/String fixture clusters —
the `List.grow` unresolved-`T` group alone was 43 excluded rows.

- **Instance-identity canonicalization.** In-body `self` is typed as the
  bare template (`Struct("List", [])`, `src/mir.rs`), and non-`__init__`
  methods carry no `param_decls`, so method instances used to key on the
  bare template name with first-enqueue-wins bindings — masking a
  wrong-instance-sharing miscompile (`List[Int].append` and
  `List[String].append` would share one body). Now: `substitute_ty`
  renames every concrete application of a generic template (a member of
  the source program's generic-template set — checker-specialized
  `Tuple$tN` names with empty `param_decls` keep their names) to its
  instance symbol; `Bindings.self_instance` rewrites the bare in-body
  `self` spelling to the concrete instance; and method instances take
  their owner's spelling (`List$mono$TInt.grow` via
  `retarget_method_symbol`), so lowering's name-composed lifecycle and
  overload lookups (`{name}.__init__`, `{name}.__deinit__`,
  `constructor_init`'s unique-overload scan) work unchanged. The
  `__init__` owner-restating `param_decls` prefix is stripped from the
  instance arguments (the owner already carries those solutions);
  overload-qualified `mut_self_methods` entries are respelled under the
  instance name (a silent write-back miss otherwise, not a reject).
- **Receiver-driven inference peels references** (the VM dereferences
  `Value::Ref` receivers before dispatch), `bind_type` ignores `ref` and
  pointer origins when comparing solutions (origins erase from the runtime
  ABI; call sites solve `T = ref Int` with differing origins — first
  spelling wins), and the reference-yielding subscript's destination is
  never unified as a result (a handle's `ref` layers are
  indistinguishable from a reference element type).
- **The raising reference-returning ABI.** `_ListIter.__next__` raises for
  exhaustion *and* returns a reference, so the Stage-4 declaration
  rejection became the tagged outcome with a single place pointer as the
  ok payload (`OutcomeAbi.ok_is_reference`). `TryNext` consumes it: the
  `for x` adapter contract copies the element out on the ok edge; the
  `for ref x` contract keeps the handle (temp slot holds the pointer, the
  exhausted edge zeroes it, the loop never reads it). Generic call sites
  (`Dict.__getitem__`) define the destination as the loaded handle — the
  checked `reference_result` contract.
- **Reference-slot conventions.** Reference-typed variable slots always
  hold real referent addresses: `MakeRef` of a bare reference-typed
  variable re-borrows (loads the stored handle, collapsing chains like
  the VM's recursive `Value::Ref` reads), while a projected place whose
  element is itself a reference (`List[ref T]` elements) addresses the
  slot — its consumers dereference explicitly. `mut`/`ref` receiver
  aliasing dereferences once when the receiver place designates a
  reference handle (an iterator's `src` field).
- **Subscripts.** Nominal dispatch is a hand-bound method call
  (`lower_subscript_call`): receiver by compiled convention, actuals
  matched through `call::match_call_slots` (the place for a `mut`/`ref`
  slot comes from the matched source — `arg_places[p]`/`kwarg_places[k]`
  — never the parameter position), `mut self` write-back, and
  `emit_bound_call`'s raising/sret/pointer returns. Intrinsic
  `TupleStorage`/`VariadicStorage` subscripts and pack place projections
  resolve constant (pending-literal) indexes to static offsets; runtime
  pack indexes stay rejected until part B. Slice descriptors are a raw
  32-byte `{start, end, step, flags}` frame (`Value::Slice`'s
  `Option<i64>` fields); `discover_structs` synthesizes the
  checker-virtual `Slice`/`ContiguousSlice`/`StridedSlice` declarations;
  bound accesses materialize `Optional` values over frame-backed payload
  slots (`{data → payload, _size ∈ {0, 1}}` — the observable state the
  VM's `slice_bound_optional` constructor calls produce, with no heap to
  own), and `indices`
  lowers as branch-free selects mirroring `normalize_slice_bounds` (zero
  step traps as the unhandled-error category — no runtime-type-error trap
  exists; recorded divergence).
- **VM-synthesized dispatches, natively.** `len`/`abs`/`round` rewrites
  gained `Int`/`Float64`/`Bool` (`builtin_convert`'s struct arm →
  `__int__`/`__float__`/`__bool__`); `Writer.write` formats each argument
  and feeds the receiver's compiled `write_string` (mirrored in mono and
  reachability — the callee exists only in the expansion); `print` of a
  nominal struct displays through its `write_to` instance over a
  builtin-string accumulator writer (the `Some[Writer]` sugar parameter is
  infer-only and binds to `StringLiteral` by spelling), whose `write`
  appends by grow-and-copy; `_mojito_abort` reports through
  `mjrt_unhandled_error` (exit-category divergence from the VM's distinct
  abort noted); the struct-to-literal bridge (`_as_string_literal`) stays a contextual
  rejection — its bytes would need an owner the drop-inert literal value
  model cannot record (the VM's arena never reclaims; a native copy
  stored into a literal-typed field leaks with no releasing owner), so
  the codepoint-display fixtures remain excluded; `Type(copy=value)`
  dispatches to `__copyinit__` (`construct_via_copy`), never an
  `__init__` contract; and a `^` transfer of a struct with a compiled
  `__moveinit__` runs it (`move_value`) when the type also owns its
  allocations through a destructor — a destructor-less pointer owner
  leaks under real frees (the VM's arena tolerates it) and stays
  rejected for S5.7 — with the reachability edge keyed on actual
  `UseVar { Move }` sites so never-moved structs with rejecting move
  constructors stay compilable.
- **Fork semantics for real frees.** The VM's plain clone
  (`place.load`/`Store` of borrowed values) aliases arena storage safely
  because the arena never reclaims; native frees make every alias a double
  release. `load_from` and `Store` of borrowed heap-owning values now
  *fork*: byte copy plus re-duplicated String/Error buffers, recursing
  through fields — silent (user `__copyinit__` never runs, matching the
  VM's plain clone), with raw-pointer owners rejecting contextually. The
  same rule fixed `GetField` of heap-owning fields. A literal argument
  entering a nominal-String parameter materializes through the
  constructor bridge (the VM's runtime coercion for generic parameters the
  checker could not wrap). Discarded stdlib-collection temporaries (a
  printed slice result, a read-convention receiver copy) release through
  their compiled destructor chains — pure frees the VM's arena leaves to
  the collector; user destructors could observe the difference (the VM
  never drops register temporaries) and stay under the invisible rule,
  and the every-row LSan lane is what forced each of these ownership
  decisions.
- The executable wrapper no longer redeclares `mjrt_unhandled_error` when
  body lowering already did (`SymbolRedefined` verify failure), and
  `MOJITO_PLIRON_DUMP_ON_VERIFY_ERROR=1` now prints the verifier's debug
  detail alongside the module dump.
- Ratchets: 147 → 206 exe-differential, 136 → 83 excluded (31
  ineligible, 4 raise rows unchanged). Register-temporary releases are
  untraced (the VM never drops register temporaries), and the
  lifecycle-trace lane keeps its prior fixtures: native traces spell
  canonicalized instance names (`List$mono$TInt`) where the VM logs bare
  templates — trace-name normalization joins part B.
### Part B — variadic packs, per-leaf flags, trace normalization

- **Variadic direct calls and runtime packs.** Monomorphization
  specializes a variadic callee per call-site arity: each overflow
  positional unifies against the declared pack *element* type (the
  declaration records the element — a concrete `RuntimePack`/`Tuple`
  spelling means the checker already specialized it), the arity joins the
  instance identity as a value argument (`sum$mono$V3`), and substitution
  reifies both the parameter and `VariadicPack(T)` spellings into concrete
  `RuntimePack([T'; n])`. Lowering builds pack storage at the declaration's
  recorded pack position by *relocating* each overflow argument
  (`store_to` transfers owned temporaries — the VM's `Tuple(*args^)`
  move); zero-sized marker parameters (`__list_literal__`'s `NoneType`)
  keep signature entries but no physical arguments. A variadic callee
  always binds through the slot matcher: an argument count equal to the
  physical parameter count (arity one against the single pack slot) must
  still build pack storage. Pack-fallback iteration advances a
  backend-side position slot (a typed `i64` entry alloca — byte-array
  slots mispromote under mem2reg) over the uniform-stride element layout;
  an empty pack's advance is a zeroed dead-code edge.
- **Value parameters.** A value-parameter member read (`Self.length`)
  folds to the bound constant carried by the receiver instance's type
  arguments in both its MIR spellings (`GetField`, and `place.load` with
  one `Field` projection); the redundant call-site value registers erase
  after a successful resolve (the instance identity carries the
  solutions). A comptime-specialized subscript accessor
  (`Tuple$tN.__getitem_param__$i`) joins its constant index to the
  instance identity — receiver-only identity collapsed same-element-type
  indexes onto one body.
- **Per-leaf presence flags (partial-move drops).** A depth-1 `MovePlace`
  out of a struct field or pack element clears a per-leaf `i1` presence
  flag (allocated in the entry block for every leaf some move targets);
  whole-variable stores and `DefVar` restore them. `DropVar` over a
  tracked variable destroys exactly the surviving leaves — struct fields
  in reverse declaration order, pack elements left-to-right (the VM's
  `drop_value` orders) — and any tombstoned leaf suppresses whole-value
  `__deinit__` work (nested flag guards; the destructor-with-
  droppable-fields combination still rejects). `ConsumeVar` with
  droppable fields destroys the surviving droppable struct leaves the
  same way (the VM's post-named-destructor field destruction); deeper
  untracked moves keep the blanket contextual rejection.
- **Ownership fixes forced by the new coverage** (each caught by the
  per-row LSan/ASan lane): a compiled-init constructor result is an owned
  temporary when it owns heap — unmarked results made a later
  `self.field = Ctor(...)` store *fork*, stranding the original's buffers
  with no releasing owner; a plain byte copy of a heap-owning value
  (`UseMode::Copy` without any copy-constructor chain) forks and releases
  after its own last use — drop elaboration destroys the owning variable
  immediately after its last use, before the temporary's read; and owned
  string bytes may never enter drop-inert literal-typed storage (the
  recorded literal-ownership gap behind the struct-to-literal bridge) —
  the `String(writable_struct)` snapshot capture rejects there, so
  `tstring_forms` returns to the excluded set with a precise reason.
- **Struct display recursion.** `Writer.write` on the builtin-string
  accumulator appends nominal arguments through their own compiled
  `write_to` over the *same* descriptor (the VM's `format_value`
  recursion); `String(x)` over a nominal struct accumulates through
  `write_to` into a fresh descriptor that doubles as the 16-byte
  StringLiteral storage aggregate consumers read. Reachability edges
  cover both (`print`/`String` calls and str-writer `write` arguments).
- **Runtime string equality.** `==`/`!=` over runtime string-shaped
  operands (a `Dict[StringLiteral, _]` key probe in `find_index`) lower
  as an inline length-then-bytes compare loop over slot-backed state;
  compile-time constant pairs keep folding.
- **Trace-name normalization.** Lifecycle events spell the bare template
  (`List`) the VM logs, splitting off the `$mono` instance suffix;
  checker-specialized names (`Tuple$t2[…]`) are the runtime struct name
  on both sides and pass through. `pliron_list_core` still stays out of
  the trace lane: generic-collection copy/consume event *sets* differ
  structurally (compiled destructor chains trace element drops the VM's
  arena never performs).
- Fixtures: `pliron_tuple_pack` (public Tuple + `*args` at arities 0/1/3;
  pack relocation under the sanitizer lane) and `pliron_partial_move_drop`
  (conditional depth-1 field move, early return, surviving-leaf drops;
  also in the lifecycle-trace lane). Debug: `MOJITO_PLIRON_DBG_TEMPS=1`
  prints owned-temporary marks and releases per function.
- Flips beyond the plan's list: `dunder_vec2`, `comptime_alias_generic`,
  `self_hosted_algorithms`, and the four Dict-iteration ownership rows
  (`__moveinit__` consumption now destroys surviving leaves; their keys
  compare as runtime literals). `generic_copyable_iterator_refinement`
  landed with part A's raising-reference `__next__` — the
  destructor-bearing-element residue had no remaining rows, so part B
  ships no `TryNext` flag machinery.
- Ratchets: 206 → 229+ exe-differential, 83 → ≤61 excluded (set to
  observed counts at regen; two new pliron rows join the manifest).
