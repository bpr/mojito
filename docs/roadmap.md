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

- [ ] **Cranelift alternate backend** *(deferred, not planned)*

  Not to be fixed now: Pliron is Mojito's supported route to LLVM and
  optimized binaries. The verified-MIR waist deliberately permits a
  Cranelift backend, so this stays open only as an option.
  - Build it only if portability, build cost, or upstream risk justifies it.
  - It must implement the same acceptance slices over the shared
    target/layout/runtime ABI and the differential corpus.
  - It must not fork language semantics or become a required compiler layer.

- [ ] **Native SIMD: lane writes still go through memory**

  Problem: `v[i] = x` stores one lane through the place address instead of
  an `insertelement` on the vector.
  - At release SROA rebuilds the vector, so only `O0` keeps the store.
  - The read side already extracts from the loaded vector.
  - Design record: `docs/notes/native-simd-pliron-assessment.md` §As Built.

- [ ] **Native SIMD: float-to-int casts convert one lane at a time**

  Problem: a vector float-to-int cast extracts each lane, saturates it
  through `llvm.fptosi.sat.i128.f64`, and re-inserts it.
  - The vector form (`llvm.fptosi.sat.v{N}i128.v{N}f64`) is legal IR, but
    its x86-64 legalization is unverified, so it was not adopted.
  - Probe it with `llc` before switching.

- [ ] **Native SIMD: signed-zero min/max is unspecified**

  Not to be fixed: float `reduce_min`/`reduce_max` fold through
  `llvm.minnum`/`llvm.maxnum`, which is the VM's `f64::min`/`max`, and
  neither side specifies the result of `min(-0.0, +0.0)`.
  - Fixtures avoid mixed-sign zeros in min/max reductions.
  - Revisit only if upstream pins a rule.

- [ ] **Native SIMD: the lane-index trap has no category of its own**

  Problem: an out-of-range lane index exits natively with the
  `unhandled error` category, while the VM raises `SIMD lane index … out
  of range`.
  - A dedicated trap category is a `mojito-runtime` ABI bump.
  - Until then the parity harness cannot map the VM error, so the trap is
    pinned by `tests/pliron_opt_regression_test.rs` instead of an
    `assets/runtime_error` fixture.

- [ ] **Native SIMD: vectorization evidence is object code, not IR**

  Not a defect, a testing rule: vectors wider than a register legalize by
  splitting, so legal vector IR proves nothing about the selected code.
  - `simd_lowering_emits_vector_code` inspects the `llvm-objdump` listing
    and is the evidence to keep green.

- [ ] **Front end: a bare literal cannot build a multi-lane SIMD field**

  Problem: `P(1)` for a struct whose field is `SIMD[DType.int32, 4]` is
  rejected with a field type mismatch.
  - Upstream accepts it through the implicit `SIMD(IntLiteral)`
    initializer.
  - Spell `P(SIMD[DType.int32, 4](1))` until then.

- [ ] **Front end: no field writes through a `List` subscript**

  Problem: `xs[i].field = v` fails MIR verification (`dynamic element
  projection requires checked indexed storage`) for every element type.
  - Copy the element out, mutate it, and write it back (`xs[i] = e`), or
    mutate through a `mut` parameter, which works.

- [ ] **Checker: `Pointer(to=<temporary>)` is unsupported**

  Not planned: `View(Pointer(to=make_list()), 0)` reports `Pointer(to=...)
  requires a place expression`; a temporary bound to an explicit
  `ref[Self.o]` constructor parameter covers the same fixture point
  (`assets/ok/pointer_field_ctor_temporary_explicit_init.mojo`).

- [ ] **Native generic-holder temporaries with heap-owning implicitly
  copyable fields read freed memory**

  Problem: `GenericHolder[Box](Box(5))`, or Dict's
  `DictEntry[Optional[Int], V]` appended to a `List`, reads freed memory
  natively while the VM is correct.
  - This keeps `Dict[Optional[Int], _]` VM-only:
    `conformance/fixtures/optional_dict_keys.mojo` stays out of
    `assets/ok`.
  - Bisect the `var` field-move of the copy-lifecycle value inside the
    generic constructor against the owned-temp marking in
    `crates/mojito-pliron/src/lower/calls.rs`.

