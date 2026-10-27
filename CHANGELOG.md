# Changelog

All notable changes to Mojito will be documented in this file. The project uses
Semantic Versioning while its public Rust API and supported Mojo subset continue
to evolve under the `0.x` compatibility rules.

## [Unreleased]

### Added

- A struct construction in a type or a parameter argument is now the
  application of the struct's `__init__`, the synthesized fieldwise one
  included, as at the pin, so `Tagged[Extent(n, n + 1)]` in `def gen[n:
  Int]` compiles, a module `comptime E = Extent(4, 4)` names the same type
  as `Tagged[Extent(4, 4)]`, and `Tagged[Extent(n, n + 1)]` against
  `Tagged[Extent(n + 1, n)]` is rejected by structure (R1483). Keywords,
  defaults, written and overloaded constructors, generic (`G[2](n)`),
  one-field register-passable, and library (`List[Int]()`) structs, and a
  struct parameter's default (`[e: Extent = Extent(2, 3)]`) all apply. The
  package format moves to `mojito-package 122`. A closed construction
  whose `__init__` takes a `String` or struct parameter, and one inside a
  tuple or list display, is still frozen (R1528, R1529).
- A call of a function or static method with a `where` clause in a type or
  a parameter argument is now its application, as at the pin, so
  `SIMD[DType.int8, h[1](n)]` over `def h[k: Int](x: Int) -> Int where k >
  0` compiles (R1472, its `where` half). The call binds the clause to its
  arguments as a runtime call does: `h[0](n)` reports `violated
  constraint`, and `h[m](n)` lacks evidence unless the enclosing
  declaration's own `where` proves `m > 0`. The package format moves to
  `mojito-package 121`. Tuple and list displays over a binder in a
  parameter position remain open (R1528, R1529).
- A call of an `@always_inline("builtin")` static method of a generic
  struct in a parameter expression is now its body's expression at the
  instance's parameters, as the pin folds it, so `G[1].plus(n)` over
  `return n + Self.k` is `n + 1` and `Tuple[Int, Bool].__len__()` is `2`
  (R1522). A builtin static method calling a sibling through `Self` folds
  too, the fold drops a body's `comptime assert` as the pin's does, and a
  call of a non-builtin static method through `Self` or an application is
  rejected with the pin's message.
- The VM holds a value of a struct declaring `TrivialRegisterPassable` with
  one field as that field, as upstream lowers a one-element
  register-passable struct to its element, so such a value no longer
  allocates a named field vector (R1396). The library's
  `RawFileDescriptor`, `_ParameterListIter`, and `OptionalReg` are held
  this way. Builtin calls and method dispatch rebox it from its static
  type, and a compile-time result of one still freezes as the nominal
  struct. A struct holding a reference or pointer stays boxed.
- A call of an `@always_inline("builtin")` function or static method in a
  parameter expression is now its body's expression, as the pin folds it,
  so `SIMD[DType.int8, twice(n)]` over `return n + n` and `SIMD[DType.int8,
  n + n]` are one type, in a signature, a field type, a body's `comptime`,
  and a `where` clause (R1395). The body is folded once at its
  declaration, through nested builtin calls, single-assignment `var`s,
  conditional expressions, and `__mlir_op` `pop` and `index` arithmetic
  over `_mlir_value`. A builtin body the pin rejects is rejected at its
  declaration with the pin's message: `raises`, an early `return`, a
  statement `if`, a loop, a reassigned `var`, or a call of a non-builtin
  callable, `Int`'s `//`, `%`, and `**` among them. The package format
  moves to `mojito-package 120`.
- `std.sys.intrinsics.llvm_intrinsic`, re-exported from `std.sys`, and the
  `pop.call_llvm_intrinsic` op it is spelled over now call an LLVM
  intrinsic by name, on the VM and natively and at compile time (R1394).
  The intrinsics computed are `llvm.ctlz`, `cttz`, `ctpop`, `bitreverse`,
  `bswap`, `fshl`, `fshr`, `copysign`, `sqrt`, `donothing`, and the
  `llvm.vector.reduce` `and`/`or`/`smax`/`smin`/`umax`/`umin`/`fmax`/`fmin`
  reductions. A reached call of any other intrinsic, or one of a signature
  LLVM rejects, fails its instance, as LLVM translation fails it at the
  pin. `std.bit` gains upstream's `count_leading_zeros`,
  `count_trailing_zeros`, `bit_reverse`, and `pop_count`. A `StringLiteral`
  binder's `s.value` is now a symbolic `!kgen.string` an instance closes,
  `Bool.__mlir_i1__()` and a collector's `get_loaded_kgen_pack()` are
  served, and a `rebind` in a `comptime if` arm an inlined call substitutes
  is left for the elaborator to judge. MIR schema 1.121 adds
  `llvm.intrinsic`, and the package format moves to `mojito-package 119`.
- `__mlir_op` now admits upstream's `pop.simd` lane ops, `select`,
  `extractelement`, `insertelement`, `reduce_and`, and `reduce_or`, typed as
  the pin's verifier types them, so upstream's `SIMD.__getitem__`,
  `__setitem__`, `select`, `__eq__`, and `__bool__` bodies run on the VM and
  natively (R1393). Each is one MIR instruction (`simd.select`,
  `simd.extract`, `simd.insert`, `simd.reduce`), which the compiler
  `SIMD`'s `select`, `reduce_and`, `reduce_or`, and lane subscripts now
  lower to instead of host methods and lane places. `reduce_and` and
  `reduce_or` also take integer lanes, as upstream's do, and
  `Bool(mlir_value=s)` and `Scalar[dt](mlir_value=s)` read a reduction. MIR
  schema 1.120 adds the four instructions and rejects an `index.get` with
  the `simd` intrinsic, and the package format moves to
  `mojito-package 118`.
- `__mlir_op` now admits upstream's `pop` ops (`add`, `sub`, `mul`, `div`,
  `floordiv`, `rem`, `neg`, `abs`, `shl`, `shr`, `cmp`, `cast`,
  `simd.splat`, `simd.and`/`or`/`xor`, the rounding ops, and `fma`) over
  `!kgen.simd` storage and its `index` ops (`add`, `sub`, `mul`, `divs`,
  `and`, `shrs`, `cmp`) over `__mlir_type.index`, with their `pred`,
  `fastmathFlags`, and `_type` attributes, typed as upstream's verifiers
  type them and run on the VM and natively, so upstream's `SIMD` and
  `SIMDLength` operator bodies can be written (R1392). `Bool(value=b)` reads
  an `index.cmp` result, `SIMD[dt, n]._mlir_type` names the storage type,
  and float `SIMD` lanes take `//`. MIR schema 1.119 adds the `pop`
  instruction, and the package format moves to `mojito-package 117`.
- ``__mlir_type[`!kgen.simd<`, length._mlir_value, `, `, dtype._mlir_value,
  `>`]``, upstream's `SIMD` storage type, now declares a field or parameter
  over a `SIMDLength` and a `DType` parameter, and `v._mlir_value` and
  `SIMD[dt, n](mlir_value=s)` read, store, and rebuild a vector through it,
  on the VM and natively (R1391). An `Int` parameter's `_mlir_value` in the
  length slot is rejected as at the pin. The package format moves to
  `mojito-package 115`.
- `Int`, `UInt`, and `Float64` take every compiler-known `SIMD` method and
  lane subscript, as upstream's width-one `SIMD` does (R1390, R143, R335):
  `x.ne(y)` and the other lane comparisons, `reduce_add` and the other
  reductions (`total(Float64(2.5))` over a `Scalar[dt]` binder),
  `Int(5).cast[DType.int32]()`, `Int(5).join(Int(6))`, and `Int(5)[0]`
  reads and `x[0] = 7` writes run on the VM and natively where they were
  rejected. A float `ne` stays ordered, so a NaN's `n.ne(n)` is `False`,
  as the pin's run-time `ne` is. Width-one comparison methods on any scalar
  (`Float32(1.5).lt(...)`) no longer fail native IR verification, and
  native `Int(-3).__abs__()` and the other math dunders on `Int`, `UInt`,
  and `Float64` reach their scalar lowering again.
- `Int`, `UInt`, and `Float64` are the width-one `SIMD` of `DType.int`,
  `DType.uint`, and `DType.float64`, as upstream's `Scalar` aliases are
  (R1389): the type lattice has no variants of their own, and
  `DType.uint` is a dtype, unsigned and integral, beside `uint64`. So
  `g(UInt(3))` over `def g[dt: DType](a: Scalar[dt])`, `var x:
  Scalar[DType.uint] = UInt(3)`, `UInt.dtype`, and `SIMD[DType.uint, 4]`
  run on the VM and natively where they were rejected (MIR schema 1.117,
  `mojito-package 114`). The three keep their own operator path and no lane
  method or lane subscript until the backends serve them (R1390, R185).
- A parameter binder's annotation resolves like any type expression, as
  upstream's `emitExprType` resolves it (R1372, R482): a trait makes a type
  parameter, and any other type a value parameter of that type, so
  `[v: Int16]`, `struct S[i8: Int8]`, `[n: MyInt]` over `comptime MyInt =
  Int`, and `show[doubled(n)]()` over a `def` returning `Int8` run on the
  VM and natively where they were rejected as unknown traits. A local
  `comptime` annotated with a sized scalar is still no parameter value
  (R602).
- A bracket after a name is classified by what the name denotes, as
  upstream's `SubscriptNode::emitLCVIR` classifies it (R1371, R326): over a
  function its operands bind the function's parameters, positionally or by
  keyword, so `var f = twice[4]`, `pair[b=3, a=4]`, `ident[Point]`, and
  `total[1, 2, 3]` are function values, and `f[]()` binds nothing rather
  than reading a pointer dereference. The parser's `reflect`, `origin_of`,
  and spread cases are gone, and a static call's receiver is one arm over
  the bracket whatever shape it parsed as. A generic function's value now
  names its instance at those bindings (MIR `function_instance`, schema
  1.116, `mojito-package 113`), so a value parameter no runtime argument
  spells reaches the body and the native backend compiles generic function
  values. The capitalized, keyword, and type-operand brackets that still
  parse as `TypeApply` are R1513 and R1514; a bracket fused into a
  following call is R1515.
- A bracket operand is one expression whatever its capitalization, as
  upstream's `parseSubscriptSuffix` parses it, and the check classifies it
  as a type or a value by what it denotes (R1370, R377, R381, R512): a
  capitalized display, reflection, or module-constant element reads as a
  value (`materialize[L[1]]()`, `materialize[NAMES[0]]()`,
  `SIMD[DType.int32, L[0] * 2]`, `show[XS[1]]()`,
  `materialize[Ts[0] == Int]()`), and an applied type (`List[Pair[Int]]`,
  `v[CBox[1]]` on a `Variant`) as a type, keyed for overloads as the type
  it spells. An undeclared applied head is "unknown type". The scalar
  names, `Self`, and `None` still parse as types in a bracket (R1511,
  R1512).
- A type expression resolves by what its names denote (R1369, R134,
  R1203): `type_of(value)` is the value's type, typed without being
  evaluated, in an annotation, a bracket argument, a `comptime` binding,
  and a type comparison (`var y: type_of(x) = 3`, `List[type_of(p)]()`,
  `comptime WriterType = type_of(writer)`), and an annotation whose head
  names a compile-time value is that value's `__getitem_param__` subscript
  (`var x: tl[1]` over a `TypeList` value, local or module-scope). A type
  parameter's value-spelled bracket argument is built by the one
  compile-time builder rather than re-spelled as a type. `type_of` of a
  literal value (R1507), of a type (R1176), and of an earlier parameter in
  a signature (R1508) is rejected explicitly.
- An assignment target is any expression, as upstream parses it (R1375,
  R1233): `=`, augmented assignment, the walrus, and destructuring leaves
  no longer match a list of shapes in the parser, and the checker rejects a
  non-lvalue with "expression must be mutable in assignment", so a call
  with explicit parameters returning a mutable reference (`w.at[0]() = 5`)
  is a target. The walrus stores through any lvalue as `=` does, a nominal
  subscript through its `__setitem__`, and yields the stored value
  (`print(xs[0] := 5)`). Its result is an owned copy where upstream yields
  the destination borrowed, so a walrus over a place of a non-`Copyable`
  type is rejected (R1506).
- A nominal subscript is a destructuring leaf, stored as upstream's
  `TupleDLValue` stores each element through its `SubscriptDLValue`
  (R1374, R1247): `(xs[0], xs[1]) = pair` evaluates each leaf's receiver
  and indices before the value and then passes each element to
  `__setitem__` in order, and a leaf whose struct has only a `ref` getter
  is written through the getter's reference. A tuple display assigned to a
  tuple target is now typed against the leaves' types, so a literal
  element converts where it is built and a `Float64` place takes `(5, 6)`
  natively too. Two getter-only leaves over one root still conflict
  (R1505).
- A subscript store is typed through the getter, as upstream's
  `SubscriptDLValue` is (R1373, R35, R690): a `__setitem__` without a
  `__getitem__` is rejected at the store, the getter is resolved over the
  indices (so a getter of the wrong arity rejects, and a raising one stays
  harmless in a non-raising body), the right-hand side is converted to the
  getter's result type, and the setter is selected over that value and
  passed it by keyword under the first setter's value name, so a
  positional-only value parameter is no longer viable and a read-`self`
  setter is called. The declaration-time rejection of a positional and
  keyword-only setter pair, which the pin does not have, is gone, and
  `index.multi_set` names its value keyword (MIR text 1.115,
  `mojito-package 112`). A value getter's result used as a mutable base,
  which used to update a temporary silently, is now rejected until it is
  written back through the setter (R1503); a slice or multi-index store
  through a getter's reference is filed as R1504.
- Whether a value owes explicit destruction is one verdict on its type, as
  upstream's `getDestructorForType` decides it (R1368, R711, R30): an
  opaque pack element, a reflected field type, and a parameter spelled
  through an alias of a linear struct are now linear when not proven
  `Deinitable`, `_ = e` abandons a linear call result, and an archetype's
  message names the first trait of its bound's composition in symbol-path
  order (`'Comparable'` for `Comparable & Copyable`), refined by its `where`
  facts, rather than `'AnyType'` or the first bound spelled.
- A `where conforms_to(T, Tr)` clause or `comptime if conforms_to(T, Tr)`
  refines the type parameter `T` for every member lookup in its scope, as
  upstream's `maybeRefineParamType` does (R1365): `T.N`, `var e: T.E`,
  `print(s.get())` for a projection the trait bounds `Writable`, and
  `a += b` through `Tr.__iadd__` now check. A requirement call through the
  refinement, or through an associated projection's declaring trait,
  records the bound it was selected by (`refined_bound` on `call.method`,
  `binary`, and `subscript_call`; MIR text 1.114, `mojito-package 110`),
  so drop analysis binds the requirement's conventions and the argument's
  owner ends before the call, as at the pin (R704). The method lookup's
  separate `copy` and `__hash__` cases are gone. Carrying the refinement in
  the receiver's type, as upstream does, is filed as R1495 and R1496.

- A type pack read as a value is the zero-sized `TypeList[Ts.values]()`
  the pin types it as (R1169): `var x = Ts`, `Self.Ts` in a method, and
  `show(Ts)` over `def show[T: Sized]` run, and a run-time `len(Ts)` is
  the library `len` calling `TypeList.__len__` rather than a checker rule.
  A `comptime` binding of a `TypeList` value reads at run time as that
  list and checks its annotation, so `comptime y: Int = Ts` is rejected as
  at the pin. A closed raw list of types is a bracket attribute, so
  `var t = TypeList.of[Trait=AnyType, Int, Bool]()` now constructs. A
  computed list read at run time (R348), a pack named in a parameter's
  type (R1490), and the pin's display of a list type (R1489) remain.

- Upstream's generators (R808): a parameterized `comptime` declaration
  read unapplied (`comptime t = Twice`, `comptime g = T.get`, `A.get`,
  `Self.times`) is a generator value, and `__generator_type[Idx: Int] Int`
  is a keyword type a value parameter may take, applied in its body
  (`G[3]`) and folded per instance on both backends. A generator converts
  to a generator-typed parameter when its parameter list matches position
  by position and its result converts; another signature or a non-generator
  is rejected with the pin's message, and `print(T.get)` now reports the
  pin's `Writable` rejection. New forms: `TyKind::Generator`,
  `CtValue::Generator`, and `ParamKind::BindParams` (MIR text 1.112,
  package 108). An unbound application (R1485) and a type-valued generator
  (R1486) are not supported yet.

- A `TypeList` pack query (`contains[T]()`, `all[P]()`/`any[P]()`,
  `length`) now holds the list it asks of as an operand, as upstream's
  `TypeList` queries take the type list, so a `nodebug` callee testing one
  over its own pack inlines at a call binding that pack to a concrete
  list, its query decided in the caller's instance, where the call used to
  be kept (R1342; MIR text 1.111, package 107). `GenericConstraint`
  carries no binder-keyed proposition any more, so `try_map` takes one
  operand closure.

- The elaborator above the check now evaluates nothing and resolves no
  type, as upstream evaluates parameter expressions only in its
  elaborator, after sema (R1388): its compile-time evaluator, its type
  resolver, its runtime-crossing folds, its compile-time environment, and
  the checker's pre-check `ConformanceOracle` built for it are deleted,
  and every `comptime` declaration is the check's as written. The check
  now types `conforms_to(T, Tr)` as a `Bool` parameter value in any
  position, a run-time `print(conforms_to(Tuple[Int, Int], Comparable))`
  and a binding over a binder included (R1465, R1460: an unknown trait now
  rejects where it answered `False`); an invoked lambda in a `comptime`
  initializer is the lifted function's nested declaration (R1476); a
  construction through a struct's parameterized type member, directly or
  through a local alias, constructs the type the member names; `len` of a
  compile-time `TypeList` value is its length; and a vector-annotated
  scalar binding splats at module scope and in a body. A struct header's
  `where` over its own pack no longer stops at the oracle but at the check
  (R1179 is now R1474's probe), and `TypeList[S[Int, Bool]._types].length`,
  `ParameterList[c].size`, and a user struct's `of` alias now run (R1175).

- A `reflect[T]` query is now answered by the check alone (and, over a
  parameter, by the elaborator below MIR), never above the check, as
  upstream's `Reflected[T]` methods return parameter attributes (R1383).
  A module-scope handle (`comptime R = reflect[P]`) runs, in its module and
  from an importer (R270); a module constant or a struct's own member over
  `reflect[Self]` now answers `field_count()` with the field count where it
  answered `0`; a reflected field type (`reflect[Q].field_at[0].T`) binds an
  `AnyType` type parameter (R79), names a type argument, aliases a local
  type, and is itself a reflection subject, while a bound asking more
  rejects it as the pin does; and an unread binding of a bad selection runs.
  `CtValue::Reflected` is gone, so the MIR text is 1.110 and the package
  format 106.

- A module `comptime` type alias and a generic alias are now held only by
  the check's alias table, as the pin holds an alias only as its
  declaration (R1382), where the elaborator substituted a module type
  alias into every later statement, applied a generic alias itself, and
  exported both in a package build. A static call or member read through a
  module type alias (`A.make()` over `comptime A = Q`, `W3.total()` over
  `comptime W3 = W[3]`, `T.__len__()` over a `Tuple` alias) now runs
  (R603, R338), an
  importer reads a package's aliases from the check's rows, and a
  declaration symbol spells an alias-typed parameter as the type it names,
  so overloads over `U256` or `c_int` resolve. The package format is 105.

- A module `comptime` value constant is now bound by the check, which
  builds every one with the one parameter-expression builder, as the pin
  resolves every module `comptime` (R1381), where the elaborator folded it
  above the check or refused it. A sized-scalar constant (`comptime K =
  Int32(4)`, `UInt8(250)`, `Float32(1.5)`, R135), a conditional (`comptime
  C = 3 + 1 if 3 > 2 else 0`), a builtin call (`comptime M = max(3, 4)`),
  an alias of another constant (`comptime B = A`), and a generic alias
  applying a `def` (`comptime z = Twice[6]`, R518) now run, read in a body,
  a bracket argument, a `comptime if`/`comptime for`, `materialize`,
  `comptime(e)`, and a default, from the program or a package. A local
  `comptime k = A + 1` over a module constant no longer fails MIR
  verification, and an annotation that cannot convert the value
  (`comptime K: Bool = Int32(4)`) is the check's type error.

- A local `comptime` type alias is now bound by the check in every body and
  read by name, as at the pin (R1380), where the elaborator substituted a
  closed one into the statements after it. A call of the alias constructs
  its type (`S(2) * 1.25` over `comptime S = Scalar[DType.float64]`, `V(7)`
  over `comptime V = SIMD[DType.int32, N]`, `Next()` over `comptime Next =
  Sized[n]`, R475), as does one over a struct's type member in a method
  (`F()` over `comptime F = Self.First`, R1170), and a static call or member
  read through it reads the type (`Q.make()`, `Q.k`). An alias of a member
  application, `comptime A = S[Int].Of[Bool]`, now binds in the check too.

- A closed local `comptime` value binding is now bound by the check to its
  parameter expression, as every local `comptime` is at the pin (R1379),
  where the elaborator folded it above the check or refused it. A
  conditional over closed bindings (`comptime x = t[1] if c else t[0]`,
  R1245), a sized-scalar construction (`comptime d = UInt8(250)`, R853), a
  local string constant in a bracket (`bind[local]()`,
  `kbind[local.value]()`, R1191), and a local binding of a generic alias
  that applies a `def` (R518's local form) now run. An operator on a KGEN
  string now reports the pin's "does not implement the '__eq__' method".

- A parameter argument that computes a `String`, a `Float64`, or a `UInt`
  from a parameter (`label[pick(n)]()`, `scale[half(n)]()`,
  `label[String(n)]()`, `label[pick(n) + "!"]()`) is now the application
  the elaborator runs per instance, as at the pin (R386, R461's
  `g[String("a")]()`), where it was rejected with "not an associated
  comptime expression". A call over `UInt` and `Float64` values applies in
  a signature too (`-> Scaled[twice(half(n))]`), and a `String` result binds
  a `String` value parameter as the compile-time string it freezes to.

- A type comparison (`T == Int`, `Self.First == Int`, `F == Int` over a
  local alias of a pack element) is now one compile-time `Bool` wherever it
  is written: a `comptime` binding, a bracket argument, a runtime argument,
  a runtime `if`, or a `comptime if`, as at the pin (R1387, R1180, R476's
  `comptime k = Self.n`). Its operands come from the one compile-time
  builder, so `constraint_operand` and `comptime_type_operand` no longer
  read types per shape. `T == Box[Int]` now holds at `Box[Int]`, and
  `T < Int`, `T == 3`, and `T == List[Foo]` report the pin's errors.

- A module string, float, or `Bool` constant is now its literal's value in
  a bracket argument (`bind[lit]()`, `bind[lit + "me"]()`,
  `kbind[lit.value]()`, `gf[F]()`), and a compile-time member read through
  a construction (`TypeList[Trait=AnyType, Self.Ts.values]().length`) reads
  the member on the constructed type, both as at the pin (R1386, R1194).
  The checker builds every compile-time expression through one
  parameter-expression builder, `compile_dependent_ct_expr`, of which
  `eval_associated_ct` is the folded `CtValue` form, where the two were
  separate per-shape evaluators with different name lookups.

- A struct's `comptime` member that applies a function (`comptime V =
  f(Self.k)`) is now the application over the struct's parameters, which
  each instance evaluates, as at the pin (R1385, R489), where it was
  rejected with "not an associated comptime expression". A compile-time
  binding reading a member whose body reads a sibling through `Self`
  (`comptime m = S[Int].b`, `S.b`, `A.get[2] + 1`) is now elaborated
  (R1206), and through the unbound struct a member depending on a
  parameter through a sibling is rejected with the pin's "cannot access
  comptime member" error.

- A capture-list closure's value, and a capturing lambda's, is now its
  closure's `__storage` struct, as at the pin (R1378): `var g = inner`,
  `Optional(a)`, `[a, a]`, `var h = peek^` and a `F: def(...)` binder all
  run, each closure its own type, and a call through the value borrows the
  storage and devirtualizes to its synthesized `__call__` witness. The
  storage gains upstream's fieldwise copy and move initializers,
  `Copyable`/`ImplicitlyCopyable` exactly when every capture is copyable,
  and trivial lifecycles exactly when it is trivially register-passable,
  so a closure value is no longer copied and destroyed around each call
  (`pliron_closure_drop_order` prints `token drop 7` once on the VM).
  Binding a capture-list closure or a capturing lambda to a compile-time
  callable parameter is now rejected as at the pin (R534 closed), where a
  capturing lambda there was a compiler invariant violation; a thin lambda
  there now runs. A display of one capturing lambda now runs on both
  backends, where it failed to verify after `automatic-inline`.

- A capture-list nested `def` a sibling captures now builds upstream's
  `__storage` struct at its declaration, one field per capture (an owned
  value for a `var` or moved capture, a `ref` for a reference capture),
  through the struct's synthesized fieldwise `__init__`, where Mojito made
  and stored a closure value (R1376). Every call of it, by name or through
  the sibling, which captures the storage, is a direct call passing the
  storage's fields, so the probe `outer(4)` over `inc() {var x}` and
  `twice() {inc}` keeps no `call.indirect` or `closure.make` in concrete
  MIR (R1104). A generic frame's storage takes the binders its fields
  name, and a generic body reached through a sibling
  (`scaled[k]() {var x}` with `sum() {scaled}`) now runs on both
  backends, where elaboration failed. A body with a first-class use, and
  every lambda, keeps its closure value. A read through a reference
  parameter now borrows it immutably, so an `imm` capture's field binds.

- A struct decides its own destructor's triviality through a declared
  `comptime __del__is_trivial: Bool` member (and copy/move through
  `__copy_ctor_is_trivial`/`__move_ctor_is_trivial`), as at the pin:
  `IsTriviallyDeinitable[S[False]]` holds for `__del__is_trivial = not
  Self.flag` even though `S` declares a `__deinit__`, where Mojito answered
  no (R1334). Drop elaboration destroys no value of an instance whose hook
  is `True`, so the `VariadicList` a call builds for a read `*xs`
  collector, which now declares upstream's `not Self.is_owned`, gets no
  destructor call.

- `Slice` and the three `slice(...)` functions are the library code
  upstream's `builtin_slice.mojo` declares, prelude names as at the pin, so
  a slice is an ordinary struct on every backend (R1158). A literal bound to
  a `Slice` parameter takes `None` and `Optional[Int]` operands
  (`c[1:3:None]`, `c[None:3]`, `c[o:]`), `Slice(None, 3)` is rejected as
  at the pin, and `Slice(1, 3).start.value()` runs on the VM. MIR text
  1.105 withdraws `slice.get`, the `slice_arg` subscript argument, and
  `field.get`'s `constructors`; the package format is `mojito-package 96`.

- `ContiguousSlice` and `StridedSlice` are the library structs upstream's
  `builtin_slice.mojo` declares, so they construct explicitly, a `Slice`
  converts to a `StridedSlice` implicitly, and a value of either indexes a
  `List` (R1157). A slice literal is now a call of its parameter type's
  `__slice_literal__` constructor with `None` for an omitted operand: a
  user struct declaring one binds a literal, a `None` bound or stride and an
  `Optional[Int]` bound pass (`xs[1:5:None]`, `xs[None:3]`, `xs[o:]`,
  R1159), an inferred parameter takes the literal's default type, and a
  literal two overloads accept is ambiguous, as at the pin. Natively a
  `Slice` written through `String(...)` or a library struct's `write_to`
  prints `Slice(start, end, step)` instead of its raw layout.

- `enum` declarations, the pinned Mojo's experimental `__enum` spelled as
  upstream's enum proposal spells it (a kept extension, `docs/non-goals.md`):
  `case name` and `case name(T, ...)` declarations become an ordinary struct
  whose members the compiler generates, as upstream's
  `StructEmitter::synthesizeEnumMembers` declares them, over a case-indexed
  storage. Construction by name and by leading dot, `match` through
  `EnumLike`, generic, register-passable, caseless, and open enums, payload
  lifecycles, and the default `Writable` text run on the VM and natively as
  the pin runs the same programs spelled `__enum`, and malformed
  declarations are rejected with the pin's words
  (`assets/extensions/ok/enum_*.mojo`, `docs/notes/enums.md`).

- A contextual list display and a set display are one call of their
  literal constructor (`List(a, b, __list_literal__=None)`), as upstream's
  parser emits them, instead of an empty collection filled by `append` or
  `add`, and a comprehension calls the empty `__init__` its check selects.
  Every construction now names its `__init__`, so the elaborator no longer
  resolves a struct-named call by arity (R1338). A display whose element
  type fails its literal constructor's `where` clause (a generic `Set[T]`
  display without `T: Deinitable`) is rejected as at the pin.

- A case payload matched over a generic enum-like type (`case .Some(x)` over
  `Optional[T]`, `case .success(value)` over `Result[T, E]`) keeps `T`'s
  bounds, where it used to lose them (R1236).

- A list, dict, or set display, a tuple display, and a comprehension are
  fresh temporaries, as a call result is, so one passed to a read
  parameter lives in a hidden slot that dies right after the call instead
  of in an SSA register, as upstream materializes a memory-only temporary
  (R1328). An `in` test's temporary container is read the same way, and a
  scalar conversion reads its argument as a read parameter does: this
  fixes `Bool(x)`, `Int(x)`, and `Float64(x)` over a bounded `x: T`
  copying `x` and converting the copy, where the pin copies nothing
  (`assets/ok/generic_conversion_reads_argument_in_place.mojo`).

- A read use of an `Error` place, and the nominal `String` operand of an
  operator whose other side is a converted string literal, are borrowed
  where they lie, as upstream borrows a `read` argument, instead of being
  copied into an owned register first (R1326). So `print(e)`,
  `print("caught", e)`, `g(e)` at a `read e: Error`, a tuple element
  `g(t[0])`, and `s + "!"` over a `read s: String` run no copy, and
  `g(e)` followed by `print(e)`, which used to fail MIR verification after
  elaboration, runs (`assets/ok/error_read_in_place.mojo`), as does
  `assets/ok/with_exit_error_swallow.mojo`.

- The MIR verifier rejects an operand that reads an owned register some
  path has already ended, as upstream's `CheckLifetimes` rejects a use of
  an owned SSA value after its consumption (R1330). MIR lowering now
  reads a temporary it stores into a hidden slot back from that slot: a
  destructured call result (`var h, t = pair()`, which `os._makedirs`,
  `os.removedirs`, and `path.expandvars` use), an immediately invoked
  closure, and a temporary method receiver. No program's output changes.

- `lower-arg-conventions` and `imm-arguments` store a register-passable
  struct argument read through its fields into a local that `sroa`
  splits, as upstream's `LowerArgConventions` stores it for `SROA` and
  `Mem2Reg` (R1268). So `take_q(q: Q)` reading `q.a` and `q.b` reads two
  `field.get`s of the argument's value instead of loads through its slot.
  A memory-only struct argument stays behind its slot, as upstream's
  `lowerPointerType` leaves it, an initializer's `out self` is never
  stored, and a type whose copy runs a constructor is stored by a load,
  which runs none. No program's output changes.

- `mem-2-reg` reads a whole-root `ref.make` of a slot, whose register
  only `ref.read` and `ref.write` take, as the allocation's own address,
  as upstream's inlined `ref` parameter is (R1105). Each read and write
  through it is a load and a store, so an inlined `nodebug` nested `def`
  over `{imm y}`, `{var x}`, or `{mut c}` leaves no slot in concrete MIR.

- `remove-unused-params` reads an operator on a struct as the call of its
  dunder and a subscript as the call of its accessor, as upstream's
  parser emits them (R1110): an argument a body passes only to such a
  dunder's removed argument is unused in turn, so `fwd[n](g: Grid[n],
  a: Arr[n])` returning `g + a` or `g[a]` loses `a` and `n` and has one
  instance, as on the pin, and a dunder that passes its operand only back
  to itself through `+` or a subscript loses it too. The VM now runs a
  selected method instance spelled off its struct's template, so `a + g`
  calls a reduced `__radd__` rather than the `__add__` its member table
  records.

- `canonicalize` folds a comparison of one int-like register with
  itself, as upstream's `foldSIMDCmp` folds `cmp(x, x)` (R1076): `lt`,
  `gt`, and `ne` to `False`, `le`, `ge`, and `eq` to `True`, splat across
  a mask; a float comparison stays, for NaN. A new first pre-elaboration
  pass, `imm-arguments`, reads each register-passable `read` parameter
  once at the entry, as upstream's parser passes it by its `imm`
  convention, so `var s = x` then `if s < x:` tests one value against
  itself and folds away. A plain `@always_inline` body of sixteen such
  `if`s now inlines, as at the pin.

- `inline-parametric` binds a `read` argument by the address upstream's
  inliner substitutes where it had none to bind (R1099): an owned
  temporary the caller still destroys, such as a homogeneous collector's
  `VariadicList` or a call's result, is lent through a slot the caller
  takes back forwarded after the body; a receiver loaded before a later
  argument's inlined body binds its slot (`String("a b c").split(" ")`);
  and one read through a `ref` register binds that register as its handle
  (`String.__copyinit__` over a tuple element). A reference-returning
  callee inlines when its returns merge through an `if` (`either`) or
  return a pointee or a static-origin reference
  (`ParameterList.__getitem__`, `List.__getitem__` in `_ListIter.__next__`).
  A `read` argument read through a pointer (R1346) and a reference returned
  through a local `ref` slot (R1347) still keep their calls.

- `canonicalize` folds a known callee, as KGEN's `CallParamOp` and
  `CallIndirectOp` canonicalizers do (R1098): a `call.indirect` whose
  callee register is defined once by a function constant or a capture-free
  `closure.make` becomes a direct `call` of that function. An inlined
  `apply[double](3)` now calls `double` directly before elaboration, so a
  `nodebug` `double` inlines in the third round and folds, as at the pin.
  A call through a capturing closure stays indirect (R1345), and so does
  one through a closure held in a slot (R1344).
- `canonicalize` folds a call through a closure of reference captures too
  (R1345), as `CallIndirectOp::canonicalize` prepends a `create_closure`'s
  captures: an inlined `apply[plus](3)` over an `@parameter` `plus` now
  calls `plus` directly, each captured place a leading argument at its
  place, and a closure over a capture-list `def`'s storage becomes the
  method call on that storage a call of the `def` by name is. A call
  through a lambda's moved storage stays indirect (R1434).
- `mem-2-reg` promotes the slot of a closure a call goes through (R1344):
  a slot of a thin function type, or a local holding a closure of
  reference captures, no longer stays behind a `call.indirect`'s
  `callee_place`, which such a value never needs, so `var g = inner;
  g(1)` and a non-inlined `apply[inner](3)` call `inner` directly before
  elaboration, as upstream's `Mem2Reg` and canonicalizer leave them.
- The post-elaboration pipeline runs upstream's `AutomaticInline`
  (`automatic-inline`, R1271): callees first over the call graph's
  strongly connected components, each concrete function takes the bodies of
  its always-inline callees at any size and of its undecorated callees
  below upstream's `-O3` threshold of 50 operations, never a `@no_inline`
  one or one on its own cycle, then runs upstream's nested function
  pipeline (`simplify-cf`, `sroa`, `mem-2-reg`, `canonicalize`, `cse`,
  `canonicalize`, `simplify-cf`, `sroa`, `mem-2-reg`, `cse`,
  `canonicalize`), followed by the guaranteed `sroa`, `mem-2-reg`, and
  `canonicalize` and the `sccp` round. A call from `main` to a small plain
  `def` is now inlined and folded, as at the pin; a recursive callee
  inlines once into a caller outside its cycle. Inlining a
  reference-returning callee binds its mutability parameter to the call's,
  and keeps the call when the body spells the reference `immutable` for a
  `mut` caller (R1339). The VM roots a reference made through a stored
  pointer or `ref` handle at the handle's referent, so it outlives the slot
  holding the handle, and Pliron gives a zero-sized local a placeholder
  slot as it gives a zero-sized parameter.

- `@unavailable` on a module `def` is accepted as at the pin: the body must
  be `...`, the function has no body to lower, overload selection still
  ranks it, and a call selecting it or a reference to it as a value is
  rejected with the decorator's reason, or "'f' is unavailable, use 'g'
  instead" for `use=g` (`assets/ok/decorator_unavailable_function.mojo`).
  Methods stay R1162's. The package format is 79.

- `match` emits its cases as upstream's `emitCases` / `emitCaseCluster`
  do: neighbouring cases whose first command tests a spelled literal or an
  enum case on one path form a cluster that reads the leading value and
  the discriminant once and tests each distinct key once, in key order (a
  literal's spelling, so `match 4` over `case 4` / `case 0x4` runs `0x4`,
  or the case index), each matching case then running its remaining tests
  before its bindings. Each case keeps its commands in pattern order and
  makes its path reads when the emission first needs them, a `|`
  alternative re-reading what the commands before it did not. A match
  whose cases cover the subject keeps every test and seals its
  fall-through, so `case (True, var x) | (False, var x): return x`
  returns on every path, and a nested residual match inside an enclosing
  case completes that case where the pin falls through
  (`assets/ok/match_literal_clusters.mojo`, `match_enum_tag_clusters.mojo`,
  `match_cluster_lifecycle.mojo`). A mapping pattern (`case {"k": v}`) is
  rejected with the pin's text. This closes R1140, R1237, and R1241.
- `comptime match` runs as upstream's compile-time conditional chain: each
  case's tests (`== Int` type equality, literals, enum tags, sequence
  lengths) and guard are one compile-time condition over the subject, its
  bare and `as` bindings are compile-time values, `|` is the disjunction of
  its alternatives with merged bindings, and the elaborator keeps the first
  case whose condition holds. Type and value parameters, tuples, struct,
  enum-case, and fixed-length sequence patterns, CTFE subjects (demanded
  once), and nesting in `comptime for` run in plain and generic functions,
  coverage warns as for `match`, and dynamic subjects, dynamic guards,
  `var`/`ref` bindings, and module scope reject with the pin's texts
  (`assets/ok/comptime_match_*.mojo`, `assets/type_error/comptime_match_*`).
  This closes R1139.

- `match` coverage ports upstream's covering spaces over each case's
  command shapes (`checker/coverage.rs`): a closed `EnumLike` (`Optional`,
  `Bool`) is its constructors, an open one keeps its fall-through and only
  finds repeats, a tuple or struct of closed leaves is its Cartesian cells
  (up to 1024, else "too complex to check for exhaustivity"), and any other
  subject finds repeated root literal spellings (`4` and `0x4` differ). A
  payload test refines without covering, a guard adds nothing, and the
  missing-case warning names eight values with an Oxford comma and counts
  the rest. A `.Some(x)` / `.None` match whose cases return now satisfies
  return analysis, and a case earlier ones cover is checked, sealed, and
  exempt from return and initialization analysis. Warnings keep the pin's
  emission order, `case None` and a non-static `_enum_is_exhaustive` are
  rejected with the pin's texts, and literal tokens keep their source
  spelling (`assets/ok/match_coverage_enums_and_products.mojo`,
  `match_coverage_enum_like_conformers.mojo`). This closes R1134.
- Enum case patterns run as on the pin: over a subject whose type declares
  `EnumLike`, `case .Some(x)`, `case Optional.None`, and a bare tag test
  `case .Some` find the case by name in `_enum_case_names`, test
  `_get_enum_discriminant()` against its index, and match the one payload
  pattern over `_unsafe_get_enum_payload[case]()`, read once per case at the
  first command that needs it. Tuple, nested-enum, literal, and `as`
  payloads run, `ref` writes through a place subject while a temporary's
  payload stays immutable, a qualified owner is checked nominally at the
  root, and a bare qualified head over any other subject is an equality
  test (`case DType.float32`). Parameterized pattern heads now parse
  (`case P[Int](a=z)`, `case Optional[Int].Some(z)`), and an enum case no
  longer counts as irrefutable. Rejections carry the pin's texts
  (`assets/ok/match_enum_cases_*.mojo`,
  `assets/type_error/match_enum_case_*`). This closes R1133.
- `EnumLike` is upstream's library trait, ported whole into the bundled
  `stdlib/std/builtin/enum_like.mojo` and exported by the prelude, and
  `Optional` and `OptionalReg` conform with upstream's members: the case
  names and payload types as raw lists, the discriminant, and a payload
  getter returning the unchecked value through an origin-cast `Pointer`. A
  hand-written conformer and `Optional` run one generic `T: EnumLike` body
  that reads the discriminant, both list lengths, `_enum_is_exhaustive`,
  and a payload by reference, on both backends; as at the pin no case
  metadata is validated at the declaration (`assets/ok/enum_like_protocol.mojo`,
  `assets/type_error/enum_like_*`). Four gaps the port met close with it:
  a trait whose signature reads a same-module struct's member waits for
  that struct (`trait_reads_module_struct_member.mojo`), `Pointer(to=)` a
  reference-returning call points at its place
  (`pointer_to_reference_call.mojo`), a docstring may precede a trait
  requirement's `...` (`trait_requirement_docstring.mojo`), and a generic
  conformer's raw-list member folds when read through a bound
  (`bound_member_list_generic_conformer.mojo`). A one-argument
  `ParameterList.of[x]` and `conforms_to(List[String], T)` also resolve now.
  `Bool` conforms with R1189. This closes R1132.
- `for` and comprehension targets destructure each element, as on the pin:
  `for a, b in pairs`, `for (var a, _) in pairs`, `for ref (a, b) in
  pairs`, `for i, (var x, ref y) in nested`, `for [first, *rest] in rows`,
  `for p.x, p.y in pairs`, and `[a * 2 for a, _ in pairs]` run. A bare name
  declares an immutable binding to its piece (upstream's `Bind` pattern),
  shadowing an outer name; the loop binds each element to a hidden
  immutable binder, never copying a yielded reference, and the checker
  desugars the target into the body's first statements, so `for ref (a, b)`
  leaves `a` read-only, a sequence target needs a raising context, and an
  owned tuple yield is destroyed after its leaves' last use. A comprehension
  clause takes a tuple target, which MIR lowers after the binder; a sequence
  or place target in a clause is rejected (R1227), and `for _ in` no longer
  declares a readable `_`
  (`assets/ok/for_destructuring_targets.mojo`,
  `for_destructuring_lifetimes.mojo`,
  `for_destructuring_closure_capture.mojo`,
  `comprehension_destructuring_targets.mojo`; `for_target_*` in the error
  folders). This closes R1138.
- Starred sequence patterns and targets (`case [first, *rest]`,
  `case [*_, 5]`, `var [a, *middle, b] = values`, `[a, *_, b] = values`)
  now run as on the pin. A pattern tests a minimum length, reads an element
  after the star at `subject[subject.__len__() - k]`, and matches the
  star's closed pattern (`*rest`, `*var rest`, `*_`, which reads nothing)
  against the subject's own slice `subject[i:subject.__len__() - k]`. A
  target checks a minimum length, reads `__len__` once more for the suffix
  start, and stores the slice (`*_` included) into any target, a nested
  `*[g, *h]` among them. Over a `List` the remainder is a `Span` borrowing
  it, and a temporary lives until the remainder's last use.
- Fixed-length sequence destructuring (`var [a, b] = values`,
  `[p, q] = mk`, `[xs[i], xs[j]] = make()`, `[var m, ref r, s] = ys`, and
  nested `var [(c, d), [e, _]] = rows`) now runs as on the pin: the value
  is evaluated once and must declare `Sized`, upstream's raising
  `check_list_length` runs before any store (so the statement needs a
  raising context and a wrong length is caught by `try`), then each element
  is read through `value.__getitem__(i)` and stored in order, a target's own
  subexpressions evaluating after its read. The checker desugars it into
  ordinary statements through the `with` statement's splice, now in
  `checker/desugar.rs`, which also reaches `match` case bodies, so a `with`
  inside a `case` no longer reaches MIR unchecked
  (`assets/ok/with_inside_match_case.mojo`,
  `assets/ok/destructure_sequence_targets.mojo`,
  `assets/ok/destructure_list_wrong_length_raises.mojo`,
  `destructure_sequence_*` in the error folders). This closes R1136.
- Fixed-length sequence patterns (`case []`, `case [x]`, `case [a, 11, c]`,
  and `if case [_, b, _] = xs`) now run over any subject that declares
  `Sized`, as on the pin: one length test through upstream's
  `std.builtin._stubs.list_length_matches` gates the case, each element is
  read through the subject's own `__getitem__` once per case and per `|`
  alternative, in pattern order, and an owned element moves into a lone
  `var` binding. A subject that does not declare `Sized`, has no getter, or
  is matched with a typed pattern inside an element is rejected with the
  pin's verdict (`assets/ok/match_sequence_fixed_length.mojo`,
  `assets/ok/if_case_and_while_case.mojo`,
  `assets/type_error/match_sequence_*`). This closes R1135.
- A trait requirement whose signature reads an element of the method's own
  pack (`def get[*Us: AnyType](self, ref x: Us[1])`, or
  `TypeList[Us.values]()[1]`) now matches a witness reading the same
  element under any pack name, as on the pin, while one reading another
  element is still rejected
  (`assets/ok/trait_requirement_indexes_own_pack.mojo`,
  `assets/type_error/trait_requirement_pack_element_mismatch.mojo`). This
  closes R1198.
- A `TypeList` element read through a bound's raw-list member
  (`TypeList[Trait=AnyType, T._types]()[0]`, or `.__getitem_param__[0]`)
  keeps its type in the body, with the trait the member's list declares as
  its capabilities, and closes to the conformer's element at a call, in a
  function, a method, and a requirement called through the bound
  (`x.get[0]()`), as on the pin. More generally, a callee signature that
  reads a bound's value member (`Box[T.K]`, `SIMD[DType.int32, T.K]`) now
  closes at a call with a concrete conformer
  (`assets/ok/typelist_element_through_bound.mojo`,
  `assets/ok/bound_value_member_in_signature.mojo`,
  `assets/type_error/typelist_element_through_bound_*.mojo`). This closes
  R1201, and R500, whose probe already matched the pin, is closed with it
  (`assets/ok/trait_value_member_comptime_position.mojo`).
- `comptime assert cond[, "message"]` runs, as on the pin: the elaborator
  judges it per reached instance (once per unrolled `comptime for`
  iteration) and fails one where it does not hold with `constraint failed:
  message`, while an untaken arm's or an uninstantiated function's is never
  judged. For the rest of its block the asserted condition is known, so
  `comptime assert conforms_to(T, Writable)` licenses `print(x)` and
  `comptime assert n >= 0` proves a callee's `where n >= 0`; a `comptime if`
  arm now knows its condition's propositions the same way. Outside a
  function the form reports the pin's placement error, and a runtime local
  in the condition (of a `comptime if` too) is the pin's "cannot use a
  dynamic value" rejection (`assets/ok/comptime_assert_*.mojo`,
  `assets/type_error/comptime_assert_*.mojo`,
  `assets/parse_error/comptime_assert_*.mojo`; MIR text 1.81). This closes
  R74.
- `ParameterList.of` and `ParameterList.empty_of` are ported as upstream's
  aliases, as on the pin: `ParameterList.of[4, 5, 6]` infers `Int` from
  its values (or takes `type=` by keyword), a read through it is a type,
  `ParameterList.of[4, 5, 6]()` and `ParameterList.empty_of[Int]()`
  construct, and a closed list's `.values` binds in a module or local
  `comptime` and feeds `ParameterList[raw]`, natively too, where a static
  list of `Int8`, `Float32`, or other one-lane `SIMD` elements now lays out
  (`assets/ok/parameter_list_of_alias.mojo`). A construction through any
  struct's parameterized type member (`S[Int].Of[Bool](3, 4)`) constructs
  the struct it names (`assets/ok/struct_member_alias_construction.mojo`),
  a struct's `comptime` members read one another in any order
  (`assets/ok/struct_comptime_member_read_order.mojo`), and a struct
  parameter reads through a type in a compile-time expression (`comptime c
  = S[7].n`; `assets/ok/struct_parameter_comptime_read.mojo`). This closes
  R809, R1207, and R1209.
- A trait's parameterized `comptime` requirement may declare a default,
  value- or type-valued, as on the pin: a conformer that omits the member
  takes it with the requirement's parameters and `where` clauses, read
  with `Self` as that conformer (`comptime get[idx: Int]: Int = idx +
  Self.K`, upstream's `EnumLike._enum_elt_type_for_case`). A parameterized
  value member's body now reads a sibling value member, and a raw list of
  types over the struct's own parameters witnesses a
  `KGENParamListType[AnyType]` requirement
  (`assets/ok/trait_parameterized_comptime_default.mojo`). This closes
  R1187.
- A struct's `comptime` value member passes as a bracket value argument,
  as on the pin: `Box[Self.n]` reads the member `n` in a later member's
  body, a parameterized member's body, a method, a field's `SIMD` width,
  and a member application (`Self.E[Self.n]`), and a trait reads its own
  requirement the same way, so `def get(self) -> Box[Self.K]` matches its
  witness and reads at `T.K` through a bound. An earlier raw-list member
  feeds a `TypeList` (`TypeList[Self._types].length`), keeping its list
  type when unannotated
  (`assets/ok/struct_member_bracket_value_argument.mojo`,
  `assets/ok/typelist_over_earlier_member.mojo`). This closes R1196.
- A struct's parameterized type member may read the struct's own type
  pack (`comptime E[id: Int]: AnyType = Self.Ts[id]` in `struct W[*Ts:
  AnyType]`, or through `TypeList[Trait=AnyType, Self.Ts.values]()[id]`),
  as on the pin: `W[Bool, Int].E[1]` closes the pack at the instance's
  elements and is `Int`, at a concrete instance and inside a generic
  function (`assets/ok/struct_pack_parameterized_type_member.mojo`). This
  closes R1202.
- A trait may require a parameterized `comptime` value (`comptime
  get[idx: Int]: Int`), as on the pin: a conformer's member witnesses it
  when its parameters and result match the requirement's by position,
  names aside, and a read through a bound (`T.get[0]`, `T.get[idx=5]`,
  `T.get[i]` under `comptime for`) is checked against the requirement and
  answered per instance from the conformer's member, carried across the
  MIR waist in a new struct declaration field (text schema 1.80)
  (`assets/ok/trait_parameterized_value_requirement.mojo`). This closes
  R1146.
- A trait's `comptime` requirement may declare a default (`comptime K: Int
  = 3`, `comptime E: AnyType = Int`), as on the pin: a conformer that
  omits the member takes the default, read with `Self` as that conformer
  (`comptime K2: Int = Self.K * 2`), through a bound (`T.K`), by name
  (`A.K`), in a compile-time expression, and in a trait default method; a
  refining trait's conformer takes its parent's default
  (`assets/ok/trait_comptime_requirement_default.mojo`). A parameterized
  requirement's default is still rejected (R1187). This closes R505.
- `TypeList.of` is the bundled `TypeList`'s upstream alias, `comptime
  of[Trait: type_of(AnyType), //, *Ts: Trait] = TypeList[Trait=Trait,
  Ts.values]`, in place of a compiler rule keyed on the spelling: its
  `.values` reads as a raw list (`TypeList.of[Trait=AnyType, NoneType,
  Int].values` in a struct member or a construction's bracket list), a
  subscript of a construction through it is a type, `comptime L =
  TypeList.of[...]` binds the type, and a template applies it to its own
  parameters (`assets/ok/typelist_of_alias.mojo`). A struct member's
  bracket list binds an infer-only parameter by keyword, and a generic
  alias's pack binds while its elements are still symbolic. `Trait` is no
  longer ignored: left to inference (`TypeList.of[Int, Bool]()`) it is
  rejected until R1176 lands. This closes R1186.
- A generic alias's variadic parameter binds as a pack, as on the pin:
  `comptime T2[*Ts: Copyable] = Tuple[*Ts]` makes `T2[Int, Bool]` the
  tuple `Tuple[Int, Bool]`, `L[Bool, Int]()[1]` reads the `Ts.values` list
  a `TypeList` body binds, a value body such as `Ts.length` or `vs[0]`
  folds, a caller's own pack forwards whole (`Len[*Ts]`), and a struct's
  parameterized member binds its own pack the same way
  (`assets/ok/generic_alias_type_pack.mojo`). This closes R1205;
  construction through such an alias joins R60, and a value body over a
  construction joins R1194.
- A `comptime` member is read through a parameterized struct left unbound,
  as on the pin: `S.n`, `S.plus[4]`, `var d: S.Of[Bool]`, and `comptime B
  = S.Of[Int]` read a member that does not depend on the struct's
  parameters, no default filling one
  (`assets/ok/unbound_struct_member_read.mojo`), and a member that does is
  rejected with upstream's "cannot access comptime member 'Y' with unbound
  parameter 'D.T'" (`assets/type_error/unbound_struct_member_dependent.mojo`).
  This closes R1204; a local binding of a member reading a sibling member
  is filed as R1206.
- A struct's parameterized `comptime` member is read through the struct,
  as on the pin: `S[Int].Of[Bool]`, `P.Pair[A, B]`, and
  `_MLIR.KGENParamListType[Int]` are types whose bracket list binds as an
  alias application's does, `P.Two[1, b=2]` and `P.sz[Int]` are compile-time
  expressions, `M.Of[Int]()[0]` subscripts a construction through the
  member, and a parameterized member's body may apply another type's member
  (`assets/ok/member_alias_application.mojo`). This closes R806;
  construction through the member stays R60, and a read through an unbound
  struct and an alias's variadic parameter are filed as R1204 and R1205.
- A subscript of a `TypeList[...]()` construction is a type, as on the pin,
  which reads it as the construction's `__getitem_param__[i]`: a parameter,
  a `ref` result, a local annotation, and a `rebind` target spell
  `TypeList[Trait=AnyType, Ts.values]()[i]`, over a pack, `Self.Ts.values`,
  or a struct member's raw list, with a symbolic or a constant index
  (`assets/ok/typelist_value_subscript_type.mojo`). The construction parses
  into a new `Type::Value` node and alone is rejected as a value
  (`assets/type_error/type_annotation_is_a_construction.mojo`). This closes
  R1185; the gaps left are R1198–R1203.

- A `TypeList[...]()` construction binds its bracket list as the bundled
  struct's, as on the pin, so `Trait=` and `values=` apply and a raw list a
  struct's `comptime` member holds reads concretely
  (`TypeList[Trait=AnyType, S[Int, Bool]._types]().length`) and through a
  bound (`TypeList[T._types].length` over `T: HasTypes`), its length
  answered per instance, in a `comptime if` too
  (`assets/ok/typelist_over_member_raw_list.mojo`). A member read has its
  annotated type, a raw-list member whose body does not convert to its
  annotation is rejected at the declaration
  (`assets/type_error/raw_list_member_bad_annotation.mojo`), and a member's
  `where` clause over `TypeList[Self.Ts.values]()` is decided per receiver
  (`assets/ok/typelist_self_pack_member_clause.mojo`). The package format
  is 76. This closes R1184 and R130; the gaps left are R1193–R1197.

- A string literal's `.value` is its `!kgen.string` parameter, as on the
  pin: `"None".value`, `("So" + "me").value`, and a module, member, or local
  `comptime` of one fold to a KGEN string, which a parameter typed by
  `` __mlir_type.`!kgen.string` `` binds, spelled directly or through a
  module or member alias (`comptime KGENString = ...`). A
  `*names: KGENString` pack's elements and raw list forward, a default
  `s: KGENString = "x".value` applies, and two strings of different text
  key different instances, on the VM and natively
  (`assets/ok/kgen_string_literal_value.mojo`). A string literal does not
  convert to one, a KGEN string implements no operator, and spelled
  directly the type conforms to no trait, each as at the pin
  (`assets/type_error/kgen_string_*.mojo`). A runtime read, which the pin
  materializes, is rejected (R1190). MIR text schema 1.79 spells the type
  `KgenString` and a value `ct_kgen_string`, and the package format is 75.
  This closes R1183.

- A type pack is upstream's `TypeList`, as on the pin: the prelude exports
  the bundled `TypeList[Trait: type_of(AnyType), //, values:
  _MLIR.KGENParamListType[Trait]]` with `length`, `__getitem_param__`, and
  `__len__`, and a pack's `Ts.length` and `Ts[i]` (`Self.Ts.length`,
  `Self.Ts[i]` in a struct's members) read those members instead of
  checker rules. `TypeList[Ts.values].length`, `len(TypeList[Ts.values]())`,
  and `var x: TypeList[Ts.values].__getitem_param__[0]` run, on the VM and
  natively (`assets/ok/type_pack_reads_through_typelist.mojo`). A
  parameterized type member binds its own value parameters by identity, at
  their declared types, and a parameter typed by a pack element (`x: Ts[0]`)
  now has the pack's bound in the body rather than `Movable` alone
  (`assets/type_error/pack_element_parameter_missing_bound.mojo`). The
  package format is 74. This closes R1168. Spelling the member out in a
  compile-time expression is R1178, and the rest of `TypeList`'s vocabulary
  is R1181 and R1182.

- A trait-valued parameter bounds a type binder, as on the pin: `T: Tr`,
  `*Ts: Tr`, and `T: Tr & Hashable` over `Tr: type_of(Copyable)` see `Tr`'s
  declared bound in the body, and each use checks its arguments against the
  trait `Tr` receives, rejecting one with the pin's "'g' parameter 'T' has
  'Copyable' type, but value has type 'AnyStruct[NC]'". A trait is a type
  where KGEN holds one: a raw list's element may be a trait or `Tr`
  (`_MLIR.KGENParamListType[Tr]`, `` !kgen.param_list<AnyType> ``), a type
  pack's `Ts.values` is such a list, an infer-only `Tr` solves from it, and
  `k[Copyable]()` binds a `!kgen.type` parameter
  (`assets/ok/trait_bounded_type_parameter.mojo`). A list of values, or of
  a trait the bound does not accept, is the pin's "cannot be converted".
  Inferring `Tr` from the types it bounds is R1176. MIR text schema 1.78
  spells `trait_type([...])` and a type parameter's `trait_param`, and the
  package format is 73. This closes R1167.

- A compile-time bracket list binds as on the pin: positional arguments
  fill the parameters that are not infer-only, in order, then each keyword
  binds its parameter by name, an infer-only binder included, wherever it
  is written. `f[T=Int, n=3](1)`, `PL[T=Int]()`, `B[d=DType.int8, 4]()`,
  `Box[Tr=Copyable, 3]()`, a method's `m[T=Int, n=5](1)`, and `C[b=2, 1]`
  now run (`assets/ok/infer_only_parameter_by_keyword.mojo`). A keyword for
  a slot a positional argument filled is "unexpected keyword parameter"
  (including `t[n=1, origin_of(v)]`, which ran before), and a positional
  argument for an infer-only binder is "unexpected parameter". The parser
  now requires `,` or `]` after the `//` marker. This closes R449.

- A parameter typed by upstream's KGEN metatype `` __mlir_type.`!kgen.type` ``
  (`__TypeOfAllTypes`) binds a type, as on the pin: spelled directly,
  through a module alias, or through a struct's member alias (`elt_type:
  Self.KGENTypeType`), it prepares as a type binder that every type
  satisfies and that implies no trait, so `List[U]` over one is rejected
  with the pin's verdict. The bundled `_MLIR` now declares upstream's
  `KGENTypeType` and bounds `KGENParamListType`'s element by it, and a
  struct's type member is an explicit type argument (`g[M.E]()`)
  (`assets/ok/kgen_type_type_parameter.mojo`). MIR text schema 1.77 spells
  the metatype `TypeType`, and the package format is 72. This closes R1150.

- A trait is a compile-time value, as on the pin: `def g[Tr:
  type_of(AnyType)]()` and `struct W[Tr: type_of(Copyable) = Copyable, //,
  n: Int]` declare a value parameter of the trait metatype
  (`AnyTrait[...]`), bound positionally or by keyword to a trait or a `&`
  composition that refines its bound, forwarded (`f[Tr]()`, `W[Self.Tr]`),
  inferred from an argument's type, and part of the instance's identity, on
  the VM and natively. A trait its bound does not accept, a type, or a value
  is rejected with the pin's "parameter 'Tr' has 'AnyTrait[Copyable]' type,
  but value has type ..." text, and `var x = Tr` with its "dynamic type
  values" one (`assets/ok/trait_valued_parameter.mojo`). `type_of(...)`
  parses as a type, though only a trait operand is accepted (R134). MIR text
  schema 1.76 spells `any_trait([...])` and `ct_trait([...])`, and the
  package format is 71. This closes R1166.

- The pin's pattern-matching syntax parses: `match subject:` with `case`
  arms in either layout, every case-pattern form (literals, `== expr`, `_`,
  bare/`var`/`ref` bindings, tuples, sequences with one star, struct and
  contextual `{field=p}` patterns, enum cases `.Some(x)`/`Optional.None`,
  `|`, `as`, guards), `case pattern = value` conditions of `if`/`elif`/
  `while`, `comptime match`, and recursive destructuring targets
  (`(var a, (ref b, _)) = pair`, `var [first, *rest] = values`). The AST
  carries them as `MatchPattern`, `DestructureTarget`, and `Condition`,
  every walker and the checked mirror (`CheckedPattern`, `CheckedTarget`,
  `CheckedCondition`) see them, and the checker rejects each form with a
  contextual "unsupported feature" until its roadmap entry lands
  (R1132–R1140). `docs/notes/pattern-matching-evidence.md` and
  twenty `conformance/probes/match_*`/`destructure_*`/`if_case_*` probes
  record the pin's behavior for the slices that follow.

- Recursive destructuring targets run: `(var a, (ref b, _)) = pair`,
  `var (x, (y, z)) = e`, `var (p, q), r = (7, 8), 9`, nested assignment
  into names, and struct-field places, through one extraction plan the
  flat `a, b = t` forms now share (the separate `Unpack` statement is
  gone). The destinations' subexpressions are evaluated before the
  right-hand side, as the pin does; a `ref` leaf binds the element's
  reference and writes through. A nominal subscript leaf (`xs[i]`) and
  sequence targets are rejected until their slices land.

- `match` statements run with literal, `== expr`, `_`, and bare-binding
  patterns: the subject is evaluated once, cases are tried in order and the
  first match's body runs, a statement no case matches falls through, each
  test is the subject's `__eq__` evaluated when its case is reached, and a
  bare `case name` binds an immutable reference to the subject scoped to
  its case. The checker synthesizes the test and binding statements and
  records a `MatchPlan` the HIR lowers to the `match` region op; the
  remaining pattern forms, `if case`, and `comptime match` stay rejected
  until their slices.

- `case var x`, `case ref x`, `pattern as x`, and `case pattern if guard`
  run: a `var` binding copies the subject by the implicit-copy rule before
  the guard and dies at its last use, `ref` and `as` keep the subject's
  own capability, a `ref` binding of a register-passable temporary is
  rejected as having no memory origin, a false guard advances to the next
  case, and `continue`/`break`/`return` and raised errors leave the match
  through the enclosing loop, function, or `try`.

- Tuple patterns `(1, (2, y))`, struct patterns `Point(x=0, y=yv)`, and
  contextual field patterns `{x=1}` run: each element or field's pattern
  applies to that projection of the subject (`$match[i]`, `$match.field`),
  a temporary subject is materialized once into a hidden owner, and the
  wrong arity, a non-tuple subject, a positional struct argument, a
  repeated or unknown field, and a head naming another type are rejected
  with the pin's texts.

- `|` patterns run: the alternatives are tried in order as an inner match
  region, every test of a pattern runs before any of its bindings
  materializes (a failed alternative copies nothing, as at the pin), the
  alternatives' `var` bindings join into one binding of one type, `as`
  after a `|` binds the whole, and alternatives binding different names or
  types are rejected with the pin's texts. A bare or `ref` binding inside
  an alternative stays rejected until reference joins land.

- The check records nonfatal warnings: `CheckedProgram::warnings()` and
  `CompiledProgram::warnings()` list them in source order, and the CLI
  prints each as `warning: FILE:LINE: message` on stderr after a
  successful compile. `match` coverage produces them with the pin's texts:
  a non-exhaustive `Bool` or `(Bool, …)` match names its missing cells, a
  case after a catch-all or repeating a root literal is unreachable, and
  guards add no coverage; a match whose cases cover the subject concludes
  it, so returns in every case satisfy return analysis. A place subject is
  borrowed to read unless a case binds it by `ref` or `as`.

- `if case pattern = value:`, `elif case …`, and `while case …` run: the
  right-hand side is evaluated only when its branch is reached, the
  pattern's bindings exist in that branch's body alone, a failed match
  takes the next branch or the `else`, and `while case` re-anchors its
  subject at every header visit, a failed match running the loop's `else`.

- MIR gains the `match` region op (`hlcf.match`) and its `match.next`/
  `match.complete` terminators, with HIR's `HirInstr::Match`, textual MIR
  (schema 1.74), the structural verifier, and `lower-control-flow`, which
  lowers a case's two exits to the next case's entry and to the join
  through a frame like a loop's.

- A `comptime` value member and a value-bodied generic alias take
  parameters, annotated or not, as on the pin: `comptime get[idx: Int]: Int
  = Self.values[idx]` reads as `S[1, 2, 3].get[1]` or `Self.get[1]`, and
  `comptime plus_one[n: Int]: Int = n + 1` as `plus_one[k]` over a
  parameter still open, by keyword, inside another alias, or as a struct's
  parameter argument. The check lowers each body once to a template over
  its own parameters and binds every application through the struct
  application contract, so an alias's `where` clause now rejects a
  violating application, and a runtime read no longer folds above the
  check. A struct member over its own pack's element (`comptime first =
  Self.values[0]`) also resolves. The package format is 69.

- `ParameterList` is declared as upstream declares it, over one raw-list
  parameter: `ParameterList[type: AnyType, //, values:
  _MLIR.KGENParamListType[type]]`, with upstream's `_MLIR` alias whose
  body `__mlir_type[`!kgen.param_list<`, T, `>`]` is the compile-time-only
  type `KGENParamList[T]`. A pack's `vs.values` is that list, so
  `ParameterList[vs.values]()`, its `.size`, a `comptime raw = vs.values`
  binding, and a user parameter `raw: _MLIR.KGENParamListType[t]` run as on
  the pin, and a pack read at run time is spelled
  `ParameterList[vs.values]()`. A struct's value parameter reads through
  its type (`S[7].n`), and a parameter may be typed by a struct's member
  (`[x: M.A]`). The bracket gate that admitted only compiler-spelled
  `ParameterList[...]` is gone. MIR text is 1.75 (`param_list(type)`) and
  the package format is 70.

### Fixed

- MIR lowering now applies the check's transfer verdict instead of deciding
  copy or move again by whether the type owns storage (R1367, R121). A
  `^` over a `TrivialRegisterPassable` struct, or a type parameter bounded
  by it, copies it as upstream's `emitTransfer` does, so the source stays
  initialized: `def take(mut v: V) -> V: return v^` now runs and prints
  `3 3` as at the pin, where Mojito rejected it with "'v' is uninitialized
  at return from this function". The checked node's `Move` adjustment is now
  `Transfer(TransferVerdict)` (`mojito-package 111`). `Pointer` and
  `StringLiteral` are not yet modelled as trivial, which is filed as R1502.
- A `^` source is judged by upstream's one ordered rule
  (`UnaryOpNode::emitTransfer`), in the checker's `transfer_verdict`, in
  place of per-shape tests (R1366, R31, R116, R120): an owned rvalue passes
  through and a trivial register value is copied, and any other source must
  reach a tracked root through field projections alone. A subscript
  element, a `ref` binding, and a loop binder not declared `var` are now
  rejected with "expression does not designate a value with an origin",
  where an implicitly copyable element was copied and a `ref` binding
  failed at run time; an `imm` capture is an immutable root, and a `mut`,
  `ref`, or `@__parameter` capture "cannot consume indirect references to
  values". Owned captures (R1500) and a field moved out of a call's
  temporary (R1501) remain.
- A conformance query answers proven, refuted, or unknown, as upstream's
  `doesConformTo` does (R1364). A trait a type parameter's or associated
  projection's bounds do not provide is unknown, not false, so a call
  needing it under symbolic arguments reports "lacking evidence to prove
  correctness" instead of "violated constraint" (R698, the dict display
  `{1: x.copy()}` and its spelled `Dict[Int, T](...)` included), a
  negated conformance is no longer accepted unproven, and a caller's own
  `where conforms_to(T, A) or conforms_to(T, B)` discharges the same
  clause at its callee. A `comptime if conforms_to(Maybe[T], Writable)`
  over a symbolic struct's conditional conformance is decided per
  instance rather than folded false. An associated projection is `Movable`
  and `Deinitable` only where its bounds say so, and the enclosing scopes'
  conformance facts and `where` propositions are one assumption store.
  An `IsTrivially*` over a symbolic struct (R1493) and a conformance
  boundary's explicit premise (R1494) remain.

- One compile-time expression is now one parameter expression, as at the
  pin (R389): the check names every function lifted for a `comptime if`
  leaf, a `comptime for` iterable, or a bracket argument, local `comptime`
  value, or crossing that applies a function, over the binders it reads,
  and MIR mints none of its own. `comptime if abs(n) > 2:` and
  `flag[abs(n) > 2]()` now share one expression, `comptime if b:` over a
  `Bool` binder is the binder itself, `range(3)`'s iterator takes no
  binder, and `flag[h(n) > 4]()`'s register is a parameter constant rather
  than a runtime call of `h`. A condition that is not a `Bool` now
  converts through `Bool(x)` (R1211): `comptime if n:` and `comptime
  assert 3, "int"` run, where MIR verification failed. A layout query
  under a comparison (`flag[size_of[Int]() == 8]()`) now folds under the
  target. A bare name of a non-integer parameter as a condition is now an
  explicit rejection (R1470).

- A `comptime if` or `where` clause comparing a scalar `SIMD` value
  parameter now decides as at the pin (R849): `comptime if x < 10` and
  `comptime if x == 3` over `x: Scalar[DType.uint8]` take the then arm for
  `g[3]()`, `comptime if y >= 1.5` over a `Float32` scalar does too, and
  `where x > 0` admits `h[5]()`. The elaborator's `constraint_holds` and
  the checker's constraint leaves compared only integers; both now compare
  through the one folder, reading a one-lane `bool` mask as `__bool__`
  does.

- `Writable` is now upstream's library trait, declared beside `Writer` in
  the bundled `std/format/__init__.mojo` and exported by the prelude
  (R424), where the compiler knew it by name and each backend formatted a
  struct without a `write_to` field by field. A conformer without its own
  `write_to` or `write_repr_to` now gets upstream's reflective default as
  an ordinary method, so the default names a generic instance by its
  parameters (`G[SIMD[DType.int, 1]](v=5)`, not `G$mono$TInt(v=5)`),
  writes a `String` field's repr as `'s'`, can be called directly
  (`p.write_to(s)`), and runs natively under `repr` (R434). A declared
  trait may refine `Writable`, a field that is not `Writable` is rejected
  only where the default is instantiated, and a `write_to` under a `where`
  clause no longer satisfies an unconditional conformance (R109), all as
  at the pin. `Codepoint` and `TString` gain upstream's `write_repr_to`.
  Natively, every struct a `Writer.write` writes now runs its own
  `write_to`, so a pack element's or a `format` argument's text matches the
  VM (R433), and `Named` loses its by-name formatting arm (R1424).

- `Defaultable`, `RegisterPassable`, and `TrivialRegisterPassable` are now
  upstream's library traits, declared in the bundled
  `std/builtin/value.mojo` and exported from the prelude (R1363), where the
  compiler knew them by name. A declared trait may now refine `AnyType` and
  the lifecycle traits, as upstream's `std.traits` declares them (R1459),
  so `TrivialRegisterPassable` refines `ImplicitlyCopyable`: a struct
  declaring only it has `copy()` and `P(copy=q)` and may be a `Copyable`
  struct's field, directly and through a `Copyable` bound (R861, R863), and
  the bundled `DivModable` gains upstream's `ImplicitlyCopyable` parent. A
  `T: TrivialRegisterPassable` value's copy is the value itself, so
  `mem-2-reg`, `cse`, and `imm-arguments` treat it as they treat `Int`. A
  `Defaultable` conformance records the ordinary witness table of its
  declared `__init__` requirement; upstream's reflective default body is
  still R1119.

- `Hashable` and `Hasher` are now upstream's library traits, declared in
  the bundled `std/hash/hash.mojo` and `hasher.mojo` (R1362), where the
  compiler knew them by name and checked a conformer's shape. Each
  conformance now records the witness table a call through the bound
  reads. The prelude exports `Hashable` but not `Hasher`, so a program
  naming `Hasher` needs `from std.hash import Hasher`, as at the pin. A
  `[H: Hasher]` witness satisfies a `Some[Hasher]` requirement, and a
  malformed `Hasher` conformer reports the member that does not witness
  its requirement. `Hasher` keeps Mojito's requirement set (R25, R1125),
  and the reflective `__hash__` default is still synthesized (R269). A
  `var self` method called through a type parameter's bound no longer
  needs the bound to prove `Movable`, as at the pin.

- The nine `std.math` operation traits (`Absable`, `Powable`, `Roundable`,
  `DivModable`, `Ceilable`, `Floorable`, `Truncable`, `CeilDivable`,
  `CeilDivableRaising`) are now upstream's library traits, declared in the
  bundled `std/math.mojo` (R1361), where the compiler knew them by name.
  A struct can now conform to `Absable`, `Powable`, or `Roundable`, and
  `abs`, `round`, `divmod`, and `**` reach a conforming struct's
  requirement directly and through a bound, where `divmod` over a struct
  failed. Only the first three are prelude names, so `Ceilable` and the
  rest need `from std.math import ...`, as at the pin, and a tuple no longer
  conforms to the rounding traits. `abs`, `round`, and
  `divmod` now reject a struct that does not declare the trait, and a
  `Roundable` struct must declare both `__round__` overloads. `Int`,
  `UInt`, and `Float64` answer each requirement as a method
  (`x.__abs__()`, `x.__pow__(2)`), and `round` over an integer type
  parameter now returns the integer, where it returned a `Float64`. A
  rounding helper over an integer `SIMD` or a vector (`floor(Int32(3))`)
  is now rejected at the check, where the VM failed at run time (R1457).

- `Boolable`, `Intable`, `Floatable`, `Indexer`, and `Identifiable` are now
  upstream's library traits, declared in the bundled
  `std/builtin/bool.mojo`, `int.mojo`, `floatable.mojo`, and the new
  `identifiable.mojo`, and exported by the prelude (R1360), where the
  compiler knew them by name. A call through the bound reaches the
  requirement, so `x.__bool__()` over `T: Boolable` runs where it was
  rejected, and `a is not b` over `T: Identifiable` calls the trait's
  defaulted `__isnot__`. The scalars conform as upstream's `SIMD`, `Bool`,
  and literal structs declare and answer a requirement through their
  builtin conversion, so `Int` and `Float64` are no longer `Floatable`
  (R191) while `Bool` is, and a float literal binds a `Floatable` type
  parameter as `FloatLiteral`, as the pin binds a literal whose
  materialized type fails the bound. A struct with an `__mlir_index__`
  that does not declare `Indexer` no longer indexes a collection, and a
  `T: Indexer` index into a `List` is normalized through `__mlir_index__`,
  where its value was passed through unconverted.

- `Equatable` and `Comparable` are now upstream's library traits, declared
  in the bundled `std/builtin/comparable.mojo` and exported by the prelude
  (R705), where the compiler knew them by name. `Equatable` defaults
  `__ne__` through `__eq__`, and `Comparable` defaults `__gt__`, `__le__`,
  and `__ge__` through `__lt__`, so a struct declaring only `__eq__` and
  `__lt__` has every comparison, directly and through a bound (R706), and
  `a.__lt__(b)` through `T: Comparable` runs (R708). `!=` calls the
  inherited `__ne__` member, and the checker's `NegatedEquality`, the
  elaborator's, and the VM's `!=`-through-`__eq__` stand-ins are gone. A
  struct declaring either trait without an `__eq__` over `Self` gets a
  fieldwise `__eq__`, the stand-in for upstream's reflective default
  until R269, so `Point(1, 2) == Point(1, 2)` runs where it was rejected.
  `Bool` is now `Comparable` (`True > False`), `NoneType` and
  `StringLiteral` no longer conform, as at the pin, and a `SIMD` vector
  still conforms to neither (R1456). A trait method may declare `/` and
  `*` markers (R419); a witness may differ from its requirement in a
  positional-only marker or leave an origin to inference (`StringSpan`'s
  `__eq__(self, rhs: StringSpan)`); and a default inherited under a
  conditional conformance is available, and checked, under its condition.
  The package format is 102.

- The bundled library types declare the conformances upstream's do (R578):
  `List[T]` is `Defaultable` and `Equatable where T: Equatable`, `Dict` is
  `Defaultable`, `Set` is `Comparable`, `String` and `StringSpan` are no
  longer `Iterable` (a `for` over one still walks its grapheme clusters)
  but are `IntableRaising` and `FloatableRaising`, upstream's traits, now
  bundled and exported by the prelude, and `StringSpan` is `Defaultable`,
  so `Int(StringSpan("12"))` parses (R430). A `Comparable` struct now
  conforms to `Equatable` (R707), and a `conforms_to` clause may name a
  trait composition (`conforms_to(T, Deinitable & Movable)`). The
  conformances that still differ are filed as R1453, R1454, and R1455.

- A struct no longer conforms to a compiler-known trait it never declares
  (R577): `conforms_to(T, Boolable)` and a `T: Boolable` bound are false for
  a struct with a `__bool__` and no `Boolable` in its list, as at the pin,
  and likewise `Identifiable`, the `std.math` traits, and
  `TrivialRegisterPassable` as a bound (R1363's undeclared-bound half).
  Only `AnyType` still holds for every struct. `Bool(x)`, `Int(x)`, and
  `Float64(x)` require the declared trait, the bundled `List`, `Dict`, and
  `Set` declare `Boolable` as upstream's do, and a scalar is no longer
  `Identifiable`.

- A local `comptime` struct binding in a plain `def` is a parameter with no
  storage, as at the pin (R471 closed): `comptime c = N(8)` read as `c.a`
  is a compile-time constant and never destroys an `N`, and each run-time
  use of the whole value materializes its own temporary
  (`assets/ok/comptime_local_struct_binding.mojo`).

- A triage of nine `stdlib` entries (R1405) re-ran each probe against the
  pin. R213 (`"abc".byte_length()` prints 3), R216 (`isdir("/tmp")`), and
  R220 (`Array[Int8, 4]()`) already matched it and are closed, as are R206
  (a type-pack call inside a nested `def`, and a whole-pack forward
  `take(*args)`, now `assets/ok/pack_call_nested_def_and_forward.mojo`)
  and R199, whose compile-time tuple program the pin rejects with Mojito's
  message while its pack twin, a `var` per unrolled iteration filling a
  list display, runs as at the pin
  (`assets/ok/pack_element_list_display.mojo`). `Variant` now opens each
  public operation with upstream's `Self._check[T]()` over the ported
  `_get_type_index` and `_InvalidTypeIndex`, where a no-op `comptime if`
  stood (R411). `String`'s literal constructor is upstream's `@inline(.nodebug)
  __init__(out self, data: StringLiteral, /)`, so `String("hi")` inlines
  (R1101). R204 is closed: no abstract `__trait_dispatch` or
  `__iterator_dispatch` target survives into concrete MIR across the
  corpus, so the VM's unreached sibling-iterator retargeting and the unused
  `TRAIT_DISPATCH_PREFIX` are gone. R1223 merged into R1397, the library
  `SIMD` port that adds `__getitem__`.

- A homogeneous collector over a `Writable` element is now `Writable`, as
  at the pin (R1072): the bundled `VariadicList` declares upstream's
  `write_to` and `write_repr_to`, so `print(xs)` over `*xs: Int` writes
  `(1, 2, 3)` and `repr(xs)` the pin's `VariadicList[...]` text on both
  backends. A method's `comptime if` over its own parameter, called from a
  sibling method that reads none of its struct's parameters, now runs where
  elaboration stopped on an unresolved struct parameter: a compile-time
  application binds a parameter it removed only when its argument
  resolves. A raising generic function whose body inlines a call that
  drops an operand, such as a value pack's `values[i]`, no longer loses
  the store of its raised error, which natively handed the caller an
  uninitialized error and trapped as a double free; `value_pack_runtime_read`
  and three `parameter_list_*` fixtures run natively again.

- A triage of seventeen `mojo-shape` entries (R1402) re-ran each probe
  against the pin. R752 (closed by R874's `try` regions), R346 (`Tuple`'s
  members come only from `tuple.mojo`), and R638 and R639 (MIR matches no
  `ExprKind::TypeApply` or `ExprKind::Call`) are closed, and R1072 is
  fixed. R950 merged into R662, and R903 into R223, whose missing
  `downcast` is its cause. R306, R800, R1169, R445, R912, R1287, and R1335
  are rewritten against today's code, none waiting on a landed entry; R336
  now waits on R1392 and R1395, and R276 on R1397 instead of R160.

- A triage of eighteen `divergences`, `pmir`, `native`, `code-org`, and
  `tooling` entries (R1403) re-ran each probe against the pin. Eight
  already match it and are closed: R245 (a compile-time `break` or
  `continue` in a `comptime for` outside a generic `def`, now
  `assets/ok/comptime_for_break_outside_generic_def.mojo`), R163 (`len`
  over a runtime pack in a `comptime for` header is rejected, as
  `assets/type_error/comptime_for_runtime_pack_length.mojo` pins), R1018
  (a callable struct is no `def(...)` value, closed with R1009), R162 (a
  keyed recursion under a run-time `if` in a compile-time evaluation stops
  at the instance budget, now a `tests/compiler_test.rs` case), R173
  (`try_region_drop_timing` now matches; `owned_pointer_api`'s early
  destroy, an owned-interior `Pointer` that does not keep its owner alive,
  joins R1155), R633 and R299 (a variadic subscript and a pack-element
  struct now run natively), and R457 (closed by R634). R823 is fixed: the
  MIR disassembler snapshots read 1.108 and are pinned through
  `expect_file!`, so `UPDATE_EXPECT=1` blesses them. R862 merged into
  R1363 (`conforms_to` now matches, and its two probes are `assets/ok`
  fixtures with R1144's two match probes), R1145 into R169, and R521 into
  R767. R423, R182 (a struct's parametric member over a bare struct, not
  the `ref`-field constructor), R1144, R1279, and R172 are rewritten
  against today's code, and R172 and R983 now wait on R533.

- A triage of eighteen `comptime` entries (R1400) re-ran each probe against
  the pin. Sixteen already match it on both backends and are closed: R279
  (`Self.N` over an associated value), R278 (`reflect[Self]` in a generic
  struct's method), R337 (`g[fib(6)]()` and `W[fib(6)]()`), R340
  (`range(Self.__len__())` in a `comptime for` header), R543 (an unread
  local `comptime ks = d.keys()`), R98 (`comptime U: UInt = 3` reads as a
  `UInt`), R319 (a local type alias constructing in an unrolled body),
  R368 (a module list declared after the `def` that iterates it), R131,
  R132, and R486 (compile-time calls of per-instance code: a `comptime if`
  method, `rep[3]()`, and the t-string and `Tuple` writers), R82 (a
  `DType`-keyed struct named `T`, `K`, `V`, or `H`), R464 and R465
  (compile-time values of a non-fieldwise and of a generic struct), R450
  (a `UInt` value parameter inferred from an argument type), and R498 (a
  nested `def` keyed on a type pack). Their probes are now six `assets/ok`
  fixtures. R72's remaining alias-body shape merged into R1371, and the
  local and member alias calls it exposed into R475 and R60. R509 is
  rewritten: a constrained module constant is a plain value where the pin
  makes it a generator type, so Mojito also accepts `print(C)`, which the
  pin rejects.

- A struct's value parameter now reads as its declared type wherever
  `Self.x` names it, as upstream's parameter constant does: `Self.b` over
  `Flag[b: Bool]`, `Self.f * 2` over `Scale[f: Float64]` (also in a static
  method), and `Self.s` over `Box[s: String]` type-check and run on both
  backends, where each read as `Int` (closes R848, R979, R790, and R395).
  The projection of a static method's interned `self` slot that the
  mistyped reads lowered to is gone
  (`assets/ok/struct_value_parameter_declared_type_reads.mojo`). A triage
  of fourteen `comptime` entries (R1401) re-ran each probe against the pin.
  R78 (a reflected field type in a local annotation), R70 (a value-keyed
  `def` forwarding its value to a keyed `def`), R68 (an imported alias in
  a generic method's signature), and R90 (a keyed `def` applied to a
  loan-carrying type) already matched it and are closed; the R70 and R68
  probes are now `assets/ok` fixtures. R1015 is deleted: the pin evaluates
  `side()` once, before `main`, as R528 states. R1303, R77, R1071, and
  R71 are rewritten against today's code, the last narrowed to a bracket
  application bound as a value, since the pin rejects a bare generic
  `def` reference too.

- A triage of twelve `calls` entries (R1404) re-ran each probe against the
  pin. Five already matched it on both backends and are closed: R719 (a
  raising `__neg__` in a non-raising `main` is rejected), R112 (an
  `@implicit` conversion in a generic method is selected once), R119
  (`P[Int, 3](7)` runs a generic constructor beside a declared one, now
  `assets/ok/value_struct_generic_constructor_overload.mojo`), R91
  (same-arity generic method overloads on a generic struct), and R128 (the
  pin now accepts `Tuple[Int, T]` over `T: Writable`, as Mojito does).
  R81's remaining shape, a tuple of a user `Comparable` struct, merged into
  R1359, which now also waits on R707.

- An owner whose last use is the copy of its field into an argument is now
  destroyed once the copy is made, before the call, as at the pin:
  `obj.own2(y=h1.c, x=h2.c)` prints `copy 5, del 5, copy 4, del 4` before
  `own2` (R688). A copy initializer's result no longer inherits the alias
  its source's shallow place read holds, while it keeps the loans its
  value carries. A triage of eight `ownership` entries (R1406) also found
  R111 (a borrowed evaluated default is destroyed) and R115 (a second
  `Dict` lookup no longer stales a view of the first value) already
  matching the pin on both backends, and folded R716 into R1130.

- An augmented element store (`l[0] += x`) through a mutable-reference
  getter calls the element's in-place dunder where the element lies and
  reads `x` by the dunder's convention, as at the pin, instead of copying
  `x` and moving the element out and back (R1426). This covers `List`,
  generic `List[T]`, and multi-index and slice `ref` getters, and a raising
  dunder no longer destroys the element twice. Through a value getter the
  getter's result is updated in place and the setter takes it (a `var`
  value), reads it (a `read` value), or converts it, with no move
  initializer.

- A generic body that only reads a pack element `Ts[i]` now reads it where
  it lies, as upstream passes a memory-only value by address, instead of
  copying it or running its move initializer: `print`, the `Bool`/`String`/
  `repr`/`Int` conversions, `==`, and an ordinary `read` parameter (R1354).
  `if value:` over a `Boolable` pack element is accepted, as at the pin.
  `Byte(x)` over an `Intable` parameter, `write_repr_to` on a generic
  element, and `hash` of a `List` or `Array` copy no element, and `in` over
  a `Tuple` no longer destroys an extra copy of the matched element.

- `List.resize(n, fill)`, `List(length=n, fill=x)`, and `List.copy()` copy
  each element once, as at the pin, instead of copying it twice and
  destroying the first copy: `unsafe_write(copy=v)` reads `v` where it lies
  (R1353).

- Converting a `List` to a `Span` (`var s: Span[T, origin_of(xs)] = xs`,
  a list passed to a `Span` parameter, a field source) builds the view over
  the list where it lies, as at the pin: no element's copy constructor
  runs and no temporary list is destroyed (R117).

- An element read through a view (`String(span[0])`, `span[0].copy()`,
  `rest[0].v` over `var rest = xs[1:]` or `var [*rest] = xs`) no longer
  conflicts with the view's own loan when the view is used again, and an
  element of a `Span` passed to a read parameter or printed is lent rather
  than copied, as at the pin (R48).

- A condition over a `Boolable` value (`if o:`, `while c:`, a conditional
  expression, a comprehension filter) reads a named value, a field, or a
  list or tuple element where it lies, as upstream's `Bool(x)` does,
  instead of copying it, so `if o:` over an `Optional[T]` with a printing
  `T` and every enum-case pattern over it (`case .Some(u)`, through
  `_get_enum_discriminant`) copy nothing, as at the pin (R1249). A `T: Boolable` parameter (`if b:`) and an element of
  a non-`ImplicitlyCopyable` type (`if os[i]:` over `List[Optional[T]]`)
  are now accepted as conditions; both were rejected.
- An implicit copy at its source's last use no longer runs an
  initializer the pin does not (R136): an owned parameter (`take(c)`,
  `take_kw(c=c)`, past a raising later argument too), an owned receiver
  (`c.consume()`, which printed `copy`), and an owned operator operand
  (`f + g`) take the dying source's storage, and a chained comparison's
  first middle (`b == c == d`) moves into its temporary instead of copying.
  A consuming receiver over a borrowed loop or `ref` binding now copies
  its element instead of destroying it twice. Copy elision's "cannot
  raise" now reads `MirInstr::raise_edge` (R827).
- A loop-free `@always_inline("nodebug")` callee of a homogeneous `read`
  collector (`def cnt(*xs: Int) -> Int: return len(xs)` called as
  `cnt(1, 2, 3)`) no longer fails verification after `inline-parametric`
  with "call.method ends owned register …": the collector is lent to the
  body instead of consumed by its slot, so the caller's
  `VariadicList.__deinit__` ends it once (R1099).
- A `ref` binding names an origin, as upstream's does, instead of
  borrowing its owner exclusively (R1082): a read, write, mutating call,
  capture, or second reference of the owner beside a live `ref` binding
  runs, as do a `match` over a temporary tuple reading an enum payload, a
  guard reading an `as` binding before a later read of the subject, the
  aliasing swap `[xs[1], xs[0]] = xs`, and a view built from a `ref`
  binding read beside the binding. Each binding's slot now holds setup loans:
  they keep the owner live and carry its interior generation, so an element
  reference still stales under a structural mutation. `Variant`'s
  `__getitem_param__` returns `ref[origin_of(self)._get_owned_interior[
  "value"]]` as upstream's does, so a payload reference used after `set`
  is rejected as an invalidated interior reference, the pin's diagnostic.
  Four `subset` error fixtures the pin runs move to the `ok` folders.

- A `ref` field passed to a `mut` or `ref` argument, or as a method's
  receiver, binds the handle the field stores on the VM, as Pliron loads
  through it, where it bound the field's slot:
  `b.pane().shifted().first()` over `shifted` returning
  `Pane(self.items, …)` through a written `ref` constructor faulted on the
  dead temporary (`invalid reference projection Field("items") on None`),
  and a `mut` callee assigning its whole parameter overwrote the field's
  handle instead of the referent
  (`assets/extensions/ok/ref_field_written_init_chained_view.mojo`).
- A destructured tuple place is no longer copied before its elements are
  read: a `for a, b in it` target over a tuple of copyable structs ran the
  tuple's copy constructor (printing `copy` lines the pin does not), where
  upstream projects each element from the tuple's address
  (`assets/ok/for_destructuring_tuple_not_copied.mojo`).
- A loan-carrying temporary argument anchored in its `$arg_loan_r` slot is
  passed as a read of that slot, as upstream materializes a temporary and
  the call borrows it, where the register the slot had taken was passed as
  well, giving the value two owners (an origin-bearing `Named(...)` in a
  `*args` pack).

- The 2026-10-24 overnight gate's failures. A raised error of a struct with
  no fields prints again instead of `<uninit>`: the VM reads a `uninitmem`
  of such a struct as the struct. A read of a direct `ref` field where a
  value is wanted, a copy or a read argument, reads its referent on both
  backends, so the bundled `_ListIter` passes `len(self.src)` with the
  pipeline off. A `Tuple`'s element origins erase in MIR type agreement as
  a struct's arguments do. A generic lambda's erased storage drops nothing.
  A call whose result holds no reference keeps none of its arguments'
  loans (`len(xs[1:3])`), and a read parameter borrows an accessor's
  reference result over a temporary key (`len(d[String("k")])`). `cse`
  keys a parameter constant by its identity, so nested `comptime for`
  binders spelled alike stay apart. A conditional expression's temporary
  arm is moved into its result, as on the pin (R1107). A loan generation's
  owner that already died is not destroyed again where the generation
  retires. `raise "message"` raises the `Error` it converts to. A nested
  `def` capturing a loop's reference target passes the referent. A `ref`
  destructuring leaf is an immutable reference, as on the pin. Copy
  elision takes a `value.copy` of a dying source as it takes a copy
  constructor. `remove-unused-params` erases a slot only stored to, so an
  unused argument's construction no longer keys the caller's instances.
  The bundled `TypeList` and `ParameterList` member aliases elaborate and
  check with the library linked from source.

- The VM's `consume.var` destroys the elements of tuple storage, as the
  Pliron backend's already did. Since R931 erased `loans.establish`, `sroa`
  splits a consumed `Tuple` into its storage slot, and the VM leaked its
  elements: `var t = (pc[0], 1)` in
  `assets/ok/pointer_subscript_reads_in_place.mojo` never printed `del C 15`.

- A `deinit` parameter's fields now die in drop elaboration's one
  field-sensitive walk, as upstream's `DestructorInsertion` destroys them,
  replacing the second walk after it: every exit marks the parameter
  destroyed, so a method that transfers `self^` on one path no longer fails
  at run time after destroying the fields it handed on, a second `deinit`
  parameter's `Tuple` field read no longer traps (R354), and a field a
  pointer borrows dies by its own destructor after the pointer's last use
  (`assets/ok/deinit_receiver_fields_die_in_drop_walk.mojo`, R1252). A
  register's or a loan's owner is now the field its place reaches, so a
  replaced field read past a sibling's read through a register or a
  reference dies as soon as nothing reads it
  (`assets/ok/replaced_field_read_through_register.mojo`, R1307).

- A field's replaced value now dies at its own last use, as the pin's
  field-sensitive destructor insertion places it, rather than at the store
  that replaces it: after the read that last uses it, right after its
  holder is built when nothing reads it, at the entry of an `if` arm that
  does not move it out, and, for a `mut` parameter the body overwrites
  before reading, at the function's entry
  (`assets/ok/field_overwrite_dies_at_last_use.mojo`). Drop liveness lays
  out each root whose fields the body stores into, per field with a
  whole-object bit, as upstream's `ValueSet` does (R1251).

- A forwarded `**kwargs^` now runs natively: Pliron bound the forwarded
  dictionary as one keyword of a fresh collector and failed IR
  verification, so `assets/ok/trait_default_wrapper_conventions.mojo`, a
  relay through `inner(**kwargs^)`, and a bounded `target.count(**options^)`
  only ran on the VM (R591). A keyword no named parameter claims beside a
  forward (`inner(c=3, **kwargs^)`) is rejected, as at the pin ("combining
  a '**' unpack with other keyword arguments for '**kwargs' is not
  supported", `assets/type_error/keyword_forward_beside_keywords.mojo`),
  where the VM merged the two.

- A raising initializer destroys the `self` it has partly built when a
  raise leaves it, as upstream's `DestructorInsertion` destroys a partially
  initialized `self`: `self.a = Tracer("a")` then `raise` now prints
  `del a` before the handler runs, on the VM and natively, kept as a call
  or inlined. Drop elaboration makes an initializer's `out self` a drop
  root that every `return` reads, so it dies where a raise path leaves it,
  first among the values dying there, and `refine_receiver_drops` destroys
  it whole once every field is set, through its `__deinit__`, or each field
  set on every path, in declaration order
  (`assets/ok/init_raise_destroys_initialized_fields.mojo`). This closes
  R956 and files R1295 and R1296.

- A `for` loop or comprehension over an iterable whose `__iter__` raises
  calls that `__iter__` as an ordinary raising method call with its error
  region, as upstream's `emitForStmt` does, instead of inside `iter.init`,
  which had no raise edge. `try: var k = S(1); for x in Bad(): ...` now
  destroys `k` and reaches its `except` on the VM, where `simplify-cf` had
  erased the `try` and the error went unhandled, and runs natively, where it
  was unsupported (`assets/ok/raising_iter_call_edge.mojo`). A raising
  `__iter__` in a normalization chain is rejected. Natively, a raising call
  with no raise edge of its own is now unsupported rather than jumping to
  the `try` or the propagate block without releasing temporaries.

- Natively, a `raise` releases the owned register temporaries of the
  blocks it leaves, up to the `try` it names or out of the function, as
  every other exit does: `(s + "!", may(n))` no longer leaks the string
  when `may` raises. The raising call's own results, which its error path
  never defined, are kept, and its error path releases the temporaries its
  arguments end, as its normal path does
  (`tests/pliron_backend_test.rs`
  `raise_edge_releases_the_temporaries_it_leaves`, which balances the
  runtime's `MOJITO_RT_DBG_POINTERS` allocation log). This closes R1113.
- A `try` on the VM is a level of its frame's region cursor, as upstream's
  `TryOp::interpret` transfers into the try region (R1272): `lit.try.yield`
  moves to the `else` region or past the op, and a raise naming a `try`
  unwinds the cursor to that `try`'s body and enters its `except`, binding
  the error. A call inside a `try` part is a pushed frame like any other,
  so recursion through one runs at any depth (100 000 deep in a debug
  build) where it overflowed the host stack at about 40; `exec_try`,
  `run_region`, and the VM's `Flow` outcome are gone. Every corpus
  fixture's output is unchanged.
- `T()` and `T(copy=x)` at a struct name the struct's member, as
  upstream's substitution folds the `Defaultable` and `Copyable`
  witnesses, rather than a call naming the struct (R1283): `T()` calls
  the `__init__` the checker now records in the struct's `Defaultable`
  witness table (`compiler_trait_declaration` declares `Defaultable`),
  and `T(copy=x)` calls the struct's recorded `__copyinit__`, each
  passing the struct's arguments as a direct call does. An explicit
  `S(copy=s)` over a synthesized copy takes the same path in
  `name_constructors`, and a struct whose copy is trivial copies by
  `value.copy`, so `var d = S(copy=s)` over a `@fieldwise_init` struct
  of `Int` fields runs on the VM instead of failing with "struct 'S' has
  no copy constructor" (R724,
  `assets/ok/copy_construction_synthesized.mojo`). The VM's
  `construct_via_copy`, the copy form of Pliron's `lower_constructor`,
  and the elaborator's struct-named copy respell are gone.

- A `finally` is copied before every exit of its `try` in MIR lowering, as
  upstream's `lowerTryFinally` pastes it, instead of running once in the VM
  on each exit: before each `return`, `break`, `continue`, and raise that
  leaves the `try`, and at the end of its handler and `else`. Each `try`
  carries a label and each raise names the `try` it reaches, as
  `lit.try.raise` does, so a raise in a `finally` copied into a protected
  body skips that body's handler; a raising body without an `except` gets
  the parser's re-raising handler; a `finally` that returns replaces the
  exit it is copied before, destroying the replaced return's value. A value
  an exit holds now dies after the copy reads it, so a `break`'s value read
  only on the fall-through path (VM: never destroyed; native: at the
  function's exit), a raise out of the function past a `finally`, and a
  return overridden by its `finally` destroy what they hold as the pin does
  (`assets/ok/try_finally_break_value_read_after.mojo`,
  `try_nested_finally_break_destroys.mojo`,
  `try_finally_raise_out_of_function_destroys.mojo`,
  `try_handler_raise_crosses_finally.mojo`,
  `try_else_raise_crosses_finally.mojo`,
  `try_finally_return_overrides_owned_value.mojo`). `simplify-cf` now
  elides a `try` whose `finally` was copied out when no raise names it. The
  `try` cleanup lists, `return.cleanup`, the VM's `finally` run, and
  Pliron's pending-outcome dispatch are gone. The MIR text format is 1.92
  and the package format 82. This closes R884 and R829.

- Each raising call is its own raise edge, as upstream's `addErrorRegions`
  makes every throwing call: a raising call, method or indirect call,
  operator, or subscript defines an error flag and an error
  (`Raises { ty, flag, error }`), and MIR lowering follows it with its
  error region, an `if` on the flag whose `then` region raises the error,
  which the verifier requires. Drop elaboration destroys at that arm's
  entry what the edge holds, so a value ended before the call is no longer
  listed on a `try` body's one shared cleanup list, a value whose last use
  is the raising call dies before the values still live past it, and a
  raising call that leaves the function destroys what its frame holds,
  through every frame between, where it was never destroyed
  (`assets/ok/raising_call_edge_destroys_held_values.mojo`,
  `assets/ok/try_raise_edge_drop_order.mojo`, which now matches the pin).
  The VM sets the flag where the call raises, a child frame's `raise`
  popping into its caller; Pliron joins the callee's outcome branch after
  the instruction; `inline-parametric` routes an inlined body's raises into
  the call's flag through its wrapper loop, and binds a raising `deinit`
  callee's field in place. A `try`'s cleanup list keeps only what a raise
  past a nested `try`'s `finally` holds (R884). The MIR text format is 1.91
  and the package format 81. This closes R831, R20, R833, R1129, and R1106.

- A `raise` statement is now a MIR terminator (`MirTerm::Raise`, HIR
  `Terminator::Raise`), as upstream's `raise` ends its block, where it was
  an instruction followed by its region's normal terminator. Drop
  elaboration reads the raise channel after it, so a value live after the
  region a `raise` leaves, and read by no handler or `finally`, is
  destroyed at the raising arm's entry, as at the pin, whether the raise
  leaves the function or a `try` body, from an `if`, `else`, loop, or
  `comptime if` arm; before, it was never destroyed when the raise left
  the function (`assets/ok/raise_exit_destroys_live_values.mojo`). The MIR
  text format is 1.90 (`raise { src }` is a terminator) and the package
  format 80. This closes R906; R20 keeps the raising-call case.

- An unrolled `comptime for` copy whose reduced call drops an operand built
  by an inlined constructor sheds the dropped registers' types, so
  `looped[n]` over a written `@always_inline` `Arr[n]` no longer fails
  elaboration ("`Arr` lays out parameter `n`, which `looped` removed";
  `assets/ok/comptime_for_nested_inlined_constructor_instances.mojo`).
- `inline-parametric` erases the read of a compile-time argument that an
  inlined call passed and its body does not take, so `G[n](5).v` over a
  constructor that never reads `n` no longer fails verification
  (`assets/ok/inlined_constructor_unread_struct_parameter.mojo`).
- A `ref` field's read passed to a `mut` or `ref` parameter binds the
  field's referent, on the VM and natively, where it failed verification
  ("argument 0 … has type ref List[Int], declared List[Int]"), and a loan
  through a `ref` binding of a `ref` field verifies
  (`assets/extensions/ok/ref_field_argument_at_ref_parameter.mojo`).
- `remove-unused-params` records a self-recursive callee's removed
  parameter once in `callee_removed_params` when the self-call both spells
  the parameter and infers it (`rec[n](k - 1)`), where it listed it twice
  (`tests/mir_passes/remove-unused-params/forwarded.mir`). This closes
  R1257.
- Every pass between drop elaboration and the pre-elaboration pipeline
  now reads the structured body, and both seams lower control flow only
  after `lower-lit`. A `deinit self` field that only a `try` handler reads
  now dies right after the body's last raising call, before the
  non-raising calls after it, as on the pin, where Mojito kept it alive
  to the end of the body
  (`assets/ok/deinit_field_dies_before_non_raising_call_in_try.mojo`).
  Field drops no longer split edges. Destructor-call lowering follows
  every region op, and copy elision stops its scans at a region op. This
  closes R878 and the field half of R827.
- Drop elaboration places its drops in the structured body, as upstream's
  destructor insertion does, and `lower-control-flow` runs right after it:
  a drop goes after a value's last use or at the entry of an `if` arm that
  leaves out a value a sibling arm keeps, and no edge is split. A `try` is
  walked as upstream walks it, so a value a `break` or `continue` out of a
  `try` leaves behind dies before the exit, where the pin destroys it,
  when the `finally` does not read it; a `break` whose `finally` reads the
  loop's reference into a named `List` now runs, where the VM read freed
  storage (`assets/ok/try_finally_break_reads_loop_reference.mojo`). A
  `try`'s raise edge destroys what its handler does not read, so a value
  the handler abandons with a `break` or `return` dies before the handler
  runs, as on the pin, where Mojito leaked it or destroyed it late
  (`assets/ok/try_handler_exit_raise_edge_drop.mojo`).
  Escape cleanup lists are gone. This closes R877 and R891, and the first
  half of R830 (its second half is R884's probe).
- The interior-origin check is now part of the move and initialization
  scan, as upstream's `TrackedAndInteriorLiveness`, and the loan check
  reads the same structured regions; `lower-control-flow` runs after both.
  A container mutation before a `break`/`continue` out of a `try` now
  invalidates a view used after the loop or in the next iteration, as on
  the pin, where Mojito ran it
  (`assets/ownership_error/list_element_view_stale_after_break_from_try.mojo`,
  `list_element_view_stale_after_continue_from_try.mojo`), and a view used
  after its owner moves reports `use of uninitialized value` for the owner,
  as the pin does (`list_element_view_used_after_list_moved.mojo`). The
  first ownership complaint in walk order is reported. This closes R876.
- The move and initialization check walks each body's structured regions
  as upstream's `CheckLifetimes` does, before `lower-control-flow` lowers
  the body for the borrow checks: an `if` joins its arms, a `match` case
  starts from the states its predecessors' `match.next` left, a loop
  re-walks its body until its `continue` states settle, and a `break` or
  `continue` carries its state to the loop it names, through any `finally`
  it crosses. A value moved before a `break`/`continue` out of a `try` is
  now rejected when it is read after the loop or in the next iteration,
  where Mojito accepted it and trapped at run time
  (`assets/ownership_error/try_break_move_then_use_after_loop.mojo`,
  `try_continue_move_reentered.mojo`), and a field or `mut` parameter
  refilled after such a loop runs on both backends, where Mojito rejected
  it (`assets/ownership_ok/try_break_partial_move_refilled_after_loop.mojo`,
  `try_break_mut_parameter_refilled_after_loop.mojo`). This closes R875.
- Compile-time conditions follow the pin: the elaborator decides `and` and
  `or` left to right and an `elif` only when no earlier arm was taken, so a
  payload read guarded by its tag never runs over the other variant; a
  struct value compared with `==` runs its own `__eq__` instead of a
  structural comparison; and a raising call in a condition, or a type
  compared with a value (`T == 1`), is rejected with the pin's text
  (`assets/ok/comptime_conditions_short_circuit.mojo`,
  `comptime_if_struct_equality.mojo`, `assets/type_error/comptime_if_*`).
  The textual MIR reader accepts the trait-valued element type of
  `TypeList`'s raw list, so an emitted artifact executes again.

- A destructured leaf or comprehension binder that shadows an outer name
  (`[x * 2 for x in xs]` beside a local `x`, `(ref x, ref y) = t` in a loop
  body) no longer stops MIR lowering on "HIR's slot for a name is its
  binding's": HIR records no slot for a read of a binding it did not
  declare whose spelling names another binding's slot.

- A pointer dereferences its address, never its origin's place, as on the
  pin: after `var p = Pointer(to=b).unsafe_origin_cast[origin_of(a)]()`,
  `p[]` reads and writes `b`, and a method's pointer under a wider origin
  than the place it addresses (`def ptr(ref self) -> Pointer[Int,
  origin_of(self)]` over `self.y`) reads `self.y` rather than `self`, where
  a store through it failed MIR verification. A field read, `+=`, and a
  method call through such a pointer reach the pointee too
  (`assets/ok/pointer_origin_cast_reads_pointee.mojo`). This closes R352.

- A field of a reference-returning call's result is a place within the
  call's result origin, as on the pin: `return inner(w).t` over `inner(ref
  w: W) -> ref[w] W` returns under `ref[w.t]` (or `origin_of(w.t)` over read
  parameters) and writes through, and `ref[origin_of(w)]` rejects it with
  "cannot return reference with incompatible origin"
  (`assets/ok/ref_return_call_result_field.mojo`,
  `assets/type_error/return_origin_widening_call_result_field.mojo`). A
  returned pointer dereference names the pointer's own origin, as
  upstream's `Pointer.__getitem__` returns `ref[Self.origin]`, so `return
  Pointer(to=self.x).unsafe_origin_cast[origin_of(self)]()[]` (the body of
  upstream's `Optional._unsafe_get_enum_payload`) returns under `ref[self]`
  (`assets/ok/ref_return_pointer_dereference.mojo`). The dereference of a
  pointer formed in place (`Pointer(to=s.x)[]`, an origin cast, an offset)
  is now a place on both backends: it assigns, passes as a `mut` argument,
  and binds a `ref` that aliases the pointee, which before silently bound a
  copy (`assets/ok/pointer_temporary_dereference_place.mojo`). This closes
  R679 and files R1212–R1215.
- A struct's `comptime` member reads its own type pack as on the pin:
  `comptime width = Self.Ts.length`, `len(Self.Ts)`, and `comptime First =
  Self.Ts[0]` (an index may read an earlier member, `Self.Ts[Self.width -
  1]`) are fixed by each application, so `Row[Int, Bool].width` prints `2`
  and `var x: Row[Int, Bool].First = 7` types as `Int`
  (`assets/ok/struct_member_reads_type_pack.mojo`). These reads are still
  checker rules, not members of a bundled `TypeList`, which waits for
  trait-valued parameters (roadmap R1166 to R1169).

- A trait body may hold `pass`, as on the pin (`trait Measured(Sized):
  pass`), where it was a parse error.

- A temporary view operand of an operator (`s.rstrip() == "  ab"`,
  `s[byte=0:2] == s[byte=0:2]`) keeps its source alive through the
  comparison, as a call argument does, instead of stopping the VM with
  "use after Pointer deallocation" when the read was the source's last
  use. On the VM, a reference reached through a heap pointer designates
  the heap element rather than the slot holding the pointer, so an element
  read through a span (`w[1]`, `xs[1:3][1]`) outlives the span, a
  reference returned through a temporary span (`return Span(l)[0]`) is no
  longer stale, and an accessor inlined at a pointee place (`p[1].get_b()`)
  returns a reference to the element's field.

- Linking renames the base of a dependent type projection (`M.Wrap[t]`),
  so a linked module, the bundled library among them, may spell one over
  its own struct.

- `ref y = x` over a loop's element reference (`for x in xs`) forwards the
  element handle instead of failing MIR verification, `ref r = Box(2)`
  over a constructor call types the temporary's hidden slot, and a value
  copied through a reference binding (`var w = r`) no longer inherits the
  binding's loans unless its type can hold a reference, so a later use of
  the referent is not a conflict.

### Changed

- `Int(x)`, `UInt(x)`, `Float64(x)`, and `Bool(x)` of a scalar operand are
  now a `scalar.cast` MIR instruction carrying its target, upstream's
  inlined `pop.cast` or compare, where they were a `call` the VM, Pliron,
  and `sccp` each recognized by its bare name (R857). The check records the
  cast, and the elaborator writes it for a conversion a template spells
  over a type parameter at a scalar instance, so neither backend resolves a
  conversion by name and a program's own `def UInt` is never taken for
  one. MIR text 1.118, package 116.
- A variadic value parameter (`*vs: T`) is now declared with the raw list
  it binds, `KGENParamList[T]`, as upstream's implicit `vs.values` binder
  is typed, where it was declared with its element (R1151). Each bracket
  argument is still checked against the element, and the checker's
  separate value-pack scope is folded into its one pack scope. A
  variadic `value_param`'s `type` is now `param_list(T)` (MIR text 1.113,
  package 109), and a 1.112 artifact, which the reader had rejected
  since R808 bumped the schema, reads again. Declaring upstream's
  visible `ParameterList` / `TypeList` binder beside the list is R1491
  and R1492.

- The elaborator above the check no longer carries a struct-value freeze
  (`Elab::freeze_struct_value_arguments`), which could no longer evaluate
  any call: a struct-valued bracket argument that calls a function
  (`Tagged[Extent.square(4)]`) is the application the check binds and
  `native::mono` evaluates, in a signature, a field type, a body, over a
  binder, and through a `comptime` alias, so it is not `Tagged[Extent(4,
  4)]`, as at the pin (R1384).

- An `@parameter` closure that captures, bound to a compile-time callable
  parameter, is now folded into the instance whether it is generic or not,
  as upstream keys such a closure on its body (R533): the instance takes
  only the captured places, as trailing reference parameters, and calls the
  body directly, where a non-generic closure was passed whole as a runtime
  parameter and called indirectly. So a method's callable parameter binds
  one (`runner.apply[scale](5)` prints `115`, R172), a forwarded binding
  calls it (`outer[f]` calling `inner[f](x)`, R980), and an unread one
  elaborates with the pre-elaboration pipeline off (R983). A method call
  now records the capture accesses of a closure its bracket arguments
  bind, as a direct call does, so a captured local stays live across it.

- A call, operator, or subscript through a bound now resolves at a struct
  instance to the member its witness table records, as upstream's
  specialization folds `#kgen.get_witness` (R1281), where the elaborator
  respelled the dispatch under the struct's name. `Copyable`'s table now
  records its `copy` default, and a generic `for` calls the `__iter__` and
  `__next__` requirements its bounds declare, where it named a
  Mojito-only `__iterator_dispatch` protocol.

- A protocol member and a subscript accessor or setter that never use an
  argument now lose it, as upstream's `RemoveUnusedParams` rewrites each
  call in library code (R1109): `E[n].__eq__` returning `True`,
  `L[n].__len__`, `C[n].__iter__`, and `C[n].__contains__` each have one
  instance for every `n`, a subscript whose accessor never reads `self`
  is the call of its generator without it, and a store whose setter
  ignores the value is the `__setitem__` call passing none. Only display,
  the hash leaf, a scalar construction's `__int__`, and a `Variant`
  handler's `__call__`, which a backend still reaches from a value's
  type, keep every argument (`mir::TYPE_REACHED_MEMBERS`). A subscript
  whose accessor never uses its index collector (`__getitem__(self, *idx:
  Int)` returning a field) now runs, where its contract kept the
  collector's argument and failed to verify, and a store on a setter
  whose `value` follows its index collector no longer misbinds its
  checked parameters when the setter drops one.

- The VM no longer dispatches a `call.indirect` over a struct value to its
  `__call__` by name (R1031): the elaborator devirtualizes every call on a
  callable struct into the `__call__` instance's method call, so a struct
  callee reaching the VM fails as any other non-callable value. A call on
  a struct value is still checked through its `def(...)` conformance
  rather than as its `__call__` method call (R1466).

- A tuple comparison is now the call of the bundled `Tuple`'s dunder under
  its `where conforms_to` clause, as at the pin (R1359), where the checker
  decided it structurally by element type and the VM ordered tuples
  itself. A compile-time comparison of tuple displays (`comptime c = (1,
  2) < (1, 3)`) is now lifted as that call and runs, where it was
  rejected (`assets/ok/tuple_comparison_dunder_call.mojo`). Over a name
  bound to a tuple display it is still folded and rejected (R1464).

- An operator over a type parameter or an associated projection is now the
  call of the requirement its bounds declare, as any method call through a
  bound is (R1358): `a + b` over `T: Addy`, a user trait declaring
  `__add__`, runs as `a.__add__(b)` where it was rejected (R1282), as do a
  reflected `2 + a` through `__radd__`, a raising requirement, `-a` and
  `~a`, and `x.first() > y.first()` over a `Comparable` projection. The
  comparison, identity, and `Powable` arms that typed such an operator by
  its trait's name are gone, so a comparison or `**` through a bound is
  that requirement's call too, also as a chained comparison's link. A
  `where conforms_to(T, Trait)` clause now declares `Trait`'s requirements
  on `T` for every method call in its body, not only `copy` and
  `__hash__`. A comparison requirement called on a `SIMD` scalar instance
  is the scalar's own comparison, so `not a.__eq__(b)` over `UInt8` runs
  where the VM rejected the lane mask, and the VM reads a subscript whose
  base lies in a pointer's element (`self.data[i].storage[1]`) through
  the pointer.

- A struct operator is now checked as its dunder's method call, as
  upstream's `emitBinOpCall` resolves it (R1357): `a op b` selects
  `a.__op__(b)` as the spelled call would, else the reflected
  `b.__rop__(a)`, and `-x`/`~x` select `x.__neg__()`/`x.__invert__()`,
  recording the whole call contract, and MIR lowers it through the spelled
  call's lowering. So a `var self` or `deinit self` dunder runs as a call
  and a `nodebug` one inlines (`N(3) - 1`, and `-(n^)`, which failed to
  verify: R1332), a display right operand is built as the parameter's type
  (`l == [1]` and `l + [2]` over `List[Int]`, which were rejected: R697), a
  leading-dot right operand resolves against it (`c == .mk()`), and a
  generic dunder solves its parameters (`h < 2` over `__lt__[T:
  Intable]`), also as a chained comparison's link. Method and function
  overload selection now ranks a parameter that leaves an origin to
  inference (`suffix: StringSpan`) as generic, as upstream's
  auto-parameterization does, so `p.__truediv__("sub")` over `Path` selects
  `__truediv__(self, suffix: Self)` where it was ambiguous. The package
  format is now `mojito-package 103`.

- A struct operator is now its dunder's method call in MIR, as upstream's
  parser spells `a + b` as `a.__add__(b)` (R1348), and an operator through
  a bound becomes its member's call once `inline-parametric` substitutes a
  struct receiver. `inline-parametric` also inlines the accessor of a
  multi-subscript (`g[i, j]`, `g[i:j, k]`), so a `nodebug` dunder or
  accessor reached through either inlines like a direct call. A dunder
  owning its receiver still lowers to a `binary` (R1357).

- `inline-parametric` and `automatic-inline` now inline a callee holding a
  `try`, splicing it into the caller's block as an inlined `if` is, as the
  pin does (R1288). So an `@always_inline` function holding a `for` loop
  over a raising iterator inlines below the size bound, and a `nodebug`
  body iterating its parameter or collector (`total(xs)`, `total(1, 2,
  3)`, `count[T: Iterable]` in `fc`) inlines too.

- A `for` loop or comprehension now calls a nominal iterable's `__iter__`
  as an ordinary method call and stores the iterator into its own slot,
  as upstream's `emitForStmt` does (R1286), so the inliner splices it like
  any call and `inline_cost` no longer prices it specially
  (`protocol_call_cost` is gone). `iter.init` starts only compiler-private
  storage and loses its `mode` and `prepare` (MIR text 1.109), and the
  elaborator's iterator-slot retyping is deleted (R664). The pin still
  keeps some loops that now inline here, `for i in range(x): break` among
  them, because it measures a callee right after splicing its own callees,
  before any cleanup (R1452). Output is unchanged on the VM and natively
  across the corpus.

- Copy lowering, drop elaboration, and the elaborator no longer look for a
  frame slot named after a non-callable value binder, which MIR stopped
  creating with R662 (R663): `materialized_parameter_slots`, the
  elaborator's seeding of a captured binder slot, and its rejection of a
  place rooted at one are deleted, and `mir::parameter_slots` keeps only
  the callable binders whose read still takes a slot
  (`types::binder_read_takes_slot`, shared with the check). Output is
  unchanged on the VM across the corpus.

- A read of a value binder (`n` in `def plain[n: Int]`) is now the
  parameter constant of the binder's declaration reference, upstream's
  `kgen.param.constant`, which the check records on the read and the
  elaborator answers per instance, where MIR read a frame slot that held
  nothing until the elaborator rewrote each read by name (R662). A lane
  slot or bracket argument over a binder (`v.length`, `f[n + 1]()`) and
  a nested `def`'s read of its enclosing function's binder lower the
  same way, so no generic body, lifted `comptime for` thunk included,
  keeps a slot for a non-callable binder. The elaborator's and
  `inline-parametric`'s by-name rewrites now serve only callable binders,
  which still read a slot (R1449). Output is unchanged on both backends.

- Register passability now judges a conditional `RegisterPassable`
  conformance at the instance's arguments, as upstream's
  `getRegisterPassability` does, a pack's condition element by element:
  `Optional[Int]` and `Tuple[Int, Bool]` are register-passable and
  `Optional[String]` is not, where every conditional row answered
  memory-only. A function value other than a closure's storage is
  register-passable too, as the checker already judged it (R1356, in
  part). A slice literal whose `__slice_literal__` constructor returns
  through memory, every library slice type's, is now written to a
  call-result slot its subscript reads in place, positional or keyword,
  as upstream gives it a `__call_result_tmp__`; the package format moves
  to `mojito-package 101`. A subscript's keyword argument place is now
  among the places every slot rewrite and scan sees (`sroa`, the move
  scan, interior uses), which the slot of `e[byte=1:3]` needs. An
  explicit destroy with compile-time
  arguments (`self^.consume_elements[h]()`) now consumes its receiver's
  place as `x^.finish()` does, where it forwarded a copy of the storage
  in a register. A memory-only value still reaches a register through
  copies and operands R1440–R1444 name, so the verifier rule rejecting
  its destruction there is R1445. No program's output changes.

- A raise out of a function now stores its error into the function's
  error slot and leaves through an `error_return` carrying no error, as
  upstream's raise outside every `try` writes the raising function's
  error argument, the slot `findNearestErrorSlot` returns there (R1437).
  Each raising function names the slot (`$error`), which holds nothing on
  entry and is demanded at each `error_return` alone, so an error a
  `finally`'s `return` overrides dies as a variable: a raising function's
  `try: raise Error("first")`, `except: raise Error("second")`, `finally:
  return 7` now runs natively (`pliron_finally_overrides`'s `k`), where
  `--backend pliron` failed on the handler's `drop.reg` ("read of
  undefined register"). A raise converts a
  `String` it stores into an `Error`-typed slot to `Error`, as it converts
  a literal; the re-raising handler a `try`/`finally` without `except`
  gets moves the caught error into the enclosing slot. MIR text 1.108
  adds a raising function's `error_slot` and drops `error_return`'s
  operand; the reader upgrades older artifacts, and the package format
  moves to `mojito-package 100`.

- A raise into a `try` now stores its error into the `try`'s error slot
  and enters the handler with no error operand, as upstream's
  `lit.try.raise` does after the raise writes `__try_error__`, the slot
  `parseTryStmt` declares and `lit.try` takes as its `$err` (R1293). The
  slot is the `except` binding's own when the clause binds one, else a
  `$try_error<N>` declared before the `try`; the handler reads it, and it
  dies as a variable, so drop elaboration destroys an unread error right
  after its raise, and a pending error a returning `finally` overrides
  (`pliron_finally_overrides`'s `g`) with its slot, where it died by a
  `drop.reg`. MIR text 1.107 gives `try` an `error` slot and splits
  `raise` into `try.raise` and `error_return`; the reader upgrades older
  artifacts, and the package format moves to `mojito-package 99`.
  Natively, six fixtures that catch an error under an `except` binding
  (`optional_raising_subscript`, `template_raise_forms`, …) no longer leak
  it, and a caught typed error is no longer copied as a 24-byte `MjError`
  into its zero-sized binding.

- A capturing closure's lifted body is now its storage struct's method,
  `<lifted>$__storage.<name>`, as upstream's `liftClosureIntoMethod` nests
  it, where it was a generator nested in its enclosing declaration taking
  the storage as a leading argument (R1428). The storage's parameters are
  the enclosing binders the closure names in its body and signature as in
  its fields (a frame's `n` read by `body() {var x}`, a type binder, a
  `comptime for` index), its own stay the method's, and a call by name, an
  immediately invoked lambda, and a closure value all call the method on
  the storage. A body naming an enclosing callable binder stays a nested
  generator until a struct's callable parameter elaborates (R92). No
  program's output changes; the inliner now also inlines a storage
  method's call that carries capture accesses, and a slot typed over a
  `comptime for` index that an inlined call leaves unread is retired with
  the loop.

- Every capturing lambda, with a capture list or without one, now builds
  upstream's `__storage` struct where it stands, as upstream's
  `resolveAnonymousClosure` does, and its lifted body takes the struct as
  `self`, where the body took each capture as a leading reference argument
  (R1427). An immediately invoked lambda is a direct call passing the
  storage, so a capturing generic lambda
  (`(lambda [N: Int](x: Int) {var k} -> Int: x + N + k)[5](3)`), which
  failed to elaborate, runs; a lambda used as a value moves the storage
  into its closure, so a `{var c^}` register-passable capture the body
  writes prints the pin's `55` through `twice`, where Mojito printed `56`.
  A `{var r}` lambda over `ref r = xs[0]` copies the referent, so it runs
  natively, where it crashed. A moved capture of a `def` or lambda now
  moves once, into the storage, where its `__moveinit__` ran twice. MIR
  text 1.106 withdraws the `copy` closure-capture mode.

- Every capture-list nested `def` now builds upstream's `__storage` struct
  at its declaration, and its lifted body takes that struct as its one
  leading `read` parameter, `self`, as upstream's lifted method does,
  reading each capture through a reference to its field, where the body
  took each capture as a leading reference argument (R1377). Every call,
  by name or through a sibling, passes the one struct: by value when every
  capture is register-passable (a reference capture by its referent), as
  at the pin, so a moved `Int` capture the body writes is written in that
  call's copy (`505`, where Mojito printed `506`), and by address
  otherwise. A first-class use makes a closure capturing the storage by
  reference. A `{var a}` or `{imm a}` capture of a reference binding now
  copies or borrows the referent, as at the pin (R1230's `def` half), and
  the copy elision's environment-slot name test is gone (R1333). A
  `@parameter` closure and a lambda keep their leading captures (R1427).
  The checked `def` records whether it declares a capture list, and the
  package format moves to `mojito-package 98`. The VM's `MakeRef` of a
  place ending at a stored reference now forwards that reference when the
  destination is typed as it, as Pliron's does, and a reference field
  drops trivially.

- A memory-only value passed to a `read` parameter reaches the callee as
  the caller's storage at every call, as upstream passes it by address,
  where the VM used to bind a copy unless the call's result borrowed the
  argument (R677). A write through an untracked `UnsafePointer` to the
  caller's value is now seen through the parameter, as at the pin under
  `-O0`. A register-passable argument is still passed by value: the
  checker records it (`RegisterPassableRead`), and the package format
  moves to `mojito-package 97`.

- A result its callee returns through memory gets one hidden slot at its
  call, as upstream's call emission passes the callee's `ByRefResult`
  argument a synthesized `__call_result_tmp__`, and each consumer without
  a destination reads, takes, or projects that slot, replacing the
  per-consumer read, forwarding, receiver, field, and discard slots
  (R1355). The checker records the convention from the callee's declared
  result type (`IndirectResult`), so a generic `-> T` returns through
  memory at every instance, a register-passable struct result stays a
  register, and a `Boolable` condition, a by-value subscript's base, and
  a stringified argument now destroy their temporary from a slot. The
  package format moves to `mojito-package 94`.

- A `for` loop calls its iterable's `__iter__` once, as upstream's
  `emitForStmt` does, so an `__iter__` whose result is not itself an
  iterator is rejected, as the pin rejects it, where Mojito used to call
  that result's own `__iter__` in turn (R1337,
  `assets/type_error/iter_result_not_iterator.mojo`). `iter.init` names at
  most one `__iter__`, and with no chain intermediate left, Pliron frees
  no iterator storage itself; nor does it free a callee's owned arguments
  after a call to an empty body, which drop elaboration already destroys.

- A `conforms_to` test holds its subject as an operand, as upstream's
  `type_conforms_to_trait` does, so a `nodebug` callee testing
  `conforms_to(T, Trait)` or `conforms_to(Ts.values, Trait)` now inlines
  at a call binding `T` to a type or `Ts` to a list, its test decided in
  the caller's instance, where the call used to be kept (R1096; MIR text
  1.102, package 92). A `TypeList` pack query at a concrete pack still
  keeps the call (R1342).
- A `@fieldwise_init` struct with a `ref` field now gets its synthesized
  `nodebug` `__init__`, as every other fieldwise struct does (R1278): a
  `ref` field's parameter is the `ref [Self.o] field: T` a written
  constructor spells, so the construction binds the argument's place,
  lends it to the value, and is alias-checked as that `__init__`'s call;
  `Two(x, x)` over two mutable `ref` fields is now rejected as the written
  constructor's call is
  (`assets/extensions/type_error/ref_field_fieldwise_aliasing_arguments.mojo`).
  No call names a struct for its fieldwise value any more, so the VM's
  struct-named `construct` arm, Pliron's `lower_constructor`, and the
  struct-name mappings in `inline-parametric` and the loan analysis's
  `CalleeRefParams` are gone; a display's empty collection still names
  its struct, which the elaborator resolves by arity (R1338).
- Inlining a `nodebug` callee with a `ref` parameter no longer breaks on a
  `ref` field: `sroa` keeps a struct with a `ref` field whole, where
  splitting it crashed the native executable, and `inline-parametric`
  binds a `mut` or `ref` parameter passed a `ref` field (`Pane(self.items,
  …)`, `bump(v.src)`) to the handle the field stores, where the VM had
  bound the field's own slot
  (`assets/extensions/ok/ref_field_written_nodebug_init.mojo`,
  `ref_field_written_nodebug_chained_view.mojo`,
  `ref_field_nodebug_mut_argument.mojo`).
- Pliron no longer tracks or releases owned register temporaries of its
  own: every owned register's end is MIR's, a `drop.reg` or the operand
  that takes it, as upstream's `CheckLifetimes` has it (R1321). The
  per-instruction release flush, the release on each terminator leaving a
  block, and the field-move bookkeeping behind them are gone. A natively
  raised `String` that MIR owns (`raise String("too big: ") + String(n)`)
  now gives the error its buffer, where the raise copied it and leaked the
  original, and a literal passed to a borrowed `String` parameter is
  viewed in place rather than copied.

- `x.write_repr_to(w)` on a built-in `Writable` writes a `repr(x)` text
  that is an owned nominal `String`, as upstream's `repr` returns, so drop
  elaboration destroys it, where it was a `StringLiteral` register only
  Pliron's own temporary tracking freed (R1336). The inverted write's
  `SemanticAdjustment::InvertedReprWrite` carries the checked `repr` type.
  A t-string segment, a literal or a captured place, is now the linked
  `String`'s own construction, as a user's `String(…)` is, where it was a
  `StringLiteral` stringify the literal constructor then copied, so a
  native t-string makes one allocation per segment fewer.

- `String(x)`, `repr(x)`, and `input()` return an owned nominal `String`,
  as upstream's `String.__init__[T: Writable]`, `repr`, and `input` do,
  where each returned a runtime `StringLiteral` the literal constructor then
  copied (R1327). MIR types the call's register as the struct, so drop
  elaboration destroys the buffer it owns and Pliron no longer releases it
  behind MIR's back; a native `String(n)` makes one allocation where it
  made two. `o = String(3)` into an `Optional[String]` now converts as any
  `String` value does, where MIR verification rejected it (R437,
  `assets/ok/optional_string_from_stringify.mojo`), and a native
  `repr(5)` no longer leaks its unwrapped digits.

- Drop elaboration destroys every owned register nothing takes, as
  upstream's `CheckLifetimes` destroys an owned SSA value at its death
  (R1329): `DropValues` numbers the owned registers after the variables,
  one bit each, so a call result only a `read` parameter borrows, or a
  copy only `print` reads, gets a `drop.reg` after its last read, placed
  among the values dying with it in discovery order. A raising call's
  result is defined past its raise edge, so neither arm destroys it; a
  region op's unread result and a region's unread argument die too. Copy
  elision applies both halves of upstream's `optimizeCopyDestroys`: a copy
  or constant nothing reads goes with its drop, and a copy of a slot that
  dies right there is its move, so `var d = c`, `take(e)`, and `(f, 5)`
  over a dying `c`, `e`, or `f` run no copy initializer, as the pin does,
  while a nested `def`'s `{var x}` capture still copies. A keyword
  collector (`var **kwargs`) takes the `**` spread and every stray
  keyword, and a register a call passes as a compile-time argument is no
  owned value. Pliron marks an owned temporary only for a register MIR
  does not own, so `MaybeUninit[String](mk("hello"))` then
  `b^.unsafe_assume_init()` no longer double-frees natively, and a native
  drop of a `Variant` destroys its active payload.
- A `Variant`'s new payload is moved in, as upstream's
  `init_pointee_move` moves it (R1064): `variant.make`, `variant.set`, and
  `variant.replace` take their operand and name the payload's
  `__moveinit__` (MIR text 1.101, package 91), so
  `Variant[CBox[1], Int](b.copy())` reads `101`, not `201`, and
  `Variant[D, Int](D(1))` runs no copy initializer.
- Which operands end an owned register is now read off each call's
  conventions, as upstream's `lit.call` carries its callee's, through one
  per-function classification the verifier reads (`mir::RegisterEnds`,
  R1325). `call.method` records its callee's receiver convention in place
  of `deinit_receiver` (MIR text 1.100, package 90), and calls bind through
  the one callee resolver the register-loan analysis uses (`mir::Callee`),
  so an owned argument through a trait bound, a callable struct's
  `__call__`, a subscript's owned key, a struct name's `__init__`, and a
  payload taken out of storage or a variant end their registers, while a
  pack over a shallow read is borrowed and takes no element, and a new
  variant payload copies its operand. `hasher^.finish()` over a `Hasher`
  bound now hands its receiver over without a move, as an ordinary owned
  receiver does, and a call `native::mono` retargets to an instance takes
  the instance's receiver convention. Output is unchanged on both
  backends; no pass destroys an unconsumed owned register yet (R1329).

- `drop.reg` names its destructor, as `drop.var` does (MIR text 1.99,
  package 89): it is the death of an owned register, upstream's direct
  `ValueToDestroy`, which `lower_drops` writes as the `deinit`-receiver
  call on the register, and which the VM and Pliron now run when it names
  none. No pass emits one yet (R1329).
- The MIR verifier rejects an owned register ended twice on some path, as
  `CheckLifetimes` rejects a use of a consumed owned SSA value: the slot-end
  walk keeps the owned registers some path has stored, passed to an owned
  parameter, built into an aggregate, carried out of a region, or dropped
  (`mir::register_ends`).

- The MIR verifier's slot-end rule is field sensitive, as upstream's
  `ValueSet` in `CheckLifetimes`: a field or tuple element a `place.move`,
  `drop.place`, `deinit` receiver call, or `move` capture ends stays ended
  until a write names it, and a `drop.var`, `drop.place`, or `deinit`
  receiver call that would destroy an ended part is rejected (R1319). With
  that proof, Pliron's flow-insensitive refusal of a whole drop after any
  part's end (`partially_moved`) and the VM's skip of a whole destructor
  over a moved-field tombstone are gone. Output is unchanged on both
  backends.

- `sroa` splits a slot stored whole from another slot's forwarded read, as
  upstream's `SROA` does, so `var w = W(3)` over an `@always_inline`
  initializer splits `w` (R1309). A `field.get` chain whose result a
  store, a `struct.create` or tuple element, or a region terminator
  consumes now moves that field out of a Pliron owned temporary, as a
  store of upstream's `kgen.struct.extract` moves the element, so the
  temporary releases only what is left instead of freeing each moved field
  a second time (`assets/ok/sroa_forwarded_out_self_splits.mojo`).

- An explicit destroy (`x^.finish()`, `x.f^.finish()`) is now the call
  alone: `call.method` records `deinit_receiver`, its callee's `deinit`
  receiver convention, which the verifier checks against the declaration,
  and the call consumes its receiver place, as upstream's call consumes a
  `DeinitMem` operand (R1313). The move scan, drop elaboration, the
  interior and loan transfers, `sroa`, and the slot-end verifier read the
  end off the call, so MIR lowering emits no trailing `consume.var` or
  `consume.place`; `consume.place` is withdrawn, and Pliron's
  `consumed_receivers` workaround and the inliner's trailing-consume
  removal are gone. Output is unchanged on both backends; MIR text is 1.98
  and the package format 88.

- Native drops no longer test a per-leaf presence flag, as upstream's
  `DestructorInsertion` destroys each field statically (R1312): every
  `drop.var` and `drop.place` Pliron lowers destroys unconditionally at the
  address it names, a part's end only records the slot so that a whole
  drop after it refuses (R1319 replaces that backstop with a verifier
  proof), and droppable locals are no longer zeroed at entry. The flags
  were written on every partially moved slot and tuple, but no corpus
  fixture ever read one false. Output is unchanged on both backends; the MIR text and package
  formats are unchanged.

- `lower-lit` now erases `ownership.mark_destroyed` and `consume.place`
  too, as upstream's `LowerLIT` erases `lit.ownership.mark_destroyed` and
  `lit.ownership.mark_consumed`, so no backend sees one: concrete
  verification, the VM, and the Pliron backend refuse a survivor, and
  Pliron's per-part end tracking (`end_presence`, `ended_inside`) is gone
  (R1311). A lowered field drop is the destructor's call on the field's
  place, whose `deinit` receiver consumes it, as upstream's call consumes a
  `deinit` operand; a pack part's drops end with their last element; and
  neither the elaborator's drop lowering nor its list-literal construction
  emits a marker. `inline-parametric` binds a destructor's `deinit self`
  in place from the call's receiver place, a part reached through a `mut`
  parameter included, where it copied the whole value into a fresh slot
  (`assets/ok/mut_parameter_reassignment_drop.mojo`, concrete instructions
  85 → 77; `always_inline_nodebug_destructor.mojo` 418 → 400). `sroa`
  counts such a call as its field's end. Output is unchanged on both
  backends; the MIR text and package formats are unchanged.

- A `consume.var` destroys nothing on either backend, as upstream's exit
  `mark_destroyed` consumes only the whole-object bit (R1310). Drop
  elaboration lays a nominal `Tuple` out as the one-field struct it is, so
  its synthesized `__deinit__` destroys `storage` by a named drop that
  `refine_pack_drops` splits per element, where the VM's `ConsumeVar` and
  Pliron's `lower_consume_var` used to tear the elements down at run time.
  Output is unchanged on both backends; a native run no longer leaks the
  elements of a consumed `Tuple`'s split storage
  (`assets/ok/pack_struct_explicit_in_generic_body.mojo`). The MIR text and
  package formats are unchanged.

- The array of element pointers a call builds for its callee's `*xs: T` is
  now upstream's `!pop.array<n, Pointer[T, origin]>`, built by one
  `array.create` (`pop.array.create`) and stored in the caller's
  `__passed_varargs__` slot, where it was compiler-private tuple storage
  (R1304, partly). MIR gains the `pop_array` type and the `array.create`
  instruction (MIR text 1.97, package format 87), which the VM and Pliron
  lower. The call still builds the `Span` and the list itself, where
  upstream calls `VariadicList`'s `@implicit` constructor: R1314, after the
  constructor's vocabulary (R1315–R1317).
- Native drops carry no whole-slot flag, and no return or propagate block
  sweeps the slots still flagged, as upstream's destructor insertion leaves
  only static destroys (R832). The MIR verifier now requires every path
  reaching a `drop.var` or `var.consume` to hold its slot, upstream's
  `UninitializedValueScan` rule, where it used to reject only a slot every
  path had ended. Drop elaboration no longer roots the slot of a
  compile-time value parameter of any type (`mir::parameter_slots`), which
  left drops of slots nothing defined. A traced native executable logs a
  catch only of an `Error`, as the VM does, not a `for` loop's
  `StopIteration`.
- Every drop is decided where drop elaboration places it, as upstream's
  `DestructorInsertion` leaves an unconditional destroy, so
  `mir::lower_drops` (formerly `lower_whole_drops`) lowers every named
  `drop.var` and `drop.place` to its destructor's call, and concrete
  verification refuses a named drop (R1306). The per-slot
  definite-initialization fold that left about 600 drops per corpus pass
  tested at run time is gone. Two producers stop leaving undecided drops: a
  temporary an expression region's argument makes now dies in that region,
  so an `or` operand's `$arg_loan` temporary is no longer destroyed at the
  other arm's entry before its definition; and a store refilling a field
  only some paths moved out no longer gets a tested drop, the one place
  that reached (snippet-mode module scope) rejecting it instead. The VM's
  `consume.place` destroys nothing, as Pliron's, so a destructor inlined on
  a `mut` parameter's whole value no longer runs twice. Output is unchanged
  on both backends; the MIR text and package formats are unchanged.
- `lower-lit` erases every `ownership.mark_initialized` too, as upstream's
  `LowerLIT` erases `lit.ownership.mark_initialized`, so concrete MIR holds
  none (about 300 per fixture before) and concrete verification, the VM,
  and the Pliron backend refuse one (R932). `inline-parametric` opens an
  inlined initializer's `out self` slot with a `var.declare`, upstream's
  `lit.var.decl`, where it emitted the mark, and the VM builds a struct or
  tuple slot's unwritten storage where an instruction first uses it rather
  than at the mark. `ownership.mark_destroyed` and `consume.place` still
  reach the backends (R1310, R1311). Output is unchanged on both backends;
  the MIR text and package formats are unchanged.
- `lower-lit` erases every `loans.establish` too, as upstream's `LowerLIT`
  leaves no loan op behind, so concrete MIR holds none (an empty
  program's held 200) and concrete verification, the VM, and the Pliron
  backend refuse one, so an artifact written by an older binary that still
  holds one no longer runs (R931). A `ref` binding whose reads lowering
  substitutes by its place (`ref r = x.a[i]`) now stores that place's
  `ref.make` handle in its slot with a `var.store`, as upstream's `lit.ref`
  is a value, where Pliron read the binding's loan marker to materialize
  it; a native place through the binding addresses from that handle.
  Output is unchanged on both backends; the MIR text and package formats
  are unchanged.
- `sroa` splits a struct slot whatever its fields' types and whatever its
  drop runs, as upstream's `SROA` tests only that the element types
  resolve (R1267). A memory-only slot, one whose type declares neither
  `RegisterPassable` nor `TrivialRegisterPassable`
  (`MirDeclarations::register_passable`, upstream's
  `getRegisterPassability`), stays whole while a call takes it by address:
  a stored call result, or a whole read a call takes, such as its
  destructor's `deinit self`. So `var s = S(1, String("a")); s.n += 1`
  keeps `s` in the first round and splits it once the second round's
  `inline-parametric` has inlined the synthesized initializer and
  destructor, as at the pin, and its concrete instructions fall from 108
  to 99. A `place.move` left on a whole field slot becomes a `forward`
  read, and the VM gives a split struct's whole field slot of tuple
  storage its placeholder when it is marked initialized. The elaborator
  now decides a struct's `RegisterPassable` and `TrivialRegisterPassable`
  conformances by its declarations, where it answered yes for every
  struct, so `Tuple[String, Int]` is no longer `RegisterPassable`. Files
  R1308 and R1309.
- A `VariadicList`'s field is upstream's `Span[Pointer[T, origin],
  ImmUntrackedOrigin]` over an array of element pointers, which every call
  builds in MIR as upstream's call builds its `!pop.array`
  (`mir::variadic_list::build_variadic_list`, shared with the elaborator's
  `Array` display construction), replacing the compiler-private
  `__VariadicStorage[T]` (R1073). A `read` collector's elements are the
  caller's own places, borrowed where they lie, rather than bitwise images;
  an owned collector's are caller slots its list then owns, marked
  destroyed once the list is built. `__len__`, `__getitem__`, and
  `__deinit__` are library code over the span, so the VM's pack storage and
  Pliron's `build_variadic_list`, pack-descriptor layout, and `VariadicPack`
  index, length, and projection arms are gone; the VM moves a place pointer
  along the array it points into, and an owned `for v in xs^` narrows the
  span on both backends. The call still builds the span itself where
  upstream runs the list's constructor (R1304), and the bundled
  `__getitem__` binds the span's pointer before indexing it (R1305).

- A homogeneous collector (`*xs: T`, `var *xs: T`) is the bundled
  `VariadicList[T, is_owned]` in its callee's body, declared beside
  `ParameterList` in `stdlib/std/builtin/variadics.mojo` with upstream's
  `__len__`, `__getitem__`, `__iter__` (`_VariadicListIter`), and
  `__deinit__`, and exported from the prelude (R743). `xs.__len__()` runs,
  `var l = xs` is rejected as at the pin, a borrowed loop is the library
  iterator, so MIR's `PackCursor` and `IterationProtocol::indexes_pack` are
  gone, and an owned list is destroyed by its `__deinit__`, last to first.
  The call builds the storage (`tuple.make`) and the list over it
  (`struct.create`); the field is still the compiler-private
  `__VariadicStorage[T]`, where upstream's is a `Span` of element pointers
  (R1073). An element reached through a mutable-reference `__getitem__`
  with no `__setitem__` is now an assignment place, so `xs[0].v += 7` over
  a `var` collector runs and prints the pin's `18 12` (closes R742), as
  `s[1].v = 5` over a `Span` does. MIR text is 1.96 (a trait
  requirement records its collector's convention); the package format
  is 86.

- Every call builds the keyword collector its callee's `var **kwargs`
  takes, as upstream's call emission builds the dictionary (R1070):
  `fill_omitted_arguments` emits `StringDict[T]`'s empty constructor into a
  forwarding slot and one `_insert` per collected keyword, keyed by the
  keyword's `StringLiteral` (the bundled `StringDict` gains upstream's two
  `_insert` overloads), and passes the slot as the call's one `**` operand,
  the shape a forwarded `**kwargs^` already had; calls through a bound and
  through a function value build theirs from the requirement's or the
  function type's call shape. The VM's `bind_args`, Pliron's
  `bind_call_slots`/`bind_contract_slots`, and `inline-parametric` bind
  that operand whole, so `named(a=1, b=2)` over a `nodebug` callee now
  inlines, and the VM's `make_kwargs_dict`/`take_forwarded_kwargs`, Pliron's
  `build_keyword_collector`, the elaborator's member-table record of the
  collector's constructor, and `mir::CONSTRUCTED_MEMBERS` are gone; no
  member table records a constructor. The verifier rejects a call of a
  keyword-collecting callee that passes no collector.

- A whole-pack spread (`show(*args)`, `take(*args^)`) passes the caller's
  pack itself, as upstream's call passes the `VariadicPack`, instead of
  reading each bound element into a fresh collector (R1103). The
  elaborator's `expand_pack_spreads` keeps only its check that the
  bindings close the pack, and expands a spread into the builtin `print`
  alone (R1287). Pliron binds a `read` collector to the caller's storage in
  place and an owning one to the transferred pack, on the native backend
  as on the VM.

- Every call builds the positional collector its callee's `*xs` takes, as
  upstream's call builds the `VariadicPack` (R1102). MIR's
  `fill_omitted_arguments` pass emits a `tuple.make` of the overflow
  operands, typed the collector's slot, and passes it as one operand at the
  call's `spread` position, a keyword-filled slot before it moving to its
  position. Tuple displays name `Tuple`'s variadic `__init__` (recorded on
  the checker's `ConstructCollection`), list displays and array literals
  their literal constructors, a call through a bound whose requirement has
  no body builds from the requirement's call shape (`TraitRequirement`
  gains `required`, `positional_only`, `keyword_only`, `variadic`, and
  `kw_variadic`), and variadic subscripts build theirs too, which now run
  natively as well (`assets/ok/variadic_subscript_collector.mojo`). The VM's
  `bind_args` and Pliron's `bind_call_slots` take the collector whole,
  `build_variadic_list` is the `tuple.make` lowering alone, and
  `inline-parametric` binds the built collector rather than building its
  own. The elaborator reads a spread's elements and builds the call's
  collector of them (`print` and the builtin-string writer still take the
  elements one by one). The verifier rejects a call of a declared
  collecting callee that passes none. A `deinit` argument transferred
  ahead of an inlined call's region now binds the caller's slot, which
  fixes the double destruction of `v^.concat(Tuple(x))`'s receiver
  elements. MIR text is 1.95 and the package format 85. R1297 files a MIR
  reader hang found on the way.

- A drop of a pack aggregate, a `Tuple`'s storage or a heterogeneous
  `var *args: *Ts` collector, names each element's `__deinit__` (R1090),
  as upstream's `Tuple.__del__` and `VariadicPack.__deinit__` unroll a
  `comptime for` of `destroy_pointee` over the elements:
  `mir::refine_pack_drops` refines it into one `drop.place` per element
  whose destruction runs code, a tuple's first to last and a collector's
  last to first, then ends the pack with a `consume.var` or
  `consume.place`, at the end of drop elaboration for a concrete pack and
  in the elaborator for an instance's. Each element's drop is then named,
  respelled, and lowered to its destructor's call as a field's is, so the
  VM and the native backend no longer tear a dropped pack down by type,
  and their owning-collector branches for a heterogeneous pack are gone
  (`assets/ok/pack_drop_names_element_destructors.mojo`).

- A `for` loop or comprehension over a raising iterator calls its
  `__next__` as an ordinary raising method call inside a `try` whose
  handler runs the loop's `else` body and breaks, as upstream's
  `emitForStmt` calls it with a `lit.loop.break.else` handler (R890). The
  element is stored into the loop's yield slot inside the `try`, and the
  loop variable binds after it. `iter.try_next` (`MirInstr::TryNext`,
  `HirInstr::TryNext`) is gone, with the VM's and Pliron's own lowering of
  it, so `inline-parametric` now inlines an `@always_inline` `__next__`
  into the loop as the pin does, while a callee holding such a loop keeps
  its call (R1288). The loan check follows a `try`'s control flow, so the
  loop's exit through that handler still carries the loans live after the
  loop (`assets/ownership_error/pointer_to_owner_across_for_loop.mojo`).
  Every corpus fixture's output is unchanged on both backends. The MIR
  text format is 1.94 and the package format 84.

- `mem-2-reg` promotes a trivially copyable local stored inside a `try`
  part and declared outside it, as upstream's `Mem2Reg` carries it through
  `lit.try`'s region values (R1264). A `try` has results and handler and
  `else` arguments, a `try.yield` carries the values entering the `else`
  or leaving the `try`, and a labelled `raise` carries the values entering
  its handler, as `lit.try.raise`'s operands do; the VM binds them on each
  edge, Pliron lowers the `try`'s exit, landing block, and `else` as block
  carriers, and `simplify-cf` forwards a removed `try`'s values. A local
  assigned in a `try` inside a loop is now carried through the loop too
  (`assets/ok/try_slot_carried_through_results.mojo`). Every corpus
  fixture's output is unchanged. The MIR text format is 1.93 and the
  package format 83.

- `inline_cost`, the size a plain `@always_inline` callee is measured by,
  prices a `for` loop by its operations rather than at the whole bound:
  the bodies of the `__iter__` and `__next__` its protocol calls, the
  latter in a `try`, as upstream's `emitForStmt` calls and then inlines
  them. A `for` over a small iterator of a program's own now inlines, as
  at the pin, and one over `range` still keeps its call
  (`tests/mir_passes/inline-parametric/for_protocol.mir`). Other prices
  were recalibrated against the pin. A field read or written through a
  reference costs its `struct.gep`, a raising body costs two, a `try`
  costs four beside its parts, and a `comptime assert` costs nothing.
  `print` stays a figure, now 30, the pin's `print(s)` call site, until it
  is the library function (R1287). Shapes still decided unlike the pin are
  R1286 and R1288. This closes R1075.

- `len` is upstream's library overload set in `std/builtin/len.mojo`
  (`@inline(.always)` over `Sized`, raising over `SizedRaising`, and an
  `@unavailable` overload over a string) rather than a checker builtin.
  `len(x)` over a `SizedRaising` value now needs a raising context, a
  struct with an undeclared `__len__` is rejected as at the pin, `len(xs)`
  inlines to the collection's `__len__`, so `flen[n](b: NBox[n])`
  returning `len(b)` is one instance (R1021). A compile-time-only
  operand (`len(Ts)`) is still a checker query (R1284). An argument with
  no runtime form (a compile-time display) now reports its crossing error
  through an overloaded call too. This closes R988 and R1021.
- The VM and the Pliron backend no longer resolve a call naming a struct to
  an `__init__` overload by arity (R1029): every constructor call runs the
  symbol it names, as upstream's backends take the call's symbol. A call
  naming the struct itself now only builds a fieldwise struct with a `ref`
  field (R1278) or copies through `T(copy=x)` (R1283), and a converting
  default runs the constructor its target names. The VM's
  `constructor_name` and `overload_name` and Pliron's `constructor_init`
  and `constructor_init_target` are gone.
- A `nodebug` callee constructing its type binder (`T()`, `Ts[0]()`,
  `T(copy=x)`) now inlines at a concrete binding (R1097): the inlined body
  holds the bound type's own construction, a scalar's zero, a vector's
  zero splat, a struct's initializer call, or the copy, so `make[Int]()`,
  `dup(s)`, and `dup(7)` leave no `make` or `dup` call in `main`
  (`assets/ok/always_inline_nodebug_type_construction.mojo`). The rewrite
  moved from the elaborator into `mir::construct_type_parameters`, which
  the elaborator and `inline-parametric` both run; a construction the
  binding leaves open, such as `Ts[i]()` under a loop binder, keeps the
  call (R445).
- A call through a trait bound that `inline-parametric` inlines at a
  concrete struct now names the member the struct's conformance witnesses
  rather than the abstract dispatch (R1052): the checker records each
  declared conformance's witness table on the struct's declaration (MIR
  text 1.89, `mojito-package 78`), and the inliner's substitution folds a
  method call, comparison, or subscript through it, as upstream's
  substitution folds the witness. A caller that reached the member only
  through an inlined bound, such as `fbound[n](b: NBox[n])` returning
  `mylen(b)`, now keeps one instance for every `n`, as on the pin.
- An `@fieldwise_init` struct's construction calls a synthesized `nodebug`
  `S.__init__`, as upstream's `StructEmitter::synthesizeFieldwiseInit`
  declares one, rather than naming the struct (R1049). Each field that is
  not register-passable-trivial is an owned `var` parameter moved into its
  field, so `W(c, 2)` copies `c` and then moves it, `W(C(5), 3)` moves the
  temporary once, and `W(c^, 4)` moves once, as the pin prints
  (`assets/ok/fieldwise_init_owned_arguments.mojo`); Mojito ran one move
  fewer. The constructor inlines, so a generic function building a struct
  over its parameter keeps one instance (`fieldwise_constructor_instances.mojo`),
  and `sroa` splits a fieldwise-built slot through its stores. A struct
  with a `ref` field keeps the struct-named call for now (R1278).
- A copy or move constructor's declaration restates its struct's
  parameters ahead of its own, as an `__init__`'s does, since upstream's
  copy and move constructors are `__init__` overloads in the struct's
  generator (R1050). `C[n](copy=c)` passes `n` as the constructor's
  compile-time argument, as any construction does, rather than naming
  `C[n]` as the call's receiver, so a copy constructor that reads no
  struct parameter is one instance for every `n`
  (`assets/ok/construction_unread_struct_parameter_instances.mojo`). The
  synthesized copy and move constructors restate them too, and each copy
  or move that copy insertion lowers to a constructor call
  (`mir::lower_copies_and_moves`) passes the value's struct arguments, as
  upstream's copy insertion calls the `__init__` generator.
- MIR has no CFG terminator (R1274): `Jump`, `Branch`, `ComptimeBranch`,
  the `ComptimeFor` terminator, and `EscapeJump` left `MirTerm`, as Mojo
  has no CFG above LLVM, and MIR text 1.88 withdraws their spellings
  (`jump`, `branch`, `comptime_branch`, `comptime_for.iterator`,
  `escape`), so an older artifact holding one fails at load as an unknown
  terminator. `mir::verify` itself now requires every region, the body
  included, to be one block, as upstream's `hlcf` regions are
  `SizedRegion<1>`, which replaces the separate shape checks
  (`verify_structured_function_in`, `verify_structured_shape`), the
  `is_structured_shape` guards of `sccp`, `mem-2-reg`,
  `lower-arg-conventions`, `simplify-cf`, and Pliron's `compile`, and
  `opt`'s `OptError::Unstructured`. Every walker reads one block per
  region: the ownership walks, drop lowering, and the slot-ends check lose
  their block-list fixpoints and escape channels, `cse` its dominator tree
  (`mir::immediate_dominators`), and `canonicalize` its unreachable-block
  erasure (`mir::prune_unreachable_blocks` and the `erased_blocks`
  counter), keeping only `cut_try_cleanups`; the VM loses its CFG arms and
  `Flow::Jump`. The pass cases that held CFG terminators were rewritten as
  structured `if`s, and `canonicalize/unreachable_join` and the two
  `lowered_body` refusal cases were deleted. No program's output changes.

- The transitional `lower-control-flow` MIR pass is gone (R1273): as
  upstream lowers `hlcf` only on its way to LLVM, Pliron's region lowering
  is now the only lowering of control flow to blocks. `lower_cfg` and
  `lower_checked_program` (with its importing and per-unit forms) return
  the structured body, replacing their `_structured` twins, and `mojito opt
  --pass lower-control-flow` is an unknown pass. The slot-ends check and
  `cut_try_cleanups` walk the structured body as upstream's
  `CheckLifetimes` does, an `if` or `match` joining its arms, a loop
  iterated to a fixpoint with its `break`s as its live-out, and each
  region exit resolved through every `finally` it crosses, so a `break`
  crossing two `try`s is no longer checked as a fall-off.

- `cse` now merges built-in operators (R1270), as MLIR's `CSE` merges
  upstream's built-in arithmetic and comparisons, which are free of
  memory effects: a `binary` or `unary` with no operator method and no
  `raises`, over built-in scalar or SIMD operands, is erased when a
  dominating identical one matches. A duplicate division is dominated by
  the identical division, which traps first. `(a * b) + (a * b)`
  multiplies once, and an operator over a type parameter stays
  (`tests/mir_passes/cse/built_in_operators.mir`). Program output is
  unchanged, and so are the MIR text and package formats. This closes
  R1270.
- `sroa` and `mem-2-reg` take a slot of a generic struct or field type, as
  upstream's `SROA` and `Mem2Reg` take any allocation (R1266). A struct's
  fields are judged and typed at its arguments
  (`types::struct_field_types`, `MirDeclarations::field_types`), so a
  generic body splits a slot of its own application (`Arr[n]`'s field
  slot is a `SIMD[DType.int64, n]` over the body's `n`) and `mem-2-reg`
  promotes it, and `P[Int]` over a field `T` splits, its synthesized
  destructor running no code at `Int`. A `consume.place` or `drop.place`
  left on a whole field slot becomes `consume.var` or `drop.var`, so a
  nested struct's field slot splits the next round and an inlined
  destructor's receiver keeps no stale marker. A dead `Arr[n](...)`
  construction passed to a nested function's removed argument then leaves
  no read of `n`, so `plain_nested[1/2/4]` is one instance, as on the pin
  (`assets/ok/generic_struct_slot_instances.mojo`). Program output is
  unchanged; over the corpus, generator instances fall from 14,003 to
  13,996, generator instructions by 0.6 %, and concrete instructions by
  0.23 % (`benchmarks/instantiation.tsv`). The MIR text and package
  formats are unchanged.
- Pliron now lowers region ops to LLVM blocks itself, as upstream's
  `LowerControlFlow` lowers `hlcf` and `lit.try` (R883, stage 14a of
  `docs/structured-control-flow-plan.md`), so neither backend runs
  `lower-control-flow` and `compile` refuses a lowered body. Each region's
  block is an LLVM block: `if` enters it with a conditional branch, `loop`
  and `match` with a plain one, an op's results and a region's arguments
  are block arguments (an aggregate through staging storage), and
  `hlcf.unreachable` is `llvm.unreachable`. A `break`, `continue`,
  `match.next`, or `match.complete` resolves to the region op it names
  and crosses each `finally` on the way, so a `break` or `continue` from a
  `try` nested in another `try`'s part, to a loop inside that part, now
  runs natively too (`assets/ok/try_nested_loop_exit_crosses_finally.mojo`,
  closing R905). Owned register temporaries release on the region tree,
  in their defining block after the instruction there that reads them
  last, where the release used to follow one block list's CFG; a final
  read inside a `try` part no longer rejects (R1113 keeps the raise-edge
  half). Every corpus fixture's native output is unchanged
  (`assets/ok/loop_result_two_breaks.mojo`,
  `try_finally_break_carries_value.mojo` pin results reached by two
  edges and across a `finally`). The capability matrix and the MIR text
  mnemonic inventories, which had fallen behind the instruction set, are
  current again. This files R1273 (the pass and its remaining callers)
  and R1274 (the CFG terminators).
- The VM now runs concrete MIR's structured control flow as it stands,
  as upstream's interpreter runs `hlcf` (R882); only Pliron still lowers
  it (`lower-control-flow`), and neither `ConcreteProgram::new` (each CTFE
  fragment) nor `VmBackend::run_concrete` does. Each frame holds a region
  cursor (`backend/vm/regions.rs`), its position through the `if`, `loop`,
  and `match` ops it is inside, which each op's entry and each region
  terminator moves as `transferControlFlowTo` moves the interpreter's
  `pc`, so a call inside any region stays on the explicit frame stack. A
  `break` or `continue` from a `try` nested in another `try`'s part, to a
  loop inside that part, now runs on the VM, running each `finally` (R905's
  VM half; Pliron keeps the limit until R883). Reaching
  `hlcf.unreachable` is now a runtime error rather than a return. Every
  corpus fixture's output is unchanged, and a 200 000-iteration loop runs
  about as fast as before. This files R1272 for a `try`, which still runs
  its parts on the host stack.
- The post-elaboration pipeline now reads structured instances (R881), as
  upstream's function pipeline reads `hlcf`: the elaborator emits each
  instance with its runtime regions, and each backend lowers them
  (`lower-control-flow`) at its entry, the VM in `ConcreteProgram::new`
  and `VmBackend::run_concrete`, Pliron in `compile`. A new `simplify-cf`
  pass, upstream's `SimplifyCF` whole, runs after the first `mem-2-reg`,
  as upstream's `AutomaticInline` function pipeline begins: it replaces a
  loop its one final `break` ends, or that nothing jumps to, by its body
  (every `inlined_cf_scope` loop an inlined body leaves), a `try` whose
  body cannot raise by its body and `else`, and, after an `if` whose arm
  cannot fall through, later reads of its condition by the constant it
  implies. `lower-arg-conventions` now judges structured bodies, and
  `canonicalize`'s CFG rewrites (R836), its constant-branch fold, and the
  CFG paths of `sccp` and `mem-2-reg` are gone, as Mojo has no `cf`
  dialect; `mojito opt` refuses a lowered body for the structured-only
  passes. The elaboration counters now count every region, and
  `benchmarks/instantiation.tsv` is regenerated: concrete instructions
  386,579 → 373,688 against the old top-level count, 4.7 % fewer than the
  previous pipeline counted over regions (concrete blocks 9.3 % fewer). Program output is unchanged, and so
  are the MIR text and package formats. This closes R881 and files R1271.
- `cse` now merges a result of any register-passable trivial type (R930),
  as upstream holds only such a value in an SSA value: a multi-lane SIMD
  construction, including a generator's `SIMD[dtype, width]`, a
  `StringLiteral`, a `TrivialRegisterPassable` struct, and the field reads
  and slot copies yielding one. Two identical `SIMD[DType.float32, 4]`
  constructions merge (`tests/mir_passes/cse/storage_values.mir`).
  `canonicalize` also forwards a copy of such a value when its source has
  other readers. Merging is safe because readers that consume a register
  now copy it. On Pliron, the callee of an owned (`var`) parameter of such
  a type copies the value its caller's pointer addresses into a slot of
  its own, as upstream passes the value by value, instead of writing
  through to the caller's register. The VM's `UninitStorage` and
  `StoreToMem` copy their operand instead of taking it. Over a
  186-fixture sample, concrete instructions fall 0.36 %. Program output is
  unchanged, and so are the MIR text and package formats. This closes R930.
- `cse` now merges reads of memory as MLIR's `CSE` merges an op whose
  only effect is a read (R929): a `place.load`, `ref.make`, `ref.read`,
  `var.use` borrow or one-register copy, or field read through a
  reference is erased when an identical read earlier in its block
  matches it with no instruction between that may write, a region op
  counting through what its regions hold. `p.x + p.x` reads `p.x` once,
  and a store between keeps both reads. Concrete instructions fall about
  0.1 %; program output is unchanged, and the MIR text and package
  formats are unchanged. This closes R929.
- The pre-elaboration pipeline's fourth round now ends with `cse`, as the
  pin's ends with a generator `CSEPass` (R926). `cse` walks a structured
  body as MLIR's `CSE` walks regions: a region sees what is available at
  its op and its own arguments, and sibling regions share nothing.
  Generator instructions fall slightly; program output is unchanged, and
  the MIR text and package formats are unchanged. This closes R926.
- Adds upstream's `SROA` as the `sroa` pass (R928), before each
  `mem-2-reg` of the first three pre-elaboration rounds and in the
  post-elaboration pipeline's `'kgen.func'` block. A local slot of a
  declared struct whose drop runs no code and whose fields are concrete,
  non-pointer, and register-passable (or such structs), read and written
  through field places, splits into one slot per field: a whole store
  becomes a `field.get` and a store per field, a whole read a read per
  field and the new `struct.create` (MIR text 1.87, upstream's
  `kgen.struct.create`), and nested structs split a level a round.
  `mem-2-reg` reads a whole-slot `ownership.mark_initialized` as a lifetime
  marker, so an inlined constructor's `out self` promotes, and
  `canonicalize` folds a field read of a `struct.create` to its operand and
  a `struct.create` of one value's fields back to that value, forwarding
  until nothing forwards. `inline-parametric` binds a `read` parameter
  passed a `struct.create`, or an owned temporary with no place whose
  type's drop runs no code, as a value, so a call whose argument the split
  no longer loads from a slot is still inlined. A generic field type (R1266), a
  destructor (R1267), a struct argument (R1268), and a pointer field
  (R1269) keep the slot. Program output is unchanged
  (`assets/ok/struct_slot_fields_split.mojo`); over the fixtures both
  instantiation reports elaborate, concrete instructions fall from 388,659
  to 386,462 (−0.57 %), generator instances from 14,037 to 13,988
  (−0.35 %), and generator-instance instructions from 158,373 to 157,799
  (−0.36 %). MIR text 1.87; the package format is unchanged.
  This closes R928.
- `mem-2-reg` promotes a structured body as upstream's `processRegion`
  does: one scoped walk in which a slot allocated in the innermost region
  holding its mentions, and stored inside a region op, is carried through
  that op's region arguments, init operands, terminator values, and
  results, so a variable assigned in an arm, an elif condition, a loop or
  `comptime for` body, or a `match` case and read after the op no longer
  keeps its slot, and a read inside a `try` part or a `comptime for` body
  of a value stored outside it is promoted. An edge on which the slot
  holds nothing carries the new constant `uninitmem` (MIR text 1.86,
  upstream's `#interp.uninitmem`), which the VM reads as uninitialized
  storage and the native backend lowers to `llvm.undef`. A store inside a
  `try` part keeps its slot (R1264), and natively a pass erases dead block
  arguments before pliron's DCE, which panics on one such shape (R1265).
  Program output is unchanged; concrete instructions over `assets/ok`
  fall 7.6 %. This closes R927.
- The elaborator instantiates each body with its structured control flow,
  as upstream's elaborator receives `hlcf`: a `comptime if` is decided on
  its region op, its taken arm spliced where it stood, and a `comptime
  for` becomes what `processComptimeForOp` makes, a loop labelled
  `param_for_outer` holding one unlabelled loop per iterator, each a clone
  of the body that renumbers only the registers it defines, with
  `simplifyParamForLoop`'s folds. `lower-control-flow` runs on each
  instance at the end of elaboration and on each clone a compile-time
  application's fragment runs, so the dominator-based body recovery
  (`mir::ComptimeForBodies`, `DominatorRoots`) is gone, a `comptime for`
  result one break carries folds, and the concrete check accepts runtime
  regions in an instance but not in a backend's program. Program output
  is unchanged; `tuple_reverse_concat` elaborates in a quarter of the
  time and memory, and `lower_whole_drops` computes each region op's flow
  once per walk instead of once per enclosing loop pass. This closes R880.
- A `match` has results, which each `match.complete` and its else
  region's `yield` carry, and a `comptime for` has operands, body
  arguments, and results, which its `comptime.for.continue`s and
  `comptime.for.break`s carry, as upstream's `hlcf.match` and
  `hlcf.comptime.for` do (MIR text 1.85, whose reader takes an older op
  or terminator as carrying none). The verifier checks every edge and,
  as `ComptimeForOp::verify` does, that a `comptime for`'s operands and
  results agree; `sccp` leaves the new values overdefined, as upstream's
  generic path does; `lower-control-flow` folds a `match` result one
  edge defines and a never-continued `comptime for`'s arguments and
  stores the others, and every `comptime for` result, in slots. The
  verifier's register lists now also cover R1259's `if` and `match`
  region arguments. Nothing produces the values until `mem-2-reg`
  promotes into them (R927), so programs run unchanged. This closes
  R1260.
- An elif's condition region and arm, an `if`'s and a `match`'s else
  region, and each `match` case after the first take arguments, which the
  `if.elifcond.yield` or `match.next`s entering them carry, as upstream's
  `hlcf.if.elifcond.yield` passes its values to its arm and the next
  region and `hlcf.match.next` its operands to the next case (MIR text
  1.84, whose reader takes an older region as taking none and a bare case
  region as a case). The verifier checks every edge, the op's own entry
  included; `canonicalize` decides or promotes an elif only when its
  condition carries no values; `sccp` now follows a constant elif
  condition to the one region it selects and enters a `match` case only
  from an executable `match.next`, as upstream's `SCCP` does, meeting
  each argument with the values carried in; `lower-control-flow` replaces
  each argument of a region one terminator enters by its value and stores
  the others in slots. Nothing produces an argument until `mem-2-reg`
  promotes into them (R927), so programs run unchanged. This closes R1259.
- A structured `loop` has arguments, which its operands define on entry
  and each `continue` that resumes it for the next iteration, as
  upstream's `hlcf.loop` passes its operands to its body block's
  arguments and `hlcf.continue` passes the next iteration's (MIR text
  1.83, whose reader takes an older loop or continue as carrying none).
  The verifier checks the values along each edge against the arguments,
  `canonicalize` gains `RemoveUnusedLoopArgs`, `RemoveDeadLoop` replaces
  each argument by its operand, and a hoisted `continue` carries the
  values its arms yield; `sccp` meets each argument with its operand and
  its continues' values; `lower-control-flow` stores them in one slot per
  argument, read at the body's entry, or replaces each by its operand
  when no `continue` resumes the loop. Nothing produces an argument until
  `mem-2-reg` promotes into them (R927), so programs run unchanged. This
  closes R1258.
- A structured `loop` has results and a `break` carries the values that
  define them, as upstream's `hlcf.loop` and `hlcf.break` do (MIR text
  1.82, whose reader takes an older loop or break as carrying none). An
  inlined call's register is now its `inlined_cf_scope` loop's result,
  each return a `break` carrying the value, as `inlineGeneratorCall`
  replaces the call by the loop's results, so the `$inline{n}$result` slot
  is gone. `lower-control-flow` replaces a result whose loop one `break`
  in its own list leaves by that break's value and stores any other in a
  slot; `canonicalize`'s `RemoveDeadLoop`, the hoists, and the new
  `RemoveUnusedLoopResults` carry break values, and `sccp` meets a loop's
  results with them. `tuple_reverse_concat` elaborates to about a fifth
  fewer concrete instructions. Loop-carried operands, elif and `match`
  region arguments, and `match`/`comptime for` results remain (R1258,
  R1259, R1260). This closes R886.
- The seams no longer lower control flow: the pre-elaboration pipeline,
  `emit-mir`, and the artifacts keep structured bodies, one block per
  region, as upstream's generators keep `hlcf`, and the elaborator lowers
  each template once where it first fetches it (`mono/bodies.rs`
  `Templates`, `PackageBodies::template`). Every inlined body now goes in
  its `inlined_cf_scope` loop, the inliner's block splice is gone, and the
  call takes the inlined result forwarded, so an inlined initializer or
  multi-return callee runs no move initializer, as on the pin
  (`assets/ok/always_inline_result_runs_no_move.mojo`). An unread `if`
  now skips its `elif` condition's call, as the pin does
  (`assets/ok/if_unread_elif_call_skipped.mojo`), and a parameter read
  only in a `comptime if False` arm is removed. The elaborator's
  `comptime for` body recovery and CSE read immediate dominators
  (`mir::immediate_dominators`), computed once per unroll step, so
  `tuple_reverse_concat` elaborates several times faster. This closes
  R1254.
- `inline-parametric` inlines a callee whose body holds an `if`, a loop,
  a `comptime if`, a `comptime for`, or a `match`, as upstream's
  `inlineGeneratorCall` does: on a structured body each inlined body goes
  in a loop labelled `inlined_cf_scope`, its returns breaks of that loop
  storing the call's result slot, and its size is priced exactly as its
  lowered form is. A returned reference stored to that slot takes the
  call's reference type, as a spliced body's lone returned register does.
  Both seams still lower control flow first, so production bodies keep the
  block splice and no program's output or MIR changes until R1254; a
  scratch build that keeps bodies structured ran every `always_inline` and
  control-flow fixture unchanged. This closes R1253.
- The pre-elaboration passes read structured bodies as upstream's
  generator passes read `hlcf`: `sccp` enters only the arm an `if` on a
  constant takes and meets the `if`'s results from its arms' yields;
  `canonicalize` applies `RemoveStaticCondition`, `ConditionPropagation`,
  `HoistUnconditionalReturn`, `HoistConditionalReturn`,
  `HoistYieldResults`, `RemoveUnusedResults`, `RemoveDeadLoop`, and
  `ComptimeIfOp::canonicalize`; and `mem-2-reg` promotes a read whose store
  lies before it in an enclosing block. Constant folding, callable-value
  resolution, and a `try`'s cleanup cut now see every region. Both seams
  still lower control flow first, so no program's output or MIR changes
  yet; `mojito opt` runs the new paths, and the rest of R879 is R1253
  (structured inlining) and R1254 (the seams stop lowering). This closes
  R879.

- `EnumLike` stays undeclared: upstream's `std/builtin/enum_like.mojo` and
  the `Optional` members that conform to it use spellings Mojito cannot read
  yet, so R1132 now names its prerequisites as roadmap entries (`!kgen.string`
  literal values R1183, `TypeList` over a member's raw list R1184, a
  `TypeList` value's subscript as a type R1185, `TypeList.of` R1186, a
  defaulted parameterized trait member R1187), and `Bool` conforms with its
  library port (R1188, R1189). Following the pin, conformance will validate
  no case metadata, and reading cases is the patterns' work (R1133).

- `Sized` and `SizedRaising` are upstream's library traits, declared in the
  bundled `std/builtin/len.mojo` and exported by the prelude, rather than
  names the compiler knew without requirements. A bound's `x.__len__()`
  now resolves, as on the pin, and a struct conforms only by declaring one:
  `List`, `Array`, `Set`, `Dict`, `StringDict`, the ranges, the string
  iterators, and `CStringSlice` now list `Sized` as upstream's do, while
  `Optional`, `String`, and an undeclared user struct no longer conform.
  `SIMD` values (`Int`, `UInt`, and `Float64` included), tuples, and packs
  conform as upstream's structs declare, so `len(7)` is one on the VM and
  natively. A user trait named `Sized` now shadows the library's in its own
  module alone (closes R1051 and R510).

- Decorators are validated against the declaration they decorate, as the
  pin resolves each decorator by name per declaration kind
  (`checker/decorators.rs`): `@value`, `@frob`, `@always_inline` on a
  struct, or `@fieldwise_init` on a `def` is rejected with the pin's
  "use of unknown declaration" text, a removed `@register_passable` with
  its removal text, and malformed `@deprecated`, `@stable`, `@doc_hidden`,
  `@fieldwise_init`, `@explicit_destroy`, and `@align` arguments with the
  pin's texts. A trait method now accepts `@doc_hidden`, `@stable`, and
  `@deprecated` (closes R943), and `@explicit_destroy` needs a conditional
  `Deinitable` conformance (closes R34). `@align`, `@unavailable`,
  `@export`, and `@fieldwise_init("implicit")` are unsupported (R1161–R1163,
  R936).
- `ImplicitlyDeletable`, which the pin has removed, is an unknown trait
  rather than a spelling of `Deinitable`.
- `ContiguousSlice` and `StridedSlice` are visible only where an import
  from `std.builtin.builtin_slice` binds them, as on the pin, whose prelude
  exports `Slice` alone. A slice literal's step value, not a second colon,
  selects `StridedSlice`, so `x[::]` and `x[a:b:]` are contiguous; a
  literal bound to a `Slice` parameter builds a `Slice`; a descriptor value
  no longer converts to `Slice`; and `StridedSlice` exposes no bounds and
  `ContiguousSlice` no `step`, matching upstream's declarations (R192).

- A contiguous `List` slice (`xs[a:b]`) is a `Span` borrowing the list's
  owned elements, as upstream's `List.__getitem__(ref self,
  ContiguousSlice)` returns, rather than a copied `List`. A store through
  it shows in the list, clearing the list while it lives is rejected, and
  `xs = rebuild(xs[0:1])` rejects with the pin's aliasing text; owning
  one through `List(xs[a:b])` waits on R1154. `Span` gains upstream's `__bool__`,
  `__eq__`/`__ne__`, `__contains__`, and conditional `Writable`, so a
  slice prints and compares as the copied list did. `Span`'s `repr`
  is not upstream's yet (R761).

- The differential pin is now Mojo `1.2.0.dev2026101705` (upstream
  `cfd79ca736`, 2026-10-17), whose window released `match`/`case` pattern
  matching, and three of its spelling changes reach Mojito. `match`, `case`,
  `class`, `del`, and `yield` are keywords: a variable, parameter, field, or
  imported name spelled by one is a parse error unless stropped
  (`` `match` ``), a free or nested `def` named by one is still rejected by
  the checker, and a method may still be named `match` or `case` and called
  as one. The bundled hashing package is `std.hash`, as upstream renamed
  `std.hashlib`. The descriptor `Writer` is `RawFileDescriptor` with a
  `_value` field, as upstream renamed `FileDescriptor`; its `c_int` type and
  `-1` niche are roadmap R1127. `docs/mojo-nightly.md` carries the audit,
  the window's unimplemented features (R1114–R1127), and the two upstream
  regressions the run exposed, which are probes rather than divergences: a
  `for ... else` over `range` skips its `else` at the pin, and the explicit
  `Tuple[*Ts](*args^)` application no longer constructs, so every fixture
  spells the inferred `Tuple(*args^)`.

- The Pliron backend now depends on the crates.io releases `pliron` and
  `pliron-llvm` `0.20.0` rather than a pinned git revision; the toolchain
  is unchanged (`llvm-sys 231`, LLVM 23.1). The adapter follows the API:
  `Located` is gone (`loc()` is inherent), `convert_module` takes a
  `ToLLVMOptions`, and `erase_given_names` and `Locator::new` take a shared
  `Context`. The capability table gains the `var.declare` row the
  instruction inventory already had. Dependencies name released versions
  from now on; a git pointer is for testing an unreleased fix only
  (`AGENTS.md` §Working Practices).
- Natively, an owned heap-carrying temporary whose final read sits in a
  later block of its list is now released after that read, with the
  buffers freed on every edge that leaves the path between the two, where
  it was rejected with "owned heap-carrying temporary used across blocks".
  So `(keys[i].copy(), values[i].copy())`, whose second subscript's inlined
  bounds check branches between the first copy and the tuple's constructor,
  runs natively: `Dict.__init__` from key and value lists
  (`assets/ok/collection_display_default.mojo`,
  `assets/ok/container_hashable.mojo`), `Codepoint.from_u32(103).or_else(
  fallback)` (`assets/ok/nominal_string_codepoint_from_u32.mojo`), and
  `text += " " + options["color"]` (`assets/ok/kwargs_keyed_read.mojo`)
  match the VM. A read the release cannot follow — one the definition does
  not dominate, a definition reached again before the read, a sibling arm
  reading the register, or a read in another `try` region's block list —
  still rejects (R1113). This closes R1091.
- A `def` declared in a `comptime for` now loses a runtime argument its
  body never uses, as upstream's `RemoveUnusedParams` treats the lifted
  function like any generator: each unrolled copy's call names the reduced
  generator and passes no operand for it, while a function value still
  names the full one. So `looped[1]` and `looped[2]` share one
  `inner(a: Arr[n], y)`, `inner(a: Arr[i + 1], y)` is one instance for both
  loop indexes, and a generic `inner[m](a: Arr[m], y)` one for both `m`, as
  on the pin. Fixture
  `assets/ok/comptime_for_nested_unused_argument_instances.mojo`. This
  closes R1039.
- A runtime argument a function only passes back to itself, at its own
  position by every self-recursive call, now leaves the generator its calls
  name, as upstream's `RemoveUnusedParams` reads through a self-recursive
  call: `remove-unused-params` assumes each such call removed the arguments
  it passes as themselves and narrows to those found unused, and then
  removes the parameters only their types named, the self-recursive call
  forwarding those too, spelled, inferred, or through its receiver. So
  `frec[n](a: Arr[n], k: Int)` returning `frec(a, k - 1) + 1`, a keyword
  `fkw(a=a, k=k - 1)`, a two-call `fdeep`, and `Grid[n].mrec(self, a, d)`
  have one instance each, `frec$REMOVED$ARG$mono` and
  `Grid.mrec$REMOVED$ARG$mono`, for every `n`, as on the pin, while an
  argument passed back at another position, one read beside the forward,
  and a forward around a longer cycle keep one per `n`. A static method's
  self-recursive call through `S[Self.n]` or `Self` forwards the struct's
  parameter to itself the same way, so `S[n].down(k)` is one instance,
  `S.down$mono`. Fixtures `assets/ok/recursive_argument_instances.mojo` and
  `assets/ok/static_method_unread_owner_instances.mojo`. This closes R1034
  and R1065.
- A dunder's runtime argument its body never uses now leaves the generator
  an operator, a call, or a subscript names, as upstream's
  `RemoveUnusedParams` removes it and then the parameters only its type
  named: `Grid[n].__add__(self, o: Arr[n])` returning `self.k + 1` has one
  instance, `Grid.__add__$REMOVED$ARG$mono`, for `g + a` and `g.__add__(a)`
  at every `n`, and `Cell[n].__getitem__(self, a: Arr[n])` one for `c[a]`,
  the subscript shedding the operand (an `index.get` or slice whose one
  operand goes becomes a multi-subscript with none), as on the pin. A
  member a backend reaches from a value's type (a table member, an
  iteration step, a builtin's dunder) and a subscript accessor's receiver
  still keep theirs (R1109). Fixture
  `assets/ok/dunder_unused_argument_instances.mojo`. This closes R1033.
- `inline-parametric` now reads an inlined callee's reduced generator
  through the view the elaborator reads an instance through, its forwards
  to its own callees' removed arguments erased, so a trait default's
  wrapper whose receiver only forwards to a default that ignores it
  (`assets/ok/trait_default_method.mojo` and five other `trait_default_*`
  fixtures) compiles again where R1032 had left it asserting that a
  retired slot was still addressed.
- A static method's runtime argument its body never uses now leaves the
  generator its calls name, as upstream's `RemoveUnusedParams` removes it
  and then the parameters only its type named: `S.st[n](a: Arr[n])`
  returning `3` has one instance, `S.st$REMOVED$ARG$mono`, for every `n`,
  whether called through its struct or an instance, as on the pin. Fixture `assets/ok/static_method_unused_argument_instances.mojo`.
  This closes R1032.
- A `def` nested in a parametric struct's method is now a generator over
  the struct's parameters beside its enclosing scope's, and removes those
  it never reads, so `Counter[1](1).get()` and `Counter[2](2).get()` share
  one instance of `get` and of its nested `bump`, a static method's nested
  `bump[m]` one per `m`, and a `comptime for`'s `bump` one, as on the pin.
  Fixture `assets/ok/nested_method_unread_owner_instances.mojo`; pass case
  `remove-unused-params/nested_owner`. This closes R975.
- A `def` declared in a `comptime for` is now a generator over the loop's
  binder beside its enclosing scope's, and removes those it never reads,
  as upstream's `RemoveUnusedParams` treats the lifted function: `inner`
  reading neither `n` nor the loop index has one instance for `looped[1]`
  and `looped[2]` where it had four, and one reading the index one per
  index. `looped` itself has one instance too, since an application of a
  lifted body, such as the loop bound's, no longer reads the binders that
  body removed. A struct instance's method whose loop declares a `def` now
  calls that `def`'s instance instead of failing elaboration as a
  construction of the struct (R974,
  `assets/ok/comptime_for_nested_unread_instances.mojo`).
- A nested generic `def` now removes its own compile-time parameters its
  body never reads, beside its enclosing scope's, as upstream's
  `RemoveUnusedParams` treats the lifted function: `inner[1]` and
  `inner[2]` share one instance when `inner[m]` ignores `m`, and an
  enclosing function that only forwards its parameter to that `m` keeps one
  instance too, as on the pin (R973,
  `assets/ok/nested_generic_unread_own_instances.mojo`).
- A converting default over a struct whose constructor reads its parameter
  now runs at a generic callee that removed the binder the parameter's type
  spells (`h[k](t: Tag[k] = 2)`): every call omitting it already builds the
  struct its own application spells, so the callee's instance names no
  construction of the erased struct, as upstream's callee names no default.
  `h[4]()` and `h[5](1)` failed elaboration before. This closes R1094, whose
  `f()` over `t: Tag[3] = 5` the 2026-10-17 gate fix had already repaired
  (`assets/ok/converting_default_reads_erased_struct_parameter.mojo`).
- A place arm of a conditional expression that is `Copyable` but not
  `ImplicitlyCopyable` is now rejected, as the pin rejects its implicit
  copy into the conditional's result (`var t = r if c else M(3)`).
- The 2026-10-17 overnight gate's failures are fixed. A native subscript
  through a pointer field of an instance that erases an argument strides by
  the pointee the place records, not the field's erased `None` pointee, so
  a second `Dict` key is found again natively. An omitted converting
  default (`t: Tag[3] = 5`) calls its owner-less constructor with the
  struct's arguments, a function named only by a callable parameter's
  default stays in the program, and a removed parameter that an omitted
  callable default reads (`fallback = callback`) keeps its argument. The
  VM's reference to a `ref`-typed place is again the field's own, and a
  synthesized copy or move of a `ref` field reads the handle through it,
  so `box.value += 2` on a `ref` field runs again.

- A field's drop is now the call of its destructor on the field, as
  upstream calls `__del__` on a field's address (R1081). Where its owner
  holds the field whole, a named `drop.place` lowers to a load of the
  field, the destructor's call with the field as its receiver's place, and
  a consume of the field, the shape `x.f^.finish()` lowers to: a
  synthesized destructor's fields, a `deinit self`'s remaining fields, and
  a field a store overwrites, through a `mut self` too. `inline-parametric`
  binds such a call's `deinit self`, and an explicit field destroy's, to
  the field's place, so a `nodebug` field destructor inlines where the
  field dies. A `try`'s `except` handler is now entered in the states its
  body's raise points leave, so a variable whole there drops through its
  inlined destructor in the handler. A destructor available only under a
  `where` clause keeps its drop until the elaborator names the instance's.
  The VM now runs an explicit field destroy through a `mut self`
  (`self.d^.finish()` then `self.d = D(v)`), which trapped with "field
  access on non-struct ref", and Pliron's `consume.place` clears the
  field's presence, which fixes a second destruction of a field an
  explicit destroy ended in a `deinit self`. Fixture
  `assets/ok/always_inline_nodebug_destructor.mojo` gains these shapes;
  residue R1106.
- `inline-parametric` now inlines a call of a nested `def` whose closure
  the caller did not show, as the pin does (R1080). A body with a copied
  or moved capture that is only called by name holds each such capture in
  a frame slot of its own, copied or moved at the declaration, and a call
  passes that slot, the shape upstream's SROA leaves of the closure's
  storage; so a call of it inlines, and its unused arguments go (R1038).
  The snapshot now ends after its last call and before its source, as the
  pin destroys the closure's storage.
  A body declared in a `comptime for` inlines inside the loop, and the
  elaborator instantiates a body nested in it from the caller's unrolled
  copy. A callable binder bound to a closure of reference captures binds
  to that closure, so the callee and the closure's body both inline. A
  closure of reference captures counts as trivially register-passable:
  `mem-2-reg` promotes its slot, `canonicalize` forwards its copies, and
  an unread `closure.make` is erased. New fixture
  `assets/ok/nested_def_inline_shapes.mojo`; residues R1104 and R1105.
- `inline-parametric` now inlines every call that collects or spreads
  arguments into a `nodebug`, `builtin`, or small `@always_inline` callee,
  as the pin inlines them (R1079): the call's block builds a positional
  collector as a `tuple.make` typed the collector's slot, a spread binds
  the caller's collector whole, a forwarded `**kwargs^` binds the keyword
  collector's slot, and the consuming `Tuple` members inline on a
  forwarded, loaded, or copied `deinit` receiver, their compile-time
  application over the owner's pack applied to the bound element list.
  The VM gives a forwarded `**kwargs^` to the callee's collector alone,
  never to a named parameter, as the pin binds a forward. Keywords
  collected afresh into `**kwargs` still keep the call (R1070).
- `inline-parametric` now binds every kind of compile-time parameter a
  `nodebug`, `builtin`, or small `@always_inline` callee declares, as the
  pin's inliner substitutes every parameter alike (R1078): a callable
  binder binds to the function the call's register names, an owner's
  origin argument binds nothing, a binder's constraints, `where` clauses,
  and conditional availability are dropped as judged at the call, and a
  `comptime for` sequence closes over the bindings. A `read` argument
  passed from a place, read through a reference the block made, or
  trivially copied binds too, and a copied, iterated, or lent place-bound
  parameter reads the caller's place. The VM forwards a static-origin
  reference made in place through a pointer as the element's heap pointer,
  reads and writes a place projected from a pointer slot through the
  pointee, and follows a `ref` field when storing to an indexed place; the
  concrete verifier compares struct applications without their origin
  arguments. A struct keyed on one pack applied to no element binds the
  pack to the empty list (R1093). New fixture
  `assets/ok/always_inline_nodebug_binders.mojo`; residues R1096–R1101.

- A copy or move naming its constructor is now the call upstream's
  copy/move insertion leaves (R1046): a `var.use copy` lowers to a
  `place.load` of the slot and a call of the `__copyinit__`, a `var.use
  move` to a forwarding use and a call of the `__moveinit__`, and a
  `value.copy` to the call alone, at the end of drop elaboration, after an
  `inline-parametric` splice, and per elaborated instance, which both
  backends run as any constructor call. A synthesized copy or move of a
  `ref` field stores the handle the source field holds, the parameter-use
  analysis reads such a call as it read the use, and Pliron's signatures
  count a `deinit` parameter as consuming. A copy of a struct held through
  a reference alias (`copy(self)` on a `ref self` iterator) now runs its
  copy constructor on the VM, where the use read through the alias. No
  fixture's output changes on either backend.

- A `for` loop over a nominal iterator now binds what `__next__` yields
  only past its exhaustion test, in the body, so its yield slot is never
  filled on the exhausted edge, and an owned `for x in a^` consumes `a`
  into an iterator slot of its own, typed by `__iter__`'s result, whose
  drop names the iterator's `__deinit__` rather than the iterable's
  (R1088), as upstream binds the element in the body and makes the
  iterator a value of its own type. Both drops are now destructor calls,
  and Pliron's conditional drop flag for a `try_next` result is gone. No
  fixture's output changes on either backend.

- The test deciding where a `drop.var` becomes the call of its destructor
  is now a definite-initialization state that follows a `try`'s regions,
  as upstream's `CheckLifetimes` walks them, and tracks moved-out parts
  (R1087): `mir::lower_whole_drops` knows a slot whole after a field moved
  out or destroyed is stored again, after a `try` body defines it, and
  across a `try` that only reads it, and erases a named drop of a slot
  empty on every path to it, which drop elaboration leaves before a `try`
  whose body defines the slot. No fixture's output changes on either
  backend. Natively, `Dict` lookups and owned operand temporaries have
  failed since R923; filed as R1092 and R1091.

- A storage destroy (`unsafe_deinit_pointee`, `MaybeUninit.unsafe_deinit`),
  a variant store's replaced payload (`Variant.set`, with or without
  `init_with=`), and an owned closure capture released at the closure's
  death now name the `__deinit__` they run (R1085), as a `drop.var` does
  since R1016 and as upstream's destructor insertion names the `__del__`
  inside `destroy_pointee`, `Variant.__del__`, and a closure's teardown:
  `mir::name_teardown_destructors` names each at MIR lowering (a variant
  store names one per alternative, selected by the tag at run time), the
  elaborator respells it to the instance, the symbol walks and the
  verifier follow it, and the VM and the native backend call it instead of
  finding it from the value's type. MIR text schema 1.73 carries the
  `destructor` (`pointer.destroy`, `uninit.destroy`, an owned
  `closure_capture`) and `destructors` (`variant.set`,
  `variant.set_init_with`) fields. `mir::destroyed_structs` now answers
  only a consumed residue's fields and a pack aggregate's elements (R1090).
- A call omitting an argument its callee never uses no longer evaluates
  that argument's default (R1083), as upstream's `RemoveUnusedParams`
  rewrites the call to pass nothing for it: the view of a caller the
  elaborator instantiates, with each direct call's operands for removed
  arguments dropped (`mir::with_removed_argument_forwards_erased`, now
  built for every caller of such a callee rather than only one whose own
  removals relied on it), erases the `$default$` call that only fed such
  an operand and the read temporary it was bound to, with its loads and its
  destruction (`erase_unpassed_defaults`). `fside()` over an unused
  `x: Int = side()` no longer prints `side`, as on the pin
  (`assets/ok/unused_argument_instances.mojo`); no other fixture's output
  changes. A converted literal default still runs its constructor (R528).
- A `drop.var` whose slot holds a whole value on every path to it is now
  the call of its destructor (R1043), as upstream's destructor insertion
  leaves a plain `__del__` call at each death: `mir::lower_whole_drops`,
  run at the end of `name_destructors` and again by the elaborator once an
  instance's destructor is named, rewrites the drop to a forwarding
  `var.use` of the slot and a `call.method` of the named `__deinit__` on
  it, the shape an explicit `deinit self` call already lowers to, so the
  verifier, both backends, and the symbol walks treat it as any call. The
  pre-elaboration inliner no longer expands destructors at drops; it binds
  a `deinit` receiver to the slot its register forwards (`Binding::Slot`)
  and inlines the call as any other, so a `nodebug` destructor inlines
  where it did. A drop whose slot some path leaves empty or partial — after
  a partial move, defined inside a region, a `for` loop's yield on its
  exhausted edge, an iterable consumed in place — stays a `drop.var`
  (R1087, R1088). The VM's lifecycle log reports a lowered drop by its
  destructor's `consume` alone. Three `assets/ok` fixtures that failed
  on the VM at the previous commit (`dict_take_items`,
  `nominal_string_codepoint_iter`, `temporary_receiver_view`) run again.
  No fixture's output changes.
- A cleanup list — a `try`'s raise-edge cleanup, a `return.cleanup`'s, an
  `escape`'s — now lists drops that name their destructor (R1042), as a
  `drop.var` does since R1016 and as upstream's destructor insertion names
  the `__del__` in the unwind blocks: each entry is a `CleanupDrop { var,
  destructor }`, `name_destructors` names it, the elaborator respells it to
  the instance, the symbol walks and the verifier hold it to a drop's rule,
  and the VM's `run_cleanup` and Pliron's edge lowering call it instead of
  finding it from the slot's type. MIR text 1.72 writes each entry as a
  `drop { var, destructor }` record and still reads the bare slots of older
  artifacts. No program's output changes.
- An owned closure capture, a variant payload store (`variant.make`,
  `variant.set`, `variant.replace`), and the copy a method or iterator
  result adapter makes of a reference-returning `__next__`'s referent name
  the constructor they run (R1045), as upstream's copy/move insertion
  leaves a call naming the `__copyinit__` or `__moveinit__` at each site:
  `name_constructors` names the template member, the elaborator respells it
  to the instance, the symbol walk and `eliminate-dead-symbols` follow it,
  and the VM and Pliron call it rather than finding it in the struct's
  member table. A method call's adapter and its constructor are one boxed
  `MirResultAdapter`. Pliron runs a named capture constructor into the
  closure record instead of rejecting a capture whose copy or move is
  user-observable. The verifier holds every such site to the copy's rule.
  MIR text 1.71 adds the `constructor` and `adapter_constructor` fields.
  No program's output changes.
- A struct with copyable or movable fields and no copy or move constructor
  of its own now has a synthesized one (R1044), as upstream synthesizes a
  fieldwise `__copyinit__` and `__moveinit__` for every struct that
  declares none: the check lists a `CopyConstructor` or `MoveConstructor`
  lowering unit for each struct whose `IsTriviallyCopyable` or
  `IsTriviallyMovable` conformance is possible but not unconditional, MIR
  lowers it to `S.__copyinit__(out self, *, copy: Self)` or
  `S.__moveinit__(out self, deinit existing: Self)` storing each field of
  the source into `self`'s through a `value.copy` naming the field's
  constructor or a `place.move`, marked `nodebug` and `synthesized`,
  recorded in the struct's member table and packaged as any unit. Every
  copy and move of such a struct names the constructor and reaches its
  fields' through its body, so `relocated_structs` closes over the fields
  only of a struct recording no such member. The checker folds a
  trivial-lifecycle field conjunct no binder decides, so a generic struct
  over `Int` and pointer fields gets no synthesized member, destructor
  included. `inline-parametric` names the drops a splice substitutes, as
  it names the copies, which 95 `assets/ok` fixtures had failed the
  verifier on since R1041. The package format is 68.
- A struct with droppable fields and no `__deinit__` now has a synthesized
  destructor (R1041), as upstream synthesizes a `__del__` for every struct
  that declares none: the check lists a `Destructor` lowering unit for
  each struct whose `IsTriviallyDeinitable` conformance is not
  unconditional, MIR lowers it to `S.__deinit__(deinit self)` with an
  empty body, marked `nodebug` and `synthesized`, recorded in the struct's
  member table and packaged as any unit, and drop elaboration's field
  refinement destroys each field at entry by its own named `drop.place`
  (a field dying at the receiver's consume is now dropped just before it,
  named, in every `deinit` body). A drop of such a struct names that
  destructor, which the pre-elaboration inliner inlines at the drop into
  the field drops, so `destroyed_structs` closes over no named drop's
  fields and the backends recurse into no struct by type at a drop.
  Neither backend traces a synthesized destructor's drop or consume. The
  MIR text format is 1.70 (a declaration may end with `synthesized: true`)
  and the package format 67.
- Every omitted argument is filled at the call, for every call shape
  (R1077, R1060), as upstream's call binding writes the callee's default
  into the call: the lowering of each checked call records its callee, and
  `mir::fill_omitted_arguments` writes each omitted argument into the call
  once every declaration is lowered, a literal materialized at the
  parameter type, a conversion's constructor, or a call of the lifted
  default function under the call's bindings. A subscript (`a[1]` over
  `__getitem__(self, i: Int, r: Owner = Owner(String("sub")))`), a sibling
  nested `def`'s call through its capture, a bundled unit's call into
  another, a call through a trait bound to a trait default's body, and a
  function value rebound to the same function now destroy a borrowed
  default after the call, as on the pin, and a positional-only slot after
  an omitted literal default is passed by position. The callee evaluates
  no default: the elaborator's declaration-side default instance, the VM's
  and Pliron's evaluation of an `Evaluate` slot, and the
  `evaluate_omitted_defaults` post-pass with its `CallSite` reconstruction
  are gone, and a callee's instances are keyed without the binders only a
  default reads (`Box[n].get[m](self, x: Int = Self.n + m)` has one
  instance however `m` varies:
  assets/ok/evaluated_default_argument.mojo,
  assets/ok/lifted_body_owner_instances.mojo,
  assets/ok/nested_unread_enclosing_instances.mojo).
- A call of a nested `def` by name is a direct call of its lifted body
  (R1037), passing the captured places as its leading reference
  arguments, as the pin calls the lifted closure generator; a closure is
  made only at a first-class use, or at the declaration for a body with a
  copied or moved capture or one a sibling captures. Such a call reduces,
  forwards, and fills its omitted defaults as any direct call does, so
  `h[n](x: Arr[n])` returning `ign2(x)` keeps neither `x` nor one instance
  per `n`, and an `imm` capture is a read parameter of the lifted body.
  The `escaping_nested_bodies` dataflow and the elaborator's
  indirect-to-direct rewrite of nested calls are gone
  (assets/ok/unused_argument_nested_variadic_instances.mojo).
- A plain `@always_inline` callee is inlined before elaboration when its
  finished body is small enough (R923), as at the pin: `inline-parametric`
  now follows `always` callees and prices each body as the pin's size
  bound does, a count of operations calibrated against the pin on probe
  bodies, so 31 `s += x` statements inline and 32 keep the call, ten `if`s
  inline and eleven do not, four `while` loops inline and five do not, one
  `print` inlines and two do not, and a `for` loop never does. The bundled
  library's `@inline(.always)` markings now act, so library-heavy programs
  instantiate fewer functions; the inlined code takes the call's location
  (R1074), `print` and `for` are priced by observation (R1075), and a
  slot with several stores is not folded before pricing (R1076)
  (assets/ok/always_inline_small_calls.mojo).
- A function with a homogeneous collector (`*args: T`) has one instance
  for every call arity (R1040), as at the pin, where each arity had its
  own: the elaborator no longer keys the instance by the overflow count,
  and the pack stays a run-time `VariadicPack`. Natively it is the pin's
  `VariadicList` layout, `{ elements, len }` over the caller's element
  slots, read behind a bounds trap against its run-time length; an owned
  pack and a consuming iterator's rest are destroyed last to first, on the
  VM too. The corpus has 27 fewer concrete functions. Spreading such a
  collector (R1071) and writing it (R1072) are still rejected
  (assets/ok/unused_argument_nested_variadic_instances.mojo).
- A slice descriptor's bound read names the `Optional` constructors it
  runs (R1030), as an upstream call names the constructor it runs:
  `field.get` carries them in concrete MIR (MIR text 1.69), and the VM
  calls the one the bound needs, so they are no longer recorded in
  `Optional`'s member table and leave the program with the last body
  that reads a bound. A keyword collector's constructor is still a table
  entry (R1070). No program's output changes.
- `Tuple`'s `nodebug` `__len__` and `__getitem_param__` are inlined
  (R986), as at the pin: the tuple's element types bind its one type
  pack, element by element or over the caller's own parameters, and
  `t[0]` holds the accessor's body with the index bound to its constant,
  so 4 fixtures rather than 148 keep a `__getitem_param__` instance and
  the corpus has 690 fewer concrete functions. A receiver typed by a
  computed pack list, as in `Tuple.concat`, still keeps its call (R1068).
  Natively, a reference to a `Pointer` element is no longer read through
  the pointer it borrows. No program's output changes
  (assets/ok/always_inline_nodebug_tuple_members.mojo).
- A call of a `nodebug` function with a variadic compile-time parameter
  is inlined (R946), as at the pin: `*values: Int` binds to the call's
  element list and `*Ts` to its types, so `last[4, 5]()` and
  `arity(pair, 2)` over `t: Tuple[*Ts]` hold the callee's body, and a
  pack the caller forwards binds to the caller's own. A generic callee
  holding a `comptime for` still keeps its call (R1067). No program's
  output changes.
- A call of a `nodebug` function or method returning a reference is
  inlined (R953), as at the pin: the reference the body makes is to the
  caller's place the call passes, a `read` argument's included, so
  `pick(y) += 1` over `pick(ref x) -> ref [x] Int` and `b.val() += 1`
  over a `ref self` getter hold the callee's body. No program's output
  changes (assets/ok/always_inline_nodebug_reference_results.mojo).
- A parameter every self-recursive call passes as itself, and that the
  body reads nowhere else, leaves the instance key (R1028), as upstream's
  `RemoveUnusedParams` discounts that forward: `rec[n](k)` calling
  `leaf[n]()` and `rec[n](k - 1)` has one instance for every `n`, by a
  call or a method call. A static method's recursion through its spelled
  receiver still keeps the parameter (R1065). No program's output
  changes.
- A read out of storage (a load, a field, an element, a variant payload)
  and a store copy nothing in the native backend either (R1010), as on
  the pin and the VM: it copies only where the MIR names a copy, so a
  struct whose values are only read keeps no copy constructor (corpus
  concrete functions 28,205 → 27,318). No program's output changes.
  Constructing a `Variant` still copies its payload (R1064).
- The VM runs an operator, protocol method, trait requirement, or other
  member it reaches from a value's type through the struct's member
  table (R1027), as upstream reaches it through a conformance's witness
  table, and never composes `{struct}.{method}` from the receiver's
  type. No program's output changes.
- A positional-only parameter's evaluated default no longer reads the
  free function's parameters when every earlier omitted default is
  filled at the call site too (R1025), as on the pin:
  `ponly[n, m](x: Int, y: Int = m * 2, /)` has one instance per `n`. One
  after an omitted literal default (R1061) or an unused one (R1062), and a
  bundled function's (R1063), still read them.
- A call of a free function's value (`var g = use; g()`) evaluates an
  omitted evaluated default at the call site (R1024), as upstream does: a
  borrowed default with a destructor is now destroyed when the call
  returns, as on the pin. The callee-side evaluation stays for the call
  shapes that still leave a default to the callee (R1060).
- A call of a nested `def` from the body declaring it, and the program's
  call of a function or method the bundled package compiled, evaluate an
  omitted evaluated default at the call site (R1023), as upstream does:
  a nested `def`'s borrowed default with a destructor is now destroyed
  when the call returns, as on the pin, and a bundled callee's default is
  skipped for an argument its finished body removed.
- A method call, a static-method call, and a construction evaluate an
  omitted evaluated default at the call site (R1022), as upstream does,
  binding the struct's parameters from the receiver's type and the
  method's own from the call: a borrowed default with a destructor is now
  destroyed when a method call returns, as on the pin. A function lifted
  from a constructor (`$default$T.__init__$x`) is no longer mistaken for
  the constructor itself.
- A construction names the constructor the check selected (R1020), as
  upstream's call names the selected `__init__` generator with the
  struct's parameters as its own: a struct's lone `__init__`, an
  `@implicit` conversion's, which now passes the struct's arguments, and
  an explicit copy constructor, which `T(copy=x)` calls with the spelled
  `T[n]` as its receiver. A construction now drops the struct parameters
  its constructor removed, so `mk[n]()` returning `Box[n](5).v` has one
  instance, as on the pin; `inline-parametric` inlines a named or
  overloaded `@always_inline` constructor; and an `@implicit` conversion
  through an overloaded constructor inside a generic body, which failed
  the verifier, runs.
- A copying or moving `var.use` and a `value.copy` name the constructor
  they run (R1017), as upstream's copy/move insertion leaves a call naming
  the `__copyinit__` or `__moveinit__` at each site: MIR lowering, copy
  elision, and `inline-parametric` name the template member, the elaborator
  respells it to the instance, the symbol walk, `eliminate-dead-symbols`,
  and `remove-unused-params` follow it, and the VM and Pliron call it
  rather than finding it in the struct's member table. The verifier
  reports a copy or move naming another symbol than its struct's
  constructor. MIR text 1.68 adds the `constructor` field. Captures,
  variant payloads, and a constructor-less struct's fields still find
  theirs by type; no program's output or instance count changes.
- A `drop.var` or `drop.place` names the destructor it runs (R1016), as
  upstream's destructor insertion leaves a call naming the `__del__` at
  each death: drop elaboration names the template `__deinit__`, the
  elaborator respells it to the instance, the symbol walk,
  `eliminate-dead-symbols`, and `remove-unused-params` follow it, and the
  VM and Pliron call it rather than finding it in the struct's member
  table. The verifier reports a drop naming another symbol than its
  struct's destructor. MIR text 1.67 adds the drop's `destructor`. Cleanup
  lists and a destructor-less struct's fields still find theirs by type;
  no program's output or instance count changes.
- A nested `def` and a function with a borrowed `*args` lose an argument
  their body never uses (R1014), as upstream's `RemoveUnusedParams`
  treats them like any generator: a nested body's capture is an argument
  like any other, each call of a nested body names its reduced generator
  (`outer$cap$REMOVED$ARG`), and only a nested body some use other than a
  call reaches, such as a function value, keeps its full generator beside
  it. `fv[n](k, *args: Arr[n])` returning `k` has one instance for every
  `n`, while an owned `var *args`, whose drop runs code, stays. Corpus
  concrete instructions fall by 393; no program's output changes.
- A runtime argument a body only passes on to a callee's removed argument
  is unused too (R1013), as upstream's `RemoveUnusedParams` walks callees
  first and rewrites each call to the clone without the argument:
  `fwd[n](a: Arr[n])` returning `fa(a) + 1` has one instance,
  `fwd$REMOVED$ARG$mono`, for every `n`, as do forwards by keyword, into
  an owned argument, and through a removed receiver, and
  `inline-parametric` inlines a callee's generator without its removed
  arguments. Copying a struct into an owned argument stays a use, as on
  the pin. MIR text 1.66 adds `callee_removed_args`. Corpus concrete
  instructions 386,911 → 386,767; no program's output changes.
- A method whose body never uses its receiver leaves the generator a
  resolved method call names without it (R1012), as upstream's
  `k3(q::Holder)_REMOVED_ARG()`: the call becomes a call with no
  receiver, and `Grid[n].k3(self)` returning `3` has one instance,
  `Grid.k3$REMOVED$ARG$mono`, for every `n`. Corpus concrete
  instructions 387,807 → 386,911; no program's output changes.
- A method's runtime argument its body never uses leaves the generator a
  resolved method call names (R1011), as upstream's `RemoveUnusedParams`
  clones every generator without it: `Holder.m[n](self, a: Arr[n])`
  returning `self.k` has one instance, `Holder.m$REMOVED$ARG$mono`, for
  every `n`, and the call passes no `a`. A static method (R1032), a dunder
  or member-table member (R1033), and the receiver (R1012) still keep
  theirs. Corpus concrete instructions 389,429 → 387,662; no program's
  output changes.
- A callable struct's `__call__` joins the concrete program only from a
  call that reaches it (R1009), as upstream instantiates only what a call
  names, instead of with every struct instance a body names. The checker
  now rejects such a struct where a `def(...)` value is expected, as the
  pin does ("cannot be converted from 'Scale' to 'def(Int) -> Int'"), so
  no call reaches `__call__` through a contract's dispatch symbol.
- `eliminate-dead-symbols` over the concrete program drops a constructor or
  an overload nothing names (R1008), as upstream removes any symbol nothing
  references; only the pre-elaboration runs, whose generic bodies the
  elaborator resolves by overload family and by type, still keep every
  constructor of a live struct and every member of a live family. Corpus
  concrete functions 27,962 → 27,853; no program's output changes.
- Every construction a compiled `__init__` serves names its constructor
  instance in the concrete program (R1007), as upstream's call names the
  constructor's symbol: `make[String]()`'s `T()` calls
  `String.__init__$ov$` instead of a call naming `String` that the VM and
  native lowering each resolved to an overload by arity. A fieldwise
  struct with no `__init__` and `T(copy=x)` keep the struct's name.
- `s & t` over two `Set[Int]` values, and
  `benchmarks/compile/collections_many.mojo`, elaborate again (R1026):
  R995's erasure of a removed parameter's valueless argument fixed the
  `Set.intersection` instance R999 broke, and the benchmark's
  `benchmarks/instantiation.tsv` row matches its counters unchanged.
- A call of a generator that removed a parameter no longer evaluates that
  parameter's argument (R995), as upstream's `RemoveUnusedParams` rewrote
  the call: the elaborator runs each instance over
  `mir::with_removed_forwards_erased`, which passes no operand and erases
  the operations that only computed one, and erases (`_REMOVED_ARG`) a
  spelled argument that has no value. `div_fwd[0]()` returning
  `leaf[10 // n]()` prints `1` instead of failing elaboration, whichever
  `div_fwd` is demanded first
  (`assets/ok/forwarded_parameter_instances.mojo`), and `s & t` over two
  `Set[Int]` values elaborates again.
- `eliminate-duplicate-functions` merges a struct's special methods and
  trait requirements too (R1006). The elaborator records every instance
  of a member the struct's type reaches in its member table, as upstream's
  conformance witness tables name them. The pass points an entry naming a
  merged member at its survivor instead of keeping the member under its
  own name, and the VM reads the table where a composed name defines
  nothing. `Sparrow.fly` and `Sparrow.eat` of
  `assets/ok/trait_inheritance.mojo` are now one function.
- A parameter only a free function's evaluated default reads is removed
  (R1005), as on the pin: each direct call omitting the default evaluates
  it at the call site, so the callee's signature naming it reads nothing
  (`mir::evaluated_at_call_site`). `dflt[n, m](x: Int = n * 2)` returning
  `x` is one instance, and `scaled[n, m](x: Int = m * 2)` one per `n`
  (`assets/ok/lifted_body_owner_instances.mojo`). A method's, a nested
  `def`'s, and a bundled function's default still read through the callee
  (R1022, R1023).
- An omitted evaluated default of a top-level free function is evaluated
  at the call site (R1004), as upstream does: `mir::evaluate_omitted_defaults`
  gives a direct call that leaves the slot out a call of the lifted
  `$default$f$x` under the call's own parameter arguments, before
  ownership analysis, so a borrowed default is the caller's temporary and
  is destroyed when the call returns (`use()` over
  `r: R = R(String("dflt"))` now prints `drop dflt`, as on the pin), and
  the default's instance is named by the call. The default of an argument
  the callee never uses is still not evaluated. A method's, a nested
  `def`'s, and a bundled function's default stay with the callee
  (R1022, R1023).
- A drop, copy, or move reads only the struct arguments its lifecycle
  member kept (R999), as on the pin: `fd[n](var b: DBox[n])`, whose
  `DBox.__deinit__` prints only `self.v`, is one instance for every `n`,
  as are a copy through a declared `__init__(copy=)`, a move through
  `__init__(deinit move=)`, a type parameter, and the same through a
  synthesized member, while a destructor reading `Self.n` keeps one per
  `n`. `remove-unused-params` orders each declared lifecycle member a body
  runs before the body and reads it through that member's removals; the
  elaborator demands the copy or move constructor of an erased instance
  when that member removed every erased parameter, and the VM copies and
  moves an erased slot's value through the slot's type
  (`assets/ok/lifecycle_member_unread_struct_parameter_instances.mojo`).
- A call of a struct's member reads only the struct arguments the member
  kept (R998), as on the pin: `fg[n](b: NBox[n])` returning `b.get()` is
  one instance for every `n`, as are a call on `self`, a subscript, an
  operator, a prefix `~`, a generic method, a `for` loop over the struct,
  and a method returning `self.copy()` over an erased type argument.
  `remove-unused-params` follows resolved operators and iteration steps
  callee first, reads each such call through the member's removals, and
  records the member in the caller's snapshot for the verifier; the
  elaborator unifies an erased type argument in a call's result. A prefix
  `-` or `~` on a struct now lowers to the call of the dunder the check
  selected, which fixes an elaboration failure when `remove-unused-params`
  removed a parameter only that dunder read
  (`assets/ok/member_call_unread_struct_parameter_instances.mojo`).
- A method that calls a method on a field reads only the root struct's
  layout (R1002), as on the pin: `SBox[1]` and `SBox[2]` share one
  `take(var self)` returning `self.s.byte_length()`. `remove-unused-params`
  reads a method call's receiver and argument places by the type each
  designates, whose member runs, not by the root and projection types
  that only address it.
- A constructor that reads no struct parameter is one instance for every
  instance of its struct (R1000), as on the pin: `Box[1](1)` and
  `Box[2](2)` share `Box.__init__$mono`. The elaborator keys such a
  constructor off its struct template, like every other method that
  removed a struct parameter, and respells each construction reaching it
  (a direct one, an `@implicit` conversion, `T()` through `Defaultable`)
  to name it; the VM builds a constructor call's destination type, a
  converting default's parameter type, rather than the struct its symbol
  names. A keyword collector's and a slice bound's constructors, which no
  call names, join their struct's member table
  (`mir::CONSTRUCTED_MEMBERS`), and `mir::composed_member` is gone
  (`assets/ok/constructor_reads_no_struct_parameter.mojo`).
- A display, repr, hash, equality, `__int__`, writer `write_string`, or
  `__call__` member that reads no struct parameter is one instance for
  every instance of its struct (R1003), as on the pin: `WBox[1]` and
  `WBox[2]` share `WBox.write_to$mono`. A concrete struct's member table
  now names every instance of such a member, keyed by its method
  spelling, and every backend site that composed `{struct}.{member}` from
  a value's type reads the table instead: the VM's display, `__int__`
  lane, writer, hash-leaf, keyword-collector, `!=`, and callable-value
  dispatch, and Pliron's display, repr, writer, hash-leaf, `__int__`,
  collector, and reachability walk. `eliminate-dead-symbols` keeps the
  dispatched members a live struct's table names. Only constructors still
  keep one instance per struct instance (R1000)
  (`assets/ok/struct_member_unread_struct_parameter_instances.mojo`).
- A destructor, copy, or move constructor that reads no struct parameter
  is one instance for every instance of its struct (R1001), as on the
  pin: `DBox[1]` and `DBox[2]` share `DBox.__deinit__$mono`. Each struct
  declaration carries a member table naming its lifecycle members'
  symbols (`StructDeclaration::members`, MIR text 1.65), which MIR
  lowering fills for a template and the elaborator for each concrete
  struct with the instances it made, as upstream's struct nests its
  member generators, and every drop, copy, and move reads it: the VM, the
  Pliron backend, `eliminate-dead-symbols`,
  `eliminate-duplicate-functions`, `inline-parametric`, and the
  trivial-copy and trivial-drop queries. No program's output changes.
- A runtime argument a free function's body never uses leaves the
  generator its direct calls name (R997), as upstream's
  `RemoveUnusedParams` removes it first and then the parameters only its
  type named: `remove-unused-params` records it as
  `MirFunction::removed_args` (MIR text 1.64), a drop that runs no code
  counting as no use, and the elaborator instantiates the reduced
  generator (`fa$REMOVED$ARG$mono`, upstream's `_REMOVED_ARG` clone) for
  each direct call, which passes no argument for it, and the full one for
  a function value. So `fa[n](a: Arr[n])` and `fav[n](var a: Arr[n])` have
  one instance for every `n`, as on the pin
  (`assets/ok/unused_argument_instances.mojo`), and a call omitting such
  an argument no longer evaluates its default, so `fside()` whose default
  prints now runs, printing nothing for it, as on the pin. No corpus
  fixture's output or instance count changes.
- A struct's copy and move constructors are instantiated only where a
  body copies or moves a value of the struct (R994), as upstream's copy and
  move insertion calls them at each site, rather than with every struct a
  body names: the elaborator demands `__copyinit__` and `__moveinit__` from
  each copy, move, owned closure capture, variant payload, reference-read
  method result, and clone-out read, and the fields they reach, and
  `eliminate-dead-symbols` keeps them on the same terms. So
  `assets/ok/mut_owned_params.mojo`, which only forwards its `String`,
  keeps neither of `String`'s. Corpus concrete functions 29,001 → 28,325,
  concrete instructions 392,176 → 387,437 (`benchmarks/instantiation.tsv`).
  No program's output changes.
- A struct's constructor is instantiated only where a call reaches it
  (R993), as upstream instantiates only what a call reaches, rather than
  every `__init__` overload of each struct a body names: a call naming it,
  a call naming the struct (`T()` at `T = String`, an instance-named
  `AHasher$mono$…()`), which the elaborator resolves by arity, or a slice
  descriptor's bound read, which builds its `Optional`. So
  `assets/ok/mut_owned_params.mojo` keeps only `String`'s `StringLiteral`
  constructor, and an uncalled overload that fails at the instance no
  longer fails the program
  (`conformance/probes/constructor_overload_uncalled_instance.mojo` prints
  `8`, as on the pin). `print`'s `sep` and `end` keywords now demand their
  display instances themselves. Corpus concrete functions −27.6 %,
  concrete instructions −35.7 % (`benchmarks/instantiation.tsv`).
- A struct's method no longer stays apart from an identical instance
  merely for being a method (R992): `eliminate-duplicate-functions` pins
  only the methods the backends reach from the struct's type, its special
  methods and its traits' requirements (`MirDeclarations::reached_from_type`),
  so `Shelf[Int]` and `Shelf[String]`'s nested defs and static methods
  merge (corpus concrete functions −23, `benchmarks/instantiation.tsv`).
  Pinned members stay apart until R1006.
- A caller no longer stays apart from an identical instance by the
  constant its call's compile-time argument was read into (R991): the
  elaborator erases that constant with the argument slot it drops, as
  upstream's call carries the parameter as an attribute. So `bar[1]` and
  `bar[2]` of `bar[n]` returning `foo[n]() + 10` merge once
  `eliminate-duplicate-functions` merges `foo[1]` and `foo[2]` (corpus
  concrete functions −1, `benchmarks/instantiation.tsv`).
- A parameter that only a body lifted from its function would read, were
  its other binders removed, no longer splits instances (R982):
  `remove-unused-params` reads a `$comptime$f$k` or `$default$f$x` the
  function names as reading only the binders that body kept, rather than
  every parameter, and walks such bodies first. So `lifted[n, m]` whose
  `comptime k` reads only `n` has one instance per `n`, and `Box[n].get[m]`
  whose default reads only `Self.n` one, as on the pin. A parameter only an
  evaluated default reads is still kept, since the callee, not the call
  site, evaluates it (R1004, R1005;
  `assets/ok/lifted_body_owner_instances.mojo`).
- A method no longer splits instances by struct parameters its receiver
  never reads (R978): `remove-unused-params` reads a method's `Self` as its
  struct spelled at its own parameters, so `get(self)`, `take(var self)`,
  and `bump(mut self)` over a `Box[n]` holding an `Int` have one instance
  each, taking the erased `Box[_REMOVED_ARG]`, while one reading `Self.n`
  or over `Arr[n]` holding a `SIMD[int64, n]` keeps one per `n`, as on the
  pin. A copy, move, or drop of `self` reads what its struct's members
  read, and `Self.n` read through the receiver reads `n`. Constructors,
  lifecycle, display, hash, and equality members keep one instance per
  struct instance, since the backends find them by name (R1000, R1001,
  R1003). A static call's receiver whose argument a body removed now names
  the erased instance, fixing `static_method_unread_owner_instances.mojo`
  and `static_call_through_self.mojo`, and an erased instance only a
  pointee or a struct argument names needs no layout
  (`assets/ok/receiver_unread_struct_parameter_instances.mojo`).
- A parameter that only a struct type's spelling names no longer splits
  instances where the struct's layout does not read it (R977):
  `remove-unused-params` reads a struct type's arguments as upstream's walk
  over `LowerLIT`'s structural types does, so `fb[n](b: Box[n])` over a
  `Box` holding an `Int`, one over `Holder[Box[n]]`, over a struct whose
  pointer field points to `T`, or over one holding a `List[T]` has one
  instance, while `Arr[n]` holding a `SIMD[int64, n]` keeps one per `n`.
  A struct type is still read whole where a type value, a member call, a
  constructor, a builtin, or a destructor, copy, or move the struct
  declares names it. The verifier keeps a removed parameter in scope
  inside a struct argument (R996), and an erased instance's pointer field
  points to `None` where its pointee would name an erased argument
  (`assets/ok/struct_layout_unread_parameter_instances.mojo`).
- An instance whose generator removed a parameter that a struct argument
  names can now take that struct (R976): the elaborator erases each such
  argument, as upstream's generator spells `_REMOVED_ARG`, so one `fb$mono`
  would take `Box$mono$V$REMOVED$ARG`, declared with the fields every
  `Box[n]` lays out, for every `n`. The verifier accepts any instance
  agreeing on the kept arguments for it, the VM destroys a value held in
  such a slot through its type's members, and an instance whose layout or
  member reads the erased parameter fails explicitly. MIR text is schema
  1.63 (`ct_removed_arg`) and the package format `mojito-package 66`.
  Nothing removes such a parameter until R977, so no program's output or
  instance count changes.
- A parameter forwarded through a subscript call no longer keeps its
  instances apart (R971): `remove-unused-params` drops a removed accessor
  parameter's argument from `__getitem__` and `__setitem__` calls as it
  does from calls and method calls, so `subscript_fwd[n]` returning
  `Grid(5)[n]`, where `Grid.__getitem_param__[i]` ignores `i`, has one
  instance (`assets/ok/forwarded_parameter_instances.mojo`).
- A forward to a removed parameter that a kept parameter's default reads no
  longer keeps its instances apart when the call supplies that parameter
  (R970): `remove-unused-params` keeps such an argument only where the call
  omits the kept parameter, so `dmid[x]` returning `dleaf[x, 2]()`, where
  `dleaf[a: Int, b: Int = a]` reads only `b`, has one instance
  (`assets/ok/forwarded_parameter_instances.mojo`).
- A parameter forwarded through a built-in operator that could trap no
  longer keeps its instances apart (R969): `remove-unused-params`' view of
  a caller erases such an operator when every reader of it was a dropped
  parameter argument, as the pin's parameter argument is an attribute with
  no operation to trap, so `div_fwd[n]` returning `leaf[10 // n]()` has one
  instance (`assets/ok/forwarded_parameter_instances.mojo`).

- A struct's destructor is instantiated only where a body drops a value of
  the struct (R985): the elaborator demands `__deinit__` from each drop,
  cleanup list, storage destroy, and owned capture, and the fields they
  reach, as upstream's destructor insertion calls it at each death, rather
  than with every struct a body names, and `eliminate-dead-symbols` keeps it
  on the same terms. A destructor every drop of which `inline-parametric`
  expanded, `String`'s in `assets/ok/mut_owned_params.mojo`, leaves the
  concrete program. The corpus's concrete functions drop from 41,035 to
  40,300 and its concrete instructions from 618,517 to 608,225
  (`benchmarks/instantiation.tsv`). No program's output changes.
- Identical instances now merge (R990): `eliminate-duplicate-functions`
  runs straight out of the elaborator, before `eliminate-dead-symbols`, as
  upstream's `EliminateDuplicateFunctions` does, keeping one of each set of
  functions whose bodies, declarations, and source locations match and
  pointing every reference to the others at it, so `foo[1]` and `foo[2]`
  are one function when a `comptime if` picks the same arm. The corpus's
  concrete functions drop from 41,061 to 41,035
  (`benchmarks/instantiation.tsv`). No program's output changes.
- The concrete program no longer keeps the instances only compile-time
  evaluation reaches (R989): `eliminate-dead-symbols` runs straight out of
  the elaborator and again after the post-elaboration pipeline, as
  upstream's `EliminateDeadSymbols` does, so the corpus's concrete
  functions drop from 46,388 to 41,061 and its concrete instructions from
  659,919 to 618,646 (`benchmarks/instantiation.tsv`). No program's output
  changes.
- `benchmarks/instantiation.tsv` is regenerated with the inliner acting
  on the bundled library (R939): `_is_valid_utf8`, `String`'s destructor,
  `String.unsafe_ptr` and `capacity_bytes`, `rotate_bits_left`, and
  `List`'s `__len__` and `capacity` are inlined where a call or drop
  reaches them directly, so the corpus's concrete functions are 46,388
  where they are 46,968 with `inline-parametric` off. Overloaded
  constructors, `Span` and `Tuple` members, and members reached through a
  trait-bound call still keep their calls (R951, R952, R986, R987, R988),
  and an expanded destructor is still instantiated (R985).
- A function lifted from a compile-time expression no longer splits
  instances on its owner's parameters it never reads (R911):
  `remove-unused-params` treats an evaluated default (`$default$f$x`) or
  a lifted application (`$comptime$f$k`) as a generator over every binder
  it declares, and the elaborator keys its instances, and the compile-time
  evaluations cached by instance name, on the binders it kept, so
  `scaled[1, 3]()` and `scaled[2, 3]()` share the instance of the default
  `x: Int = m * 2` (assets/ok/lifted_unread_parameter_instances.mojo). The
  owner itself still keeps a parameter only its evaluated default reads,
  where the pin removes it (R982).
- A callable compile-time parameter its body never calls no longer splits
  instances (R910): `remove-unused-params` now considers callable value
  parameters, the elaborator binds no runtime closure or environment for
  one the callee removed, and copying or dropping its slot to forward it is
  no read, so `ignore[double]` and `ignore[triple]`, a capturing or generic
  closure's binding included, share one instance, as does a function that
  only forwards the parameter to one that removed it, as on the pin
  (assets/ok/unread_callable_parameter_instances.mojo). Forwarding a
  capturing callable parameter to a callee that calls it is still rejected
  (R980), as is a callable default naming a function declared later (R981).
- A static method that never reads its struct's compile-time parameter
  no longer splits instances (R909): `remove-unused-params` treats a
  method as a generator over its struct's parameters, as upstream's
  `RemoveUnusedParams` does, and the elaborator keys and names such a
  method off the struct template (`S.st$mono`), so `S[1].st()` and
  `S[2].st()` share one instance, and a caller that only forwards `n`
  through `S[n].st()` removes it too
  (assets/ok/static_method_unread_owner_instances.mojo). `Self.n` of a
  struct's value parameter is now a parameter constant rather than a read
  of a `self` slot. A method with a receiver, and a function whose only
  read of a parameter is a by-address argument's struct type, still keep
  it where the pin removes it (R976–R978); `Self.x` of a `Float64`
  parameter reads as `Int` (R979).
- A nested `def` that never reads its enclosing function's compile-time
  parameter no longer splits instances (R908): `remove-unused-params`
  records on a nested body the enclosing parameters it never reads, the
  elaborator keys the body's instances on the rest, and the enclosing
  function reads through a nested body only those, so `outer[1]` and
  `outer[2]` share one instance of `outer` and of its `inner`
  (assets/ok/nested_unread_enclosing_instances.mojo). A nested generic
  `def`'s own parameters, a `def` in a `comptime for`, and one nested in
  a parametric struct's method still split instances (R973–R975).
- A compile-time parameter only forwarded to a parameter its callee never
  reads no longer splits instances (R907): `remove-unused-params` walks
  the call graph callees first, as upstream's `RemoveUnusedParams` does,
  and reads each body as if its calls dropped the arguments of the
  callee's removed parameters, so `mid[n]` calling `leaf[n]()` has one
  instance (assets/ok/forwarded_parameter_instances.mojo). A forward
  around a recursive call still counts, as on the pin, and so does one
  into the package, whose generators upstream treats as external; a
  forward through a trapping operator, to a removed parameter a kept
  default reads, or through a subscript call still counts where the pin
  removes it (R969–R971). MIR text is now schema 1.62, which adds a
  body's `callee_removed_params`.
- A drop of a value whose `__deinit__` is `nodebug`, `String`'s
  included, is inlined before elaboration (R922): `inline-parametric`
  expands the drop into the destructor's body where the value dies,
  binding its `deinit self` to the dropped variable and the struct's
  parameters to the variable type's
  (assets/ok/always_inline_nodebug_destructor.mojo). A variable a path may
  have moved from keeps the destructor call; a drop inside a region of a
  variable stored before it, and a struct with no destructor of its own,
  keep it too (R967, R968).
- A call to a nested `nodebug` `def` is inlined before elaboration
  (R921): `inline-parametric` follows the call through the closure its
  caller makes, binds each capture by reference to the captured place,
  and reads an enclosing declaration's value parameter as that binder
  (assets/ok/always_inline_nodebug_nested_calls.mojo). A copied or moved
  capture, a closure the caller does not make, a nested `def` in a
  `comptime for`, and a capturing closure passed as a callable binder
  keep the call (R962–R965); the inlined closure is still constructed
  (R966).
- A `nodebug` call passing keywords or omitting a defaulted argument is
  inlined before elaboration (R920): `inline-parametric` binds the call's
  arguments through the shared call matcher, and materializes each
  omitted default at the call, a literal or a converting constructor run
  on one, dropping it after the body when the parameter only borrows it
  (assets/ok/always_inline_nodebug_calls.mojo). A trait default's
  wrapper called by keyword now inlines too. Variadic collectors, spreads,
  keyword collectors, computed defaults, and literal defaults at a SIMD
  vector or a constructor's binder keep the call (R957–R961).
- A raising `nodebug` callee is inlined before elaboration (R919): its
  raises propagate as the call's, and inlined into a `try`'s body it adds
  the callee's slots a raise may hold to that `try`'s raise-edge cleanup,
  so a callee's local is destroyed on the raise edge as at the pin
  (assets/ok/always_inline_nodebug_calls.mojo). Erasing unreachable blocks
  now cuts a `try`'s cleanup list to what its remaining raise points hold,
  which a `try` holding a raise in a constant-false branch failed the MIR
  verifier for.
- A `nodebug` call through a `mut` or `ref` parameter, or a `mut self`
  receiver, is inlined before elaboration (R918): `inline-parametric`
  binds the parameter to the caller's place the call passes, as the VM
  binds it to a reference
  (assets/ok/always_inline_nodebug_calls.mojo). A callee returning a
  reference, a `deinit` parameter, and a borrowed argument with no place
  to bind keep the call (R953–R955). Program output is unchanged.
- A `nodebug` method of a parametric struct, and a `nodebug` constructor,
  are inlined before elaboration (R917): `inline-parametric` binds the
  struct's parameters to the receiver's or the constructed type's
  arguments, folds `Self.k` to its constant, and initializes the
  constructed value in a fresh `out self` slot
  (assets/ok/always_inline_nodebug_struct_calls.mojo). Composing a
  place-bound parameter with an indexed caller place no longer misnumbers
  the caller's index register. A struct with several `__init__`s, and one
  with an origin parameter such as `Span`, keep the call (R951, R952).
  Program output is unchanged.
- A generic `nodebug` callee is inlined before elaboration (R916):
  `inline-parametric` binds a callee's own compile-time parameters, and a
  non-parametric struct method's, to the call's arguments through
  `mir::visit`, so such a callee is no longer instantiated per binding
  (assets/ok/always_inline_nodebug_generic_calls.mojo). An argument over
  the caller's parameters stays symbolic for the elaborator. A callable or
  variadic binder, a `where` clause, a `T()` at a concrete binding, and a
  `conforms_to` test of one keep the call (R945–R949). Program output is
  unchanged; the corpus loses 19 instances.
- MIR has a type and parameter visitor below the elaborator (R915):
  `mir::visit` walks every type and compile-time parameter site of a body,
  regions and terminators included, and the elaborator substitutes an
  instance's bindings through it, with unchanged output. A
  multi-dimensional subscript's keyword places are now substituted as
  well. The verifier's own type walk remains narrower (R944).
- A trait method may carry inline-level decorators (R914): `@always_inline`,
  `@no_inline`, and `@inline(level)` on a requirement or a default parse
  and are validated as a function's are, and a default's member function
  (`Trait.m`) records the level in its MIR declaration. Any other
  decorator on a trait method is rejected as unsupported (R942, R943).
- `@inline(level)` now sets a function's inline level (R913): `.always`,
  `.nodebug`, `.never`, and `.automatic`, or the same members spelled
  `InlineLevel.member`, reach its MIR declaration and the package as
  `@always_inline` and `@no_inline` do, so the bundled library's
  `@inline(.nodebug)` callees reach `inline-parametric`. A missing, extra,
  or keyword operand and an unknown member are rejected with the pin's
  messages; an operand that is any other compile-time expression is
  rejected as unsupported (R940, R941). Program output is unchanged.
- The bundled library carries the pinned std's inline markings (R786):
  each function it shares with upstream takes upstream's
  `@inline(.always)`, `@inline(.nodebug)`, `@inline(.never)`, or
  `@always_inline("builtin")`, spelled as at the pin. No program's output
  or instantiation counters change, since none of the marked callees is
  one `inline-parametric` inlines yet (R939).
- `eliminate-dead-symbols` removes every program function and struct no
  root reaches, with its declaration (R834), as upstream's
  `EliminateDeadSymbols` removes every unreachable non-`export` symbol. Its
  roots are `main`, `__toplevel__`, and the functions a compilation
  exports by name with the new `Compiler::with_exports`, which a caller
  compiling a native entry such as `compute` from `drop_elaborated_mir`
  must now pass. An instruction that calls a bundled body unnamed (`**`
  on integers, a scalar display) keeps it live, by
  `mir::implicit_callees`, which the elaborator now shares.
  `ProgramPass::run` and `pre_elaborate_program` take the exports. Program
  output is unchanged.
- `eliminate-dead-symbols` follows typed symbol references (R835): every
  MIR, checked, and type field that names a function or struct encodes its
  spelling as a `SymbolRef` (`mojito_types::symbol_ref`, MLIR's
  `SymbolRefAttr`), and `mir::SymbolTable` looks up only those, so a local,
  field, keyword, literal, or parameter binder spelling a function no
  longer keeps it live. In source mode 14 more of std's bodies go and the
  pass is about 15 % cheaper. Program output is unchanged; the MIR text and
  package formats are unchanged.
- Adds `lower-lit` (R837), the part of upstream's `LowerLIT` that MIR has:
  right after drop elaboration, at the program and package seams and with
  the pre-elaboration pipeline on or off, it erases every
  `lifetime.keep_alive` and `interiors.invalidate`, which only the
  ownership analysis and drop elaboration read. No generator or instance
  carries them; concrete verification, the VM, and the Pliron backend
  refuse one, so an artifact written by an older binary that still holds
  them no longer runs. `var.declare` stays, as upstream lowers
  `lit.var.decl` to an allocation. `loans.establish` (R931) and the
  ownership marks (R932) still reach the backends. Corpus concrete
  instructions fall 8.9 %. Program output is unchanged; the MIR text and
  package formats are unchanged.
- Adds MLIR's `CSE` as the `cse` pass (R845), last in the post-elaboration
  pipeline, as upstream's `'kgen.func'` pipeline ends with `CSEPass`. It
  walks each block list's dominator tree and erases an instruction free of
  effects that a dominating one matches, when its result is held whole in
  one register. Reads of memory (R929) and values held in storage (R930)
  are not merged yet. The Pliron backend now lowers each block list in
  reverse postorder, so a reused definition is lowered before its reads.
  Corpus concrete instructions fall 3.3 %. Program output is unchanged; the
  MIR text and package formats are unchanged.
- Adds upstream's `Mem2Reg` as the `mem-2-reg` pass (R787), before each of
  the first three pre-elaboration rounds' `canonicalize` and after
  elaboration, where `lower-arg-conventions`, the callee half of upstream's
  `LowerArgConventions`, first stores each `read` or owned register-passable
  argument into a local. A slot of a concrete trivially register-passable
  type that is only stored, read whole, or marked, each of whose reads sees
  one reaching store, becomes registers. A slot whose reads merge two
  stores keeps its slot (R927), and `SROA` is not yet added (R928). The
  slot-renumbering helpers move from `native::mono` to `mojito-mir`.
  Corpus concrete instructions fall 9.1 %. Program output is unchanged; the
  MIR text and package formats are unchanged.
- A trait default's synthesized forwarding wrapper is now `nodebug`, as
  upstream's `synthesizeDefaultTraitMethodWrapper` marks it (R785), so
  `inline-parametric` replaces a statically resolved call to an inherited
  default with the call of its trait's member, and a wrapper reached only
  by such calls leaves the concrete program. A call by trait dispatch keeps
  the wrapper, as on the pin. Program output is unchanged; the MIR text and
  package formats are unchanged.
- The pre-elaboration pipeline now runs the pin's four rounds in order
  (R869): `remove-unused-params` and `eliminate-dead-symbols` in each, the
  first ending with `canonicalize`, the second opening with
  `inline-parametric` and ending with `sccp` and `canonicalize`, and the
  third running `inline-parametric` and `canonicalize` again. The fourth
  round's generator CSE waits on a CSE pass (R926). Program output is
  unchanged.
- The pre-elaboration pipeline's second round now opens with
  `inline-parametric`, upstream's `InlineParametric` (R784): a call to an
  `@always_inline("nodebug")` or `@always_inline("builtin")` function that
  names none of its own compile-time parameters, called positionally into
  `read` or owned parameters, non-raising and value-returning, is replaced
  by the callee's finished body, its registers at the call's location, at
  both the program seam and the package seam. A callee on a cycle of such
  calls stays a call. Program output is unchanged; the MIR text and package
  formats are unchanged.
- A function's MIR declaration now records the inline level its decorator
  names, as upstream's function op does (R783): `@always_inline` is
  `always`, `@always_inline("nodebug")` `nodebug`, `@always_inline("builtin")`
  `builtin`, and `@no_inline` `never`. The checker rejects an operand the
  pin rejects and two decorators naming different levels, with the pin's
  messages. No pass acts on the level yet, so program output is unchanged.
  MIR text is 1.61, which writes `inline` on a declaration other than
  `automatic`, and the package format is 65.
- The pre-elaboration pipeline now ends with `remove-unused-params`,
  upstream's `RemoveUnusedParams` (R782): a generic function's own
  compile-time parameters that neither its body nor its signature reads
  leave the instance key and symbol, so `f[n]` whose body ignores `n` has
  one instance, `f$mono`, for every `n`. A read that only an `if False`
  arm or code after a `return` held, or only another parameter's default,
  keeps no parameter. Program output is unchanged. MIR text is 1.60, which
  writes a body's `removed_params`; the package format is unchanged.
- Each pre- and post-elaboration body pass now reports whether it changed
  the body, as an MLIR pass reports what it preserved, and the
  post-elaboration re-admission re-verifies only the changed concrete
  functions in a release build (R844). On `collections_many` the
  `post_elaboration` span falls from about 31.6 ms to 16.6 ms. Program
  output is unchanged.
- The statements after a `return`, `break`, `continue`, or `raise` are now
  erased before HIR lowers them, as upstream's `LowerSemanticCF` erases the
  ops after a terminator (R889), so dead code interns no variable slot,
  builds no block, and compiles no nested `def` or lambda. Program output
  is unchanged.
- A `try`'s parts are now regions of the function's own HIR walk, as
  upstream's parser builds `lit.try`, instead of separately built CFGs, and
  each completes by `try.yield` (`lit.try.yield`, the renamed `falloff`)
  (R874). A region op after a `return`, `break`, or `continue` is erased,
  as `LowerSemanticCF` erases dead code, so every structured region is one
  block, and MIR lowering now verifies that shape on every body before
  `lower-control-flow`. Program output is unchanged. MIR text is 1.59 and
  the package format is 64.
- A `comptime for` body now opens with upstream's `comptime if` on the
  iterator's `has_next`, whose else arm leaves the loop, as
  `LowerSemanticCF` leaves `hlcf.comptime.for` (R885), and the elaborator
  unrolls it N+1 times as upstream's does, the exhausted iterator's copy
  keeping only that stop arm. Program output is unchanged. MIR text 1.58
  records the body's test, and the package format is 63.
- The built-in `comptime for` iteration rule is gone (R796): a loop
  header is upstream's iterator protocol alone, so `ComptimeSequence` is
  the iterator's four applications, the header and HIR keep no slot for
  the loop variable (an alias, as upstream's is), and the compile-time
  `Tuple` rejection is the `__iter__` rule's own. Diagnostics and program
  output are unchanged. MIR text 1.57 withdraws `comptime_for.elements` and
  the header's `slot` field, and the package format is 62.
- A `comptime for` over a value pack (`*vals: Int`, a struct's
  `Self.values`) or a reflected field-name list (`reflect[T].field_names()`,
  or a `comptime` binding of one) now steps its `__iter__()` iterator by
  upstream's protocol (R795), so every `comptime for` takes that path and
  the checker's built-in rule and `ComptimeSource` are gone. A pack of
  `Copyable` structs that are not `ImplicitlyCopyable` is now rejected, and
  a loop over a runtime variadic is rejected at the check as a dynamic
  iterator expression, both as on the pin. The package format is 61. Loops
  over a type pack or a reflected field-type list still fail with Mojito's
  own wording (R904).
- A `comptime for` over a list, set, or dictionary display, a local
  display binding, a module constant, or a compile-time application such
  as `mk(n)` now iterates the collection's own `__iter__()` by upstream's
  iterator protocol (R794), and the built-in collection rule
  (`ComptimeSource::{Evaluated, Bound, Applied}`) is gone. A display of
  `SIMD` elements now runs, and a display of non-`Copyable` elements is
  rejected as on the pin, though with Mojito's wording (R903). The package
  format is 60. Only value packs and reflected field-name lists keep the
  built-in rule (R795).
- A compile-time list read at an `Array` is one frozen value (R789):
  `materialize[L]()` over `comptime L = [M(1), M(2)]` materializes the
  `Array` the display's CTFE built, as the pin's `kgen.param.constant`
  does, and no longer runs `Array`'s list-literal initializer, and each
  element's move initializer, at every read
  (`assets/ok/comptime_array_materialized_frozen.mojo`). The dead
  slot-read construction (`construct_parameter_reads`) is gone; a reflected
  field-name list is still constructed at each read (R900).
- A `comptime for` over a compile-time iterator that borrows its
  collection now runs (R793): `d.keys()`, `d.items()`, `d.values()`, a
  list's or a set's `__iter__()`, and a user iterator holding a `Pointer`,
  over a local `comptime` binding, a module constant, an inline display, or
  a temporary. A lifted evaluation stores each value it borrows in VM
  compile-time memory, which is never destroyed, through the new
  `store_to_mem` instruction (upstream's `#lit.store_to_mem`), and the
  iterator freezes its reference as `CtValue::StoreToMem`. MIR text is
  1.56 and the package format is 59. `materialize[it]()` of such an
  iterator is still rejected (R898), and the reference is not yet typed
  `ComptimeOrigin` (R899).
- A `comptime for` over `range(...)` iterates by upstream's iterator
  protocol (R791): the check spells `range(...).__iter__()`, and the
  elaborator steps the bundled range struct through the `paramfor_*` stubs,
  as it steps every other iterator. `ComptimeSequence::Range` and
  `ComptimeSource::EvaluatedRange` are gone, MIR text 1.55 withdraws the
  `comptime_for` range form and adds the iterator form's `next_value`, and
  the package format is 58. Along the way an iterator that is a generic
  struct instance now works as a parameter argument (`show[x]()`), a
  collector's `args.__len__()` bounds an iterator loop at compile time, and
  `Ts[k]()` over a local `comptime k = i` selects its element. Diagnostics
  that named `Ts[i]` now spell the stub application, as the pin does.
  Program output is unchanged; a debug sweep of the range-loop fixtures is
  about 1.45 times slower (R896, R897).
- A conditional expression whose condition is a parameter value (R887) —
  `1 if flag else 2` over a `Bool` parameter, `n == 0`, `comptime(e)`,
  `conforms_to(T, Copyable)` — lowers as upstream's `hlcf.comptime.if`, so
  each instance keeps only its live arm and a recursive call in the dead
  arm is never elaborated; `conforms_to` is now accepted there, and the
  then arm sees the conformances it proves. `and`/`or` and ordinary calls
  stay run-time conditions, as the pin has them. A struct's `Bool` value
  parameter read as `Self.flag` now types as `Bool` rather than `Int`,
  which fixes `comptime if Self.flag`.
- HIR builds `comptime if`/`elif`/`else` as one `hlcf.comptime.if` op
  (R873): every condition first, then one arm region per condition and an
  else region, each ended by `comptime.yield`, and MIR lowering builds the
  matching `ComptimeIf` op, which `lower-control-flow` lowers to today's
  branch chain. HIR has no jump terminator left. No program's output
  changes.
- HIR builds run-time statement control flow as upstream's `hlcf` region
  ops (R872): `if`/`elif`/`else` is an `if` with elif condition regions,
  `while` and `for` a uniquely labelled `loop` whose body opens with an
  `if` on the condition or has-next flag, its else arm holding the loop's
  `else` and a `break`, and `comptime for` a `comptime.for` op;
  `break`/`continue` name their loop, so one leaving a `try` part needs no
  outer block map, and `lower-control-flow` makes its escape. Comprehension
  loops are labelled too. No program's output changes.
- MIR lowering builds `and`/`or`, conditional expressions, comparison
  chains, and comprehensions as upstream's `hlcf.if`/`loop` region ops
  (R871), each region one block, and ends every body with
  `lower-control-flow`. A conditional expression yields a result the
  checker proved register-passable and stores any other into a result
  slot, as upstream does; a comparison chain nests one `if` per link.
  Package format 56. No program's output changes.
- MIR has upstream's structured `hlcf` region ops beside `try` (R870):
  `If`, `Loop`, `ComptimeIf`, and `ComptimeFor`, with their region
  terminators, MIR text 1.54 (package format 55), verifier rules for each
  terminator's placement plus a structured mode, and the transitional
  `lower-control-flow` pass that lowers them to blocks. Nothing produces
  them yet, so no program's output changes.
- The plan for structured control flow above the MIR waist is recorded in
  `docs/structured-control-flow-plan.md` (R859): `hlcf`-shaped region ops
  replace the HIR CFG, and one `lower-control-flow` pass moves down to the
  native backend, staged as R870–R884. `docs/pliron-future.md` no longer
  names `kgen.param.if`/`for`, which upstream does not have: its
  compile-time control flow is `hlcf.comptime.if`/`for`.
- `canonicalize` now folds an instruction whose operands are each defined
  once by a constant (R842), as MLIR's greedy driver applies an op's `fold`,
  so the first `canonicalize` folds constant operators before `sccp` runs,
  through one replacement routine both passes share. Concrete MIR is
  unchanged.
- An instance's type bindings now carry no origin (R866), as upstream binds
  a type parameter to a type whose origins read `origin={ }`, so an instance
  demanded by callers differing only in origins no longer keeps whichever
  caller's binders or places came first, and its concrete MIR no longer
  varies from run to run. Its own origin parameters stay symbolic, as
  upstream's do. No program's output changes.

- An instance now keeps its substituted generic register and slot types
  (R865), as upstream's elaborator does, instead of re-deriving them from
  one another after substitution, and the concrete MIR verifier rejects a
  reference slot read as its handle. No program's output changes.

- A generic `value.copy()` whose receiver is read through a `ref` binding
  to a built-in (`for value in values: value.copy()` over a variadic
  collector) is now a value copy in its instance too (R864): the instance
  types a whole-root load of a reference root by its referent, as lowering
  and both backends read it. The Pliron and VM fallbacks that answered the
  mistyped `copy` call are gone, with no output change.

- A generic `x.copy()` instantiated at a built-in receiver is now a value
  copy in that instance (R839), as upstream's `Copyable.copy` body
  `Self(copy=self)` is, so the post-elaboration `canonicalize` forwards it
  and neither backend answers a scalar `copy` method any more.
  `collections_many`'s concrete instructions fall by 103, with no output
  change.

- `canonicalize` now forwards a copy of a vector SIMD, a `StringLiteral`,
  or a struct that declares `TrivialRegisterPassable` (R838) when the copy
  is its source's sole reader, as upstream's parser loads such a value into
  an SSA value; Pliron no longer aliases a load of such a type to its
  place. A tuple copy stays, as upstream's `Tuple.__init__(copy=)` does.
  Corpus concrete instructions fall by 26, with no output change.

- `canonicalize` now threads each edge into an empty block that only
  jumps on, folds a branch whose arms name one block to a jump, and merges
  a block into its single jump predecessor (R836), so generic bodies no
  longer carry join and latch blocks into every instance: corpus concrete
  blocks fall by 10.4 % and generator-instance blocks by 13 %, with no
  instruction or output change. Upstream's generators keep this control
  flow structured, so these MLIR CFG patterns stand in for that structure.
  The elaborator now also counts blocks (`concrete_blocks`,
  `generator_blocks`, `generator_instance_blocks` under `--timings` and in
  `benchmarks/instantiation.tsv`).

- `sccp` now folds a per-lane SIMD construction such as
  `SIMD[DType.int32, 4](1, 2, 3, 4)` (R855) to one constant vector, as
  upstream folds `SIMD.__init__(*elems: Scalar[dtype])`, when each element
  is a literal, a one-lane constant of the vector's dtype, or a `Bool` into
  a `bool` vector. Its lanes come from one construction rule
  (`ct::simd_construction_lanes` over `CtLane::from_value`) that the
  comptime evaluator and the checker's constraint evaluator now share, so
  a compile-time integer literal in a float lane now rounds once, as at
  run time, instead of twice.

- `sccp` now folds a built-in scalar conversion of a constant
  (`UInt(Int(7))`, `Float64(n)`, `Bool(1)`, R856), Mojito's counterpart of
  the `pop.cast` upstream's inlined `SIMD.__init__` leaves, by one rule
  (`ct::convert_lane`, `CtValue::folded_conversion`) that the VM's
  `builtin_convert` and the comptime evaluator now share. A float
  converted to `Int` or `UInt` out of range or not finite does not fold,
  and is not a compile-time value, as the pin's interpreter refuses it;
  `comptime` conversions such as `UInt(Int(7))`, `Bool(1)`, and
  `Int(UInt(9))`, which the evaluator rejected, now fold.

- A built-in scalar constructed from a literal (`UInt(7)`, `Int(7)`,
  `Float64(2.5)`, R854) is now that literal's materialization, as upstream's
  builtin-inline `SIMD.__init__(IntLiteral)` is the literal's conversion,
  so `sccp` folds `UInt(7) // 2`; the nullary `UInt()` is the literal `0`
  materialized. The bundled package format is now 54.

- `sccp` now folds a literal splatted to a SIMD type of any width
  (`var v: SIMD[DType.int32, 4] = 1`, R847) to one constant vector, as
  upstream folds the splat `SIMD.__init__` to a `#pop.simd` attribute; a
  splat of a one-lane constant (`SIMD[DType.int32, 4](Int32(7))`) or a
  `Bool` fill folds the same way, and operators over such vectors fold
  too. Each lane comes from the one
  literal-to-lane rule (`CtLane::of_int_literal`, `of_float_literal`), which
  the VM's `simd_from_values` now calls instead of its own copy.

- `sccp` now folds operators over `UInt` and one-lane SIMD constants
  (R846), and a one-lane `simd.make` of a literal, as upstream's `SCCP`
  folds `pop` arithmetic on `#pop.simd` attributes. Lanes fold by one rule
  the VM's `simd_binop` now shares (`ct::int_lane_binop`,
  `float_lane_binop`, `lane_compare`, `bool_lane_binop`): an integer lane's
  zero divisor gives a zero lane, a narrow float lane rounds once, and a
  comparison folds to a `bool` mask. An out-of-range lane shift and
  float-lane `//`, `%`, and `**` stay unfolded. The same folder now accepts
  compile-time operators over sized scalars the pin accepts
  (`comptime b = a * 2` over a `Float32`, `y * 2` over a
  `Scalar[DType.float32]` parameter), and Pliron lowers a folded float lane
  from its bits, so an infinite or NaN lane no longer panics.

- `sccp` now folds a literal materialized to `UInt` or a one-lane SIMD
  type (R841) to a typed `Const::Value`, upstream's `kgen.param.constant`
  over a typed attribute, using one literal-to-lane rule
  (`CtLane::of_int_literal`, `of_float_literal`) that the VM's
  `materialize_literal` now shares. A `UInt` value-parameter read is that
  constant too, which the verifier, the elaborator, and the native backend
  accept. As at the pin, a struct's `UInt` value parameter now reads at
  `UInt` (`Self.n + 1` was rejected as `Int`), and an integer literal bound
  to a sized-scalar value parameter (`lane[300]()` over
  `x: Scalar[DType.uint8]`) materializes into its lane instead of being
  rejected.

- A post-elaboration pipeline, `sccp` then `canonicalize`, now runs over
  every concrete function the elaborator emits (R840), as upstream's
  per-function `'kgen.func'` pipeline runs `SCCP` and `Canonicalizer` after
  `ElaborateGenerators`: operators over an answered parameter read fold, a
  `branch` they decide becomes a `jump`, and a copy whose type substitution
  made trivial is forwarded. The program is re-admitted as concrete MIR and
  its elaboration counts recounted. `sccp` now clears a folded producer's
  in-place binding, which the verifier rejected. Corpus concrete
  instructions fall by 0.96 %; `benchmarks/instantiation.tsv` is rerun.

- The pre-elaboration pipeline now runs `sccp` (R781), upstream's `SCCP`,
  then `canonicalize` again, as upstream's second round does: operators,
  literal materializations to `Int` and `Float64`, and copies over
  constants that do not depend on a parameter become one `const` on the
  generator (`MirInstr::folded_constant`), and a `branch` on a constant
  becomes a `jump` with its untaken arm erased. Nothing folds where the
  compile-time folder and the VM could disagree. Corpus concrete
  instructions fall by 8 %; `benchmarks/instantiation.tsv` is rerun.

- `canonicalize` now forwards an identity copy's operand to its readers
  (R780), as upstream's `Canonicalizer` applies a fold that returns an
  operand: a `value.copy` of a scalar, pointer, `None`, or SIMD lane, typed
  alike on both sides, is removed and its readers name the source. Copies
  of structs, aggregates, and type parameters stay. Corpus concrete
  instructions fall by 1.8 %; `benchmarks/instantiation.tsv` is rerun.

- The pre-elaboration pipeline's second pass is `canonicalize` (R779),
  upstream's `Canonicalizer`: it erases blocks the entry does not reach and
  instructions whose results nothing reads when
  `MirInstr::would_be_trivially_dead` declares them free of effects, before
  the elaborator copies them into each instance. Copies, operators, calls,
  drops, and loan markers stay. Corpus generator-instance instructions fall
  by about 1 %; `benchmarks/instantiation.tsv` is rerun.

- The pre-elaboration pipeline's first pass is `eliminate-dead-symbols`
  (R778), upstream's `EliminateDeadSymbols`: it removes the program's own
  generic bodies that no concrete function or entry root reaches before
  the later passes and the post-drop verify see them, so `emit-mir` no
  longer prints them. A pipeline pass is now a program pass or a body pass
  (`analysis::Pass`); the package seam runs only body passes. Every
  concrete function stays, since Mojito has no `@export` (R834).
  `entry_roots` moved from `native::mono` to `mir`.

- A cleanup list names only what is still held where it runs (R826). A
  `try`'s list runs on its body's raise edge alone, on both backends, and
  keeps the slots some raise point there holds. A return drops in-region
  what nothing after it reads and lists only what it holds past the
  `finally` blocks it crosses, so a value the loop after a `try` needed, or
  one its `finally` read, is no longer leaked by a return through the `try`
  (`assets/ok/try_return_destroys_held_values.mojo`). A handler's escapes
  skip what its raise edge ended. The MIR verifier now models all three
  lists and rejects one naming a slot every path already ended. `UnOp` and
  `BinOp` record a raising operator's error type (MIR 1.53), mono keeps it
  on the method call it rewrites them to, and the checker now rejects a
  raising prefix operator outside `try` or a `raises` function, as the pin
  does (`assets/type_error/raising_prefix_operator_unhandled.mojo`).

- The MIR verifier rejects a `drop.var` or `var.consume` of a slot that
  every path reaching it already dropped, consumed, or moved (R825), so
  `mojito opt`, `run_pipeline`, package body finish, and concrete MIR all
  refuse a pass that destroys a value twice. The rule is a must-analysis over
  blocks, `try` regions, and their escape edges. Drop elaboration was fixed
  first: a moved slot no longer backs a loaded register (`two(h^, h.v)`
  dropped `h` after forwarding it), a handler's live set is seeded only at
  checked raise points (a non-raising call after a move kept the slot live),
  and a `for` loop's yielded slot left the loop cleanup list (a `continue`
  out of a `try` dropped it after the binding moved it). `mojito opt` now
  prints its load findings, and `tests/mir_passes` gains `.error` cases.

- Drop elaboration no longer drops slots around a `try` that the `try`
  already ended (R824). A handler's error binding is a def site at the
  handler's entry, every region's locals are scoped to their `try`, and the
  enclosing walk places no death after a `try`. A handler local used to die
  one statement late round a loop, a caught error rebound by the next
  iteration or bound by `except e: pass` was never destroyed, and outer
  values the regions ended were dropped again after the `try`. The verifier
  now rejects a branch to a `try` region's entry block.

- Drop elaboration now owns every iterator drop and is idempotent (R822,
  partly): a `GetIter` defines its iterator slot, an input `DropVar` is the
  slot's teardown, and HIR/MIR loop lowering emits no drops of its own
  (`HirInstr::Drop` is gone). A loop iterator used to be dropped twice at
  its exit, and an inner loop's iterator again at the outer loop's exit;
  running `elaborate-drops` twice doubled every drop. Drops around a `try`
  (R824) and the verifier check against a drop of an ended slot (R825)
  remain.

- A MIR pass can be tested as MIR text in, MIR text out (R776), after
  MLIR's `mlir-opt`: `analysis::pass_named` registers passes by name
  (every pre-elaboration pass, plus drop elaboration as
  `elaborate-drops`), `analysis::opt` and the new `mojito opt [--pass
  NAME]… [FILE]` command run them over a loaded artifact and verify after
  each in every build, and `tests/mir_pass_test.rs` compares each case under
  `tests/mir_passes/<pass>/` with its expected `functions:` section
  (`mir::text::disassemble_functions`).

- A package body whose lowering, finish, or verify reached a package
  declaration that does not decode now says so in its own verdict (R821):
  each unit's lowering and each body's finish read the package through a
  view of their own (`PackageBodies::reached`, leading
  `PackageBodies::findings`), and the elaborator refuses such a body as
  `MonoErrorKind::Decode`. A body that reached one used to verify clean,
  lowered as if the struct had no parameters.
- A package declaration that does not decode fails only a compilation whose
  lookups reach it (R820): its verdict lives in its own record's cell
  (`PackageDeclarations::decode`), each compilation's view notes the failed
  records it reaches (`ReachedFailures`), and the check, the program
  verifier, and the elaborator (`MonoErrorKind::Decode`) refuse that
  compilation alone. `CompiledProgram::concrete_mir` no longer refuses every
  later compilation in the process once any one reached a bad record.
- The elaborator refuses a package body it reaches whose finish or verify
  left findings, a pre-elaboration pass's included (R819): each body keeps
  its own verdict (`PackageBodies::findings`), so a body no program reaches
  fails none. `CompiledProgram::concrete_mir` now refuses a program whose own
  bodies carry findings for every backend, not only `Compiler::execute`.
- Mojito has a pre-elaboration pass pipeline (R777): `analysis::PIPELINE`
  runs between drop elaboration and the elaborator, over the program's own
  bodies and over each imported package body as it is finished, once per
  body, verifying after each pass in debug builds. It is empty, so nothing
  changes yet; `MOJITO_PRE_ELABORATION=0` turns it off.
- The `pre_elaboration::*` corpus group (R775) runs every executable fixture
  with the pre-elaboration pipeline on and off and fails on any difference
  in output, error text, or verifier findings, so a pass can join
  `analysis::PIPELINE` only if the VM cannot tell it ran.
- Every `benchmarks/compile` program now runs at the pinned Mojo with
  Mojito's output (R818): `tuple.mojo`'s `Pair` declares `Sized`, which the
  pin requires for `len` and Mojito does not yet (R191), so
  `scripts/sweep-assets-mojo --benchmarks` passes on all eleven programs.
- The overnight gate checks `benchmarks/compile` against the pinned Mojo
  (R814): its `benchmarks-mojo-sweep` step runs the new
  `scripts/sweep-assets-mojo --benchmarks`, which fails on any benchmark the
  pin or Mojito rejects or whose stdout differs, with no burn-down list.
  `benchmarks/compile/tuple.mojo` is respelled as the pin accepts it
  (`__getitem_param__`, a `Deinitable` bound, R530), so all 1 018 fixtures of
  `benchmarks/instantiation.tsv` now elaborate.
- The overnight gate ratchets the instantiated program's size (R773):
  `scripts/instantiation-report --check` fails when the concrete-instruction
  total over the fixtures elaborated in both runs grows past, or shrinks
  below, the committed `benchmarks/instantiation.tsv`, or when a fixture
  stops elaborating, so expansion moves only when a change commits a new
  baseline.
- `scripts/instantiation-report` records the elaborator's size counters
  over the corpus (R772): it runs every `assets/ok` and `benchmarks/compile`
  fixture and writes one row per fixture plus a `total` row to the committed
  `benchmarks/instantiation.tsv`, so a change's effect on expansion is that
  file's diff. The first baseline elaborates 1 017 fixtures to 892 391
  concrete instructions (`docs/performance.md` §3).
- `benchmarks/compile` gains three instantiation-heavy programs (R774):
  `collections_many.mojo` (803 generator instances), `nested_generics.mojo`,
  and `trait_defaults.mojo`, each checked against the pinned Mojo by hand.
  Writing them found three Mojito-only acceptances, filed as R815–R817
  with probes, and the missing pin sweep of the folder, R814.
- `--timings` says how much the elaborator multiplied the program (R771):
  beside `concrete_functions` it reports `concrete_instructions`,
  `generators`, `generator_instances`, `generator_instructions`, and
  `generator_instance_instructions`, and `MOJITO_TIMING_NOTES=1` adds one
  `generator` note per instantiated template. The elaborator returns them as
  `SpecializedProgram::counts` (`ElaborationCounts`), replacing the unread
  `ParametricInstances`, which missed every generator of the imported std
  package.
- The bundled `ParameterList` conforms to `Writable` where its element type
  does, as upstream's does (R493): `print(vs)`, `String(vs)`, and
  `repr(vs)` over a value pack now work instead of being rejected, through
  upstream's `_write_elements`, `write_to`, and `write_repr_to`
  (`assets/ok/value_pack_parameter_list_writable.mojo`). The list's other
  missing members are filed as R806–R813.
- A value pack is its `ParameterList` in more positions, as upstream's is
  (R494): the bundled struct declares upstream's compile-time `size`
  member, `__len__` returns it, and a value pack's runtime `len` or
  subscript calls `__len__` or `__getitem__` instead of a checker-recorded
  constant. A compile-time value member reads through an instance
  (`args.size`, `c.K`), so `vs.size` works in a body, a bracket argument,
  and a signature type. `comptime e = vs[1]` binds the element where it was
  an undefined name, a runtime read of a local `var l = vs` no longer
  yields the element's pointer, and an `x == DType.float32` comparison no
  longer leaves an untyped slot the native backend rejects
  (`assets/ok/value_pack_parameter_list_members.mojo`). A compile-time
  subscript is still a checker rule (R800, R801).
- A `comptime for` over an iterable the built-in rule does not serve
  iterates by upstream's iterator protocol (R321): the check spells the
  iterable as its `__iter__()` call, the MIR header
  (`comptime_for.iterator`, schema 1.52, package format 53) binds the
  iterator, and the elaborator steps it with the bundled
  `std.builtin._stubs` functions `paramfor_has_next` and
  `paramfor_next_iter`, the variable each copy's `paramfor_next_value`.
  A user `Iterator & Copyable`, or a struct whose `__iter__()` returns one,
  now runs as at the pin where it was rejected
  (`assets/ok/comptime_for_user_iterator.mojo`). A compile-time
  application of a generic function given a closed generic struct type
  now binds the struct's instance. `range`, collections, packs, and
  reflection lists keep their own rule (R791, R794, R795).
- Every closed compile-time value is materialized by one operation, the
  typed `Const::Value` its read's register types, as upstream's
  `kgen.param.constant` is (R524): a string read at `String`, and a
  struct or tuple holding one — a module or local `comptime` value, a
  value parameter, a `comptime for` element — is no longer rebuilt by
  calling its constructor on the frozen fields, so an `__init__` that
  transforms its arguments runs once, at compile time, where each read
  ran it again (`assets/ok/comptime_value_materialized_without_constructor.mojo`).
  Pliron materializes a top-level string constant. A list read at an
  `Array` is still constructed at each read (R789).
- A local `comptime` binding of an application (`comptime s = make(3)`)
  is an alias of it, as upstream's is (R523): each read is the
  application's `ParamValue`, as a module constant's read is
  (`Checker::local_comptime_application`), and the statement op holds the
  application (`CheckedStmtOp::Comptime::denotes`), so MIR gives the
  binding no slot. Every read now materializes a fresh value that dies
  after the read, where one slot died at its last use; a field read
  extracts from the application; and a nested `def` may read the binding,
  which was rejected as an uninferable capture
  (`assets/ok/comptime_local_application_per_read.mojo`,
  `assets/type_error/comptime_local_struct_application_crossing.mojo`).
  The package format is 52. A binding of any other parameter expression
  still stores a slot (R788).
- Every checked signature holds a parameter default as an attribute
  (`SignatureDefault`: a literal constant, an evaluated default, or a
  trait requirement's parameter expression), as its lowered declaration
  does, rather than an own method or trait requirement holding the
  default's syntax (R590). A requirement default reading the method's
  value parameters is a `ParamExpr` with module constants folded, which a
  call through the bound evaluates with its solved binding of the
  parameters, so a caller's own binder or an enclosing struct's
  parameter (`s.scale[Self.w](5)`) now binds it, where `Self.w` was
  rejected (`assets/ok/trait_requirement_default_from_caller_parameter.mojo`).
  An imported trait's requirement holds no syntax; the package format is
  51 and the generated MIR is unchanged. Completing the call without
  rewriting its syntax is R769; R770 is a struct-parameter bracket
  argument defect found on the way.
- The checked program holds no syntax and builds no second table from it
  (R625): `CheckedProgram::statements` and the syntax walk that made the
  declarations (`build_checked_declarations`) are gone, the declarations
  are made by a walk over the checked statement nodes, each declaring
  node naming its own, and a `def`'s explicit captures are its op's, as
  upstream's closure op holds its captured values. A package build slices
  its units from the compiled program (`CheckedProgram::units`) rather
  than from a copy of the check's fact records, and
  `check_program_for_discovery` and the discovery queries are gone. The
  generated MIR is unchanged; the bundled package format is 50. Deleting
  `DiscoveryResult` itself waits on the check emitting its nodes (R767),
  and the declaration table's remaining duplication is R768.
- A package unit's checked tables are copied out of the whole program's
  checked nodes, the subtree under the unit's statement node renumbered
  in build order, as upstream's unit is an op of its module, rather than
  rebuilt from the unit's syntax and the check's fact records (R624's
  first slice). The arena builder runs once per program and has no scan
  mode; the package bytes and the generated MIR are unchanged. Emitting
  the nodes during the check is left to R764–R766.
- Each checked expression, statement, and declaration node in a package
  unit encodes as an MLIR bytecode op does: an encoding mask naming the
  attributes it holds, then its parts, then those attributes alone,
  rather than every attribute slot, empty or not (R632). The std
  package's units shrink from 1.04 MB to 0.87 MB (the package 1.33 →
  1.16 MB) and decode in 19.5 ms rather than 21 ms. `NodeFacts` loses its
  unused encoding. The bundled package format is 49.
- A package unit no longer records its struct's `Origin` and `OriginSet`
  parameter names, and the checked program no longer collects them
  (`CheckedProgram::origin_parameters_at`, `origin_parameter_names`), since
  nothing has read them since R628 (R631). The bundled package format is
  48. A struct's origin parameters still stay out of its checked parameter
  list, where upstream's holds them (R761–R763).
- A checked declaration op holds what the check resolved for it rather
  than its signature as written, as upstream's function op holds its
  resolved function type (R630). A `def`'s and a method's op hold the
  symbol it lowers to, a method's its registered name, and its checked
  signature: compile-time parameters, parameter and result types, and
  effect. A struct's op holds its fields' checked types. No op keeps
  annotation, type-parameter, or decorator syntax, and MIR reads a
  declaration from its op rather than its declaration-site rows. The
  generated MIR is unchanged; the bundled package format is 47.
- MIR matches no operator, lambda, or other expression syntax: every such
  site lowers from its checked op (`CheckedOp::Prefix`, `Infix`, `Lambda`),
  as upstream's lowering consumes typed ops (R644). The last two syntax
  sites went with R628 and R643; the generated MIR is unchanged.
- The check records each bracket argument's parameter attribute once, as
  the binder that resolved the bracket list returns it, rather than
  resolving a type-shaped argument a second time from its spelling (R659).
  A value-shaped argument a builtin binds to a type (`size_of[T]()`,
  `_unqualified_type_name[T]()`, a `Variant` method's `[T]`) now carries
  that type as its attribute too; the generated MIR is unchanged.
- A bracket argument written as a value that the check binds to a type or
  an origin (`List[String]()`'s `String`, `Pointer[Int, origin_of(x)]`'s
  origin) is now the call's parameter attribute rather than a name node,
  as upstream's call op carries it (R658). The check records it as it
  binds the argument, and MIR no longer reads an erasure mark to skip it;
  the generated MIR is unchanged. The bundled package format is 46.
- A copy construction a read receiver takes at its source's last use
  (`M(copy=d).show()`) lends the source's storage to the method, as the pin
  does (R746): no copy or move initializer runs, and the source dies when
  the call returns. It used to run the move initializer. A `ref self`
  receiver keeps the move, as at the pin.
- Every potentially raising operation is a raise edge of the
  explicit-destruction check, as at the pin (R749): a value that must be
  explicitly destroyed and is still owned where an error is thrown is
  rejected as abandoned — a local, a `var` parameter, an owned temporary
  evaluated before a later raising operand (`take(L(1), boom(1))`), or a
  transfer its call has not yet consumed (`take(l^, boom(1))`). An `except`
  arm starts from the state its raise edges reach it with, so a `raise`
  inside a `try`, an arm closing a value the body closes after the raising
  call, and an unreachable arm are accepted, where they used to be
  rejected.
- A discarded transfer, `_ = x^` or `x^` as a statement, destroys `x`
  where it lies without running the move initializer, as the pin does
  (R748), over a local, a `var` parameter, or a generic one. A bare `x^`
  statement used to move the value and then leak it.
- A call consumes its owned operands itself, after every operand is
  evaluated, as the pin does (R747): a raise in a later operand destroys a
  `^` transfer, a temporary, a copy construction, a keyword or `*xs`
  collector argument, a conditional's result, or an owned receiver before
  the `try` handler runs, where they used to leak, and `two(x^, look(x))`
  and `owe(k.v, k^)` are accepted. An outer value whose last use in a `try`
  body follows a raise now dies on the raise edge, before the handler.
- A copy construction an owned (`var`/`deinit`) receiver takes at its
  source's last use forwards the source's storage, as the pin does (R737):
  `M(copy=d).consume()` runs neither the copy nor the move initializer. A
  raising operand evaluated after the receiver or an owned argument keeps
  the move, as at the pin, where an owned argument used to forward past it.
- A read temporary whose call takes a conditional expression argument after
  it is destroyed when the call returns, as the pin does (R745):
  `show_n(M(2), 3 if c else 4)` prints `show_n 2 3` before `del 2`, at top
  level, inside a `try`, round a loop, and as a method receiver, where the
  callee used to read a destroyed value.
- A copy construction a read parameter or read operator operand takes at
  its source's last use lends the source's storage, as the pin does
  (R736): `show(M(copy=a))` runs neither the copy nor the move
  initializer, and `a` dies when the call returns, inside a `try` too. A
  raising argument evaluated after the construction keeps the move, as at
  the pin.
- A conditional expression's result is an owned temporary its consumer
  takes, as the pin does (R735): `take(y^ if c else z^)` runs one move
  initializer and no copy, a binding, `return`, or receiver takes the
  result the same way, a read parameter borrows it and the caller destroys
  it after the call, and a `ref` parameter arm is copied once instead of
  aliased and destroyed twice.
- A `^` transfer an owning `var *xs` collector gathers hands over the
  source's storage without running its move initializer, and a loop or
  comprehension over a named homogeneous collector, read or owning,
  borrows each element in place instead of copying the pack and moving
  its elements out, as the pin does (R734): `many(a^, b^)` printing each
  element prints `x 1`, `x 2`, `del 2`, `del 1`. MIR indexes the pack
  through a cursor where upstream iterates `VariadicListMem` (R743).
- A `^` transfer that an owned (`var`/`deinit`) parameter takes, whether
  an argument, a keyword argument, a receiver, or a struct operator's
  operand, hands over the source's storage without running its move
  initializer, as the pin does (R730). So does `take(M(copy=y))` at `y`'s
  last use. A binding, a return, a field store, and a container element
  still move-initialize. MIR adds `UseMode::Forward` (`forward`, text
  1.51), and the package format is 45.
- `S(copy=x)` over a temporary, or over a whole owned local or `var`
  parameter at its last use, moves the source instead of copying it, as
  the pin does (R723): `S(copy=S(1))` no longer runs the copy initializer
  or leaks the temporary, and `var d = S(copy=s)` with `s` dead after runs
  `S`'s move initializer once. A source read afterwards, a field, a type
  parameter's `T(copy=x)`, and `.copy()` still copy.
- A local declared before a `try` and first assigned inside its body
  (`var x: S`, then `try: x = S(may(0))`) survives the body's exit and is
  read after the `try`, as the pin does (R729): MIR marks the declaration
  with `DeclareVar` (`var.declare`, MIR text 1.50, `mojito-package 44`),
  which drop elaboration counts as a def site, so the slot is no longer in
  the body's `Try.cleanup`. The VM no longer fails with `field access on
  non-struct None`, and natively the value is no longer destroyed before
  the read.
- A local declared without an initializer (`var a: Int`, `var a: S`) no
  longer stores a placeholder `None` into its slot; its first assignment
  initializes it (R727). Natively such a local no longer stops at an
  "erased analysis register" or "undefined aggregate register", and on the
  VM a struct local assigned in both arms of an `if` and then read no
  longer fails MIR verification.
- A call's argument temporaries in a `try` body inside a loop die when
  the call returns, as the pin does (R722): `print(mk(S(5), S(6), j))` in
  a looped `try` destroys `6` and `5` before printing, where they used to
  wait for the `try`'s cleanup. A `try` no longer counts its own body's
  locals as read, so they are not held live round the loop.
- A raise in a `try` body destroys the body's live values in reverse
  declaration order, as the pin does (R720): `S(1) + S(9)` over a raising
  `__add__`, a raising method or free call over temporaries, and a raising
  constructor destroy `9` before `1`, and a named local after both.
- A construction is alias-checked as its initializer's call, as the pin
  has it (R721): `P(s, s)` over `__init__(out self, mut a: S, b: S)`, its
  keyword and generic forms, a `*rest` collector beside a `mut` argument,
  and `Tuple(Span(xs), Span(xs))` are now rejected instead of running.
- A static call read as a call argument or operator operand
  (`print(Box[String].pick(4))`, `show(Plain.same(a))`) no longer interns
  a slot named after its type receiver (R725): natively such a call no
  longer stops at an "untyped variable", and a `ref`-returning static call
  as an argument no longer fails MIR verification on either backend.
- A construction is its `__init__`'s call, as the pin has it (R717):
  `P(S(1), S(2))` destroys each `S` temporary when the initializer
  returns instead of leaking it, and `P(s, s)` reads `s` in place instead
  of copying it and leaking the copies. A raising `__init__` makes the
  construction a raising call: its result is taken after its argument
  temporaries die, and using it outside `try` or a `raises` function is
  rejected. Natively, a construction's read argument is borrowed as a
  call's is, so it runs no copy.
- A non-raising call writes its result in place, as the pin does (R714):
  a discarded result (`mk(S(2), S(3))`, `S(1) + S(2)`, `1 + S(18)`, a
  method, function-value, or generic call) and an unused
  `var k = mk(...)` die at the call, before its argument temporaries and
  the owners it last uses. MIR's `DefVar` gains `in_place` (text schema
  1.49, package format 43). A raising struct infix dunder is now a raising
  call, so using it outside `try` or a `raises` function is rejected, as
  the pin does.
- A by-value `__getitem__` result is an owned temporary, as the pin does
  (R713): `f(b[1])` and `print(b[2] < b[3])` destroy each element once the
  call or dunder returns instead of leaking it, and passing `b[9]` to a
  `var` parameter, binding `var t = b[10]`, `_ = b[11]`, and `b[12]^` no
  longer demand an implicit copy. A discarded operator result
  (`S(1) + S(2)` as a statement) is now destroyed instead of leaked.
- A struct operator reads a place operand where it lies, as the pin does
  (R710): `print(xs[0] < xs[1])` over a `List[S]` reads both elements
  through `__getitem__`'s reference instead of copying them out and leaking
  the copies. This covers prefix and reflected dunders, a named operand of
  a prefix operator (`-s`), and tuple elements; a chained comparison copies
  only its first middle operand.
- A temporary operand a struct operator's dunder reads is destroyed when
  the dunder returns, the argument before the receiver, as the pin does
  (R709): `print(S(1) < S(2))` prints `del 2`, `del 1`, then `True`. This
  covers prefix operators, reflected dunders, an operator result used as an
  operand, argument, or field base, a chained comparison's two ends, and a
  comparison through a bound; a linear temporary operand, and a generic one
  not proven `Deinitable`, is now rejected as an abandoned temporary.
- Each link of a chained comparison over structs is its dunder's call, as
  the pin does (R703): `print(a < b < c)` destroys `a` after the first link
  and the middle's temporary and `c` after the second. Only the first
  middle is copied, right after the first link (a temporary or `^` middle
  moves in), so `a < M(2) < c < d` and `a < b^ < c < d` over a
  non-copyable `M` now compile, and a generic first middle not proven
  `Deinitable` is rejected as an abandoned temporary. The package format
  is 42.
- A comparison (`==`, `!=`, `<`, `>`, `<=`, `>=`) over a type parameter
  bounded by `Equatable` or `Comparable` names the bound's requirement, so
  it is that requirement's call, as the pin does (R702): `print(x < a)`
  over a copied `x` destroys `x` when `__lt__` returns, before `print`
  runs. A builtin instance still compares primitively.
- A trait requirement carries its signature, so a method or subscript call
  through a trait bound in a generic body passes its arguments by the
  requirement's declared conventions, as the pin does (R682):
  `s.sh(b.t)` over `def sh(self, x: Int)` destroys `b` before `sh` runs,
  through a refined bound, a trait default's `self.sh(...)`, and `s[c.t]`
  alike. Textual MIR is 1.48 and the package format 41.
- A struct operator names the dunder the checker selected even when the
  struct declares only one, so it is that method's call (R687):
  `print(o + 1)` over a lone `__add__` destroys `o` when `__add__` returns,
  before `print` runs, as the pin and `o.__add__(1)` do; likewise `b - c`
  over two struct operands, `d != e` served by `__eq__`, and a generic
  struct's operator.
- A pointer subscript (`p[i]`, `p[]`, `p[unsafe_offset=i]`) reads its
  pointee in place, as upstream's `ref`-returning `__getitem__` does, and
  only a consuming use copies it (R700): `p[0].copy()` copies once instead
  of twice with a leaked intermediate, `print(p[0].v)` copies nothing, and
  `var z = pa[]` through `Pointer(to=a)` copies `a` on the VM instead of
  aliasing it. `List` slicing, through `List._get_copy`, copies each
  element once.
- A non-empty dict display is `Dict`'s `__dict_literal__` constructor over
  a key list and a value list, as the pin lowers it (R694): each entry is
  copied as upstream's constructor copies it, so `{1: mk(4)}` traces
  `copy 4 copy 14 del 14` as at the pin, and a display over a value that is
  not `Copyable & Deinitable` (`{1: M(1)}` over a `Movable`-only `M`) is
  rejected at the display. The bundled constructor no longer leaks the
  intermediate copy `List._get_copy` made.
- A list, set, dict, or array display passed as a call argument is built at
  the call's setup, after every argument is evaluated, as the pin builds it
  (R693): `ha([a.t, 1], bump(a))` and `hd({1: a.t}, bump(a))` see `bump`'s
  write, as do keyword, constructor, method, generic, and `print`
  arguments and a display nested in such a display, and
  `ha([lst[0], 1], set0(lst))` is rejected as a use of an invalidated
  interior reference. Tuple displays and comprehensions are still built
  where written.
- A display reads its elements at its constructor's setup, after every
  element is evaluated, as the pin reads it (R686): `(u.t, bump(u))` and
  `[v.t, bump(v)]` see `bump`'s write, as do `List`, `Set`, and `Dict`
  displays, a dict display evaluates all its keys before its values, and
  `(il[0], setz(il))` is rejected as a use of an invalidated interior
  reference.
- A by-value operand with a reference result is evaluated to its reference
  where written and read at call setup, as the pin reads it (R685):
  `two(tup[0], bt(tup))`, `two(s.get(), bump(s))`, `two(pick(t), bt(t))`,
  and `print(t[0] + bt(t))` now see the later argument's write, a `String`
  element passed to a read parameter beside an argument that appends to it
  no longer traps in the VM, `two(lst[0], bl(lst))` is rejected as a use of
  an invalidated interior reference, and `two(tup[0], take(tup^))` as a use
  of the moved `tup`. A `Dict` lookup no longer stales a reference to an
  earlier lookup's value, so `print(d["x"] + d["y"])` and a held view of
  one value across a second lookup run as upstream. MIR text format 1.47
  adds a loan's `setup` flag, and the package format is 39.
- A by-value place argument is read at call setup, after every argument
  is evaluated, as the pin reads it (R681): `two(c.t, bump(c))` with a
  mutating `bump` now prints `bump`'s write, as do `d.t + bump(d)`, a
  method receiver or argument, a comparison chain's first operand, a
  subscript getter's or setter's index, and a function value's argument;
  a method's `var` copies run in parameter order; and
  `two(a.t, take(a^))` is rejected as a use of the moved `a`. A loan
  conflict now names the same live reference on every run.
- Each `_ = e` statement owns its own discard slot (R683): two discards in
  one function no longer share one slot typed by the last of them, so
  natively `_ = two(p.n, q.n)` beside `_ = pair^` no longer fails LLVM
  verification, and `_ = pair^` followed by `_ = 5` runs `Pair`'s
  destructor instead of silently skipping it.
- Values whose last use is the same call are destroyed in the pin's order
  (R675): a scalar read passed by value, to a concrete register-passable
  `read` or `var` parameter, a built-in operator, or a slice bound, is
  loaded at call setup, so its owner dies before the call in parameter
  order. `a.t + b.t` now destroys `a` first, and `two(h.t, e.t)` destroys
  `h` and `e` before `two` runs, including for methods, keywords,
  subscripts, function values, nested defs, and a `deinit self` body's
  fields. An argument passed by address (`print`, a generic `T`, a `*xs`
  collector, a by-reference argument) still keeps its owner through the
  call.
- Natively, a function value or nested def returning a reference runs
  (R676): `var f = field; print(f(w))`, a nested
  `def inner(w: W) -> ref[origin_of(w.t)] Int`, and a raising
  `var g = raising; print(g(w))` no longer stop with "unsupported
  reference-returning indirect call". An indirect-call contract with a
  reference result returns the referent's address as a compiled callee
  does, and a contract disagreeing with its target in result form is
  still rejected.
- A reference result through a read parameter borrows the caller's
  argument (R672): `def field(w: W) -> ref[origin_of(w.t)] Int` no longer
  stops with "stale reference" on the VM. A call whose result borrows its
  reads retains each read argument's place, the VM binds it as a caller
  handle on every call path, and a temporary argument the result names
  materializes and lives as long as the reference, for free calls,
  methods, statics, keywords, function values, and nested defs. A static
  method's reference result is now read through, and `h.pick(w) = 5`
  through a read parameter is rejected as the pin rejects it.
- A result names `origin_of(p)` only when `p` has memory to borrow, as the
  pin judges it (R668): a `ref` parameter or receiver always, any other
  only when its type is proven not `RegisterPassable`. A generic, an
  associated type, a trait's `Self`, or a conditionally register-passable
  struct is rejected with "might expand to a RegisterPassable type", a
  proven one with "has RegisterPassable type", and a read trivially
  register-passable one with "doesn't have a memory origin". The bundled
  `Optional` and `Tuple` declare upstream's conditional `RegisterPassable`,
  and `conforms_to(_, RegisterPassable)` is no longer answered shallowly.
- A generic iterable's explicit `items.__iter__()` result answers its
  `Iterator` bound's methods (R665): a method on an associated projection
  resolves through the projection's declared bound, the result names
  `origin_of(items)` and borrows `items`, and a signature may spell
  `-> C.IteratorType[origin_of(items)]` over a `ref` parameter. As in the
  pin, that origin converts to no other, so returning it as
  `C.IteratorType[o]` for `ref[o] items` is rejected.
- A split loop iterator slot is typed by its `__iter__` result (R661):
  the check records `IterationProtocol::iterator_ty`, and MIR types the
  `$iterobj<N>` and `$compiterobj<N>` slots at the `GetIter` that
  declares them, as an upstream iterator is a value of `__iter__`'s result
  type. Over a generic `C: Iterable` the type is `C.IteratorType[origin_of(xs)]`,
  which the elaborator now answers from the conformer's declared member
  (`StructDeclaration::parameterized_associated_types`, MIR text 1.46,
  package format 38), so `var it = items.__iter__()` in a generic body
  runs. `mir::verify` rejects a `UseVar` or `DropVar` slot with no type,
  and mono no longer retypes a split slot. No program's output changes.

- A value binder's frame slot is typed in MIR (R660): `Flatten::var`
  types the slot it interns at a binder's first read with the binder's
  declared type, as an upstream parameter has its declared type, so `n`
  in `def plain[n: Int]` is no longer an untyped slot. Mono's
  `scalar_parameter_ty` backfill is gone; `substitute_function`
  substitutes the declared type. Only `var_types` changes; no program's
  output does.

- HIR's CFG no longer types slots (R634): it typed each slot by the
  first same-spelled name read anywhere in the program, so another
  function's local could type it. MIR types each slot from the op that
  declares it, a frame's parameter, value-parameter, and named-result
  slots from its declaration, as an upstream value's type is its
  defining op's. MIR changes in one place: a value binder read inside a
  `try` no longer takes a stray type (R660 files the binder's slot
  type); no program's output changes.

- A local display binding references its value's node rather than
  copying its HIR at each crossing (R651), as an upstream use references
  its SSA definition, and is scoped to the region that declares it, as the
  check already scopes it: a `try` region's bindings go out of scope with
  it. `Flatten::held` and `mojito-mir`'s `typed-arena` dependency are
  gone. No program's output or MIR changes.

- A package unit installs as its checked nodes alone (R650): a lowering
  unit names its declaration's statement node, never its syntax, and
  `CheckedProgram::installed` rebuilds no statements, wrapper body, or
  trait member declaration from the nodes, as upstream's importer
  materializes ops, never source. The node-to-syntax rebuilder
  (`CheckedTables::statement_syntax_of`, `syntax_of`) is deleted. No
  program's output or MIR changes.

- MIR decides whether a parameter default reads an enclosing binder from
  resolved attributes alone (R656): a lambda in the default answers through
  its checked function type, as upstream's closure op carries its resolved
  signature, instead of a walk over its hidden `def`'s annotations as
  written, so `mir.rs` matches no `ExprKind`. A lambda in a struct method's
  parameter default is now the contextual `lambda expressions in parameter
  default values` error, as in a free `def`, instead of an invariant
  violation. No program's output or MIR changes.

- A checked op's type operand is the attribute the check resolved, not the
  annotation as written (R628): a bracket argument written as a type is a
  `CheckedParamArg::Attribute` holding its `TyArg` — a type, a value it
  names (`Self.n`, a pack spread `*vs`, a display element `L[0]`), or an
  origin — and a type in value position holds its `Ty`, as upstream's call
  op carries each bracket argument as a parameter attribute. The check
  records them at the occurrence (`NodeFacts::parameter_attributes`,
  `type_operand`), and MIR reads them instead of guessing the decision
  again from the spelling, so its annotation-index folder, display-element
  and pack-spread spelling matchers, and origin-name list are gone. The
  package format is 37. No program's output or MIR instructions change.

- A lifted thunk's prologue is the statement nodes of the `comptime`
  bindings it reads (R655), handed to `Cfg::build_checked_thunk` and
  nested discovery as ids, as upstream's thunk region holds the bindings'
  ops. The fix landed with R629 and R643; closing the entry changes no
  code, MIR, or output.

- A trait default's body is built as checked nodes once (R657): the
  trait's op names its member's `def` node (`CheckedTraitMethod::default`)
  instead of holding a second copy of the body, as upstream's trait names
  its member function, and lowering units find the member through its
  trait. No program's output or MIR changes.

- MIR reads each declaration it lowers from its checked op (R643): a
  `def`'s name, parameters, markers, and defaults from its `CheckedDef`, a
  struct's fields and methods from its `CheckedStruct`, and a trait
  default from its own `def` node, which the whole program now holds as a
  package unit does, as upstream lowers a function from its op. Nested
  `def`s and lambdas are found by walking statement ids alone, so a lifted
  thunk no longer rebuilds its prologue's syntax (R655). No program's
  output or MIR changes.

- HIR is handed every body as its statement nodes (R629): a unit's from its
  declaration's op, the top level's and a wrapper's from the tables' roots,
  a nested `def`'s or lambda's from its checked `def`, and a lifted thunk's
  from its bindings' nodes, as upstream lowers a region by walking the ops
  it holds. Nested discovery walks syntax in lockstep with the ids, so no
  checked statement is found by its source location, and the span-keyed
  statement index is deleted. No program's output or MIR changes.

- MIR holds no syntax rebuilder or address-keyed fact index (R649): the
  scaffolding R646 listed was deleted as R647, R652, R653, and R648 moved
  each lowering onto node handles. A lifted thunk's prologue still
  rebuilds its bindings' statements (R655), and a binder scan still walks
  annotation syntax (R656). No program's output or MIR changes.

- MIR lowers a straight-line statement from its checked node (R648): the
  op alone decides the lowering, and each operand is the handle of the
  statement's matching HIR expression, a subscript store's slice bounds and
  multi-subscript arguments the ids the op names, as upstream lowering reads
  a statement's operands off its operation. A parameter default's binder
  scan walks the default's checked node, and a lifted thunk rebuilds its
  prologue where it is lowered. MIR rebuilds no syntax for a body
  expression or statement, and the syntax bridge and its address-keyed
  frames are deleted. No program's output or MIR changes.

- MIR's call family takes the call's node handle (R653): a direct call,
  an invocation, and a method call read their callee, receiver,
  arguments, keywords, and compile-time arguments off the handle, as
  upstream lowering reads a call operation's operands off the operation,
  so no expression lowering is handed syntax rebuilt from its node. No
  program's output or MIR changes.

- MIR's literal, access, display, and operator families take the
  expression's node handle (R652) and read each operand off it by the id
  the op names, a slice bound, subscript argument, or comprehension clause
  included, as upstream lowering reads an operation's operands off the
  operation. Only a call, an invocation, or a method call is still handed
  the syntax rebuilt from its node, through one bridge (R653). No
  program's output or MIR changes.

- MIR lowers an expression's adjustments, its place, and a call's
  arguments, receiver, compile-time arguments, anchors, and invalidations
  from the expression's node handle (R647), as upstream lowering reads an
  operation's operands off the operation: a place or an argument is
  matched by its op and its operands read off the handle, a compile-time
  argument is the op's checked one, and the facts these read are the
  node's own methods. Only the expression families are still handed the
  syntax rebuilt from a node, through one crossing (R652, R653). No
  program's output or MIR changes.

- MIR reads a checked expression's facts off a node handle (R646), as
  upstream lowering reads an op's attributes and operands off the op
  itself: a `Node` pairs HIR's tree for the expression with its checked
  node, its operands are the op's, and its type, owner, call contract,
  adjustments, and loans are read from it rather than from the address of
  the syntax rebuilt for it. Lowering still walks that syntax, bridged to
  the handle both ways until R649. No program's output or MIR changes.

- MIR no longer finds a checked expression by its source location, and a
  lifted compile-time thunk carries no syntax (R623), as upstream's
  lowering reaches an op through its operands and never by location: the
  checked tables keep no expression span index, a thunk request holds only
  its returned node, a local `comptime` display binding holds its HIR
  value rather than a copy of its syntax and facts, and a `comptime for`
  over such a binding finds its sequence by the binding's owner, which the
  check records on the loop. The package format is `mojito-package 35`.
  No program's output or MIR changes.

- HIR receives an expression thunk's returned value, for a lifted
  compile-time application and for an evaluated parameter default, as its
  checked node (R645), as upstream builds a lifted function from the typed
  value it was elaborated from, rather than finding the value by source
  span. Each lowering unit names its checked statement, so a default's node
  is read off its declaration's op. No program's output or MIR changes.

- MIR finds the compile-time applications the check lifted out of a body,
  and the local bindings a lifted thunk reads, by walking the body's
  checked statement and expression nodes (R642), as upstream walks a typed
  body's ops, rather than walking its syntax and looking each expression's
  node up by source span. An annotation's lifted applications are now
  named by the op that spells the annotation, so the walk reaches them
  without shifting any operand position; the package format is
  `mojito-package 34`. No program's output or MIR changes.

- MIR lowers prefix and infix operators, comparison chains, ternaries,
  transfers, spreads, and lambdas from their checked ops (R635), as
  upstream's typed op carries its operator as an attribute and its
  operands as an ordered list: the operator comes from the op and each
  operand is the child at its position among the op's operands, and the
  transfer, spread, and walrus predicates and a `comptime if` condition's
  `not`/`and`/`or` decide by the op too. Every expression family now
  dispatches on its node, so an expression without one is a single
  explicit `Unsupported` boundary. No program's output or MIR changes.

- MIR lowers straight-line statements and `try` from their checked
  statement ops (R622), as upstream lowers a statement from its typed op:
  the op decides each statement's lowering, a `ref` declaration binds the
  slot HIR declared for it, a `try`'s bodies are its op's statement nodes,
  which HIR lowers directly rather than finding each by span, and the
  break/continue escape check and reassigned-name scan walk the ops. No
  program's output or MIR changes.

- MIR lowers list, dict, set, and tuple displays and comprehensions from
  their checked ops (R621), as upstream's lowering consumes a typed
  collection construction: the constructor and insertion method come from
  the node's construction adjustment, and each comprehension clause's
  kind, binder, and iterable from the op. A display without its checked
  construction is an explicit `Unsupported` boundary instead of a
  collection guessed from its delimiters, and a `comptime for`'s
  iteration is read through its statement node rather than by span. No
  program's output or MIR changes.

- A subscript accessor's compile-time value argument names its operand by
  bracket position rather than by source span (R641), as upstream's
  accessor call carries its parameters positionally: MIR reads the
  register at that position from the subscript's operand list and builds
  no span-to-register list. The augmented-subscript record drops its
  unused stored-value span, and the package format is
  `mojito-package 33`. No program's output or MIR changes.

- MIR lowers member reads, subscripts, slices, multi-subscripts, and
  `Variant` projections from their checked ops (R620), as upstream's
  lowering consumes a typed member or subscript op: the op decides, and
  the field name, the subscript arguments' kinds and keywords, and every
  adjustment are read from the node; the place builders, subscript
  stores, and projection predicates decide by the op too. A leftover
  probe that kept the bundled library from being packaged is gone. No
  program's output or MIR changes.

- MIR lowers literals and names from their checked ops (R618), as
  upstream's lowering consumes a constant or a value use as a typed op:
  each rebuilt expression's facts name its node, whose attributes are
  read in place, a literal or name read lowers by matching its node's
  `CheckedOp`, a name's slot is its binding's and its spelling is
  `name_spelling` alone, and MIR no longer makes up name syntax for a
  call through a local callable. No program's output or MIR changes.

- HIR lowers checked nodes (R617), as upstream's control-flow lowering
  consumes typed ops: `hir::Lower` walks each body's `CheckedStmtOp`s
  and `CheckedOp`s, a `HirExpr` names its node with its operands' trees
  and checked place, a `HirStmt` names its statement node, and
  `Cfg::slots` records the slot each local-name node resolves to, so HIR
  clones no syntax and copies no node attribute. MIR rebuilds the syntax
  it still lowers from the nodes, spelling each name as its slot. A
  `for` header branches on its has-next slot (`Terminator::BranchOnSlot`),
  and the unchecked `Cfg::build` builders are gone. A discarded call
  result that is a reference is no longer bound to a hidden slot, which
  the guard meant and never saw. No program's output changes.

- A package unit holds its checked nodes (R616), as upstream's package
  holds each body as typed `lit` ops: its expression and statement nodes,
  each carrying its own op, type, and attributes, and its declarations,
  rather than its syntax beside one fact record per occurrence. The
  importer installs them as the unit's checked program and rebuilds no
  arena. A struct member's unit records the symbol it lowers to and its
  struct's origin parameter names, and no longer holds the struct's
  parameter syntax. Decoding, installing, and lowering all 876 library
  units takes 123 ms release, from 160; the package is 1.32 MB, from
  1.16. The package format is `mojito-package 31`. No program's output
  changes.

- A statement is a checked node (R615), as upstream's is an op: a
  `CheckedStmt` holds its own `CheckedStmtOp`, its `StmtKind` with each
  expression a node and each body a list of statement nodes, so control
  flow stays structured, plus the binding it introduces, that binding's
  type, and its declaration. HIR and MIR read a statement's declaration
  and binding from its node rather than from a declaration table keyed by
  location, a lambda names its `def`'s node instead of keeping a cloned
  `def`, and a nested `def`'s declaration is no longer a linear search.
  Every program's output and MIR are unchanged, and so is the total
  compile time. HIR and MIR still find the node by location (R629), and a
  declaration's signature is still syntax (R630).

- A checked expression holds its own operation (R614), as an upstream op
  does: `CheckedOp` is the node's `ExprKind` with each child expression
  replaced by the child's node, so the arena no longer copies every
  node's whole syntax subtree and its operands are its child edges. HIR
  rebuilds the syntax it still lowers through the transitional
  `CheckedTables::syntax_of`. Every program's output and MIR are
  unchanged; building the library's arena from source takes 17.7 ms, from
  20.5 ms. A type operand is still its annotation syntax (R628).

- The checker writes each fact straight into its occurrence's one record
  (R613), as upstream sets a decided fact as an attribute on the
  occurrence's op: its 35 per-fact tables are one logged store of
  `NodeFacts`, which body carrying copies and an overload probe undoes as
  whole records, and the handoff is that store moved out rather than
  gathered from the tables. The package's bytes, every program's output,
  and the check's time are unchanged.

- An overload candidate is checked inside a probe (R612): every checker
  fact store journals what a probed write displaces, and closing the probe
  restores it and drops the keys it logged, so only the selected
  candidate's re-run leaves facts. The free-call overload loop no longer
  clones eight whole tables per candidate, and a losing candidate no
  longer leaves its types, records, or generic instantiation behind; an
  overloaded function value probes its candidates the same way. The
  package's bytes and every program's output are unchanged. A method's
  overload scoring is not yet probed (R626), and the checker's lifting
  memo and effect observations are not rolled back (R627).

- The check hands its facts over as one record per occurrence (R604, the
  handoff's half of its next stage): `DiscoveryResult` holds a `NodeFacts`
  per occurrence where it held 35 span-keyed tables, `CheckedProgram::new`
  takes it whole instead of 47 arguments, and the checked arena reads a
  node's facts from its occurrence's record. A package unit's records are
  these records, so an importer installs them as they decode instead of
  refilling one table per fact. The checker's own working stores stay one
  logged table per fact, since an overload probe saves and restores single
  tables per candidate (R612). The package's bytes are unchanged, and no
  program's output changes.

- A package unit holds the check's facts as one record per occurrence,
  as an op holds its own attributes (R604, first slices): the record names
  the occurrence once, numbered densely within the unit, gives a location
  only when it is not the occurrence's own node's (34 of 14,287 records),
  and lists its facts, each named by its table. A unit previously wrote
  one row per fact, each repeating the occurrence's file, span, and
  build-wide identity. A struct's parameter list is no longer repeated in
  every member's unit: the importer reads it from the struct's
  declaration record. The package is 1.16 MB, from 1.98 MB; decoding all
  units takes 20 ms release, from 24, and building their checked arenas
  about 10 ms more. The package format is `mojito-package 30`. No
  program's output changes.

- A type is a handle to shared, immutable storage (R585), as MLIR's `Type`
  is a pointer to storage its context uniques. `Ty` is one pointer to a
  node that holds its variant (`TyKind`, which code now matches through
  `Ty::kind`) and caches its hash and closedness; a node's children are
  handles too, and `Ty::update` is the one in-place write. The check's
  `ParamContext` uniques every type it records (`ParamContext::intern_ty`;
  `--timings` counts `types.interned` and `types.intern_hits`), except a
  type whose transfer effects or display spelling its equality ignores.
  The bundled package holds every type once in a type section its records
  refer to by index, as MLIR bytecode does, decoded the first time a
  record refers to it: the package is 2.0 MB, from 2.5 MB, and decoding
  the units takes about 25 ms release, from 30. `Ty` is 8 bytes, from 72,
  and a MIR place 80, from 208; an instruction stays 296, which its call
  variants' argument lists set. The package format is `mojito-package
  29`. No program's output changes.

- Every program imports the standard library's package (R605), as the pin
  always imports its `std.mojoc`: a program that declares a library name
  again, such as `struct List`, `def range`, `struct Slice`, or `struct
  String`, no longer drops the package and compiles the library from
  source. The entry's declaration takes the entry module's own linked name
  and shadows the library's there, so those programs now run as at the
  pin; declaring a name the program imports explicitly is the link's
  "invalid redefinition" error. `std.*` always resolves at the bundled
  library, never at a `std/` beside the entry or under a search root, and
  the flat `stdlib/` facades (`from list import List`, `from math import
  floor`), which the pin rejects, are gone. `check` and `own` compile
  through the driver, so they import the package too. An installed bundle
  ships the package and no library source; there `MOJITO_STD_PACKAGE=0`
  and `precompile` are errors. The package format is `mojito-package 28`,
  without the list of library names a program had to avoid.

- An installed compiler reads the standard library's package its bundle
  ships at `<prefix>/lib/mojito/std.mojoc` (R566), as upstream's toolchain
  ships `lib/mojo/std.mojoc`, and refuses to compile when it is missing or
  another build's; it no longer builds a package into the user's cache.
  The new `mojito precompile <stdlib>/std -o <path>/std.mojoc` writes it,
  as `mojo precompile` does, and `scripts/package-pliron` runs it. The
  package names every library file relative to the library's root and no
  longer records the root, so it reads wherever it is installed, and the
  build refuses one that names its root; it is keyed by the build id
  alone. The package format is `mojito-package 27`, and the package is
  2.5 MB, from 3.0 MB.

- The bundled library's package holds each body as checked (R546), as
  upstream's `.mojoc` holds each body's IR as its checks validated it: one
  record per lowering unit (`checked_unit::CheckedUnit`) holding the
  unit's elaborated syntax and its share of the check's facts, keyed by
  occurrences an encoding scope renumbers (`token::encoding_occurrences`).
  The importer lowers a unit (HIR CFG, MIR) when the elaborator first
  reaches one of its functions, then inserts drops and verifies, and the
  build refuses a package whose units, lowered alone, give any function
  other than the whole library's lowering. MIR lowering runs by unit
  (`mir::lower_unit`); the package format is `mojito-package 26`, and the
  package is 3.0 MB, from 1.47 MB (R604).
- A type bracket argument is compile-time data on its call (R601), as
  Mojo's call carries a type parameter as an attribute:
  `Array[UInt8, 16](fill=0)` and `external_call["strlen", UInt]` no longer
  emit a type-name string register. The checker's `ReifyTypeArgument`,
  MIR's `reified_type_argument`, `MirParamArg::binder`, a constructible
  type parameter's frame slot, and the elaborator's binding of a type
  parameter from a string are gone; the elaborator binds a type parameter
  from the call's instantiated arguments alone. `native::mono` drops every
  compile-time argument slot of a concrete call, a builtin's included, and
  the concrete verifier rejects any slot. MIR text is now 1.45 (a 1.44
  `param_arg`'s `binder` is read and ignored) and the package format
  `mojito-package 25`.
- A closed parameter argument outside a generic call's brackets is
  compile-time data on its call (R474), as Mojo's call carries a parameter
  attribute: a construction's bracket arguments (`Counter[4](1)`), a value
  parameter inferred from an argument's type (`size(Counter[4](1))`), and a
  string (`f["x"]()`, `external_call`'s callee) emit no register. A
  construction records the struct's arguments then its constructor's own
  (`record_construction_instantiation`), an inferred value rides the call's
  instantiated arguments (`InferredValueArguments` and
  `Flatten::inferred_param_arg_regs` are gone), and both backends read
  `external_call`'s callee from them. A `UInt` value parameter inferred from
  an argument type (`name(S[UInt(3)]())`, R450) now passes MIR verification.
- A function the check lifts from a body is named by its declaration and
  an index within it, `$comptime$<declaration>$c<k>` (R547), beside MIR's
  own `$comptime$<declaration>$<k>`, as upstream keeps a parameter
  expression inside the declaration that holds it. Before, a local
  `comptime` application was `$comptime$$<n>` and a display binding
  `$comptime$<binding>$<n>`, numbered by a count each compilation
  restarts, so the package build refused a library body that lifted one
  and the library compiled from source instead. `lifted_anonymous_name`,
  `lifted_binding_name`, and `LiftedOwner::Anonymous` are gone.
- A library trait's overloaded requirement no longer keeps the bundled
  library from being packaged (R597): a trait declaration records its own
  requirements, defaulted or not (`TraitDeclaration::requirements`), and
  an importer's lookup and the build's overload comparison read a
  `Trait.method` set from them, as upstream's trait declares each
  requirement. Before, a requirement with no default lowered to no
  function, and a defaulted member counted its receiver, so any overloaded
  library requirement refused the package. MIR text is now 1.44 and the
  format `mojito-package 24`.
- A package record keys nothing by a name its directory holds (R598): a
  projected struct's method residue names its method by its function's
  index in the record, whose name splits back into the method's name and
  overload qualifier (`symbol::method_of_symbol`); an effect summary names
  its function by that index; and a record's constant is the declaration's
  own, named by its row. The build refuses a record that would not join
  back. The bundled package shrinks by 10.8 KB, from 1,477,584 to
  1,466,816 bytes (debug); the format is now `mojito-package 23`.
- A module integer constant is folded once, by the check, before any
  declaration's symbol is spelled (R599), as upstream folds a constant once
  in its declaration: the checker's binder owners, the conformance oracle,
  and MIR's name lowering all read the check's value
  (`CheckedProgram::constants`), and the overload scan no longer folds or
  holds constants. The package format and MIR text are unchanged.
- A library module integer constant's value is held once in its package
  record (R596), exactly, in its declaration's lowered part, as upstream
  holds a constant once in its declaration's op: the checker's integer row
  and the elaboration's value are joined back from it on decode. The
  overload scan now folds a constant with the check's one exact folder, so
  `//` floors toward the divisor's sign and shifts fold in its spellings,
  and the build refuses a library whose scan, check, or elaboration fold a
  constant to other values. The package format is now `mojito-package 22`.
- A package declaration's record names nothing its directory holds (R581),
  as an MLIR bytecode op names its symbol through the one string section:
  its lowered part holds each function by its directory row, its struct and
  trait by the declaration's row, and no reference mask, which derives from
  the function's conventions; its checker residue writes each table's
  values without their keys, and each projection without its name, all
  restored from the declaration's directory keys on decode. The build
  refuses a record that would not join back. The bundled package shrinks
  from 1.53 to 1.48 MB; the format is now `mojito-package 21`.
- A package declaration stores no overload facts (R575): an importer finds
  a library name's overload set, as Mojo does, by looking up the reached
  declarations under the name, and its struct's fieldwise constructor from
  its declaration; the build refuses a library whose lowered declarations
  imply other sets than its check scanned. The package format is now
  `mojito-package 20`.
- A trait default's parameter defaults are now held once, by its trait
  member's declaration (R588): a conformer's wrapper adopts them, an
  evaluated default applied to the wrapper's instantiation of the member,
  instead of each conformer re-checking the default's syntax and lowering
  its own default function. MIR text is now 1.43 and the package format
  `mojito-package 19`.
- A trait default is now its trait's member `Trait.method`, one function
  over the trait's `Self` checked once after every module declaration and
  lowered in its trait (R551), as Mojo emits a default inside its trait,
  instead of a module function `Trait$default$method` placed ahead of its
  first conformer. A default's body may now call a function declared after
  it; the package format is now `mojito-package 18`.
- The bundled standard library is imported from a compiled package instead
  of being checked and lowered again in every compilation (R11), as Mojo
  imports its precompiled `std.mojoc`. The package is built once per
  compiler build and cached under `target/mojito-cache`; a compilation
  checks only its own bodies, and the elaborator decodes a library body,
  inserts its drops, and verifies it when it first reaches it. Hello World
  now compiles and runs in 0.15 s release (from 0.93 s) and 0.5 s debug
  (from 4.8 s). `MOJITO_STD_PACKAGE=0` compiles the library from source.
- A compilation reads none of the bundled library's source (R545): its link
  reads each library module from the package's bytecode record of what
  linking it produced, and no library body reaches the steps before
  lowering. Hello World compiles and runs in 76 ms release, from 140 ms.
  The package format is now `mojito-package 2`, a byte container whose
  module records are bytecode.
- An importer's check no longer registers the bundled library's
  declarations from their headers (R552): the package build's check
  exports each library module's resolved declarations as a bytecode
  declaration interface, and an importer's check installs those of the
  modules its link reached and checks only the program's own statements,
  as Mojo's importer takes a package declaration already resolved. Hello
  World's check falls from 25 ms to 3 ms release, and the whole run from
  76 ms to 51 ms. The package format is now `mojito-package 3`.
- An importer no longer decodes and copies every MIR declaration of the
  bundled library (R554): the package holds each declaration as its own
  bytecode record, the importer's lowering attaches the package's
  declarations to its program instead of copying them, and every phase
  looks one up by name and decodes it the first time, as Mojo's importer
  resolves a package declaration on first lookup. Hello World decodes 3
  of 942 declarations and compiles and runs in 33 ms release, from 51 ms.
  The package format is now `mojito-package 4`.
- An importer no longer prepares and elaborates the bundled library's
  declarations again (R553): each module record holds its declarations as
  the package build prepared them, each declaration interface holds its
  module's constants as the build's comptime elaboration resolved them,
  and an importer prepares and elaborates only the program's own
  statements beside them, as Mojo's importer takes a package declaration
  already resolved. Hello World compiles and runs in 20 ms release, from
  33 ms. `Compiler::compile_linked`, given a source link, now compiles the
  library from source.
- An importer no longer reads the whole bundled package, and no package
  part is MIR text any more (R557): the package is a directory followed by
  bytecode records, its bodies and interface included, and the importer
  maps the file and decodes a body from its own bytes when it first
  reaches it, as Mojo's importer reads a bytecode package on demand. The
  package shrinks from 8.0 MB to 1.7 MB; Hello World compiles and runs in
  16.5 ms release, from 19.5 ms, and a program reaching 70 library bodies
  in 75 ms, from 91 ms. The package format is now `mojito-package 5`.
- An importer's check no longer installs every reached library module's
  declarations into each pass and into the elaborator's conformance oracle
  (R555): the package holds one declaration interface per library
  declaration under a directory of its table keys, and a lookup the
  program's own declarations miss decodes the record the first time, as
  Mojo's importer resolves a package declaration on first lookup. Hello
  World compiles and runs in 14.1 ms release, from 17.0 ms. The package
  format is now `mojito-package 6`.
  The elaborator's conformance oracle shares those records with every
  checker pass, so a library declaration decodes once per compilation for
  all of them (R558).
- An importer's check no longer answers every reached library struct's
  conformances again (R556): a library struct's rows are the package's,
  the link gives it an empty row for each trait the program declares, and
  the check answers only the program's own structs, as Mojo's importer
  decides only its own types' conformances. Hello World compiles and runs
  in 8.8 ms release, from 12.4 ms.
- An importer no longer reads and digests the bundled library's source to
  find its package (R562): the compiler build's identity now covers the
  library's source, and the package cache is keyed by that identity and
  the library's root, as Mojo's importer consults no library source.
  Editing the library takes a compiler rebuild, which every `cargo`
  command does. Hello World compiles and runs in 7.8 ms release, from
  9.0 ms. The package format is now `mojito-package 7`.
- An importer's link resolves a bundled library import against the
  package's module table, now keyed by dotted module name, instead of
  probing and canonicalizing the library's files (R565), as Mojo's importer
  finds a module in a package. Hello World makes 2 `readlink` calls instead
  of 1593 and runs in 8.5 ms release, from 10.2 ms. The package format is
  now `mojito-package 8`.
- An importer's front end reads a bundled library struct, function, and
  SIMD alias as the package resolved them (R559): its elaboration asks the
  conformance oracle for a library struct's parameters, fields, and
  associated members and a library `def`'s parameters, and its `prepare`
  reads the library's SIMD aliases from its elaborated constants, as
  Mojo's emitters read a package declaration resolved. A module record now
  keeps only its trait declarations, which a conformer's default wrapper
  still clones (R568). Hello World compiles and runs in 6.1 ms release,
  from 8.2 ms, and the package shrinks from 1.74 to 1.64 MB. The package
  format is now `mojito-package 9`.
- A conformer's wrapper for a trait default it inherits is built by the
  check from the default's resolved signature, which the trait's check
  records, instead of cloned from the trait's syntax at `prepare` (R568),
  as Mojo's `synthesizeDefaultTraitMethodWrapper` builds it from the trait
  function's signature. The wrapper is a member with no syntax of its own,
  which MIR lowers beside the struct's spelled methods, so a package's
  module record now holds no declaration and an importer reads every
  library declaration from the declaration interfaces. A default whose
  signature names an origin is now rejected at its conformers (R571). The
  package format is now `mojito-package 10`.
- A package records each library declaration once (R560): one record per
  declaration under one directory holds its MIR struct and function
  declarations and its callables' effect summaries, which every phase of
  an importer decodes once per process, followed by only the checker rows
  those do not say, as Mojo's package holds a declaration in one op every
  phase of its importer reads. The package build compares each struct's
  facts, method signature, and `def` type with its MIR declaration byte
  for byte and refuses the package on a difference. A MIR struct
  declaration now carries every named destructor and its
  explicit-destruction obligation whole (MIR text schema 1.41). The
  package shrinks from 1.64 to 1.56 MB; the package format is now
  `mojito-package 11`.
- An importer copies nothing of the bundled package in when a compilation
  starts (R563): a declaration's record now holds the overload facts of
  its names, its module constant as the build's elaboration resolved it,
  and its module-scope compile-time facts, and the check, the
  elaborator, and MIR answer a name their own tables miss from the record
  of the reached declaration owning it, decoded on first lookup, as
  Mojo's importer resolves a package declaration when a lookup names it.
  No library module has a checker record of its own any more, and MIR
  shares one copy of the program's overload sets instead of one per
  function. Hello World compiles and runs in 4.0 ms release, from 4.4 ms
  (`package.interfaces` 0.19 → 0.02 ms, `elaborate` 0.27 → 0.11 ms). The
  package grows from 1.55 to 1.57 MB, and its format is now
  `mojito-package 12`.
- A struct's conformance to a trait is decided when a clause asks it
  (R564), as Mojo decides it from the type's declared conformances and the
  trait refinement graph: a struct declaration carries its declared
  conformances and its lifecycle rows, and a trait declaration its
  parents, and no compilation answers a struct for a trait ahead of use.
  An importer no longer answers every reached library struct for a trait
  the library never asks (`Absable`), and the package no longer records
  the traits its bodies ask: a program asking one compiles 15% faster
  (`check.conformances` 4.2 → 0.02 ms). MIR text schema 1.42 adds a
  struct's `declared` list and each trait's parents; the package format
  is now `mojito-package 13`.
- A package lists each library module once (R572): the declaration
  directory's module table holds every module the build's link recorded,
  by dotted name, with its source paths and link record, as Mojo's
  package holds one module tree its importer's link and front end both
  read. The link reports the modules it reached by their table positions,
  every declaration names its module by position, and the interface keeps
  only the package's declared names. The package shrinks from 1.57 to
  1.56 MB, and its format is now `mojito-package 14`.
- A package names each library function once, and an importer reads its
  declaration directory in place (R573): one string section holds every
  name once, as an MLIR bytecode string section does, and one row per
  function holds its name, its declaration, its signature masks, and its
  body's range, as an MLIR function op names its symbol once and holds its
  region. A lookup binary-searches the mapped rows, the check and the loan
  analysis ask a package callee's facts on lookup instead of indexing all
  of them, and Hello World runs in 3.2 ms release, from 4.1 ms. The package
  shrinks from 1.56 to 1.54 MB, and its format is now `mojito-package 15`.
- The elaborator finds a callee's overload set by looking its name up
  (R580), as upstream's symbol table does, instead of scanning every
  function of the program and the bundled package: a call's family is one
  range of the package's by-name function table beside the program's own
  bodies, kept by name. Hello World's elaboration falls from 1.61 to
  1.48 ms release, and a method-heavy program's by about 9%.
- Drop elaboration and the move-state check behind it run as worklist
  dataflows over live state only (R582): register-loan states forget dead
  registers, each block's deaths come from one backward walk instead of
  per-instruction snapshots, and the move states, register loans, and drop
  liveness visit blocks in reverse postorder instead of round-robin. An
  importer still drop-elaborates each library body it decodes, as Mojo's
  importer runs its lifetime pass on a package that holds bodies before
  destructor insertion, but finishing all 874 bodies takes 62 ms release,
  from 161 ms (`StringSpan._grapheme_class` 23.6 → 1.9 ms), and a
  method-heavy program's elaboration falls from 14.0 to 10.9 ms.
- A type and an instruction keep their large, rare payloads behind a box
  (R584), as MLIR keeps an operation's storage proportional to what it
  holds: a callable type's signature is one boxed `FuncTy`, shared by the
  generic form, and an instruction's optional places, subscript calls, and
  call-only types are boxed. `Ty` shrinks from 256 to 72 bytes and
  `MirInstr` from 1904 to 296, each size asserted. Decoding all 874
  bundled library bodies falls from 32 to 18 ms release, finishing them
  from 64 to 47 ms, and `self_hosted_dict.mojo` runs in 51 ms from 64 ms.
  The package format is now `mojito-package 16`.
- A library method's parameter defaults are held once in the package, as
  its lowered declaration's constants, instead of again as checker syntax
  (R574): an importer's check reads a package method's defaults from its
  declaration, as Mojo's signature holds a default once. The package
  build refuses a method whose checked defaults and declaration differ.
  The package format is now `mojito-package 17`.
- A module under a search root is linked under its path there, whichever
  import reaches it first: `std/io/file.mojo` declares
  `__module$std$io$file$…`, not `__module$file$…`.

### Fixed

- An assignment inside a `try`, `except`, `else`, or `finally` body to a
  name a nested scope redeclares (`var h = 2` inside an `if`, then `h = 5`
  in a `try` there) writes that inner variable, as Mojo's lexical scoping
  does (R636). It used to write the outer same-spelled variable. HIR now
  resolves a name to its checked binding's slot, a `try` region starts from
  its frame's binding slots, and HIR declares an `except` name's slot.
- A module constant of a one-lane SIMD type — declared (`comptime S: UInt8
  = 2`, the bundled `SEEK_END`), folded from a call (`comptime C =
  hash(Int(1))`, a `lane(...) -> Int32` application) — compiles natively and
  prints what the VM and the pin print, where IR verification used to stop
  it: the native backend binds the folded value as its lane scalar, not a
  pointer to storage (R654; also fixes R541 and R576).
- A static call through a type name (`Point.origin()`) calls the static
  even after a block's local spelled `Point` has gone out of scope, as at
  the pin, where it used to call the method on the dead local's value
  (R619). MIR now decides calls, invocations, and method calls by their
  checked node rather than their syntax, reads a call's loan transfers from
  its node, and names a folded bracket argument by its position (package
  format 32). `assets/ok/static_call_through_shadowed_type_name.mojo` pins it.
- A call or read through a local callable names the declaration its
  name resolves to (R637): a `var f` declared in a block shadows an outer
  `f` only inside that block, and once a block's `var g` is out of scope
  `g` names the module function again, as at the pin, where both used to
  reach the outer or dead slot by spelling.
  `assets/ok/shadowed_local_callable_call.mojo` pins it.
- A static member on a `Tuple` type over a pack still open
  (`Tuple[*Ts].__len__()`, also over `*Ts.reverse()`, a
  `TypeList._concat`, and a struct's `Self.Ts`) runs, served by `Tuple`'s
  own `@staticmethod`, and `assets/ok/tuple_static_len_open_pack.mojo` pins
  it (R347). It has run since `Tuple` became a struct generator.
- A `@fieldwise_init` struct whose copy or move constructor is spelled
  `__init__(out self, *, copy: Self)` keeps its fieldwise constructor, so
  `P(y=2, x=1)` runs as at the pin instead of failing with "keyword
  arguments to 'P' are not supported".
- A bundled library function used as a value in an importer (`var g =
  next_pow2`, or passed to a `def` parameter) runs instead of failing with
  "'None' is not callable": the importer's check keeps the package's
  overload sets and applicable functions beside its own (R555).
- A trait default is checked once, in its trait, and a conformer that does
  not spell it gets a forwarding wrapper that calls it (R544), as upstream's
  `synthesizeDefaultTraitMethodWrapper`: a default calling a method only the
  conformer has, or an ill-typed default no struct conforms to, is now
  rejected as at the pin; overloaded defaults are inherited per overload
  instead of failing conformance; conflicting defaults, a default parameter
  named like a struct parameter, and `Trait.method(x)` are rejected with the
  pin's words; and an importer no longer checks the library's `Writer.write`.
  A free `def` parameter named `self` is now writable by its own convention,
  and a keyword collector in a trait method links the library's `StringDict`.

- A local `comptime` display binding, a `Dict[...]`/`Set[...]` constructor
  binding (module or local), and a closed `reflect[P].field_names()` list
  are applications the check binds (R539): `comptime s = total(l)` over a
  local `comptime l = [1, 2, 3]`, `size(d)` over a local `{"a": 1}`,
  `comptime(total(l))`, and `comptime N = size(D)` over a module
  `Dict[String, Int, H](["a", "b"], [1, 2], None)` or `Set[Int, H](1, 2, 3)`
  run as at the pin instead of failing with "Undefined variable", an
  unannotated `comptime e = {}` reports the pin's own error, and the
  elaborator no longer spells any collection into a read.
- A display argument to a generic call is typed by its parameter (R540):
  `Dict[String, Int](["a"], [1], None)`, `W[String](["a", "b"], None)` over
  `var keys: List[Self.K]`, a fieldwise `List[Self.K]` field, and
  `f[String](["a", "b"])` construct `List`s as at the pin instead of
  failing with "no constructor overload matches" or "expected List[String],
  found Array[String, 2]", and an inferred element parameter is solved from
  the display (`f(["a", "b"])`, `W(["a", "b"], None)`, `{"a": 1}` against
  `Dict[K, V]`).
- A closed module constant is a checked parameter value at every read
  (R517): a list, dictionary, or set display is one compile-time
  application, so `comptime S = total(L)` over `comptime L = [1, 2, 3]` and
  `comptime N = size(M)` over a dictionary display run as at the pin instead
  of failing with "Undefined variable", and a `DType`, vector, tuple, or
  struct constant is a folded parameter value, so the elaborator no longer
  spells any module constant into a body. An annotated list constant
  (`comptime L: List[Int] = [1, 2]`) read by a lifted initializer, a module
  alias over a `DType` constant (`SIMD[D, 4]`), and a `DType` constant as a
  parameter default now work, and a raising dictionary subscript in a
  `comptime` binding of a raising body reports the pin's error instead of
  failing MIR verification.
- A `Pointer` element of a `Tuple` or `List` keeps its handle (R529):
  `t[1][] += 2` over `(Pointer(to=left), Pointer(to=right))`, and the same
  through a `Tuple[Pointer[Int, o1], Pointer[Int, o2]]` struct field, print
  what the pin prints instead of failing with "expected reference handle" on
  the VM or crashing natively. A reference returned by a call that ends at a
  stored pointer now designates that pointer on both backends.

- A generic closure that captures, bound to a callable parameter
  (`invoke[captured_add]()` with `callback: def[n: Int](Int)
  capturing[origins] -> Int`), now elaborates and prints what the pin prints
  (R526): the instance folds the closure's body, takes its by-reference
  environment as trailing parameters, and calls the body's specialization
  directly, on the VM and natively.

- A module display constant that applies a callable — a dictionary or set
  one included — is one compile-time application, evaluated once, as at the
  pin, instead of being respelled (and re-run at run time) at each read
  (R516). A lifted constant declared after the body that reads it, an alias
  of an applied constant (`comptime E = D`), `len(D)` or `"a" in D` in a
  `comptime` binding, and a module `comptime C = M.copy()` over a closed
  collection now compile; a run-time `T[1]` borrow of a tuple constant's
  `String` element no longer outlives its materialized temporary.

- Regressions the 2026-10-09 overnight gate found after the cloner and the
  AST compile-time route were deleted (R9, R10) and pointer-backed results
  landed (R196). The VM indexes an elaborated `Tuple$mono$…` result
  (`divmod`, `Slice.indices`); a callable struct passed as a function value
  keeps its `__call__`; a generic `__setitem__[T]` and a nested generic
  `def` over a pack instantiate, a nested call recording its `*args^` spread
  (MIR text schema 1.40); a closed bracket argument on an indirect call keeps
  its register; a struct-returning static method in a type argument
  (`Tagged[Extent.square(4)]`) is evaluated by the elaborator; a temporary
  bound to a `ref` field constructor stays borrowed; an elaborated type name
  spells its template (`AHasher[...]`); a module constant no longer leaks
  into a stdlib local of the same name; and the checker again reports the
  pin's rejections for a bare struct value, a non-`Deinitable` pack field, a
  walrus onto a function name, and a captured pack moved out whole.
  Residues: R526–R531.

### Added

- VM compile-time results now retain pointer-backed storage (R196).
  Optional values, nested structs, lists, dictionaries, and sets materialize
  into fresh allocations in the VM and Pliron, preserving uninitialized
  capacity. Compile-time queries and explicit collection materialization
  work; implicit non-copyable reads still reject. Frozen pointers round-trip
  in MIR schema 1.39.

- The replaced mechanisms are deleted (roadmap R10, P5): the AST cloner
  (`comptime/{mono,specialize,packs,unparse}.rs`), the checker's template
  derivation (`checker/template_facts*`, `mojito-checked/src/templates.rs`,
  `MOJITO_VERIFY_TEMPLATE_FACTS`), the VM's erased dispatch and the erased
  oracle (`--erased`, `MOJITO_VM_ERASED`, `VmInstantiation`,
  `Backend::run_elaborated`, `run_artifact_as`, the corpus `erased_vm`
  group), and the stage-composed `Backend::run(&CheckedProgram)` seam with
  the `mojito-vm → mojito-analysis` crate edge. Every generic `def` is a
  template the one check infers once and `native::mono` instantiates below
  MIR, a `Tuple`- or struct-typed value binder included; the VM runs
  concrete MIR only, reporting a parametric instruction as an invariant
  error, and a runtime struct value carries no compile-time arguments. The
  check runs in a `CheckContext` (`check_program_in`), every executing test
  goes through `Compiler`, and the corpus binary's `artifact` group compares
  the production run with the serialized artifact's. Residue: R521.

- Source validation and the executable check are one check, every
  function body keeps its `comptime if` and `comptime for` to MIR, module
  constants are parameter expressions, and the AST CTFE route is gone
  (roadmap R9, P4). The check types every arm and loop body once, plain
  or generic, with its binders symbolic, folds a closed condition to its
  verdict, and `native::mono` decides and unrolls. A scalar or applied
  module constant is read as a `ParamValue` fact lowered as `Const::Param`
  (a lifted display constant as `$comptime$<name>$module`), the toplevel
  lowers no `comptime` declaration, and the pending/forced machinery,
  `CtMarker::Layout`, and `ParamKind::Apply::evaluated` (MIR text schema
  1.38) are deleted with `validate_comptime_templates`,
  `VmBackend::run_function_value`, and the `mojito-vm → mojito-checker`
  and `mojito-comptime → mojito-vm` crate edges. A `comptime if` over a
  runtime local reports the pin's "cannot use a dynamic value in 'comptime
  if' condition", a named destructor no longer defeats trivial
  deinitialization, and R139's folded identity is fixed. Residues: R516,
  R517, R518, R519.

- The compiler driver elaborates once and checks once: the discovery
  rounds that re-elaborated and re-checked every program with the
  instantiations the previous check recorded are gone, with their request
  plumbing, the divergence diagnostic, the cross-round body-fact carry, the
  clone census, and `--instantiation-census`. The instances a program needs
  are found below the MIR waist by the elaborator's worklist, as at the
  pin. A checker-selected scalar `range(Int32(4))` is now spelled as the
  linked range struct's construction in the same check, so it no longer
  waits for a second round. The release frontend of every compile
  benchmark drops about 15% (`docs/performance.md`).

- No method of a generic struct is cloned per instance any more, and no
  `def` is compile-time-keyed, as at the pin: every `comptime for` in a
  generic body stays in its template and is unrolled per instance below
  MIR. A loop bound that reads a type's value member through a binder
  (`range(T.K)`, `range(Self.T.K)`) or a field of a struct-valued parameter
  (`range(Self.e.w)`) is now a parameter expression, which fixes a method's
  own binder failing with "'T' is not a compile-time type" and a
  struct-valued instance aborting with "unspecialized type-keyed method"
  at run time. A loop over a module constant list of struct values forces
  the constant at the header. The per-instance cloner, the driver's
  instance requests and template-reach analysis, and the clone-symbol
  lookups in the checker, the VM, and `native::mono` are deleted.

- A module constant whose initializer is a display, a tuple, or a subscript
  that applies a callable (`comptime QS = [Q(1, "x"), mk(7)]`,
  `comptime T = (f(1), "a")`, `comptime Y = XS[1]`) now waits for its first
  demand like every other applied constant, instead of being folded where
  it is declared. A body's field read of a pending constant (`q0.s`) is a
  compile-time projection inside its request, and a body's read that
  would materialize a collection or a struct that is not
  `ImplicitlyCopyable` (`print(XS[1])`, `var x = q0`, `comptime(mk(3))`)
  is now rejected with the pin's "cannot materialize comptime value"
  message; `var x = q0` used to run. A lifted compile-time initializer now
  copies a field or element of a temporary out before dropping it, which
  fixes `comptime first = [Q(1, "x"), mk(7)][0].s` failing VM CTFE with a
  raw pointer and `comptime q = [Q(1, "x"), mk(3)][1]` being rejected as an
  implicit copy, and a tuple value holding a `String` (`comptime T =
  (f(1), "a")` in a body) is now constructed at its `Tuple` instance.

- No compile-time evaluation in a function body runs on the AST route any
  more, as at the pin: an application of a `def` generic over a type
  (`comptime c = capacity[Buffer[8]]()`, whose instance reads `T.size`), a
  struct construction (`comptime p = P(3)`), a display, a tuple, or a
  subscript whose operands apply a callable (`comptime t = (f(3), 4)`),
  and a method of a compile-time value (`M.get("a").value()`,
  `S.byte_length()`, `E.get()` over a pending constant) are requests the
  elaborator below MIR serves, the value spelled by its literal form so the
  request constructs it. The elaborator and the checker now draw that line
  with one shared predicate. An annotated module constant that applies a
  callable waits for its first demand too, and every such constant is
  typed where it is declared, read or not, so `comptime D: Float64 = f(2)`
  and an unread `comptime W = f("x")` are rejected as at the pin. A
  pending module constant's name no longer leaks into a body that binds a
  local of the same name (an unread `comptime p = f(9)` failed every
  program), a compile-time expression over a requested binding
  (`comptime(a + 1)`) now evaluates, and a request whose result is a
  `Dict` or a `Set` is rejected with the VM-CTFE crossing message. The
  bundled `stdout`/`stderr` constructions are now pending constants, which
  takes a debug build's `print(..., file=stderr)` from 36 to 7 seconds.

- A type's compile-time value member reads at run time, as at the pin:
  through a bound type parameter (`print(T.K)`, `Self.T.K`, `comptime k =
  T.K + n`), on a struct by name (`A.K`, `B[4].K`, `V[A.K]()`), in a
  struct's method (`Self.K`), and by the leading-dot form (`return .RED`).
  A read through a type parameter is a new parameter-expression node the
  elaborator answers per instance from each struct's checked value
  members, so a member computed from the instance's own parameters
  (`comptime K = Self.n + 1`) reads its instance's value, on the VM and
  natively. A string literal now witnesses a `comptime NAME: String`
  requirement, and a read of a member the bound does not require is the
  pin's "'HasK' value has no attribute 'Z'". MIR text schema 1.36 adds a
  struct declaration's `associated_values` and the `param_type_member`
  node.

- A nested `def` or lambda declared in a `comptime for` body reads the
  loop's index, as at the pin, in a generic `def`, a plain one, and a
  method: each unrolled iteration declares its own, specialized to its
  index, whether the nested `def` is plain, generic (`inner[5]()`),
  `@parameter`, or capturing, and its signature may name the index. A
  nested `def` in a kept `comptime if` arm of a generic `def`, which was
  rejected, runs too. A lambda in a plain body's loop, which read the
  first iteration's index in every iteration, now reads its own. MIR text
  schema 1.35 adds a declaration's `region_binders`. The erased oracle
  reads a `comptime for` index whatever spelling its slot took, so a
  second loop reusing `i` decides its `comptime if`.

- A method with compile-time parameters of its own is now always served by
  its template, as at the pin: the per-call clone path, the checker's
  retargeting to its clones, and the driver's method requests are deleted.
  A `comptime for` holding a binding with a converting annotation
  (`comptime t: String = "s"`, which also failed MIR verification in any
  body), a range bound or binding applying a function whatever its result
  (`range(len(mk(n)))`, `comptime e = mk(n)[i]`), or iterating a sequence a
  compile-time application builds (`comptime for x in mk(n)`, or over
  `comptime l = mk(n)`, which failed in a plain `def` too) is now evaluated
  per instance below MIR, in a method or a `def`. A runtime read of such a
  `List` binding is rejected with the pin's "cannot materialize" message.
  `range(T.K)` over a trait's value member in a method, which ran on a
  per-call clone, is now rejected where called, as `comptime if T.K` already
  was (R500).

- Every `def` keyed on a type pack is now served by its template, as at
  the pin, whatever its body holds: the cloner's type-pack branch, its
  served-shape gate, and the signature-only stub a deferred pack call
  checked against are deleted, and no type-pack `def` is cloned. A
  `comptime for` display element reading a field off a method's result
  (`[P(n).twin().v, n]`), and a loop-body binding applying a `def` to the
  index (`comptime w = twice(i + 1)`), are now evaluated per instance
  below MIR, beside a pack or not, where they were cloned.

- A reflected field-name list materialized whole over a type parameter
  now runs from the template, on the VM and natively, as at the pin: `var
  all = materialize[names]()` over `comptime names =
  reflect[T].field_names()` in a generic struct's method stopped with
  "materialize[...]() of a reflected list over a type parameter", and in a
  generic `def` was cloned per instance. The list is the `Array` of the
  names sized by the field count, which the elaborator constructs per
  instance from its parameter value. A query called in place (`var names =
  reflect[T].field_names()`), rejected before, is the same `Array`.

- A value pack read as a runtime value now runs, on the VM and natively, as
  at the pin: `for v in values`, `values[i]` at a runtime index, and `var l
  = values` over `def f[*values: Int]` (or `Bool`, `Float64`, `DType`, a
  method's own pack, or a struct's `Self.values`) stopped with "type
  'Tuple[Int, Int, Int]' has no method '__iter__'" or "expected a
  compile-time Int index". Such a read is the bundled `ParameterList` the
  pin types it as, now in `std/builtin/variadics.mojo` with its iterator,
  whose elements live in a constant array laid out once per program (MIR
  text schema 1.34 adds `param_list.address`). A `def` that reads its pack
  this way is no longer cloned. `Span`'s element bound is upstream's
  `AnyType`, and a struct whose own value pack appears in its `Self` type
  (a method returning `Self`) no longer fails MIR verification. A bracket
  spelling of `ParameterList` in user code is rejected: the pin takes the
  list as one `values.values` parameter, which is not supported yet.

- A compile-time evaluation in a function body is now requested from the
  elaborator's worklist, as at the pin: a local `comptime` binding, a
  `comptime if` condition, a `comptime for` range bound, or a
  `comptime(...)` operand that applies a callable, in a generic body or
  not. A binding over a binder (`comptime x = f(k)` in `def g[k: Int]`)
  no longer calls `f` at run time on every call, and a binding may hold a
  `String`, a tuple, or a struct (`comptime s = label(3)` through a
  t-string or a `Tuple`, which stopped with "TString.write_to:
  unspecialized type-keyed method"). A module constant whose initializer
  applies a callable is evaluated on demand: a body's read of it is such a
  request, so one that calls a generic struct's method holding a
  `comptime if` (which stopped at "Cell.m: unspecialized type-keyed
  method") or a value-keyed `def` recursing under one (which stopped at
  "unknown compile-time function 'rep'") now runs, natively too; a type
  that reads it forces it once, and one nothing reads is never evaluated.
  A constant whose evaluation demands itself is now upstream's "function
  instantiation in parameter domain that recursively requires itself"
  (was "Undefined variable"), a keyed recursion under a runtime `if` stops
  at the instance budget instead of printing a value, and a binding that
  reads a runtime value or calls a raising function is rejected as at the
  pin (`comptime a: Float64 = f(1)` over an `Int` result is rejected too).
  A demand runs only its instance's reference closure, and its effect
  check follows method calls, closures, and subscripts, so a `print`
  reached through a method is rejected rather than silently dropped. A
  tuple-valued parameter constant now materializes as its `Tuple` instance
  on the VM, which fixes `assets/ok/comptime_for_aggregate_binder.mojo`.

- A `t"…"` literal is now written by the checker in one pass, as at the
  pin, as a call of the bundled `__make_tstring` entry point, named by
  module path rather than looked up in user scope. Previously a driver
  request was rewritten in the next discovery round. A t-string in a
  template-served generic body or a generic struct's method now runs on
  the native backend, where it failed with "unsupported runtime
  StringLiteral operand". Interpolating a `Copyable` place that is not
  `ImplicitlyCopyable` (`t"l={l}"` over a `List[Int]`) no longer fails
  with "cannot be implicitly copied (argument 2 to 'TString')". A literal's
  type is now `TString[String, …]`. A t-string inside a function evaluated
  at compile time now stops as a `Tuple` there does (R486), where it ran
  through MIR's eager concatenation fallback, which is gone.

- A generic struct's method or a generic `def` whose `comptime for` body
  binds an annotated `comptime` (`comptime m: Int = i * Self.n`,
  `comptime b: Bool = i > 0`) or an element of a closed list at the index
  (`comptime q = xs[i]`, `[1, 2][i]`), or whose `comptime if` reads a local
  type alias of a binder (`comptime U = Self.T`), is now served by its
  template, as at the pin, instead of cloned per instance. A named list
  read in such a binding failed with "Undefined variable 'xs'" and the type
  alias with "the instance does not decide"; both now run. A read past the
  display fails the instance only where it reaches the read, and an
  annotation that is not the value's type is rejected, as at the pin.

- A template-served generic body's call over its own binder into an
  overload family that keeps a member the cloner specializes (`kind(a)` in
  `inferred[dt: DType]`, `kind[n](a)` in `by_value[n: Int]`) now runs, as at
  the pin, served by the declaration the checker selected. It was rejected
  with "requires compile-time parameter 'T'", naming a sibling's parameter,
  or failed MIR verification with "value parameter 'n' has no runtime
  register", which a losing overload's probe of `n` as a type also caused
  outside a served body (`kind[T](1) + kind[n](a)`).

- A `DType`- or lane-keyed `def` is now an ordinary generic `def`, served by
  its template unless its body keys a clone, as at the pin. A value binder
  a runtime parameter names outside a lane slot (`h[dt: DType, n: Int](a:
  Box[n], b: Scalar[dt])` called as `h(Box[3](0), Int32(1))`), or an erased
  origin beside the lane (`o2(a)`), was rejected with "requires
  compile-time parameter 'dt'" and now runs; a type or pack binder beside
  an inferred value is served too. A vector-valued binder now reads on the
  native backend.

- An overloaded `DType`- or lane-keyed `def` (`kind[dt: DType](a:
  Scalar[dt])` beside `kind(a: String)`) is now served by its template per
  declaration, as at the pin, instead of making its name a template family
  that cloned every member per call; a call selecting it in a family that
  keeps a compile-time-keyed member is left as written.

- A local `comptime` binding of a compile-time parameter in a `DType`- or
  lane-keyed `def` (`comptime lane = dt`) is now an alias of the parameter,
  as at the pin, and the template serves the body: a read in a type, at run
  time, inside a `comptime for`, or as a `comptime if` receiver
  (`lane.is_floating_point()`) no longer keys a clone per call. The same
  binding of an `Int` parameter (`comptime k = n`) in any generic `def`,
  which was rejected as a generic alias outside module scope, now runs.

- A `comptime for` over tuple or struct elements holding a `String` in a
  generic `def` or a generic struct's method (`comptime PAIRS = [(1, "a"),
  (2, "b")]`, a literal display, `[(n, "c"), (4, "d")]`, `[Q(n, "z"),
  mk(n)]`) now keeps one loop in the template, its variable a compile-time
  parameter, as at the pin, where the elaborator copied the body per
  element or keyed a clone. Each run-time use of the variable — a read
  argument, a copy, a field read, a method receiver — constructs the
  element afresh into a temporary the ownership analysis destroys, and the
  variable's own slot owns nothing. The erased oracle now materializes a
  tuple element at its checked type.

- A compile-time struct value with a `String` field, or a nested fieldwise
  struct holding one, now works as at the pin: a display `comptime QS =
  [Q(1, "x"), Q(2, "y"), mk(7)]` iterated by `comptime for`, a module or
  body `comptime r = Q(3, "z")`, `W(Q(4, "in"), "out")`, and a field read
  bound as `comptime`, on the VM and natively. It was rejected with "a
  compile-time 'String' value needs fieldwise construction". The
  elaborator now freezes every compile-time evaluation result with the
  VM's own freezer instead of a weaker copy that read a `String` only at
  the top level.

- A tuple or struct value parameter with a `String` element or field now
  works as at the pin: `g[(1, "a")]()` against `def g[p: Tuple[Int,
  String]]()`, a default `(7, "d")`, a named `g[p=(1, "k")]()`, a nested
  `Tuple[String, Tuple[Int, String]]`, and `g[Q(1, "x")]()` over a struct
  with a `String` field, read whole, by element or field, and copied and
  mutated, on the VM and natively. The literal argument was rejected as
  `Tuple[Int, StringLiteral]`, and a struct or default value failed MIR
  verification. The checker now types the argument against its declared
  type as a run-time argument, and each instance constructs the value
  into the parameter's slot instead of folding it to a constant.

- A `comptime for` over tuple or struct elements in a generic body now keeps
  one loop in the template, as at the pin: `[(1, 2), (3, n)]`, a module
  `PAIRS` or `PS = [P(1, 2), P(3, 4)]`, `[P(1, n), mk(n)]`, a local
  `comptime L = [(1, n)]`, nested tuples and struct fields, and the same
  loop in a pack-keyed `def` or a struct method, where each was unrolled in
  the AST or cloned per instance. The variable is a compile-time parameter
  of the element's type, passed whole, copied, subscripted, and read by
  field, method, and `comptime if`, on the VM and natively. A tuple-typed
  value parameter (`def g[p: Tuple[Int, Int]]`, `g[(1, 2)]()`), which failed
  MIR verification, now runs too. Elements with a `String` leaf are still
  unrolled in the AST.
- A `comptime for` display element that calls a method now runs from the
  generic body's template, as at the pin: `[P(n).get(), n]`, `[S.g(n), n]`,
  `[S[n].g(n), n]`, `[String(n).upper(), "x"]`, a method on a local
  `comptime` receiver, and a struct method's `[Self.g(Self.n), Self.n]`,
  where each was cloned per instance or failed ("compile-time method 'g'
  needs a value receiver", "a compile-time 'String' result cannot cross
  back from VM CTFE", "unspecialized type-keyed method"). Source validation
  now decides which display calls return a scalar, from their checked
  types, instead of the callee's declaration.
- A value parameter typed by an earlier type parameter now works as at the
  pin: `struct S[T: ImplicitlyCopyable & Writable, //, v: T]` with
  `S[3]().get()`, `def f[T: …, //, v: T]() -> T` with `f[True]()` and
  `f[2.5]()`, the value pack `*vs: T` (the shape of upstream's
  `ParameterList`, solved from a forwarded `P[*xs]` too), and the method form
  `def pick[v: Self.T](self)`, where each stopped on "unknown trait 'T' in a
  type-parameter bound" or a parse error. The infer-only `T` is solved from
  the value, an argument type, or a default, and in CTFE; a later value of
  another type and a type outside `T`'s bounds are rejected. A value that
  solves `T` to `String` is still rejected explicitly. A `UInt` value
  parameter (`def g[n: UInt]() -> UInt: return n`) now reads at run time,
  where it stopped on "unsupported runtime value parameter".
- A struct's own value pack now reads in its methods as at the pin:
  `len(Self.values)`, `Self.values[i]` at a compile-time index,
  `materialize[Self.values[i]]()`, and `comptime for v in Self.values`,
  where each stopped on "'Self.values' is not a type parameter of the
  enclosing struct". A value pack still a parameter now spreads whole into
  another application's brackets — `Pack[*vs]()`, `total[*vs]()`,
  `Sink().count[*vs]()`, `Pack[*Self.qs]()` — where it stopped on "expected
  a value, found a type". `materialize[Self.n]()` over a struct's scalar
  value parameter runs too. A runtime read of a `def`'s whole value pack
  (`for v in values`, a runtime index) is still rejected: upstream's
  `ParameterList` needs a value parameter typed by a type parameter, which
  Mojito does not yet accept.
- A generic `def` whose `comptime for` constructs a reflected field type —
  `types[i]()` or `FT()` over `comptime FT = types[i]`, with `comptime types
  = reflect[T].field_types()` — or annotates a local with it (`var y:
  types[i]`) is now served by its template, as at the pin, where it was
  cloned per instance. On the clone, `types[i]()` stopped on "Undefined
  variable 'types'" and `x: types[i]` on "unknown type 'types'"; both now
  run. MIR text schema 1.33 adds `type.construct_expr`, the construction of
  the type a parameter expression denotes, which the elaborator closes per
  unrolled copy.
- A nested `def` or lambda is now a generator over its enclosing
  declarations' compile-time parameters, as at the pin: a generic `def` or
  a generic struct's method holding one is served by its template, where it
  was cloned per instance, and a nested `def` with binders of its own is
  instantiated per call below MIR, where the comptime cloner minted it. A
  nested body reads an enclosing value parameter as a parameter, not a
  capture, so `var f = lambda (y: Int) -> Int: y + n` runs where it stopped
  on an escaping closure. A nested body may now name an enclosing type or
  value binder in its signature, its captures' types, and its own
  `comptime if`/`comptime for`, where MIR verification rejected it; a
  spelled application of a nested generic `def` (`inner[Int](3)`) binds its
  own type parameters; and a pack-keyed `def` holding a nested `def` spreads
  its pack into a generic constructor. MIR text schema 1.32 records each
  nested function's enclosing declaration. The nested-function cloner and
  the `NESTED_DEFS` derivation class are gone.
- A `def` keyed on a `UInt`, `Float64`, or `StringLiteral` value beside a
  type pack (`def show[scale: Float64, *Ts: Writable](*args: *Ts)`) is now
  served by its template, as one keyed on an `Int` is, where it was cloned
  per call. The clone ran `range(len(args))` as a `comptime for` bound,
  which the pin rejects; it is now rejected as at the pin. A `def` keyed on
  a `Float64` or `StringLiteral` value now also compiles natively, where
  the backend stopped on an untyped variable.
- A pack-keyed `def` that spreads its pack into a constructor —
  `Tuple[*Ts](*args^)`, `Tuple(*args^)`, a user variadic struct's
  `Bag[*Ts](*args^)` or `Bag(*args^)`, or an own-pack `__init__` such as
  `Box(*args^)` or `Sink(*a)` — is now served by its template, as at the
  pin, where it was cloned per call or rejected. The construction selects
  the struct's declared `__init__` and binds the forwarded pack whole to its
  collector, so a read pack spread into `Tuple`'s `var` collector is now
  rejected, as at the pin. A pack-keyed `def` holding a nested `def` stays
  on the cloner, where it was claimed served and minted its nested `def`
  twice; it rejects a spread into a generic constructor (R440).
- `String.format` and `StringSpan.format` are now upstream's declared
  `format[*Ts: Writable](self, *args: *Ts) raises -> String`, over a bundled
  port of upstream's `_FormatUtils` in `std/string.mojo`, where the checker
  answered `format` intrinsically and the VM formatted the template in Rust.
  A pack-keyed `def` spreading its collector into `format` is now served by
  its template, as at the pin, where it was cloned per call. A String's
  `format` now raises, so a call outside `raises` or `try` is rejected, and
  a malformed template raises upstream's catchable errors instead of an
  uncatchable run error. A string literal's `format` stays non-raising, as
  upstream's `StringLiteral.format`, through the bundled stand-in
  `_FormatUtils.format_literal`; a malformed literal template aborts at run
  time where the pin rejects it at compile time (R425). Format specifiers
  (`{:>5}`, `{:.2f}`) and mixed manual and automatic indexing, which
  Mojito accepted and the pin rejects, now fail with the pin's messages. A
  struct argument formatted through `{}` now writes through its own
  `write_to`, where the VM fell back to the reflective default, and
  `format` over built-in arguments now runs natively (a struct argument
  does not yet: R433, R434). A format-heavy loop is about fifteen times
  slower on the VM (10,000 calls: 2.3 s → 35.6 s, release build; R439).
- A pack spread into a static method's collector now runs, after leading
  arguments (`U.inner(w, n, *args)`) or alone (`U.show(*a)`), where the
  checker rejected the first and the concrete VM bound the whole pack as
  one argument. An explicit `[*Ts]` or `[Int, String]` now reaches a
  generic `def`'s or static method's result type (`-> B[*Ts]`), and the
  erased oracle forwards an explicit pack through a frame entered by a
  spread. A keyword-only `mut` parameter after a `*args` collector now
  writes back to the caller, where the VM rejected the call.
- `Writer` is now upstream's library trait, bundled in
  `std/format/__init__.mojo` and exported by the prelude: a
  `write_string(mut self, string: StringSlice)` requirement and a default
  `write[*Ts: Writable](mut self, *args: *Ts)` that writes each argument
  through its `write_to`, which a conformer may override. `String` declares
  upstream's `write[*Ts]` and `write[T]` overloads. A `w.write(...)` call
  now resolves to that declared method, where the checker answered it
  intrinsically, so a pack-keyed `def` spreading its collector into `write`
  (`s.write(*a)`, `w.write(*a)` over `W: Writer` or `Some[Writer]`) is
  served by its template, as at the pin, where it was cloned per call. A
  conformer's own `write` is now reached by a direct call, where the
  intrinsic shadowed it, and one that forwards through
  `args[i].write_to(self)` no longer overflows the stack. Writer-heavy runs
  cost about 3–5% more wall time and memory (`nominal_string_writer.mojo`
  5.47 s → 5.73 s, 199 → 207 MB, debug build).
- A trait witness may now name a regular parameter otherwise than its
  requirement (`def f(self, y: Int)` for `def f(self, x: Int)`), as at the
  pin, where it failed with "does not match the signature required by
  trait"; a keyword-only parameter's name must still match. Binding such a
  renamed parameter by keyword through the bound is rejected as
  unsupported, where the pin binds it by the requirement's name.
- A call whose argument a method's pack collector bound rejects now says
  so, as the pin does ("an element of 'args' with type ... does not conform
  to trait 'Writable'"), where it reported "no overload matches".
- A trait requirement may now declare a `*args` collector, homogeneous
  (`def sum(self, *xs: Int) -> Int`) or a pack (`def take[*Ts: Writable](self,
  *a: *Ts)`), read or `var`, as at the pin, where it was rejected with
  "unsupported feature: variadic '*args' parameters". A call or a spread
  through the bound (`s.take(*a)` over `S: Taker`) dispatches to the
  witness, which may rename the collector but must match its element type
  and convention.
- A generic `def` that spreads its pack into a method's collector
  (`Sink().take(*a)`, `Sink().drain(*a^)`, `V[*Ts]().put(*a)`) is now served
  by its template, as at the pin, where it was cloned per call. A variadic
  struct's method may now collect the struct's own pack (`def put(self, *b:
  *Self.Ts)`), where every call was rejected with "no overload matches", and
  a `comptime for` over `b.__len__()` there now runs, where it stopped with
  "unspecialized type-keyed method". A forwarded pack whose bounds differ
  from the struct pack's is rejected, as at the pin.
- A generic `def` that returns a variadic struct over its own pack
  (`def first[*Ts]() -> Variant[*Ts]`), or calls an overloaded generic
  method or constructor (`Box[T](x^, "t")`), is now served by its template
  for every call, as at the pin, where it was cloned per call. The erased
  oracle (`--erased`) now forwards such a pack element by element, where it
  read an explicitly applied pack as one element and printed a wrong
  length.
- `Variant` is now a struct generator, as at the pin: its template serves
  every instance, each storage operation selecting the alternative
  `_get_type_index[T, *Ts]()` names, which elaboration closes per instance.
  A generic `def` or a variadic struct may now call `Variant` operations
  over its own binders (`v.isa[T]()` in `def pick[T]`), where the call
  stopped with "unspecialized type-keyed method"; `Variant[Int, Int]` is
  accepted and selects the first alternative, where it was rejected as a
  duplicate; and a `T` that is no alternative fails the instantiation with
  "constraint failed: Type does not exist in Variant.". MIR text schema
  1.31 carries the symbolic index (`pack_index_of`).
- A type parameter inferred from a string literal now binds `String`, the
  type the literal materializes to, as at the pin (`f("x")` for `def
  f[T: Movable](var x: T)`), where it bound `StringLiteral`.
- A static method called through an instance of a variadic struct
  (`p.has[Int]()` for a `@staticmethod` reading `Self.Ts`) now runs, where
  elaboration stopped with "keeps the parameter constant".
- A `comptime for` over tuple- or struct-valued elements in a generic `def`
  (`comptime for p in PAIRS:` over `comptime PAIRS = [(1, 2), (3, 4)]`) now
  runs its body once per element, as at the pin, where it was rejected with
  "'comptime for' over elements of type 'Tuple[Int, Int]' in a generic
  body". A tuple display subscripted as a temporary (`(1, 2)[0]`) now reads
  its element, where it stopped with "reference binding to a non-place
  expression". A display mixing tuples that differ only in which elements
  are literals (`[(1, 2), (3, n)]`) is a list of their materialized tuple.
  Compile-time construction of a struct no longer pulls in a user `def`
  whose name matches a callable parameter of a library body.
- A method of a generic struct over a value parameter (`struct S[n: Int]`)
  holding a `comptime for` the template does not serve (a struct-valued
  display, a body `comptime m: Int = ...`) now runs, as at the pin, where
  it stopped with "unspecialized type-keyed method": each instance mints
  its own clone of the method, as an instance over type parameters does.
- A `comptime for` display over a generic body's parameters may now call a
  `def` returning a scalar, compute a float, or subscript a compile-time
  list (`[twice(n), n]`, `[Float64(n) * 0.5]`, `[SQUARES[n]]`), and the
  template serves it, as the pin evaluates it per instance; the `def` is no
  longer cloned, and a generic struct's method spelling such a loop no
  longer stops with "unspecialized type-keyed method". A list display
  mixing `String` and string-literal elements (`[label(n), "z"]`) is now a
  list of `String`, where it failed with "type mismatch for list element".
- A local `comptime` alias of a display binding over a generic body's
  parameters (`comptime A = L` after `comptime L = [n, n * 2]`) is now
  another name of that binding, as at the pin: `comptime for x in A`, `A[0]`
  and `len(A)` in a condition or a type argument, and `materialize[A]()`
  read the display, so a generic struct's method that iterated the alias no
  longer fails with "'A' is not a compile-time type", and a `def` doing so
  is no longer cloned per instantiation.
- A signature or a struct field type can now apply any callable over `Int`
  and `Bool` values to a compile-time parameter, and spell `not` or a
  conditional, as it can at the pin: `def gen[n: Int]() -> SIMD[DType.int32,
  twice[n]()]` for a generic `def twice[k: Int]() -> Int`, an overloaded
  callee, one with a default, a keyword, or a `var` parameter, a static
  method (`S.f(n)`, `G[n].w()`, and `Self.w()` in the struct's own field
  types), `Flag[not (n == 9)]`, and `SIMD[DType.int32, n if n > 2 else
  h(n)]` were rejected with "not a compile-time Int constant". Each call is
  the application of its callee, the same value wherever it is spelled: a
  default stands for an omitted argument (`d(1)` is `d(1, 3)`), `Self.w()`
  is `G[2].w()` for the instance, and a call infers a parameter through one.
  `not (n == 9)` is `n != 9`, and a conditional over a closed condition is
  its taken arm. A raising callee is rejected with "cannot call raising
  function in type parameter".
- A static method can now be called through `Self` in a method body
  (`Self.cells()`), as it can at the pin, where the check stopped with
  "Undefined variable 'Self'".
- A signature or a struct field type can now apply a function to a
  compile-time parameter, as it can at the pin: `def make[n: Int]() ->
  SIMD[DType.int32, h(n)]`, a parameter `v: SIMD[DType.int32, h(n)]`, and
  the field `var v: SIMD[DType.int32, h(Self.n)]` of `struct Wrap[n: Int]`
  were rejected with "not a compile-time Int constant". A call of a plain
  module `def` over `Int` and `Bool` arguments is now the application of
  that `def` itself, the same value wherever it is spelled, so a body that
  spells the call again names it, a caller spells it by the argument (`var
  v: SIMD[DType.int32, h(2)] = make[2]()`), another generic body spells it
  over its own parameter, and a call infers the parameter from it
  (`total(v)`). The elaborator runs the function per instance. A
  diagnostic spells such a type as written (`SIMD[DType.int32, h(n)]`),
  and the call is not the value it computes: `make[2]()` is no
  `SIMD[DType.int32, 4]`.
- A type or a parameter argument can now compute an `Int` or a `Bool` from a
  generic body's parameters by a call, a comparison, a boolean operator, or
  a conditional, as it can at the pin: `g[h(n)]()`, `flag[n > 2]()`,
  `SIMD[DType.int32, h(n)](1)`, `Buf[h(n)]()`, and the same over a local
  `comptime` display binding (`g[h(L[0])]()`, `g[min(L[0], L[1])]()`) or a
  local `comptime` value (`comptime e = h(n)` then `g[e]()`) were rejected
  with "not a compile-time Int constant", and a `def` that also iterated the
  display was cloned per instantiation and failed the same way. The check
  names a function for the expression, the template carries its application
  to the parameters the expression reads, and the elaborator runs it once
  per instance. Two spellings of one expression are one type, arithmetic
  over applications stays arithmetic (`h(L[0]) + h(L[1])`), and an
  application is not the value it computes: `SIMD[DType.int32, h(n)]` does
  not take a `SIMD[DType.int32, n * 2]`. A closed application in a plain
  `def` (`g[h(3)]()`, `SIMD[DType.int32, h(2)]`) runs the same way.
- A type or a parameter argument can now read a local `comptime` display
  binding over a generic body's parameters (`comptime L = [n, n * 2]`): an
  element by position or the length, alone or under integer arithmetic, and
  a local `comptime` value bound to one. `SIMD[DType.int32, L[0]](1)`,
  `Buf[vals[1]]()`, `g[len(vals) + 1]()`, and `comptime e = L[1]` then
  `g[e]()` were rejected with "not a compile-time Int constant" in a generic
  struct's method and in a `def` with no `comptime for` beside them, and
  cloned the `def` per instantiation otherwise. The binding denotes one
  symbolic value in a type, as upstream's does: `L[0]` twice is one type,
  `L[0] + 0` is `L[0]`, and `L[0]` is not `n`. The elaborator evaluates the
  display once per instance, when a type or an argument of that instance
  needs it.
- A local `comptime` binding of a display over a generic body's parameters
  (`comptime L = [n, n * 2]`) is now served by the template whatever reads
  it, where a body that read an element or the length, or materialized the
  binding, beside a `comptime for` over it was cloned per instantiation. The
  binding is a compile-time value with no runtime form, as upstream's
  `comptime` alias is: a `comptime if` condition (`L[0] == 3`), a `range`
  bound (`range(len(L))`), and an `Int` or `Bool` another binding or
  crossing reads (`comptime e = L[1]`, `comptime(len(L))`) are each lifted
  as a function the elaborator runs per instance, and `materialize[L]()`
  builds the display where it crosses to runtime. Programs that failed now
  run: any of these reads in a generic struct's method, a `comptime if` over
  an element with no loop beside it, a display that reads another,
  `comptime(expr)` over a generic body's parameters (`comptime(n + 1)`), and
  a `comptime if` over a local `comptime` value that is not arithmetic over
  the parameters (`comptime big = n > 2`, then `comptime if big:`).
- A local `comptime` binding of a display over a generic body's parameters
  (`comptime L = [n, n * 2]`) that the body only iterates with `comptime
  for` is now served by the template, where the cloner unrolled each loop in
  the AST per instantiation: MIR lifts the display once, where the binding
  is declared, and the elaborator evaluates it once per instance for every
  loop over the name, as upstream evaluates a `comptime` alias. Programs
  that failed now run: such a binding iterated under a `comptime if`, inside
  another served loop, or in a generic struct's method, and a display, bound
  or written in a loop header, that reads a local `comptime` value bound
  before it (`comptime k = n + 1`, then `[k, k * 2]`).
- A generic `def`'s template now serves the remaining `comptime for`
  sequences, where the cloner unrolled each in the AST per instantiation: a
  module `comptime` collection constant or a local binding of a literal
  display, a `def`'s own value pack, a reflected field-name list and a loop
  bounded by a reflection count, and a list, set, or dictionary display of
  scalar expressions over the binders (`[n, n + 1]`), which MIR lifts as a
  function the elaborator runs per instance, as upstream evaluates the
  display. A `comptime` binding over the loop variable in the body, and a
  `Float64` element, are served too. Five programs that failed now run: a
  module collection iterated in a generic `def`, a display over an enclosing
  loop's variable, a display over `Self.n` in a generic struct's method, a
  body binding over the index in such a method, and `comptime for name in
  reflect[P].field_names()`. In a template-served body,
  `materialize[names[i]]()` over a type parameter and a `comptime if` over a
  bound reflection handle (`r.field_count() == 2`) now compile, and a
  condition `conforms_to(types[i], Trait)` over a reflected field type is
  decided per instance, and a runtime read through a bound name list
  (`print(len(names))`) is rejected as at the pin. A module constant of
  tuple-valued elements iterated in a generic `def` reports its element type
  instead of a materialization error.

- The public `Tuple` and the lazy `TString` are struct generators, as in
  the pin: each is one template in the bundled library, checked once with
  its pack symbolic and instantiated by `native::mono` for both backends,
  where the cloner re-declared each per element list from requests the
  driver derived. A user variadic struct over either (`Pair[*Ts]` holding a
  `Tuple[*Self.Ts]`) is served by its own template too, and Hello World's
  instantiation census reads 0 clones, from 175. A tuple's type is the
  nominal `Tuple[Int, String]` in every phase: `Tuple$t…` names, the
  driver's tuple requests, the generated-tuple predeclaration, and the
  per-index accessor clones with their value twins are gone for it. `t[k]`
  is the template's `__getitem_param__[k]`, and a temporary tuple lends
  that reference from a hidden slot, so `var a, b = make()` and
  `make()[1]` read through it. The consuming members (`reverse`, `concat`,
  `consume_elements`, `deinit_with`) move each element out with
  `Pointer(to=self[i]).unsafe_take_pointee()`, as upstream moves it through
  a pointer, and end the storage with upstream's
  `__mlir_op.`lit.ownership.mark_destroyed``, a new `MarkDestroyed`
  instruction (MIR schema 1.30); both spellings are admitted only in the
  bundled modules that reach compiler-private storage. A `t"…"` occurrence
  is rewritten into a `TString(...)` construction of the template. Four
  things that were rejected or wrong now run: a method of a variadic struct
  that decides a `comptime if` over a pack-element alias (`comptime T =
  Self.Ts[i]`), which aborted at run time as an unspecialized stub; a
  module `comptime i` no longer reaches a `comptime for i` the template
  keeps; a method's own value argument computed from an open pack
  (`self[Self.Ts.length - 1 - i]`) types its result at that index; and
  `Tuple.reverse`, and `Tuple.concat` over a `String` element of its
  receiver, no longer double-free it natively (a `String` in `concat`'s
  argument still does, roadmap R277). `Variant` and the variadic structs over it are
  still specialized whole (roadmap R4)
  (`assets/ok/tuple_template_served.mojo`,
  `assets/ok/pack_struct_element_alias_condition.mojo`).

- `Tuple.reverse` and `Tuple.concat` are now declared in
  `std/builtin/tuple.mojo` with upstream's signatures, as in the pin, where
  the checker typed both in Rust from the element list and the elaborator
  synthesized their bodies: `def reverse(deinit self, out result:
  Tuple[*Self.Ts.reverse()])` and `def concat[*OtherTs: Movable](deinit
  self, deinit other: Tuple[*OtherTs], out result:
  Tuple[*TypeList._concat[Self.Ts.values, OtherTs.values]()])`. So
  `pair.concat[Bool](Tuple(True))`, which was rejected, binds the pack
  explicitly. Five pieces of Mojo landed for the port. A method takes a
  named `out` result, as a function does. A callable's own pack is inferred
  through a `Tuple[*Ts]` parameter from the argument's element list, the
  caller's own pack included (`inner(t)` over `t: Tuple[*Us]`). A type
  list reverses (`Ts.reverse()`) and concatenates
  (`TypeList._concat[A.values, B.values]()`), over closed lists and over
  packs still open, and spreads into a type (`Tuple[*Ts.reverse()]`): the
  open form is upstream's `param_list.tabulate` / `param_list.concat`, which
  MIR carries (schema 1.29) and the elaborator closes per instance. The
  `lit.ownership.mark_initialized` statement marks a method's named result.
  A subscript index that starts with a `Self` chain
  (`storage[Self.Ts.length - 1 - i]`) parses. A closed `TypeList`'s `length`
  now folds in a runtime position (`print(tl.length)`) and its element
  indexes in a `comptime if` (`tl[2] == Float64`), both of which were
  rejected. Each transform is kept on a `Tuple` specialization only where a
  checked call asks for it, and a call that reaches a `Tuple` no
  specialization serves yet, or one over a pack still open (`t^.reverse()`
  in `def flip[*Ts: Movable](var t: Tuple[*Ts]) -> Tuple[*Ts.reverse()]`),
  is typed against the declaration. The bodies
  still transfer each element out of the private storage where upstream
  moves it through a pointer (roadmap R343).
- `Tuple`'s default initializer is now upstream's, as in the pin: it marks
  `self.storage` initialized with
  `__mlir_op.`lit.ownership.mark_initialized`(__get_mvalue_as_litref(...))`
  and writes each element through `Pointer(to=self[i]).unsafe_write({})`,
  where it stored each element into the private storage. Three pieces of
  Mojo landed for it: the `__mlir_op` statement (accepted only in the
  bundled modules that reach compiler-private storage, carried in MIR as
  `MarkInitialized`, schema 1.28, which ownership treats as the place's
  definition and the backends emit nothing for), a pointer to a `Tuple`
  element (`Pointer(to=t[0])`, the `ref` its compile-time-index accessor
  returns, which failed in the VM before), and the initializer list `{}` /
  `{a, b}` at any contextual type (`var s: String = {}`, `take({1, 2})`,
  `var p: T = {}` over a `Defaultable` binder), which the checker spells and
  checks as that type's construction and HIR lowers in the brace's place. A
  brace with no contextual type reports upstream's `cannot emit initializer
  list without a contextual type`. The checker rule that counted element
  stores into a `Tuple` field as its initialization is gone.
- A struct keyed on a vector value (`AHasher[key: U256]`) or on a
  struct-typed value (`Tagged[e: Extent]`) is now a generator, as in the
  pin: its members are checked once with the value symbolic and
  `native::mono` mints each instance, where the cloner re-declared the
  struct per value. A closed vector or struct parameter read at run time is
  a parameter-value constant (MIR schema 1.27), and a field of a
  struct-typed value is a parameter expression, so `comptime if
  Self.e.cols > 2`, `return Self.e.rows`, and `SIMD[DType.int32,
  Self.e.rows]` all fold per instance (`SIMD` of such a width failed to
  check before). A `def` takes a struct-typed value parameter too
  (`rows_of[e: Extent]`), and a computed argument (`Extent.square(4)`) is
  frozen to its fieldwise construction before the check
  (`assets/ok/struct_generator_struct_value.mojo`,
  `assets/ok/struct_generator_vector_value.mojo`); a field the value's type
  lacks is rejected where the generator reads it
  (`assets/type_error/struct_value_param_missing_field.mojo`). Hello World
  clones no vector-keyed struct (`struct_dtype_vector` falls from 8 to 0).
  A variadic struct whose source names no `Tuple`, `TString`, or `Variant`
  (`struct Row[*Ts]`) is a generator too, its pack bound per instance;
  `Tuple`, `TString`, `Variant`, and the variadic structs over them are
  still specialized whole (`docs/roadmap.md` R4). The erased oracle now
  reifies a struct's pack, a type argument's value arguments, and a
  constructor's own solved parameters, so three fixtures it could not run
  now agree.

- `hash(v)` over a vector whose dtype or width is a binder now runs at every
  width, in a lane-keyed `def` or a `DType`-keyed struct's method, as in the
  pin; a vector wider than one lane failed at run time for want of the
  hasher's `_update_with_simd` clone. The hashers' `_update_with_simd(mut
  self, value: SIMD[_, _])` is now upstream's generator over an infer-only
  dtype and `SIMDLength` binder pair, which `native::mono` instantiates at
  each leaf type it reaches, so no leaf is cloned (Hello World's per-call
  method clones fall from 75 to 47) and a compile-time `hash` of a vector
  folds (`assets/ok/lane_keyed_hash.mojo`). A vector argument now solves a
  `DType` and a `SIMDLength` binder together, and `SIMD[_, _]` is accepted
  on a `def` and a trait requirement too
  (`assets/ok/simd_lane_binders_inferred.mojo`); an `Int` width binder stays
  unresolved, as in the pin
  (`assets/type_error/simd_int_width_binder_not_inferred.mojo`). A member of
  a vector- or struct-keyed struct with binders of its own is served by its
  template, and a method over an `H: Hasher` binder no longer clones per
  hasher.

- A `DType` float-format query over a binder (`DType.mantissa_width[dt]()`
  and `max_exponent`, `exponent_width`, `exponent_bias`) is served by its
  `def`'s template, as in the pin, where the `def` was cloned per dtype:
  the checker records the static method's application, which each instance
  answers, failing the instantiation at a non-float dtype with the pin's
  `dtype must be floating point`. The query over a generic struct's
  `Self.dt` in a method, which failed MIR verification, now runs
  (`assets/ok/dtype_float_query_template_served.mojo`).

- A lane gather in a `DType`- or width-keyed `def` (`v.shuffle[1, 0]()`,
  `v.slice[w // 2, offset = w // 2]()`, `v.shuffle[n, 0]()`, `v.join(w)`)
  is served by its template, as in the pin, where the `def` was cloned per
  call and a gather whose own arguments named a binder failed to check:
  MIR carries the mask in its method's form (`LaneMask`, text schema
  1.26), and the elaborator closes it per instance and checks the method's
  constraints, failing the instantiation with the pin's message
  (`assets/ok/lane_gather_template_served.mojo`). The MIR text reader now
  keeps an operator over constants unfolded as written (`4 // 2` in an
  instance's type), so such a program round-trips.

- An annotated vector binding at a symbolic width or dtype with a numeric
  literal initializer (`var v: SIMD[DType.int32, n] = 7` in `def h[n: Int]`,
  or at a `comptime for` index) now runs, as in the pin, where it failed
  with "binding of Int to a slot of type SIMD[DType.int32, n]": the checker
  records the literal's materialization at the symbolic vector type, which
  each instance splats at its own width; a literal argument to such a
  parameter runs too (`assets/ok/simd_symbolic_width_literal_binding.mojo`).

- A `comptime if` whose condition applies a function inside a
  template-served `comptime for` (`comptime if is_even(i):` in `comptime for
  i in range(n)`) now runs, as in the pin, where it failed with "binds 2 of
  its 1 parameters": the condition's thunk declares every binder in scope,
  the enclosing loops' indices included, and reads an index as a parameter
  reference, so nested loops, two loops of one name, and a shadowing index
  decide per copy. A second loop of one name, or a `Bool` loop variable
  only a condition reads, also compiles natively now
  (`assets/ok/comptime_if_condition_reads_index.mojo`; roadmap R328, R329).

- A `def` keyed on a variadic value pack (`*values: Int`, `*flags: Bool`,
  `*kinds: DType`), a method's own included, is served by its template, as
  in the pin, where every call cloned the body with the values substituted:
  the checker types the pack as a parameter list and records each
  `len(values)`, `values.__len__()`, and `values[i]` as a parameter
  constant, which the elaborator folds per instance from the call's
  brackets and the erased oracle from the frame's reified pack. A
  `values.__len__()` in a method, which failed with "type '$variadic[Int]'
  has no method '__len__'", now runs (`assets/ok/value_pack_template_served.mojo`;
  roadmap R325 keeps a runtime read of the whole pack on the clone).

- A reflection query over a template-served body's parameter
  (`reflect[T].field_count()`, `is_struct()`, `field_index["x"]()`,
  `len(field_names())`, `field_names()[i]`, in a runtime position, a
  runtime `if`, a bound `comptime n` or `comptime r = reflect[T]`, or a
  `comptime if` condition; `reflect[Self.T]` and `reflect[Self]` in a
  generic struct's method) now runs, as in the pin, where it failed with
  "vm backend does not support the built-in or callee 'reflect.field_count'"
  or a MIR verify error: the checker records it as a parameter constant,
  which the elaborator answers per instance from the bound struct by the
  policy the checker applies to a closed subject, now one shared function
  (`ReflectQuery::answer`). A field the struct lacks fails the instantiation,
  as at the pin (`assets/ok/reflection_template_served.mojo`,
  `assets/type_error/reflection_template_unknown_field.mojo`; roadmap R323
  and R324 file the erased-oracle gap and the builtin `reflect`).

- A local `comptime` binding over a generic `def`'s or a generic struct's
  parameters (`comptime m = N + 1`, `comptime n = Us.length`, `comptime k =
  Self.n + 1`, `comptime U = T`) now runs, as in the pin, where it failed
  with "'N' is not a compile-time type": the elaborator keeps the binding
  for the template, and the checker binds it to its parameter expression
  (or, for a type, as a scoped alias), so a `comptime if`, a vector width,
  or a `comptime for` bound over it is decided per instance and the body
  stays served by its template
  (`assets/ok/comptime_local_over_binder.mojo`).

- A `comptime for` over a list, set, or dictionary display of `Int`,
  `Bool`, or string literals in a generic `def` or a generic struct's method
  is served by its template, as in the pin: MIR's `ComptimeFor` header now
  iterates a compile-time sequence (`ComptimeSequence`, schema 1.25), as
  upstream's `kgen.param.for` does, instead of only a `range`, and the
  elaborator unrolls one copy per element. A compile-time `break` in such a
  loop, which the AST unroller rejected, now runs, and a list loop no longer
  keys a pack-keyed `def`'s clone
  (`assets/ok/comptime_for_collection_template_served.mojo`; roadmap R246
  keeps the loops still cloned).

- A `def` keyed on an `Int`, `Bool`, or `DType` value beside a type pack
  (`def scaled[n: Int, *Us: Writable](*extra: *Us)`) is served by its
  template, as in the pin: an explicit application `scaled[3](7, "x")` now
  records the pack the call's arguments solve beside the bracketed value
  instead of an empty pack, so the elaborator instantiates the template
  rather than the cloner minting a copy
  (`assets/ok/pack_beside_value_binder.mojo`). With no pack-keyed instance
  left to derive, the `PackElements` certificate class is deleted. The
  cloner's type-pack branch stays until R256, R316, and R246 land
  (roadmap R253).

- A method keyed on a type pack of its own (`def take[*Ts: Writable](self,
  *a: *Ts)`) is served by its template, as in the pin: its body is
  elaborated with the pack open, a `comptime for` or `comptime if` over the
  pack kept for the elaborator, and each call instantiates the template from
  its solved pack instead of minting a per-call clone, on a plain struct and
  on a generic struct's instance alike, a method spreading its pack into
  another included (`assets/ok/pack_method_template_served.mojo`; roadmap
  R307). A member of a struct specialized whole still clones (R310).

- A method call in MIR carries the position of a whole type pack spread into
  the method's collector (`Sink().take(*args)`; textual MIR 1.24,
  `call.method`'s `spread`), as a `def` call has since 1.17: the verifier
  checks it, the elaborator replaces it with the bound pack's element
  places, and the erased oracle splices the elements, so a template-served
  body that spreads its pack into a method runs on the VM, erased, and
  natively. A `def` spreading into a method still keys a clone until a
  method with a type pack of its own is served by its template (roadmap
  R256, R307). The cloner failure filed as R291 no longer reproduces.

- A generic struct's method holding a `comptime if` or `comptime for` over
  its struct's parameters (`comptime if Self.T == Int`, `comptime for i in
  range(Self.n)`) is served by its template, as in the pin: the elaborator
  decides the branch and unrolls the loop per instance, where the cloner
  used to mint a clone per instance (`keyed.mojo`'s `Cell.label` now mints
  none; `assets/ok/struct_method_comptime_template_served.mojo`). A
  `rebind[Dest](x)` is a template form too (textual MIR 1.23, `value.rebind`):
  a `def` or a method holding one is served by its template and the
  elaborator asserts the type equality only for the instances a call
  reaches, after deciding their `comptime if`s, as the pin does. A
  `Box[String]` beside an uncalled `rebind[Int](self.value)` method now runs
  where it was refused with "type mismatch for rebind", and a reached
  mismatch fails with the pin's "rebind input type 'String' does not match
  result type 'Int'" (`assets/ok/rebind_method_template_served.mojo`,
  `assets/type_error/rebind_method_instance_mismatch.mojo`). The erased
  oracle now binds a struct's type parameters on an inferred construction,
  through a caller's binder, and on a static call's spelled receiver, so it
  decides such a `comptime if` as concrete MIR does.

- A method with compile-time parameters of its own is a generator, as in the
  pin: its call carries the arguments the checker solved (textual MIR 1.22)
  and the elaborator instantiates the method's template per call, where the
  cloner used to mint a per-call AST clone. This covers generic
  constructors, statics, a method's own `DType` or lane binder, and a
  `comptime if` or `comptime for` over its own binders. `C("hi")` against
  `__init__[T: Writable]` now binds `T` at `StringLiteral` and runs, where it
  failed MIR verification with "argument 0 of 'C.__init__$y6:String' has
  type StringLiteral, declared String"
  (`assets/ok/generic_constructor_literal_argument.mojo`,
  `assets/ok/template_method_own_comptime_if.mojo`).

- A struct keyed on a `DType`, or on a value parameter it uses as a lane
  width, is a generator, as in the pin: its members are checked once and
  instantiated by the elaborator from the template's MIR, where the cloner
  used to specialize the struct whole. `Lanes[n](3).v` for `struct
  Lanes[w: Int]` holding `SIMD[DType.int32, Self.w]` now runs in `def
  owned[n: Int]` and at a `comptime for` index, where it failed with "type
  'Lanes[n]' has no field 'v'", and a lane-keyed `def` building such a
  struct over its own binder is served by its template
  (`assets/ok/struct_generator_lane_symbolic_width.mojo`).

- A `comptime T = Ts[i]` alias of a pack element under a pack-keyed `def`'s
  `comptime for`, and a `Ts[i]` annotation spelled directly, are served by
  the template, as the pin elaborates them: the alias stands for the
  dependent element it denotes, so `var first: T = args[i]` and `T()` no
  longer key a clone per call. A `Ts[i]` annotation used to fail with
  "unknown type 'Ts'" (`assets/ok/pack_element_alias_served.mojo`).

- A pack-keyed `def` whose `comptime for` binds an element (`var value =
  Ts[i]()`, `var first = args[i]`) is served by its template, as the pin
  elaborates it: each unrolled copy keeps its own local at its element's
  type. `Ts[i]()` lowers to MIR's element form of `type.construct` (textual
  schema 1.21), which the elaborator writes as the selected element's
  default construction, a literal index included. Such a `def` used to be
  cloned per call, and failed there on a `StringLiteral` element ("register
  r7 has no checked type") (`assets/ok/pack_element_binding_served.mojo`).

- A `var` declared in a template-served `comptime for` whose type names the
  index (`var v = SIMD[DType.int32, i](7)`, `var l = Lanes[i](3)`) runs as
  in the pin: the elaborator gives each unrolled copy its own slot at that
  iteration's type and drops the template's, on the VM and natively. Such a
  binding used to fail MIR verification ("place root slot … is typed Int32,
  declared SIMD[DType.int32, 2]") or the comptime walk ("'i' is not a
  compile-time type") (`assets/ok/comptime_for_index_typed_local.mojo`).

- A `def`'s or a method's own type pack answers `Us.length`, `len(Us)`,
  `Us.contains[X]()`, and `Us.all_conforms_to[T]()` in a runtime position,
  as the pin does: a template-served body carries the query as a parameter
  constant (`Const::Param`, textual schema 1.20, upstream's
  `kgen.param.constant`) that the elaborator folds per instance, on the VM
  and natively, and a cloned `def` folds it at the clone. Such a query used
  to fail with "Undefined variable 'Us'" or a MIR verifier error
  (`assets/ok/pack_length_runtime_position.mojo`).

- Constructing a type parameter through `Copyable`'s initializer
  (`Self.T(copy=self.x)` in a generic struct's method, `T(copy=x)` in a
  generic `def`) copies the source as the bound type does, in the template,
  in an instance clone, and natively; `Int(copy=x)` names a built-in's copy
  initializer. `type.construct` carries the initializer's keyword arguments
  (textual schema 1.19). Such a call used to fail with "Undefined variable
  'Self'" (`assets/ok/type_param_copy_construction.mojo`).

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

- A projection after a `rebind` (`rebind[S](x).f`) is now judged per
  instance, as at the pin: a rebound place carries an explicit
  `Proj::Rebind` step (MIR text schema 1.37, the `rebind` projection),
  which every ownership pass and the VM read through, and the elaborator
  judges it against the type it applies to and erases it. Before, the
  step's equality was lost under the later projection, so an instance
  whose operand was another struct with an `f` field silently read that
  field, and others failed with unrelated VM or verifier errors.

- A subscript or `ref self` method call on a list display with no
  contextual type (`print([1, 2][0])`, `[1, 2].__getitem__(0)`) now runs as
  at the pin: the `Array` display is materialized as a temporary the
  statement destroys, where it stopped with "reference binding to a
  non-place expression". A temporary whose struct is not `Deinitable` —
  a call result, a list or tuple display — borrowed as a receiver or read
  through a field (`mk().id`, `[Res(1)][0].id`) is now rejected as
  abandoned, as at the pin, with upstream's messages: the bundled `Array`,
  `List`, `Tuple`, `Set`, `Dict`, and `OwnedPointer` carry their
  `@explicit_destroy` text, and a struct without one reports "type 'Res'
  does not conform to 'Deinitable' and must be explicitly destroyed".

- A `comptime if` condition applying a function to a local `comptime`
  binding over a binder (`comptime m = n + 1` then `comptime if
  is_even(m):`), chained or read beside a `comptime for` index, now decides
  as at the pin; the lifted thunk reads the binding as the parameter
  expression it denotes, and a fixture now pins the shape.
- A local `comptime` binding of one function no longer leaks into a later
  function's body: `def g[n: Int]()` holding `comptime xs = ["a",
  String(n)]` printed the closed `xs` of an earlier `def f` instead of its
  own.

- A local `comptime` `DType` binding (`comptime lane = DType.int8`) no
  longer stays visible after its block: a later, unrelated function naming
  `Scalar[lane]` was accepted, where the pin reports an unknown declaration.

- A closed compile-time argument in a generic call's brackets
  (`tup[(1, Tag(8, "x"))]()`, `one[Q(8)]()`, `s[1 + 2]()`) is now
  compile-time data on the call, as at the pin, instead of run-time code in
  the caller: before, the caller built the `String`, the `Tag`, and the
  `Tuple` and never destroyed them. A method's own tuple or struct value
  parameter (`h.show[(1, Tag(8, "x"))]()`, `H.st[(2, Tag(9, "y"))]()`),
  which failed MIR verification with "register has no checked type", now
  runs. The erased oracle now reads a defaulted value parameter holding a
  `String` (`def d[p: Tuple[Int, String] = (7, "d")]()`).
- A field read of a struct value parameter (`q.a`, `q.i.v`, `q.s` in `def
  f[q: Q]()`) now reads the field alone, as at the pin, instead of building
  and destroying a whole `Q` for each read: a number is a constant, and a
  field that owns something is constructed by itself into a temporary,
  which `var j = q.i` takes with no copy. `q.s^` is now rejected with
  "cannot transfer from a parameter expression". The same holds for a
  `comptime for` element's field, except a `String`, float, `UInt`, or
  tuple field in a non-generic `def`.
- A tuple or struct value parameter holding a `String` (`def tup[p:
  Tuple[Int, Tag]]()`) is now constructed at each use with no slot of its
  own, as at the pin, so an element type's destructor runs once per use.
  Before, the value was built into a slot at entry, each use copied out of
  it, and the slot was destroyed once more, which a copy initializer or
  destructor that prints made visible.
- A transfer out of a compile-time parameter (`take(p^)` in `def g[p:
  Int]()`, or over a `comptime for` variable) is now rejected with "cannot
  transfer from a parameter expression", as at the pin, where an `Int`
  value parameter was accepted and moved.
- A struct method whose clone fails to elaborate for a reached instance
  (`comptime q = [1, 2][i]` in a loop over `range(Self.n)`, called on
  `S[5]`), or a generic method's per-call instantiation that fails the same
  way, is now rejected at compile time with "function instantiation of
  `S.f…` failed", as at the pin, where it stopped at run time with
  "unspecialized type-keyed method". An instance no reachable call needs
  still compiles.
- A `comptime for` the template does not serve (`comptime for x in
  [twice(n), i]:`) now runs when it is nested in a served loop or a kept
  `comptime if` arm of a generic `def`, as at the pin, where it failed with
  "'n' is not a compile-time type": the `def` now keys a clone for it there
  as it does at the top of the body.
- A runtime read of a local `comptime` display over a generic body's
  parameters (`comptime L = [n, n * 2]`, then `print(L[1])` or `len(L)`) is
  now rejected with upstream's "cannot materialize comptime value of type
  'Array[Int, Int(2)]' to runtime because it is not 'ImplicitlyCopyable'",
  where a body with no `comptime for` over the binding ran it as an ordinary
  list.

- A named `out` result is transferred to the caller. The implicit return
  copied the result and then destroyed the callee's own binding, so a
  result with a destructor ran it once in the callee and again at the
  caller, on the VM and natively: `def make(out result: Loud)` printed the
  destructor's line before the caller's first use, and `Tuple.reverse` and
  `Tuple.concat`, which build a named result, destroyed every element
  twice on the VM. The return now moves the result
  (`assets/ok/named_result_transferred.mojo`).

- A pack element's `.copy()` in a served pack-keyed body (`var x =
  a[i].copy()` under `comptime for i in range(Ts.length)`) calls the
  element's `copy`, where it read the element without copying and the VM
  stopped on a double free of a `String` element
  (`assets/ok/pack_element_copy_served.mojo`).

- Programs the VM used to run only through its erased path now run on
  concrete MIR, as the first full gate since the VM switched found: a
  callable parameter's default (`callback: def(Int) thin -> Int =
  increment`), a `**kwargs` collector filled by keywords or a forwarded
  `**options^`, a `String` value parameter read at run time, a subscript
  whose callback parameter is inferred, a nominal callable returning a
  reference, an origin-specialized nested function value, a runtime
  parameter typed by a generic `def`, and a nested `def` given a capturing
  callable parameter through an indirect call.

- A call through a callable contract takes the contract's own defaults
  (`def[primary: … = increment]`), as the pin does, over the
  implementation's; a contract scalar default binds at its declared type.

- `print(opt.value())` of a view no longer destroys the viewed string before
  the print reads it ("use after Pointer deallocation").

- `var s = id("hi")` over a template-served generic `def` compiles; its
  result register named the unsolved `T`.

- Three fixtures compile natively again: a `DType` value parameter folded
  into an instance keeps its slot typed, and a pack-keyed `def`'s instance
  names a generic struct element (`show(xs)` over a `List[Int]`) by its
  instance symbol ("struct 'List' has no MIR declaration to lay out").

- `comptime if conforms_to(T, Hashable)` in a template-served `def` decides
  the condition per instance again; it failed with "register r0 has no
  checked type".

- `SIMD.to_bits` to a narrower lane in a template-served `def` is rejected
  when the elaborator closes the lane, as the pin rejects it; it used to run.

- A method holding a `comptime for` or `comptime if` derives its instances
  from the checked template again (the bundled `Tuple` methods among them),
  instead of being checked once per instance.

- `comptime N = pow2(3)` applied to a `def`'s value parameter (`scale[N]`)
  runs, as the pin does; it was rejected as a struct argument.

- A dead `comptime if` arm whose closed arithmetic fails (`1 // 0`) no
  longer rejects a template-served `def`; an instance that takes the arm
  still fails.

- The `comptime for` over a tuple diagnostic spells element types as the
  user wrote them (`Tuple[String, String, String]`).

- `comptime T = Ts[i]` then `var first: T = args[i]` in a pack-keyed `def`
  types a `StringLiteral` element's local as that literal's closed type, as
  the pin does, instead of failing with "'StringLiteral[_]' is not
  concrete": the elaborator spells a binder bound to an inferred literal's
  type with the compiler-only `Type::ClosedStringLiteral`
  (`assets/ok/pack_element_alias_string_literal.mojo`).

- `print(Ts[i]())` in a template-served pack-keyed `def` prints each
  element's default instead of failing with "vm backend does not support the
  built-in or callee 'Ts'", and `Ts.length` over a `Defaultable` pack no
  longer fails MIR verification ("variable slot 0 retains ABI-only
  RuntimePack type"): an instance keeps no slot for a type parameter.
- `T()` at `T = Tuple[Int, Bool]` builds `(0, False)` instead of failing
  with "vm backend does not support the built-in or callee 'Tuple'": a
  `Tuple` bound to a call's type parameter or pack element is specialized.
- Natively, an element of a template-served `def`'s heterogeneous pack read
  at a `comptime for` index (`print(args[i])`) projects statically instead of
  failing with "unsupported heterogeneous runtime pack projection".

- A `var` declared in a `comptime for` body that shadows an enclosing `var`
  of its name is its own binding in every copy, as in the pin; it used to
  fail with "operator Add is not defined for None and Int", since the
  checked declaration table never descended into a `comptime for` body
  (`assets/ok/template_loop_local.mojo`).

- A `comptime if` whose condition asks a layout query (`comptime if
  size_of[Self.T]() == 8:` in a generic struct's method, or `size_of[Int]()`
  in any function) is decided by the elaborator under the compilation's
  target, as the pin decides it, on the VM and natively. It used to fail
  with "'size_of' is not a compile-time-callable function"
  (`assets/ok/comptime_if_layout_query.mojo`).

- `Self.T()` in a method or constructor that a generic struct clones per
  instance (one holding a `comptime if`) now constructs the bound type, as
  the pin does, on the VM and natively
  (`assets/ok/struct_type_param_construction_clone.mojo`). The clone used to
  keep the template's `T`, failing with "in 'reset' instantiated for
  'Box[Float64]': type mismatch for assignment target: expected Float64,
  found T", and a constructor initializing a field in every arm of a
  `comptime if` was reported as leaving it uninitialized.
- A trait default holding a `comptime if` is elaborated for each conformer
  that inherits it, as upstream decides it per conformer: the inherited
  defaults are materialized into each conformer before compile-time
  elaboration (`comptime::prepare`) rather than at checking, so the arm is
  selected like a conformer method's own on the VM and natively
  (`assets/ok/trait_default_comptime_if.mojo`). A condition the elaborator
  folds, such as `reflect[Point].field_count() == 2`, used to reach the
  compile-time request path and fail with "vm backend does not support the
  built-in or callee 'reflect.field_count'". `reflect[Self]` in such a body
  waits on R269 and R278.
- A generic struct's `Tuple` field over its parameter (`var pair:
  Tuple[Self.T, Int]` on `Holder[T]`) reads and indexes as the closed
  `Tuple` it names at each instance, from the struct's own methods and from
  outside, as upstream does, on the VM and natively
  (`assets/ok/generic_struct_tuple_field.mojo`). `some_int.pair[1]` stopped
  at run time with "cannot index Tuple$t2[y3:Inty3:Int]", and natively the
  instance's field had no declaration to lay out.
- A tuple display or bare `Tuple(...)` call infers a string element as
  `String`, as upstream's `Tuple.__init__(var *args: *Ts)` infers `Ts`, so
  `var t: Tuple[Int, String] = Tuple(1, "x")`, or a `var w = (1, "q")`
  bound later to a `Tuple[Int, String]`, type-checks where it reported a
  `Tuple[Int, StringLiteral]` mismatch. An annotated tuple binding,
  parameter, or field keeps `reverse` and `concat`
  (`assets/ok/tuple_annotated_binding.mojo`); natively, either transform
  over a `String` element still double-frees it (roadmap R277).
- `repr` of a sized scalar names its type as upstream's
  `SIMD.write_repr_to` does: `repr(Int8(3))` and `repr(Float32(0.5))` print
  `Int8(3)` and `Float32(0.5)` on the VM and natively, where the VM printed
  the bare lane and native code labelled every float `Float64(`, and the VM
  writes a wider vector as `SIMD[DType.int32, 2](1, 2)` instead of
  `[1, 2]` (`Dtype::repr_alias`; `assets/ok/repr_sized_scalars.mojo`).
  Native `repr` of a wider vector remains unsupported (roadmap R275).
- A runtime read through a `comptime names = reflect[T].field_names()`
  binding (`print(names[i])` under `comptime for i in range(len(names))`,
  `len(names)`) is now rejected with the pin's "cannot materialize comptime
  value ... not 'ImplicitlyCopyable'", since the name list is a compile-time
  `Array` like a list display, instead of printing; and
  `materialize[names[i]]()` and `materialize[names]()` now cross the element
  or the whole list explicitly, as at the pin, where the crossing took a
  bare binding only (`assets/ok/reflection_field_name_materialize.mojo`,
  `assets/type_error/comptime_field_names_runtime_use.mojo`).
- `reflect[T].is_struct()` now answers `True` for every type Mojito names —
  `Int`, `Float64`, `Bool`, `NoneType`, a vector, `DType`, and a field
  handle over an `Int` field — as the pin's `#kgen.is_struct_type` does,
  instead of `False` for every type that is not a declared struct; and
  `reflect[NoneType]` resolves its subject in the elaborator
  (`assets/ok/reflection_runtime_query.mojo`,
  `conformance/probes/reflection_builtin_is_struct.mojo`).
- A reflection query in a runtime position (`print(r.field_count())` after
  `comptime r = reflect[Point]`, `reflect[Point].is_struct()`,
  `r.field_index["y"]()`, `r.field_names()[1]`) now prints its answer, as
  the pin's static `Reflected[T]` call does, instead of reporting an
  undefined `r` or an unsupported `reflect.field_count` callee. The
  elaborator's crossing pass folds the query to the literal form of its
  compile-time value where it stands, and a query that has no answer (an
  unknown field name) is rejected at compile time
  (`assets/ok/reflection_runtime_query.mojo`,
  `assets/type_error/reflection_runtime_query_unknown_field.mojo`).
- A direct `size_of[T]()` in a SIMD width now checks and runs in generic
  parameters, return types, and vector constructions. The checker preserves
  the layout application and substitutes its type operand before the
  elaborator resolves the width; a literal lane count remains a different
  checked type, matching Mojo (`assets/ok/comptime_layout_constant.mojo`).
- A place passed mutably and again into a positional collector read by
  borrow (`r(x, x)` over `r[*Ts](ref b: Int, *rest: *Ts)`, `both(k, k)`
  over `both(mut a: Int, *b: Int)`, `s.m(s.v)` over `m(mut self, *rest:
  Int)`) is now rejected with "aliasing values passed mutably to 'b'
  argument and passed immutably to 'rest' argument", as the pin rejects it,
  instead of running: a gathered element is held by reference even when
  its type is trivial, and a method's collector is now judged by argument
  exclusivity as a free function's is
  (`assets/type_error/ref_argument_aliases_pack_element.mojo`,
  `assets/type_error/mut_argument_aliases_variadic_element.mojo`,
  `assets/type_error/mut_receiver_aliases_method_variadic_element.mojo`).
- A store to a field of `self` (or of a `mut` parameter) while a view of
  that field is live (`var view = self.name.strip()`, `self.name =
  String("q")`, then `view.byte_length()`) is now rejected as a use of the
  invalidated interior reference `self.name["bytes"]`, as the pin rejects
  it, instead of running. A view returned from a field receiver lends the
  field's named interior rather than the whole field, through a local too.

- A returned reference must now name its declared origin itself, as the pin
  compares them: `return self.item` under `ref[origin_of(self)]`, and
  `return self.items[i]` under `ref[origin_of(self.items)]`, are rejected
  with "cannot return reference with incompatible origin" instead of running,
  since a field is a different origin from its owner and an element interior
  from its container.

- Forwarding a named accessor's reference result as a method's own
  (`return self.items.unsafe_get(index)` under
  `ref[origin_of(self.items)._get_owned_interior["element"]]`) now stays
  within the declared origin, as the pin runs it: the return check binds the
  accessor's `self` to its receiver instead of the enclosing method's `self`,
  instead of reporting "returned reference escapes storage outside its
  declared origin" (`assets/ok/reference_return_forwarded_accessor.mojo`).
- A reference result now converts to a borrowing view in place
  (`width(p.name_ref())` or `out += p.name_ref()` over an accessor returning
  `ref [self.name] String`, `width(pick(t))` over a free `ref` function), as
  the pin does: the view's `ref [origin]` source binds the referent, instead
  of being rejected as "reference binding to a non-place expression". A
  binding annotated with the view type over such a source or a collection
  element (`var v: StringSpan[origin_of(xs)] = xs[1]`) now keeps the source's
  owner alive while the binding lives, instead of trapping on a use after
  deallocation (`assets/ok/reference_result_view_conversion.mojo`).

- A collection element now converts to a borrowing view argument in place
  (`out += parts[i]` over a `String` collector, `t += xs[0]` or
  `width(xs[1])` over a `List[String]`), as the pin does: the view's
  `ref [origin]` source binds the element place and the view keeps the
  container alive across the call, instead of stopping at MIR verification or
  trapping on a use after deallocation. An in-place update whose operand
  overlaps its subscript target (`xs[1] += xs[0]`) is rejected as aliasing,
  as the pin does
  (`assets/ok/string_element_view_conversion.mojo`,
  `assets/type_error/subscript_inplace_operand_aliasing.mojo`).

- An in-place update of a container element through its reference
  (`ys[0] += "tail"`) now stales a live view lent from that element
  (`var v = ys[0].rstrip()`), so a later use of the view is rejected as an
  invalidated interior reference, as the pin does, instead of trapping on
  the VM with a use after deallocation
  (`assets/ownership_error/list_element_view_stale_after_inplace_update.mojo`).

- `print` reads its arguments as the pin's read collector does: a named value
  of a type parameter (`print(y)` with `y: T`) is read in place instead of
  copied and leaked, and an owned temporary (`print(x.copy())`,
  `print(IC(5))`) is destroyed once `print` returns instead of never, on the
  VM and natively (`assets/ok/print_argument_read_in_place.mojo`).

- `len(a)` of a homogeneous collector (`def count(*a: IC)`) or of a named
  tuple reads the storage in place, as the pin's `len[T: Sized](value: T)`
  does, where it used to copy every element and never destroy the copies,
  on the VM and natively (`assets/ok/variadic_collector_len.mojo`).

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

- A `ref` type argument (`Tuple[ref[o] Int]`, `List[ref[o] Int]`) is
  rejected at parse with the pin's own "expected ']' in call argument
  list" (R529). The reference-field extension is the direct field
  declaration alone; reference-valued elements are spelled
  `Pointer[T, origin]`, as upstream spells them.

- The support left for structs specialized whole, which nothing mints since
  every struct became a generator, is deleted: a per-call clone trace's
  separate template owner, first-copy stub traces, a struct's fixed
  arguments, the minted value-struct renaming in template derivations, the
  per-position `Tuple` accessor recipes (a template's `Tuple.__getitem_param__`
  pack accessor is now refused at the instance outright), and the per-call
  constructor provenance that let a clone named `__init__` displace its
  template. Behaviour is unchanged.

- `Tuple` and `TString` members and conformances now come only from their
  bundled declarations (`std/builtin/tuple.mojo`, `std/format/tstring.mojo`),
  as at the pin, including the declared conditional conformances
  (`Hashable where conforms_to(Ts.values, Hashable)` and the rest). The
  checker's Rust-typed `Tuple` member surface, the element-wise conformance
  answers and clone-name coercions left from their clone era, the
  `TString` clone derivation in `template_facts`, and the opaque
  compiler-only callable annotation (`Type::MaterializedCallable`) are
  gone, and `check_program_with_templates`, `check_program_for_discovery`,
  and `check_program_carrying` no longer take a callable map. A check with
  no linked prelude now reports `NoSuchMethod` on a `Tuple` member.

- Every elaboration now follows a source validation run and reads its
  verdict: a `comptime for` display element's call or method call
  (`[P(n).get(), n]`) is judged by the type validation gave it, as at the
  pin, never guessed from the callee's declaration. The validation-abort
  path and the template-shell struct form, which nothing produced any
  longer, are deleted, and the composed-stage request seam validates
  before elaborating.

- A generic `def` passed where a non-generic `def(...)` value is expected
  is rejected at the argument, as the pin rejects it: only an explicit
  specialization (`f[...]`) is a non-generic function value. It used to
  infer the specialization and then fail in the elaborator. A generic `def`
  named as a tuple display element is rejected for the same reason
  ("cannot use parametric function as a runtime closure").

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
