# Mojito Roadmap

The single task tracker: an ordered checklist of **unfinished** work only.
Completed work leaves this file — the supported surface lives in
[`docs/features.md`](features.md), user-visible history in
[`CHANGELOG.md`](../CHANGELOG.md), and lasting design invariants in
[`docs/architecture.md`](architecture.md). North star: self-hosting — prefer
the smallest honest language change that unlocks a real library pattern, with
positive and negative tests.

Sections and their checkboxes are in implementation order; the first unchecked
box is the default next task. *(recurring)* and *(any order)* sections are
exempt from the ordering.

## Ordered Work

### 1. Native Backend

- [ ] **Cranelift alternate backend** *(deferred)* — Pliron is Mojito's
  supported route to LLVM and optimized binaries, but the verified-MIR waist
  deliberately permits a Cranelift backend. If justified by portability,
  build-cost, or upstream-risk evidence, implement the same acceptance slices
  with the shared target/layout/runtime ABI and differential corpus. Do not
  fork language semantics or make Cranelift a required compiler layer.
- [ ] **Native SIMD lowering residues** — the vector lowering (LLVM fixed
  vectors in SSA over the unchanged lane-aligned storage ABI;
  `docs/notes/native-simd-pliron-assessment.md` §As Built) leaves: lane
  *writes* (`v[i] = x`) store one lane through the place address (SROA
  rebuilds the vector at release; `O0` keeps the store); float→int casts
  saturate lane by lane through `llvm.fptosi.sat.i128.f64` (the
  `v{N}i128` vector intrinsic's x86-64 legalization is unverified); float
  `reduce_min`/`reduce_max` fold lane by lane through `llvm.minnum`/
  `llvm.maxnum` (the VM's `f64::min`/`max`), whose `(-0.0, +0.0)` result is
  unspecified on both backends, so fixtures avoid mixed-sign zeros; a
  lane-index trap exits natively with the `unhandled error` category while
  the VM raises `SIMD lane index … out of range` (a dedicated category is a
  `mojito-runtime` ABI bump); vectors wider than a register legalize by
  splitting, so `simd_lowering_emits_vector_code`'s object listing, not IR,
  is the vectorization evidence. Front end: a struct with a multi-lane SIMD
  field rejects a bare literal argument (`P(1)` for `SIMD[DType.int32, 4]`:
  upstream's implicit `SIMD(IntLiteral)` initializer), and a field write
  through a `List` subscript (`xs[i].field = v`) fails MIR verification for
  every element type.
- [ ] **Native generic-holder temporaries with heap-owning implicitly
  copyable fields** — `GenericHolder[Box](Box(5))` or Dict's
  `DictEntry[Optional[Int], V]` appended to a `List` reads freed memory
  natively (VM correct). Keeps `Dict[Optional[Int], _]` VM-only
  (`conformance/fixtures/optional_dict_keys.mojo` stays out of `assets/ok`).
  Bisect the `var` field-move of the copy-lifecycle value inside the generic
  constructor against the owned-temp marking in
  `crates/mojito-pliron/src/lower/calls.rs`.
- [ ] **Native converting-constructor defaults** — lower a
  `CheckedConst::Construct` default (`arg: Optional[T] = None`); the
  default-fill sites reject the aggregate (`checked_const_value` errors on
  `Construct`). Emit the constructor call at default-fill via `lower_call`
  (the `NoneType` argument is `LowerTy::ZeroSized`).

### 2. Catch Up To Current Mojo *(recurring — reopens at every nightly re-pin)*

- When the pinned nightly moves: re-pin [`docs/mojo-nightly.md`](mojo-nightly.md),
  re-probe [`conformance/parity.tsv`](../conformance/parity.tsv), burn down
  the divergences (`parity.tsv` notes; `mojito-only`/`mojo-only` rows of
  `conformance/cases.tsv`).
- Rule: Mojito matches or subsets Mojo — extensions only as temporary
  bridges tracking upstream deprecations or cited upstream-roadmap features,
  re-probed at every re-pin.
- The `a79fbdf59f2` pass (2026-08-26, Mojo `1.1.0.dev2026082605`) is
  complete (`docs/mojo-nightly.md`); the next re-pin recreates this
  section's checkbox.
- [ ] **Behavioral divergences from the pinned Mojo — burn to zero**
  *(standing; every new one lands here with a probe or a `cases.tsv`
  `mojito-only`/`output-diff` row, and leaves when its probe promotes to an
  `assets/ok` fixture)*. Open today: `partial-field-move-parent-used` —
  moving one field out of a struct (`p.a^`) while the parent is used
  afterwards (`p.b`) is rejected upstream (`value 'p.a' cannot be consumed,
  because 'p' is used later`) but accepted and tracked field-wise by Mojito
  (`tests/drops_test.rs::partially_moved_field_is_dropped_once_at_its_new_owner`
  pins the Mojito behavior). Retained rows, by decision:
  `subtree-origin-cast` (a cited bridge to upstream's `#lit.origin.subtree`
  experiment, re-probed each re-pin), `reference-valued-aggregate` (the
  extension tracked by the next task), and the `output-diff` row
  `deinit-body-field-loop-read` (the pinned Mojo destroys a `deinit self`
  field read only inside a loop body at the destructor's entry and then
  reads the destroyed value — an upstream bug Mojito does not reproduce;
  re-probed each re-pin).
- [ ] **Direct `ref` struct fields are a Mojito extension** — upstream
  rejects `var f: ref[o] T` fields (`'ref' patterns are only valid on the
  left side of an assignment`) and spells reference storage through
  `Pointer[T, origin]`. Migrate the stdlib and the ~40 assets declaring ref
  fields / ~110 using `ref[...]` annotations (e.g.
  `assets/ok/ref_field_chained_view_call.mojo`,
  `assets/ok/delegated_ref_return_projection.mojo`,
  `assets/ok/ref_field_view_for_temporary.mojo`) to Pointer storage, then
  reject ref fields with upstream's diagnostic and flip
  `conformance/fixtures/reference_valued_aggregate.mojo` to `reject`.
- [ ] **Mojito-specific shortcuts to move toward Mojo's shape** *(standing,
  any order)* — places where Mojito's stdlib leans on the Rust runtime
  where upstream is pure Mojo. Each is a candidate port, preferred over any
  new bridge (2026-09-07 direction):
  - the literal-filled `String.__init__(literal)` and
    `StringSpan.__init__(literal)` constructors and the
    `String._as_string_literal()` struct-to-literal bridge behind
    `String.write_to` (upstream: `StringLiteral` is a `StaticString`
    and `write_to` writes the bytes);
  - number formatting in Rust — the VM's `Display for Value` and the
    native `mjrt_fmt_i64`/`mjrt_fmt_u64`/`mjrt_fmt_f64` (upstream formats
    `Int`/`Float64` in Mojo: `_write_int`, Dragonbox in `format_float`);
  - `mjrt_repr_string` for the native string repr (the VM side is already
    Mojo, `String.write_repr_to`);
  - `mjrt_pow` for integer `**` (upstream's `Int.__pow__` is Mojo).
  Runtime services with an upstream analogue stay: `_mojito_abort`
  (`os.abort`), `mjrt_read_line` (`input`), allocation, traps.

### 3. Grow The CPU Standard Library *(demand-first)*

- [ ] **Collection API parity** — grow the tuple, slice, optional/variant,
  and String surfaces toward the audited head (`docs/features.md` records
  what lands). The tasks below are in impact order: soundness of the
  executable oracle first, then everyday spellings that reject today, then
  parity details; no task depends on a later one. Every bullet is a
  conscious, recorded limit (not silently unfinished work); a task closes
  when its bullets are done, and a residue discovered inside a task moves
  to the task that owns its fix, never back to a finished one. The tasks
  group the limits by the fix they share.

  1. **Temporary views and their origins** (the temp-view-anchoring
     family; the one cluster where the VM oracle can be unsound).
     - Destructuring a returned `Tuple` of origin-bearing views (`var a, b
       = s.split_at_grapheme(n)`) reports `use after Pointer deallocation`
       on the VM while a bound pair read by index (`pair[0]`, `pair[1]`) and
       a returned tuple literal of views both run.
     - `split_at_grapheme` returns the receiver's origin where upstream
       returns `ImmOrigin` views.
     - An overloaded free def over a temporary view argument
       (`reversed(StringSpan(s))`; bind the view first, or call
       `graphemes_reversed()`) frees the view before the loop runs.
     - `value()` on a temporary `Optional` holding a view
       (`it.peek_next().value()`) is rejected as a reference to a non-place
       (bind the Optional first).
     - A `String` copied from a temporary view of a local at the local's last
       use inside a `return` expression or a self-assignment (`return
       String(head.rstrip("/"))`, `head = String(head.rstrip("/"))`) frees
       the local before the copy reads it
       (`conformance/probes/view_copy_return_use_after_free.mojo`); bind the
       copy to a local first, as `std/os/path/path.mojo` does.
     - Boundary, not work: casting a tracked pointer to an untracked origin
       at its source's last use
       (`s.unsafe_ptr().unsafe_origin_cast[ImmUntrackedOrigin]()`) frees
       the source before the pointer is read — as upstream would.
  2. **Compile-time evaluation residues** (what VM CTFE can bind and
     resolve).
     - A VM-evaluated compile-time expression whose result is pointer-backed
       (`comptime C = M.copy()`, a bare `Optional`, a `String`) cannot cross
       back (`cannot cross back from VM CTFE`; upstream binds it): the
       freezable results are scalars, Bool, String, tuples, fieldwise
       structs, and displays.
     - Compile-time Dict/Set key identity is structural `CtValue` equality,
       exact for every prelude key type (a user-struct key with a
       non-fieldwise `__eq__` diverges).
     - The typing probe checks the CTFE subprogram once more per VM-bound
       expression.
     - A non-parameterized struct alias mentioning a value parameter
       (`comptime Alias = S[Self.T, Self.length]`) resolved through an
       instance (`S[Int, 3].Alias`) keeps `length` symbolic (`expected
       S[Int, length], found S[Int, 3]`): `associated_type_from_base`
       substitutes type parameters only. Parameterized aliases substitute
       both.
  3. **Monomorphization coverage and cost** (where the erased path still
     stands in for an instance clone, and what minting costs).
     - An instantiation whose argument mentions `StringLiteral` (`{"a": 1}`
       is `Dict[StringLiteral, Int]`) keeps the erased path: its values
       keep the literal runtime representation while an un-annotated
       binding materializes `String`. Lifting it needs one runtime
       representation for `StringLiteral` and `String`, or typing the
       display as `Dict[String, Int]` as Mojo does. `StringDict.__getitem__`
       stays a `Copyable`-guarded copy for the same reason: its
       `StringLiteral`-keyed entry list keeps the erased path, where a
       reference result cannot spell its interior origin.
     - A call inside an unstamped bundled body on a bundled struct's
       generic method keeps the erased path (requests are admitted from
       user code, clone bodies, variadic specs, instances, and user
       structs).
     - An instance clone whose walk cannot resolve an application (a
       variadic template over a nested public `Tuple` argument) is dropped
       to the erased path rather than failing the program.
     - Value-parameterized structs get no instance clones, so an erased
       body's `_unqualified_type_name[Self.T]()` spells `T`
       (`repr([1, 2])` prints `Array[T, 2]([Int(1), Int(2)])` where
       upstream prints the element type), and a `Self.n` bracket argument
       inside such a body (`Counter[Self.length](i)`) is VM-only: native
       monomorphization needs a compile-time-constant value argument
       (`unsupported value parameter 'length' is not compile-time
       constant`).
     - Clones are minted per whole instance (no reachability pruning) and
       re-checked each discovery round: `benchmarks/compile/stdlib_heavy`
       is about 2.2x its pre-clone baseline in release
       (`docs/performance.md`); the repr methods added 2026-09-05 cost
       about 5% in debug across the compile benchmarks. Lever:
       reachability-pruned minting.
  4. **Variadic packs and tuples** (what the pack machinery types
     syntactically or refuses).
     - Type-pack calls inside a nested `def` and whole-pack-forwarded calls
       keep the syntactic element-typing path (`a heterogeneous pack
       specialization needs an expression whose type is statically evident
       before checking` for `take(b, 1)` over a local `b`); top-level calls
       consult the checker's instantiation. Lifting it needs four pieces:
       nested templates live only in `NestedMono` and are deleted by
       `replace_templates` before the checker sees them, the checker would
       have to accept a nested variadic shell abstractly,
       `def_specialization_requests` is top-level only, and
       `Mono.def_call_targets` must reach `NestedMono::scan_expression`
       with a `def_request_target` fallback.
     - A user variadic struct application as a pack element
       (`Tuple[TypeNames[Int]]`) keeps the fixed-arity diagnostic: its
       erased shell has no sound nominal form (the public `Tuple` is the one
       compiler-known template whose `*Ts` absorbs every argument).
     - A public Tuple as an explicit type argument does not conform to
       `Deinitable` (`make[T: Defaultable & Deinitable]()` over
       `Tuple[Int, Bool]` reports the bound failure) while the inferred
       shape runs.
     - A standalone nullary vector construction `SIMD[d, w]()` rejects
       (`SIMD construction expects w element(s) or 1 to splat, got 0`);
       only a Tuple element defaults to zero lanes.
  5. **Everyday spellings that still reject** (checker context and
     stdlib API shapes).
     - A list display as an argument to an explicitly applied constructor at
       runtime (`Dict[String, Int](["a"], [1], None)`) takes no context from
       the parameterized `List[Self.K]` parameter and materializes as
       `Array`, so no overload matches; spell the lists
       (`List[String]("a", __list_literal__=None)`), as a compile-time
       value's materialization does.
     - A struct's own value parameter as a SIMD width in a signature
       (`def zeros(self) -> SIMD[DType.int64, Self.length]`) reports `SIMD
       width must be a positive power of two, got a type`: the width slot
       needs a concrete comptime `Int`, where upstream binds the parameter.
     - String literals have no methods (`"abc".byte_length()` rejects):
       a literal-typed value converts through `String(...)` first.
     - An annotated `Optional[T]` local initialized from a bare struct
       construction (`var o: Optional[P] = P(4)`) fails MIR verification
       with an untyped register for the constructor call; spell the
       wrapper (`Optional[P](P(4))`, `Optional(P(4))`) — a literal payload
       (`var n: Optional[Int] = 6`) converts.
     - `if`/`while` accept a width-1 bool lane through the `Bool(x)`
       truthiness conversion, but `and`/`or` still demand `Bool` operands
       (`a == b or c < d` over `UInt64`s rejects; nest the tests).
     - A string literal does not bind a trait-bounded type parameter the
       nominal `String` satisfies (`isdir("/tmp")` reports `'StringLiteral'
       ... does not conform to trait 'PathLike'`); spell `String("/tmp")`.
     - Struct-level `comptime NAME = Self(n)` constants (upstream's
       `ErrNo.ENOENT`) do not fold: the associated-constant evaluator handles
       prefix/infix/tuple/list expressions only.
     - `@fieldwise_init` beside a hand-written `__init__` rejects; spell every
       constructor by hand.
     - A bound-generic def whose template body calls itself with a concrete
       argument (`makedirs(head, exist_ok=...)` inside `makedirs[PathLike]`)
       mints its own specialization while checking and reports it undefined;
       recurse through a non-generic helper.
     - Nullary construction of a sized-scalar array (`Array[Int8, 1024]()`)
       reports the scalar's `Defaultable` construction unsupported; spell
       `Array[Int8, 1024](fill=0)`.
     - `Pointer.unsafe_bitcast[U]()` is not typed (an origin-cast-style
       forwarding leaves the MIR register at the old element type);
       `CStringSlice` views `Byte` elements instead of upstream's `c_char`.
     - No `std.builtin.rebind.downcast`: `Dict`/`Set` guard `keys`/`values`/
       `items`/`__iter__` (and `Dict.keys` needs copyable values as well as
       keys, since the key view wraps the entry view) with `where
       conforms_to(K, Copyable)` instead of laundering the parameter.
  6. **Naming and Unicode details** (output text only).
     - `_unqualified_type_name` spells nested structs unqualified where
       upstream keeps a non-prelude struct's module path
       (`Optional[std.collections.dict.Dict[...]]`, `List[up.Flag[True]]`)
       while `List`/`Optional`/`String`/`SIMD` stay bare.
     - Grapheme segmentation is the documented UAX #29 essentials subset
       (hand-maintained Control/Extend/SpacingMark ranges, no
       Extended_Pictographic or Prepend data); reverse iteration re-scans
       forward from the nearest CR/LF/Control boundary.

- [ ] **Filesystem and I/O residues** — behind the landed files, streams,
  paths, and tempfile stage (`docs/features.md`):
  1. the deferred tail: public `stat`/`lstat`/`stat_result`, `realpath`,
     `symlink`/`link`/`chdir`, `isatty` (`FileDescriptor.isatty`/`fchdir`),
     `OptionalPointer` (null tests spell `Int(ptr) == 0`), `ErrNo`'s named
     constants, `~user` expansion (`getpwnam`), the VM's per-process
     environment overlay (native `setenv` writes the real environment),
     `NamedTemporaryFile`, `FileHandle.read` into a typed
     `Span[Scalar[dtype], origin]`, `Path.stat`/`lstat`/
     `_dir_of_current_file`, `KeyElement`, `_get_random_name` over
     `std.random` (reads `/dev/urandom` today), a generic `__exit__[E]`;
  2. traps the stage worked around, each its own fix: an overloaded
     constructor spelled with the `StringSlice` or `Byte` alias mangles a
     different key than the `StringSpan`/`UInt8` the call selects (the ports
     spell the canonical names); a temporary view as a method argument
     (`s.take(String("x").as_bytes())`) is a VM "reference receiver must be a
     place" rejection and as an augmented-assignment operand
     (`p /= StringSpan(s)`) a VM use-after-free; `Span[mut=True, T, _]`
     fails origin inference (spell an `[origin: Origin[mut=True]]` binder);
     the two VM destruction-order divergences the stage found live in the
     behavioral-divergences task of section 2; `range` is
     invisible in `std.string`, and a module loaded while the prelude
     bootstraps (`std.io` and the whole `std.os` graph now) must import
     `String` explicitly — that graph costs Hello World about a second of
     debug compile time (`docs/performance.md`).
- [ ] **Time, random, and testing slices** — deterministic testable cores;
  host-dependent behavior behind runtime services.

### 4. Packaging, Artifacts, And Developer Tooling *(any order unless noted)*

- [ ] **Compile-time performance** — Hello World is 0.8 s release / 4.6 s
  debug (`docs/performance.md`; was 24 s / 52 s before the shared
  `Arc<CheckedTables>`). Next, in order:
  1. avoid the redundant checker passes (Hello World re-elaborates and
     re-checks once because the request scan always finds the prelude's own
     Tuple/def requests, and every check runs two transfer rounds);
  2. `checked_var_types` scans the whole expression table per variable and
     `explicit_destroy` re-derives deinitability per struct per pass;
  3. only then cache the elaborated/checked stdlib across processes.
- [ ] **Feature and target options** — checked CLI/build configuration
  recorded in artifacts and diagnostics.
- [ ] **Compiled package artifacts** — a versioned `.mojoc` representation
  (modules stay non-first-class). Per-directory resolution order:
  1. source package
  2. `.mojoc`
  3. source module
  4. legacy `.mojopkg`
- [ ] **Debugging metadata and inspection** — stack/source diagnostics, MIR
  inspection, debugger-oriented value rendering.
- [ ] **Testing tools** — Mojito-native assertions, expected-error tests,
  differential-harness integration.
- [ ] **Distribution reproducibility gate** *(last)* — the release check
  rebuilds, tests, documents, and reproduces conformance from the crates.io
  archive alone.

### 5. Code Organization Follow-Ups *(any order — behavior-preserving)*

The 2026-09 module split (`docs/symbol-map.md`) removed every file over
3,000 lines. What remains needs semantic extraction, not line moves:

- [ ] **Split `expr_unconverted`** — `mir/lower_expr/expr.rs` (~2,090 lines)
  is one match over `ExprKind`; extract arm groups into `Flatten` methods.
- [ ] **Split `infer_method_call`** — `checker/method_calls/mc_infer.rs`
  (~1,530 lines) is one method; extract receiver-family branches beside
  `selection`/`statics`/`builtin_types`.
- [ ] **Split `verify_instruction`** — `mir/verify/instr.rs` (~1,320 lines)
  is one match over `MirInstr`; extract per-family check helpers.
- [ ] **Shrink the 2 kloc band** — split further only along a cohesive
  seam while touching them: `checker/traits.rs` (2,629),
  `mir/lower_stmt.rs` (2,595), `checker/inference.rs` (2,520),
  `checker/statements.rs` (2,471), `ast.rs` (2,464), `mir.rs` (2,426),
  `checker/type_resolution.rs` (2,395), `runtime.rs` (2,235), `checker.rs`
  (2,232), `comptime/rewrite.rs` (2,179), `checker/declarations.rs`
  (2,118), `mir/text/write.rs` (2,116), `backend/vm/exec.rs` (2,085).

## Task Lifecycle Policy

`roadmap.md` is the only task list: no parallel todo file, no retained
completed tasks.

- Unfinished work is an unchecked, outcome-oriented task in **Ordered
  Work**; design detail lives in plans or `docs/notes/`.
- A task is complete only when implementation, focused positive and negative
  coverage, documentation, and `scripts/check` agree. In the same change,
  delete it here and record the outcome in `docs/features.md`,
  `CHANGELOG.md`, and (for design invariants) `docs/architecture.md`.
- Rewrite partially completed tasks so only the remaining outcome is stated;
  prefer one checkbox per independently demonstrable outcome.

## Working Rule

1. Start with a self-hosted library or small user-facing acceptance case.
2. Record the current failure with a focused test.
3. Implement the smallest compiler change that makes the program honest.
4. Add positive and negative coverage at the owning compiler phase.
5. Run `scripts/check` before marking the task complete.
