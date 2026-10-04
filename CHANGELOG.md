# Changelog

All notable changes to Mojito will be documented in this file. The project uses
Semantic Versioning while its public Rust API and supported Mojo subset continue
to evolve under the `0.x` compatibility rules.

## [Unreleased]

### Added

- A uniquely named `def` keyed on a `DType` binder, on a parameter used as a
  lane width, or on a layout operand (`lane[dt: DType]` constructing
  `Scalar[dt](v)`, `wide[w: Int]` constructing `SIMD[DType.int32, w](v)`,
  `bits[dt: DType, w: Int](v: SIMD[dt, w])`) is served by its template
  instead of being cloned per call: the checker records every lane fact —
  a construction's dimensions, a cast, a `to_bits`, `v.length`, `v.dtype` —
  as the `SimdDtype`/`SimdWidth` slots themselves, MIR carries the slots in
  its register types and in `MakeSimd`, `SimdCast`, and `SimdBitcast`
  (textual schema 1.18, `ct_expr(...)` in their `dtype` and `width` fields),
  a symbolic lane count or dtype read lowers to the evaluation of its
  parameter expression, a literal beside a symbolic lane materializes to
  it, a `Scalar[dt]` splats into a `SIMD[dt, w]`, a `comptime if` over the
  binder (`dt.is_integral()`) reads the binder's slot in its thunk, and the
  elaborator binds `dt` and `w` from the call's recorded arguments or a
  vector argument's own slots and closes every slot per instance, on the VM
  and natively (`assets/ok/lane_template_served.mojo`,
  `assets/ok/template_value_keyed_lane_def.mojo`,
  `assets/ok/simd_symbolic_surface.mojo`). A `def` whose body shuffles,
  slices, joins, or hashes a lane value, queries a float format over its
  binder, binds a local `comptime`, or holds a nested `def`, and every
  overloaded lane-keyed `def`, still clone per call (roadmap R257–R261); a struct keyed on a lane and a
  method with a lane binder of its own wait on R4 and R5.
- A pack-keyed `def` that spreads its pack whole into `print` or into
  another served `def` (`show(*a)`, `tally(10, *rest)`, `drain(*items^)`)
  is served by its template: MIR carries the call with the collector as its
  spread argument (`MirInstr::Call::spread`, textual schema 1.17), the
  ownership analysis sees the collector lent or moved whole, and the
  elaborator replaces the argument with the bound pack's element places
  before binding the callee, on the VM and natively; the erased oracle
  splices the collector at the call. A `def` is served when every callee it
  spreads into is, by fixpoint, and a clone that spreads its bound pack into
  a served `def` passes the elements itself (`assets/ok/pack_template_served.mojo`'s
  `relay` and `forward_owned`, `assets/ok/pack_forwarding_print.mojo`,
  `assets/ok/pack_argument_destroyed_after_call.mojo`,
  `assets/ok/pack_overload_sibling_forward.mojo`).
- A pack-keyed `def` with an owned `var` collector is served by its
  template, as one with a read collector is: the body is checked once with
  the collector a `VariadicPack` of the symbolic `Ts`, each element is read
  in place, and the pack is destroyed last to first after its last element
  use, on the VM and natively (`assets/ok/pack_template_served.mojo`'s
  `drain`, `assets/ok/owned_pack_destroyed_in_reverse.mojo`). An element
  transferred out by subscript (`a[i]^`) is rejected, as the pin rejects it
  (`assets/type_error/owned_pack_element_transfer.mojo`).
- A `def` keyed on a type pack is served by its template when its collector
  is read and its body neither spreads the pack nor binds an element's type
  under its loop: the checker types the body once with the collector a pack
  of the symbolic `Ts`, each `args[i]` the dependent `Ts[i]`, and the pack's
  length (`args.__len__()`, `Ts.length`, `len(Ts)`) a parameter expression
  bounding the `comptime for`; MIR carries the pack binder and the loop as
  themselves and records the elements the checker solved at each call; and
  the elaborator binds the pack from the call, unrolls the loop, and folds
  each element's type, for the native backend as for the VM
  (`assets/ok/pack_template_served.mojo`). The erased oracle reads a pack's
  length from its collector's arity or the call's recorded elements.
  `len(args)` as a `comptime for` bound is rejected, as the pin rejects it
  (`assets/type_error/comptime_for_runtime_pack_length.mojo`). The spreads
  and the element-typed binding keep their clone (roadmap R251–R252).
- A generic `def` holding a `comptime for` over a `range` is served by its
  template, so the loop crosses the MIR waist as itself: the checker types
  the body once with the index a symbolic `Int` binder of the loop's own and
  records the range's bounds as parameter expressions, HIR and MIR lower the
  loop with the new `ComptimeFor` header (MIR text schema 1.16,
  `comptime_for`), the ownership analysis and drop elaboration decide it as
  a loop with its trip count unknown — so a move in a `range(0)` body with a
  use after the loop and a move in a `range(1)` body without a refill now
  reject as the pin rejects them, and a compile-time `break` and `continue`
  are the loop's, destroying the iteration's values where the pin does —
  and `native::mono` unrolls it under each instance's bindings before
  substitution: one copy of the body per index value with fresh registers,
  the index folded, the loops nested in the copy unrolled and the `comptime
  if`s over the index decided per copy, and the copies chained where the
  loop stood. Arithmetic bounds, a loop nested over the outer index, a value
  parameter applied to the index, a loop inside a `try` region, two loops of
  one name, and a loop-local `var` per iteration run on one template, which
  also closes the per-iteration scope defect of a generic `def`'s loop. The
  erased oracle runs a kept loop from its index slot. The cloner keeps only
  the loops the template does not serve — over a list, a pack, a reflection
  query, or with a local `comptime` binding in the body — and the AST
  unroller now refuses a compile-time `break` or `continue` it would splice
  into the wrong loop.
- A `def` holding a `comptime if` is served by its template, the first
  compile-time construct to cross the MIR waist as itself: the checker types
  every arm with the binders symbolic and records each condition it compiles
  as a constraint, HIR and MIR lower the region as the `if` diamond with the
  new `ComptimeBranch` terminator (MIR text schema 1.15, `comptime_branch`),
  the ownership analysis and drop elaboration decide it as the `if` of the
  same shape — so a move in an untaken arm with a use after the join, an
  untaken arm's read of a moved value, and an unused declaration that moves
  now reject as the pin rejects them, and a value one arm alone consumes is
  destroyed at the other arm's entry — and `native::mono` decides the
  condition under each instance's bindings, keeps the taken arm with its
  drops, and prunes the other before any call in it is enqueued. The
  cloner's `comptime if` class, its trap stub, and the `--comptime-regions
  keep` experiment are gone; a scalar value parameter an application binds
  is served by the template too. The compile-time request path landed with
  it: a condition that applies a function is a thunk the elaborator demands
  — every pending instance materialized, the completed output verified as
  the fragment `VmBackend::call_concrete` runs, an effectful callee refused,
  fuel one counter per compilation, the result cached by instance, a demand
  on an instance being materialized the pin's parameter-domain cycle — and a
  module constant whose initializer is `size_of[Pair]()` stays the
  application through the check and is answered by the elaborator under the
  target (`assets/ok/comptime_if_template_served.mojo`,
  `comptime_if_condition_applies_def.mojo`, `comptime_layout_constant.mojo`;
  `assets/ownership_error/comptime_if_untaken_*.mojo`,
  `assets/type_error/comptime_layout_constant_mismatch.mojo`;
  `conformance/probes/p3_mixed.mojo`). `mojito-native` now depends on
  `mojito-vm`.

- Register types over parameter expressions are specified
  (`docs/notes/param-expr-attributes.md` §Register types): a generator's
  MIR names a type binder, a dependent type, a symbolic vector lane or
  width, a symbolic struct argument, or the new compile-time application
  `ParamKind::Apply` in a register type, and `mir::verify` now checks that
  every binder such a type names is declared by the body's declaration, its
  struct, an enclosing callable signature, or a contract, and that every
  expression is well-kinded for its slot (`verify/scope.rs`), while the
  concrete mode rejects them all. A module constant whose initializer
  applies a function is its application in a type, as at the pin (decision
  D3): `SIMD[DType.float32, C]` over `comptime C = f(A) * 2` matches itself
  on both sides of a call and rejects `SIMD[DType.float32, 8]`
  (`assets/ok/comptime_applied_constant_in_signature.mojo`,
  `assets/type_error/comptime_applied_constant_mismatch.mojo`). Layout is
  asked only of a concrete type (`LayoutError::Symbolic`), and the
  elaborator answers every `size_of` under the compilation's native target
  — the host for a VM run, `--target` for a native compile — so concrete
  MIR holds the constant on both backends. MIR text schema 1.14 carries
  `param_apply`, and reads back a pack element and a reflection query.

- `docs/notes/comptime-region-ownership.md` records the ownership rule for
  a compile-time region: a `comptime if` and a `comptime for` are decided as
  the runtime region of the same shape, with the condition opaque and the
  trip count unknown, which upstream's lifetime check does on its structured
  compile-time ops and which nineteen pin probes
  (`conformance/probes/comptime_region_*.mojo`) observe. `mojito run
  --comptime-regions keep` takes a program's own regions through that path
  as an experiment: every arm and body is kept through the check and the
  ownership analysis, and the elaborator folds the literal branches after
  drop elaboration (`native::mono::fold_literal_branches`). Every probe's
  verdict and output then matches the pin's. The production path still
  selects the arm before the move analysis, so five probes Mojito runs where
  the pin rejects are now `assets/extensions/ownership_ok/` fixtures on the
  roadmap ledger, and the probes found that a value live after a region is
  never destroyed when the region raises out of the function, also filed.
- `benchmarks/compile/keyed.mojo` calls `def`s keyed by a `comptime if`, a
  `comptime for`, and a type pack, user and bundled, at several
  instantiations, plus a generic struct method holding a `comptime if`: the
  bodies stages P3a and P3b of the parametric-MIR plan move, which the three
  P0 programs never mint. Its instantiation census and debug timing sit
  beside the P0 baseline in `docs/parametric-mir-plan.md` §P0 and
  `docs/performance.md`.
- The elaborator decides a struct member's `where` clause in every form the
  checker compiles: an `IsTrivially*` predicate, a value comparison
  (`where Self.n > 2`), and a `TypeList` query over a bound pack, besides
  `conforms_to`. The checker gives each struct a trivial-lifecycle row per
  `IsTrivially*` spelling the program names, so `MaybeUninit`'s members and
  a conditional copy, move, or destroy member are no longer materialized on
  the checker's word. `IsTrivially*[Self.T]` in a member's clause names the
  struct's binder, where it used to be judged false at every call.
  `assets/ok/conditional_member_trivial_value_clauses.mojo` pins it.
- Every closed call of a plain trait-bound generic `def` (`show(n)`,
  `show[Int](n)`, `bytes[Int]()`) is served by its template and mints no
  clone: the elaborator instantiates the template's MIR for the VM and the
  native backend alike. The call carries the compile-time arguments the
  checker solved (`MirInstr::Call::instantiated_args`, text schema 1.13),
  so a type parameter no runtime parameter or result spells binds too, and
  the erased oracle reifies a struct's name from them. A `def` with a value
  parameter, a compile-time construct held or reached, a struct specialized
  whole over its parameters, or an overloaded name keeps its clone. Along
  the way a bound dispatch of an overloaded method with binders of its own
  selects the receiver's overload, a comparison through a bound borrows a
  named operand where it lies (no copy constructor runs), and a `for` over a
  bound reads its exhaustion type from the bound's associated iterator.
  `assets/ok/template_served_def_closed_call.mojo` pins it; three probes
  that now run are promoted (`overloaded_hash_through_hash`,
  `generic_def_static_receiver_binder`,
  `keyed_def_builds_loan_carrying_instance`). The `BoundedOperations`
  certificate class served nothing afterwards and is deleted.
- A generic `def` that names an associated type only in its body
  (`def count[C: Iterable](items: C) -> Int` with `for item in items`) is
  served by its template at a loan-carrying argument and mints no clone. A
  struct's MIR declaration carries its unparameterized associated types
  (`MirStructDeclaration.associated_types`, text schema 1.12), and the
  elaborator reads `C.Element` off the struct `C` is bound to, for the VM and
  the native backend alike.
  `assets/extensions/ok/template_served_iterable_def.mojo` pins it.
- A method that spells a type name over its struct's parameters
  (`_unqualified_type_name[Self]()`, the `write_repr_to` of `List`, `Dict`,
  `Set`, `Array`, and `Optional`) is served by its template and mints no
  clone. The template's MIR carries the type in a new `type.name`
  instruction (`MirInstr::TypeName`, text schema 1.11), and the elaborator
  writes the name from the substituted type as a string constant, for the VM
  and the native backend alike. The erased oracle spells the parameters as
  written (`Box[T]`), so four fixtures leave its comparison
  (`docs/roadmap.md` 1.22).
- A generic `def` called at a loan-carrying argument (`keep(m, p)` with
  `p: Pointer[List[Int], ImmOrigin(origin_of(xs))]`) is served by its
  template and mints no clone. The call closes the stored type in the
  `def`'s transfer summary with the types it binds to the `def`'s own
  parameters, and a method's own parameters close the same way. A `def`
  whose body holds or reaches a compile-time construct, or names an
  associated type its signature does not spell, keeps its clone.
  `assets/ok/template_served_def_loan_carrying_argument.mojo` pins it.
- A generic struct instance over a loan-carrying argument
  (`Bag[Pointer[List[Int], ImmOrigin(origin_of(xs))]]`,
  `List[Span[Int, origin_of(xs)]]`) is served by its template's methods and
  clones only the methods a plain-data instance clones. A template's store of
  a value of a parameter type publishes the stored type as its transfer
  summary's source (`SigOrigin::Carried`, `sig_carried(type)` in text schema
  1.10), and a call closes it with the receiver's arguments. Such an instance
  no longer fails on a member it never calls (`write_to` over a pointer
  element).
- `--instantiation-census` reports a clone that keeps a parameter of its own
  apart from the templates no clone replaces
  (`instantiation.cloned_parametric.*`), so the `instantiation.erased.*`
  rows count erased templates only.
- A struct member's `where` clause reaches MIR
  (`MirFunctionDeclaration.availability`), with each struct's conformance
  rows (`MirStructDeclaration.conformances`) and the declared traits
  (`MirDeclarations.traits`). The elaborator decides the clause under an
  instance's bindings instead of trying every constructor and dropping the
  ones that fail: a member its instance disproves is skipped where struct
  discovery reaches it and is an error where a call demands it, and an
  instance that fails to materialize fails the compilation. The text schema
  is 1.9; an older artifact reads as carrying none.
  `assets/ok/conditional_member_availability.mojo` pins it.
- `T()` over a parameter bound to a SIMD type builds the zero vector, so
  `Array[c_char, 4]()` runs on the VM and natively
  (`assets/ok/simd_parameter_default_construction.mojo`). The eager
  constructor walk used to hide that it could not.
- A method of an ordinary generic struct with no compile-time construct in
  its body mints no clone per instance. The elaborator instantiates the
  template's MIR for both backends, lifecycle members and constructors
  included, and nothing is checked or derived for the instance.
  `stdlib_heavy.mojo` mints 255 bodies where it minted 666, and 11
  per-instantiation method clones where it minted 417. A method still clones
  where its body holds or reaches a compile-time construct, and an instance
  over a loan-carrying argument clones every method (roadmap section 1).
  `assets/ok/generic_struct_template_reach.mojo` pins what a template body
  reaches at an instance.
- MIR records the spelled receiver of a static call on a generic struct
  (`Pair[Self.U].count()`, `MirInstr::Call::receiver`), and the elaborator
  binds the struct's parameters from it. The text schema is 1.8; an older
  artifact reads as recording none.
- The driver reads off a template's checked types which instances a
  template-served body reaches, and which methods only an instance's own
  check can serve (`src/compiler/template_reach.rs`).

- The VM runs concrete MIR. `Compiler::execute` and `mojito exec` elaborate
  drop-elaborated MIR with `native::mono` from `main` and module
  initialization and run the result, as the native backend does, and the
  driver caches that graph (`CompiledProgram::concrete_mir`) so no backend
  elaborates twice. `mir::ConcreteMir` is the verified wrapper, and
  `native::mono::entry_roots` defines the roots. The erased path stays as the
  differential oracle: `--erased`, `MOJITO_VM_ERASED=1`, or
  `Compiler::with_vm_instantiation`. A body the elaborator refuses is
  `CompilerError::Elaborate`. Debug `run` is within 1% of the erased path on
  the three compile benchmarks, and peak memory falls by about 60 MB.
- The corpus binary's `specialized_vm::*` group is now `erased_vm::*`: the
  concrete run against the erased oracle for every `assets/ok` and
  `assets/runtime_error` program, comparing output or error text, ordered
  lifecycle events, and the serialized artifact's outcome.
- The native backend runs an evaluated default argument
  (`def f(s: String = String("a"))`, called as `f()`): the caller calls the
  lowered default function, a `var` parameter takes the value, and a
  borrowing parameter's value is released after the call. Monomorphization
  and native reachability follow the edge from a declaration to its default
  functions. `assets/ok/evaluated_default_argument.mojo` pins it on both
  backends. A borrowed default's user destructor, which neither backend
  runs, is roadmap 3.108, and a list-literal default is roadmap 3.109.
- `mir::verify` gains a concrete mode, `verify_concrete`, for elaborated MIR:
  it rejects every symbolic type, compile-time parameter, and erased-dispatch
  result adapter that parametric MIR may carry. `native::mono` verifies its
  output in that mode, and its own concreteness scan is gone. The
  compile-time argument slots a resolved call still lists are filed in
  roadmap section 1.
- The corpus binary gains a `specialized_vm::*` group: each `assets/ok`
  program's elaborated MIR is specialized by `native::mono`, must pass
  `mir::verify`, and must run on the VM to the erased program's outcome. It
  needs no LLVM. All 810 fixtures agree, and the expected-failure list
  (`SPECIALIZED_VM_RESIDUE`) is empty.
- `mojito run --instantiation-census FILE` reports which mechanism
  instantiates each generic body of a compilation: the bodies the AST cloner
  mints by class, the cloned bodies the checker infers and the ones it
  derives, and the parametric bodies left in MIR with the instances they
  serve. `--timings` carries the cloned and checked counts as
  `instantiation.*` counters, and `CompiledProgram::instantiation_census`
  returns the record. The table for Hello World, `generic.mojo`, and
  `stdlib_heavy.mojo`, with the debug `run` baseline for the plan's budget,
  is in `docs/parametric-mir-plan.md` §P0.

### Fixed

- A value read of a homogeneous collector's element (`var x = a[1]` inside
  `def h(*a: IC)`) copies that element alone, as the pin does: MIR loads the
  element place and runs the checked value copy on it, where it used to copy
  the whole collector and never destroy the extra copies. A field read
  through an element (`a[0].n`) copies nothing, on the VM and natively
  (`assets/ok/variadic_element_value_read.mojo`). `len(a)` still copies the
  collector (roadmap R263).

- Elaborating a deeply nested type no longer takes time quadratic in its
  depth: expanding polymorphic recursion (`depth(W[T](x.copy()), n - 1)`)
  reaches the 4096-instance budget after under 2 s of debug-build
  elaboration, where it took about 80 s, and the `Optional[T]` form after
  under 2 s, where it took about 140 s. Each struct type's shared argument
  list caches its hash and whether it is closed — holding no parameter,
  literal, callable, pointer, expression, or bound origin — so hashing a
  type visits one level and the symbolic, parametric, substitution,
  literal, origin, and callable-environment walks pass a closed level
  whole. The elaborator also keeps its layout field index and its output
  structs' positions as it declares them, looks up a method's
  per-instantiation clone only when the checker minted one, and remembers
  each concrete type's conformance to a trait.
- A nested instance type now holds the level below it once, so the
  elaborator's instance budget is back at 4096: expanding polymorphic
  recursion (`depth(W[T](x.copy()), n - 1)`) stops there within 380 MB in a
  debug build, where 1024 instances used to take 2.5 GB. A struct type's
  argument list is shared between its clones (`TyArgs`), and a walk that
  changes nothing keeps the list it read. An instance symbol whose argument
  spelling passes 96 characters names its arguments by digest
  (`W$mono$H…`), so a symbol's length is bounded at any nesting depth.
  Elaboration time still grows quadratically with the depth (roadmap 2.2).
- Expanding polymorphic recursion now stops at the elaborator's instance
  budget in seconds: `depth(W[T](x.copy()), n - 1)` is rejected with
  "instantiation past the 1024-instance budget" after about twelve seconds
  of elaboration in a debug build, where it used to spin for more than five
  minutes (the pin runs out of memory). A nested instance's symbol now
  spells the level below once, as its own symbol (`W$mono$TW$mono$TInt`),
  where it used to re-encode the whole of it and repeat its arguments, so
  the names grew threefold per level. The budget is the named
  `INSTANCE_BUDGET`, counted where an instance is demanded, and lowered
  from 4096 because a nested type's size grows quadratically with its
  depth (roadmap 2.2). The elaborator runs on its own thread with a deep
  stack, and its instance lookup is indexed.
- A collector indexed by a runtime value compiles natively: `sum += xs[i]`
  in `def total(*xs: Int) -> Int` gives `6` for `total(1, 2, 3)` under
  `--backend pliron`, as on the VM and at the pin, and so does a method
  called on `xs[i]` (`assets/ok/collector_runtime_index.mojo`). Native
  lowering used to reject it with "unsupported runtime index into pack
  storage". An instance with an empty collector, such as `total()`, used to
  fail MIR verification on both backends.
- A native `**kwargs` collector holds its keywords: `return kwargs["b"]` in
  `def pick(var **kwargs: Int)` gives `2` under `--backend pliron`, as on
  the VM and at the pin, through direct and indirect calls and over
  `String` values (`assets/ok/kwargs_keyed_read.mojo`). The native caller
  used to zero the collector and store only its count, so a keyed read
  stopped with "dereference of dangling Pointer".
- A `@staticmethod` of a value-parameterized struct reads its struct's
  parameter, as at the pin: `return Self.k` in `struct W[k: Int]` gives `5`
  for `W[5].st()`, and so do a bracket slot a callee infers from
  (`size(Counter[Self.k](a))`) and a static receiver spelled over it
  (`W[Self.k].st()`), on concrete MIR, the erased oracle, and natively
  (`assets/ok/static_method_reads_struct_value_parameter.mojo`). The
  inferring call used to fail MIR verification with "required compile-time
  value parameter 'n' is missing", and the erased oracle stopped with "field
  access on non-struct None".
- A collection display default takes its parameter's type as context, as
  at the pin: `def grow(var xs: List[Int] = [1, 2])` gives each call that
  leaves `xs` out a fresh list, and `[1, 2.5]` for `List[Float64]`, a set
  display for `Set[Int]`, and a dict display for `Dict[String, Int]` work
  alike on functions and methods
  (`assets/ok/collection_display_default.mojo`). The default used to be
  rejected as an `Array[Int, 2]`.
- A parameter default may read a compile-time parameter in scope, as at the
  pin: `V[3]().m()` beside `def m(self, x: Int = Self.n * 2)` in
  `struct V[n: Int]` prints `6`, and a method's or function's own parameter,
  an enclosing function's from a nested `def`, and a type parameter's
  `T()` work alike (`assets/ok/default_reads_binder.mojo`). The VM used to
  stop with "non-constant default", and `def g[n: Int](x: Int = n)` or a
  method's `x: Int = Self.n + j` was rejected with "Undefined variable".
- A trait requirement's default may read the method's own value parameters
  (`def scale[n: Int](self, value: Int, factor: Int = n)`), as at the pin: a
  call through the bound runs it with the call's compile-time arguments, or
  a parameter's own constant default, in their place
  (`assets/ok/trait_requirement_parameter_default.mojo`). It was rejected as
  unsupported. A requirement default is now also bound at a call that spells
  compile-time arguments (`s.scale[2](5)`), where the witness's default used
  to run instead.
- A declaring unpack binds its names even where a module constant shares
  one: `var i, j = 100, 1` beside `comptime i = 40` declares a local `i`,
  in a generic `def` as in a plain one, where elaboration replaced the
  target with the constant's literal and the program stopped with "invalid
  assignment target". A plain local shadowing a constant in a generic
  `def`'s body already read the local, as at the pin;
  `assets/ok/comptime_shadowed_locals.mojo` now pins both.
- An overloaded method is declared under the symbol its calls name. The
  lowered symbol now spells the method's own binders with their bounds
  (`First.pick$ov$T$Copyable$Int`), a callable-bounded binder by its callable
  type, an existential `Some[Writer]` parameter, and a `Scalar[dt]` pattern
  exactly as the checker spells a call's selected overload, where it spelled
  them bare and no call could reach the template. `self.item.write_to(writer)`
  beside a generic `write_to[U]` rival stopped with "vm: unknown method
  'Twin.write_to'" and now prints the item; `self.item.__hash__(hasher)`
  through a `Hashable` bound on such a struct runs; a struct whose
  `write_to` has a same-arity `String` rival displays through its `Writer`
  overload instead of the reflective default; and a call of a
  callable-bounded overload no longer stops in elaboration. The per-call
  clone filter no longer falls back to minting every same-named overload
  for an unrecognized qualifier. `assets/ok/overloaded_method_own_binder_symbols.mojo`
  pins it.
- An in-place operator applies through a module `def`'s reference result,
  as through a method's: `text(s) += "d"` for `def text(ref t: String) ->
  ref[origin_of(t)] String` calls `String.__iadd__` on `s` and prints `ad`,
  as the pin does, where it was rejected ("an in-place operator on the
  reference returned by 'text()'"). The operator's `AugmentedInPlace`
  record now keeps the call's reference result, which it used to replace.
  `assets/ok/reference_call_assignment_target.mojo` pins it.
- A chained comparison treats its operands as the pin does: its two ends
  are read where they lie when their link's dunder reads them, and a middle
  operand, which both links read, is copied once and must be
  `ImplicitlyCopyable`. `b == c < d` over an `ImplicitlyCopyable` struct
  prints one `copy` instead of three, and over a merely `Copyable` struct it
  is rejected ("cannot be implicitly copied"), even when the middle operand
  is a temporary. `assets/ok/chained_comparison_operand_copy.mojo` and
  `assets/type_error/chained_comparison_middle_not_implicitly_copyable.mojo`
  pin it.
- An annotated `comptime` binds at its declared type, as at the pin:
  `comptime ONE: Int32 = 1` prints `int32` for `ONE.dtype`,
  `comptime ONES: SIMD[DType.int32, 4] = 1` prints `[1, 1, 1, 1]`, and
  `comptime TWO: Float64 = 2` prints `2.0`; each used to bind as an `Int`
  literal. A literal the annotation does not accept
  (`comptime LABEL: String = 1`) is now rejected.
  `assets/ok/comptime_annotated_scalar.mojo` pins it on both backends.
- A pointer held in a generic struct's parameter-typed field dereferences in
  place as at the pin: `h.get()[]` twice over `h = Holder(Pointer(to=x))`,
  or two bound copies beside `h.item[]`, no longer conflict, because a
  pointer a method returns is now a shared alias of its owner like
  `Pointer(to=x)`. A subtree pointer (the extension) a generic `def`
  returns, read as `first(h)[]`, keeps its owner alive through a hidden slot
  instead of reading a destroyed one.
  `assets/ok/generic_field_pointer_deref_in_place.mojo` and
  `assets/extensions/ok/subtree_pointer_generic_return_deref.mojo` pin both
  on both backends.
- A pointer returned by a call and dereferenced in place (`print(idp(p)[])`
  over `def idp[o: MutOrigin](v: Pointer[Int, o]) -> Pointer[Int, o]`) reads
  its owner, where it printed `None` once the owner had no later use: the
  result is bound to a hidden slot carrying the loan its origin names, as
  `var q = idp(p)` already did. A generic identity, a method result, and
  the `unsafe_offset` spelling take the same path.
  `assets/ok/call_result_pointer_deref_keeps_owner.mojo` pins it on both
  backends.
- A numeric scalar is a condition, as at the pin: `if n:`, `while n:`, a
  conditional expression, and a comprehension filter over an `Int`, `UInt`,
  `Float64`, or width-one lane test it through `Bool(x)`, and `not n` reads
  it the same way, where each was rejected with "expected Bool".
  `assets/ok/numeric_truthiness_condition.mojo` pins it on both backends.
- A module-level compile-time evaluation no longer fails with "unsupported
  feature: comptime if" when the program holds a generic struct whose
  method has a `comptime if` on the struct's parameters: the evaluation's
  subprogram carries such a method as its template stub, as it already did
  for one holding a `rebind`.
  `assets/ok/ctfe_beside_comptime_if_struct_method.mojo` pins it.
- `repr` of an `Array` names its element type
  (`Array[SIMD[DType.int, 1], 2]([Int(1), Int(1)])`); it printed
  `Array[T, 2](...)`, the parameter as the template spelled it.
- A generic `def` storing a copy of its argument
  (`into.append(value.copy())`) lost the loan the copy carries, so the
  pointee was destroyed while the list still pointed at it and the program
  stopped with "subscript receiver is None".
- A template-served method calling a generic `def` that iterates an
  `Iterable` parameter (`Shelf[Int].first` calling `first_or`) failed with
  "iterator copy-reference adapter is attached to concrete target". The
  elaborator now resolves the adapted step, and a `for` over a named source
  of a parameter type borrows it in place instead of copying it into the
  loop (`assets/extensions/ok/template_served_iterable_def.mojo`).
- A method storing a copy of its argument (`self.push(value.copy())`) lost
  the loan the copy carries, so the pointee could be destroyed while the
  container still pointed at it. The receiver now borrows every place its
  type arguments name.
- On the VM, a place pointer bound to a type parameter was read as the value
  it points at: `value.copy()` and a read of a pointer-typed slot inside an
  instantiated body now yield the pointer.
- The elaborator minted one instance twice, and stopped on the symbol
  collision, where two calls differed only in a pointer argument's origin.
- A `mut` parameter transferred away must be written back before the
  function leaves. `def take(mut x: String) -> String: return x^` ran and
  printed `<ref 1:0>`, where the pin reports "'x' is uninitialized at return
  from this function". The ownership analysis now requires every `mut`
  parameter and `mut self` receiver to hold a whole value at each return, and
  at each raise ("use of uninitialized value 'x'"), and counts a write
  through the parameter as putting the value back. `x^` now takes the value
  out through the reference instead of handing over the reference, so a body
  that moves and refills (`var r = x^`, `x = String("b")`, `return r^`)
  returns the caller's old value on the VM and natively; a value that owns no
  storage transfers as a copy. A field left moved out of a `mut` parameter
  reports the same message in place of "destroyed out of the middle of a
  value". `assets/ok/mut_parameter_transfer_refilled.mojo` and six
  `assets/ownership_error/mut_*` fixtures pin it.
- A generic constructor declared beside another one runs on a generic
  struct. `C[Int](7)` for `struct C[U: AnyType]` declaring
  `__init__(out self, var a: String)` beside
  `__init__[T: Writable](out self, a: T)` failed with "vm backend does not
  support the built-in or callee 'C.__init__$ov$T$Writable' yet", where the
  pin prints `2`. The constructor's clone is now requested against the
  instance and minted with the instance's other per-call clones, keyed by the
  instance's arguments and then the call's (`__init__$y3:Int$y6:String`),
  and `native::mono` keeps such a clone on its own symbol instead of the
  instance's plain `__init__`.
  `assets/ok/generic_struct_generic_constructor_overload.mojo` pins it.
- An element passed to a read parameter is read where it lies. `print(a[0])`
  inside `show[*Ts: Writable](*a: *Ts)` ran the element's copy constructor
  and never destroyed the copy, and `g(xs[0])` or `print(t[0])` over a
  `List`, an `Array` or a `Tuple` did the same, where the pin copies
  nothing. MIR now loads a tuple or pack element's place and lends the
  referent of an owning container's accessor, as it lends a field, and the
  checker marks a pack or tuple element handed to `print` as read in place.
  The VM and the native backend agree
  (`assets/ok/pack_element_read_borrowed.mojo`). A `Span` element and a
  value read of a homogeneous collector's element still copy (roadmap 3.112
  and 3.1).
- A place gathered by an owned `var *args` collector must be implicitly
  copyable. `take(xs)` with `def take(var *xs: List[Int])` ran and printed
  `1` `2`; the checker now reports "value of type 'List[Int]' cannot be
  implicitly copied", as the pin does, on the free, method, static and
  constructor call paths. A tuple display (`(xs, 1)`) and `Tuple(xs, 2)`
  reach `Tuple`'s own `var *args` collector and are rejected the same way. A
  transfer (`x^`), an explicit `.copy()`, and a fresh temporary still pass
  (`assets/ok/variadic_var_collector_transfer.mojo`). The
  `assets/type_error/implicit_copy_variadic_var_collector*.mojo` fixtures and
  `implicit_copy_tuple_display_element.mojo` pin it.
- A place passed to a static method's `var` parameter must be implicitly
  copyable. `Box.keep(self.item)` with `keep(var v: Self.T)` was accepted for
  any `T`, and a `Box[String]` then stopped on the VM with "double free of
  Pointer allocation"; the checker now reports "value of type 'T' cannot be
  implicitly copied", as the pin and the instance-method path do. A transfer
  (`x^`), an explicit `.copy()`, and a fresh temporary still pass.
  `assets/type_error/implicit_copy_static_var_argument.mojo` pins it.
- A `^` transfer out of a read parameter is rejected. `def ident(x: String)
  -> String: return x^` moved the caller's `String` into the result, so the
  VM stopped with "use after Pointer deallocation" or a double free once the
  caller's value was dropped; the checker now reports "cannot transfer out
  of immutable reference", as the pin does. The rule covers a read `self`,
  a field of either, a bare type parameter, and a `for` or comprehension
  binder, and leaves a trivial register value (`Int`) transferable as a
  copy. `assets/type_error/read_parameter_transfer_returned.mojo` and four
  sibling fixtures pin it.
- Unpacking a tuple place copies each element into its target. `var s, n = t`
  over a `(String, Int)` tuple shared the `String` buffer between `s` and
  `t`, so the VM stopped with "use after Pointer deallocation" and the
  native backend with a double free; both now print `t 6` as the pin does.
  An element that is not `ImplicitlyCopyable` (`List[Int]`) is rejected at a
  named target, as the pin rejects it, and a `_` target reads nothing.
  `assets/ok/tuple_unpack_place_copies.mojo` and
  `assets/type_error/tuple_unpack_place_element_not_implicitly_copyable.mojo`
  pin it.
- A view returned by a method on a container element keeps the container
  alive: `var v = ys[0].rstrip()` followed by `String(v)` prints the stripped
  text where the VM stopped with "use after Pointer deallocation". The view
  lends the element's owned interior instead of nothing, so sibling element
  reads and a second view coexist with it, while `ys.append(…)` or
  `ys[0] = …` under a live view is rejected with "use of invalidated interior
  reference", as the pin rejects it. The same holds for a field of an element
  (`ps[0].name.rstrip()`, which used to be rejected as a loan conflict once a
  second element was read), a nested list, and a `Dict` value.
  `assets/ok/list_element_view_method_result.mojo` and
  `assets/ownership_error/list_element_view_stale_after_store.mojo` pin it.
- A constructor overload set ranks every candidate, as free functions and
  methods do. Selection used to keep only the concrete constructors whenever
  one matched, so `C(s)` for `__init__(out self, var a: String)` beside
  `__init__[T: Writable](out self, a: T)` printed `1` where the pin prints
  `2`, and a generic constructor needing fewer conversions lost too. A
  generic constructor of a non-generic struct, which the filter was hiding as
  unexecutable, now runs as a per-call clone (`C.__init__$y6:String`) the
  construction is retargeted to. Two type-pack constructors of one struct
  construct as well, where the VM reported "checked constructor … is missing
  from MIR". `assets/ok/overload_var_copy_constructor.mojo` and
  `assets/ok/pack_overload_constructor.mojo` pin it. On a generic struct, a
  generic constructor declared beside another is now selected and fails as
  unsupported instead of running the wrong constructor (`docs/roadmap.md`
  3.3).
- An owning `var *args` collector lives until the last use of any element
  read out of it, and its elements are then destroyed last to first.
  `take(Noisy(1), Noisy(2))` against `take[*Ts](var *a: *Ts)`, printing each
  `a[i]` in a `comptime for`, destroyed both elements before printing the
  second, and in first-to-last order. An element read out of pack storage
  now borrows the pack as a field read borrows its owner, and both backends
  destroy an owning collector in reverse where `Tuple` storage stays
  left to right (`assets/ok/owned_pack_destroyed_in_reverse.mojo`). The pin
  destroys a heterogeneous owned pack before consuming a literal-index
  element read that is its last use (`docs/non-goals.md`).
- An argument a read `*args` collector gathers is lent to the call, as a
  `read` parameter's argument is. `show(y, 5)` against
  `show[*Ts: Writable](*a: *Ts)` ran `y`'s destructor before `show`'s body,
  and a temporary argument was never destroyed. Neither the ownership
  analysis nor drop elaboration misread anything: the checker recorded no
  borrow for a collected argument, so MIR copied the local and its last use
  ended before the call. The native backend builds a read collector's pack
  by lending each element instead of cloning it. A module `def`, a method,
  and a static method behave alike, on the VM and natively
  (`assets/ok/pack_argument_destroyed_after_call.mojo`).
- The bitwise and shift operators have their own precedence levels. They
  shared one level with `+` and `-`, so `a << 1 | b >> 1` parsed as
  `(a << 1 | b) >> 1` and printed `7` for `Int32(6)` and `Int32(3)` where
  the pin prints `13`. The levels are now Python's and Mojo's: `|` loosest,
  then `^`, `&`, `<<`/`>>`, and `+`/`-`. No bundled or fixture source parsed
  differently before. `assets/ok/bitwise_shift_precedence.mojo` pins it.
- A reference a method returns through an origin binder keeps the argument
  it names alive. `print(words.pick(w))` printed `None` at `w`'s last use,
  where `pick[o: Origin](self, ref[o] x: Self.T) -> ref[o] Self.T` returns
  its parameter. MIR now reads such a result out of the hidden handle slot a
  module `def`'s result already used, which loans the places the result's
  origin names (`assets/ok/origin_binder_result_keeps_argument.mojo`).
- A `^` transfer of a value of a parameter type is rejected on the
  declaration unless a bound proves `Movable`, as the pin rejects it, where
  Mojito asked `Movable` of each instance
  (`assets/type_error/template_method_transfer_requires_movable.mojo`). A
  `where conforms_to(T, Copyable)` premise proves `Movable` too. The bundled
  `_ListOwnedIter.__next__` gains the clause it relied on.
- An `@implicit` conversion in a generic struct's method is selected once,
  on the declaration, where the template serves the method
  (`assets/ok/implicit_conversion_bound_on_declaration.mojo`). A method that
  still clones repeats the selection (roadmap 3.110).
- A method call that stores a parameter-typed element of the receiver back
  into the receiver (`self.items[i] = self.items[j]`) no longer leaves a
  loan of the receiver on itself at an instance whose element carries none.
- An elaborated `!=` on a struct that declares only `__eq__` dispatches
  `__eq__` and negates it, a comparison of sized scalars a template typed
  `Bool` converts its mask, and a construction through a variadic
  initializer names the instance keyed by its element count. The erased VM
  answers the first two the same way.
- A generic struct's static called on a spelled instance with no argument
  carrying the struct's parameter (`Pair[Int].count()` beside
  `def count(*values: Self.T)`, or a body-only `List[Self.T]()`) compiles
  natively and elaborates: the call names the instance's clone of the static,
  from an inferred body and a derived one alike, and concrete verification
  accepts a pack subscript over the tuple the elaborator rewrote the pack to.
  `assets/ok/template_method_generic_static_owner_unbound.mojo` pins it on
  both backends and the pin.
- A specialized program builds and writes through a `List[ref T]` on the VM.
  `RefList([a])` followed by `self.values[0] += 2` stopped with `vm: read of
  uninitialized Pointer storage`: `List.append`'s `self.data[size] = value`
  has the element type `ref Int` once `native::mono` substitutes `T`, and the
  VM took a store into a reference-typed place as a write through the handle
  already there, which an unfilled slot does not hold. A store into an
  element slot of multi-element pointer storage now fills the slot whatever
  its element type, as the erased body does. The
  `reference_list_write_through_method` row leaves `SPECIALIZED_VM_RESIDUE`,
  which is now empty.
- A specialized program's `**kwargs` callee runs on the VM. `total(first=1,
  second=2, third=3)` over `def total(var **kwargs: Int)` panicked with `no
  entry found for key`: the VM built the collector as `StringDict`, which a
  specialized program declares only as `StringDict$mono$TInt`, and
  `native::mono` dropped the instance's `__setitem__`, which no MIR call
  names. The VM now names the collector's instance, the specializer keeps
  its empty constructor and `__setitem__` for every callee with a keyword
  pack, and constructing an undeclared struct is a `RuntimeError` rather
  than a panic. The `kwargs` and `function_typed_kwargs` rows leave
  `SPECIALIZED_VM_RESIDUE`. A native collector still holds only its count
  (roadmap 2.3).
- A specialized program prints an instance through its display witness.
  `print(xs)` on a `List[Int]` wrote
  `List$mono$TInt(data=Pointer(…), size=2, cap=4)` on the VM where the erased
  program writes `[1, 2]`. `native::mono` keeps the witness as the
  per-instantiation clone re-owned to the instance
  (`List$mono$TInt.write_to$y3:Int`), which the VM's `format_value` did not
  look for, and it dropped the `write_to` of an argument a nominal writer's
  `write` formats (a `String` inside a float-parse error). The 21
  `specialized_vm` fixtures this held back now agree with their erased runs.
- The native backend calls a callable binding through its current value once
  the binding is written again. Monomorphization treated a `var f = one` as
  the constant `one` for the whole function, so after `keep(f)` assigned `two`
  through a `mut` parameter the caller still called `one` and printed `1`
  where the VM and the pin print `2`, and a plain reassignment `f = two`
  never finished compiling. A binding is now a known callable only while one
  definition is its sole write. `assets/ok/mut_callable_parameter_write_back.mojo`
  pins it on both backends.

- A function-local `comptime` float binding (`comptime x = Float64(2.5)`,
  `comptime h = 0.25`) is now stored as a `Float64`. Its runtime reads kept
  the exact literal, so the VM printed `5/2` where the pin prints `2.5`, and
  the native backend stopped with "unsupported display of a runtime
  FloatLiteral value". `assets/ok/local_comptime_float.mojo` pins it on both
  backends. A nested `def` reading such a binding is still rejected (roadmap
  3.107).
- Native: a generic struct's static taking a pack of the struct's parameter
  type (`def first(*values: Self.T) -> Self.T`) now gets one instance per
  element type. `native::mono` keyed such an instance by its pack length
  alone, so `Pair[Int].first(3, 4)` beside `Pair[String].first("x", "y")`
  shared one body and failed IR verification. Fixture
  `assets/ok/template_method_generic_static_parameter_pack.mojo`. A static
  whose arguments do not carry the parameter at all (an empty pack) is
  roadmap 2.1.

### Changed

- A runtime scalar now builds a multi-lane vector of its own dtype wherever
  one is expected, as upstream's implicit `SIMD.__init__(Scalar[dtype])`
  splat does: `var v: SIMD[DType.int32, 4] = Int32(9)`, an `Int` into a
  `DType.int` vector, a `Float64` argument at a `float64` vector parameter,
  a field, an assignment, a return, a default, or a generic body's
  `Scalar[dt]` (`assets/ok/simd_scalar_splat.mojo`), on the VM and
  natively. Each was rejected with a type mismatch; a scalar of another
  dtype and a `Bool` still are, as at the pin.
- A view of a container element lends the interior its method's return
  origin names: `ys[0].rstrip()` lends `ys["element"]["bytes"]` instead of
  the whole subtree below the element, so a stale use is reported with the
  pin's spelling, "use of invalidated interior reference 'v' to
  'ys["element"]["bytes"]'". `SemanticAdjustment::BorrowViewResult` carries
  the tags to MIR. A view whose origin is the element itself
  (`ys[0].codepoints()`) keeps the subtree loan, so a store over the element
  still stales it. The pin compiles that program and reads the bytes the store
  destroyed, an upstream defect recorded in `docs/non-goals.md`. Fixture
  `assets/ownership_error/list_element_view_stale_after_append.mojo`. This
  closes roadmap 3.2, and the new 3.2 records that `ys[0] += …` under a live
  view is not rejected.
- An owned heterogeneous pack keeps outliving a literal-index element read
  that is its last use: `print(a[0])` over `var *a: *Ts` prints the element
  and then destroys the pack. The pinned Mojo destroys the pack first and
  prints an element whose destructor already ran, a `String` field included,
  so the difference is an upstream defect recorded in `docs/non-goals.md`
  rather than an order to copy. This closes roadmap 3.2 with no code change.
- `CompiledProgram::elaborated_mir` is `drop_elaborated_mir`, naming the
  phase apart from concrete MIR. `backend::pliron::compile` takes the
  driver's `SpecializedProgram` and elaborates nothing; `compile_mir` keeps
  the old signature for a caller that names its own entries.
  `SpecializedProgram::program` is a `mir::ConcreteMir`.
- `verify_instruction`, one 1,300-line match over `MirInstr` in
  `mir/verify/instr.rs`, is now the checks every instruction gets followed by
  a dispatch on the instruction's family. The loan checks move to
  `verify/loans.rs`, the iterator-protocol checks to `verify/iteration.rs`,
  the subscript checks to `verify/subscripts.rs`, and the call checks to
  `verify/calls.rs`; references, value construction, private storage, SIMD,
  and effects stay in `instr.rs`. No function needs a `too_many_lines` or
  `cognitive_complexity` exemption. The move changes no behavior;
  `docs/symbol-map.md` lists the owners.
- `infer_method_call`, one 2,400-line method in
  `checker/method_calls/mc_infer.rs`, is now a short sequence of stages over
  one `MethodCallSite`. The receiver families move beside `selection`,
  `statics`, and `builtin_types`: `type_receivers.rs`,
  `intrinsic_receivers.rs`, `simd_receivers.rs`, `resolution.rs`,
  `receiver_effects.rs`, and `call_contract.rs`. No method needs a
  `too_many_lines` or `cognitive_complexity` exemption, and
  `infer_method_call` borrows its span. The move changes no behavior;
  `docs/symbol-map.md` lists the owners.
- `expr_unconverted`, the 2,600-line match over `ExprKind` in
  `mir/lower_expr/expr.rs`, is now a dispatch table: each arm calls one
  `Flatten` method. The call, method-call, and access arms move to
  `lower_expr/expr_call.rs`, `expr_method.rs`, and `expr_access.rs`, and no
  method needs a `too_many_lines` or `cognitive_complexity` exemption. The
  move changes no behavior; `docs/symbol-map.md` lists the owners.
- `checker/template_facts.rs`, 16,146 lines, is split along its five seams
  into `checker/template_facts/`: `capture.rs`, `certificate.rs` with the
  `BodyShape` grammar in `grammar.rs` and nine `grammar_*.rs` class files,
  `realization.rs` with `realization_calls.rs` and `realization_folds.rs`,
  `install.rs`, and `verify.rs`. The parent keeps the shared types, the
  shared helpers, and the body entry points. No file is over 3,000 lines.
  The move changes no behavior; `docs/symbol-map.md` lists the owners.
- The A1 shadow core's canonical text is 1.56 to 2.00 times the v1 text,
  down from 2.6 to 3.4: the `identity` attribute is left to the location
  that already names it, locations and contract keys are local to their
  function, a source record names its source by index, and the dialect is
  `mojito` (text schema `mojito-a1-core 1`).
- The A1 shadow core (feature `pliron-a1`) now converts 887 of the 893
  inputs of its decision corpus, where it converted the ten focused
  inputs: floats, `DType`, function types, closures, indirect calls,
  reference results, field reads, reference writes, slices, lane shuffles,
  pack iteration, uninitialized storage, variants, and returns and escapes
  crossing a try region, with or without a `finally`, all have core forms
  (`docs/notes/pliron-a1.md` §Coverage). Its census counts every rule the
  importer, the verifier, and legality apply, and its call-target rule
  reads the VM's own builtin tables (`mojito_vm::builtins`).
- Native monomorphization reifies an arity-specialized variadic collector
  as a tuple rather than the ABI-only runtime pack, and a method call it
  retargets to an instance sheds the instance's compile-time parameter
  declarations, so such specialized programs verify.
- Native monomorphization now runs `mir::verify` on the program it
  specializes and refuses one that does not verify. An indirect call's
  callable-contract target names the instance's concrete parameter types,
  and the verifier accepts a reference of any origin in a `ref` element
  slot and a capturing closure in unqualified callable storage, so
  `List[ref[o] Int]`, an `Array` of capturing closures, and
  `Array.deinit_with` specialize into verifying MIR, and the A1 shadow
  core converts 891 of its 893 corpus inputs.
- A specialized program runs on the VM where it slices or hashes a string
  literal (`assets/ok/keyword_slice_subscripts.mojo`,
  `assets/ok/tuple_hashable_dict_key.mojo`): the VM takes the slice bound's
  `Optional` and the literal's `String.__hash__` from the instances the
  program declares. A type argument named by string no longer drops a
  minted `Tuple`'s elements where the receiver spells them, so the A1
  shadow core converts 892 of its 893 corpus inputs.
- The A1 shadow core converts a `return` or escape inside a `finally` body
  (`assets/ok/pliron_finally_overrides.mojo`), the last of its 893 corpus
  inputs: the exit overrides the pending outcome and leaves as an ordinary
  exit.

- `print` with a call result as a keyword (`print(x, sep=String("-"))`,
  `print(x, file=FileDescriptor(1))`) in a method or a runtime `def` now
  reuses the template's checked facts
  (`assets/ok/template_print_call_keywords.mojo`). Such bodies used to be
  checked again per instance.
- `print` with a `sep`, `end`, `flush`, or `file` keyword
  (`print(self.w, sep=" ")`, `print(x, end="")`) in a method or a runtime
  `def` now reuses the template's checked facts, each instance seeing each
  keyword's type again (`assets/ok/template_print_keywords.mojo`). Such
  bodies used to be checked again per instance.
- `print` of a temporary of a struct parameter's type
  (`print(self.w.copy())`, or `print(self.s.echo(self.w))` through a bound
  whose result is the requirement's binder) now reuses the template's
  checked facts, each instance proving the temporary `Writable` at its own
  type (`assets/ok/template_method_print_parameter_temporary.mojo`). Such
  methods used to be checked again per instance.
- `String(value)` of a named value of a struct parameter's type
  (`String(self.w)` over `W: Writable`, or of a local or parameter of that
  type) now reuses the template's checked facts, each instance writing a
  numeric or `Bool` value itself and any other through its `Writable`
  conformance (`assets/ok/template_method_stringify_parameter_value.mojo`).
  Such methods used to be checked again per instance.
- A pack element's default construction over a pack holding its type
  more than once (`print(Self.Ts[i]())` over `Row[Int, Int, Bool]`, `var
  value = Ts[i]()` in `build[Int, Int]`) now reuses the template's checked
  facts, since nothing else in the loop copy tells those elements apart.
  Such instances used to be checked again.
- A call through a bound whose result is typed by the requirement's own
  binder (`self.s.echo(self.w)` over `echo[T: Copyable](self, other: T) ->
  T`) now reuses the template's checked facts, the result taking the
  argument's type in every instance
  (`assets/ok/template_method_bound_binder_result.mojo`). Such methods used
  to be checked again per instance.
- A call through a bound into an overload set no longer ranks the set on
  the argument types when no single member witnesses the requirement: the
  instance takes the requirement's witness, as the pinned Mojo binds the
  call, or is checked again. The ranking that fell back to the clone
  check's rules is gone.
- A call through a bound (`self.item.__hash__(hasher)` over `Pair[Int]`)
  whose witness has binders of its own and sits in an overload set of a
  generic struct now reuses the template's checked facts, naming the
  member of the instance's clone family that carries the witness's
  overload qualifier. Such instances used to be checked again.
- A float literal beside a value of a struct's symbolic lane
  (`self.pos * 0.5`, `self.pos < 0.5`, `self.pos += 0.25` over
  `Scalar[Self.dtype]`) now reuses the template's checked facts, the literal
  materializing at an instance whose lane folds to `Float64`. Such methods
  used to be checked again per instance.
- The borrowed `Array` and `List` iterators' `__next__`, which returns a
  reference into the iterated collection through the iterator's `ref`
  field, now reuses the template's checked facts. It used to be checked
  again in every checker pass.
- A string literal handed to a generic struct's static whose own binder
  types the parameter (`Pair[Self.T].pick("s")`, or spelled
  `Pair[Self.T].pick[String]("s")`, over `pick[U: Writable](u: U)`) now
  reuses the template's checked facts, each instance converting the literal
  into the per-call clone's parameter type. Such methods used to be checked
  again per instance.
- `String(value)` of a closed value in a generic struct's method or a
  nested `def` (`return String(k)` for `k: Int`, `return sep + String(x)`)
  now reuses the template's checked facts, as it already did in a
  module-level `def`. Such methods used to be checked again per instance.
- A field read or method call on a nested `def`'s read parameter holding a
  struct (`def f(x: Int, q: Pair) -> Int: return x + q.a` inside a generic
  struct's method) now reuses the template's checked facts, the parameter
  read where it lies as a method's own parameter is. Such methods used to
  be checked again per instance.
- A pack element's default construction handed to `print` or bound to a
  local (`print(Self.Ts[i]())` in a method, `var value = Ts[i]()` in a
  method or a pack-keyed `def`) now reuses the template's checked facts,
  each instance checking the elaborated construction alone
  (`assets/ok/pack_element_default_construction.mojo`). Such bodies used to
  be checked again per instance.
- A value of a struct parameter's type handed through a bound to a
  requirement parameter typed `Self` or by the requirement's own binder
  (`self.s.merge(self.t)` over `merge(self, other: Self)`,
  `self.s.pair(self.w)` over `pair[T: Writable](self, other: T)`) now
  reuses the template's checked facts
  (`assets/ok/template_method_bound_self_argument.mojo`). Such methods used
  to be checked again per instance.
- A list, set, or dict display handed to a method's parameter or built
  into a typed local (`self.item.total([1, 2, 3])` over `T: Totaler`,
  `var items: List[Self.T] = [...]`) now reuses the template's checked
  facts, a string literal element's conversion selected again per instance
  (`assets/ok/template_method_bound_display_argument.mojo`). Such methods
  used to be checked again per instance.
- A call through a built-in trait's bound (`self.item.__hash__(hasher)`,
  `self.item.write_to(writer)`) over an instance whose struct overloads
  the method now selects the member witnessing `Hashable`'s, `Writable`'s,
  or `Hasher`'s requirement, as it already did for a declared trait, so a
  rival such as `__hash__[U](self, mut hasher: List[U])` no longer refuses
  the derivation (`template_method_builtin_requirement_witness_beside_generic_rival_derives`).
  Such instances used to be checked again, since the ranking fallback could
  not infer the rival's own binder.
- A call through a bound with keyword arguments out of parameter order
  (`self.s.shift(1, extra=a, by=b)`) now derives a clone's call boundary
  in source order, as the clone's own check records it. The derived
  boundary used to list the arguments in parameter order, which template
  fact verification (`MOJITO_VERIFY_TEMPLATE_FACTS`) flagged.
- `Array`'s copy and move initializers, `deinit_with`, and both `__iter__`
  overloads now reuse their checked templates in every later checker pass
  (`array_members_reuse_templates`). They used to be inferred again: the
  method class refused lifecycle initializers and `deinit` parameters, a
  construction of a struct with a value parameter, and a call-through
  residue over types the reuse left symbolic.
- A runtime `while` in a compile-time-keyed `def` — at the top level of
  the body, inside a `comptime for` with `break` and `continue`, or under a
  `comptime if` arm — now derives the instance's facts from the checked
  template, each unrolled copy keeping its own loop
  (`assets/ok/template_keyed_runtime_while.mojo`). Such instances used to be
  checked again.
- A generic struct's method or keyed `def` binding a comparison of its
  symbolic lane's values to a local (`var m = self.pos == other`), read
  as a condition or through `Bool(m)`, or comparing such a value with an
  integer literal (`self.pos < 0`), now reuses its template's checked
  facts, re-typing the mask local and its reads to `Bool` where the lane
  folds to `Int` or `Float64` (`lane_comparisons_derive`). Such bodies
  used to be checked again per instance.
- A generic struct's method calling a float lane's `__fma__` with a
  temporary argument (`k.__fma__(self.step * self.step, self.start)` over
  `Scalar[Self.dtype]`) now reuses its template's checked facts, recording
  the temporary as a read one at the native `Float64`
  (`float_lane_methods_derive`). Such methods used to be checked again per
  instance.
- A t-string nested in another (`t"outer={t"inner={x}"}"`) is now stored
  as its own specialization, so the outer t-string's storage `Tuple` is
  declared in the same discovery round and its `write_to` derives there
  (`assets/ok/tstring_forms.mojo`). That early round used to check the
  outer `write_to` again.
- A generic struct's method reading a scalar field off a sibling call's
  whole-value result (`return self.bumped().count`) now reuses its
  template's checked facts
  (`assets/ok/template_method_variadic_struct.mojo`). Such methods used to
  be checked again per instance.
- A generic struct's method calling one of its statics with binders of
  its own through an explicit application (`Pair[Self.T].scaled[3](2)`),
  or calling a static overload family one of whose members declares
  binders on a spelled receiver (`Pair[Self.T].pick(5)`), now reuses its
  template's checked facts
  (`assets/ok/template_method_applied_overloaded_static.mojo`). Such
  methods used to be checked again per instance.
- A nested `def` in a generic struct's method, or a generic module `def`,
  whose parameter default constructs a declared struct
  (`sep: String = String("-")`) now reuses its template's checked facts,
  as a literal default already did. Such bodies used to be checked again
  per instance.
- A runtime `def` calling `repr` or `_unqualified_type_name[T]()`
  (`def shown[T: Writable & ...](x: T)`) now reuses its template's checked
  facts, as a generic struct's method already did
  (`assets/ok/template_def_string_builtins.mojo`). Such bodies used to be
  checked again per instance.
- `Tuple`'s default initializer is now spelled in the bundled source over
  the symbolic pack and checked once (`comptime for i in
  range(len(Self.Ts)): self.storage[i] = Self.Ts[i]()`), where the
  specializer used to write one per specialization. Every Defaultable
  specialization derives it from that template, each element's concrete
  construction recording its own facts
  (`assets/ok/template_tuple_default_initializer.mojo`). Hello World checks
  no such body again where it checked 4.
- A store to a compile-time index of the compiler-private tuple storage is
  now a place of its own, and an `out self` initializer that stores every
  element initializes the field, on the VM and natively.

- A call through a bound handing a string literal, a named `String`, or a
  field of `self` by value to a closed parameter of the requirement
  (`self.s.scale(2, label="y")` over `S: Scaler`) now reuses the template's
  checked facts, as does one leaving a string default the check spells at
  the call (`assets/ok/template_method_bound_value_argument.mojo`). Such
  methods used to be checked again per instance.
- A `hasher.update(self.value)` or `hasher._update_with_simd(self.inner.lanes)`
  call in a generic method whose own `H: Hasher` an instance binds to
  `AHasher` now derives from the checked template: a field argument is
  borrowed where it lies, told apart from the hasher by its base binding,
  instead of being refused as a place overlapping the receiver.
- A call through a bound whose witness is overloaded now runs the member
  that witnesses the requirement, as the pin binds it while it checks the
  generic body: `self.entry.total(3)` over `T: Tally` with `total(self,
  by: Float64)` required prints the `Float64` witness's result beside an
  exact `total(self, by: Int)` or a variadic `total(self, *by: Int)`,
  where it used to rank the set again and run the rival. Instances beside
  a variadic rival, or a rival with a reference parameter, now derive from
  the checked template instead of being checked again
  (`tests/compiler_test.rs:template_method_requirement_witness_*`).
- A write through a reference over a value the body cannot implicitly
  destroy is now rejected, as the pin rejects it: in a `def` over `T:
  Copyable`, `ref r = x; r = v.copy()` and `same(x) = v.copy()` through a
  call returning `ref[origin_of(x)] T` used to run, and now report the
  overwritten value "abandoned without being explicitly destroyed", as the
  plain `x = v.copy()` already did. The same holds for an
  `@explicit_destroy` referent
  (`assets/type_error/reference_binding_write_linear_parameter.mojo`,
  `assets/type_error/reference_call_write_linear_parameter.mojo`,
  `assets/type_error/reference_write_explicit_destroy.mojo`).
- A homogeneous `*args` over a loan-carrying element type now runs, as the
  pin runs it: `two(Span(xs), Span(xs))` over `def two[T: Copyable](*args:
  T)` with `xs` a read parameter used to be rejected with "type mismatch
  for variadic argument to 'two$y9:Span[Int]'". The collected arguments now
  bind the element's origins (`assets/ok/homogeneous_variadic_loan_carrying_element.mojo`).
- A loan-carrying value stored through a dereferenced pointer keeps its
  borrowed owner alive while the pointer is used, as the pin does: `q[] =
  Span(xs)` into an `unsafe_alloc[Span[Int, origin_of(xs)]](1)` followed by
  `print(q[][1])` prints the element on both backends instead of failing on
  the VM with "use after Pointer deallocation"
  (`assets/ok/pointer_deref_store_loan_carrying_pointee.mojo`).
- Each element a `*args` pack collects, and each element of a tuple
  literal, is now judged by argument exclusivity as an argument of its own,
  as the pin judges it: `show(Span(xs), Span(xs))` over `def show[*Ts:
  Copyable](*args: *Ts)` and `(Span(xs), Span(xs))` over a `var xs` are
  rejected with "aliasing values passed mutably to 'args' argument and
  passed mutably to 'args' argument". Both used to compile
  (`assets/type_error/variadic_elements_share_mutable_origin.mojo`,
  `assets/type_error/tuple_literal_elements_share_mutable_origin.mojo`).
- A nested `def` that reads only its enclosing function's value parameters
  is now `thin`, as the pin's is: `apply(inner)` against `def apply(f: def()
  thin -> Int)`, where `inner` reads `outer[n]`'s `n`, prints the result on
  both backends instead of being rejected with "expected def() thin -> Int,
  found def() capturing[...] -> Int". Natively each instance of the
  enclosing function gets its own environment-free instance of `inner`
  (`assets/ok/thin_nested_def_reads_value_parameter.mojo`).
- A generic nested `def` that captures now compiles natively, as it runs on
  the VM and the pin: `inner[10]()` reading its enclosing function's value
  parameter, a captured local, or a `mut` capture used to stop with
  "generic retained callable `outer$inner` has captures". A generic nested
  `def` called inside a `try` region no longer fails natively with "value
  parameter `k` is not compile-time constant"
  (`assets/ok/generic_nested_def_captures.mojo`).
- A `def` value parameter now infers from an argument's type, as the pin's
  does: `size(Counter[4](1))` against `def size[n: Int](c: Counter[n])`
  prints `4` on both backends, where it used to stop with "required
  compile-time value parameter 'n' is missing". An infer-only parameter
  (`def size[n: Int, //](c: Counter[n])`) used to read as `None`
  (`assets/ok/value_parameter_inferred_from_argument_type.mojo`).
- `String` now conforms to `Defaultable`, as the pin's does: `make[String]()`
  over a `T: Defaultable` bound and `Tuple[String, Int]()` run, where they
  used to fail the bound or stop with "no constructor overload matches the
  supplied arguments" (`assets/ok/string_default_construction.mojo`).
- A SIMD value now constructs with no arguments, as the pin's
  `SIMD.__init__()` does: `Float32()`, `UInt8()`, `Scalar[DType.int16]()`,
  and `SIMD[DType.int32, 2]()` zero every lane, and `T()` under a
  `T: Defaultable` bound to one runs, where each used to stop with "SIMD
  construction expects 1 element(s) or 1 to splat, got 0"
  (`assets/ok/simd_nullary_construction.mojo`).
- A variadic struct's index-keyed method under a name other than
  `__getitem__` (`def item[i: Int](self) -> Self.Ts[i]`) now runs as the pin
  does: `b.item[1]()` prints the element instead of stopping with "type
  'Bag$t2[…]' has no associated type 'element_types'"
  (`assets/ok/pack_index_named_method.mojo`).
- A generic struct's static with a binder of its own, called from a
  generic method on the bare struct name (`Pair.both(7, self.item)`) beside
  a spelled call (`Pair[Self.T].both(1, self.item)`), now infers `Pair`'s
  parameter and runs both, as the pin does, where it used to reject with
  "cannot infer type parameter 'T' of 'Pair'"
  (`assets/ok/template_method_static_binder_infers_struct_parameter.mojo`).
- A generic struct's overloaded static called on a spelled receiver from a
  generic method (`Pair[Self.T].pick(1, 2)` beside `pick[U: Writable](u: U)`
  and `pick(u: Int, v: Int)`) now calls the two-argument member at every
  instance, printing `3` as the pin does, where it used to stop with
  "'Pair.pick$y3:Int$y3:Int' expects 1 argument(s), got 2". The same
  overloads as instance methods, called on a `Box[Self.T]` local, no longer
  reject with "no overload matches"
  (`assets/ok/template_method_static_binder_overload_arity.mojo`).
- A generic struct's overloaded static called on a spelled receiver
  (`Pair[Self.T].pick(v)` beside `pick(v: Self.T)` and `pick(v: Float64)`)
  now calls the member its template ranked at every instance, as the pin
  does: at `T = Float64`, where both members take a `Float64`, it prints
  `1` instead of `2`, and `Pair[Self.T].pick(1)` calls the `Float64` member
  at `T = Int` too
  (`assets/ok/template_method_collapsed_static_overload.mojo`).
- A call may now leave out a parameter whose default is a construction or
  other expression, as the pin allows: `f()` beside `def f(s: String =
  String("a"))` prints `a`, and `x: Int = TWO + 1` runs `3`, where the VM
  used to stop with "non-constant default". Such a default now rejects when
  it names an enclosing local ("cannot use a dynamic value in default
  argument", as the pin does) or prints, since the pin evaluates a default
  at compile time.
- A trait requirement's default may now name a module constant, as the pin
  allows: `def scale(self, value: Int, factor: Int = TWO)` beside `comptime
  TWO = 2` used to be rejected as unsupported, and a call through the bound
  leaving `factor` out now runs `2` even where a local `TWO` is in scope at
  the call. A compile-time `Int` expression over such constants folds too.
- A witness may now default a parameter differently from its trait
  requirement, or not at all, as the pin allows. A call through the bound
  leaving the parameter out runs the requirement's default, so
  `self.s.scale(5)` beside `factor: Int = 2` in the trait and `factor: Int
  = 7` in the witness prints `11` as the pin does. Such a witness used to
  be rejected as a signature mismatch. A call on a nominal receiver still
  runs the witness's own default.
- Two same-arity generic overloads of one method that a call specializes
  alike no longer collide: `First(0).pick(2, 3)`, beside `pick[T](self, a:
  T, b: T)` and `pick[T](self, a: T, b: Int)`, prints `2` as the pin does,
  where it used to reject with "'pick$y3:Int' is already declared in this
  scope". Each call now mints a per-call clone of only the overload it
  selects, on a plain struct, a generic struct's instance, and through a
  trait bound.
- A call returning a mutable reference is now an assignment target, as the
  pin allows: `bump(k) = 9` and `bump(k) += 1` for `def bump(ref a: Int) ->
  ref[origin_of(a)] Int` print `9` then `10`, and a method's reference
  result (`p.x_ref() *= 3`) or a field below one (`pick(p).y = 4`) is
  written through alike. They used to stop at parse with "invalid
  assignment target". An in-place operator dunder on a module `def`'s
  reference result is still rejected (roadmap 3.99).
- A SIMD scalar now conforms to `Equatable` and `Comparable`, so
  `(UInt64(1), UInt64(2)) < (UInt64(1), UInt64(3))` prints `True` as the
  pin does, where it used to reject with "operator '<' is not defined".
  A scalar comparison now converts implicitly to `Bool`, so `-> Bool:
  return a == b` and `not (a == b)` over `UInt64`s run.
- Argument exclusivity now counts an origin an argument's type carries
  through a type argument, as the pin does. `l.insert(0, Span(xs))` on a
  `List[Span[Int, origin_of(xs)]]`, `pick(l, Span(xs))` over a generic
  `def`, and `p.unsafe_write(Span(xs))` through a pointer to such spans are
  rejected when `xs` is mutable ("aliasing values passed mutably to 'self'
  argument and passed mutably to 'value' argument in 'insert' call"). They
  used to compile. The same calls over an immutable origin still run.
- `@__unsafe_nested_origins_read_only` is now acted on: a callee so
  decorated reaches the origins its argument types carry immutably, so
  `l.append(Span(xs))` stays accepted. The bundled library declares the
  members upstream declares.
- Five `assets/ok` fixtures that relied on the missing check are respelled
  over a read parameter's origin, which the pin accepts, and their
  `divergence` rows leave `conformance/assets-mojo-rejects.tsv`.
- A nested `def` now reads its enclosing function's or method's value
  parameter without naming it in a capture list, as the pin does. It used
  to fail with "Could not infer capture convention of the captured value
  n". A generic nested `def` reading one still runs on the VM only
  (roadmap 3.93).
- A struct constructed over arithmetic on a compile-time parameter,
  `Counter[1 + Self.length](i)` in a method or `Counter[n + 1](i)` in a
  generic `def`, now runs on the VM and natively, as the pin does. The
  bracket argument used to reach MIR untyped and fail verification with
  "place rooted at slot 0 lacks complete checked type metadata" or
  "untyped register".
- `Int()`, `UInt()`, `Float64()`, and `Bool()` now construct their zeros,
  at run time, in a compile-time constant, and through a `Defaultable` type
  parameter, as upstream. They used to fail with "'Int' expects 1
  argument(s), got 0".
- A pack element's default construction, `Self.Ts[i]()` in a variadic
  struct or `Ts[i]()` in a pack-keyed `def`, is now checked with the pack
  symbolic and constructs the bound element in each instance. It used to
  fail with "'Self.Ts' is not a type parameter of the enclosing struct" or
  "Undefined variable 'Ts'". `Tuple`'s default initializer is still
  synthesized per specialization (roadmap 1.4).
- The program native monomorphization produces now runs on the VM as the
  original does. It keeps the nominal `String`'s `__copyinit__` and
  `write_to`, which native lowering bridges and never reaches, and the VM
  displays through a `write_to` whose writer specialization bound to the
  builtin string writer. `tuple`, `tstring`, and `stdlib_heavy` used to
  fail there with "struct `String` has no copy constructor".
- The Pliron A1 parameter payload now exports a closed atom kept unfolded
  (`8 // 2` after substitution) and a cancelled term holding a partial atom,
  through the new non-folding `ParamContext::rebuild`, instead of refusing
  both. A stored operator that is not canonical is still refused.
- A `Pointer` type argument over an interior-projected or subtree place
  (an array's `unsafe_ptr()`, `Pointer[Int,
  origin_of(a)._get_owned_interior["element"]]`) now bakes into its clone
  with a clone origin binder that re-applies the projection, so every such
  place shares one clone of a generic `def` or struct method
  (`interior_pointer_type_argument_clones_with_projected_binder`). It used
  to mint one clone per place. Natively, two such clones of one method that
  differ only in pointer provenance share one instance.
- A generic `def` call whose loan-carrying type argument names an enclosing
  origin binder (`unsafe_alloc[Span[Int, o]](n)` inside `def slots[o:
  MutOrigin]`, or inside a clone whose own binder stands for `o`) now
  calls the same clone as a call over a caller place, supplying the binder
  by name (`unspelled_loan_carrying_type_argument_clones`). It used to run
  the erased body.
- `Array`'s fill and default initializers, its comparison operators,
  `__contains__`, `unsafe_ptr`, and `write_repr_to` are now inferred once
  as templates and reused in every later checker pass, where each used to
  be inferred again (`array_members_reuse_templates`). The same widening
  lets `List`, `Dict`, `Optional`, and `Set.__ne__` reuse theirs.
- A comparison between two values of a symbolic lane (`self.pos <
  Scalar[Self.dtype](limit)` in a `DType`-keyed struct's member, `x > y`
  over a keyed `def`'s `Scalar[dt]` parameters), tested as a condition or
  through `Bool(...)`, now reuses the template's checked facts at every
  lane, comparing to a mask at a sized lane and to a `Bool` at `Int` or
  `Float64` (`lane_comparisons_derive`).
- A member of a `DType`-keyed struct calling `__ceil__`, `__floor__`,
  `__trunc__`, or `__fma__` on values of its symbolic lane, including the
  bundled float range's initializer and `__next__`, now reuses the
  template's checked facts at every float lane (`float_lane_methods_derive`).
  A `Scalar[Self.dtype](x)` construction in such a member, and
  `AHasher[key]`'s initializer, derive again: since the elaborator began
  naming a spelled type's expansion by derived identities, their instances
  had been checked again as if unrelated to the template.
- `TString.write_to`, which binds each element with `ref element =
  self.storage[i]` inside a `comptime for`, now reuses the template's
  checked facts for every t-string specialization, including one nested in
  another (`tstring_write_to_derives`). It used to be checked again per
  specialization, and a nested t-string's members could not name their
  element type at all.
- A user variadic struct's index-keyed accessor (`def __getitem__[i:
  Int](self) -> Self.Ts[i]` over `self.storage[i].copy()`, or one returning
  a reference to `self.storage[i]`) now reuses the template's checked facts
  for each unrolled `__getitem__$k`
  (`assets/ok/template_method_pack_accessor.mojo`). Each such accessor used
  to be checked again per specialization.
- A variadic struct's initializer building its storage with
  `Tuple(*args^)`, a user struct's and the bundled `TString`'s, now reuses
  the template's checked facts: each instance's per-element expansion takes
  identities derived from the spread (`spread_pack_initializer_derives`).
  Such bodies used to be checked again per specialization, and every
  `TString` member's trace was recorded under its storage pack's name
  rather than the specialization's, so none reached the checker.
- A generic `def` or method converting a parameter, a local, or a field
  through its bound (`Int(mode)` on `mode: intable` with `intable:
  Intable`, `Float64` on `Floatable`, `Bool` on `Boolable`) now reuses the
  template's checked facts (`assets/ok/template_def_bound_conversion.mojo`). The bundled
  `stat.S_ISDIR`, `S_ISREG`, and `S_ISLNK`, which `path.isdir`, `isfile`,
  and `islink` call, used to be checked again per instance. A struct
  declaring `Floatable` now satisfies a `Floatable` bound; it was rejected
  as missing `__float__`.
- A generic `def` declaring a literal parameter default
  (`mode: Int = 0o777` in `os.mkdir` and `os.makedirs`,
  `label: String = "tag"`, `note: Optional[Int] = None`) now reuses the
  template's checked facts, whether a call leaves the default out or
  supplies it (`assets/ok/template_def_defaulted_parameter.mojo`). Such
  bodies used to be checked again per instance.
- A generic `def` spelling a type alias in its body
  (`external_call["close", c_int](c_int(fd))`, `var status: c_int`) now
  reuses the template's checked facts: the alias's expansion takes the same
  identities in every clone (`assets/ok/template_def_type_alias.mojo`). Such
  bodies used to be checked again per instance.
- A generic struct's static with an availability condition, a `mut` or
  `ref` parameter, a read-only variadic pack, or binders of its own
  (`Pair[Self.T].swap(a, b)`, `Pair[Self.T].show(5)` beside
  `show[U: Writable](u: U)`) now reuses the template's checked facts; with
  its own binders each instance calls the per-call clone keyed by its own
  receiver (`assets/ok/template_method_generic_static_shapes.mojo`). Such
  bodies used to be checked again per instance.
- A generic struct's overloaded static whose members differ in a parameter
  of the struct's parameter type (`Pair[Self.T].pick(v)` beside
  `pick(v: Self.T)` and `pick(v: Float64)`) now reuses the template's
  checked facts on a spelled receiver: each instance ranks the family again
  at its own argument types and keeps the template's member where it ranks
  best (`assets/ok/template_method_overloaded_parameter_typed_static.mojo`).
  Such bodies used to be checked again per instance.
- A struct method declaring a nested `def` with a string-literal or `None`
  default, a typed `raises`, or an `out` parameter (`def join(x: Int, sep:
  String = "-")`, `def check(x: Int) raises Low`, `def make(x: Int, out r:
  Int)`) now reuses the template's checked facts, and so does a call handing
  such a `def` a literal
  (`assets/ok/template_method_nested_def_defaults.mojo`). Such methods used
  to be checked again per instance.
- A trait requirement may now declare a literal default, and a call
  through the bound may leave it out; every witness must declare the same
  defaults, since current Mojo runs the requirement's there. Such a call,
  or one passing a keyword argument, now reuses the template's checked
  facts (`assets/ok/template_method_bound_defaulted_argument.mojo`).
- A keyed `def` reading a lane of a value-shaped vector of a fixed width
  (`lanes[0]` on `SIMD[dt, 4]`), converting it or copying it into a local
  (`var first = lanes[0]`), now reuses the template's checked facts: every
  instance closes the receiver to a vector, and re-proves the copy at its
  own lane type. Such bodies used to be checked again per instance.
- A hasher leaf that holds the defaulted `value.to_bits()` in a local and
  casts each lane before using it (`bits[i].cast[DType.uint64]()`) now
  reuses the template's checked facts: a lane over the open dtype is
  admitted as a cast's source alone, and each instance records the cast
  over its own lane
  (`assets/ok/template_method_simd_leaf_default_bits_lanes.mojo`). Such
  leaves used to be checked again per instance.
- A `print` of a whole value in a trait-bound `def` or a generic struct's
  method (`print(x, s)`, `print("item", self.item)`) now reuses the
  template's checked facts: `print` reads the argument where it lies, and
  each instance proves it `Writable` again at its own type
  (`assets/ok/template_print_whole_value.mojo`). Such bodies used to be
  checked again per instance.
- A method whose own `H: Hasher` an instance binds to `AHasher` now
  derives when it hands the hasher a multi-lane vector
  (`hasher._update_with_simd(SIMD[DType.int32, 4](...))`, or `update`):
  each hash-leaf demand is keyed by the bound call that made it, so the
  template withholds the leaf the instance's struct method never records.
- A call through a bound whose witness overloads the requirement now
  derives beside a generic rival the recorded argument types alone rank:
  the rival's binders are inferred as the clone check infers them, inside
  a parameter type (`List[U]`), from a `Scalar[dt]` pattern, or from a
  literal, and an unavailable or static member drops out as it does there.
- The elaborator now gives a struct method's own binders the checker's
  identities. Two overloads whose parameter slots agree own distinct
  binders there too, named through the shared
  `symbol::MethodBinderOwners`, and a binder's slot skips the erased
  `Origin` parameters and their mutability binders, as the checker's does.
- Two overloaded trait requirements whose parameter slots agree
  (`def pick[T](self, a: T)` beside `def pick[T](self, a: T, b: T)`) no
  longer give their binders one identity. Each is owned by the
  signature-qualified symbol its requirement lowers to, as an overloaded
  struct method's already was, and trait requirements now count in the
  program's overload sets.
- A method reading a value through a module `def`'s reference result
  (`var r = pick(a, b, first)` then `self.value = r^`, or
  `pick(a, b, first).copy()`, with `pick` returning
  `ref[origin_of(a, b)] T`) now derives its instances from the checked
  template instead of checking each again. The call's reference is kept by
  template owner as a method's is, and its `ref` arguments as kept places.
- A variadic struct specialized whole (`Bag$t2[Bool, Int]`) now derives a
  member's `self.copy()` call with the specialization as its result type,
  where the derived contract kept the template's `Bag[*Ts]`: a derived
  method call's result, raised, parameter, and referent types substitute
  the instance's pack and folded values too. Only
  `MOJITO_VERIFY_TEMPLATE_FACTS=1` saw the difference.
- A `Tuple` over loan-carrying elements (`Tuple[StringSpan, StringSpan]`)
  no longer infers every member again in each pass. An instance argument
  whose origins the elaborator erased now derives from the checked
  template as plain data does, and capture no longer mistakes the facts a
  clone's `rebind[T]` target records for facts outside the body. Hello
  World's clone inferences drop from 53 to 24.
- A generic hasher's wildcard `_update_with_simd` trap stub
  (`AHasher[key]`) is no longer inferred in every specialization. The
  specializer shapes it once, spanned at the struct with identities derived
  from the method's first statement, and every other copy derives from the
  first one checked. Hello World's clone inferences drop from 55 to 53.
- A Defaultable `Tuple` specialization's synthesized default initializer
  is no longer inferred in every discovery round. It is spanned at the
  struct and identified apart from the variadic initializer, builds its
  storage at exactly the element types (`Int(0)` for an `Int` element), and
  later rounds derive from its first checked copy. Hello World's clone
  inferences drop from 62 to 55.
- `Tuple`'s consuming teardowns (`consume_elements`, `deinit_with`) are no
  longer inferred in every specialization over plain-data elements. Their
  compile-time callable binder is kept by each clone, so each unrolled
  `elt_handler[i](self.storage[i]^)` derives from the template checked with
  the pack symbolic. Hello World's clone inferences drop from 88 to 62.
- An unavailable member's trap stub (`Tuple[Int, Optional[Int]].__lt__`)
  is no longer inferred in every specialization. The specializer shapes one
  stub per template method, spanned and identified at the unavailable
  clause, so each copy keeps the first copy's identities and derives from
  its check. Hello World's clone inferences drop from 138 to 88.
- Scalar arithmetic on a direct `ref` field (`self.exact + self.source`,
  with `var source: ref[o] Int`) now compiles natively
  (`assets/extensions/ok/pliron_a1_gate.mojo`); the native backend fed the
  field's handle to the operator and failed IR verification. An operator's
  operand now reads through a handle to scalar storage, as the VM's place
  load does.
- The A1 shadow core now converts all ten focused benchmark inputs, where
  it refused four of the seven compile benchmarks
  (`a1_focused_inputs_convert`). Its closed inventory grows from 26 to 37
  operations, and its types, declarations, and calls carry the facts those
  benchmarks need (`docs/notes/pliron-a1.md`). Still optional and default
  off.
- Native monomorphization now produces MIR that passes `mir::verify` for
  `benchmarks/compile/stdlib_heavy.mojo`: a rewritten dunder call records
  one place per argument, and a subscript contract's parameter types are
  substituted with the rest of the call.
- The Stage A1 shadow core is built as an optional, default-off experiment
  (`pliron-a1`): verified MIR re-expressed as a `mojito_core` Pliron dialect
  and exported back. The vertical slice passes and the pivot decision is
  open, since overhead is unmeasured (`docs/notes/pliron-a1.md`). No
  pipeline, schema, or backend input changed.
- A nested generic `def` whose body builds a callee's value argument from
  its own parameter (`scaled[k + 1]()` inside `inner[k: Int]`) now compiles
  natively (`assets/ok/nested_def_value_argument.mojo`); the native backend
  also compiled the nested `def` once with its parameter unbound and refused
  the argument as unresolved. A generic nested `def` is now specialized only
  at the call sites that bind its parameters.
- A one-element tuple (`(7,)`) and a variadic struct over one element
  (`Bag[Bool](True)`) now compile natively
  (`assets/ok/tuple_one_element.mojo`); the native backend passed the lone
  argument where the constructor expected its collected pack and failed IR
  verification. A constructor call now binds a variadic initializer's
  arguments through the pack collector whatever their count.
- A named destructor called on a field of a consumed `self`
  (`self.lease^.release()` in a `deinit self` method) now compiles natively
  when the field's type has droppable fields
  (`assets/ok/explicit_destroy_consumed_field_droppable.mojo`); the native
  backend refused it as a place consumption with droppable fields. The
  callee's `deinit self` exit already destroys those fields in place.
- A struct instance over a multi-lane vector
  (`Box[SIMD[DType.float32, 2]](v)`) now compiles natively
  (`assets/ok/template_method_vector_instance.mojo`), where the native backend
  refused its constructor with a mangled name holding a fragment of the type
  argument. A subscript of a `List` of multi-lane vectors (`l[0]`, `l[0] = v`)
  now runs instead of failing MIR verification
  (`assets/ok/list_of_vectors_subscript.mojo`). A method symbol now splits
  from its receiver only at a `.` outside brackets, so a clone's baked
  `DType.float32` never splits it.
- A struct instance over a generic instance (`Bag[List[Int]](List[Int]())`)
  and a struct whose initializer collects `var *values` (`Bag(1, 2)`) now
  compile natively (`assets/ok/instance_constructor_symbols.mojo`,
  `assets/ok/template_method_variadic_initializer.mojo`). Both ran on the VM
  and the native backend refused their constructors as colliding with an
  existing symbol. An instance over a loan-carrying argument
  (`Bag[Span[Int, o]]`) now clones its constructors too.
- A value argument built from the caller's own value parameters
  (`successor[n, 1 + n]()`, a method's `below[Self.n, k]()`,
  `Counter[Self.length](i)`) now compiles natively
  (`assets/ok/value_argument_from_caller_parameter.mojo`). It ran on the VM
  and the native backend refused it as "not compile-time constant". MIR
  records the argument as a parameter expression, which monomorphization
  evaluates under the caller instance's bindings (MIR text schema 1.7).
- A struct operator whose dunder reads its operands (`b == c`, `b + c`)
  now borrows a named operand where it lies, and `hash(b)` hashes its
  argument in place, as upstream (`assets/ok/operator_operand_borrow.mojo`),
  on the VM and natively. Each took a lifecycle copy of the operand, so a
  printing copy constructor printed once per operand.
- An exact literal now builds a multi-lane vector wherever one is expected,
  as upstream's implicit `SIMD(IntLiteral)`/`SIMD(FloatLiteral)` splat does:
  `P(1)` for a `SIMD[DType.int32, 4]` field, `var v: SIMD[DType.float32, 4]
  = 1.5`, an argument, an assignment, a return, or a parameter default
  (`assets/ok/simd_literal_splat.mojo`), on the VM and natively. Each was
  rejected with a type mismatch.
- A `Pointer` type argument naming a caller place
  (`alloc(Layout[Pointer[Int, origin_of(x)]](count=1))`,
  `unsafe_alloc[Pointer[Int, origin_of(x)]](n)`, a user `Holder[T]` built
  over `Pointer(to=x)`) now bakes into its clone with a clone origin binder
  in place of the place, so every place of the shape shares one clone
  (`assets/ok/pointer_type_argument_clone_binder.mojo`). Such a clone was
  minted once per place, and its name spelled that run's owner id.
- `copy()` on a `Pointer` now returns the pointer, as upstream's
  `Copyable` pointer does, on the VM and natively. A generic body calling
  `.copy()` on a parameter bound to a pointer was rejected once the pointer
  was baked into its clone.
- An iterator a method returned over a loan-carrying list (`it =
  l.__iter__()` over `List[Span[Int, origin_of(xs)]]`) now gets its own
  clone of `next` or of a user generic `def` it is handed to
  (`assets/ok/receiver_origin_iterator_argument.mojo`). The checker records
  its origin slot as the method's receiver origin, which the elaborator
  refused to bind, so such a call kept the erased body.
- The native backend now runs an explicit `l.__iter__()` over a
  loan-carrying list. The iterator instance was rejected as an identity
  collision because its `ref` field's referent differed only in an origin.
- A generic `def` whose loan-carrying type argument reaches only its result
  (`unsafe_alloc[Span[Int, origin_of(xs)]](n)`, a user `make[T](n) ->
  Pointer[T, MutUntrackedOrigin]`) now gets its own clone
  (`assets/ok/unspelled_loan_carrying_type_argument.mojo`): the argument's
  origin slots become explicit binders of the clone, which the call
  supplies from its own application. Such a call kept the erased body,
  since the clone's binders could only be inferred from its arguments.
- An explicit origin argument now binds the origin slots of the callee's
  parameter and result types (`slots[origin_of(xs)](n)` over `-> Pointer[
  Span[Int, o], MutUntrackedOrigin]`), not only its `ref` signatures, so
  writing a span of `xs` through the result no longer fails to type.
- A trait-bound `def` handing its `T: Titled` parameter to a method taking
  `Some[Named]`, where `trait Titled(Named)`, now derives from its checked
  template, as the same body over `T: Named` did
  (`template_def_builds_hasher_derives`). The method grammar admitted the
  existential only when the binder's bounds named its bounds literally, so
  a refining bound kept the clone check.
- A whole value discarded with `_ =` in a method (`_ = first` over a local
  of the struct's parameter type, `_ = first^`, `_ = self.items.pop(i)`) now
  derives from the method's checked template
  (`assets/ok/template_method_discarded_value.mojo`), and so do `Set.remove`
  and `Set.discard`. The method grammar's discard arm admitted only a closed
  scalar, so such bodies were checked again per instance.
- A module `def` updating a `mut` parameter through its in-place dunder
  (`def add_in[T: Accum](mut a: T, b: T): a += b`) now derives each instance
  from its checked template
  (`assets/ok/template_method_parameter_built_store.mojo`). The function
  class refused any `mut` parameter and any in-place update, so the body was
  checked again per instance.
- An augmented assignment through an in-place dunder that a parameter's
  bound declares raising (`self.total += x` on a `T: Checked`) now derives
  from the method's checked template, whether the instance's witness raises
  or not (`assets/ok/template_method_inplace_place.mojo`). The method grammar
  refused a raising dunder dispatched through a bound, so the body was checked
  again per instance.
- An augmented assignment to a `var` local through its in-place dunder
  (`var m = self.meter; m += Meter(3)`) now derives from the method's checked
  template (`assets/ok/template_method_inplace_place.mojo`). The method
  grammar admitted such an update only on a field of `self` or a `mut`
  parameter, and took a local copied from a field for a scalar, so the body
  was checked again per instance.
- An element store into a `mut` parameter's field (`other.items[i] = x`,
  `other.counts[i] += 1`) now derives from the method's checked template
  (`assets/ok/template_method_subscripted_receiver.mojo`). The method grammar
  admitted element stores only on `self`, so such a body was checked again
  per instance; a store taking its value from a parameter's field already
  derived.
- A struct specialized whole because a value binder is a lane width
  (`Width[n: Int]` spelling `SIMD[dt, Self.n]`) now has its members and each
  per-call clone of a method keyed on its own binder (`w.rep[DType.int16](v)`)
  derived from checked templates
  (`assets/ok/dtype_keyed_method_value_struct.mojo`). Its members were never
  validated and its per-call clones left no trace, so every one was checked
  again per specialization and per call.
- A method keyed on a `DType` or vector-width binder of its own
  (`kind[dt: DType](self, …)` building `Scalar[dt](…)`, `lanes[w: Int](self)`
  building `SIMD[DType.int32, w](…)`) now derives each per-call clone from
  its checked template (`assets/ok/template_method_own_lane.mojo`). Such
  clones used to be checked again per call: the certificate refused a
  `DType` binder, and a body constructing at its own lane had only a trap
  stub as its template.
- A method reading its struct's closed scalar value binder (`Self.rows` in
  `Grid[T, rows: Int]`, `Self.length` in `Array`) is now certified as a
  template, so `Grid.count`, `Array.__len__`, and `Array`'s literal
  initializer are reused from their checked facts in every later checker
  pass (`assets/ok/template_method_value_parameter.mojo`). The method
  grammar used to refuse such a read, so the body was inferred again.
- The bundled `AHasher[key]`'s own members — both initializers, `_update`,
  `_large_update`, `_update_with_bytes`, `update`, and `finish` — now derive
  from their checked templates in every specialization
  (`assets/ok/template_method_vector_key.mojo`). A struct keyed on a closed
  vector reads `Self.key`, calls a vector alias as a constructor, reads a
  module's integer constants, and hands closed values to a module function
  within the method grammar; a lane read of a closed vector copied into a
  local derives too. They used to be checked again per specialization.
- A member of a struct specialized whole on a `DType` binder that holds
  values of its symbolic lane (`Scalar[Self.dtype]`) — converting them,
  constructing one, storing them, and combining them with literals — now
  derives from its checked template at every dtype, including the native
  `Int` and `Float64` lanes (`assets/ok/template_method_struct_lane.mojo`).
  The bundled integer ranges' members used to be checked again per
  specialization.
- A per-call clone minted for compile-time evaluation — a hasher's
  `_update_with_simd` leaf in the VM-CTFE subprogram of `comptime CT =
  hash(...)`, or a leaf of the `AHasher` specialization it carries — now
  derives its facts from the compilation's checked template
  (`assets/ok/comptime_hash.mojo`). The subprogram used to be checked on a
  fresh template catalog, so every such clone was checked again, and the
  traces its minting recorded leaked into the driver's elaboration.
- A member of a user variadic struct specialized whole (`Bag$t2[…]`) now
  binds its pack to the element types its trace names and names that
  specialization for its template's `Self`, so `__len__`, sibling calls,
  and the synthesized `copy` derive from their checked templates
  (`assets/ok/template_method_variadic_struct.mojo`). Such members used to
  be checked again per specialization; a synthesized `copy` of any struct
  that is not `ImplicitlyCopyable` did too.
- A runtime `if` in a compile-time-keyed `def` — inside a `comptime for`
  body, under a `comptime if` arm, or outside both — now derives the
  instance's facts from the checked template, each unrolled copy keeping its
  own `if` (`assets/ok/template_keyed_runtime_if.mojo`). Such instances used
  to be checked again.
- A module-level `def` holding a keyword slice of a closed local, the
  `String(value)` stringify builtin, `external_call`, a whole rebinding of a
  local, a returned tuple display, a direct call's result held whole, a
  method call through its parameter's bound, or a raised `Error` of a built
  `String` now derives its instances from the checked template. The bundled
  `os.rmdir`, `os.remove`, `path.split`, `path.dirname`, and `path.exists`
  used to be checked again per instance, as did a method raising such an
  `Error`.
- Native monomorphization now solves and substitutes a value binder by its
  declaration's identity, as it does a type binder, so two value binders
  sharing a spelling keep their own solutions. A body's read of a
  value-parameter local resolves through its declaration's own binders.
- Most members of a `Tuple` specialization now reuse their template's
  checked facts: the static `__len__`, the initializer, every unrolled
  element accessor and its value twin, the synthesized `copy`, each
  per-element `__contains__` overload, and `write_repr_to`. Hello World's
  generated body inferences fall from 336 to 200. Such members used to be
  checked again per specialization.
- A struct method calling an overloaded static of a generic struct whose
  members differ only in closed parameter types (`Pair[Self.T].pick(v, 1)`
  beside `pick(v: Self.T, f: Float64)`) now reuses its template's checked
  facts, a spelled receiver naming the instance's clone of the ranked member
  (`assets/ok/template_method_overloaded_generic_static_call.mojo`). Such
  methods used to be checked again per instance.
- A struct method declaring a nested `def` with a `mut`, `var`, or `ref`
  parameter, a closed-scalar default, or `raises` (`def bump(mut x: Int, y:
  Int)`, `def scaled(x: Int, by: Int = 3)`, `def check(x: Int) raises`) now
  reuses the template's checked facts
  (`assets/ok/template_method_nested_def_conventions.mojo`). Such methods
  used to be checked again per instance.
- A struct method calling a method on a nominal receiver and leaving a
  defaulted parameter to its default (`self.name.find("y")`,
  `self.tag.joined(times=2)`, a named destructor's `tag^.reap()`) now
  reuses its template's checked facts in every instance
  (`assets/ok/template_method_defaulted_argument.mojo`). Such methods used to
  be checked again per instance.
- A generic struct's method handing a construction a pointer to `self` or
  a field of it rebound to the whole receiver
  (`Pointer(to=self.items).unsafe_origin_cast[origin_of(self)]()`) now
  reuses its template's checked facts in every instance, so the borrowed
  `Set.__iter__` and `Dict.__iter__` do
  (`assets/ok/template_method_receiver_pointer.mojo`). Such methods used to
  be checked again per instance.
- A keyed `def` applying an operator, a reduction, a cast, a lane count, or
  a built-in conversion to a value built by a value-shaped construction
  (`var lanes = SIMD[DType.int64, w](v)`, then `lanes + lanes` or
  `Int(lanes.reduce_add())`) now reuses its template's checked facts in
  every instance (`assets/ok/template_value_shaped_operations.mojo`). Such
  instances used to be checked again per call.
- `to_bits()` with its defaulted target now type-checks over a lane whose
  dtype is still a parameter (`def bits[dt: DType, w: Int](value: SIMD[dt,
  w])`), its target the unsigned dtype of the lane's width, as upstream
  (`assets/ok/simd_to_bits_default_symbolic.mojo`). It used to be rejected
  with "an explicit target dtype for a symbolic source lane".
- A `Hasher`'s `_update_with_simd` reading `value.to_bits().cast[DType.uint64]()`
  now reuses its template's checked facts in every per-call leaf, each
  recording its own reinterpretation and cast
  (`assets/ok/template_method_simd_leaf_default_bits.mojo`). Such leaves used
  to be checked again per call.
- A `def` keyed on a `DType` binder, or using a parameter as a lane width,
  with no compile-time control flow (`def lane[dt: DType](v: Int)`
  constructing `Scalar[dt](v)`) now has its body checked once by source
  validation with the parameters symbolic, and each per-call instance reuses
  those facts (`assets/ok/template_value_keyed_lane_def.mojo`). Such
  instances used to be checked again per call, their template dropped.
- A `print` statement in a runtime `def` or a struct method (`print(n)`,
  `print("count", self.count, extra)`) now reuses the template's checked
  facts, as it already did in a compile-time-keyed body; each instance proves
  its arguments `Writable` again (`assets/ok/template_print_statement.mojo`).
  Such bodies used to be checked again per instance.
- A division, a comparison, or a `~` over folded `comptime for` variables,
  value parameters, and literals alone (`print(~i, i * 2 < 5)`, `acc += i /
  2`, `if n > 2:` in a runtime `def`) now reuses the template's checked
  facts, as integer arithmetic already did; an `Int` fold past the machine
  range wraps, as the clone check does (`assets/ok/template_folded_operators.mojo`).
  Such bodies used to be checked again per instance.
- The Pliron backend lowers a unary `-` or `~` over an operator on two
  literals (`-(2 * 3)`, `~(i * 2)` in an unrolled loop), which it rejected as
  an untyped operand.
- A surviving trait-bound module-level `def` now derives its instances
  through a tuple unpacking of a direct call's result (`var head, tail =
  split(path)`, and `head, tail = split(head)` again) and a runtime `try`
  with a bare or error-binding `except`, so the bundled `os.removedirs`
  inherits its template's facts (`assets/ok/template_def_try_unpack.mojo`).
- A method result whose loans arrive only through a type argument, such as
  `List.pop()` over `List[Span[Int, o]]` or `List[RefBox[o]]`, no longer
  borrows its receiver: the list may be used again while the popped value
  lives, where it was rejected as a conflict with a live reference. A
  per-instantiation clone judges a sibling clone's result by its template's
  declared type, and a call through a bound whose witness returns a view now
  derives that view, so `MOJITO_VERIFY_TEMPLATE_FACTS=1` agrees on
  `List.pop` and `List._get_copy` over such elements
  (`assets/ok/interior_dest_rebind_releases_loans.mojo`).
- A `hasher.update(x)` or `hasher._update_with_simd(x)` call in a generic
  method whose own `H: Hasher` an instance binds to a nominal hasher such as
  `AHasher` now derives as the struct's own method call, as the instance's
  own check selects it, instead of the checker builtin the bound proved, so
  `MOJITO_VERIFY_TEMPLATE_FACTS=1` agrees on
  `assets/ok/template_method_simd_construction.mojo`. A multi-lane vector
  argument there still keeps the clone check.
- A call through a bound whose instance type overloads the requirement at
  one arity now derives wherever the clone check's own ranking decides it on
  the recorded argument types: past the conversion count, the copies a
  `var` parameter makes of a place, a member's own binders, and the
  `SIMD`-pattern and receiver tie-breaks now rank, and an operator's or a
  call's argument is ranked on its own type. A rival the recorded types
  cannot rank still keeps the clone check.
- A pack query (`ParamKind::PackQuery`) now names its pack by the binder's
  identity rather than its spelling, so the Tuple closedness check no longer
  counts it as bound by an enclosing pack that merely shares the spelling.
  MIR text writes the pack as a `binder` record, which moves the textual
  format to schema 1.6; artifacts of schema 1.0 through 1.5 still load, a
  spelled pack reading as an unbound reference.
- A deferred slot (`CtValue::Deferred`) now names the binder whose slot it
  fills by its declaration's identity rather than its spelling, so the Tuple
  closedness check no longer counts it as bound by an enclosing value binder
  that merely shares the spelling. The elaborator's markers for a name that
  is no parameter are a separate `CtValue::Marker`. MIR text writes the slot
  as a `binder` record and the marker as `ct_marker`, which moves the
  textual format to schema 1.5; artifacts of schema 1.0 through 1.4 still
  load, a spelled slot reading as an unbound reference.
- The conformances a `where` clause guarantees inside its declaration's
  body, and those a `comptime if conforms_to(...)` arm proves, are recorded
  against the binder they name rather than its spelling, so two
  same-spelled binders no longer share an assumption. A struct's
  conditional conformance is now judged as its compiled condition under
  arguments bound to the struct's own binders.
- A generic method call's compile-time arguments are kept in declaration
  order rather than keyed by spelling, and its availability clause binds
  them and the struct's arguments each to their own binders, so a method
  binder and a struct binder of the same spelling no longer share one
  argument. Every availability check over a variadic struct's positional
  element list now binds its pack to the whole list.
- The comptime elaborator's parameter metadata now names each binder by
  the declaration it belongs to (its template, or `Struct.method` for a
  method's own binder) rather than one shared `$elaborated` owner, so two
  declarations' binders at the same slot no longer share an identity.
- Two overloads of one generic struct method whose parameter slots agree no
  longer share their binders: an overloaded method owns them under the
  symbol its template lowers to (`Box.pick$ov$…`), which MIR text spells as
  the binder's owner, and every clone of an overload shares its template's.
- A compiled `where` clause now names each operand by its declaration's
  identity rather than its spelling, and is judged under arguments keyed by
  binder, so a clause's `T` is the binder of the declaration it was written
  on. A callable default naming an earlier parameter carries that identity
  too. Both are written to MIR text by `owner` and `slot`, which moves the
  textual format to schema 1.4; artifacts of schema 1.0 through 1.3 still
  load, a spelled operand reading as the binder of that spelling in its own
  parameter list.
- Native monomorphization now solves and substitutes a type parameter by
  its declaration's identity, so two binders that share a spelling inside
  one instance no longer share a solution. MIR carries that identity on a
  type-parameter construction and on a type argument forwarding an
  enclosing binder (`hash[Self.H](key)`), and the textual format is schema
  1.3; artifacts of schema 1.0 through 1.2 still load.
- A generic body's outward store whose source is a union of its
  parameters (a value borrowed through a `ref[origin_of(a, b)]` result) now
  keeps that transfer for its instances, each dropping the parameters that
  are plain data there. Such a transfer used to refuse capture; the one
  known body that records it is still refused for its reference-typed call
  result.
- A bundled generic `def` called with a loan-carrying type argument
  (`alloc(Layout[Span[Int, origin_of(xs)]](count=2))`, `dealloc`) now gets
  its own clone, as a user function's call does, with the argument's origin
  slots bound to binders inferred from the call's arguments. Such calls used
  to run the template's erased body.
- A generic method called with a loan-carrying argument of its own
  (`k.keep[Span[Int, origin_of(xs)]](v)`), on a plain struct, a generic
  instance, or a static receiver, now gets a per-call clone whose origin
  binders are inferred from its arguments. Such calls used to run the
  template's erased body.
- An instance of a bundled struct over a loan-carrying argument
  (`List[Span[Int, origin_of(xs)]]`, `Dict[Int, Span[Int, origin_of(xs)]]`)
  now gets its own method clones, as a user struct's instance does, and
  each clone of a certified method reuses its checked template's facts.
  Such instances used to run the template's erased body.
- A generic body storing a value of a type parameter outward
  (`self.item = value^`) now keeps that store's transfer for its instances:
  an instance whose argument carries a loan publishes it, as its own check
  would. A derived instance used to drop it.
- A trait-bound `def` that constructs a struct and calls a method on it
  with a value argument, such as the bundled `hash_seeded` building an
  `AHasher` and feeding it `value`, now reuses its checked template's facts
  in every instance. Such functions used to be checked again per instance.
- A struct method binding or returning an element of a tuple-typed local
  by value (`var head = entry[0]`, `return entry[0]` from a
  `Tuple[Self.T, Int]`) now reuses its checked template's facts in every
  instance, which owes the element's copy at its own type. Such methods
  used to be checked again per instance.
- A struct method updating a field or a `mut` parameter through its
  in-place dunder (`self.total += x` on a bound `T`, `self.meter +=
  Meter(1)`, `into += x`) now reuses its checked template's facts in every
  instance: a dunder dispatched through the bound is re-selected on the
  instance's type, and a struct's own dunder retargets to the instance's
  clone. Such methods used to be checked again per instance.
- A struct method calling a method on a subscripted element
  (`self.items[i] = self.items[j].copy()`, `other.entries[i].key.copy()`),
  or taking the built-in `len` of a parameter or its field, now reuses its
  checked template's facts in every instance, so the bundled `Dict.update`
  does too. Such methods used to be checked again per instance.
- A struct method keyed on its own `Int` or `Bool` value binder
  (`def scaled[n: Int](self)`) is now checked once with the value symbolic,
  and every clone reuses those facts: a per-call clone reads the value as
  the literal it was folded to, and a per-instantiation clone keeps the
  binder as its own compile-time parameter. Such clones used to be checked
  again per call and per instantiation.
- A struct keyed on a `DType` or vector value, which the elaborator
  specializes whole per value, now has every member checked once by source
  validation with the value symbolic, and each specialization's members
  reuse those facts, the bundled `AHasher`'s SIMD leaves and the ranges'
  `__iter__` included. Such members used to be checked again per
  specialization, since no template was kept for them.
- A `Hasher`'s `_update_with_simd(mut self, value: SIMD[_, _])` is now
  checked once by source validation, with the wildcard parameter viewed as
  a lane-shaped vector, and every per-call leaf clone reuses those facts,
  folding the lane's dtype and width into the reinterpretation and lane
  count the template left open. Such clones used to be checked again per
  leaf type; a body spelling `value.to_bits()` with its default target
  still is.
- A member of a struct the elaborator specializes whole (`Tuple$t2[…]`, a
  `DType`-keyed range, a keyed `AHasher`) is now traced to its template, and
  the bundled `Tuple`'s `__len__`, comparisons, `__hash__`, and `write_to`
  reuse the template's checked facts, each instance reading its elements at
  its own types. Such members used to be checked again per instance, with
  no refusal counted.
- A struct method whose operator has a literal or closed scalar left operand
  (`1 + self.low`, `n + self.low`), dispatching the right operand's reflected
  dunder, now reuses the template's checked facts. Such methods used to be checked again per instance.
- A struct method calling a method on a parameter holding a struct, or on
  its field (`self.item == value.head()`, `other.items.__len__()`), now
  reuses the template's checked facts. Such methods used to be checked again
  per instance.
- A struct method calling a static method of a generic struct
  (`Counter[Self.T].start(n)`, `Pair.keep(local^)`, or `.twice(self.item)`
  against an expected `Pair[Self.T]`) now reuses the template's checked
  facts. Such methods used to be checked again per instance.
- A struct method declaring a nested `def` over the struct parameter's type
  (`def same(x: Self.T) -> Self.T`), with a capture-all default (`{imm}`), a
  `ref`, transferred, or `self` capture, or a nested `def` inside another now
  reuses the template's checked facts. Such methods used to be checked again
  per instance.
- A struct method passing a string literal to a method's `StringSpan` or
  `String` parameter (`self.name.rstrip(" z")`) now reuses the template's
  checked facts. Such methods used to be checked again per instance.
- A struct method reading or writing a field of a field of `self`
  (`self.scaler.base`, `self.inner.count += v`), or reading a field of a
  struct parameter (`entry._hash`), now reuses the template's checked facts;
  the bundled `Dict._append_new` and the owned `Set.__iter__` derive. Such
  methods used to be checked again per instance.
- A compile-time-keyed `def` constructing a `SIMD` value whose dtype or
  width names its own value binder (`SIMD[dt, w](v)`, `Scalar[dt](v)` under
  `comptime if`) now reuses the template's checked facts, each instance
  taking the construction's dimensions from its substituted type; a `DType`
  binder no longer keeps such a body out. A surviving trait-bound `def`
  constructing a closed `SIMD` value (`UInt8(1)`) reuses them too. Such
  bodies used to be checked again per instance.
- A struct method calling a generic module function on a value of the
  struct's parameter type (`hash(e)` in `Set.__hash__`, `hash(self.value)`)
  now reuses the template's checked facts, keeping the overload the template
  bound. Such methods used to be checked again per instance, and an `Int`
  instance could rank a concrete `pick(x: Int)` above the generic member
  the pinned Mojo binds.
- An initializer collecting `var *values` (`def __init__(out self, var
  *values: Self.T)`), optionally beside a `None` default, now reuses the
  template's checked facts, as do the bundled `List` and `Set` literal
  initializers' templates. Such methods used to be checked again per
  instance.
- A `var self` method returning its receiver (`return self^`), such as the
  owned `List` iterator's `__iter__`, now reuses the template's checked
  facts, the transfer owing `Movable` at each instance's type. Such methods
  used to be checked again per instance.
- A trait-bound module-level `def` taking a `var` parameter
  (`def dealloc[T: AnyType](var allocation: Allocation[T], /)`) now reuses
  the template's checked facts: the parameter may be consumed through a
  method taking its receiver (`allocation^.unsafe_leak()`), transferred, or
  handed on, and an untracked pointer local freed. Such bodies used to be
  checked again per instance.
- A `def` keyed on a type and a scalar value parameter with no compile-time
  control flow (`def scaled[T: Copyable, n: Int](x: T) -> Int`) now reuses
  the template's checked facts, each instance reading the value as the
  literal it folded to. Such bodies used to be checked again per instance.
- A `rebind`-keyed method of a generic struct (`rebind[Int](self.value)`
  under `comptime if Self.T == Int`) now reuses the template's checked facts,
  each instance discharging the `rebind`'s equality at its own type. Such
  methods used to be checked again per instance.
- Integer arithmetic over folded `comptime for` variables, value parameters,
  and literals alone (`sum += i * 10 + j`, `acc += -i`) now reuses the
  template's checked facts: the instance's whole expression is an
  `IntLiteral` materialized to the template's `Int`. Such bodies used to be
  checked again per instance.
- A method or module-level `def` that raises a bare string literal, or calls
  a raising method or function, now reuses the template's checked facts; such
  bodies, and any module-level `def` that raises, used to be checked again
  as a clone per instance.

- A method returning a field of a subscripted element as a reference
  (`return self.entries[i].value`, as `Dict.__getitem__` does) now reuses the
  template's checked facts; it used to be checked again as a clone per
  instance.

- A method wrapping a sibling's immutable view in a construction
  (`return KeyView(self.entries())` over an `ImmOrigin(origin_of(self))`
  result) now reuses the template's checked facts; it used to be checked
  again as a clone per instance, refused with "a construction binds an origin
  immutably".

- A method calling another method on a call's temporary result
  (`return self.entries().size()`, `self.name.strip().upper()`) now reuses
  the template's checked facts; it used to be checked again as a clone per
  instance.

- A method binding a view annotated with an inferred origin
  (`var span: Span[Self.T, _] = self.items`), or handing a place to a
  sibling's view parameter, now reuses the template's checked facts; it used
  to be checked again as a clone per instance.

- A method calling a generic module function with explicit type arguments
  (`self.data = unsafe_alloc[Self.T](n)`), and a construction handed an
  untracked pointer field, now reuse the template's checked facts, so the
  owned `List.__iter__` and `Optional.__iter__` and `List._realloc` are no
  longer checked again per instance.

- A call to another struct's overloaded method on a place built over the
  struct parameter (`self.items.extend(self.items.copy())` in a user struct
  over `List[Self.T]`, or on a `var` local) now reuses the template's checked
  facts; it used to be checked again as a clone, refused with "a called
  method's clone family has no member for the selected overload".

- A call through a trait bound whose witness belongs to a generic struct
  now reuses the template's checked facts when a concrete hasher bakes the
  witness's `[H: Hasher]` binder (naming the per-call clone keyed by the
  instance and the call) and when the witness is a synthesized
  `Copyable.copy` (`List._get_copy` over `DictEntry`); both used to be
  checked again as a clone.

- A call through a trait bound whose instance type overloads the
  requirement with members of one arity (`total(self, by: Int)` beside
  `total(self, by: String)`) now reuses the template's checked facts,
  ranking the members on the recorded argument types; it used to be
  checked again as a clone.

- A `@fieldwise_init` struct now constructs from keyword arguments naming
  its fields (`P(b=True, a=1)`, `Pair[Int, Bool](storage=(1, True))`,
  `Pair(storage=(2, False))`), in any order and after leading positionals,
  evaluated in source order as the pinned Mojo does; each used to report
  "'P' expects 2 argument(s), got 0".

- `len` now answers a SIMD vector's lane count (`1` for a scalar alias and
  `Float64`), and a `SIMD[dt, _]` local annotation takes its width from the
  initializer, so `var v: SIMD[DType.int32, _] = SIMD[DType.int32, 4](...)`
  then `len(v)` prints `4`; both used to report "no matching function in
  call to 'len'", and a `Float64` initializer under `SIMD[DType.float64, _]`
  used to be a type mismatch.

- A method keyed by its own `DType` or SIMD-width parameter may now
  construct a vector at that lane (`def make[dt: DType](self, x: Int) ->
  Scalar[dt]: return Scalar[dt](x)`), on a plain or generic struct; the
  program used to fail with "register r0 has no checked type".

- A method overloaded on a `Scalar[dt]` pattern and a bare type parameter
  (`def kind[dt: DType](self, a: Scalar[dt])` beside `def kind[T:
  Copyable](self, a: T)`) now selects the pattern for an argument with a
  lane, as a free call already did and the pinned Mojo does; the call used
  to be ambiguous.

- A `Bool` argument now binds a `Scalar[dt]` pattern's lane at
  `DType.bool` (`kind(True)` against `def kind[dt: DType](a:
  Scalar[dt])`), converting into `Scalar[DType.bool]` as the pinned Mojo
  does; it used to report "cannot infer type parameter 'dt'". Beside a
  bare `T` overload the conversion still loses to binding `T`.

- An instance of a user template whose argument carries a loan through an
  origin-slotted struct (`Bag[Span[Int, origin_of(xs)]]`), and a user
  generic `def` over such an argument, are now cloned instead of keeping
  the erased path; the instance's constructors stay erased. Each origin slot becomes an origin binder the
  clone declares and infers per call from its receiver and arguments, and
  the clones derive their facts from the checked template, replaying its
  transfers on their own bindings.

- A per-call method clone (`b.echo[String](x)` on `Box[Int]`, or a generic
  method of a non-generic struct) now leaves an expansion trace naming the
  struct's and the method's own baked parameters. A clone of a checked
  template whose own binders are trait-bounded types derives its facts from
  that template instead of being checked again. SIMD-keyed hasher leaves and
  methods with their own value binders are traced but still checked.

- A checker pass now carries the facts of every body whose inputs are
  unchanged since the previous pass instead of inferring it again: each
  module-level `def` and struct method records the fact-store entries it
  wrote, the effect entries it read, and a hash of its syntax, and the next
  transfer pass or discovery round copies the entries when the record is
  clean and every read still matches. Hello World's six full body passes
  become one full pass and three cheap ones (2.3 s to 1.5 s release, 15 s
  to 7.9 s debug on the reference machine). `MOJITO_BODY_FACT_REUSE=0` and
  `Compiler::with_body_fact_reuse(false)` infer everything, and
  `TemplateStats::carried` lists the carried bodies.

### Fixed

- A constructor whose `where` clause fails for the constructed type is no
  longer selected. `Box[Plain]()` over `def __init__(out self) where
  conforms_to(Self.T, Defaultable)` used to run the initializer, and now
  reports the violated clause, as the pin does
  (`assets/type_error/unavailable_initializer.mojo`,
  `assets/type_error/tuple_default_element_not_defaultable.mojo`).
- A module `def` returning `ref T` is read through wherever a value is
  wanted, as upstream reads it and as a method's reference result already
  was: `var r = pick(a, b)` owns a copy (and demands `ImplicitlyCopyable`,
  as the pin does), `print(pick(w))`, `pick(x, y) + 1`, `r = pick(a, b)`,
  and `pick(a, b).copy()` run. The local used to hold the reference
  itself, so `self.value = r^` stored the handle (`<ref 3:1>`, or a stale
  frame), and the read is taken from a slot that loans the arguments, so
  one read at an argument's last use no longer prints `None`.
- A value rebound only after a `try` in a loop body, and read past the
  loop, now survives a handler's `break` or `continue`: drop elaboration
  counted it dead at the loop body's entry, missing the escape edge, and the
  VM read an empty slot (`operator Add is not defined for None and Int`).
- A free function's own origin binder is now bound in its result as in its
  parameters: `f(Span(xs))` on `def f[o: Origin](s: Span[Int, o]) ->
  Span[Int, o]` has type `Span[Int, origin_of(xs)]`, where it used to keep
  the unbound binder and reject a later use at the argument's origin.
- A generic function's clone over a loan-carrying argument no longer
  reports aliasing between two arguments that carry only the argument's
  origin (`pick(l, Span(xs))` on `def pick[T](items: List[T], default: T)`).
- A generic function whose loan-carrying type parameter reaches only its
  result (`mk[Span[Int, origin_of(xs)]](1)`) now runs its erased body
  instead of a clone whose origin binder nothing binds.

- A method returning a view of a field (`-> Span[Self.T,
  origin_of(self.items)]`) on an instance over a loan-carrying argument
  (`Bag[Span[Int, origin_of(xs)]]`) now compiles. Deriving it failed with
  "template derivation lost a struct origin", and checking it failed with
  "no constructor overload matches": a constructor, a fieldwise
  construction, an alias application, or a bound dispatch substituted its
  own origin binders after the type arguments, and so rewrote an argument's
  binder of the same slot number.
- A read of a loan-free value no longer counts as aliasing a `mut self`
  receiver that carries a loan (`self._rehash(self.nbuckets)`), and the
  bundled `Dict` no longer panics with "already borrowed" when a view
  result's origins are gathered.
- A method with its own compile-time parameter on a struct keyed on a value
  (`def rep[dt: DType](self, a: SIMD[dt, Self.n])` on `Width[n: Int]`) now
  clones per call, explicit (`w.rep[DType.int16](v)`) or inferred. Such a
  call used to keep the template, failing MIR verification with "'dt' has
  register type Error", or trapping as an unspecialized method.

- A struct method keyed on its own `DType` may forward that lane to a
  `DType`-keyed `def` (`return helper[dt](x)`), and each per-call clone
  reaches the def's clone for its lane. Elaborating the method's template
  used to reject the bracket with "'dt' is not a compile-time type".

- A view moved into a container (`views.append(Span(xs))`) no longer makes
  the viewed list exclusive: reading `xs` while the container holds the view
  used to fail with "conflicts with live reference". A forwarded value's
  loans stay shared at the destination; only a borrowed parameter's own
  place, or a mutable reference, is lent exclusively.
- A call's own origin binder inside a type argument
  (`Span[Span[Int, o], _]`) is no longer captured by the receiver struct's
  origin slot at the same declaration index, which made
  `out.append(views[0].copy())` fail with "no overload matches".

- A generic body can construct a variadic struct from a nested variadic
  construction over its own parameters (`Outer[T, Int](Variant[T, Int](x^))`),
  which used to fail with "expected Variant[T, Int], found Variant[(T, Int)]":
  a constructor parameter spreading the struct's pack now binds it as one
  list, the spelling every annotation of a user pack already resolves to.

- A pack-keyed body can forward its pack whole into a pack-keyed method
  (`self.take(*a)`) or into `print` (`print(*a)`, with or without `sep` and
  `end`), which used to fail with "'a' is not a compile-time type" and
  "call spread outside a specialized type pack". A method template that
  reads `a.__len__()` of its own collector no longer fails to elaborate.

- One overload of a type-pack `def` can forward its collected pack whole to
  a same-named sibling (`def tally[*Ts](first: Int, *rest: *Ts)` returning
  `first + tally(*rest)`), which used to fail with "no overload matches the
  supplied arguments". The clone's forward binds the one declaration whose
  collector the spread follows, as the pinned Mojo binds it when it checks
  the template (`assets/ok/pack_overload_sibling_forward.mojo`).
- Two overloads of one module-level generic `def` no longer share their
  parameter binders: an overloaded `def` owns them under the symbol it lowers
  to (`tally$ov$…`), which MIR text now spells as the binder's owner.
- A specialization keyed by a string or a literal value (`s`, `I`, `F`) is
  now rebuilt by `demangle_specialization`, so its unqualified spelling
  expands the baked value and the conformance oracle consults its template.
  The Tuple closedness check reads a type parameter or a value reference by
  identity, so a nested contract's `T` no longer counts as bound by an
  enclosing `T`.
- A surviving trait-bound module-level `def` whose body holds a whole value
  of its parameter type — a local copied from a parameter, transferred into
  another local or into a `-> T` result, or handed by value to a direct
  call — or iterates a `List[T]` parameter now derives its instances from
  the checked template instead of keeping the clone check. An overloaded
  call inside such a body is therefore bound once, as the pinned Mojo binds
  it: `outer(3)` with `var kept = x` and `return pick(kept)` prints 2 where
  re-checking the `Int` instance used to rank the set again and print 1.
- A method of a generic struct that reads a scalar element of a tuple-typed
  local at a literal index (`var start = bounds[0]`), including the tuple
  `slice.indices(n)` returns, now derives its instances from the checked
  template. Such bodies used to keep the clone check, among them the
  `StridedSlice` overload of `List.__getitem__`.
- A method of a generic struct that copies its read `self` into a local
  (`var result = self`) and then reads and stores the local's fields
  (`result.size -= 1`, `result.value = value^`, or `Span`'s contiguous slice
  storing `result._data.unsafe_offset(start)`) now derives its instances from
  the checked template. Such bodies used to keep the clone check.
- Augmented assignment on a value of a bare type parameter whose bound
  requires the in-place dunder (`a += b` with `T: Accum`, a trait declaring
  `__iadd__`, or `self.items[i] += x` over `List[Self.T]`) now dispatches
  through the bound, as the call `a.__iadd__(b)` already did. It used to be
  rejected with "operator '+' is not defined for T and T".
- A method of a generic struct storing augmented through a subscripted value
  built over its parameter (`self.box[i] += 1` with `box: Box[Self.T]`, or
  `self[i] += 2`), or through an element of the bare parameter type that
  updates with its bound's `__iadd__`, now derives its instances from the
  checked template: the value getter is realized on the instance's receiver
  and the dunder's witness re-selected on the instance's element type. Such
  bodies used to keep the clone check.
- A direct `v.__hash__(hasher)` on a sized scalar or a `SIMD` vector
  (`UInt8(3).__hash__(h)`, `SIMD[DType.float32, 2](...).__hash__(h)`) now
  checks and feeds the hasher as `hash(v)` does. It used to report "has no
  method '__hash__'", so a generic struct's `self.value.__hash__(hasher)` ran
  for such an instance while `mojito check` and template verification
  rejected the same body.
- A method of a generic struct whose operator takes a call's result, another
  operator, or a right-hand literal as an operand (`self.first_value() ==
  value`, `self.low + (self.high + self.low)`, `self.low + 5`) now derives its
  instances from the checked template, dispatching the dunder again from both
  operand types and keeping the copy or literal conversion the template's own
  dispatch recorded. Such bodies used to keep the clone check because the
  method grammar admitted an operator only over two places.
- A method of a generic struct that calls a static method of a non-generic
  struct on its type — `Color.pick(n)`, or a leading-dot `.of(n)` the
  expected type resolves — now derives its instances from the checked
  template, which carries the leading-dot base across. Such bodies used to
  keep the clone check because the method grammar admitted no static call
  and the contextual base had no derivation recipe.
- A method of a generic struct that declares and calls a nested `def` now
  derives its instances from the checked template: the nested signature is
  closed scalars, keyed by its statement, and each `imm`, `mut`, or `var`
  capture of a local or parameter, of any type, is rebound to the
  instance's own binding. Such bodies used to keep the clone check because
  a nested declaration's facts and its capture list had no derivation
  recipe.
- A method that loops, or builds a comprehension, over `range(...)` in a
  generic struct now derives its instances from the checked template: the
  call selects a member of `range`'s overload set from closed scalar
  arguments, which no instance changes, and the loop's protocol is selected
  again from the range's type. Such bodies used to keep the clone check
  because the method grammar admitted no direct call as an iterable and no
  overloaded direct call at all.
- A compile-time-keyed method of an ordinary generic struct — `comptime if
  Self.T == Int` arms, a `comptime for` over a literal range with its
  variable read as a runtime value and a local declared inside — now derives
  its instances from the template source validation checked: each instance
  keeps the arms the elaborator selected, once per unrolled copy, and its
  trace names its own first statement. Such bodies used to keep the clone
  check because a validated method had no derivation class, and a body
  opening with a `comptime if` left a trace no instance matched.
- A method that hands a hasher a multi-lane vector
  (`hasher._update_with_simd(SIMD[DType.uint8, 4](1, 2, 3, 4))`) now derives
  its instances from the checked template: the template keeps each closed
  leaf it hashed, and every instance records it again. Such bodies used to
  keep the clone check whenever their check was the first to hash that
  vector type.
- A method that computes over closed `SIMD` values (`UInt64`, `Int32`, a
  `SIMD[DType.uint8, 4]` field) now derives its instances from the checked
  template: an operator, an augmented assignment, a local, a field store, and
  a bool-lane condition (`while n != 0` over a `UInt32`) over one read as
  they do over `Int`, so `Dict.__hash__` derives. Such bodies used to keep
  the clone check because the method grammar's only scalars were `Int`,
  `UInt`, `Bool`, and `Float64`.
- A method that constructs a value of its own trait-bounded binder
  (`var inner = H()` in `def __hash__[H: Hasher](self, mut hasher: H)`) and
  consumes it with `inner^.finish()` now derives its instances from the
  checked template: every clone keeps that binder symbolic, so it records the
  same construction. Such bodies used to keep the clone check because a
  binder's construction had no derivation recipe.
- A method holding a `with` statement now derives its instances from the
  checked template: the template keeps the form its manager's declarations
  select, and each instance builds its own desugar from its syntax, whose
  synthesized nodes now take identities derived from the statement's. Such
  bodies used to keep the clone check because the desugar's nodes took fresh
  identities no instance could trace.
- A method that builds a list, set, or dict comprehension over a place of the
  body or a sibling call's result (`[x for x in self.items if x != drop]`)
  now derives its instances from the checked template: each clause selects
  its iterator protocol again at the instance's type, and each binder is
  declared from that protocol's binding. Such bodies used to keep the clone
  check because comprehension binders had no derivation recipe.
- A method that binds a view over an owned interior of a field or local
  (`var view = self.name.strip()`) now derives its instances from the
  checked template, keeping the interior projection and the immutable origin
  binder the callee's declared return origin records at the call. Such
  bodies used to keep the clone check because view-result interiors had no
  derivation recipe.
- A method that calls another struct's method with explicit compile-time
  arguments (`self.scaler.scaled[3](x)`) now derives its instances from the
  checked template, keeping the per-call clone the call requests as its
  target. Such bodies used to keep the clone check because parameterized
  method calls had no derivation recipe.
- A method that unpacks a tuple into `var` locals — from a parameter, a
  local, a field of `self`, or a sibling call's result — now derives its
  instances from the checked template: the instance builds the element reads
  again from its substituted tuple type and its own binding of the place.
  Such bodies used to keep the clone check because tuple unpackings had no
  derivation recipe. A derived instance also names the generated `Tuple` for
  a closed public one, as its clone check does, where it used to keep the
  generic `Tuple[Int, Int]` spelling.
- A method whose `if` or `while` tests a struct through `__bool__` — a field
  of `self`, a parameter, or a local — now derives its instances from the
  checked template: the instance judges each such condition again at its own
  type. `Bool(x)` of a closed struct place and a reference call on a `var`
  local derive beside it. Such bodies, like `OptionalReg.or_else` and
  `List.index`, used to keep the clone check because truthiness conditions
  had no derivation recipe.
- A method whose body constructs a closed `SIMD` or scalar-alias value
  (`UInt8(1)`) and hands it to a hasher, to a by-value parameter, or to a
  `var` local now derives its instances from the checked template: the
  construction's dtype and width are closed, so the instance installs the
  template's record as it stands. Such bodies, like `Optional.__hash__`, used
  to keep the clone check because SIMD constructions had no derivation
  recipe.
- A method whose body holds a runtime `for` — over a field of `self`, over
  `self` through its own iterator, over a sibling call's temporary, or owned
  over a local — now derives its instances from the checked template: the
  instance selects the iterator protocol again from its substituted iterable
  type and resolves it against its own binding of the source. A built-in
  scalar conversion of a closed value (`Int(key_hash)`) derives beside it.
  Such bodies, like `Dict._find_index`, used to keep the clone check because
  the iterator protocol had no derivation recipe.
- A method whose body calls a consuming method on a named place it does not
  own, which the call copies first (`slice.start.or_else(0)`, or a `var self`
  requirement through a bound on a parameter), now derives its
  per-instantiation clones from the checked template instead of keeping the
  clone check; the instance owes the copy at its own type. A direct call of a
  non-generic module function taking scalars derives beside it, so the
  `ContiguousSlice` overload of `List.__getitem__` is covered.

- A method whose body consumes a value through a method that takes its
  receiver — a named `deinit self` destructor on a local or on a field of a
  consumed `self` (`entry^.reap_value()`, `self._alloc^.unsafe_leak()`), a
  `var self` method, or a `deinit self` requirement through a bound — now
  derives its per-instantiation clones from the checked template instead of
  keeping the clone check. Which methods a struct declares as destructors
  does not change with its arguments; through a bound, the instance's own
  struct is asked. `Dict.pop`, `Dict.clear_with`, `Dict.deinit_with`, and
  `Set.difference_update` are among the bundled bodies this covers.

- A `var` declared inside a `comptime for` body that unrolls more than once
  is now one binding per iteration, as upstream: each unrolled copy that
  declares a binding is its own scope, so the body may also shadow an outer
  local, and an owning local is destroyed within its iteration. The copies
  used to share one scope, and the second declaration was rejected as
  already declared. A selected `comptime if` arm is likewise a scope, so its
  `var` no longer collides with a later one of the same name. A compile-time-keyed `def` holding such a scalar local
  now derives its instances from the checked template, each copy of the
  declaration a local of its own, instead of keeping the clone check.

- A checked template whose body replays a callee's transfer summary
  (`self.items.append(value^)`) is now reused from its own facts in every
  later transfer round instead of being inferred again, and its
  per-instantiation clones replay the same transfers on their own bindings:
  the template keeps each call transfer, the origins it merged, and the
  effect its frame derived by template owner, and an instance keeps a source
  only while its binding's type may still carry a loan. A plain-data
  instance records nothing at the call, as before.

- A method of a struct with a scalar value parameter or an origin parameter
  (`Array`, `Span`, a user `Grid[T, rows: Int]`) is now certified as a
  checked template, so its facts are reused in every later pass instead of
  the body being inferred again: a `Self.rows` read is a runtime read of the
  reified value on the erased path such a struct keeps, and a pointer field
  whose provenance is the struct's own origin names no checker-local place.
  A receiver origin naming one of the method's own origin binders
  (`ref [o] self`) is admitted as a signature fact, so each per-instantiation
  clone of such a method derives too.

- A compile-time-keyed `def` clone now inherits its checked template's facts
  when the body reads a `comptime for` variable or a scalar value parameter
  as a runtime value (`sum += i`, `acc = acc * 2 + i`, `bump(i)`,
  `print(i)`, `var seen = flag`): the literal the elaborator folds there
  takes a literal's facts and its materialization to the template's type.
  Such a body used to keep the clone check.

- A per-instantiation method clone now inherits its checked template's facts
  when its instance argument is a struct declaring fields of its own
  parameter types (`List[DictEntry[Int, String, H]]`,
  `Optional[DictEntry[…]]`): the argument's fields are judged at its own
  arguments rather than as declared, which read every such struct as
  possibly carrying a loan. The clone check's loan bookkeeping narrows the
  same way.

- A per-instantiation method clone now inherits its checked template's facts
  when the method raises: a bare or typed `raises` declaration whose `raise`
  names `Error("…")` or a construction of the declared error type
  (`Optional.__getitem__`, `Dict.popitem`). Such a body used to keep the
  clone check.

- A per-instantiation method clone now inherits its checked template's facts
  when the body calls a sibling method whose result is a view over `self`
  (`_DictKeyIter(self.items())` in `Dict.keys`, `Dict.values`, and
  `Dict.__iter__`): the call's loan and the origins its result binds are kept
  by template owner and rebuilt on the instance's receiver. Such a body used
  to keep the clone check.

- A per-instantiation method clone now inherits its checked template's facts
  when the body binds an annotated `var` whose value converts to a type built
  over the struct's parameter (`var label: Label[Self.T] = 4`,
  `var box: Wrapper[Self.T] = self.item`), or whose annotation is the value's
  own type: the instance selects the constructor again at its own types. Such
  a body used to keep the clone check.

- A per-instantiation method clone now inherits its checked template's facts
  when the body binds a `var` local whose type is built over the struct's
  parameter (`var result = List[Self.T]()`, `var view = View(self.items)`):
  the local's deletability is judged again at the instance's type, and the
  local may be a method call's receiver or `len`'s operand. Such a body used
  to keep the clone check; `List.__mul__`, `List.__imul__`, and
  `Dict.__or__` now derive.

- A per-instantiation method clone now inherits its checked template's facts
  for an augmented element store through a value getter and a setter
  (`self.table[i] += 1`), for a struct element's in-place `__iadd__`
  through either kind of getter (`self.counters[i] += 3`), and for a store
  on `self` itself (`self[k] = v^`). Each used to keep the clone check. A
  subscripted value or an element whose type is built over a parameter still
  does (`docs/roadmap.md` §1).

- A per-instantiation method clone now inherits its checked template's facts
  when the body hands a reference on as a call argument: a `ref` local, a
  field reached through one, or a reference call's result (`self.items[i]`),
  read where it lies by a read parameter, copied into a `var` one, or kept by
  a `mut` one, and a place lent to a hand-written constructor's `ref`
  parameter. Such a body used to keep the clone check. A generic view built
  that way into a local still does (`docs/roadmap.md` §1).

- A per-instantiation method clone now inherits its checked template's facts
  for a call through the struct parameter's bound whatever shape the
  instance's witness takes: a member of an overload set its arity selects, a
  `[H: Hasher]` witness handed a concrete hasher (which names the per-call
  clone once it is minted), a `mut self` requirement, and a `var self` one
  consumed through a `^` transfer. A named place of a closed type handed to
  such a call no longer leaves the template uncertified either. Each used to
  keep the clone check. An overload set whose members share an arity still
  does (`docs/roadmap.md` §1).

- A per-instantiation method clone now inherits its checked template's facts
  when the body puts any operator a trait names over two places of one
  parameter-typed type, not only a comparison: `a + b` under an arithmetic
  bound, and the bitwise and shift operators, derive, and so does an operator
  whose result is the operand's own type rather than a `Bool`. An instance
  whose dispatch adapts the operand derives as well — the implicit copy a
  by-value dunder parameter takes, the `@implicit` conversion an operand
  reaches its declared type through, and the `!=` an `Equatable` type serves
  with `__eq__` and a negation are each recorded at the instance's own types.
  Such a body used to keep the clone check. An operand that is not a place — a
  literal, a call's result, a nested operator — still does
  (`docs/roadmap.md` §1).

- A per-instantiation method clone now inherits its checked template's facts
  when the body stores a whole value through a field's `__setitem__`
  (`self.items[i] = value^`, a construction, a moved closed value) or stores a
  scalar element, whole or augmented, through the mutable reference a field's
  `__getitem__` yields (`self.counts[i] += 1`, `self.grid[i] = n` on a struct
  with no setter). Such a body used to keep the clone check: the setter's
  grammar demanded a scalar, and the augmented store's record embedded the
  getter's contract, which no recipe could substitute. An augmented store
  through a value getter and a setter, and a struct element's `+=` through
  `__iadd__`, still do (`docs/roadmap.md` §1).

- A body reading `reflect[T]` over a symbolic type is now validated from its
  template like every other compile-time-keyed body: a query over a struct is
  answered from the struct table, a query over a parameter is a dependent node,
  and a field type under the loop index is opaque until a `comptime if
  conforms_to(FT, ...)` arm proves a trait of it — which now licenses a plain
  parameter and a pack element in the same way, as the pinned Mojo does. An
  untaken arm in such a body used to go unreported until an instance selected
  it.
- A body that forwards its variadic pack to another callee (`inner(*a)`,
  `collect(30, *items^, tail=10)`, a method's own pack, `print(*a)`) is now
  validated from its template like every other pack-keyed body: the callee's
  pack binds to the caller's whole pack, so an untaken `comptime if` arm after
  the call is rejected as the pinned Mojo rejects it, where it used to be
  reported by nothing. A pack forwarded into a callee whose bound the caller's
  pack does not carry, into a homogeneous collector, into a regular parameter
  slot, or beside a second spread is rejected at the call.
- The ownership of a forwarded pack is now checked as the pinned Mojo checks
  it: an owned pack (`var *a`) forwards only with the `^` and only into an
  owned collector, and a read pack cannot be transferred. All four shapes used
  to run.
- A call inside a generic body is now bound once, while the body is checked
  with its parameters symbolic, as the pinned Mojo binds it. With `pick(x:
  Int)` beside `pick[T: Copyable](x: T)`, `outer[T](x)` calling `pick(x)`
  selects the generic overload for every instantiation, so `outer(3)` prints 2
  where it printed 1: re-checking the `Int` clone used to rank the set again.
  The fix holds for an instance derived from its checked template (below); an
  instance that still takes the clone check re-ranks, which
  `docs/roadmap.md` section 3 carries.
- Overload ranking now charges every candidate the implicit copy a place costs
  a `var` parameter, as the pinned Mojo does, so `h(s)` against
  `h(var a: String)` beside `h[T: Writable](a: T)` selects the generic
  overload while the rvalue `h(String("t"))` selects the `var` one. The copy
  ranks below one conversion and above the signature-length tie-break, a
  keyword slot and a field place cost it as a positional place does, and
  methods and constructors are ranked by the same term. A method is also
  ranked by its own compile-time parameters now, so a concrete method beats a
  generic sibling instead of reporting an ambiguous call. Among variadic
  candidates a collector that takes at least one argument outranks both the
  copy and the signature length, which fixes a second silent wrong answer.
- An owning temporary is now destroyed as soon as its consumer finishes,
  running its `__deinit__` as the pinned Mojo does, where it was never
  destroyed before. A method receiver (`B(3).show()`) dies right after the
  call returns, so `print(B(2).get())` prints `deinit 2` before `2`; an
  argument at a read parameter (`take(B(2))`, positional or keyword) dies once
  the callee returns; a temporary read for a field (`print(B(1).x)`,
  `var n = B(9).name`, `print(B(14).twin().name)`) dies after the instruction
  consuming the field, an owning field being copied out first. Chained
  temporaries, free-function results, and generic structs behave alike, on
  the VM and natively. A `var`/`deinit` receiver or parameter takes the
  temporary and destroys it once, in the callee. The checker records which
  arguments a call only borrows (`ReadTemporaryArgument`), so lowering never
  guesses a callee's conventions.
- A generic struct's method may now call a `def` whose `comptime if` keys on
  its own type parameter, as in the pinned Mojo: `def f(self): show(self.x)`
  in `struct Box[T]`, inferred or explicit (`show[Self.T](self.x)`), and the
  same call from a non-generic struct's generic method, no longer fails with
  "generic 'show' requires compile-time parameter 'T'". Each closed instance
  reaches the call through its own method clone, on the VM and natively.
- A generic struct's lifecycle methods now key on the instance: a
  single-signature `__init__`, a copy or move constructor, and `__deinit__`
  each mint a per-instantiation clone, and construction, copying, moving and
  destruction reach it. A `comptime if Self.T` in a constructor or destructor
  no longer aborts with "unspecialized type-keyed method", and such a body may
  call a compile-time-keyed `def`. Bundled `List`/`Dict`/`Optional` keep their
  lifecycle methods on the erased path, as does an overloaded constructor
  family.
- A generic `def`'s abstract body may now call a `def` whose `comptime if`
  keys on its own type parameter, as in the pinned Mojo: `def forward[T:
  Copyable](x: T): show(x)`, or `show[T](x)`, no longer fails with "generic
  'show' requires compile-time parameter 'T'" or "'T' is not a compile-time
  type", even when `forward` is never called. Callers several levels deep
  work too, on the VM and natively. Such a call from a generic struct's
  method, or from a generic `def` nested in non-generic code, is still
  rejected.
- An inferred call to a `@staticmethod` whose `comptime if` keys on its own
  type parameter now runs, as in the pinned Mojo: `S.show(3)` on `def
  show[T: Copyable](x: T)` prints `int` instead of aborting with
  "S.show: unspecialized type-keyed method", on the VM and natively. This
  holds on a parametric owner too (`B[Int].show(3)`, `B.make(1, "s")`). A
  generic method, static or instance, that calls such a method from two of
  its own instantiations no longer aborts either.
- A generic `def` whose `comptime if` keys on an inferred type parameter now
  runs, as in the pinned Mojo: `def show[T: Copyable](x: T)` with `comptime
  if T == Int`, called as `show(3)`, prints `int` instead of failing with
  "generic 'show' requires compile-time parameter 'T'", on the VM and
  natively. Inferred calls from inside other specializations and recursive
  inferred calls work too. An inferred call whose type stays symbolic, such
  as one from another generic's abstract body, is still rejected.
- A display of one capturing lambda now runs, as in the pinned Mojo: `var fns =
  [lambda (x: Int) {k} -> Int: x * k]; print(fns[0](2))` prints `6` instead of
  rejecting the element as a non-storable callable, on the VM and natively.
  A lambda bound straight into storage now keeps its reference captures
  borrowed. A `capturing[_]` local over `{k}` used to read a dropped `k`.
  Two capturing lambdas in one display, a transferred non-copyable capture,
  and reassigning or returning an aggregate that stores a closure are
  rejected.
- `SIMD` types and values now have upstream's `dtype` alias: `Int32.dtype`
  and `s.dtype` on a `SIMD[DType.int16, 4]` print `int32` and `int16`
  instead of failing with "has no field 'dtype'", at run time and in
  `comptime d = Int32.dtype`, on the VM and natively. A call receiver
  (`f().dtype`, and likewise `f().length`) still runs. `DType`'s static
  floating-point queries `mantissa_width`, `max_exponent`, `exponent_width`,
  and `exponent_bias` answer as the pin does, and reject a non-float dtype.
- `Float16` now exists, as in the pinned Mojo: `range(Float16(0.5),
  Float16(2.0), Float16(0.3))` runs instead of failing with "Undefined
  variable 'Float16'", yielding the half-precision fused `k * step + start`
  values. `Float16`, `SIMD[DType.float16, n]`, and `DType.float16` (code 79,
  `is_half_float()` True) compute, convert, compare, hash, and report
  `size_of` 2 bit-identically to the pin on the VM and natively. Values print
  their exact double view (`0.7998046875`), as `Float32` does, where the pin
  prints single-precision text (`0.7998047`).
- `DType` is now a runtime value, as in the pinned Mojo: `var x =
  DType.float32; print(x)` prints `float32` instead of failing with
  "Undefined variable 'DType'". A `DType` passes, returns, defaults, and
  stores in fields and collections; prints its name (`repr` spells
  `DType.float32`); compares with `==`/`!=`; hashes as upstream's one-byte
  code; and answers `is_integral`/`is_floating_point`/`is_signed`/
  `is_unsigned`/`is_numeric` at run time and in `comptime if`, on the VM and
  natively. A compile-time call returning a `DType` now keys `SIMD[c, n]` and
  `[dt: DType]` specializations.
- A field store below a `List` subscript (`xs[i].field = v`, `xs[i].n += 1`,
  `h.items[i].inner.n = v`, `xs[i][j].n = v`) now runs on both backends
  instead of failing MIR verification with "dynamic element projection
  requires checked indexed storage": the assignment target is lowered like a
  `ref` binding, evaluating the selected reference-returning `__getitem__`
  once into a hidden `ref` handle and storing through it, so a replaced
  droppable field is destroyed at the store as in the pinned Mojo.
- A whole value written through a reference now destroys the value it
  replaces, as in the pinned Mojo: reassigning a `mut` parameter or `self` in
  a `mut self` method runs the caller's old value's `__deinit__` at the
  assignment, and so does a store through a place pointer (`q[] = Inner(2)`)
  or a whole-variable `ref` binding, on the VM and natively. Such a write has
  no redefining `DefVar` to end the old value's live range, so drop
  elaboration splices a `DropPlace` before it — only where the whole subtree
  is intact, since a partially moved value cannot be dropped whole. Three
  `assets/ok` fixtures verified against the pin replace the two probes (exe
  ratchet 550 → 553), closing the `mut-parameter-reassignment-drop` and
  `pointer-deref-store-overwrite-drop` ledger rows and the first of section
  3's four prerequisites.

- A store into a field now destroys the value it replaces at the store, as in
  the pinned Mojo: `p.b = Inner(3)` runs the old value's `__deinit__` before
  the write, on the VM and natively, whether the field is reached from a
  local, a `mut self` receiver, a `mut` parameter, or a nested field chain. A
  constructor's first store into an `out self` field only initializes it and a
  second destroys the first, and a field moved out on some paths is destroyed
  only where it still holds a value. Two `assets/ok` fixtures verified against
  the pin replace the probe (exe ratchet 548 → 550), and the
  `field-store-overwrite-drop` ledger row is closed.

- A returned view no longer widens a field's origin to its holder's, as in
  the pinned Mojo: `return` judges a struct's origin tail against the return
  annotation resolved over the body's own places, so `-> View[origin_of(self)]`
  returning a view over `self.items`, and a free function's `-> View[origin_of(b)]`
  returning one over `b.items`, reject with upstream's "cannot implicitly
  convert 'View[origin_of(self.items)]' value to 'View[origin_of(self)]'",
  while the exact `-> View[origin_of(self.items)]` spelling is accepted. The
  bundled `Dict`, `Set`, and `StringDict` iterators hold a `Pointer` to their
  backing list rebound to the whole container's origin, as upstream's do.
  Three `assets/type_error` fixtures pin the rule against the pin, and three
  former extension fixtures move to `assets/ok`; the
  `return-origin-widening` ledger row is closed. A view stored in a direct
  `ref` field still widens (new `ref-field-return-origin-widening` row).

- A struct's origin arguments are now part of its checked identity, as in
  the pinned Mojo: `Ty::Struct` carries an origin tail (one entry per explicit
  `Origin` slot) that constructors bind from their arguments and view-returning
  calls bind from their return contracts, so rebinding a view local across
  origins (`p = P(Pointer(to=ys))` over a `P[origin_of(xs)]`), storing a box
  over another origin into a `RefBox[Self.origin]` field, and holding two
  receivers' views in one local reject with upstream's `cannot implicitly
  convert 'P[origin_of(ys)]' value to 'P[origin_of(xs)]'`. An origin-slotted
  struct nested as a type argument (`List[RefBox]`, `List[RefBox[_]]`) is not
  concrete in any position. A new argument-exclusivity rule judges the origins
  a callee's declared parameter types carry (`aliasing values passed mutably
  to 'sink' argument and passed mutably to 'box' argument in 'stash' call`),
  while a generic `append` stays accepted. The bundled iterators take
  upstream's `IteratorType[...] = Self` shape and `StringSpan.__eq__` its bare
  `rhs: StringSpan` parameter. Eight `assets/type_error` fixtures pin the
  rules against the pin; twenty-eight extension fixtures that pinned the old
  leniency are gone (exe ratchet 560 → 548), and the `erased-origin-parameter`
  ledger row is closed. A returned view's sub-origin still widens to the
  declared `origin_of(self)` (new `return-origin-widening` row).

- A write through the pointer field of a view a call returns
  (`make(xs).src[][0] = 9`, `h.view().src[][0] = 9`, a local bound from
  either, a field chain below it, and a whole store `make(b).src[] = Box(11)`)
  now follows the pinned Mojo: a struct-typed return annotation's origin
  slots (`-> P[origin_of(xs)]`) lower to a view-return contract that the call
  site resolves against its places with the callee's own capability, so a
  `mut` parameter or `mut self` writes through while a read parameter or
  receiver rejects with `expression must be mutable in assignment`. The call
  result holding the dereferenced pointer materializes as a hidden owned slot,
  which also fixes reads through such a temporary (`print(make(xs).src[][0])`
  died in the VM). An `ImmOrigin(o)` cast on a nested construction now
  survives the field chain (`Wrap(Cell[ImmOrigin(…)](…))` rejects at
  `w.cell.src[][0] = 1`). Six `assets/ok` and three `assets/type_error`
  fixtures pin the shapes against the pin.

- Materializing a temporary receiver no longer changes what its loan permits.
  The hidden slot a view-returning method's owning temporary is materialized
  into was lent unconditionally mutably, so a second view derived from the
  first — `String("abcdef").as_bytes()` sliced, or a peek through a
  `codepoint_slices()` iterator — was rejected as a conflicting access to the
  anonymous binding, though the same program over a named local is accepted.
  The checker now records on `MaterializeBorrowSource` the capability the
  borrow that forced the materialization needs (a read receiver lends
  immutably), and MIR lends the slot with it;
  `assets/ok/temporary_receiver_view.mojo` pins the two-view case.

### Changed

- A deferred slot (`CtValue::Deferred`) now names the binder whose slot it
  fills by its declaration's identity rather than its spelling, so the Tuple
  closedness check no longer counts it as bound by an enclosing value binder
  that merely shares the spelling. The elaborator's markers for a name that
  is no parameter are a separate `CtValue::Marker`. MIR text writes the slot
  as a `binder` record and the marker as `ct_marker`, which moves the
  textual format to schema 1.5; artifacts of schema 1.0 through 1.4 still
  load, a spelled slot reading as an unbound reference.
- The differential pin is now Mojo `1.2.0.dev2026092105` (upstream
  `26cfe94f40`, 2026-09-21), and two of its changes reach Mojito. A walrus
  updates a name already in scope and never introduces one, so `(n := 1)` on
  an unknown `n` is the same error as a var-less `n = 1`; upstream reports
  "use of unknown declaration 'n'" and made an implicit declaration a hard
  error in the same window. `String`/`StringSpan` `startswith`/`endswith` now
  normalize their `start` the way `find` does instead of comparing a found
  position with the raw argument, so a negative offset counts from the end
  (`"hello".startswith("llo", -3)` is `True`), the empty affix matches at any
  offset within the string, and one past the byte length matches nothing.
  `docs/mojo-nightly.md` carries the audit and what the window leaves
  unimplemented.

- A checked generic template is now the authority for the instantiations it
  covers. Source validation and the abstract check retain a body's facts
  (`mojito_checked::templates`), the elaborator leaves a trace from each `def`
  clone to its template, and a covered clone inherits the facts by
  substitution instead of being inferred. Covered today are module-level
  functions returning scalars over closed scalar expressions, direct calls,
  the built-in `len`, `comptime if` arms, scalar `comptime for` loops, scalar
  locals, and `rebind`; everything else keeps the clone check, and a failed
  per-instance obligation is reported by that check in its own words.
  `MOJITO_VERIFY_TEMPLATE_FACTS=1` infers every derivable body as well and
  requires the facts to agree. The design record is
  `docs/notes/instantiation-from-template.md`.
- A generic struct's per-instantiation method clones now derive from their
  checked template too, for scalar getters: reads of `self`'s scalar fields,
  the built-in `len` over a field, and argument-free method calls on `self` or
  a field, whose target an instance realizes through its own clone. A derived
  clone records the generic-struct applications its body reaches, so discovery
  requests the same instances as before. `stdlib_heavy` derives about one
  instance-clone check in ten; wall time is unchanged so far.
- A per-instantiation method clone whose body calls one of the method's own
  `def(...)` parameters, or forwards it to a sibling call, now derives from
  its checked template (`List.deinit_with`, `Optional.deinit_with`,
  `DictEntry.reap_with`). The call-through residue such a body publishes
  names the parameter's slot and each argument's signature place and nothing
  about a type, so the instance republishes the template's; a body that
  reads a callee's residue owes that its realized callee still publishes the
  same one, and the transfer fixpoint re-runs if it grows. A residue naming
  a compile-time callable, one whose argument carries an origin, and a named
  callable's own effects behind one keep the clone check.
- A per-instantiation method clone whose body writes its own type name or a
  value's `repr` now derives from its checked template, and so do the
  `write_repr_to` and `write_to` families beside it (`stdlib_heavy` derives
  1050 clone bodies before the change and 1102 after).
  `_unqualified_type_name[T]()` keeps its type beside its spelling, so the
  instance re-renders the spelling from its own substituted type instead of
  inheriting the template's wording, and `repr(value)` re-proves its argument
  `Writable`. The implicit conversion either records — the nominal-string
  wrap at `repr`, or an `@implicit` constructor — is selected again from the
  instance's own source and target types, so a clone may name a different
  constructor of the family; one that reaches the target by no conversion, or
  whose constructor consumes, raises, or borrows its source, keeps the clone
  check.
- `--timings` no longer counts a module-qualified bundled struct's methods as
  clones: "generated" is what the elaborator lists (`GeneratedDeclarations`),
  not a `$` in a name. `template_census.*` reports what keeps each generic
  body from being captured.
- The discovery loop builds the checked expression arena once per compilation
  instead of once per round: a round's check returns a
  `DiscoveryResult` that the request collectors read directly. Hello World
  compiles about 6% faster in a debug build.
- `--timings` reports body inference visits by declaration kind, template
  classes, template capture and derivation counts, and arena builds.
  `MOJITO_TIMING_NOTES=1` adds per-declaration `note` lines.

- A variadic struct's members now spell the struct's own pack `Self.Ts`, as
  upstream requires: a field type, a `comptime` member, a method signature,
  an availability clause, or a body naming it bare (`Tuple[*Ts]`,
  `var *args: *Ts`, `Ts.length`, `Ts[i]`, `where Ts.all_conforms_to[X]()`)
  is rejected with upstream's `unqualified access to struct parameter 'Ts';
  use 'Self.Ts' instead`. The bare name remains the struct header's — its
  parameter list, conformance clauses, and trailing `where` — and a `def`'s
  or a method's own pack. The parser keeps the qualified spread `*Self.Ts`
  apart from the bare one, an elaboration pass checks each variadic struct
  before anything copies its field types, and `conforms_to(Self.Ts.values,
  X)` is accepted on a method beside `Self.Ts.all_conforms_to[X]()`. The
  stdlib's `Tuple`, `PackTuple`, `TString`, `TypeNames`, and `Variant` are
  respelled; five `assets/type_error/pack_*_unqualified.mojo` fixtures and
  `assets/ok/pack_bare_in_struct_header.mojo` pin both sides against the pin.
- A write through a pointer field (or pointer parameter) whose origin binder
  has symbolic mutability (`Origin[mut=m]`, or a bare `Origin`) is now judged
  as upstream does. It is rejected inside the generic body, and in
  non-generic code it is resolved per binding through the holder's
  construction-time origins, an `ImmOrigin(o)` application included. The
  dereference of a pointer field takes the pointer's capability rather than
  the holder binding's, which also lets a plain `self` method write through a
  `MutOrigin` field with `+=`. The two extension fixtures became `type_error`
  fixtures, and the old "cannot write through a Pointer with an immutable
  origin" text is now upstream's "expression must be mutable in assignment".
- A field moved out of a struct (`p.a^`) must now be written back before
  any other part of the value is used, before the variable is redefined, and
  before the function exits, as upstream. The ownership analysis reports
  upstream's two diagnostics, "value 'p.a' cannot be consumed, because 'p'
  is used later" and "field 'p.a' destroyed out of the middle of a value,
  preventing the overall value from being destroyed". A `deinit` parameter's
  direct fields stay independently movable. The two extension fixtures that
  pinned the old leniency became rejections, and `DictEntry.reap_value`,
  `DictEntry.reap_with`, and `Set._take_items` replace the stdlib's field
  moves out of locals.

- A `for` loop now accepts only current Mojo's iterator protocol, a
  `__next__` that raises `StopIteration`. The bounded protocol, where
  `__len__()` decides whether a non-raising `__next__` is called, was removed
  upstream in Mojo 0.26.1. A non-raising `__next__` is now rejected with the
  pin's words, "'X' does not implement the '__has_next__' method", and a loop
  over a generic `Iterable` bound advances through the raising contract too.
  `Iterator`, `Iterable`, `IterableOwned`, and `StopIteration` move to
  `std.iter` and join the prelude, as upstream homes them, and a program's
  own top-level declaration of one of those names shadows the prelude's. A
  loop source that its iterator cannot refer to is destroyed as soon as
  `__iter__` returns, for a named source, a temporary, and a comprehension
  alike; one the iterator borrows still lives until the loop exits.
  `conformance/assets-mojo-rejects.tsv` is empty: its last 21 fixtures were
  respelled, or moved to `assets/extensions/` where the pin refuses their
  shape.
- Five behavioral divergences from the pinned Mojo are closed, and two more
  are retained on purpose. `input()` raises `Error("EOF")` at end of input
  and must be called from a raising context; native `input()` still returns
  the empty string until the next runtime ABI bump. A nested `def` that names
  itself from its own body is rejected, as upstream asks for the recursion at
  file scope. `SIMD.ne` is the ordered predicate, so a NaN lane is unequal to
  nothing. A `shuffle` mask must have one index per receiver lane, and the new
  `slice[width, offset=]()` and `join(other)` narrow and widen instead. A bool
  mask splats with `SIMD[DType.bool, N](fill=b)`, and no longer takes a
  positional `Bool` splat. The masked shift amount and the saturating
  `Int(f)` join `docs/non-goals.md` as retained divergences.

- `Optional`/`OptionalReg` now live at `std.collections.optional` and `Tuple`
  at `std.builtin.tuple`, upstream's module homes; `std.optional` and
  `std.collections.tuple` no longer exist. The prelude, `from std.collections
  import Optional`, and the flat `optional` facade are unchanged. Five
  `assets/` error fixtures that the pinned Mojo refused on the old import now
  reach the defect they pin, and all five agree with it
  (`conformance/assets-mojo-errors.tsv`).

- The pinned Mojo now rejects only 21 of the ordinary `assets/` `_ok`
  fixtures, down from 102, and all 21 are the one `__has_next__` iterator
  family that has its own roadmap task. Thirty-five fixtures were respelled
  the way the pin demands and are now `conformance/cases.tsv` rows; twenty-two
  whose *shape* the pin refuses outright — origin-parameter erasure,
  cross-origin field stores, parametric-mut pointer writes, partial field
  moves, capturing-lambda locals, a `mut self` `__call__`, owned-interior
  generations, the Mojito-only `std.algorithms` and
  `std.collections.string_dict` — moved under `assets/extensions/`, where
  `scripts/sweep-assets-mojo --extensions` asserts the rejection and the whole
  pipeline still runs them. `AGENTS.md` invariant 1 and `assets/README.md` now
  separate an extension *kept on purpose* from a ledgered divergence, and
  `docs/roadmap.md` carries the twenty-six new divergence entries the pass
  found.
- Five compiler changes came out of the respellings. `SIMD` gains upstream's
  elementwise comparison methods `lt`/`le`/`gt`/`ge`/`eq`/`ne` on both
  backends (bool lanes order `False` below `True`, as the `i1` unsigned
  predicates do); `round` is nearest-ties-to-even rather than
  ties-away-from-zero, matching upstream and `llvm.roundeven.f64`;
  `origin_of(self.field)` is accepted wherever `origin_of(self)` is in an
  abstract signature, widening to the whole receiver; `Self.Ts[i]` folds like
  the bare `Ts[i]` pack index; and a bracketed projection index accepts a
  qualified struct binder (`Self.InnerType[Self.o]`).

- Six behavioral divergences from the pinned Mojo are closed. `unsafe_origin_cast`
  now requires the target origin's mutability to equal the pointer's in both
  directions, matching upstream's `target_origin: Origin[mut=Self.mut]`
  (`assets/type_error/pointer_origin_cast_no_downgrade.mojo`); a builtin `SIMD`
  lane index takes a plain `Int` and no longer normalizes an `Indexer` the way
  every other subscript still does
  (`assets/type_error/simd_subscript_indexer.mojo`); a `Pointer` loan is
  provenance rather than an exclusive borrow, so a second reader — another
  `Pointer`, or a `ref` argument naming the place — coexists with it while
  writes to the owner still conflict
  (`assets/ok/live_pointer_ref_argument.mojo`); assigning to a `mut p:
  Pointer[...]` parameter verifies and runs, because a pointer-typed slot
  stores its handle as the pointer value and the assignment replaces the slot
  instead of writing through it
  (`assets/ok/mut_pointer_parameter_reassign.mojo`); a `comptime if` outside a
  function is rejected with upstream's sentence
  (`assets/parse_error/comptime_if_module_level.mojo`), which withdraws
  module-level conditional declaration generation; and the Mojito-only
  `is_same_type[T, U]()` predicate is gone in favour of upstream's `T == U`
  type comparison (`assets/type_error/is_same_type_removed.mojo`).
- Three implementations Mojito carried in Rust or hand-written LLVM are now
  bundled Mojo, called by both runtimes: integer `**` (a wrapping
  square-and-multiply `std._intrinsics._pow_int` replaces the backend-emitted
  `mjrt_pow` helper and the VM's `i64::wrapping_pow`), integer display
  (`_int_digits`/`_uint_digits` write the decimal bytes into the caller's
  buffer — the native backend's formatting alloca or a reused VM heap
  scratch — replacing `mjrt_fmt_i64`/`mjrt_fmt_u64` and the VM's `Display`
  rendering wherever a program can see the text), and `repr` of a nominal
  `String` natively, which now calls the compiled `String.write_repr_to`
  instead of `mjrt_repr_string`. The new `stdlib/std/_intrinsics.mojo` holds
  the bodies, `std.prelude` links it into every program without exporting a
  name, and `mojito_symbol::symbol` owns the symbols and the rule that
  decides when a `**` calls one. The three runtime symbols keep their ABI
  rows until the batched `MJRT_ABI_VERSION` bump retires them; the VM's
  `Display for Value` keeps its Rust integer arms for diagnostics, the CLI
  binding dump, and `SIMD` lanes.
- The eight `assets/ok` fixtures the pinned Mojo rejected now compile and run
  on both compilers, and all eight joined `conformance/cases.tsv` as `run`
  rows so they cannot drift again. The `IsTrivially*` predicates import from
  `std.traits`, `is_same_type[T, U]()` gives way to the upstream `T == U` type
  comparison (in `stdlib/std/algorithms.mojo` and
  `stdlib/std/collections/tuple.mojo` too), the `Tuple` import goes away
  because both preludes already export it, a variadic struct's pack parameter
  is spelled `Self.Ts` inside its methods and bare in its own conformance
  clauses over a `Movable` bound, and the two module-level `comptime if`
  assertions fold into a second `comptime` alias instead. The two accessors
  that narrow a pack element to a method's own type parameter — which upstream
  spells with a `rebind` Mojito does not implement — move to
  `conformance/fixtures/pack_element_type_narrowing.mojo` as a `mojito-only`
  case, and four divergences found along the way join the roadmap ledger.
- A view-returning method called on an owning temporary receiver
  (`for c in String("abc").codepoints()`, `len(String("abc").__reversed__())`)
  no longer reads freed memory natively. The checker now materializes such a
  receiver into an anonymous owned binding, the same hidden slot a temporary
  bound to a `ref [origin]` parameter already gets, so the view's loan has a
  real place and the temporary lives as long as the view that borrows it.
  Previously only a named receiver worked: the temporary had no place to
  lend, so no loan was established and the native owned-temp release freed it
  after its last use as an operand.
- An omitted argument whose default runs a converting constructor now lowers
  natively instead of rejecting. Previously only a `None` default over a
  generic struct instance (`arg: Optional[T] = None`) worked; now any folded
  literal the checker wrapped in an `@implicit` conversion does, including
  `x: Optional[Int] = 5` and a hand-written `@implicit` constructor recorded
  under its bare struct name (`m: Meters = 3`). Backend monomorphization
  resolves that bare name through the shared callable-symbol policy and
  enqueues the constructor, which a default may be the only thing to reach,
  and a subscript's omitted argument fills from the same path as a call's, so
  an aggregate default works there too.
- A multi-lane SIMD value moves natively as one typed `<N x lane>` load and
  store instead of `llvm.memcpy`, and its slot is allocated at that vector
  type (still aligned like one lane, so `LayoutCx` and the ABI are
  unchanged). A `memcpy` is a non-promotable use, so previously any slot
  read or written as a whole stayed in memory even when its lane writes
  vectorized; now mem2reg lifts SIMD locals into vector SSA at `O0`.
- Direct `ref` struct fields (`var f: ref[o] T`) are kept as a tracked
  Mojito extension: upstream rejects them today but may adopt them. Their
  fixtures move to the new `assets/extensions/<folder>/` class (110 files),
  which the corpus and native-parity harnesses run under
  `extensions_<folder>` names, so every ordinary `assets/` fixture now
  compiles with the pinned Mojo. Where a `ref`-field fixture has a Mojo-valid
  twin, the twin spells the storage through `Pointer[T, origin]` under the
  same name in the ordinary folder.
- Diagnostics respelled to the pinned Mojo's texts: a use after a transfer —
  definite, path-dependent, or through a `try` region — is `use of
  uninitialized value 'x'`; `len` on a value that is not `Sized` is `no
  matching function in call to 'len'`; `print` on a non-`Writable` value is
  `invalid call to 'print': an element of 'values' with type '…' does not
  conform to trait 'Writable'; …`; abandoning, overwriting, or discarding
  (`_ = x^`) an `@explicit_destroy` or `Movable`-only value is `'x'
  abandoned without being explicitly destroyed: <message>` and destroying it
  twice `use of uninitialized value 'x'`; a `@__parameter` def's capture
  list is `expected ':' in function definition` with no token prefix.
- `std.memory` is a package (`alloc`, `maybe_uninit`, `owned_pointer`
  submodules mirroring upstream's layout); `unsafe_alloc` is exported only by
  `std.memory.alloc`, so `from std.memory import unsafe_alloc` now rejects as
  upstream does.
- Rejections aligned with the pinned Mojo: a tracked local bound to a
  `ref[ImmStaticOrigin|ImmUntrackedOrigin|MutUntrackedOrigin]` parameter
  (upstream's conversion text and origin note), an `except` arm re-consuming
  a value whose raising named destructor already consumed it (`use of
  uninitialized value`), a `var` parameter or local typed by a type parameter
  or dependent pack projection that does not prove `Deinitable` being
  abandoned (`Tuple.consume_elements`/`deinit_with` handlers included), and an
  `Indexer` whose `__mlir_index__` returns `Int` — upstream's
  `-> __mlir_type.index` spelling is accepted and `Int.__mlir_index__()` is
  the identity. `@__parameter` closures capture implicitly per binding
  (mutable locals as `mut`) and reject an explicit capture list; augmented
  assignment to an immutable binding reports upstream's in-place text.

### Fixed

- VM: a write through a `Pointer` to a `List` element (`p[][1] = 99`,
  `p[][0] += 1`, a method's `self.src[][i] = v`) no longer fails with a
  reference-projection error; the pointer boundary detector steps through
  the dereference like the reader does.
- A positional variadic pack (`var *values: T`) of a non-`Deinitable` `T`
  is no longer linear — upstream destroys its elements implicitly — while
  `**kwargs` packs, `mut`/`var` parameters, and locals of such a `T` stay
  linear as upstream; `CStringSlice.byte_length` (absent upstream) is
  removed in favor of `len`; the try-region, drop-order, and diagnostic
  pins moved by the last two parity passes are aligned with the pinned
  Mojo, and the `p.a^`-while-`p`-is-used shape is recorded as a Mojito
  divergence (`partial-field-move-parent-used`).
- VM SIMD: `DType.int` lanes wrap at 64 bits like the native `Int`, and a
  width-one SIMD operand beside a vector splats instead of narrowing the
  result. Native SIMD: float `/`, a splatting left operand, `DType.int` lane
  operators, Bool-lane `& | ^`, and NaN lanes in float `reduce_min`/
  `reduce_max` (now `minnum`/`maxnum`, the VM's rule) lower correctly.
- Inside a `__deinit__`/`deinit self` body each direct field of the receiver
  is destroyed at its own last use on both backends (an unused one at entry,
  a branch-only one in its arm, a nested read keeping the whole direct field
  alive), through the new `DropPlace` MIR instruction (`drop.place`) that a
  field-granular refinement of drop elaboration emits; the receiver's
  `ConsumeVar` destroys only the survivors. A field read only inside a loop
  dies after the loop, where the pinned Mojo destroys it at entry (recorded
  as the `deinit-body-field-loop-read` `output-diff` row).
- The move checker walks `try` regions: a value consumed before a raising
  call is uninitialized in the `except` arm, in `else`, and after the `try`
  (a handler re-initialization does not cover the normal path), a raising
  named destructor's receiver is consumed at the call, and a double move
  inside the body rejects statically instead of failing at run time; a
  value consumed after the body's last raise point stays usable in the
  handler.
- Discarding a transferred linear or `@explicit_destroy` value (`_ = x^`)
  is rejected as abandonment instead of discharging the obligation.
- Destruction timing and order match the pinned Mojo on both backends: a
  discarded call result (an expression statement, the unbound result of a
  non-consuming `__enter__`) is destroyed before the next statement; an owner
  whose last use is a field read feeding a call is destroyed after that call
  returns; a consuming `var`/`deinit` parameter or receiver is destroyed at
  its last use inside the callee (an unused one at entry, so a raising named
  destructor no longer leaves the value for an `except` fallback); struct
  fields destroy in declaration order. Exe ratchet 444 → 448.

- Promote the feature-gated Pliron backend from experimental to supported as
  Mojito's path to LLVM and optimized native binaries. Update to pinned Pliron
  revision `477e6b0e`, `llvm-sys 231.0.0`, and LLVM 23.1 discovered through
  `LLVM_SYS_231_PREFIX`; verified MIR remains the backend-independent waist,
  the VM remains the semantic oracle, and Cranelift remains a feasible future
  alternate backend.

### Added

- Native SIMD lowering: the Pliron backend computes multi-lane `SIMD`
  values as LLVM fixed vectors in SSA (construction, elementwise operators
  with masked shifts and trap-free floor division, comparisons, `select`,
  `shuffle`, lane extraction, casts, `to_bits`, and `llvm.vector.reduce.*`
  reductions) over the unchanged lane-aligned storage ABI; 14 new
  `assets/ok/simd_*` differential fixtures cover every dtype, widths up to
  16, stored Bool masks, ABI crossings, and the numeric edges.
- The filesystem slice's files, streams, paths, and temporary directories:
  the `with` statement as a checker desugar selected by the manager's methods
  (a consuming `__enter__` whose result lives to the block end, a plain
  `__exit__` by `try`/`finally`, both overloads with `__exit__(self, err) ->
  Bool` deciding propagation; upstream's rejection texts), `std.io`
  `FileHandle`/`open`/`FileDescriptor` as prelude names, `std.sys`
  `stdin`/`stdout`/`stderr`, `print(sep=, end=, flush=, file=)` on both
  backends, `std.pathlib` (`Path`, `cwd`, `DIR_SEPARATOR`), `std.tempfile`
  (`gettempdir`, `mkdtemp`, `TemporaryDirectory`), `String(from_utf8=)`
  with UTF-8 validation, `Span.unsafe_ptr()`; beneath them, width-1 scalars
  gain `copy()`, the VM-CTFE subprogram folds every retained module-scope
  constant, and an omitted `Optional[T] = None` argument compiles natively
  (the monomorphizer instantiates the recorded constructor for the parameter
  type); exe ratchet 434 → 444, errors 30 → 34.

- The filesystem slice's host boundary and `std.os`: upstream's
  `external_call[callee, return_type, num_fixed_args=](*args)` builtin over a
  closed libc callee table (`mojito_types::ffi`; the VM runs each callee on
  Rust's standard library with libc's return, `errno`, and buffer contract,
  the native backend calls libc directly, no runtime-ABI change), `std.ffi`
  (`c_*` aliases, `CStringSlice`, `get_errno`), `String.as_c_string_slice()`
  and `String(unsafe_from_utf8_ptr=)`, `std.sys._libc_errno` (`ErrNo`
  rendering glibc's `strerror` text), `std.stat` mode predicates, and the
  `std.os` package with upstream's signatures — `listdir`, `mkdir`/`makedirs`,
  `remove`/`unlink`, `rmdir`/`removedirs`, `getenv`/`setenv`/`unsetenv`, `sep`,
  `SEEK_*`, `PathLike` (conformed to by `String`/`StringSpan`), and `os.path`
  (`exists`, `lexists`, `isdir`, `isfile`, `islink`, `getsize`, `is_absolute`,
  `join`, `split`, `basename`, `dirname`, `split_extension`, `splitroot`,
  `expandvars`, `expanduser`); `Int(pointer)` is the address conversion and
  `String(error)` converts natively; sized scalar aliases (`Int8`, ...) are
  module-level `comptime` type values; exe ratchet 429 → 434.

- Diagnostic wording and strictness in upstream's shape: a failed
  message-less `where` clause reports `invalid call to 'show': violated
  constraint; constraint declared here evaluated to False, expected
  'conforms_to(T, Copyable)'` (the clause as declared, for a sole candidate
  and for a pack-gated method such as `Variant.set`; `(condition,
  "message")` clauses keep `constraint failed: message`, and every call
  diagnostic now starts `invalid call to`); a struct's own parameter
  spelled bare inside its body (`var value: T`, `Counter[length]()`)
  reports `unqualified access to struct parameter 'T'; use 'Self.T'
  instead`; a field typed by a bare struct type parameter needs a
  `Deinitable`-proving bound or a (conditional) `Deinitable` conformance
  (`field 'value' has non-'Deinitable' type 'T'`; the fixtures and tests
  that stored `Self.T` under `Copyable & Movable`/`AnyType` bounds now
  carry the bound, and `is_deinitable` follows trait refinement); and
  `Self.<field>` in a bracket slot reports `cannot access instance field
  'index' without an instance of 'Counter[length]'` instead of naming the
  applied struct. The bundled `std.range` iterators spell `Scalar[Self.dtype]`
  as upstream does (a `Self.n` bracket argument now substitutes inside a
  value specialization). (`assets/type_error/where_violated_sole_candidate.mojo`,
  `struct_field_unqualified_param.mojo`,
  `struct_field_non_deinitable_param.mojo`, `fieldwise_anytype_field.mojo`,
  `struct_param_bare_bracket_slot.mojo`, `struct_field_in_bracket_slot.mojo`)
- Compile-time collections: a `comptime` binding of a set or dictionary
  display (or the explicit literal constructors
  `Dict[K, V, default_comp_time_hasher](keys, values, None)` /
  `Set[T, H](...)`, or an annotated empty `{}`) is a compile-time value;
  `len`, `in`, `keys()`/`values()`, and `comptime for` read it at compile
  time, any other method call or subscript over a compile-time value runs as
  one VM-CTFE entry whose result type a checked probe infers
  (`comptime g = M.get("a").value()`; a raising call reports upstream's
  `cannot call raising function in comptime initializer`), and a runtime use
  crosses through the new `materialize[X]()` and `comptime(expr)` forms — a
  bare runtime use of a compile-time list, set, or dictionary now rejects
  with upstream's `not 'ImplicitlyCopyable'` diagnostic. The VM-CTFE purity
  walk is an effect classifier (only `print`/`input` reject up front). Two
  fixes beneath it: a call's erased compile-time argument keeps its slot so
  later positional arguments stay aligned (`Dict[String, Int, H](...)`
  reified `Int` as the key type), and an erased-body `hash[Self.H]` reads the
  hasher through a `self` reference handle (inserts inside `Dict.__init__`
  hashed with the default hasher). (`assets/ok/comptime_collections.mojo`,
  `assets/type_error/comptime_dict_raising_subscript.mojo`,
  `comptime_collection_runtime_use.mojo`, `comptime_list_runtime_use.mojo`,
  `comptime_dict_result_not_freezable.mojo`)
- Upstream's Movable-only collection bounds: `Dict[K: Hashable & Equatable &
  Movable, V: Movable]`, `Set[T: Hashable & Equatable & Movable]`, and
  `StringDict[V: Movable]`, with the copying APIs `where`-guarded on
  `Copyable` (a method's `where` clause now refines its own signature, a
  comptime alias resolves under its clause or the conditional conformance
  requiring it, and a conditional `Copyable` conformance verifies under its
  condition); a violated struct-parameter bound at construction names the
  bound. `List`/`String` gain `(*, unsafe_uninit_length)` construction and
  resize plus `String.unsafe_ptr_mut()`: the VM tracks never-written heap
  slots (reads trap, destroys are no-ops) and `len`/`print` read a named
  struct place in place rather than copying it.
- Parameter kinds in nested positions: the bundled Array and Span iterators
  iterate themselves (a stored `arr.__iter__()`/`span.__iter__()` drives a
  loop and keeps its source view alive — a view-returning method lends a
  named view receiver itself), a struct's own value parameter spells
  `Self.n` in its aliases and bracket arguments, a `Tuple[...]` application
  resolves in every type-argument position (`Tuple[Int, Tuple[Int, Bool]]()`,
  `TypeNames[Tuple[Int, Bool]]()`, a `T: Defaultable` bound) with bare
  `Tuple(...)` calls nesting without an annotation and SIMD elements
  defaulting to zero lanes, `_unqualified_type_name` spells minted value
  specializations at every nesting level plus `value : Type` / `True`
  value arguments, and `Array` gains `write_repr_to`.
- Origin-bearing `Span`/`Pointer` construction in upstream's shape:
  `Span(unsafe_ptr=, length=)` takes `Pointer[Self.T, Self.origin]` — a
  tracked pointer (`xs.unsafe_ptr()`, `Pointer(to=x)`) binds and loans the
  span's origin, an explicit application (`Span[Byte, origin_of(self)](…)`)
  is checked against the pointer's provenance, and an untracked or
  unsafe-any pointer binds the slot untracked; `String.as_bytes` and the
  hashlib bytes overloads spell it that way. Constructor calls partition
  explicit origin arguments like annotations do (`V[origin_of(s)](3)` on a
  hand-written `__init__`), generic free defs return origin-bearing structs
  (`def view[T](ref xs: List[T]) -> Span[T, origin_of(xs)]`),
  `MutUntrackedOrigin`/`ImmUnsafeAnyOrigin` fill struct origin slots, and
  `ImmStaticOrigin` is importable from `std.origin`
  (`assets/ok/span_pointer_construction.mojo`,
  `assets/ok/span_generic_free_def_view.mojo`).
- `ptr[unsafe_offset=i]` is a place: it stores (`=`, `+=`), binds a `ref`,
  passes as a `mut` argument, and takes a field-chain or call-result base;
  a pointer parameter is written through (a pointer is a value whose
  origin, not whose binding, carries the write capability).
- A bare `Pointer[T, _]` parameter is accepted as the immutable alias, the
  verdicts upstream's per-call origin inference reaches (reads through it,
  rejects writes and mutable-demanding forwards, not concrete outside
  parameter position): `case:pointer-placeholder-parameter` and the three
  reject cases.
- Free type-pack defs (`def show[*Ts: Writable](*args: *Ts)`) specialize
  over any checked argument — a local, a generic construction, an
  origin-bearing temporary (`Named("k", w)`, its erased origin slot spelled
  `_`) — through the checker-recorded instantiation, and method packs
  accept origin-bearing temporaries (`FormatStruct.params(Named("k", k))`,
  `case:pack-named-repr`; `assets/ok/pack_element_temporaries.mojo`).
- The bundled `_ListIter`/`_SetIter` iterate themselves (`for x in it` over
  a stored iterator), and an inferred generic call over an origin-slotted
  struct value (`next(it)`, `ident(named)`) keeps the abstract path instead
  of failing to spell the erased slot.

### Fixed

- A tracked pointer passed to a call at its source's last use
  (`first(s.unsafe_ptr(), 5)`, `var q = s.unsafe_ptr(); first(q, 5)`) no
  longer frees the source before the callee reads the pointee; a
  `Pointer(to=x)` handle bound to a placeholder-origin parameter is
  dereferenced as the handle rather than read through.
- A `ref self` method returning a value classifies as a read of its
  receiver: two live byte views of one String coexist, and a read of the
  source under a live view is accepted.
- An origin argument in a local's annotation (`var w:
  StringSpan[ImmStaticOrigin] = s`, `Span[Int, origin_of(ys)] = xs`) is a
  demand the initializer must satisfy instead of being validated and
  erased.

- Hashing parity with current Mojo: `Hasher._update_with_simd(mut self,
  value: SIMD[_, _])` (one per-type clone per hashed scalar/vector; the
  retired `UInt64` leaf is a conformance error), `SIMD.to_bits[dtype]()` and
  `.length`, the keyed `AHasher[key: U256]` with `default_hasher` as its
  zero-keyed specialization (`repr(set)` spells `Hasher=AHasher[[0, 0, 0,
  0] : SIMD[DType.uint64, 4]]`), the bytes `hash(bytes: ImmPointer[UInt8, _],
  n)` overload and `hash_seeded_bytes`, conditional `Hashable` on `List`,
  `Array`, `Set`, and `Dict` in upstream's bodies (`Variant` tags with a
  `UInt8`), and compile-time `hash[...](x)` through VM CTFE for scalar,
  Bool, Float64, and string arguments (`case:hasher-upstream-spellings`,
  `case:container-hashable`, `case:hashlib-keyed-ahasher`,
  `case:set-repr-keyed-hasher`, `case:hashlib-bytes-hash`,
  `case:comptime-hash`, `case:dict-hasher-forwarding`).
- Compile-time SIMD values (`CtValue::Simd`), SIMD-typed struct value
  parameters (a vector alias as a bound), typed scalar constructors and SIMD
  construction in compile-time evaluation, a placeholder-origin pointer
  parameter in free functions (`ImmPointer[T, _]`/`MutPointer[T, _]`; the
  bare `Pointer[T, _]` asks for the permission), and a `where`-assumed
  `Hashable` proving `element.__hash__(hasher)` on an unbounded `T`.

### Fixed

- Native lane: a supplied constructible type argument (`hash[Fnv1a](x)`)
  and an erased body forwarding its own binder (`hash[Self.H](key)` in
  Dict) bind the hasher the source names instead of the declaration default;
  width-1 vector registers index and reduce as scalars; the VM reifies a
  method-level `Hasher` binder inferred from its `mut` argument (`H2()`).

- `StringSpan` parameters in upstream's shape: the String result APIs
  (search, affix tests, `replace`, `split`/`splitlines`, case, predicates,
  justification, the strip family) live on `StringSpan` with `StringSpan`
  needle parameters and `String` forwards through `StringSpan(self)`; a
  literal converts to `StringSpan` (upstream's `StaticString` initializer,
  bridged in the VM and natively), a String temporary converts at a view
  parameter or binding, and `split(sep)`/`split(sep, maxsplit)` are
  upstream's two overloads (`case:string-span-methods`,
  `case:string-span-literal`, `case:string-span-implicit-temporary`).
- Repr vocabulary: the compile-time `_unqualified_type_name[T]()` intrinsic
  (`std.reflection.type_info`; the checker folds each resolution to current
  Mojo's unqualified spelling, `SIMD[DType.int, 1]`, `Optional[String]`, a
  struct's bare name, and MIR carries it as a string constant),
  `TypeNames[*Ts]()` in `std.format._utils`, and `write_repr_to` on `Tuple`
  (`case:repr-type-names`, `case:repr-vocabulary`).

### Changed

- A deferred slot (`CtValue::Deferred`) now names the binder whose slot it
  fills by its declaration's identity rather than its spelling, so the Tuple
  closedness check no longer counts it as bound by an enclosing value binder
  that merely shares the spelling. The elaborator's markers for a name that
  is no parameter are a separate `CtValue::Marker`. MIR text writes the slot
  as a `binder` record and the marker as `ct_marker`, which moves the
  textual format to schema 1.5; artifacts of schema 1.0 through 1.4 still
  load, a spelled slot reading as an unbound reference.
- `StringLiteral` and the nominal `String` are distinct overload keys: `def
  f(x: StringLiteral)` beside `def f(x: String)` is legal and a literal
  argument selects the `StringLiteral` overload, as upstream
  (`case:string-literal-overload`; the former redeclaration fixture moved to
  `assets/ok`).
- `repr` now writes upstream's texts: `Int(7)`/`UInt(7)`/`Float64(2.5)` for
  scalars, single-quoted Strings with backslash escapes (`'it\\'s'`,
  `'a\\nb'`), `Slice(start=1, end=4, step=None)` for slice descriptors, and
  `Tuple[<element types>](<element reprs>)` for tuples; the native `repr`
  of a String single-quotes as well.

- String result APIs, batch 3: `as_bytes` (a borrowed `Span[Byte]`; `Span`
  gained upstream's pointer-backed `Span(unsafe_ptr=, length=)` constructor,
  which `StringSpan.__hash__` now uses instead of copying into a
  `List[Byte]`), `unsafe_ptr`, `capacity_bytes`, `resize(length, fill_byte)`,
  `append(codepoint)`, the raising `__int__`/`__float__` (so `Int(s)` /
  `Float64(s)` are raising calls the checker gates), the prelude free
  functions `atol(str, base=10)`/`atof(str)` with upstream's parsing rules and
  error texts, and the `codepoints()`/`codepoint_slices()`/`graphemes()`
  iterators (`case:string-bytes-span`, `case:string-parse`,
  `case:string-parse-errors`, `case:string-codepoint-iter`).

### Fixed

- An operator dunder's `where` availability clause (`__eq__ ... where
  conforms_to(Self.T, Equatable)`) is now judged in the operator path as a
  method call judges it: `Optional[Opaque] == Optional[Opaque]` and the same
  shape on a user struct reject as "operator '==' is not defined for …"
  instead of failing at runtime.
- A consuming-receiver copy (`bare.reverse()` on an implicitly copyable
  public tuple bound as `var bare = 4, "four"`) takes the receiver place's
  exact type; the binding keeps a literal element where the checked
  expression type defaults it, and the verifier compares the two for
  equality.
- Native: a raising function's propagation block freed the heap buffers of
  its *borrowed* heap-owning parameters (parameters arrive flagged as
  initialized), so the caller's `String` was freed by the callee's raise and
  again by the caller (`case:raise-borrowed-string-parameter`); only owned
  parameters release there now.
- Overload symbols: a `Pointer[T, origin]`/`UnsafePointer[T]` parameter
  annotation now mangles like the call side's `Ty::Pointer` (origins erase),
  so same-arity overloads that differ by a pointer parameter resolve.

- String result APIs, batch 2 (pure library code): `upper`/`lower` over a
  simple-case subset (ASCII, Latin-1, Latin Extended-A, Greek, Cyrillic;
  `ß` uppercases to `SS`), `isupper`/`islower`, `isspace`, `is_ascii_digit`,
  `is_ascii_printable`, byte-width `ascii_rjust`/`ascii_ljust`/`ascii_center`
  (a multi-byte fill character aborts), and `Codepoint`'s
  `is_ascii_digit`/`is_ascii_upper`/`is_ascii_lower`/`is_ascii_printable`/
  `is_posix_space`/`is_python_space` (`case:string-case-predicates`,
  `case:string-predicates`, `case:string-justify`,
  `case:codepoint-predicates`). The native backend lowers a `String`
  parameter's literal default (`fillchar: String = " "`): a borrowed slot
  reads a global-backed descriptor, an owned slot receives a heap copy.

### Changed

- A deferred slot (`CtValue::Deferred`) now names the binder whose slot it
  fills by its declaration's identity rather than its spelling, so the Tuple
  closedness check no longer counts it as bound by an enclosing value binder
  that merely shares the spelling. The elaborator's markers for a name that
  is no parameter are a separate `CtValue::Marker`. MIR text writes the slot
  as a `binder` record and the marker as `ct_marker`, which moves the
  textual format to schema 1.5; artifacts of schema 1.0 through 1.4 still
  load, a spelled slot reading as an unbound reference.
- `codepoint_count()`/`grapheme_count()` are spelled `count_codepoints()`/
  `count_graphemes()` on `String` and `StringSpan`, matching upstream; the
  old spellings are gone.

- String result APIs, batch 1 (upstream signatures, pure library code):
  `find`/`rfind(substr, start=0)`, `count`, `startswith`/`endswith(affix,
  start=0, end=-1)`, `replace`, `join[T: Copyable & Writable](elems: Span[T,
  _])` (a `List[T]` argument now solves `T` — the checked analogue of Span's
  implicit List constructor), `split(sep, maxsplit=-1)` (the empty separator
  yields upstream's `['', <codepoints>..., '']` instead of raising),
  whitespace `split(maxsplit=-1)`, `splitlines(keepends)`, `Boolable`,
  `__mul__`, String as a `Writer` (`s.write(a, b, …)`), and the borrowed-view
  strip family (`strip`/`lstrip`/`rstrip` with the POSIX-space default or a
  codepoint set, `removeprefix`/`removesuffix`) on both `String` and
  `StringSpan`. `StringSpan` is prelude-visible, `Boolable`, `Equatable`
  against another view or an owned `String`, and supports `in`
  (`case:string-replace-join-strip`, `case:string-join`,
  `case:string-strip-views`, `case:string-search-extras`,
  `case:string-split-forms`, `case:string-bool-mul`,
  `case:string-span-equality`).
- Operator dunders overloaded at the same arity (`StringSpan.__eq__(rhs:
  Self)` beside `__eq__(rhs: String)`): the infix operator selects the
  overload by the right operand's type (by value coercion, then through an
  `@implicit` conversion of the operand, as a call argument would convert) and
  records the exact lowered symbol, which the VM and the native backend
  dispatch directly.

### Fixed

- A borrowing-view temporary returned by a method call and passed as a call
  argument (`print(s.strip())` as the source's last use) kept its source alive
  only for plain-function-call temporaries; method-call temporaries now anchor
  the same hidden slot, so the source is no longer dropped before the
  consuming call.

- `Variant` gap-fill inside the intrinsic design: upstream's `init_with=`
  placement constructor (a zero-parameter factory selects the alternative by
  its result type; `Movable where False` alternatives construct), the
  method-spelled projection `unsafe_get[T]()`, `is_type_supported[T]()` on the
  parameterized type itself, and `repr(v)` in upstream's
  `Variant[<alternatives>](<payload>)` frame with the alternatives spelled as
  current Mojo's unqualified type names. Native parity: Variant `__hash__`
  (discriminant, then a tag switch over each alternative's hash) and Variant
  `==`/`!=` (tag, then the active payload's equality) now lower through
  pliron, with monomorphization and reachability enqueuing every nominal
  alternative's `__hash__`/`__eq__` instance, so Variants serve as native Dict
  keys; the VM compares nominal payloads through their `__eq__` instead of
  rejecting (`case:variant-init-with`, `case:variant-unsafe-get`,
  `case:variant-hash-key`).
- The `is` / `is not` comparison operators (current Mojo syntax): `is` is a
  keyword token, `is not` is one two-word operator at comparison precedence
  (`not x is None` reads as `not (x is None)`), and both dispatch to the left
  operand's `__is__` / `__isnot__` like every other struct dunder — scalars
  and structs without `__is__` reject. `Optional` gains upstream's
  `__is__`/`__isnot__(self, other: NoneType)`, so `opt is None` works on
  both backends (the native method-call path now skips the zero-sized
  `NoneType` operand) (`case:optional-is-none`).
- `Optional[T]` value protocols matching upstream: declared `Boolable` and
  `Defaultable`, conditional `Equatable` (`__eq__`/`__ne__`), `Hashable`
  (a `UInt8` presence tag then the payload, so Optionals serve as Dict keys),
  and `Writable` (payload text or `None`); upstream's `@implicit` value
  constructor (`var x: Optional[Int] = 5`); the consuming
  `or_else(deinit self, var default)`; and `unsafe_value`, `unsafe_take`,
  `bounds`. Compiler changes that surfaced with it: an `@implicit`
  constructor may take its argument by `var`; a `deinit self` call whose
  `ImplicitlyCopyable` receiver is implicitly copied now runs the copy
  lifecycle into a temporary instead of sharing the original's storage;
  an implicit conversion's result register carries the converted-to type
  with its arguments (`Optional[Int]`, not bare `Optional`); `table[None]`
  over a value base parses as a subscript rather than a parameter
  application; and pliron skips zero-sized (`NoneType`) constructor
  arguments and marks direct compiled-constructor results as owned
  temporaries (`case:optional-value-protocols`, `case:optional-dict-keys`).
- Slice descriptor protocols: `Slice` is `Equatable` (`==`/`!=` over the
  three bounds; `ContiguousSlice`/`StridedSlice` stay non-comparable like
  upstream), every descriptor kind prints as `Slice(start, end, step)` with
  `None` for an omitted bound, `ContiguousSlice.indices(length)` returns
  upstream's two-element `(start, end)`, and explicit `Slice(...)`/`slice(...)`
  construction now lowers natively. List's normalizing slice overload is
  spelled `StridedSlice`, which a `Slice`-typed descriptor value selects
  through the new `Slice -> StridedSlice` widening (closing the recorded
  normalizing-overload residue). `std.builtin.builtin_slice` gained a
  docstring-only module home exporting the descriptor names, so upstream's
  import spelling resolves — the `slice-implicit-conversion-rejected` case had
  been rejecting only because the module failed to load
  (`case:slice-descriptor-protocols`, `case:contiguous-slice-not-equatable`).
- `Tuple` is `Hashable` when every element is (hasher-protocol `__hash__`
  feeding the elements in order, matching upstream) and declares `Sized`;
  tuples now serve as `Dict` keys and `Set` elements, `__contains__` takes
  upstream's `[T: Equatable]` bound, and a public tuple satisfies
  `Hashable`/`Equatable`/`Writable` generic bounds — the checker evaluates
  those contracts structurally across the specialization staging seam (as it
  already did for `Comparable`), and compile-time elaboration spells a tuple
  specialization back as the canonical `Tuple[...]` when it re-checks a
  generic body, so `hash(x)` with a tuple-bound `T` resolves
  (`case:tuple-hashable`, `case:tuple-unhashable-key`).
- Static-method dispatch on parameterized nominal types (parametric statics):
  the explicit `Dict[Int, String].fromkeys(...)` receiver — whether it parses
  as a `TypeApply` or, for a single non-builtin type argument, as a subscript
  the checker reinterprets — and the bare `Dict.fromkeys(keys, 0)` receiver
  with struct parameters inferred from the argument types via the constructor
  binder. `Self` and `Self.K`-style signature references resolve to the
  instantiated struct; overloads (including same-arity pairs) resolve through
  the checker-selected symbols and execute on the VM and pliron backends.
  `Dict.fromkeys` lands in the bundled stdlib (a `List` key source subsetting
  upstream's `Iterable`/`IterableOwned` overloads) as the first parametric
  static.

- Current Mojo's hasher-based `Hashable` and `std.hashlib`: `Hashable`'s
  requirement is `__hash__(self, mut hasher: Some[Hasher])` (or
  `__hash__[H: Hasher](self, mut hasher: H)`) with a reflective field default
  synthesized at elaboration; `Hasher` is `__init__`/`_update_with_bytes`/
  `_update_with_simd`/`update`/`finish(var self) -> UInt64`; the prelude
  `hash[T: Hashable, //, HasherType: Hasher = default_hasher](x) -> UInt64`
  is stdlib code, and `std.hashlib` ships `AHasher` (`default_hasher`) and
  `Fnv1a` (`default_comp_time_hasher`) whose values match the audited head.
  `Dict`/`Set` gain `H: Hasher = default_hasher` (entries cache their
  `UInt64` hash), `String`/`StringSpan` conform through the hasher, and
  scalar leaves reach the hasher through one normalized `UInt64`
  (`-0.0` folded). Supporting compiler features: SIMD bitwise/shift
  operators and `SIMD[DType.bool, 1]` conditions, zero-parameter `comptime`
  type aliases, `H()` construction of a `Hasher`/`Defaultable`-bounded type
  parameter (`MirInstr::ConstructTypeParam`, reified at runtime), `Some[…]`
  arguments on concrete receivers, and `std.bit.rotate_bits_left`.
- Ref-field residue lifts: a ref-field view returned through a plain
  read-convention *parameter* (free function or method argument) now executes.
  A borrowing-view call (`BorrowViewResult`) retains its place arguments as
  shared reads — MIR `arg_places` may name a read-convention slot, the
  ownership analysis classifies a retained place by the callee's `ref_params`
  at that slot (`check_ownership_program` threads a `CalleeRefParams` table
  into the loan analysis), and the VM binds such a parameter to the caller's
  storage — and a method's view result lends its aggregate place arguments
  like a free function's. Loan-carrying temporary arguments anchor across
  callable-value (`CallIndirect`) calls, and across `return` values and branch
  conditions (terminator operands flush their anchors before the terminator;
  previously the temporary's source could be dropped before the call).
  Delegated-origin binder correspondences now resolve through alias-typed
  fields: a field spelled through the declaring struct's comptime alias
  (`var iter: Self.dict_entry_iter`, parameterized `Self.view_t[o2]`) records
  the same binder bindings as a direct application, read from the alias body
  in source form with the alias's own binders substituted by the application,
  so two-binder delegations through such fields no longer fall back to the
  single-binder heuristic (or reject). The return-site resolver now chains the
  recorded map with the single-binder fallback exactly like the signature
  side.
  Upstream's iterator-storage shape — `var src: Pointer[T, Self.o]` stored
  from `Pointer(to=xs)` of a `ref[Self.o] xs` constructor parameter, then
  `self.src[][r]` — now checks and runs: the checker records the struct
  binder a `ref[Self.o]` parameter names and mints it as the pointer's origin
  (`PointerOrigin::Param`, matching the field by identity), handwritten
  initializers record pointer-field sources like ref-field sources,
  view-result borrowing (`BorrowViewResult`) covers results
  carrying origin-bearing pointer fields and lends `mut`/`ref`-bound place
  arguments of any type, and the VM gains `RefProjection::Deref` — the
  offset-0 dereference of a single-pointee pointer is an identity projection
  through the stored handle instead of an element-0 index into the pointee.
  The `pointer-field-iterator-storage` conformance row pins the shape.
  A delegated origin clause rooted at a bare carrier parameter
  (`def first_key(c: EntryCursor) -> ref[c.current().key] Int`) is accepted:
  the signature resolver falls back to the carrier for a callee binder nothing
  names, the return-site check compares against the carrier's place, and the
  call site records the carrier's construction-time origins as the returned
  reference's loans. The `origin-delegated-parameter-receiver` conformance
  row pins it. With that, the "Ref-field residue lifts" roadmap bullet is
  closed. Regenerating the pliron parity manifest for the new fixtures
  exposed a pre-existing native gap (`list_methods_growth`,
  `overload_transfer_effect_isolation` failed natively at the previous head
  despite their `exe-differential` rows): a trait-dispatched `copy()` whose
  receiver register is typed as a `ref` to a scalar — a reference result
  retained in a hidden `$call_ref` slot, `span[i].copy()` inside
  `List.extend(Span)` — now lowers as the scalar value read like the plain
  scalar receiver; the "unresolved method call" diagnostic names the
  receiver's type.

- The owned-`var` transfer convention: a place of a `Copyable`-only type can
  no longer be implicitly copied into a consuming position (`var`/`deinit`
  parameters and receivers, operator `var` operands such as `p + q` on
  `List`, variable initialization, assignment, field stores, returns, and
  displays); spell `^` or `.copy()`, as upstream. The new
  `TypeError::ImplicitCopy` diagnostic carries upstream's `^`/`.copy()` notes.
  `String`, `Codepoint`, and `Optional[T: ImplicitlyCopyable]` are
  `ImplicitlyCopyable`; an `ImplicitlyCopyable` struct may declare an explicit
  copy initializer; `TrivialRegisterPassable` refines `ImplicitlyCopyable`;
  `copy()` resolves on built-in copyable values and on generic parameters whose
  copyability is proven by a where-clause assumption. Non-`ImplicitlyCopyable`
  reads overlapping a `mut`/`ref` access in the same call are aliasing errors.
  Textual MIR `call.method` records gain `recv_writes`. The pliron backend
  lowers trait-dispatched `copy()` on a scalar receiver (a generic body's
  `value.copy()` monomorphized to a builtin) as the value read, matching the
  VM's non-struct `copy` intrinsic.

- Overloaded method transfer effects are keyed and replayed by the selected
  signature-qualified callable identity. A consuming overload no longer
  inherits a borrowing sibling's loan effects; abstract trait dispatch still
  unions every conforming implementation and overload. This removes List's
  private `_extend_moving` workaround, so `extend`, `+`, `+=`, `*`, and `*=`
  use the public overloaded surface directly.
- `size_of[T]()` target-layout queries, backed by the shared native ABI layout
  engine in both the VM and Pliron paths. Checked MIR retains the concrete type
  in a typed `SizeOf` instruction, including aggregate padding, and generic
  uses specialize before lowering.
- Bitwise augmented assignment (`&=`, `|=`, and `^=`) for builtin Int/UInt
  places and user-defined values through the dedicated `__iand__`, `__ior__`,
  and `__ixor__` methods. The lexer distinguishes the new tokens from ordinary
  bitwise operators and the transfer sigil.
- Delegated-call origin expressions generalized (2026-08-30, pin-attested):
  the callee may take arguments (`ref[self.iter.step(1).key]` — the
  clause's origin depends only on the receiver walk, and arguments are
  checked per call), and multi-origin-binder correspondences resolve
  through the field application's recorded binder bindings
  (`var second: EntryCursor[Self.o2]` maps the callee's binder to `o2`,
  and the returned reference loans exactly the resolved source). The
  former zero-argument rejection fixture graduated to `assets/ok`.
- Struct origin binders in member origin clauses require the qualified
  spelling (2026-08-30, pin-attested): `ref [o] src:` parameter clauses and
  `-> ref[o]`-rooted return clauses inside a struct now reject with the
  pin's exact message ("unqualified access to struct parameter 'o'; use
  'Self.o' instead"), and the qualified spelling works in direct,
  projected (`Self.o._get_owned_interior[...]`), and
  `Origin[...].cast_from[...]`-wrapped clause forms. Function-, method-,
  and alias-own origin binders stay bare (compiler-reserved `__`-prefixed
  synthesized binders too), and ref-field annotations accept both
  spellings (`ref[Self.o]` fields now resolve alongside the extension's
  bare form). The bundled iterators and `String`/`Span` constructors now
  use the upstream spellings.
- Signature-position origin concreteness (2026-08-29, pin-attested).
  Parameter annotations now follow the initialized-local rule: a bare
  origin-slotted generic (`def read(h: Holder)`) infers per call, while a
  partial application (`def first(s: Span[Int])`) rejects with the
  placeholder hint — the accepted spelling is `Span[Int, _]` (the stdlib
  `List.extend` signature updated to match upstream's). Return annotations
  require applied origins: `-> EntryIter[origin_of(self.entries)]` is now
  accepted (origin arguments in signature positions resolve syntactically
  and erase), while a bare `-> EntryIter` or a placeholder return rejects
  as not concrete, exactly like the pin. Compiler-generated (`$`-mangled)
  specializations keep their reconstructed annotations exempt.
- Origin placeholders `_` and `...` in type applications (2026-08-29,
  pin-attested): `var s: Span[Int, _] = xs` and the `...` spelling mark the
  origin slot explicitly inferred — the application counts as complete and
  the origin resolves from the initializer or call context. Function
  parameters accept them like the bare-generic spelling; struct fields and
  uninitialized locals reject them exactly like an omitted slot
  ("is not concrete").
- Temporaries auto-borrow into `ref` constructor parameters and `ref`
  bindings (2026-08-29, pin-attested): `View(make_list(), 0)` and
  `ref x = make_list()` accept an owned temporary, which is materialized
  into a hidden owned slot with the borrower's lifetime — upstream's
  temporary-lifetime rule. The view's loans keep the temporary alive and
  conflict-checked; returning a view of a temporary rejects with the same
  `escapes storage` family as the owned-place precedent. Parameter origin
  clauses additionally accept upstream's qualified `Self.o` binder spelling
  (previously return-clause-only).

### Removed

- The fork hashing surface: `std.hashing` (`IncrementalHasher`,
  `bucket_index`) and its flat `hashing` facade, the value-returning
  `__hash__(self) -> UInt` shape (now a `Hashable` conformance error), the
  checker/VM `hash` builtin, the runtime FNV intrinsic, and pliron's inline
  string/identity hashes.
- The Mojito-only `HashDict[K, V]` and `HashSet[T]` collections are retired:
  upstream ships only `Dict`/`Set`, so the fork is burned down. `Dict`
  absorbs HashDict's hash-bucketed layout (dense insertion-ordered entries,
  nested-list bucket index, doubling at load factor one) behind its
  unchanged API, and `Dict`/`Set` now carry upstream's `Hashable`
  key/element bound (KeyElement); differential reject fixtures pin the
  bound on both compilers. `std.hashing.bucket_index` stays as Dict's
  engine.

- The bundled `layout` package (`Layout`, `IntTuple`,
  `LayoutTensor[dtype: DType, layout: Layout]`) is deleted. Upstream moved
  the layout library out of the Mojo standard library to the MAX kernels
  tree (`max/kernels/src/layout/` at the audited head), so under the
  match-or-subset rule it is no longer Mojito surface. The DType and
  frozen-struct value-parameter machinery it exercised is shared language
  infrastructure (SIMD, generics) and stays, with its fixtures rewritten
  onto neutral structs (`comptime_frozen_struct.mojo`,
  `struct_value_param_frozen.mojo`). `std.memory.Layout[T]` — the
  allocation descriptor — is unrelated and unchanged.

### Fixed

- A borrowed `self` method call on a reference-result receiver
  (`self.items[i].copy()`) no longer reports a false `$call_ref` self-conflict:
  the loan analysis classifies the receiver access from the call's
  `recv_writes` flag instead of treating every retained receiver as a write.
- The VM reads a pointer-subscript receiver through a `ref`-typed field
  (`self.src.data[i]` in Optional's borrowed iterator) instead of failing with
  "field access on non-struct ref".
- An adjacent postfix `^` binds tightest: `p + q^` transfers `q` rather than
  the sum.
- `Dict.get`/`StringDict.get`/`List._get_copy` copy elements without leaking a
  one-element allocation.

- Three pre-existing VM gaps in the ref-field adapter family (2026-08-29).
  A ref-field view returned from a method with a plain (read) `self`
  receiver, or from a free function borrowing a place argument, now keeps
  its borrow contract: the VM passes such receivers as caller-place handles
  (the returned view's `ref` fields root in the caller frame instead of the
  dying callee frame), and free-function calls returning ref-field structs
  install the same caller-side view loans methods do. A temporary
  constructor result whose fields borrow caller storage
  (`read(Holder(Pointer(to=n)))`) is anchored in a hidden slot whose loans
  keep the borrowed source alive through the consuming call — previously the
  source was dropped early and the pointer field read back `None`. A
  heap-backed field projected off a reference-returning call result in
  value position (`var k = it.__next__().key` with a `String` key) now runs
  its `__copyinit__` instead of aliasing the source allocation, which
  double-freed. A `return` leaving a `try` region now re-roots returned
  reference handles exactly like an ordinary return.

### Added

- Ref-field adapter residues closed (2026-08): four acceptance gaps and two
  recorded over-acceptances around borrowing iterator adapters. A bare owned
  place now auto-borrows into a `ref` constructor parameter
  (`View(source, i)` without a prior `ref` binding), installing the same
  source loan, escape rejection, and origin-mutability check as the explicit
  reborrow. A method or subscript chained directly onto a temporary
  borrowing view (`b.pane().first()`, `b.pane()[i]`, nested chains) retains
  the view in a hidden slot whose loans keep the source alive and
  conflict-checked across the chained call; `mut self` methods on
  temporaries stay rejected (now with a readable diagnostic). Subscript
  writes through a parametric-mut ref field (`self.src[0] += 1` under
  `Origin[mut=m]`) are accepted inside the generic body and judged per
  instantiation at each call site of the writing method — an immutable
  source rejects, and a receiver that leaves the parameter symbolic rejects
  (write-requirement propagation through another generic body is a recorded
  subset limit). Upstream's expression-origin ref return is accepted for
  delegated call projections (`-> ref [self.iter.__next__().key]` — the
  origin resolves structurally from the delegated callee's declared
  contract), the qualified `Self.o` binder spelling is accepted in origin
  clauses, and the stdlib `Dict` key/value iterators now delegate stepping
  to the wrapped entry iterator in upstream shape. Two acceptance
  leniencies are removed: storage annotations (struct fields and local
  `var` types) must bind explicit origin slots, mirroring the pinned
  upstream — a partial application that omits an origin slot fails to
  infer it in every storage position, and a bare origin-slotted generic
  (`var inner: Holder`) rejects as not concrete on fields and
  uninitialized locals, while an initialized local may leave the name
  bare and infer from its initializer (`var v: StringSlice = ...`) —
  with the stdlib iterator aliases (`IteratorType`/`TakeIterType`
  bodies, `_GraphemeIter`/`StringSpan` spellings) migrated to
  fully-bound origins.

- Ref-field struct construction from ref-field places + explicit origin
  arguments (2026-08): a ref-field struct can now be constructed from
  another struct's ref *field* (`View(self.src, i)`; the ctor argument's
  read-through facts type the projection for MIR) and a ref field can be
  reborrowed into a local binding (`ref s = self.src`; a
  parametric-origin receiver classifies as an immutable ownership access,
  and MIR verify plus the pliron backend accept the forwarding
  interpretation of a `MakeRef` whose place ends at a stored reference).
  Struct applications accept explicit origin arguments in every
  annotation position and at constructor expressions —
  `EntryIter[K, V, some_origin]`, `View[origin_of(self)]`, bare in-scope
  origin parameter names, and `Self.origin` spellings — validated
  (a concrete `Origin[mut=True]` slot rejects a provably immutable
  argument) and then erased from the origin-free struct identity;
  applications omitting the origin slots keep checking unchanged.
  Comptime alias bodies can bind origin arguments, and a struct's own
  comptime aliases are now visible to its field types (associated
  members resolve before fields, in the checker and the specialization
  conformance oracle), enabling upstream's monomorphic-alias-in-field
  iterator shape. On all of that, the stdlib `Dict`/`StringDict` key and
  value iterators became upstream-shape adapters: `_DictKeyIter` and
  `_DictValueIter` wrap a `_DictEntryIter` field (`var iter:
  Self.dict_entry_iter` / a direct origin application) instead of
  borrowing the entries list directly, with `IteratorType` alias bodies
  binding their view origins.

- Ref-field struct returns from ordinary method calls (2026-08): a method
  whose non-consuming receiver returns a struct containing a `ref` field
  (a borrowing view/iterator) now lends the receiver to the result, so
  the source outlives the view and mutating it while the view lives is
  rejected; the VM dispatches methods on receivers read out of `ref`
  fields and writes mutations back through stored reference handles. On
  that machinery, `Dict.keys`/`values`/`items` became upstream-shape
  borrowing views (still self-iterable, non-indexable, and without
  `len`; value/entry yields are read-only) and `take_items` drains
  lazily through a mutably borrowing iterator whose mid-drain
  observations match upstream exactly, retiring the conformance suite's
  only output-divergence case.
- Collection API parity growth (2026-08): `Dict` is now hash-bucketed over
  its dense insertion-ordered entries (doubling at load factor one) and
  carries upstream's `Hashable` key bound, adding `pop` (raising and
  defaulted), LIFO `popitem`, ref-returning `setdefault`, `update`,
  `clear`, merging `__or__`, `__bool__`, order-insensitive equality, and
  `(*, capacity)` construction; `keys`/`values`/`items` now return
  self-iterable, non-indexable snapshot iterators (indexing and `len` on a
  view reject, matching upstream). `Set` gains the `Hashable` element
  bound, `remove`/`discard`, LIFO `pop`, `clear`, `update`, the
  union/intersection/difference/symmetric_difference families with
  `&`/`|`/`-`/`^` operators, subset/superset comparisons, and `__bool__`;
  its borrowed iteration yields read-only element references (upstream's
  accepted write-through corrupts its hash index — recorded subset gap).
  `List` gains `(*, capacity)` and `(*, length, fill)` construction,
  `reserve`/`resize`/`shrink`/`swap_elements`/`capacity`,
  `unsafe_get`/`unsafe_set`, equality, `__bool__`, `+`/`+=` concatenation,
  `*`/`*=` repetition, Optional-returning `try_index`, a raising `index`
  with upstream's ValueError message, upstream's consuming
  `extend(var other)`, and the borrowing `extend(Span)` overload. `Dict`
  also gains `take_items`, draining every entry into an owned iterator
  (Mojito drains eagerly where upstream's borrowed iterator drains lazily —
  mid-drain observation is a documented output divergence). The parser now
  accepts `where` clauses after a bare `raises` effect (previously `where`
  mis-parsed as the raises error type). Every accept/reject/output claim carries differential fixtures
  against the pinned Mojo (`dict-methods-growth`, `set-methods-growth`,
  `list-methods-growth`, view/bound rejection cases, and the
  `set-ref-write-gap`/`list-extend-consumes` transfer pins).

- Contextually inferred member references (upstream 2026-08): a leading-dot
  chain (`var c: Color = .red()`, `takes_color(.of(7))`, `[.red(), .of(3)]`
  under a `List[Color]` annotation, `return .red()`) resolves its base against
  the expected type. The parser builds the chain over a compiler-internal
  `$contextual` sentinel, the checker re-checks the spelled form with spans
  preserved and records the base, and HIR substitutes the name physically.
  Without a contextual type the form rejects with a targeted diagnostic.
  First slice: static-method calls with postfix chains in expected-type
  positions; bare `.member` comptime value members, parametric statics,
  non-struct expected types, and generic expected types are recorded gaps.

- Function-type `where` clauses (upstream 2026-08): a `thin` function type
  with its own `def[...]` parameters may carry trailing clauses
  (`F: def[w: Int](Int) thin -> None where (w > 0, "msg")`), lowered onto the
  anonymous contract's parameter declarations with binder references
  alpha-renamed into canonical identity. Explicit specializations through the
  parameter evaluate the clauses (`F[0]` fails the example), and the binding
  rule is directional: a constrained function requires the contract to
  declare the matching clause, while unconstrained-into-constrained stays
  allowed and free. `Ty::GenericFunc` now renders its binders and clauses in
  diagnostics. Binder-less clauses and `comptime` function-type aliases are
  recorded mojo-only gaps.

- In-subset library alignment with the `a79fbdf59f2` head: `List`'s element
  bound is `AnyType` with per-API `Movable` requirements on the moving
  surface (an empty `List` of a pinned element type is legal on both
  compilers); `Array` gains lexicographic `Comparable`; `String` gains the
  bare empty constructor, `String(capacity_bytes=…)`, and `reserve_bytes`;
  and `MaybeUninit.write()` lands as the safe, `IsTriviallyDeinitable`-gated
  counterpart of `unsafe_write`. Array `concat`/`repeat` (dependent result
  lengths) and `Defaultable` (generic `Self.T()` construction) are recorded
  mojo-only gaps.

- Nightly re-pin to upstream `a79fbdf59f2` (2026-08-26, Mojo
  `1.1.0.dev2026082605`) with the expired-bridge sweep the new head requires:
  the legacy `read` argument convention is now a hard error with upstream's
  migration diagnostic (`'read' was removed; use 'imm'`; MIR text emits and
  accepts only `imm`, and the internal convention variant is renamed
  `ArgConvention::Imm`); the removed `SIMDSize` and `TypeList.size` alias
  spellings reject; the origin-alias vocabulary is unified on the surviving
  `Imm*`/`Untracked*` set across `ref[...]` clauses, Pointer type arguments,
  type display, and the `std.origin` exports (`ImmUnsafeAnyOrigin` added,
  removed spellings reject with targeted diagnostics); `UnsafeMaybeUninit` is
  renamed `MaybeUninit` with upstream's triviality-gated
  Movable/ImplicitlyCopyable/Deinitable/RegisterPassable conformance header
  (a non-trivially-deinitable payload makes the wrapper linear); and the
  parametric-closure decorator accepts the canonical `@__parameter` spelling
  (`@parameter` stays a warning-era bridge, with the diverging capture model
  recorded as a probe).

- Pliron Stage 5 completion: every runnable `assets/ok` and
  `assets/ownership_ok` fixture is now an O0/O1 executable differential with
  a clean O0 ASan/LSan lane (295 total), every runnable runtime-error fixture
  is an O0/O1/ASan category differential (29 total), and the generated parity
  gate asserts exactly zero exclusions. The closing slice completes dependent
  callable specialization, recursive lifecycle transfer and iterator cleanup,
  TString ownership, String/codepoint/writer/hash fidelity, pointer and
  uninitialized-storage traps, and structural runtime-error classification;
  runtime ABI v6 carries the required abort and lifetime services.

- Pliron Stage 5 slice 4 — pointer/uninit-storage intrinsics and builtins:
  the compiler-private `UnsafePointer` storage instructions lower natively
  (raw single-take moves, in-place element destructors at computed element
  offsets, `unsafe_offset` pointer `+` arithmetic, the
  `UnsafePointer.alloc`/`alloc_aligned`/`unsafe_dangling` family), inline
  `__UninitStorage[T]` storage is payload-only (raw writes/takes, in-place
  destroys, the VM's leak-by-design no-op drop, no synthesized declaration),
  and the `len`/`abs`/`min`/`max`/`round`/`divmod`/`input` builtins plus the
  scalar `__floor__`/`__ceil__`/`__trunc__`/`__ceildiv__` intrinsics run
  VM-exactly — nominal receivers (and struct-lhs binary operators, notably
  `String.__add__`) monomorphize to their dunder methods through the shared
  resolver, so compiled stdlib byte loops execute natively. `input()` reads
  through the runtime ABI v4 `mjrt_read_line` (stdin-failure trap category 6;
  EOF yields `""`), with a test-only `VmBackend` input override feeding both
  backends identical bytes so `input.mojo` becomes a true exe-differential
  row (`run --backend pliron` now inherits the CLI's stdin). Direct
  specialized-`__init__` calls bind their destination as the `out self`
  receiver (unblocking the `$ov$$mono$` constructor-arity family), `None`
  constants lower as erased zero-sized registers, and monomorphization fixes
  the order-dependent literal/concrete binding conflict (`Int` vs
  `IntLiteral` both display as `Int`) and rejects struct instance-identity
  collisions (two instantiations of one generic template) contextually —
  tolerating fields equivalent modulo pointer element types — until the
  Collections slice canonicalizes instance names. Five new `pliron_*`
  fixtures join the exe/ASan gates (the pointer-lifecycle one also joins the
  lifecycle-trace lane); the parity manifest ratchets 129→147
  exe-differential and 149→136 excluded. `Slice` descriptor construction
  moved to the Collections slice.

- Pliron Stage 5 slice 3 — the iterator protocol: native `for` loops over user
  iterators and the scalar ranges. Monomorphization folds each `GetIter`
  prepare chain (typing the split iterator slot, retargeting `__iter__`/
  `__len__`/`__next__` symbols to concrete instances, and statically unrolling
  dynamic trait dispatch under the VM's budget), the bounded `__len__`/
  `__next__(mut self)` protocol advances the iterator slot in place, raising
  iterators run over the tagged-outcome ABI with a statically typed
  StopIteration exhausted edge, the `CopyIteratorReference` adapter
  lifecycle-copies through concrete reference returns, and
  `raise StopIteration()` lowers nullary error structs to owned `MjError`s
  with byte-exact unhandled messages. Elements with user destructors and
  raising reference-yielding `__next__` (List/Span iterators) reject until the
  Collections slice. New `pliron_iter_*`/`pliron_raise_iter_*` fixtures join
  the exe, raise, ASan, and lifecycle-trace gates; the parity manifest's
  exe floor and exclusion ceiling ratchet accordingly (116→129
  exe-differential, 158→149 excluded, 2→4 raise-differential). The slice
  also fixes two latent monomorphization unification gaps: a
  reference-returning call's declared referent now unifies with the
  caller's `ref` handle (restoring `pliron_reference_write_back`), and
  literal-typed argument registers unify with the concrete storage the
  checker admitted (restoring `implicit_conversion`).

- Pliron Stage 5 slice 2 adds backend-private monomorphization below the stable
  MIR waist. Native compilation now discovers concrete generic function,
  method, struct, constructor, and lifecycle instances from entry-reachable MIR,
  substitutes their retained type/value facts, and rejects unresolved symbolic
  state before layout. VM/native callable and method dispatch share the symbol
  layer; cached/canonical MIR and VM execution are unchanged.

- Pliron Stage 5 slice 1 — narrow scalars and literal value types: the native
  backend now lowers the width-1 SIMD scalar aliases (`Int8`…`Int64`,
  `UInt8`…`UInt64`, `Float32`) at their lane width with VM-exact wrapping
  arithmetic, signedness-correct comparisons, width-1 construction and
  `cast[DType.x]()` (float→int truncation saturates at the 128-bit
  intermediate before wrapping, like the VM), and builtin conversions over
  sized operands; `Float32` computes and prints through its f64 view, so
  formatting reuses `mjrt_fmt_f64` and the runtime ABI stays at v3. The
  literal value types lower as first-class storage: materialization into
  scalars and sized lanes wraps VM-exactly (`Float32` correctly rounded from
  the exact rational literal), `IntLiteral`/`FloatLiteral`-typed storage is
  exact i64/f64 — a constant exceeding i64 rejects with `LiteralOutOfRange`
  instead of silently diverging from the VM's arbitrary precision — and
  `StringLiteral`-typed values (variables, parameters, returns, fields) use
  the borrowed `MjStrDesc` descriptor with printing and `String(x)`
  conversion. This unlocks narrow struct fields, `UInt8` pointer/allocation
  elements (nominal-String internals), and the CTFE literal-variable
  fixtures in the parity manifest; design notes and recorded divergences in
  `docs/notes/pliron-stage5.md`.

- Pliron Stage 5 groundwork (roadmap §4): the generated native capability
  matrix `conformance/pliron-capability.tsv` — one row per textual-MIR
  instruction mnemonic, checked-type constructor spelling, and exported
  runtime symbol (`since` versions included), rendered by
  `backend::pliron::capability` and pinned against the canonical schema
  vocabulary (`INSTRUCTION_MNEMONICS` and the new
  `mir::text::TYPE_SPELLINGS` inventory), so a new MIR instruction or type
  constructor forces an explicit native capability decision. The Stage 4 exe
  manifest is renamed to the stage-neutral `conformance/pliron-parity.tsv`
  (test `parity_exe_manifest_and_differential`), and its coverage guards now
  also ratchet the exclusion count downward toward the Stage 5
  zero-exclusion target.

- Shared native target, layout, and runtime ABI (roadmap §4): the normative
  contract [`docs/native-abi.md`](docs/native-abi.md), owned in code by the
  new `src/native/` module — checked build configuration (`Triple` with the
  pinned LLVM-22 data-layout string, `CpuFeatures`, `OptLevel`, `EmitKind`,
  and the new `--target TRIPLE` CLI flag on `compile`/`run`), the shared
  layout engine (declaration-order C-style aggregates, `u32`-tag Variant
  overlays, `MjStrDesc`/`MjString`/`MjError` string and error
  representations, pointer-sized origin-erased references), the relocated
  injective `mj_` mangler, and the `rt_abi` runtime contract table. The
  repository is now a Cargo workspace whose second member,
  `crates/mojito-runtime`, is the independently versioned, dependency-free
  C-ABI runtime (ABI version 1: `mjrt_version`/`mjrt_abi_version`,
  `mjrt_alloc`/`mjrt_dealloc`, `mjrt_write_stdout`, the VM-display
  `mjrt_fmt_i64/u64/f64` formatters, and `mjrt_trap` with exit codes
  `64 + category`); every Pliron-emitted LLVM module is stamped with the
  target triple and data-layout string, every produced executable links the
  runtime archive and exposes the inspectable `mjrt_abi_version` symbol, and
  agreement is pinned mechanically from both sides — Rust signature/layout
  checks in the default lane (`tests/native_abi_test.rs`) and target-only
  LLVM cross checks in the pliron lane (target-data agreement, declaration
  snapshots, a clang data-layout pin, and `llvm-nm` symbol inspection).

### Fixed

- Native (Pliron) String construction from literals works again. The
  2026-08 parity pass changed literal→String conversion to emit the
  nominal constructor overload symbol (`String.__init__$ov$String`)
  instead of the type-name call shape, so the native backend's
  constructor bridge no longer intercepted it and the declared
  never-execute stub body ran instead — every natively built String was
  empty (the full pliron gate had not run since before that pass). The
  native call lowering now routes that overload symbol to the same
  constructor bridge. The pass's seven new `assets/ok` fixtures also
  gained their missing pliron parity/scalar manifest rows, and the
  native runtime's uninitialized-storage trap messages now say
  `MaybeUninit` to match the renamed type (message text only; no ABI
  change).

- Module-level `comptime` constant materialization is now shadow-aware:
  a local `var`/`ref` declaration, implicit function-scope assignment,
  unpack target, loop variable, except binding, or `with … as` binding
  that rebinds the constant's name stays local for the remainder of its
  block instead of being replaced by the materialized literal.
  Previously `comptime i = 2 + 3` broke any program whose linked code
  (including the bundled stdlib's `var i` loops) rebound the name —
  `i += 1` materialized into the invalid `5 += 1`. The declaring
  statement's own initializer still reads the constant
  (`var n = n + 1` sees the outer value).

### Changed

- A deferred slot (`CtValue::Deferred`) now names the binder whose slot it
  fills by its declaration's identity rather than its spelling, so the Tuple
  closedness check no longer counts it as bound by an enclosing value binder
  that merely shares the spelling. The elaborator's markers for a name that
  is no parameter are a separate `CtValue::Marker`. MIR text writes the slot
  as a `binder` record and the marker as `ct_marker`, which moves the
  textual format to schema 1.5; artifacts of schema 1.0 through 1.4 still
  load, a spelled slot reading as an unbound reference.
- `docs/roadmap.md` is restructured into a lean ordered checklist: the
  "Where Mojito Stands" overview moved to `docs/features.md`, the native
  backend architecture/dialect/testing contract moved to
  `docs/architecture.md` ("Native Backend Contract"), completed-milestone
  prose was deleted per the task lifecycle policy, the Pliron Stage 5 work is
  itemized as ordered slice checkboxes, and recurring/unordered sections are
  marked explicitly.

- Int/UInt overflow is now defined two's-complement wrapping on both the VM
  and the native backend — `+ - *`, unary negation, and `**` wrap
  (square-and-multiply over wrapping multiplication), and the single
  overflowing signed-division case is defined as `Int.MIN // -1 == Int.MIN`
  with `Int.MIN % -1 == 0` (the native lowering sanitizes the LLVM
  `sdiv`/`srem` poison case). This closes the overflow divergence recorded in
  the Stage 1/2 notes; the zero-divisor and `**`-exponent traps are
  unchanged. New `assets/ok/pliron_wrap_*` fixtures pin the wrap points
  through both backends at `O0` and `O1`.

- Pliron Stage 1 scalar native backend (roadmap §4, experimental): behind the
  `backend-pliron` feature (LLVM 22), `mojito compile [FILE] --backend pliron
  --emit plir|ll|bc|obj|exe [-o PATH]` compiles the call-graph closure of
  `main` for the scalar subset — Int/Bool constants and arithmetic,
  comparisons, branches, loops, direct calls, recursion, return — from the
  cached post-drop `elaborated_mir` artifact to Pliron's LLVM dialect and on
  to LLVM IR, bitcode, relocatable objects, and linked host executables
  (bitcode + clang). VM parity is pinned by a JIT differential over seven
  `assets/ok/pliron_*` fixtures (including a FloorDiv/Mod sign matrix and
  masked-shift cases matching `runtime.rs` exactly); canonical Pliron text is
  a byte-stable parse/print fixpoint, repeated builds are deterministic, and
  every construct outside the subset — including `print` until the Stage 3
  runtime — rejects with a contextual, source-located diagnostic. Execution
  stays on the register VM (`run`/`exec --backend pliron` still refuse). The
  default build resolves no LLVM dependency (`tests/backend_isolation_test.rs`
  now guards the default feature graph via `cargo tree`); the LLVM lane's
  gate is `scripts/check-pliron`. Design record: `docs/notes/pliron-stage1.md`.

- Pliron Stage 0 feasibility gate (roadmap §4): a standalone spike crate
  (`spikes/pliron-stage0/`, gated by `scripts/check-pliron-spike`) pins
  `pliron`/`pliron-llvm` 0.17.0 against LLVM 22 and proves IR construction,
  canonical textual round trips, located non-panicking verification
  diagnostics, custom and built-in passes, a toy-dialect lowering through the
  dialect-conversion framework, LLVM IR/bitcode export, and host execution of
  `main -> i32` via both LLJIT and a clang-linked executable exiting 42. The
  default build stays LLVM-free: the spike is not a workspace member,
  `tests/backend_isolation_test.rs` guards the root lockfile, and the empty
  `backend-pliron` feature reserves the Stage 1 seam. Pin record, ecosystem
  audit, and facility classification: `docs/notes/pliron-stage0.md`. Verdict:
  GO for Stage 1.

- MIR artifact milestone close-out (roadmap §3): `CompiledProgram` now retains
  the drop-elaborated, re-verified `MirProgram` as one lazily cached artifact
  (`CompiledProgram::elaborated_mir`) that backend execution and
  `emit_mir`/`mojito emit-mir` both consume, unifying their previously separate
  elaborate-and-verify paths. The milestone's contracts are validated and
  recorded: `emit-mir | exec -` is the backend-independent producer/consumer
  composition, every shared runnable conformance case pins direct execution,
  canonical print → parse → print byte equality, and artifact execution with
  identical output and displayed bindings, and corpus-shrink guards protect the
  conformance and round-trip fixture sets. The five completed textual-MIR
  design-note documents (schema, disassembler, assembler parser, round trips,
  artifact execution) folded into `docs/mir-text-format.md`,
  `docs/architecture.md`, and `docs/features.md` and were removed.

- Element-call dispatch: the bare `value[i](args)` spelling over an indexable
  runtime value dispatches as subscript-then-indirect-call, matching current
  Mojo — identifier bases (`objs[0](3)`), member bases (`h.items[0](5)`), and
  multi-index brackets (`g[1, 1](10)`). The checker re-dispatches the shape
  into a recorded two-call plan (the selected `__getitem__` contract plus the
  element's `__call__` target), and MIR lowers it through the existing
  subscript-contract and indirect-call instructions, so a raising getter keeps
  its own catchable effect and a reference-returning getter retains the
  hidden loan-bearing handle. Only non-value bracket arguments (types, names)
  on an indexable value stay rejected.

- Compiler/test textual-MIR integration (roadmap §3): `CompiledProgram` caches
  the ownership-verified MIR lowered by the authoritative pipeline, and both
  execution and `CompiledProgram::emit_mir` consume its shared post-drop form.
  `mojito emit-mir [FILE]` writes that canonical executable artifact to stdout
  and composes directly with `mojito exec -`. Every shared runnable conformance
  case now pins byte-stable assembly plus direct/artifact output and binding
  equivalence.

- VM artifact execution (roadmap §3): `mojito exec [FILE]` runs a verified
  textual MIR artifact (file or stdin) directly on the register VM.
  `artifact::run_artifact` composes the `load_artifact` gate (parse plus the
  canonical MIR semantic verifier) with the new
  `Backend::run_elaborated`/`VmBackend::run_elaborated` entry, which executes
  the serialized, already drop-elaborated program exactly as written — no
  re-elaboration, re-verification, or post-drop ownership re-analysis.
  Loading diagnostics render with artifact line:column, the offending line,
  and the mapped artifact path; non-VM backends refuse. A frozen executable
  snapshot plus output/bindings-equivalence tests pin artifact execution
  against the direct VM run.

### Added

- Lossless textual-MIR round trips (roadmap §3): `mir::text::parse_artifact`
  now decodes the complete 1.0 schema — every instruction, terminator, type,
  origin, and declaration-metadata form the canonical printer emits,
  including nested `try` regions (dense per-region block namespaces, kept out
  of the artifact source map) and the `structs:`/`decls:` sections — with
  missing-required-field, unknown-tag, and duplicate-entry diagnostics.
  `FloatLiteral::parse_exact` is the exact inverse of the literal's display
  spellings (`-0.0`, `{n}.0`, reduced `{numer}/{denom}`), so serialized exact
  literals reproduce bit-for-bit. The new `roundtrip::*` corpus group
  enforces disassemble → parse → re-disassemble byte equality over the
  drop-elaborated MIR of every executable fixture, with the second
  disassembly re-running the canonical verifier on the parsed program;
  `tests/snapshots/mir/metadata.mir` pins a declaration-metadata artifact
  through the same parse/reprint/load gates.

### Fixed

- Per-instruction drop elaboration inside `try` regions (roadmap §3): region
  interiors now get the same ASAP death/`DropVar` elaboration and edge drops
  as top-level blocks, seeded per exit kind with a raise-observer liveness
  seed at every potentially-raising instruction. The overwritten value of an
  outer variable rebound in a `try` body now runs its destructor at the
  rebind on the normal path (still skipped when the constructing call
  raises, so the handler observes the original value), and a variable
  rebound in an `except`/`else`/`finally` region that is dead afterward is
  now dropped inside the region instead of leaking. The `Try.cleanup` and
  `EscapeJump.cleanup` lists remain as idempotent raise-edge/scope-exit
  backstops, and drop timing inside regions now matches the identical code
  outside a `try` (a value's destructor can run between its last use and the
  enclosing statement's effect, as at top level).

### Added

- Artifact verifier integration (roadmap §3): `mir::text::verify_artifact`
  runs the canonical `mir::verify` semantic pass on assembled textual-MIR
  programs and reports every finding as a source-located artifact
  diagnostic — resolved through the retained assembly source map at block,
  then function, then artifact-root precision, with the resolved artifact
  path named in the diagnostic context. `mir::text::load_artifact` composes
  parse-then-verify as the loading gate that the upcoming VM artifact
  execution will sit behind. Verification policy stays in `mir::verify`;
  the text module only maps its canonical finding prefixes to spans.
- The Int/Scalar range family (nightly §8): `stdlib/std/range.mojo` now
  mirrors current Mojo's three private range structs
  (`_ZeroStartingRange`/`_SequentialRange`/`_StridedRange[dtype: DType =
  DType.int]`), each its own borrowed typed-raising iterator over
  `Scalar[dtype]` elements with upstream's construction rules (negative
  zero-start ends clamp empty, the two-argument form never counts down, a
  zero step canonicalizes to the empty range at construction). The public
  `Range` prelude identity is gone (upstream has none); `range` remains the
  stable exported overload set. Scalar arguments (`range(Int32(4))`,
  `range(Int16(3), 7, 2)`) infer the dtype through a checker intercept that
  records the family instantiation and the specialization fixpoint rewrites
  the call into the generated concrete struct's constructor — upstream's
  infer-only `range[dtype: DType, //]` overloads have no explicit spelling,
  so no source-def encoding exists. Mixed dtypes, Bool, and float dtypes
  reject at checking (floats with upstream's needs-a-step message for 1/2
  arguments, an explicit unsupported-subset message for the strided form).
  Struct DType value parameters may now declare upstream's `= DType.int`
  default, and `comptime IteratorType[...] = Self` (a self-iterating
  struct) resolves. Recorded subset gaps: non-Int `Indexer` `range`
  arguments, `reversed()`/`bounds()`/`__has_next__`, containment and
  formatting, float strided ranges, Int-only subscripts, `comptime for`
  over scalar ranges, and bare default application of the dtype parameter.

- The `TypeList` vocabulary (nightly §8): current Mojo's type-level list
  runs as a compile-time subset with the post-rename member names —
  `TypeList.of[Trait=..., T1, ...]()` (concrete constructor),
  `TypeList[Ts.values]()` (the pack adapter, lowering to the same checked
  pack-constraint forms as `conforms_to(Ts.values, ...)`), `length` with
  the deprecated still-shipped `size` alias (probe
  `typelist_size_deprecated_alias`), `any`/`all` per-element predicate
  reductions (builtin `IsTrivially*` spellings or one-parameter predicate
  aliases), `all_conforms_to[Trait]()`, and `contains[T]()` — valid in
  `where` clauses, conditional-conformance conditions, and `comptime if`,
  plus Sized/indexable/bindable compile-time TypeList values. The
  map/filter/reduce/reverse/slice/tabulate family, generator-typed
  predicates, generic-parameter `of` elements, and runtime-position values
  are recorded gaps; pack-adapter forms in variadic-def `where` clauses
  share `conforms_to`'s pre-existing whole-pack limitations.

- Bool-bodied generic comptime aliases (predicate aliases, nightly §8
  groundwork): `comptime IsSmallCopy[T: AnyType] = conforms_to(T, Copyable)
  and IsTriviallyCopyable[T]` compiles its body into the checked constraint
  algebra, and an application (`IsSmallCopy[T]`, `IsSmallCopy[Plain]`) inlines
  it wherever `conforms_to`/`IsTrivially*` propositions are valid — `where`
  clauses, conditional-conformance conditions, and `comptime if` — with
  concrete bindings folding `conforms_to` eagerly and a predicate alias
  freely expanding an earlier one. Generic aliases now pre-register like
  struct shells, so a conformance condition or synthesized conditional
  method may reference an alias declared later in the file. Predicate-alias
  parameters take no bounds beyond `AnyType`, no defaults, and no packs;
  the builtin `IsTrivially*` names are unshadowable; non-Bool value bodies
  stay rejected subset gaps.

- The experimental conservative subtree origin (nightly §7): `origin._subtree`
  is accepted in Pointer origin arguments and `origin_cast` targets — over
  origin parameters, `origin_of(...)` places, and interior projections, but
  strictly terminal and rejected in `ref [...]` clauses — carried through
  checked HIR and verified MIR as a terminal path segment. A subtree pointer
  designates its base or any descendant: it is never multi-element, its
  generation stales on mutation at, above, or below the base (across
  branches, loops, and `try` regions), and a write through a mutable subtree
  pointer succeeds once and invalidates the pointer for every later use
  (current Mojo's first-write rule). `Pointer(to=…)` through a `ref` binding
  — a local `ref` or a `ref` parameter — now mints exactly this provenance
  instead of rejecting.

- Temporary-origin inference (nightly §7): an `@implicit` constructor may
  take a single `ref [origin]` parameter, and the selected conversion then
  borrows its source place so the temporary's origin refines to the source.
  Span's List constructor is `@implicit`, so a `List` passes directly where
  a `Span` is expected — call arguments, annotated bindings, and returns —
  with the same loan, conflict, and escape behavior as the explicit
  construction (a bare list literal stays a recorded subset gap: it types
  as fixed-size `Array`).

- Span borrowed iteration: Span is `Iterable where conforms_to(T, Copyable)`
  through the origin-parameterized protocol (`_SpanIter`), yielding element
  references — `for x in span` reads, `for ref x` writes through to the
  underlying List, sub-slice views iterate their own window, and structural
  source mutation during iteration rejects.

### Changed

- A deferred slot (`CtValue::Deferred`) now names the binder whose slot it
  fills by its declaration's identity rather than its spelling, so the Tuple
  closedness check no longer counts it as bound by an enclosing value binder
  that merely shares the spelling. The elaborator's markers for a name that
  is no parameter are a separate `CtValue::Marker`. MIR text writes the slot
  as a `binder` record and the marker as `ct_marker`, which moves the
  textual format to schema 1.5; artifacts of schema 1.0 through 1.4 still
  load, a spelled slot reading as an unbound reference.
- The prioritized native-backend direction is now Pliron first (staged in
  `docs/roadmap.md`), then Cranelift on material Pliron failure, with a C or C++ source backend as a
  possible addition; direct LLVM or MLIR lowering and eBPF are no longer
  prioritized. The textual MIR/VM assembly is also no longer described as
  human-readable — it is a deterministic tooling format whose output is long
  even for trivial programs.

- The README is now a focused ~110-line landing page (intro, goals, pipeline,
  build, quick start, documentation links). Its former CLI/fixture/conformance/
  library-API material moved to `docs/usage.md`, and the status snapshot, Mojo
  gap inventory, semantics tour, and development direction moved to
  `docs/overview.md`; no content was dropped.

- `Allocation.unsafe_ptr()` is tracked: it returns the Allocation's
  `element` interior-generation origin instead of `MutUntrackedOrigin`, so
  the owner stays alive through the pointer's uses and use-after-free
  rejects statically (`ThinAllocation.unsafe_ptr()` remains the raw
  untracked escape hatch). Supporting this, a whole-variable move —
  `dealloc(a^)`, any `var` argument, a rebind — now invalidates every
  interior generation rooted at the moved variable (the owner-side dual of
  the consume-time interior-liveness rule, which Mojito's architecture
  already enforced and this pass pins with fixtures).

- A minimal `OwnedPointer[T]` owning smart pointer in `std.memory`
  (nightly §6), with current Mojo's naming from day one: value and
  `init_with=` placement construction, `into_inner(deinit self)`, an
  interior-generation `unsafe_ptr(ref self)` view, and a conditional
  destructor (a linear pointee makes the OwnedPointer itself linear).
  Upstream's `p[]` borrowed dereference is a recorded subset gap (the
  empty subscript stays raw-pointer-only). The §6 probe set in
  `conformance/probes/` pins every guessed API shape (handler
  conventions and drain order, `insert` semantics, Optional/Variant
  spellings, the owned-iteration declared family, the mut-receiver
  UnsafeMaybeUninit take) for the next re-pin, closing the roadmap's
  "Linear containers and owning APIs" checkbox.

### Changed

- A deferred slot (`CtValue::Deferred`) now names the binder whose slot it
  fills by its declaration's identity rather than its spelling, so the Tuple
  closedness check no longer counts it as bound by an enclosing value binder
  that merely shares the spelling. The elaborator's markers for a name that
  is no parameter are a separate `CtValue::Marker`. MIR text writes the slot
  as a `binder` record and the marker as `ct_marker`, which moves the
  textual format to schema 1.5; artifacts of schema 1.0 through 1.4 still
  load, a spelled slot reading as an unbound reference.
- The §6 owning family APIs land across the bundled containers:
  linear-capable `deinit_with(deinit self, handler)` on List, Array, Dict,
  Set, StringDict, and Tuple (the `consume_elements` family spelling);
  `clear_with(mut self, handler)` on Dict and Set; and
  displacement-returning `insert` (an `Optional` of the replaced value) on
  Dict, Set, and StringDict. Handlers are `def(deinit …) capturing[_]`
  funargs. Supporting this safely required completing the named-destructor
  contract: an explicit-destructor call now retains its receiver place and
  the VM writes the callee's final `self` state back before the trailing
  consumption, so residual destruction sees moved-field tombstones and
  drained containers instead of a stale pre-call clone (previously a moved
  struct field double-dropped and a drained pointer-backed field would have
  double-freed).

- Variant aligns with current Mojo's owning surface (nightly §6): consuming
  extraction is spelled `unwrap`/`unsafe_unwrap` (the pre-rename
  `take`/`unsafe_take` reject as ordinary unknown members — no compat
  bridge); both `set` forms require every alternative `Deinitable` (the
  previous payload is destroyed under a runtime tag);
  `set[T](init_with=factory)` performs in-place placement replacement; and
  `deinit_with(handler)` is the linear-capable consuming teardown — a
  monomorphic or generic consuming handler, checked to admit every
  alternative, receives the payload under the runtime tag. The two owning
  operations lower through the new `VariantSetInitWith`/`VariantDeinitWith`
  MIR instructions (a deliberate §6 schema addition beside the existing
  Variant family), executed by a narrow synchronous callable-value channel
  in the VM. This folds roadmap §4's `Variant.destroy_with` item into the
  §6 arc.

- Optional is rebuilt as current Mojo's owning container (nightly §6):
  `T: AnyType` over one owned heap slot with conditional
  `Copyable`/`Movable`/`Deinitable`/`Iterable`/`IterableOwned` conformances,
  `init_with=` placement construction (a factory result lands directly in
  storage — no `Movable` requirement), `is_some`/`__bool__`/`or_else`/
  `value`/`take`, linear-capable `deinit_with` and the `deinit_assert_empty`
  named destructor, consuming `map`/`and_then`, and borrowed plus owned
  iteration. The legacy `(value, present: Bool)` constructor is removed; the
  VM's Slice-bound bridge now selects the unique positional constructor
  overload. `UnsafeMaybeUninit` gains `unsafe_init_with` and the
  mut-receiver `unsafe_take`.

- Owned iteration now carries current Mojo's `Movable & Deinitable` element
  bounds (nightly §6). The bundled List `__iter__(var self)`/`IterableOwned`
  gained `Deinitable`-element where clauses, a linear-element specialization
  rejects at iterator selection with the bound named, and a checker gate
  covers user-declared owned iterators. The Mojito-only linear-element
  extension — the `_finish(deinit self)` named-destructor convention, its
  checked `IterationProtocol.finish` channel, and `HirInstr::FinishIter` —
  is removed. Linear variadic-pack forwarding is unaffected: packs are not
  library iterators, and the exhaustion guards (early-exit, raising-call,
  and comprehension-filter rejections) still protect that channel.

### Fixed

- A user-called named destructor (`value^.method()` with `deinit self`) no
  longer re-runs the receiver's whole-value `__deinit__`: drop elaboration
  now treats the lowered `ConsumeVar` as the variable's teardown instead of
  splicing a competing ordinary drop after the call (silent for linear
  receivers, a double-teardown for `Deinitable` ones).
- A callable-typed parameter in an overloaded method now mangles its
  overload suffix from the annotation's structural shape (mirroring the
  checked callable Display) instead of a Debug dump embedding spans and
  source paths, so declaration and call-site symbols agree — required for
  the keyword-selected `__init__(*, init_with=…)` overload.
- A thin (non-capturing) callable now satisfies `capturing[...]` value
  contracts, matching upstream (its capture set is empty).
- The try-region reassignment wipe: a plain reassignment of an outer
  variable inside a `try` body was collected as a body-local and destroyed
  by the region's scope-exit cleanup, so the slot read back as `None` after
  the block (also poisoning loop-carried accumulators, `finally` reads, and
  `break`/`continue`/`return` crossing the `try`). `Try.cleanup` now keeps
  the body's genuine locals — variables whose every definition lies within
  the body region — plus rebound outer variables that are provably
  unobservable after the block (so their values still run destructors at
  the region boundary), and escape-edge cleanups drop a rebound outer
  variable exactly when it is dead at the jump target. The
  `stdlib/std/string.mojo` keyword-slice methods and `_GraphemeIter.__next__`
  no longer need the return-inside-`try` workaround.

### Changed

- A deferred slot (`CtValue::Deferred`) now names the binder whose slot it
  fills by its declaration's identity rather than its spelling, so the Tuple
  closedness check no longer counts it as bound by an enclosing value binder
  that merely shares the spelling. The elaborator's markers for a name that
  is no parameter are a separate `CtValue::Marker`. MIR text writes the slot
  as a `binder` record and the marker as `ct_marker`, which moves the
  textual format to schema 1.5; artifacts of schema 1.0 through 1.4 still
  load, a spelled slot reading as an unbound reference.
- Views and strict bounds (nightly §5): `Span(list)` and `StringSpan` are
  borrowed contiguous views — multi-element origin-bearing pointers (a new
  origin capability: an interior-generation-projected pointer origin
  legally addresses many elements, minted by the new `origin_cast` rebind
  and `List.unsafe_ptr()`, staled by source mutation) plus a length, with
  construction lending the source's place so mutation conflicts while any
  view lives. Contiguous List/Span slices and the new String/StringSpan
  `byte=`/`codepoint=`(/`grapheme=` on StringSpan) keyword slices are
  strict: negative, out-of-range, or reversed bounds abort through the new
  uncatchable `os.abort` trap, and byte endpoints must fall on UTF-8
  codepoint boundaries; strided List slicing keeps `StridedSlice.indices()`
  normalization and copied results. Positional String slicing (contiguous
  and strided) now rejects with a keyword-slice hint (StringLiteral keeps
  the builtin literal slice pending a probe), `StringSlice` is accepted as
  a never-emitted alias of `StringSpan`, and ordinary String, StringSpan,
  and StringLiteral iteration yields borrowed grapheme-cluster StringSpan
  views. Keyword slices (`x[name=a:b]`, omitted bounds preserved) are a
  general subscript form binding keyword-only slice-descriptor parameters;
  `MirInstr::MultiIndex` keyword arguments now carry slice descriptors.
  The `roadmap.md` task lists were also reformatted into nested bullets.
- Pointer/allocation model closure (nightly §4): the bundled collections,
  String, and fixtures now allocate through `std.memory` (`unsafe_alloc` +
  `unsafe_free`), the compiler-private `take(i)`/`destroy(i)` pointer methods
  are deleted in favor of their public `unsafe_offset(i).unsafe_take_pointee()`
  / `unsafe_deinit_pointee()` replacements (the `PointerStorageTake`/`Destroy`
  MIR operations are unchanged), the static-alloc heap primitive is narrowed
  to `std/memory.mojo` alone, and `Pointer[T].unsafe_dangling()` replaces the
  removed `dangling()` spelling (rename hint on the old name). The
  `pointers.unsafe` parity row moves from divergence to subset with the
  remaining upstream surface (address spaces, SIMD load/store families,
  casts, `ManagedAllocation`, `Layout.alignment()`) recorded as deferred;
  `current-pointer-allocation` becomes a shared differential run case and
  the static-alloc rejection gains a reject case. Test threads get a
  16 MiB stack via `.cargo/config.toml` — checker recursion depth scales
  with the linked program, and the grown prelude pushed two
  subscript-contract tests past the old 2 MiB default. Fixed a
  pre-existing specializer bug this migration exposed: a module-level
  `comptime` constant substituted into a same-named type parameter retained
  on a specialized generic def clone (the clone body is materialized as a
  bare statement list, so the declaration's own compile-time parameter
  names must be removed from the substitution); with prelude-linked
  `unsafe_alloc[T]`, any user constant named `T` used to corrupt its
  `Pointer[T, MutUntrackedOrigin]` annotation.

- Layout-based allocation (nightly §4): new self-hosted `std.memory` module
  with `Layout[T]` (`count=`/`alignment=` keyword construction, `count()`),
  linear `ThinAllocation[T]`/`Allocation[T]` (`Deinitable where False` +
  `@explicit_destroy` — implicit drop is a checker error), `unsafe_ptr()`,
  `layout()`, `into_thin()`, `unsafe_leak()`, `unsafe_with_layout()`, free
  `alloc(Layout[T](count=n)) -> Allocation[T]` (prelude-exported),
  `dealloc(allocation^)`, and raw `unsafe_alloc[T](count, alignment=…)`.
  The legacy static `UnsafePointer[T].alloc[_aligned]` surface is retired
  from user code with a migration-hint diagnostic; it survives only as the
  compiler-private heap primitive inside bundled standard-library sources
  (fixtures and linked tests migrated to the current vocabulary). Alignment
  0 means natural element alignment; `Layout.alignment()` is deferred until
  an `align_of` builtin exists, and `unsafe_ptr()` returns an untracked
  origin (`origin_of(field)` pointer results are not yet expressible).

- Keyword pointer subscript (nightly §4): `ptr[unsafe_offset=i]` reads the
  pointee at offset `i` — current Mojo's replacement spelling for the
  deprecated positional `ptr[i]` read, which stays accepted as a bridge.
  It lowers exactly like the positional read (place substitution or handle
  read on origin-bearing pointers, the pointer-intrinsic `Index` otherwise);
  keyword subscripts remain read-only, so stores keep `ptr[i] = v` and
  `unsafe_write`.

- The `unsafe_*` pointer operation vocabulary (nightly §4): `unsafe_offset(i)`
  (provenance-preserving arithmetic), `unsafe_write(value)` and
  `unsafe_write(copy=v)` (move/copy pointee initialization at offset 0 —
  the first keyword-accepting builtin pointer method), `unsafe_take_pointee()`
  and `unsafe_deinit_pointee()` (zero-argument public forms of the raw
  initialized-slot operations, fixed to offset 0 and gated to
  allocation-owning untracked pointers; deinit requires a Deinitable
  element), and `unsafe_free()` (`free()` stays accepted as upstream's
  deprecated bridge). Writes work through place-origin pointers with mutable
  provenance too, reusing the `ptr[] = v` store lowering; chained
  `p.unsafe_offset(i).unsafe_write(x)` receivers store through a synthetic
  binding. All lower onto existing MIR operations — no schema changes.

- Empty-subscript pointer dereference (nightly §4): `ptr[]` reads, and
  `ptr[] = e` / `ptr[] += e` write, the pointee — offset-0 access on heap
  pointers and direct pointee access on `Pointer(to=place)` handles. The
  parser emits a dedicated marker for empty brackets (grammar: the subscript
  argument list may now be empty), so a source `p[None]` can never pose as a
  dereference, and a non-pointer receiver rejects with a contextual
  empty-subscript error instead of dispatching an accessor.

- Pointer naming and origin unification (nightly §4, first slice): `Pointer` is
  now the canonical spelling of the builtin pointer type — type display,
  runtime value display, and every checker/VM diagnostic say `Pointer`, while
  `UnsafePointer` stays accepted as upstream's deprecated alias (internal
  callable mangling intentionally keeps the stable `UnsafePointer$…` identity).
  The internal `Legacy` pointer provenance is retired: the one-argument
  `Pointer[T]`/`UnsafePointer[T]` spelling now resolves to
  `MutUntrackedOrigin`, the origin of heap allocations, so heap pointers type
  as `Pointer[T, MutUntrackedOrigin]` exactly like upstream's `unsafe_alloc`
  result. New `MutPointer`/`ImmPointer` aliases validate a statically known
  origin mutability and reject a mismatch; `Pointer(to=place)` joins
  `UnsafePointer(to=place)` as the place-pointer constructor spelling; and
  writes through any statically immutable provenance (untracked, unsafe-any,
  or static, in addition to the existing place/param rejections) now reject at
  the checker.

- Lifecycle-predicate rename alignment (post-pin follow-up to nightly §0):
  the comptime predicates are now spelled `IsTriviallyMovable[T]`/
  `IsTriviallyCopyable[T]`/`IsTriviallyDeinitable[T]`, matching upstream
  `22b5036987` (one day after the `ae386d1b204` audit pin), which
  hard-renamed them with no deprecated aliases — the pre-rename `Trivially*`
  spellings no longer resolve anywhere (compiler recognition, `std.traits`
  exports, diagnostics). The same upstream change made
  `conforms_to(T, TrivialRegisterPassable)` a sufficient first disjunct:
  a declared `TrivialRegisterPassable` conformance or parameter bound now
  satisfies the predicates ahead of the structural check (which continues to
  prove primitives and ordinary structs). The shallow builtin-marker
  `conforms_to` default is deliberately not consulted for that disjunct.

- Fixed-size `Array[T, length]` and the list-display retarget (nightly §3): a
  new prelude-exported, self-hosted `std.collections.array.Array` declares
  conditional `Copyable`/`Movable`/`Deinitable`/`Equatable`/`Iterable`/
  `IterableOwned`/`Writable` conformances over a `T: AnyType` element and an
  `Int` value parameter, with keyword `fill`/`copy:`/`deinit move:`
  construction, by-reference `__getitem__` (no `__setitem__`, matching the
  audited head), `__len__`/`__eq__`/`__ne__`/`__contains__`, and borrowed plus
  owned iteration; it is neither `ImplicitlyCopyable` nor `Defaultable`. An
  uncontextualized `[1, 2, 3]` now materializes as `Array[Int, 3]` through a
  single nominal variadic literal-constructor call (a new
  `ConstructArrayLiteral` checked fact — no new MIR instructions); an expected
  type with a list-literal constructor (notably `List[T]`) still controls
  contextual materialization, assignment targets now count as expected context,
  and comprehensions still produce `List`. Supporting generalizations: value
  parameters resolve symbolically in struct bodies and infer from argument
  types, generic-struct constructors accept keyword arguments, constructor
  declarations carry their struct's compile-time parameters for value-param
  reification, same-type lifecycle constructors inherit the source's reified
  value parameters, and `unify` solves through reference patterns. Plain
  subscript assignment on a receiver with no `__setitem__` now writes through
  a mutable-reference-returning `__getitem__` (upstream Array's contract) and
  lowers to a direct reference write. Recorded subset gaps: Array's
  `Hashable`, `unsafe_ptr`, `__getitem_param__`, `uninitialized:`
  construction, `deinit_with`, and expected-type context for displays inside
  tuple-unpack right-hand sides and `Variant` constructor arguments.

- Repeated declaration constraints and generic comptime aliases (nightly §1
  follow-up): every declaration family that accepts a trailing `where` clause —
  functions, methods, structs, trait requirements, associated and trait
  comptime members, and comptime declarations — now retains its full clause
  list (`where (c1, "m1") where (c2, "m2")`) through a plural checked
  constraint contract; each clause validates independently and the first
  failing clause reports its own retained message. Per-trait
  conditional-conformance conditions stay single-clause. Generic top-level
  `comptime Alias[params]: Type where ... = ...` declarations lower once into
  a checked alias registry (classified parameters plus the symbolic template
  machinery shared with parameterized associated members) and expand per
  application in any type position — directly, through another alias, or as a
  generic argument, including across module imports — validating arity,
  bounds, defaults, and the alias's own clauses through the same contract as
  a struct application. Bare primitive names (`Int`, `UInt`, `Bool`,
  `Float64`) now resolve as types in expression-derived type positions such
  as alias and associated-member bodies. Recorded subset gaps: value-bodied
  generic aliases, origin-parameterized aliases, function-body generic
  aliases, and constructor-through-alias calls reject with contextual
  diagnostics.

- Mojo dev-branch parity catch-up (nightly §1): keyword collectors now require
  canonical `var **kwargs` syntax in declarations and function types (bare
  `**kwargs` rejects), and the checked/lowered callable identity retains a
  distinct keyword-variadic slot across free, generic, method, static, bounded,
  and indirect calls. The source linker now rejects two different explicit
  imports binding one local name and diagnoses exact canonical self-imports,
  while preserving idempotent imports, prelude shadowing, and real mutual
  cycles through provisional exports. Zero-step ranges are empty in direct
  compile-time unrolling, VM-backed CTFE, and nominal runtime iteration. The
  existing rejection of declarations differing only by `imm`/`mut` is now a
  differential conformance pin.

- Trailing `(condition, "message")` constraints retain the diagnostic in the
  checked algebra across functions/methods, structs, conditional conformances,
  associated and trait comptime members, and non-generic top-level comptime
  declarations. Messages survive specialization, concrete associated-member
  projection, conformance/lifecycle failure, and origin-mutability inference,
  but remain irrelevant to generic identity and logical implication. The
  future-syntax identifiers `class`, `del`, `match`, and `yield` are rejected
  contextually as free or nested function names without becoming lexer
  keywords; ordinary variable/parameter use and current method spellings remain
  legal. Repeated trailing constraints and generic top-level comptime aliases
  are now recorded separately instead of being overstated as implemented.

- Lifecycle canonicalization (Mojo parity catch-up, slice B / nightly §0):
  the internal vocabulary is now `Deinitable` + `__deinit__` end to end
  (builtin-trait registry, checker capability queries and diagnostics, MIR
  drop commentary, the VM's destructor-symbol lookup, bundled stdlib,
  fixtures, tests, and docs); the upstream-deprecated `ImplicitlyDeletable`
  and `__del__` spellings stay accepted and normalize at parse time (the
  `read` → `imm` precedent), pinned by a dedicated compat fixture. A missed
  normalization path fails loudly as `UnknownTrait` because `BUILTIN_TRAITS`
  lists only the canonical name. Declared conditional `Movable` conformance
  is now effective: `Movable where False` rejects `^` transfers, `var`
  parameters and receivers, and move/copy captures, while `deinit`
  consumption (destructors and named destructors) stays legal and Copyable
  pass-by-value is untouched. Added the `TriviallyMovable[T]`/
  `TriviallyCopyable[T]`/`TriviallyDeinitable[T]` comptime predicates
  (semantics pinned from the audited head's `std/traits/*.mojo`: base
  capability plus a compiler-generated lifecycle operation with recursively
  trivial fields), usable in comptime control/bindings, `where` clauses, and
  conformance conditions — and rejected as bounds. New docstring-only
  `std.traits`/`std.origin` module homes export the builtin identities
  (named, aliased, and wildcard imports resolve; unknown names stay
  `NameNotFound`), mirroring the audited upstream export surface.


- Extension alignment sweep (Mojo parity catch-up, slice A): Mojito now
  accepts what the audited Mojo head accepts within its subset — extensions
  remain tolerable only as upstream-tracked deprecation bridges or as cited
  implementations of features on Mojo's own roadmap/proposals — and every
  recorded acceptance in neither category became a rejection with a
  contextual diagnostic plus pinning fixtures. Removed acceptances: the legacy
  `unified {...}` capture spelling (parse error); the bare `move:`
  initializer parameter (migration diagnostic naming
  `__init__(out self, *, deinit move: Self)`); the competing
  positional/keyword-only `__setitem__` overload pair (declaration-time
  rejection — lone setters of either shape still work); `def(...)`-typed
  struct fields and collection elements, including `capturing[...]` storage
  and display-inferred callable elements (the FieldInvocation machinery
  remains internal-only); materializing an explicit Origin specialization of
  a capturing nested function (direct specialized calls still execute); and
  binding a capturing closure to an unqualified `def(...)` value contract
  (the checker now hints the `capturing[...]` spelling; comptime callable
  bounds still ground capturing values, matching upstream). `SIMDSize` stays
  accepted because the audited head still ships it as a deprecated alias.
  The bare `objs[0](args)` element-call spelling is recorded as a subset gap
  with a parenthesization-hint diagnostic; real element-call dispatch is a
  new roadmap item. `conformance/parity.tsv` rows moved from `divergence`
  to `match` accordingly.

### Added

- Cross-call transfer residues closed — every call shape now replays
  loan-transfer effects, ending the MIR-schema-prerequisite work. Checked
  function types carry identity-transparent inferred effects, replayed when
  a `def` value is called indirectly; callable-struct calls replay
  `Struct.__call__`; overloaded call sites replay the shared entry (a
  previously silent gap); abstract trait dispatch replays the union over
  conforming implementations; and higher-order bodies calling through their
  own callable parameters record call-through residues that each call site
  resolves against the concrete callable, composing through forwarding
  chains. The capture channel records concrete `Bound`-destination effects
  for stores through captured owners (including enclosing locals, closing a
  soundness hole where a closure-local reference stored into a captured
  local crashed the VM at runtime), stored closures loan their reference
  captures' owners while the storage lives, and stored callables invoke
  through the new field-invocation channel (`holder.callback(1)`, capturing
  environments included) — which also fixed the pre-existing
  "checked nominal subscript receiver is None" crash and a phantom-variable
  lowering bug that made a `def` name unusable as a value after appearing
  in a list display. Transfer destinations are interior-precise:
  `EstablishLoans` carries a destination domain, sibling fields keep
  independent generations, and rebinding the exact interior place releases
  its transferred loans.

- CPU Layout and LayoutTensor semantics. A new bundled `layout` package
  (import-only, never prelude) self-hosts the CPU layout core: a flat
  rank-≤4 `IntTuple`, `Layout` with `row_major`/`col_major` factories,
  `rank`/`size`/`cosize`, equality, printing, and the callable
  coordinate-to-linear mapping via `def(IntTuple) -> Int` conformance;
  and `LayoutTensor[dtype: DType, layout: Layout]`, a layout-aware view
  over a caller-managed `UnsafePointer[Scalar[dtype]]` buffer with
  rank-1/rank-2 indexing, `size()`/`dim(i)`, and write-through
  assignment. Underneath it, two new compile-time parameter forms
  monomorphize their declarations before checking (no MIR schema
  change): `[dtype: DType]` value parameters on defs and structs
  (`Scalar[dtype]`/`SIMD[dtype, w]` positions resolve concretely per
  application), and struct-typed value parameters on structs
  (`[layout: Layout]`) whose arguments — constructor or static-method
  calls like `Layout.row_major(2, 3)` — evaluate through VM-backed CTFE
  and freeze as `CtValue::Struct` specialization keys (fieldwise
  construction and recursively pointer-free fields required; frozen
  field reads fold to constants; `comptime L = Layout.row_major(2, 3)`
  works as a module constant too). Struct monomorphization generalizes
  from type packs to mixed scalar/dtype/struct value declarations with
  retained `mut`/Origin binders, and value parameters now bake into
  signature/field type positions. Also fixed en route: overloaded
  methods with `Scalar[DType.x]`/`SIMD[...]`-annotated parameters
  produced mismatched overload symbols between declaration and call
  sides (now both use the canonical checked spelling). Deferred and
  documented: origin-parameterized borrowed tensor views, the GPU
  surface, tile/slice views, SIMD load/store, the layout algebra, and
  recursive IntTuple.

- SIMD semantic completion. `SIMDLength` is now the width-parameter
  spelling (`SIMDSize` stays a deprecated, never-emitted compatibility
  alias, and the comptime elaborator now classifies both as value
  parameters). Explicit scalar/SIMD construction converts runtime values —
  integers wrap to any integer width and convert to float lanes, floats
  adjust precision, and any `Intable` value (bounded parameter or
  conforming struct, whose builtin-trait conformance the checker now
  recognizes for declaring structs) constructs integer scalars through
  `__int__` — while implicit contexts stay literal-exact, and
  `Scalar[DType.x](arg)` now lowers as ordinary width-1 SIMD construction
  (previously a MIR-verify failure for non-canonical dtypes). New
  CPU-visible operations: unary negation, `cast[DType.target]()`
  (elementwise; float→int truncates toward zero; bool casts deferred),
  bool-mask `select(t, f)`, the `reduce_add/mul/min/max` and mask
  `reduce_and/or` reductions (collapsing to canonical native scalars),
  and `shuffle[*mask]()` with compile-time lane indices — the latter two
  compile-time-payload methods ride new verified `SimdCast`/`SimdShuffle`
  MIR instructions. A `def` may take its width as a `[w: SIMDLength]`
  value parameter: each call monomorphizes (value parameters now bake
  into signature type positions), so an invalid bound width rejects
  during checked elaboration, with a MIR-verify width backstop for
  assembled artifacts. The runtime `Byte(Int)` conversions unlock
  `Codepoint.from_u32(scalar)` — Int-based, `None` for
  negatives/surrogates/out-of-range — whose character text is
  UTF-8-encoded in ordinary library code.

- String result APIs, non-raising slicing, and the un-annotated binding
  default. The nominal String's slice is now non-raising byte-wise library
  code with Python-normalized bounds and strides, matching the builtin
  literal slice — a cut inside a multibyte UTF-8 sequence keeps the raw
  bytes and the struct-to-literal read-back renders them lossily instead
  of erroring. New byte-offset result APIs: `find`/`rfind` (`-1` when
  absent; the empty needle matches at the search start/end),
  `startswith`/`endswith`, and `split(sep)` returning eager owned
  `List[String]` pieces (raising on an empty separator, and the first
  stdlib use of `List[String]`). With slicing and result parity in place,
  an un-annotated `var s = "lit"` binding now materializes the nominal
  String through the `@implicit` literal constructor, as in current Mojo;
  aggregate elements, `comptime` bindings, and bare literal expressions
  stay `StringLiteral`, and seams without the linked stdlib struct keep
  the literal default.

- The StringLiteral/String type split. Source `String` annotations now
  resolve to the self-hosted nominal String struct through the ordinary
  prelude/linker path (the parser no longer treats `String` as a builtin
  type keyword), while string literals and literal-only operations stay
  on the compile-time `StringLiteral` type — spellable in annotations,
  mirroring `IntLiteral`, with `Ty::StringLiteral` as the renamed checker
  variant. A literal converts wherever the nominal String is expected via
  the struct's new `@implicit` literal constructor: bindings, arguments,
  returns, tuple-display and collection elements, fieldwise and
  specialized-pack constructor fields, and mixed operator operands
  (`"a" + s` and `s == "b"` normalize onto the struct's migrated
  `__add__`/`__iadd__`/`__contains__`/comparison dunders). Conversion
  retargeting makes `String(x)` stringify, `input()`, `repr(x)`, and
  `.format(...)` produce nominal String values through the VM's
  materialize bridge; `Error(msg)`/`raise` and the Writer `write_string`
  contract accept either spelling (a nominally-declared `write_string`
  receives a materialized payload). StringDict kwargs keys and other
  literal-typed internals are respelled `StringLiteral` with an unchanged
  VM ABI. Overload symbols keep the stable `String` spelling for both
  types, so overloads differing only in that pair are rejected as
  redeclarations. Un-annotated `var s = "lit"` bindings deliberately stay
  `StringLiteral` (a recorded divergence and follow-up); unlinked seam
  programs reject bare `String` annotations explicitly.

### Fixed

- Consuming a borrowed loop binding (a `var`-convention argument such as
  `list.append(element)` inside `for element in source`) now runs the
  referent's `__copyinit__` instead of aliasing its owning storage — a
  pre-existing double free for any pointer-owning Copyable struct,
  exposed by nominal String keys flowing through dict iteration.

- Variant projections spelled with a struct-typed alternative
  (`v[String]`) resolve and lower like the type-token spelling, and
  implicit conversions now participate in constructor overload scoring
  (ranked below direct coercions) and in specialized heterogeneous pack
  argument binding.

- Variant payloads own independent values: construction, `set`, and
  `replace` deep-copy a lifecycle payload into the variant, and a
  projection value read copies a Copyable payload out of the variant's
  storage (keeping the owner alive through the copy) instead of
  aliasing a buffer the owner's drop then frees.

- Lazy captured t-strings. A `t"…"` now produces the self-hosted prelude
  `TString[*Ts: Movable & Writable]` (stdlib `std.format.tstring`) instead
  of an eagerly concatenated builtin `String`: the whole-program compiler's
  discovery fixpoint types each occurrence, materializes the concrete
  variadic specialization, and rewrites the expression into its
  construction, whose interleaved pack captures literal segments and
  interpolation snapshots at creation. Formatting defers to Writable
  `write_to`, so `print(t"…")` and explicit `String(t"…")` flow through
  the ordinary machinery, and t-strings nest. Capture is by typed value
  snapshot — copyable interpolations copy in, non-Copyable places
  snapshot as creation-time formatted strings — a documented deviation
  from real Mojo's borrow-holding `TString` (its exclusivity rejects
  mutating a captured value before use; Mojito prints the snapshot).
  Assigning a t-string to a `String` annotation is now a type error, and
  `TString` values reject copy/concatenation/equality. The raw
  parse-then-check seam and retained abstract generic bodies keep an
  output-identical eager-concatenation fallback, and generic applications
  inside interpolations now monomorphize (previously a latent gap).

- Grapheme segmentation and a `Codepoint` result type for the
  self-hosted String. `s[codepoint=i]` now yields a prelude-exported
  `Codepoint` carrying the decoded scalar plus the character's text
  (captured through the struct-to-literal bridge): `Int(cp)` via
  `Intable`, scalar-ordered comparison and equality, Writable printing
  as the character, `is_ascii()`, and `utf8_byte_length()`; direct
  construction is rejected until runtime scalar conversions land.
  `s[grapheme=i]` returns the extended grapheme cluster as a `String`
  substring and `grapheme_count()` walks the whole buffer, both raising
  on out-of-range indexes and truncated UTF-8. Segmentation implements
  a documented UAX #29 subset — hand-maintained Control/Extend/
  SpacingMark essentials ranges, regional-indicator pairing, and fully
  arithmetic Hangul — with GB11 simplified to "never break after ZWJ"
  (common emoji ZWJ sequences join) and GB9b (Prepend) omitted.

- Self-hosted String core. The stdlib gains a nominal UTF-8 `String`
  (byte buffer over `UnsafePointer[Byte]`), constructed explicitly from
  a literal (`String("...")`); annotations and non-literal `String(x)`
  conversions keep the builtin compile-time string until the type-split
  migration. The struct supports byte-length `len`, copy/move/drop,
  byte-wise equality and ordering, DJB2 hashing (Dict/HashDict keys),
  `print`/`repr` through Writable, explicit `s[byte=i]` and
  `s[codepoint=i]` access (pure UTF-8 leading-byte decode, raising
  bounds and validity errors; positional `s[i]` stays rejected), and
  boundary-checked contiguous slicing that raises on mid-sequence
  splits. En route, three general features landed: keyword subscripts
  (`x[name=i]` over value bases, dispatching keyword-only `__getitem__`
  overloads; named brackets over type names remain parameter
  application), keyword-only parameter names as part of overload
  identity, and scalar conversions (`Int`/`UInt`/`Bool`) accepting
  width-1 SIMD scalar aliases like `Byte`. Graphemes, a `Codepoint`
  type, lazy TString, and the literal-operations migration are recorded
  follow-ups.

- Cross-call transfer hardening. Transfer-effect visibility is now
  declaration-order independent: the checker reruns with the prior
  round's committed effects whenever a call site observed a stale callee
  entry, so a method calling a later-declared (or mutually recursive)
  storing method carries its effect to every caller, converging in one
  round for programs without order-sensitive effects. The store-outward
  escape rule now also covers stores inside nested `def`s through
  captured `self`/parameters (previously a diagnosed bypass that reached
  a stale-frame crash at runtime), unpack-into-place targets (the
  transferred-tuple shape now rejects with the escape diagnostic), and —
  through ordinary method selection — user in-place dunders
  (`sink += carrier` replays `__iadd__`'s transfer effects). Remaining
  residues (indirect-call effects, interior-precise destinations, the
  capture-effect channel) are recorded on the roadmap.

- Owned iteration of linear elements. `for var item in xs^` and owned
  comprehensions now accept a `List` of non-`ImplicitlyDeletable` elements
  when every element is transferred by guaranteed exhaustion: the bundled
  owned iterator's `ImplicitlyDeletable` gates are lifted, and only
  abandoning control-flow paths are rejected — `break`/`return`/`raise`, a
  raising call whose handler sits outside the loop (a `try` inside the body
  contains its error), and comprehension filters over a linear binder — each
  naming the element's `@explicit_destroy` obligation. With linear elements
  the iterator itself is linear (no `__del__`), so the exhaustion edge
  consumes it through the checker-selected `_finish(deinit self)` named
  destructor, an ordinary method call that frees the buffer as visible
  library code; a user-defined linear owned iterator without a finisher or
  unconditional destructor is rejected contextually. Deletable-element
  behavior is unchanged, including residual drops on early exit.

- Cross-call reference lifecycle and loan transfer. A callee's accepted
  store of a loan-carrying value into `self` or a `mut`/`ref` parameter
  is now a checker-recorded transfer effect replayed at every call:
  the store-outward escape rule fires across the call boundary, the
  caller's own escape analysis sees callee-installed loans, wrapper
  callables carry effects transitively outward, and MIR installs the
  transferred loans on the destination actual so ownership analysis
  rejects mutating or dropping the loan root while the stored alias
  lives (naming both variables) and keeps a borrowed source alive while
  a carrier collection holds a reference to it. Borrowed (`mut`/`ref`)
  parameter sources loan the actual's own storage; owned parameters only
  forward loans their moved values carry. The bundled
  `List.append`/`insert`/`__setitem__` are seeded, so appending a
  reference-carrying struct to a `List` is fully tracked. Alongside:
  value writes through a `ref`-typed field run the referent's copy
  lifecycle (non-`Copyable` writes reject, explicit `^` transfer stays
  raw), chained element-referent subscripts (`sink[0].value[0]`) verify
  and execute, and `capturing[_]` fields/elements retain the stored
  closure's concrete capture-origin set so escaping captured locals
  reject. v1 limits (permissive, recorded on the roadmap): effects use
  declaration-order visibility, indirect calls and abstract dispatch
  carry no effects, and destinations are tracked at root granularity.

- Reference-carrier lowering fixes. A bare struct name as an explicit
  compile-time type argument (`List[RefBox]()`) no longer emits a
  phantom runtime value register, so locals binding collections of
  origin-erased carrier structs construct and execute instead of failing
  MIR invariants. Assignment through a `ref`-typed field behind an
  aliased root now writes through the stored handle into the referent
  (cross-frame scalar write-through executes), and returned aggregates,
  `mut` writebacks, and cross-frame stores re-root their interior
  reference handles before the owning frame dies. Capturing closures no
  longer erase their environment into plain `def(...)` storage: fields
  and collection elements reject the coercion (with the environment
  shown in the diagnostic) while `capturing[...]`-annotated storage and
  call-position downward funargs are unchanged. The remaining
  call-boundary loan/lifecycle residue is consolidated into one roadmap
  item.

- Reference-escape analysis beyond returns. Stores into storage that
  outlives the frame — fields of `self`, parameter-rooted places, and
  `ref`-field rebinds — now reject frame-locally rooted loans at check
  time ("stored reference escapes storage outside its declared origin"),
  closing a checker-accepted use-after-free; parameter-rooted loans store
  outward freely, and both escape-context builders now include variadic
  collector parameters. The collection-store and closure edges are pinned:
  `List[ref T]` offers no handle-installing channel, and a stored closure
  can never be invoked after its captured referent dies. Reference result
  signatures accept the declaration-level immutable-origin cast
  (`ref[Origin[mut=False].cast_from[o]] T`, upgrade direction rejected),
  and mapping key iteration over Dict, HashDict, and StringDict now
  yields immutable key references (Mojo parity; writes through a `for
  ref` key binding reject, reads stay live borrowed references).

- Bound-generic monomorphization, Stage E: erased-dispatch retirement
  confirmed and the residue re-pinned through the authoritative pipeline.
  New Compiler-driven witnesses assert that a `comptime for`-conflict-
  retained template keeps the `__iterator_dispatch` protocol and its
  `CopyIteratorReference` adapter in MIR and executes them end to end, and
  that a bound generic used as a function value retains its template and
  invokes through runtime retargeting; the deliberate raw-seam machinery
  pins are annotated so a schema-freeze audit reads them correctly. The
  roadmap item is closed: erased dispatch survives only for the designed
  residue (function values/indirect calls, overloaded generic names,
  generic methods, comptime-class inferred calls, open instantiations,
  conflicting unrolled occurrences, and abstract-body pre-checks), and the
  six `mir::verify` abstract-dispatch witnesses to re-confirm at the
  backend-ready MIR checkpoint are now named on that roadmap item.

- Bound-generic monomorphization, Stage D: inferred applications. The
  compiler's pipeline now iterates discover→elaborate→check to a fixpoint,
  replaying each closed checker-recorded generic instantiation
  (`DefSpecializationRequest`) at its exact call occurrence, so inferred
  calls like `first_or(range(3, 7), -1)` run through concrete clones with
  no erased dispatch at the call site. A request can only upgrade a call:
  misaligned or non-closed arguments, occurrence conflicts from `comptime
  for` unrolling, and drifted spans all keep the abstract path, and a hard
  round cap reports inferred polymorphic recursion as the new
  `CompilerError::SpecializationDivergence` diagnostic. A retained
  bound-generic template now precedes its clones so an inferred recursive
  clone can reference it under sequential name binding. The stage-composed
  test seam stays request-free (Compiler-only machinery). Landing this
  surfaced and fixed three latent concrete-path bugs that erased dispatch
  had been masking: the drops pass now retains a direct ref-returning
  method call's receiver storage until the handle's last register use (a
  use-after-free reachable in plain concrete code); a ref-returning
  method's receiver — including a read receiver — passes as a reference so
  the returned handle roots in the caller's live slot; concrete scalar
  receivers resolve the rounding dunders (`__floor__`/`__ceil__`/
  `__trunc__`/`__ceildiv__`) to the same VM intrinsic the abstract
  Floorable-family dispatch uses; and type-binding substitution now
  rewrites the parser's bare-identifier (`Tuple[T, T]`) value-encoded
  arguments in annotations, fixing explicit applications of generics with
  parameterized signature types.

- Bound-generic monomorphization, Stages A-C. Explicit concrete generic
  applications now clone and re-check the body with the concrete type
  substituted, matching real Mojo's per-instantiation model. Every comptime
  specialization bakes its concrete type arguments into the clone
  (annotations, compile-time argument lists, and constructor heads) and drops
  them from the residual signature and rewritten calls, with each dropped
  parameter's trait bounds enforced at the requesting call via the new
  contextual `ComptimeError::GenericBound`; this fixes the explicit
  non-generic type-argument bug (`pick[Plain]`, `first_or[Range]`). Plain
  trait-bound generics with a unique top-level name join the specialization
  registry with soft resolution: unresolvable references (inferred calls,
  symbolic arguments, function values) stay on the retained template's
  abstract erased-dispatch path, and a dead template keeps its abstract
  pre-check. Generic-bound reference iteration now works through explicit
  application: the clone iterates concretely with ordinary borrowed loans.
  The checker also records every resolved generic instantiation
  (`CheckedProgram::generic_instantiations`, groundwork for monomorphizing
  inferred calls), `Ty::Assoc` copyability now derives from declared
  associated-member bounds instead of defaulting to copyable (the stdlib
  `Iterable.Element` bound strengthened to `Copyable & Movable`
  accordingly), and the stage-composed test seam's `VmBackend::run` enforces
  the pre-drop ownership analysis, closing the raw-path divergence.

- Mapping invalidation and borrowed-iteration safety (core). The Dict,
  HashDict, and StringDict key iterator (`_DictKeyIter`) now borrows the
  entries list through a parametric-mut struct origin and yields key
  references, replacing the snapshot copy, and every bundled borrowed
  iterator declares its yielded reference at
  `_get_owned_interior["element"]` granularity — the checker derives each
  borrowed source loan's granularity from that declared projection, retiring
  the List/Set/Dict collection-name whitelist from the production path (an
  unregistered-collection shim remains for the focused checker). Mapping
  mutation during iteration is now a defined, lazily rejected error across
  all three mappings — previously HashDict/StringDict were unprotected —
  while `d[key]` value reads stay legal through the sibling `value`
  generation and `keys`/`values`/`items` views remain eager snapshots by
  design. Signature lowering gained the two pieces the derivation needed: an
  uncarried origin parameter lowers to its checked semantic binder rather
  than an inferred union, and an interior projection off a parametric-mut
  origin parameter carries that parameter's declared mutability. New coverage
  pins the mapping rejections (statement and comprehension), value reads and
  coexisting shared iterations, the lazy-discard model, and the sound
  reference-escape rejections (returning a loop reference or a loan-carrying
  closure, mutating a source under a manually held borrowing iterator).
  Deferred to their own roadmap items: generic-bound reference iteration and
  store-outward escape analysis.

### Changed

- A deferred slot (`CtValue::Deferred`) now names the binder whose slot it
  fills by its declaration's identity rather than its spelling, so the Tuple
  closedness check no longer counts it as bound by an enclosing value binder
  that merely shares the spelling. The elaborator's markers for a name that
  is no parameter are a separate `CtValue::Marker`. MIR text writes the slot
  as a `binder` record and the marker as `ct_marker`, which moves the
  textual format to schema 1.5; artifacts of schema 1.0 through 1.4 still
  load, a spelled slot reading as an unbound reference.
- Developer infrastructure: the whole-corpus fixture sweeps are now one
  generated test per fixture in the `tests/corpus_test.rs` binary
  (libtest-mimic, the project's first dev-dependency), preserving each sweep's
  distinct pipeline entry path while letting the test runner schedule fixture
  compiles across cores; `scripts/check` runs the suite through
  `cargo nextest run`, and the `quick` nextest profile excludes the corpus
  binary for the iteration loop.

### Added

- Generic reference-yielding collection iteration. The bundled List and Set
  iterators now genuinely borrow their source and yield element references:
  `_ListIter[iterable_mut: Bool, //, T, iterable_origin:
  Origin[mut=iterable_mut]]` holds `ref[iterable_origin] List[T]` and its
  `__next__` returns `ref[iterable_origin] T`, with the reference's mutability
  resolved from the source at each loop or comprehension site — a mutable
  source yields writable `for ref` handles (named or temporary sources alike)
  while a read parameter yields immutable ones. The List-only `for ref` index
  desugaring is gone: reference iteration runs the ordinary checked
  `__iter__`/`__next__` protocol end-to-end, Set reference iteration works for
  the first time (through its delegated borrowed `_ListIter`, two borrow
  frames deep), and the iterator's source loans re-establish on each yielded
  binding so a structural invalidation names the user's variable. A `ref`
  loop target over an abstract generic `Iterable` bound is now rejected with a
  clear error (the abstract `Iterator.__next__` contract yields `Element`
  values; the previous behavior silently mutated a per-iteration copy).
  Compiler capabilities that landed with the migration: top-level struct and
  trait declarations register order-independently (shells, member types, and
  method signatures precede body checking, so same-module structs may
  reference each other in either order — with by-value self-containment now
  rejected explicitly), an infer-only `Bool` parameter binding a sibling
  origin's `mut=` erases from a struct's runtime parameters like the origin
  itself, generic substitution descends into `ref` field referents, and the
  specialization conformance oracle registers struct names before resolving
  field types. Mapping iterators still snapshot entries and yield copies —
  mapping invalidation is the next roadmap item.

- The bundled borrowed `Iterable` protocol is now origin-parameterized, as in
  current Mojo: `std.iterable` declares
  `comptime IteratorType[iterable_mut: Bool, //, iterable_origin:
  Origin[mut=iterable_mut]]: Iterator` with
  `def __iter__(ref self) -> Self.IteratorType[origin_of(self)]`, replacing the
  legacy monomorphic `Iter` member, and Range, List, Set, Dict, HashDict, and
  StringDict conform through the parameterized member with the
  application-spelled `ref self` return. The bundled conformers erase the
  origin in their member templates, so borrowed iterators still yield element
  copies; borrowing the source through the origin parameter and yielding
  references is the next generic-reference-iteration subtask. Two compiler
  gaps closed with the migration: trait conformance now enforces a
  parameterized associated member's declared bound (instantiating the
  definition's template with placeholder explicit arguments, discharging
  conditional cases through the struct's conformance assumption, and keeping
  the arity-only contract when an explicit value parameter has no fabricable
  witness), and generic borrowed `__iter__` dispatch now reaches a conformer
  whose borrowed receiver spelling (`self` vs `ref self`) differs from the
  abstract dispatch symbol — previously a `RuntimeError` for an overloaded
  borrowed/owned `__iter__` pair like migrated List's.

- Borrowed iteration sources now lower uniformly, in `for` statements and
  comprehensions alike: every borrowed named source is bound as a genuine
  reference (`MakeRef`) into a retained-source slot, the iterator object is
  normalized into a distinct slot, and whole-source versus interior borrowing
  is expressed only as loan granularity — a whole-place shared loan for a
  user iterable, an interior `element` generation for a concrete
  List/Set/Dict place — re-established on the long-lived iterator slot. The
  former collection-only `LoadPlace` single-slot bridge is gone.
  Comprehensions gain the statement loop's semantics they previously lacked: a
  comprehension over a named user iterable borrows its source instead of
  copying it (one `__del__`, source usable afterward), mutating a borrowed
  source mid-comprehension is now rejected as a loan conflict or interior
  invalidation (previously a silent copy permitted it), and a comprehension
  over a borrowed user-iterable temporary no longer leaks the source — it is
  retained in its own slot and destroyed exactly once, after the
  comprehension.
- Loop and comprehension targets now model their binding convention —
  unadorned (immutable), `var`, and `ref` — independently of whether the source
  is borrowed (`for x in xs`) or consumed (`for x in xs^`). Each of the six
  combinations carries an explicit checked requirement: a `var` target moves an
  owned result or lifecycle-copies a yielded reference (requiring an
  `ImplicitlyCopyable` referent); an unadorned or `ref` target binds an owned
  result directly into per-iteration storage (requiring a droppable element —
  `ImplicitlyDeletable` or `Copyable` — dropped each iteration) or retains a
  yielded reference handle to read/write through the borrowed referent. The raw `__next__` result is retained in a
  compiler-owned slot and adapted to the target only on the yielded edge, so
  moves and copies never run on the `StopIteration` path. Mutating an immutable
  target, and a copying `var` target over a non-`ImplicitlyCopyable` reference
  result, are rejected with contextual checker errors.
- Trait conformance now accepts Mojo's directional `__next__` result
  refinement: a concrete `ref[o] T` result may implement an abstract value
  result `T` only when the referent matches exactly and `T` is proven
  `Copyable`. The reverse direction, mismatched referents, and non-`Copyable` values
  remain errors. Abstract method calls and generic loop advancement retain an
  explicit checked/HIR/MIR adapter; after runtime retargeting, the VM consults
  the concrete declaration ABI and performs the reference read plus lifecycle
  copy. Caller and just-returned iterator frames remain reachable while user
  copy code runs, so a `Copyable` element containing reference handles can read
  its nested referents. Registered `Iterator` declarations are now authoritative
  instead of being bypassed by the focused checker's builtin compatibility
  marker.
- Structural iterator selection now preserves a reference-returning
  `__next__` as an origin-bearing `Ty::Ref` result through the checked protocol,
  HIR, typed MIR, verification, and VM dispatch. Previously the VM happened to
  carry the reference handle through a register statically typed as the
  referent, leaving the checked/MIR boundary inconsistent even though simple
  reference-yielding loops executed.
- A method may now return the *dereference* of an origin-bearing pointer field
  whose origin is a struct/callable parameter (`def get(self) -> ref[o] Int:
  return self.p[0]` on `struct Borrow[o: Origin[...]]` with `var p:
  UnsafePointer[Int, Self.o]`). `UnsafePointer(to=v)` is a runtime handle straight
  at `v`, so the returned `ref[o]` re-roots at the single pointee at the return
  boundary and the VM forwards the pointer's offset-0 index as the identity deref
  of that pointee — an immutable origin reads, a mutable origin writes through the
  caller's storage. Previously the checker rejected this shape (`escapes storage`)
  because the residual offset-0 index had no runtime forwarding.
- A `for` loop over a user-defined iterator whose `__next__` returns a *reference*
  into the borrowed source now executes: the yielded reference flows through the
  loop as a handle. The loop invokes `__iter__`/`__next__` with the loop frame
  reachable — previously the synchronous protocol-call path drove the callee with
  its caller popped off the frame stack, so a user iterator holding a `ref` into the
  loop frame could not dereference it (`vm: stale reference to frame N`) — and a
  borrowed `__iter__(ref self)` receives a `ref self` handle so the iterator's
  borrow roots at the live loop frame. Supported for an owned-temporary source
  (`for x in Numbers(3)`), retained and dropped exactly once after the loop. A
  *named* source (`for x in nums`) is now borrowed rather than copied: the source
  slot binds a genuine reference (`MakeRef`) and the whole-source dependency is
  recorded as a shared loan on the iterator, so the source is not copied, stays live
  through the loop without the `KeepAlive` liveness hack, and mutating it during
  iteration (a `mut self` call, reassignment, …) is rejected as a loan conflict.
- A method may now return a `ref[origin] T` field or binding whose origin is a
  struct/callable origin *parameter* (for example `def get(self) -> ref[o] Int:
  return self.slot` on `struct Cell[o: Origin[...]]`). The stored handle already
  names its borrowed region, so returning it stays within the declared origin;
  previously the return contract re-synthesized the handle's storage as a place
  rooted at the receiver and rejected it as an escape. Immutable origins yield a
  read-only borrow and a mutable origin returns a write-through handle to the
  caller's storage. This is a foundation piece for generic borrowed reference
  iteration.
- A method may now also return a reference obtained by *indexing/projecting
  through* a `ref[origin] <aggregate>` field (for example `def at(self, i: Int) ->
  ref[o] Int: return self.src[i]` on a `ref[o] List[Int]` field). The VM re-roots
  the returned handle at the borrowed storage — following the stored `ref`/pointer
  handle across frames, including through a `mut`/`ref self` receiver — so it
  survives the accessor frame instead of dangling (`vm: stale reference to frame
  N`). Dereferencing an origin-bearing *pointer* field and returning it
  (`self.p[0]`) remains rejected: its place lowering keeps an offset-0 index the
  runtime cannot yet forward.

### Fixed

- A reference returned from a struct method whose declared origin is a struct
  origin parameter, then bound to a `ref` local, now keeps its ultimate source
  alive instead of dangling (`invalid reference projection … on None`). The
  caller-side origin resolution maps the struct origin parameter to the origin the
  receiver's `ref[o]` field borrows (recorded at construction), so the returned
  reference records a loan on the owner and drop elaboration keeps it live while
  the reference is used — previously the abstract parameter was dropped by the loan
  machinery and the owner was freed early. A `mut self` reference-yielding accessor
  (`def take(mut self) -> ref[o] Int: return self.src[i]`) bound to `ref` locals
  now reads and writes through end-to-end.
- Reading a `ref[origin] <aggregate>` field's referent under a `mut self`/`ref
  self` receiver (subscript `self.src[i]`, `len(self.src)`, …) no longer fails with
  `vm: checked nominal subscript receiver is ref`. A borrowed receiver is a runtime
  alias, so the `LoadPlace` fast-path reached the field's stored handle but skipped
  the `ref`-typed post-dereference the by-value path applies; the value load now
  yields the referent under every receiver convention. (Value reads only — a
  *returned* reference bound to a `ref` local still needs its source loan, tracked
  as later borrowed-iteration work.)
- Borrowed iteration over a temporary that owns its storage no longer leaks the
  source. `for x in Numbers(3)` (or `for x in make_list()`) normalized the
  iterable to an iterator *in place*, overwriting the source in its only slot, so
  its `__del__` never ran and a borrowing iterator aliased freed storage. The
  borrowed-iteration source and iterator now occupy distinct slots (`GetIter`
  reads the source, writes the iterator into its own slot); the source stays live
  through the loop via a liveness anchor and is destroyed exactly once after it,
  including on early `break`/`return`. Owned iteration keeps the single slot
  (`__iter__(var self)` consumes the source), and concrete List/Set/Dict borrowed
  iteration is unchanged (its named place is retained by an external loan). This
  also gives a future origin-bearing iterator a live source to loan.

### Changed

- A deferred slot (`CtValue::Deferred`) now names the binder whose slot it
  fills by its declaration's identity rather than its spelling, so the Tuple
  closedness check no longer counts it as bound by an enclosing value binder
  that merely shares the spelling. The elaborator's markers for a name that
  is no parameter are a separate `CtValue::Marker`. MIR text writes the slot
  as a `binder` record and the marker as `ct_marker`, which moves the
  textual format to schema 1.5; artifacts of schema 1.0 through 1.4 still
  load, a spelled slot reading as an unbound reference.
- The bundled owned-iteration protocol now uses current Mojo's monomorphic
  `IteratorOwnedType`. `IterableOwned`'s associated iterator member (and `List`'s
  conformance) is renamed from the legacy `OwnedIter` to `IteratorOwnedType`; a
  consuming iterator owns its storage, so the member needs no origin parameter.
  The borrowed `Iterable` trait still uses the legacy monomorphic `Iter` member —
  migrating it to origin-parameterized `IteratorType[origin_of(self)]` needs
  self-origin resolution and lands with generic borrowed reference iteration.

### Added

- A parameterized associated-type application on a concrete struct base now
  resolves. A conformer may spell the application directly as its own return type
  (`def __iter__(ref self) -> Self.IteratorType[origin_of(self)]`): with the
  concrete struct substituted for `Self`, the indexed application routes through
  the struct's parameterized member instead of failing as a dependent type index.
  This is the faithful current-Mojo `__iter__` shape for the borrowed `Iterable`
  conformers. A generic free function returning `C.IteratorType[origin_of(c)]`
  remains later work (a declaration-time value-parameter-origin gap).

- Self-origin resolution for a parameterized associated type. A trait method's
  abstract `Self.IteratorType[origin_of(self)]` has no bound `self` place; its
  receiver origin now lowers to the symbolic `Origin::SelfParam` (the
  `Origin`-level analogue of the signature contract's `SigOrigin::Self_`), so the
  application carries its origin argument instead of collapsing to zero args. A
  conforming struct then resolves the origin-parameterized member concretely (the
  origin erasing from the runtime ABI like a pointer origin), so a requirement
  returning `Self.IteratorType[origin_of(self)]` is satisfiable and conformance
  succeeds. This is the self-origin prerequisite for migrating the borrowed
  `Iterable` protocol; the reference-yielding iteration runtime and the stdlib
  migration remain later work.

- Concrete parameterized-associated-type substitution. The checked `Ty::Assoc`
  now carries a parameterized application's arguments — `TyArg` gained a
  first-class `Origin` variant, so an origin argument participates in checked type
  identity while erasing from the runtime ABI like a `Ty::Pointer` origin. When a
  conforming struct instantiates a parameterized member, the application resolves
  concretely by substituting the arguments into the member's lowered template: a
  type-parameterized member (`C.Wrap[T]` → `List[T]`) resolves end-to-end through
  checked declarations, specialization, HIR, verified MIR, and the register VM.
  (An `origin_of(self)` origin argument now resolves too — see the self-origin
  entry above. Forwarding a value parameter into another parameterized struct
  remains blocked by a pre-existing generic value-forwarding gap.)

- Parameterized associated types (foundation). Trait and struct compile-time
  members now retain a parameter list — type, value, and origin parameters with
  the `//` infer-only boundary — so current Mojo's
  `comptime IteratorType[iterable_mut: Bool, //, iterable_origin:
  Origin[mut=iterable_mut]]: Iterator` parses and checks. A parameterized
  application such as `Self.IteratorType[origin_of(self)]` (spelled like a
  dependent index but naming a parameterized member) is recognized, validated
  against the declared explicit-parameter arity, and resolves to a symbolic
  associated type.

- Augmented assignment on a user-defined value now dispatches to its dedicated
  in-place dunder — `x += y` selects `__iadd__(mut self, y)` (and `__isub__`,
  `__imul__`, `__itruediv__`, `__ifloordiv__`, `__imod__`, `__ipow__`) as a
  checked `mut self` method call carried through checked HIR and verified MIR,
  mutating the receiver in place for variable, projected-field, and
  nominal-subscript-element targets. Mojo no longer falls back to the ordinary
  `__add__` family: a missing in-place dunder is a hard error, an immutable
  receiver and a mismatched right-hand side are rejected, and a raising in-place
  dunder participates in ordinary `try` handling. A nominal-subscript element
  dispatches through both getter paths — a value getter materializes the element
  into a mutable temporary and writes the result through `__setitem__`, while a
  mutable-reference getter applies the in-place dunder through the handle. Native
  scalar targets keep the builtin read-modify-write.

- Method-dispatched nominal `Index`, `Slice`, `MultiIndex`, and `MultiSet` now
  carry one complete
  checker-selected method-call contract through checked HIR and verified MIR:
  the exact target, executable result, and typed error; receiver and argument
  conventions and caller places, capture accesses, generic value arguments,
  reference-result origin,
  and setter write-back. Raising accessors participate in ordinary `try`
  handling; receiver/argument alias checks and persistent loans use the
  effective access convention. A reference-returning subscript is evaluated
  once into a hidden reference slot with its owner loans, so it can serve
  directly as a chained receiver or place, including through a reference-valued
  aggregate and when passed dynamically to a `mut`/`ref` parameter. Setter
  overload selection distinguishes positional and keyword-only `value` shapes,
  validates the actual right-hand-side type, and preserves source evaluation
  order (receiver, indices/bounds, then right-hand side); heterogeneous
  variadic Indexer positions normalize against their own selected element type.
  Ordinary index operands execute selected user `@implicit` conversions, while
  synthesized slice descriptors permit only descriptor-family widening and
  reject arbitrary user wrapping before MIR. Owned nominal element reads retain
  their selected accessor and copy lifecycle in verified MIR.
  A field projected below a nominal reference-returning accessor extends the
  one-evaluation hidden handle, so direct `ref` bindings and dynamic `mut`/`ref`
  actuals never flatten the accessor into a raw nominal index place. Reference
  returns use the same handoff, and subsequent Pointer, SIMD, and private Tuple
  indices retain typed projection metadata. The MIR verifier checks hidden
  handle slots, referent-versus-handle terminal types, dynamic index-register
  types, and projected element types while allowing analytical loan paths to
  retain nominal interior-origin projections.
  Augmented nominal subscripts retain call-local getter and setter conversions
  and effects without reevaluating the receiver or indices. A value getter
  follows the pinned receiver/raw-index, RHS, getter-conversion/getter,
  operator, setter-conversion/setter order and reloads a getter-mutated caller
  place before the setter. A mutable-reference getter instead establishes the
  lvalue before the RHS and writes directly through its handle without calling
  a setter. Keyword-only setter values retain their selected implicit
  conversion.
  Write-through assignment to a runtime reference preserves the handle slot's
  `ref` type, including free-function returns with union origins. Borrowing a
  reference-valued aggregate field now receives an outer `ref (ref T)` type,
  while projections through an existing reference capability preserve its
  mutability instead of manufacturing a mutable handle. Substituted local
  aliases likewise retain their checked `ref T` capability type even though no
  runtime handle is stored. Cloned declarations in sibling compile-time scopes
  receive distinct typed HIR/MIR slots, so heterogeneous specializations cannot
  overwrite one another's place type. Indexing `List[ref T]` peels the outer
  reference to the element slot before an augmented write or chained method
  call, so the operation reaches the stored referent instead of replacing its
  handle. Bare-name augmented assignment in a structured region resolves its
  checked binding identity for both halves, including same-spelled sibling
  `ref` declarations.
  Retained reference receiver/argument places are revalidated after every later
  index argument has run, without rereading ordinary copied arguments.
  Resolving a competing positional-only/keyword-only `value` overload pair is
  an explicit Mojito extension because the pinned nightly currently rejects
  that focused pair.
  MIR declarations retain receiver presence and exact fixed-parameter
  conventions, while verification checks concrete declaration contracts and
  the internal consistency of abstract trait-bound subscript results. Call-less indexing/slicing carries an explicit
  intrinsic family discriminator, so the VM never infers Tuple/pack, SIMD,
  pointer, compile-time-list, or String-slice semantics from a runtime value.
  The narrow nominally typed `Slice.indices()` result explicitly uses the
  private Tuple-storage bridge.

- Public `List`, `Set`, `Dict`, `Range`, and heterogeneous `Tuple` values are
  now nominal self-hosted structs supplied by the implicit prelude. Collection
  displays preserve contextual element inference and lower to ordinary
  constructors; comprehension leaves call `append`, `add`, or `__setitem__`;
  supported indexing, sizing, and containment use the same checked methods as
  user structs. Borrowed List/Set/Dict/Range iteration and owned List iteration
  likewise use selected nominal iterator methods; public Tuple has no runtime
  iteration contract. An `Indexer` argument is evaluated once and normalized
  through the checker-selected `__mlir_index__` only when the selected subscript
  needs `Int`; a direct overload accepting the source index type takes
  precedence. Checked single-index assignment retains its exact selected setter
  and receiver place through `MultiSet`, including a receiver rooted in a
  `mut`/`ref` parameter, so nested collection mutation cannot bypass lifecycle
  behavior by overwriting raw backing storage. Nominal `in` and `not in` retain
  the container place through the selected `__contains__` call, and the
  `Writable` `print`/`String`/`repr` formatting paths retain nominal argument
  places through the intrinsic call; pointer-backed collection storage is
  therefore borrowed rather than copied into a short-lived shallow owner.
  Concrete borrowed List, Set, and Dict place iteration retains the live owner's
  interior generation. Dict lookup replaces exactly its `value` owned-interior
  generation and all ordinary projections below it, matching Mojo without
  invalidating the sibling `element` generation used by key iteration. List
  additionally observes nonstructural element replacement and rejects a later
  iterator use after structural invalidation;
  List `for ref` writes through checked element handles. These are concrete
  compiler bridges, while current Mojo's origin-parameterized associated `IteratorType[...]`
  remains roadmap work. Owned List iteration also still requires
  `ImplicitlyDeletable` elements, including when a loop is statically guaranteed
  to exhaust a linear List. Exhaustion and `break` destroy the synthetic owned
  iterator at their common exit; a `return` materializes its result, runs pending
  `finally` regions, then destroys current and residual elements from the
  innermost loop outward. An ordinary value read through a returned reference
  now requires a `Copyable` referent and lowers to explicit typed `CopyValue`, so
  binding a Copyable nested collection runs its copy lifecycle while an explicit
  `ref` binding retains even a linear referent without duplicating it.
  HashSet's implicit deletion and bucket-replacing `add` operation are
  conditionally available only when its element type is
  `ImplicitlyDeletable`, matching the nested List setter's lifecycle contract.
  Checker-proven consuming reads of projected Copyable fields likewise lower to
  `CopyValue`; returning or assigning a pointer-owning field now invokes its
  nested copy initializer instead of creating a shallow second owner, while
  borrowed receiver and formatting reads remain place-preserving.
  The native List/Set/Dict/Range runtime variants and
  dedicated collection-construction MIR operations have been removed. Legacy
  flat standard-library modules are now thin public re-export facades over the
  authoritative `std.*` modules; explicit named re-exports can deliberately
  overlap an implicit-prelude name without causing ordinary modules to re-export
  the prelude.
  `Ty::ComptimeList`/`Value::ComptimeList` remain only at the CTFE bridge, while
  `Ty::Tuple`/`Value::Tuple` and `MakeTuple` remain only as private heterogeneous
  storage behind `__RuntimeTuple` and the specialized runtime-pack ABI. Static
  private-pack element projections retain distinct move state, so whole-pack
  transfer relocates linear elements without double destruction. Public Tuple
  specializations retain exact generic callable element contracts through
  opaque compiler-only annotation ids and a checker-seeded semantic map, rather
  than synthesizing a lossy source function type. They provide comparison,
  membership, reversal,
  concatenation, and dependent-handler `consume_elements`. Their lifecycle
  conformances are element-conditional: consuming transforms implicitly copy
  only `ImplicitlyCopyable` tuples and otherwise require `^`; indexed transfer
  of a non-`ImplicitlyCopyable` public element remains rejected in parity with
  Mojo.
- Current Mojo closure declarations now accept brace capture lists directly
  after effects, including `imm`, `mut`, `ref`, `var`, move, and default forms;
  the removed `unified {...}` position remains an explicit Mojito-only
  compatibility spelling. Closure environments materialize when their nested
  declaration executes, so owned captures snapshot or transfer at declaration
  while reference captures retain live frame/slot handles without imposing a
  declaration-to-call loan. Lexical declarations
  lift recursively at arbitrary depth with exact shadowing, permitted
  intermediate forwarding, effects, argument markers, named results, and
  reference-return ABIs. Explicit Origin arguments now participate in direct
  overloaded/generic calls and in contextually selected overloaded function
  values; their `mut`/`ref` conventions and origin-bearing results survive
  indirect calls. Callable structs with same-arity `__call__` overloads now
  retain the contract-matching lowered target through typed MIR and both VM
  indirect-call paths instead of reconstructing a target from arity. Captured
  nested Origin specialization also runs as a documented Mojito extension over
  the pinned nightly.
- Generic `F: def(T) -> T` bounds now retain a complete dependent callable
  contract, type calls through `F` inside the generic body, and validate each
  specialization against a monomorphic function or nominal callable struct with
  directional convention/effect compatibility. Anonymous contracts and callable
  values may also declare their own alpha-equivalent `def[...]` binders,
  including value defaults that govern invocation without becoming conformance
  identity. An explicit `thin` or `capturing[...]` qualifier creates a
  callable-value parameter; `OriginSet`, `capturing[_]`,
  `capturing[origins]`, and infer-only `//` syntax survive checking, and a
  supplied function or `@parameter` closure is reified as a hidden typed MIR
  local for indirect execution. Omitted callable arguments use a symbolic
  declaration-order plan for a selected function, an earlier callable
  parameter, or a compile-time conditional; generic specializations retain and
  reify these values rather than storing a function or closure in `CtValue`.
  Generic indirect-call MIR preserves named bracket arguments and the selected
  anonymous contract, so its scalar and callable defaults govern partial calls
  even when the concrete implementation declares different defaults; the
  verifier checks that parameter, effect, and reference-result metadata.
  Scalar-controlled `comptime` specialization can therefore residualize a
  captured callback, and variadic type/value packs may be interleaved with
  explicit positional or named `Origin` arguments. Callable types retain
  default/thin/capturing environments and canonical concrete capture origins
  with read/write access. Checked call adjustments and MIR call instructions
  carry those accesses into persistent-loan analysis, including calls through a
  non-escaping callable argument. This does not add arbitrary callable CTFE or
  escaping closures; unqualified stateful downward funargs remain a
  pinned-nightly Mojito extension.
- Checked syntax now has collision-free concrete occurrence identities after
  compile-time and trait-default cloning. Equal source provenance no longer
  aliases expression, declaration, overload, or type facts; MIR source maps
  still expose only file/byte provenance. Nested captures retain exact owner,
  storage type, and convention facts, including explicit-unused entries and
  intermediate forwarding. Runtime loop, tuple-unpack, and exception-handler
  binders keep distinct typed owner slots through HIR and structured regions, so
  same-name shadows and nested handler closures cannot overwrite outer values.
- Reference-bearing calls now use one caller-handle ABI in ordinary frames and
  synchronous `try` regions. Direct, indirect, method, callable-struct, and
  handwritten-constructor paths retain positional and keyword places; a
  temporary mirror of the caller frame preserves projected and aggregate
  reference returns plus mutations on raising paths. Reference navigation also
  crosses `UnsafePointer` and self-hosted nested-List storage without losing the
  caller identity.
- Nested heterogeneous-pack functions now specialize in their lexical context
  with scope-qualified declaration identities and are emitted at their original
  declaration site. Independent empty and nonempty instances, compile-time
  value parameters, explicit captures (including an outer runtime pack),
  defaults, keywords, named results, and sibling pack forwarding retain their
  selected ABI through recursively nested lifting. Whole-pack forwarding after
  a fixed positional prefix moves its Tuple collector as one value, so linear
  elements are not copied or illegally transferred through tuple indexing;
  keyword/default tails retain their normal slots, and call inference selects
  only the variadic overflow. Multiple spreads and explicit positional overflow
  after a spread are rejected in parity with the pinned nightly.
- Compile-time pack rewriting now assigns private, monotonic binding identities
  in separate value and type namespaces. Specialized `$pack[...]` parameters
  are recognized from their declaration rather than a source-name table, so
  block, loop, comprehension, nested-function, nested-type-parameter, and
  sibling-method shadowing cannot be mistaken for the outer pack. Empty packs
  remain distinguishable from non-pack values, and HIR loop lowering now gives
  loop binders lexical slots and restores an outer same-named binding after the
  loop.
- Collection-owned interior references now carry named, field-sensitive
  generations from checked HIR into typed MIR. `EstablishLoans` groups every
  dependency of a reference-bearing binding, while `InvalidateInteriors`
  records structural List mutation, Dict generation-defining lookup, Variant
  replacement, mutable/ref calls, whole-owner replacement through direct,
  reference-field, and pointer access, and replacement of interiors that own
  deeper interiors. Forward dataflow rejects a later stale use across branches,
  loops, and exact normal/raising/return-or-escape paths through nested
  try/except/else/finally regions and points to both the use and invalidation;
  ordinary owner reads, overlapping element aliases, direct List element
  writes, freshly rebound ordinary or interior generations, union-valued
  interior returns, and ordinary reborrows through a parent reference remain
  valid. Current
  `origin_of(place)._get_owned_interior["tag"]` return contracts are parsed and
  retain the complete projected receiver path. Drop liveness follows the
  reaching grouped generation, so rebinding a reference-bearing aggregate
  releases its old owner and retains its replacement without double-drops;
  transient MIR-register provenance keeps the selected owner alive through the
  complete consuming expression and no longer than that.
  Bare `ref` parameters/receivers now preserve parametric mutability without
  granting body writes, and immutable references cannot be escalated through an
  explicitly mutable origin contract.
- Heterogeneous function-pack bounds are now checked before specialization by a
  declaration-only conformance oracle shared with the checker. A failed call
  identifies the one-based pack element, its concrete type, the pack and trait,
  and the requesting instantiation instead of failing later in a generated
  body.
- The private heterogeneous pack carrier follows Mojo's left-to-right
  destruction order under conservative owned-root drop elaboration, including
  public-Tuple backing fields and exceptional edges. Specialized heterogeneous
  `*args^` forwarding relocates its whole moved pack through an explicit checked
  ABI, so the callee's source cleanup cannot destroy an element a second time;
  ordinary tuple-valued homogeneous variadics remain nominal List collectors.
  Transfers from non-implicitly-copyable indexed public tuple values are
  rejected instead of being silently copied and destroyed twice.
- Arbitrary-precision numeric literals now survive the whole compiler pipeline:
  integer spellings use `BigInt`, finite floating spellings use exact rationals
  with signed-zero preservation, literal-only arithmetic and CTFE remain exact,
  and typed MIR carries exact constants plus an explicit `MaterializeLiteral`
  boundary. Integer scalar/lane materialization wraps to the destination width;
  binary32/binary64 materialization rounds once from the exact value. Checked
  generic value-parameter declarations now cross into MIR/VM metadata so
  reification materializes at the declared type instead of leaking a literal
  value into an erased runtime slot. The differential arbitrary-precision case
  now matches Mojo.
- The tracked nightly target advances to Mojo 1.0.0b3.dev2026072505. Newly
  exposed interior-origin invalidation, collection-literal initializer
  inference, origin-parameterized associated iterator types, and the
  `SIMDLength` rename are recorded in dependency order. Interior origins,
  nominal collection inference, and concrete borrowed List provenance now run;
  general parameterized associated iterator types precede Unicode String and
  SIMD work in the MIR-schema-prerequisite roadmap.
- Variadic-generic structs: `struct S[*Ts: Bound]` declarations are specialized
  by compile-time elaboration per explicit instantiation (`S[Int, Bool](...)`),
  mirroring pack functions. Pack-typed members such as `var storage: Tuple[*Ts]`
  expand to the concrete element list, per-index reads (`s.storage[0]`) carry
  the exact element type, and specializations construct, copy, move, and drop as
  ordinary concrete structs. One trailing type pack (and no other compile-time
  parameters) is supported; instantiation requires explicit bracket arguments;
  a bare or argument-less template use and runtime-varying pack indexing are
  rejected with contextual errors. Struct annotation sites are now identified by
  the struct's unique name and each specialization carries a distinct source
  tag, so checked facts no longer collide across specializations sharing the
  template's spans.
- Variadic struct methods bind heterogeneous packs: real Mojo's pack
  constructor `def __init__(out self, var *args: *Ts)` (with the scoped
  `Tuple(*args^)` spread) specializes per instantiation, each constructor
  argument is checked against its per-index element type with exact pack arity,
  and method bodies can use `len(args)`/`args[i]`/`comptime for` over the pack.
  Method calls with a specialized heterogeneous variadic now score per-position
  everywhere (previously every overflow argument checked against one erased
  element type).
- Compile-time parameter subscripts use current Mojo's
  `def __getitem_param__[i: Int](...)` hook. General structs pass the source
  index as a checked value parameter, while a dependent variadic accessor
  returning `Ts[i]` unrolls into one concrete accessor per element at
  specialization. Public Tuple now defines only the current hook; the earlier
  `__getitem__` spelling remains an intentional compatibility fallback for
  user templates. `s[k]` requires a compile-time-constant in-range index, is
  typed by that element's exact type, and carries the checker-resolved accessor
  on MIR `Index`, so the VM does not guess a name. Reference-returning current
  hooks also receive value twins for implicitly-copyable rvalue subscripts, and
  explicit `ref` bindings retain the checked returned handle.
- Builtin scalar operators, comparisons, conversions, and rounding are typed
  through checked operation traits rather than ad-hoc numeric rules. Per-operator
  traits (`Addable`, `Subtractable`, `Multipliable`, `Divisible`,
  `FloorDivisible`, `Modable`, the bitwise/shift set, and `Negatable`) join the
  existing `Comparable`/`Equatable`/`Intable`/`Floatable`/`Boolable`/`Absable`/
  `Roundable`/`Powable`, so generic numeric code (`def f[T: Addable](a: T, b: T)
  -> T: return a + b`) type-checks and a struct declaring an operation trait must
  define its dunder. User structs now dispatch prefix operators (`-x` →
  `__neg__`, `not x` → `__bool__`) and concrete `Int()`/`Float64()`/`Bool()`
  conversions and `abs()`/`round()` through their dunders, matching the paths
  opaque generic parameters already used. Result types and execution are
  unchanged for existing programs; scalar execution stays primitive.


## [0.2.0] - 2026-07-19

Current-Mojo alignment through the pinned 1.0.0b3 nightly, executable origin
and pointer loans, and completely typed, semantically verified MIR — the
milestone gating the textual MIR/VM schema and native-backend work.

### Changed

- A deferred slot (`CtValue::Deferred`) now names the binder whose slot it
  fills by its declaration's identity rather than its spelling, so the Tuple
  closedness check no longer counts it as bound by an enclosing value binder
  that merely shares the spelling. The elaborator's markers for a name that
  is no parameter are a separate `CtValue::Marker`. MIR text writes the slot
  as a `binder` record and the marker as `ct_marker`, which moves the
  textual format to schema 1.5; artifacts of schema 1.0 through 1.4 still
  load, a spelled slot reading as an unbound reference.
- The public `Backend` trait object is now a statically dispatched enum over
  concrete implementations. `BackendKind` recognizes the planned
  `vm`/`cranelift`/`ebpf`/`llvm`/`mlir` seams; `BackendKind::make` parses a
  backend name and constructs it, and recognized-but-unimplemented backends
  refuse construction.

### Added

- MIR is completely value- and instruction-typed: every register — expression
  results, synthetic handles, markers, control-flow and iterator temporaries —
  carries a checked type, recorded at emission or copied from existing
  instruction facts by a closing pass that never re-implements checker
  inference. Functions and callable declarations retain their checked return
  types, raising contracts, and per-slot types; the last source-annotation
  reads left MIR lowering, and parameter slot types now come from checked
  declaration facts instead of name-matched body expressions.
- `mir::verify` is the standalone semantic verifier of record over MIR plus
  checked declaration metadata: place and projection consistency, register
  bounds and type completeness, store/binding/return/call-argument type
  consistency through the checker's coercion predicate, CFG-edge validity,
  effect protection for raising sites, and reference write-back invariants.
  The compiler pipeline gains a dedicated verification stage
  (`CompilerError::Verify`) composed with ownership analysis over one lowered
  program, the VM re-verifies the drop-elaborated program it executes, and the
  CLI `check`/`own` commands consume the same checked pipeline instead of
  silently re-checking.

- `UnsafePointer(to=place)` infers an origin-bearing pointer whose provenance is
  the concrete source place, with mutability taken from the owner binding. The
  checked pointer type retains the origin through HIR and MIR; the VM represents
  the value as an origin-free frame/slot handle. Pointer bindings and
  pointer-storing aggregates carry executable owner loans: the owner stays alive
  through the pointer's last use, and overlapping access, owner invalidation,
  and dangling escapes (`PointerEscapesOrigin`) are rejected statically. A place
  pointer binds a declared field origin parameter at aggregate-storage sites
  without inventing mutable capability, and non-zero offsets, arithmetic,
  comparison, and `free()` on origin-bearing pointers are rejected as a strict
  subset.

- Source imports now follow the current source-side namespace rules: source
  packages beat same-named source modules, ordinary directories can form dotted
  namespace paths, every dotted prefix binds, and submodules require explicit
  import or package-initializer re-export. Compiled `.mojoc`/`.mojopkg` lookup is
  reserved for the versioned artifact work.

- Homogeneous `**kwargs` collectors now use the self-hosted, insertion-ordered
  `StringDict[T]`. A final `**kwargs^` consumes and forwards its entries through
  the shared call binder with duplicate and element-type checking.

- Slice syntax now distinguishes `ContiguousSlice` and `StridedSlice`, preserves
  optional/negative bounds, implements `indices(length)`, and dispatches checked
  mixed or variadic `__getitem__` and `__setitem__` arguments, including slice
  assignment. Built-in collection view/API parity remains standard-library work.

- `std.utils.Variant` now supports compile-time type-membership queries,
  checked and unchecked consuming extraction, and checked and unchecked
  ownership-returning replacement. Unsupported arms reject statically, checked
  operations validate runtime tags, and `take` participates in use-after-move
  analysis.

- Current Mojo literal spellings now include leading/trailing-point floats,
  exponent forms, repeated/trailing digit separators, raw and case-insensitive
  string prefixes, one-to-three-digit octal escapes, triple-string line
  suppression, adjacent ordinary and t-string forms, nested interpolation
  boundaries, and the `Byte == UInt8` alias. Mojo does not define a distinct
  byte-string literal family.

- `CheckedProgram` now exposes stable checked expression and declaration arenas
  with child identities, resolved types, value/place/type categories, binding
  owners, extensible effect facts, and explicit semantic adjustments. Call,
  conversion, move, and explicit-destruction decisions are canonical node data.
  VM CTFE now passes rewritten fragments through the authoritative checker, and
  MIR retains checked types for source-derived registers.

- Checked HIR now retains stable checked-node identity, resolved type, value
  category, and semantic adjustments through function and exception-region CFGs.
  MIR consumes checked call/conversion/destruction decisions directly. Stored
  origin-parametric reference fields preserve frame/slot handles and owner loans,
  and user-defined slicing dispatches a checked `Slice` through `__getitem__`;
  slice-descriptor selection coexists with the canonical selected-call
  adjustment instead of replacing it.

- Checked HIR and MIR places now retain root, per-projection, and final storage
  types. Production lowering verifies complete typed-place metadata before VM
  execution, and reference field reads/writes use the checked storage type rather
  than rediscovering reference semantics from runtime values.

- Unsafe pointers now retain allocation provenance and typed offsets, support
  arithmetic, same-allocation subtraction, equality, aligned allocation and
  non-null dangling placeholders, and diagnose out-of-bounds access, invalid
  frees, double frees, and use after free. Static, untracked, and unsafe-any
  reference origins now lower into checked contracts, and local reborrows retain
  executable reference handles. The differential manifest now records that the
  pinned nightly has replaced static `UnsafePointer.alloc[_aligned]` and
  `pointer[0]` dereference with free `alloc[T](...)` and `pointer[]`; migrating
  that public spelling remains standard-library/syntax work.

- CPU-language surface work now includes definite late initialization,
  function-scoped implicit and walrus bindings, context-manager elaboration,
  loop `else`, list `for ref`, declaration destructuring, Writable-backed
  t-strings, integer bitwise/shift operators, and `__matmul__` dispatch.

- Callable and closure semantics now include contextually selected overloaded
  function values with effects, generic callable specialization, explicit
  unified capture conventions, sibling and generic nested calls, reference-backed
  closure environments, and nominal `def(...)` callable structs. Escaping
  closures remain statically rejected.

- A versioned Mojo nightly audit now tracks 1.0.0b3.dev2026071705 and records
  breaking drift affecting immutable conventions, linear deletion, constraints,
  closures, reflection, scalar/SIMD types, origins, imports, and keyword
  variadics.

- Compile-time parameters support typed scalar and aggregate values, type/value
  defaults, named arguments, infer-only parameters, dependent defaults and
  predicates, and heterogeneous type/value packs with per-index types.
- Generic constraints cover parameter and trailing `where` clauses, boolean and
  comparison predicates, `conforms_to`, conditional methods, and conditional
  conformance.
- Specialization uses structural cache keys, a deduplicated shared-fuel worklist,
  and source-located quota diagnostics.
- Current `reflect[T]` handles expose compile-time struct detection, field
  counts, names, types, named field indexes, and chainable `.field[name]` /
  `.field_at[index]` reflected handles whose selected type is `.T`. The removed
  `field_type` spelling is rejected, and reflection can drive
  declaration-producing compile-time branches.
- Generic-target `@implicit` conversions substitute concrete target parameters
  before constructor matching.

- Trait associated-type requirements compose bounds across refinements, and
  conditional conformance predicates are evaluated after type/value specialization.
- Current Indexer normalization, incremental caller-provided hashing, UTF-8 Writer
  buffering, Writable display/repr hooks, reflective formatting defaults, and
  String replacement fields replace the former direct `__str__` formatter path.

- Current Mojo consuming parameters use `var`; the removed `owned` spelling is
  rejected, and the convention is represented as `Var` throughout the compiler.
- Unified `__init__(out self, *, copy: Self)` and current
  `__init__(out self, *, deinit move: Self)` lifecycle declarations drive copy
  and move construction through the existing checked MIR and VM lifecycle
  machinery. A bare `move:` parameter remains a compatibility spelling.
- Calls materialize Copyable `imm` arguments before overlapping `mut`/`ref`
  access, allowing calls such as `f(mut x, x)` while retaining alias errors for
  non-Copyable values and multiple exclusive accesses.
- Current `ImplicitlyDeletable` lifecycle vocabulary replaces the superseded
  `ImplicitlyDestructible` spelling in bundled sources and generic checking.
- Validated, nonraising `@implicit` constructors now provide explicit MIR-lowered
  conversions for typed bindings, arguments, returns, and overload selection.
- `ImplicitlyDeletable where False`, rather than `@explicit_destroy`, now makes
  a type linear. The decorator requires a string and only supplies its
  diagnostic. Field-sensitive obligations preserve partial moves and projected
  destruction while rejecting whole destruction of incomplete aggregates,
  double and conditional destruction; raising destructors preserve the value
  for an `except` fallback, and automatic VM destruction is suppressed.
- Generic constraints now use only trailing `where`, compare types with
  `==`/`!=`, and accept pack-wide `conforms_to(Ts.values, Trait)`. `Int` is the
  canonical VM representation of `Scalar[DType.int]`; `SIMDSize` width values
  and `_` construction-width inference follow the pinned nightly vocabulary.

## [0.1.0] - 2026-07-15

Initial crates.io release.

### Added

- Indentation-sensitive lexer, Pratt parser, semantic checker, HIR and flattened
  MIR pipeline, ownership analysis, drop elaboration, and register VM.
- Functions, methods, structs, traits, generics, overloads, compile-time
  elaboration and VM-backed CTFE for the supported subset.
- Move checking, partial moves, ASAP destruction, stable origins, persistent
  loans, local and cross-call references, reference returns, and frame/slot
  runtime handles.
- Scalar, string, list, tuple, range, exception, iterator, unsafe-pointer, and
  VM-emulated `SIMD[...]` lane-vector semantics needed by the bundled self-hosted
  standard-library proofs. The VM executes lanes serially; hardware SIMD and
  native vector code generation are not included.
- Dotted, relative, qualified, and aliased source-module imports; package
  `__init__.mojo` discovery and re-exports; collision-free linked identities;
  and bundled `std` search roots.
- CLI stages for lexing, parsing, checking, ownership verification, and running
  `.mojo` source files.
- A versioned CPU-parity manifest and Pixi-driven differential harness for
  matching execution output and matching compiler rejection against a pinned
  Mojo reference build.
- A validated Mojo 1.0.0b2 manual inventory that distinguishes parity,
  strict-subset gaps, divergences, representation differences, exclusions, and
  stretch goals; every recorded divergence has an executable differential case.
- An expanded differential corpus covering the implemented first-pass parity
  surface with matching execution, matching rejection, strict-subset,
  acceptance-divergence, and output-divergence modes. The comparison also pins
  lowercase Bool formatting and Mojito's conservative same-place mutable-call
  rejection as known differences from the reference build.
- Mojo-compatible module-scope validation: production compilation rejects
  executable file-scope statements and enters runtime code through `main()`.
- Source package namespace completion includes wildcard privacy for
  underscore-prefixed declarations and isolates same-named declarations and
  overload sets from different modules.
- Module namespaces now preserve lexical shadowing, support imports inside
  functions and nested blocks, resolve unaliased full dotted paths and exported
  types, and implement dots-only relative sibling-module imports.
- User-defined static methods now type-check, participate in overload selection,
  lower without an implicit receiver, and execute with default and keyword arguments.
- `raise` now requires a surrounding handler or a `raises` function/method, and
  direct calls to raising free functions must be handled or propagated.
- Raising instance and static methods now retain their effect through method
  overload selection, so calls must likewise be handled or propagated.
- Non-capturing functions are runtime values with checked function types and can
  be stored, passed as arguments, and invoked through MIR indirect calls.
- Function types retain their `raises` effect; selected free-function overloads
  and indirect callable calls now require effect handling or propagation.
- Typed and parametric errors now survive parsing and checking through direct,
  overloaded, method, and indirect calls. Handlers receive the inferred typed
  error value, and `Never` acts as the bottom and nonraising error type.
- Free functions support a single named `out` result with caller-transparent
  invocation, checked initialization, and direct VM return-slot execution.
- Generic free functions accept heterogeneous `*args: *ArgTypes` packs, check
  every supplied type against the pack bound, and execute type-erased pack
  length queries. Compile-time loops can specialize literal/constructed packs,
  query `args.__len__()`, and index elements through their common bound.
- Expected function types contextually specialize non-overloaded generic function
  values for checked indirect invocation. Hand-written constructors now share
  default and keyword argument binding with free and method calls.
- Overload selection now follows first-pass Mojo precedence across conversion
  counts, fixed versus variadic candidates, signature length, and generic ties;
  defaulted and variadic declarations can participate in overload sets while
  overlapping defaulted calls retain ambiguity.
- Trait refinement now inherits method and associated-member requirements, and
  executable defaults are statically materialized with override/ambiguity rules.
- Lifecycle definite initialization follows normal, returning, raising,
  branching, looping, and protected exceptional paths instead of collecting
  assignments flow-insensitively.
- Opaque trait-bounded indexing dispatches through `__getitem__`; the self-hosted
  library includes an incremental hasher proof; and user-defined printed values
  must opt into Writable/Representable formatting. Bool output is `True`/`False`.

### Scope

- Targets an evolving single-threaded CPU subset of Mojo.
- GPU execution, concurrency/parallelism, distributed execution, Python
  interoperability, MLIR, and optimized native code generation are not included.

[0.2.0]: https://github.com/bpr/mojito/releases/tag/v0.2.0
[0.1.0]: https://github.com/bpr/mojito/releases/tag/v0.1.0