- [ ] **Native converting-constructor defaults are rejected**

  Problem: a `CheckedConst::Construct` default such as
  `arg: Optional[T] = None` is rejected at the native default-fill sites
  (`checked_const_value` errors on `Construct`).
  - Emit the constructor call at default-fill through `lower_call`.
  - The `NoneType` argument is `LowerTy::ZeroSized`.

### 2. Catch Up To Current Mojo *(recurring — reopens at every nightly re-pin)*

- When the pinned nightly moves: re-pin [`docs/mojo-nightly.md`](mojo-nightly.md),
  re-probe [`conformance/parity.tsv`](../conformance/parity.tsv), and burn
  down the divergences (`parity.tsv` notes, and the `mojito-only` /
  `mojo-only` rows of `conformance/cases.tsv`).
- Rule: Mojito matches or subsets Mojo. An extension is admitted only when
  it tracks an announced upstream direction (today: direct `ref` struct
  fields), is listed here, keeps its fixtures under `assets/extensions/`,
  and is re-probed at every re-pin.
- The `a79fbdf59f2` pass (2026-08-26, Mojo `1.1.0.dev2026082605`) is
  complete (`docs/mojo-nightly.md`). The next re-pin recreates this
  section's checkbox.

- [ ] **Behavioral divergences from the pinned Mojo — burn to zero**
  *(standing)*

  Every new divergence lands here with a probe or a `cases.tsv`
  `mojito-only` / `output-diff` row, and leaves when its probe promotes to
  an `assets/ok` fixture.

  Open today:
  - `partial-field-move-parent-used`: moving one field out of a struct
    (`p.a^`) while the parent is used afterwards (`p.b`) is rejected
    upstream (`value 'p.a' cannot be consumed, because 'p' is used later`)
    but accepted and tracked field-wise by Mojito. The Mojito behavior is
    pinned by
    `tests/drops_test.rs::partially_moved_field_is_dropped_once_at_its_new_owner`.
  - `symbolic-origin-pointer-write`: a write through a pointer field whose
    origin binder has symbolic mutability (`Origin[mut=m]`, or a bare
    `Origin`) is accepted however the binder was bound, while upstream
    judges the binder per instantiation and rejects the write from an
    immutable place (`expression must be mutable in assignment`). Origin
    arguments are erased from checked identity, so no per-instance binding
    reaches `check_pointer_write`; carrying it there is the fix.
  - `simd-subscript-indexer`: Mojito normalizes any `Indexer` through
    `__mlir_index__` at every subscript, so a `SIMD` lane accepts one,
    while upstream's `SIMD.__getitem__` takes a plain `Int` and rejects it.
    A user type's own `__getitem__` does take an `Indexer` upstream
    (`assets/ok/indexer_normalization.mojo`), so only the builtin SIMD
    subscript diverges.

  Retained on purpose, re-probed at each re-pin:
  - `subtree-origin-cast`: a cited bridge to upstream's
    `#lit.origin.subtree` experiment.
  - `reference-valued-aggregate`: the `ref` field extension, kept on
    purpose (next task).
  - `deinit-body-field-loop-read` (`output-diff`): the pinned Mojo destroys
    a `deinit self` field that is read only inside a loop body at the
    destructor's entry and then reads the destroyed value. That is an
    upstream bug Mojito does not reproduce.

- [ ] **Direct `ref` struct fields stay a Mojito extension** *(kept on
  purpose)*

  Not to be removed: upstream rejects `var f: ref[o] T` fields (`'ref'
  patterns are only valid on the left side of an assignment`) and spells
  reference storage through `Pointer[T, origin]`, but upstream has signalled
  that `ref` fields may arrive, so Mojito keeps the capability
  (2026-09-09 decision; the stdlib's six `ref`-field structs stay as they
  are).
  - Fixtures that use `ref` fields live under `assets/extensions/<folder>/`;
    the ordinary `assets/` folders hold only programs the pinned Mojo
    compiles (`assets/README.md`).
  - Where a `ref`-field fixture has a Mojo-valid twin, the twin spells the
    storage through `Pointer[T, origin]` under the same name with
    "ref_field" replaced by "pointer_field" in the ordinary folder.
  - `conformance/fixtures/reference_valued_aggregate.mojo` stays a
    `mojito-only` row until upstream decides.
  - If upstream lands `ref` fields, re-probe the extension fixtures against
    the new spelling and promote them back.

- [ ] **Mojito-specific shortcuts to move toward Mojo's shape** *(standing,
  any order)*

  Problem: parts of Mojito's stdlib lean on the Rust runtime where upstream
  is pure Mojo. Each is a candidate port, preferred over any new bridge
  (2026-09-07 direction).
  - The literal-filled `String.__init__(literal)` and
    `StringSpan.__init__(literal)` constructors, and the
    `String._as_string_literal()` struct-to-literal bridge behind
    `String.write_to`. Upstream: `StringLiteral` is a `StaticString` and
    `write_to` writes the bytes.
  - Number formatting in Rust: the VM's `Display for Value` and the native
    `mjrt_fmt_i64` / `mjrt_fmt_u64` / `mjrt_fmt_f64`. Upstream formats
    `Int` and `Float64` in Mojo (`_write_int`, Dragonbox in
    `format_float`).
  - `mjrt_repr_string` for the native string repr. The VM side is already
    Mojo (`String.write_repr_to`).
  - `mjrt_pow` for integer `**`. Upstream's `Int.__pow__` is Mojo.

  Kept as runtime services because upstream has the same boundary:
  `_mojito_abort` (`os.abort`), `mjrt_read_line` (`input`), allocation,
  and traps.

### 3. Grow The CPU Standard Library *(demand-first)*

- [ ] **Collection API parity**

  Goal: grow the tuple, slice, optional/variant, and String surfaces toward
  the audited head (`docs/features.md` records what lands). The tasks below
  are in impact order: soundness of the executable oracle first, then
  everyday spellings that reject today, then parity details. No task
  depends on a later one. Every bullet is a conscious, recorded limit, a
  task closes when its bullets are done, and a residue found inside a task
  moves to the task that owns its fix.

  1. **Temporary views and their origins** — the one cluster where the VM
     oracle can be unsound.
     - Destructuring a returned `Tuple` of origin-bearing views
       (`var a, b = s.split_at_grapheme(n)`) reports `use after Pointer
       deallocation` on the VM. A bound pair read by index (`pair[0]`,
       `pair[1]`) and a returned tuple literal of views both run.
     - `split_at_grapheme` returns the receiver's origin where upstream
       returns `ImmOrigin` views.
     - An overloaded free def over a temporary view argument
       (`reversed(StringSpan(s))`) frees the view before the loop runs.
       Bind the view first, or call `graphemes_reversed()`.
     - `value()` on a temporary `Optional` holding a view
       (`it.peek_next().value()`) is rejected as a reference to a
       non-place. Bind the Optional first.
     - A `String` copied from a temporary view of a local at the local's
       last use inside a `return` expression or a self-assignment
       (`return String(head.rstrip("/"))`,
       `head = String(head.rstrip("/"))`) frees the local before the copy
       reads it (`conformance/probes/view_copy_return_use_after_free.mojo`).
       Bind the copy to a local first, as `std/os/path/path.mojo` does.
     - Not to be fixed: casting a tracked pointer to an untracked origin at
       its source's last use
       (`s.unsafe_ptr().unsafe_origin_cast[ImmUntrackedOrigin]()`) frees
       the source before the pointer is read. Upstream does the same.

  2. **Compile-time evaluation residues** — what VM CTFE can bind and
     resolve.
     - A VM-evaluated compile-time expression whose result is
       pointer-backed (`comptime C = M.copy()`, a bare `Optional`, a
       `String`) cannot cross back (`cannot cross back from VM CTFE`).
       Upstream binds it. The freezable results are scalars, Bool, String,
       tuples, fieldwise structs, and displays.
     - Compile-time Dict/Set key identity is structural `CtValue`
       equality. That is exact for every prelude key type, and diverges for
       a user-struct key with a non-fieldwise `__eq__`.
     - The typing probe checks the CTFE subprogram once more per VM-bound
       expression.
     - A non-parameterized struct alias mentioning a value parameter
       (`comptime Alias = S[Self.T, Self.length]`) resolved through an
       instance (`S[Int, 3].Alias`) keeps `length` symbolic (`expected
       S[Int, length], found S[Int, 3]`). `associated_type_from_base`
       substitutes type parameters only; parameterized aliases substitute
       both.

  3. **Monomorphization coverage and cost** — where the erased path still
     stands in for an instance clone, and what minting costs.
     - An instantiation whose argument mentions `StringLiteral` (`{"a": 1}`
       is `Dict[StringLiteral, Int]`) keeps the erased path: its values keep
       the literal runtime representation while an un-annotated binding
       materializes `String`. Lifting it needs one runtime representation
       for `StringLiteral` and `String`, or typing the display as
       `Dict[String, Int]` as Mojo does. `StringDict.__getitem__` stays a
       `Copyable`-guarded copy for the same reason: its
       `StringLiteral`-keyed entry list keeps the erased path, where a
       reference result cannot spell its interior origin.
     - A call inside an unstamped bundled body on a bundled struct's
       generic method keeps the erased path. Requests are admitted from
       user code, clone bodies, variadic specs, instances, and user
       structs.
     - An instance clone whose walk cannot resolve an application (a
       variadic template over a nested public `Tuple` argument) is dropped
       to the erased path rather than failing the program.
     - Value-parameterized structs get no instance clones, so an erased
       body's `_unqualified_type_name[Self.T]()` spells `T` (`repr([1, 2])`
       prints `Array[T, 2]([Int(1), Int(2)])` where upstream prints the
       element type). A `Self.n` bracket argument inside such a body
       (`Counter[Self.length](i)`) is VM-only: native monomorphization
       needs a compile-time-constant value argument (`unsupported value
       parameter 'length' is not compile-time constant`).
     - Clones are minted per whole instance with no reachability pruning
       and re-checked each discovery round. `benchmarks/compile/stdlib_heavy`
       is about 2.2x its pre-clone baseline in release
       (`docs/performance.md`), and the repr methods added 2026-09-05 cost
       about 5% in debug across the compile benchmarks. Lever:
       reachability-pruned minting.

  4. **Variadic packs and tuples** — what the pack machinery types
     syntactically or refuses.
     - Type-pack calls inside a nested `def` and whole-pack-forwarded calls
       keep the syntactic element-typing path (`a heterogeneous pack
       specialization needs an expression whose type is statically evident
       before checking` for `take(b, 1)` over a local `b`). Top-level calls
       consult the checker's instantiation. Lifting it needs four pieces:
       nested templates live only in `NestedMono` and are deleted by
       `replace_templates` before the checker sees them, the checker would
       have to accept a nested variadic shell abstractly,
       `def_specialization_requests` is top-level only, and
       `Mono.def_call_targets` must reach `NestedMono::scan_expression`
       with a `def_request_target` fallback.
     - A user variadic struct application as a pack element
       (`Tuple[TypeNames[Int]]`) keeps the fixed-arity diagnostic. Its
       erased shell has no sound nominal form; the public `Tuple` is the
       one compiler-known template whose `*Ts` absorbs every argument.
     - A public Tuple as an explicit type argument does not conform to
       `Deinitable` (`make[T: Defaultable & Deinitable]()` over
       `Tuple[Int, Bool]` reports the bound failure), while the inferred
       shape runs.
     - A standalone nullary vector construction `SIMD[d, w]()` rejects
       (`SIMD construction expects w element(s) or 1 to splat, got 0`).
       Only a Tuple element defaults to zero lanes.

  5. **Everyday spellings that still reject** — checker context and stdlib
     API shapes.
     - A list display as an argument to an explicitly applied constructor
       at runtime (`Dict[String, Int](["a"], [1], None)`) takes no context
       from the parameterized `List[Self.K]` parameter and materializes as
       `Array`, so no overload matches. Spell the lists
       (`List[String]("a", __list_literal__=None)`), as a compile-time
       value's materialization does.
     - A struct's own value parameter as a SIMD width in a signature
       (`def zeros(self) -> SIMD[DType.int64, Self.length]`) reports `SIMD
       width must be a positive power of two, got a type`. The width slot
       needs a concrete comptime `Int`, where upstream binds the parameter.
     - String literals have no methods (`"abc".byte_length()` rejects). A
       literal-typed value converts through `String(...)` first.
     - An annotated `Optional[T]` local initialized from a bare struct
       construction (`var o: Optional[P] = P(4)`) fails MIR verification
       with an untyped register for the constructor call. Spell the wrapper
       (`Optional[P](P(4))`, `Optional(P(4))`); a literal payload
       (`var n: Optional[Int] = 6`) converts.
     - `if`/`while` accept a width-1 bool lane through the `Bool(x)`
       truthiness conversion, but `and`/`or` still demand `Bool` operands
       (`a == b or c < d` over `UInt64`s rejects). Nest the tests.
     - A string literal does not bind a trait-bounded type parameter the
       nominal `String` satisfies (`isdir("/tmp")` reports `'StringLiteral'
       ... does not conform to trait 'PathLike'`). Spell `String("/tmp")`.
     - Struct-level `comptime NAME = Self(n)` constants (upstream's
       `ErrNo.ENOENT`) do not fold. The associated-constant evaluator
       handles prefix, infix, tuple, and list expressions only.
     - `@fieldwise_init` beside a hand-written `__init__` rejects. Spell
       every constructor by hand.
     - A bound-generic def whose template body calls itself with a concrete
       argument (`makedirs(head, exist_ok=...)` inside `makedirs[PathLike]`)
       mints its own specialization while checking and reports it
       undefined. Recurse through a non-generic helper.
     - Nullary construction of a sized-scalar array (`Array[Int8, 1024]()`)
       reports the scalar's `Defaultable` construction unsupported. Spell
       `Array[Int8, 1024](fill=0)`.
     - `Pointer.unsafe_bitcast[U]()` is not typed: an origin-cast-style
       forwarding leaves the MIR register at the old element type.
     - `CStringSlice` views `Byte` elements instead of upstream's `c_char`.
     - No `std.builtin.rebind.downcast`: `Dict`/`Set` guard `keys`,
       `values`, `items`, and `__iter__` with `where conforms_to(K,
       Copyable)` instead of laundering the parameter. `Dict.keys` needs
       copyable values as well as keys, since the key view wraps the entry
       view.

  6. **Naming and Unicode details** — output text only.
     - `_unqualified_type_name` spells nested structs unqualified where
       upstream keeps a non-prelude struct's module path
       (`Optional[std.collections.dict.Dict[...]]`, `List[up.Flag[True]]`).
       `List`, `Optional`, `String`, and `SIMD` stay bare.
     - Grapheme segmentation is the documented UAX #29 essentials subset:
       hand-maintained Control/Extend/SpacingMark ranges, no
       Extended_Pictographic or Prepend data. Reverse iteration re-scans
       forward from the nearest CR/LF/Control boundary.

- [ ] **Filesystem and I/O residues**

  Behind the landed files, streams, paths, and tempfile stage
  (`docs/features.md`).

  1. The deferred tail, each a small port:
     - public `stat` / `lstat` / `stat_result`, `realpath`, `symlink` /
       `link` / `chdir`, and `isatty` (`FileDescriptor.isatty` / `fchdir`);
     - `OptionalPointer` (null tests spell `Int(ptr) == 0`);
     - `ErrNo`'s named constants;
     - `~user` expansion (`getpwnam`);
     - the VM's per-process environment overlay (native `setenv` writes the
       real environment);
     - `NamedTemporaryFile`;
     - `FileHandle.read` into a typed `Span[Scalar[dtype], origin]`;
     - `Path.stat` / `lstat` / `_dir_of_current_file`;
     - `KeyElement`;
     - `_get_random_name` over `std.random` (reads `/dev/urandom` today);
     - a generic `__exit__[E]`.
  2. Traps the stage worked around, each its own fix:
     - An overloaded constructor spelled with the `StringSlice` or `Byte`
       alias mangles a different key than the `StringSpan` / `UInt8` the
       call selects. The ports spell the canonical names.
     - A temporary view as a method argument
       (`s.take(String("x").as_bytes())`) is a VM "reference receiver must
       be a place" rejection, and as an augmented-assignment operand
       (`p /= StringSpan(s)`) a VM use-after-free.
     - `Span[mut=True, T, _]` fails origin inference. Spell an
       `[origin: Origin[mut=True]]` binder.
     - The two VM destruction-order divergences the stage found live in the
       behavioral-divergences task of section 2.
     - `range` is invisible in `std.string`.
     - A module loaded while the prelude bootstraps (`std.io` and the whole
       `std.os` graph now) must import `String` explicitly. That graph
       costs Hello World about a second of debug compile time
       (`docs/performance.md`).

- [ ] **Time, random, and testing slices**

  Goal: deterministic testable cores, with host-dependent behavior behind
  runtime services.

### 4. Packaging, Artifacts, And Developer Tooling *(any order unless noted)*

- [ ] **Compile-time performance**

  Problem: Hello World is 0.8 s release / 4.6 s debug
  (`docs/performance.md`; it was 24 s / 52 s before the shared
  `Arc<CheckedTables>`). Next, in order:
  1. Avoid the redundant checker passes. Hello World re-elaborates and
     re-checks once because the request scan always finds the prelude's own
     Tuple/def requests, and every check runs two transfer rounds.
  2. `checked_var_types` scans the whole expression table per variable, and
     `explicit_destroy` re-derives deinitability per struct per pass.
  3. Only then cache the elaborated/checked stdlib across processes.

- [ ] **Feature and target options**

  Goal: checked CLI/build configuration recorded in artifacts and
  diagnostics.

- [ ] **Compiled package artifacts**

  Goal: a versioned `.mojoc` representation (modules stay non-first-class).
  Per-directory resolution order:
  1. source package
  2. `.mojoc`
  3. source module
  4. legacy `.mojopkg`

- [ ] **Debugging metadata and inspection**

  Goal: stack/source diagnostics, MIR inspection, and debugger-oriented
  value rendering.

- [ ] **Testing tools**

  Goal: Mojito-native assertions, expected-error tests, and
  differential-harness integration.

- [ ] **Distribution reproducibility gate** *(last)*

  Goal: the release check rebuilds, tests, documents, and reproduces
  conformance from the crates.io archive alone.

### 5. Code Organization Follow-Ups *(any order — behavior-preserving)*

The 2026-09 module split (`docs/symbol-map.md`) removed every file over
3,000 lines. What remains needs semantic extraction, not line moves.

- [ ] **Split `expr_unconverted`**

  `mir/lower_expr/expr.rs` (about 2,090 lines) is one match over
  `ExprKind`.
  - Extract arm groups into `Flatten` methods.

- [ ] **Split `infer_method_call`**

  `checker/method_calls/mc_infer.rs` (about 1,530 lines) is one method.
  - Extract receiver-family branches beside `selection`, `statics`, and
    `builtin_types`.

- [ ] **Split `verify_instruction`**

  `mir/verify/instr.rs` (about 1,320 lines) is one match over `MirInstr`.
  - Extract per-family check helpers.

- [ ] **Shrink the 2 kloc band**

  Split these further only along a cohesive seam, while touching them:
  - `checker/traits.rs` (2,629), `mir/lower_stmt.rs` (2,595),
    `checker/inference.rs` (2,520), `checker/statements.rs` (2,471),
    `ast.rs` (2,464), `mir.rs` (2,426), `checker/type_resolution.rs`
    (2,395), `runtime.rs` (2,235), `checker.rs` (2,232),
    `comptime/rewrite.rs` (2,179), `checker/declarations.rs` (2,118),
    `mir/text/write.rs` (2,116), `backend/vm/exec.rs` (2,085).

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

## Entry Style

Every entry is written for a human reader who has not seen the code.

- One problem per checkbox. Its first sentence states the issue to fix, or
  says the item is not to be fixed and why.
- Details follow as short bullet points: the symptom, the workaround, the
  cause or the lever, and the file or test that pins it.
- No run-on sentences and no semicolon chains. If a thought needs a
  semicolon, it is two bullets.
- Code spellings appear only where the reader must go look
  (a file, a test, a diagnostic text, an example line).
- A residue list from a finished task becomes several checkboxes, not one
  paragraph.

## Working Rule

1. Start with a self-hosted library or small user-facing acceptance case.
2. Record the current failure with a focused test.
3. Implement the smallest compiler change that makes the program honest.
4. Add positive and negative coverage at the owning compiler phase.
5. Run `scripts/check` before marking the task complete.
