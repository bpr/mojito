# Architecture After Parsing

Companion references: [feature support](features.md),
[symbol ownership and navigation](symbol-map.md), and the
[VM instruction set](vm-instruction-set.md).

This document describes mojito after parsing. Its simplest input is the parsed
program:

```rust
Vec<ast::Stmt>
```

When the compiler is running from a file path, the first post-parse stage may
also parse imported modules and link them into that same shape. Lexing and
parsing are intentionally out of scope here; see [the frontend guide](frontend.md).
This file starts where the parser stops and follows a program through module
linking, compile-time elaboration, semantic checking, HIR lowering, MIR lowering,
compiler analyses, drop elaboration, and execution on the register VM.

## Big Picture

The post-parse pipeline is:

```text
Vec<Stmt>
  -> module link
  -> comptime elaboration
  -> check
  -> HIR CFG
  -> MIR
  -> ownership / borrow / liveness analysis
  -> drop elaboration
  -> register VM
```

Normal whole-program clients use `compiler::Compiler`, which embodies this
ordering rather than requiring callers to compose stages manually:

```rust
let compiler = Compiler::default();
let program = compiler.compile_path(path)?;
let execution = compiler.execute(&program)?;
```

`CompiledProgram` is a private-field wrapper around `CheckedProgram` produced by
the driver only after linking, comptime elaboration, semantic checking, and
ownership analysis succeed. The wrapper records pipeline provenance rather than
adding a second semantic representation. `CompilerError` identifies the failing
stage. Individual stage functions remain public for tests and diagnostic tools;
a stage-composed execution still cannot bypass the ownership contract, because
`VmBackend::run` itself re-checks pre-drop ownership before executing (the
composition remains non-authoritative only for the whole-program
discovery/specialization handoff).

The design is an hourglass:

```text
parsed AST
   |
   v
module linker
   |
   v
comptime elaborator
   |
   v
semantic checker
   |
   v
HIR CFG
   |
   v
MIR  <---- stable waist
   |
   v
analysis + drop elaboration
   |
   v
register VM
```

The MIR is the important waist. Earlier phases preserve source structure,
perform declaration-time rewrites, and protect later phases from unsupported
syntax. Later phases should consume verified MIR rather than rediscover language
semantics from the AST.

## Design Goals

The architecture prioritizes:

- correct subset semantics over raw speed
- explicit control flow before ownership analysis
- explicit places before borrowing and partial moves
- deterministic ASAP destruction
- clean rejection of unsupported constructs
- a small compiler that is still recognizable as a systems-language
  implementation

mojito is not trying to reproduce Mojo's production architecture. First-pass
parity targets single-threaded CPU language semantics and excludes GPU,
concurrency/parallelism, distributed execution, Python interoperability, and any
requirement that MLIR be the compiler's internal IR layer. The register VM is
the executable specification. A versioned textual MIR/VM assembly form is the
next representation boundary; the prioritized native backends are LLVM and the
MLIR-family frameworks — MLIR and the Rust-native, MLIR-inspired
[Pliron](https://github.com/pliron-org/pliron) (its LLVM dialect emits LLVM IR) —
with Cranelift and eBPF as lower-priority options that follow them.

### Source Module Boundaries

Large phases keep orchestration in their root module and delegate reusable
policy or data-model responsibilities to focused children:

- `call.rs` is the phase-neutral function-call contract: it normalizes parser
  marker indexes and matches positional, keyword, default, `*args`, and
  `var **kwargs` collector inputs to parameter slots. The checker and VM separately adapt its
  structural errors and matched slots to types and runtime values.
- `checker.rs` holds the `Checker` state, constructors, and shared prelude
  types, and coordinates checking; the single `Checker` type's methods are split
  by responsibility across `impl Checker` blocks in the `checker/` submodules —
  `statements`, `inference`, `indexing`, `method_calls`, `call_inference`,
  `type_resolution`, `traits`, `origins`, `scopes`, `constraints`, `operators`,
  `iteration`, plus the earlier `annotations`, `builtins`, `calls`,
  `declarations`, `generics`, and `places`. See `docs/symbol-map.md` for the
  per-file responsibility map.
- `mir/mod.rs` lowers ordinary code; `mir/ir.rs` defines the MIR data model and
  `mir/nested.rs` owns capture analysis and nested-function lifting.
- `backend/vm.rs` drives execution; `backend/vm/calls.rs` owns runtime argument
  binding and construction, while `backend/vm/places.rs` owns projected storage
  navigation and access.
- `comptime.rs` runs elaboration and specialization;
  `comptime/rewrite.rs` owns AST substitution and value materialization.

Child modules expose only the phase-internal operations their coordinator needs.
The public entry points for each compiler phase remain in its root module.

## Stage 1: Module Linking

Module:

```rust
src/module.rs
```

Entry points:

```rust
module::link_with_options(entry_path, options) -> Result<Vec<Stmt>, ModuleError>
module::link_source_with_options(source, entry_path, options) -> Result<Vec<Stmt>, ModuleError>
```

`link` and `link_source` are convenience wrappers using default options.

The module linker is deliberately small. It consumes parsed source plus an entry
path and returns one flat `Vec<Stmt>` for the rest of the compiler.

Currently it supports:

- `from module import Name, Other`
- `from module import *`
- dotted module paths such as `from collections.list import List`
- relative imports such as `from .optional import Optional`
- qualified `import module as alias` member access and selective member aliases
- unaliased dotted namespaces, qualified exported types, and lexical block imports
- dots-only relative sibling imports (`from . import sibling`)
- source packages identified by `__init__.mojo`, including package re-exports
- ordinary namespace directories, every dotted-prefix binding, and lexical
  shadowing of an imported namespace tree
- source-package precedence over a same-named source module in each search root
- underscore-prefixed wildcard privacy and collision-free module identities
- transitive imports, dependency-first hoisting, deduplication, and simple cycle
  breaking by canonical path

Imported declarations receive module-qualified internal names before they are
hoisted. Import bindings are rewritten to those names, so two modules can export
the same source name without merging declarations or overload sets. A module
exports top-level `def`, `struct`, `trait`, and `comptime` declarations except
`main`; wildcard imports omit underscore-prefixed names.

Each lexical import scope separately records names established by explicit
imports. Repeating the same target is idempotent and an explicit import may
shadow an implicit prelude binding, but a different target cannot overwrite an
already explicit local name. Import resolution also compares canonical source
paths and rejects an exact self-import. A module publishes provisional local
exports before following dependencies, so this check does not mistake a real
two-module cycle for self-import and mutually recursive modules can bind each
other's declarations deterministically.

Package directories resolve through `__init__.mojo`. Imports in that file become
package re-exports. Mojo forbids executable file-scope statements, so package
initialization means declaration and compile-time initialization rather than
Python-style runtime import side effects.

Submodules are not implicitly visible to siblings or to a package namespace.
They become visible only through an explicit import or an initializer re-export.
The source half of nightly lookup is therefore deterministic. Versioned `.mojoc`
and legacy `.mojopkg` artifacts are intentionally deferred to the artifact
loader; once present, the complete per-directory order is source package,
`.mojoc`, source module, then `.mojopkg`.

Linking retains flat name-binding semantics, but recursively stamps every
statement and expression with its source module path. The file path and local
byte range remain diagnostic provenance. After compile-time elaboration and the
checker's final trait-default cloning pass, one exhaustive walk
uniqueness-normalizes each concrete statement/expression `SyntaxId`: an
already-unique input identity is retained, while every repeated clone receives a
new occurrence identity before semantic facts are recorded. The transitional
`SourceSpan` key carries that optional occurrence discriminator for checker/HIR
lookups, but provenance comparison and MIR source maps deliberately ignore it.
Thus two elaborated clones may point at the same source text without sharing a
type, binding, overload, effect, or declaration fact. Compile-time rewriting
still re-stamps generated subtrees from their owning declaration for diagnostics;
source stamping is no longer relied on as semantic identity.

## Stage 2: Comptime Elaboration

Module:

```rust
src/comptime.rs
```

Entry point:

```rust
comptime::elaborate(program: Vec<Stmt>) -> Result<Vec<Stmt>, ComptimeError>
```

`comptime` is implemented as a phase distinction before type checking. The
elaborator rewrites compile-time constructs into ordinary AST so the checker,
HIR, MIR, and VM do not need to carry special `comptime if` or `comptime for`
semantics.

Compile-time values are represented by:

```rust
CtValue::Int
CtValue::Bool
CtValue::Str
CtValue::Tuple
CtValue::List
CtValue::Type
CtValue::Param
```

The implemented forms are:

- `comptime NAME = expr`: evaluates `expr` immediately, records the result in
  the compile-time environment, and keeps a folded declaration whose value is a
  literal.
- `comptime if`: evaluates each condition as a compile-time `Bool` and keeps
  only the selected branch. Dropped branches disappear before type checking,
  which lets them contain code that would be invalid for the selected
  specialization.
- `comptime for`: evaluates the iterable as either `range(...)` or a
  compile-time tuple/list, substitutes the loop variable with a literal, and
  splices a fresh elaborated copy of the loop body for each element. A
  zero-step range has no elements in both this direct evaluator and the
  VM-backed CTFE path; the shared loop predicate simply never enters.
- CTFE calls: a compile-time expression may call a pure top-level `def`,
  including value-parameterized helpers and helpers whose type parameters are
  used only for compile-time facts. The elaborator clones the needed helper call
  graph, folds compile-time-only operations such as `is_same_type[T, U]()` and
  `T.size` out of the cloned bodies, and executes the resulting helper through
  HIR, MIR, and the register VM in compile-time mode.
- Materialization: module-level `comptime` constants are inlined as runtime
  literals into later code, so a function can use a constant computed at module
  elaboration time.

Generic `def` templates monomorphize in two classes sharing one worklist,
mangling, and clone generator. A **comptime-class** template (a `comptime
if`/`for` body, or a type pack) must specialize at every reference: resolution
failure is an error, and the template is replaced by its clones (a dead
template is dropped unchecked). A **bound-generic** template — a plain
trait-bound generic `def` with no comptime constructs and a unique top-level
name — resolves softly: only an explicit application whose arguments resolve
concretely monomorphizes, while inferred calls, symbolic arguments, and
function-value uses stay on the template's abstract erased-dispatch path and
retain the template; a template with no references at all also survives,
keeping its Mojo-style abstract pre-check. In both classes each clone bakes
its concrete type arguments into every remaining type position — annotations,
compile-time argument lists, and constructor heads — and drops them from the
residual signature and the rewritten calls, so the clone checks concretely.
Because the checker never re-validates a dropped argument, the resolver
enforces each dropped parameter's declared trait bounds at the requesting call
through the conformance oracle. Type packs, callable-value bindings, and
types that do not round-trip to source syntax remain symbolic on the residual
signature.

Inferred applications reach the same clones through the compiler's discovery
fixpoint: `Compiler::compile_linked` iterates elaborate→check, deriving
`DefSpecializationRequest`s from the checker's recorded generic
instantiations (closed arguments only, keyed by occurrence span with the
phase-local syntax id stripped) and re-elaborating the original linked
program with the accumulated monotone request set until a round discovers
nothing new; a hard round cap reports inferred polymorphic recursion as a
dedicated divergence diagnostic. Request seeding records each occurrence's
target without queuing work; the clone job queues lazily when the soft
resolution path fails on source arguments and consults the request — so a
request can only upgrade a call from the abstract path, and a drifted or
conflicting request (a `comptime for` unrolling duplicates one source
occurrence) leaves the call abstract. A retained bound-generic template is
emitted before its specializations because a clone may still reference the
template abstractly (an inferred recursive call) and the checker binds
top-level names sequentially.

Under the production compiler, the erased-dispatch machinery
(`__trait_dispatch.*`/`__iterator_dispatch.*` symbols, VM retargeting, and
the `CopyIteratorReference` result adapter) is therefore reachable only
through retained-template residue: function values/indirect calls,
overloaded generic names, generic methods, comptime-class inferred calls,
open instantiations, conflicting unrolled occurrences, and abstract-body
pre-checks. Its verification witnesses live in `mir::verify`:
`verify_iterator_result_adapter`, the `GetIter` undeclared-prepare
tolerance, the subscript abstract-target tolerance, the `MethodCall`
abstract-`__next__` adapter symmetry, `CallIndirect`'s callable-contract
validation, and the direct-`Call` undeclared-callee tolerance — the set the
backend-ready MIR checkpoint re-confirms before freezing the schema.

The important distinction is that the elaborator still owns compile-time AST
rewriting, while function-body execution now goes through the MIR/VM path. The
remaining expression evaluator in `src/comptime.rs` is not a second function
runtime; it exists to decide `comptime if`, enumerate `comptime for`, resolve
type-valued compile-time facts, and fold those facts before a CTFE helper is
lowered to MIR.

### VM-Backed CTFE

When a compile-time expression calls a helper `def`, the elaborator first resolves
the explicit compile-time arguments into `CtValue`s. Value parameters are passed
to the VM as reified frame locals; type parameters remain compile-time facts in
the elaborator's environment.

Before lowering the helper for CTFE, the elaborator walks the transitive helper
call graph and rejects runtime effects:

- `print`
- `raise`
- pointer allocation
- methods and user-value dunder dispatch
- `try`
- nested declarations
- keyword calls and other unsupported runtime forms

For the accepted call graph, the elaborator clones the needed top-level `def`s.
In the root helper body it folds compile-time-only expressions into ordinary
runtime literals:

```mojo
return T.size
```

may become:

```mojo
return 8
```

for an instantiation such as `capacity[Buffer[8]]()`. Similarly,
`is_same_type[T, Int]()` is replaced with a `Bool` literal. After this rewrite,
the cloned helper program is ordinary AST and can be lowered through the same
HIR/MIR/VM machinery as runtime code.

The VM has a narrow CTFE entry point:

```rust
VmBackend::run_function_value(...)
```

It executes a named top-level helper without running `__toplevel__` or `main`,
burns the shared compile-time fuel budget, and returns a runtime `Value` plus the
remaining fuel. The elaborator converts the result back to `CtValue`. Exact
`IntLiteral`/`FloatLiteral` values and runtime-materializable `Int`, `UInt`,
`Float64`, `Bool`, and `String` values can cross that boundary. Compile-time
lists cross through the CTFE-only `Ty::ComptimeList`/`Value::ComptimeList`
carrier; compile-time tuples cross through the same private heterogeneous
`Ty::Tuple`/`Value::Tuple` storage used by specialized runtime packs. Public
`List` and `Tuple` values are nominal structs and do not use either bridge.

### Fuel

In this codebase, **fuel** means a compile-time step budget. It is not a runtime
performance mechanism and not user-visible gas. The current budget is a fixed
program-wide quota:

```rust
const FUEL: usize = 100_000;
```

The elaborator burns fuel for expression-level compile-time work and
`comptime for` unrolling. VM-backed CTFE burns from the same budget for function
calls, basic-block execution, and instructions. If the budget reaches zero,
elaboration fails with a compile-time quota error.

The goal is to prevent compile-time execution from hanging the compiler. A bad
`while True` in a CTFE function or an enormous generated loop should fail
deterministically instead of making compilation unbounded. This is similar in
spirit to Zig's compile-time branch quota, though mojito keeps the mechanism
small and fixed for now.

### Checker Interaction

The checker still has a narrow constant folder for value-parameter contexts such
as SIMD widths and simple value-parameterized types. The comptime elaborator now
runs before the checker, so CTFE-computed values are folded into literals before
those checks run.

That layering is useful but not final. Today there are two related mechanisms:

- `src/comptime.rs` handles language-level `comptime` declarations, branch
  selection, loop unrolling, materialization, and CTFE.
- `src/checker.rs` still validates type/value-parameter positions and folds the
  small expression subset it needs for those positions.

`ParamDecl::Value` retains its declared checked type and optional compile-time
default. The shared `CtValue` model carries integers, booleans, strings,
tuples/lists, types, symbolic parameters, and zero-sized reflection handles.
Only literal-shaped values materialize into runtime AST; type and reflection
handles are consumed and erased during elaboration.

### Exact Numeric Literals

Numeric source literals have dedicated compile-time types and representations.
`IntLiteral` owns a `num_bigint::BigInt`; finite `FloatLiteral` owns a reduced
`num_rational::BigRational` plus a negative-zero bit. Lexer tokens, AST nodes,
`CtValue`, checked constants, MIR constants, and VM CTFE bridges preserve those
values without first passing through an `i64` or `f64`. Literal arithmetic is
therefore exact, subject only to the compiler's explicit exponent/shift resource
quota.

Contextual typing selects the one transition to a runtime scalar. The checker
records `SemanticAdjustment::MaterializeLiteral(target)`, HIR retains that
decision, and MIR emits `MaterializeLiteral { value, target }`; the verifier
requires an exact-literal source and compatible concrete target. Integer targets
use destination-width two's-complement wrapping. Floating targets round once
from the exact rational directly to binary32 or binary64, preserving signed
zero and producing IEEE infinity on overflow. Bindings, stores, calls, returns,
typed tuple/list/set/dictionary elements, and `range` arguments all record their
scalar boundaries rather than relying on VM container coercion.

Generic value parameters are also materialization boundaries. Their resolved
`ParamDecl`s cross `CheckedProgram` through stable declaration-owned
`GenericSite`s into MIR declaration metadata, so the VM reifies a value at its
declared type without reclassifying source bounds. This prevents an exact
literal from leaking into an erased runtime slot and keeps hashing, equality,
stores, calls, and returns consistent. The defensive erased-value path compares
finite numeric values in one exact rational domain and hashes that canonical
form, including treating positive and negative zero as numerically equal.

Current Mojo reflection enters through the zero-sized `reflect[T]` compile-time
handle. Mojito implements `is_struct`, `field_count`, `field_names`,
`field_types`, and `field_index`. The type-valued aliases `.field[name]` and
`.field_at[index]` return another reflection handle, so selection composes across
nested structs and the terminal handle's `.T` supplies the concrete dependent
type. A reflected type list can likewise be indexed in type position, so a
specialization may use `types[i]` as a concrete dependent type. The removed
`field_type` alias is diagnosed during elaboration rather than leaking into
ordinary member or call checking.

## Stage 3: Semantic Checking

Entry point:

```rust
checker::check(program: &[Stmt]) -> Result<(), TypeError>
checker::check_program(program: &[Stmt]) -> Result<CheckedProgram, TypeError>
```

`check` is the compatibility validation wrapper. The compiler pipeline uses
`check_program`, whose checked handoff owns the elaborated AST for diagnostics and
an explicit semantic arena. Every checked expression has a `CheckedNodeId`, child
edges, resolved runtime type, value/place/type category, stable owner identity for
binding uses, extensible effect facts, and a list of semantic adjustments. Checked
declarations have independent `CheckedDeclId` identities. Source spans index
diagnostics and associate a checker-approved compile-time value argument with
the register produced from that same source occurrence; they are not used to
re-resolve a type, overload, effect, or origin decision.

Call targets, implicit conversions, moves, explicit-destruction decisions, and
reference-handle preservation are stored canonically as `SemanticAdjustment`
values on checked nodes. `HirExpr` recursively retains those checked children in
AST structural order. MIR pairs the active HIR syntax tree with that recursive
semantic tree by node identity. One narrow association remains span-keyed:
source-ordered compile-time value arguments reuse registers already evaluated
for the corresponding checked subscript operand. This is a register handoff,
not semantic reconstruction. Register spans otherwise serve diagnostics only.
SIMD dtype and width selection is likewise a checked adjustment rather than
MIR-side syntax evaluation. Span-indexed call/conversion maps remain only as
public compatibility queries and are not part of lowering.

Specialization value forms include compile-time struct instances and dtypes:
`CtValue::Dtype` binds a `[dtype: DType]` parameter, and `CtValue::Struct`
freezes a fieldwise-constructible, recursively pointer-free struct instance
produced by VM-backed CTFE (a constructor or static-method call runs through a
synthesized entry against the checked CTFE subprogram). Both monomorphize
their declarations before checking — the checker never sees a symbolic dtype
or struct value, no MIR schema is affected, and a frozen instance materializes
back as its ordinary fieldwise construction wherever the specialized body
reads the parameter. Freezing and materialization are inverses by
construction: the freeze precondition (fieldwise constructor, freezable
fields) is exactly what guarantees the materialized construction re-creates
the same value.

`SemanticAdjustment::SelectedCall` is the canonical method-like boundary for
ordinary method calls and method-dispatched nominal subscripts. It records the exact lowered
target, executable result type, and typed raising effect; declared
receiver/argument place requirements independently from origin-solved effective
access; source-to-parameter binding,
including defaults; capture origins; reference-result origin; and source-ordered
compile-time parameter arguments and declarations. `SliceDescriptors` is an
orthogonal adjustment, so descriptor selection and call semantics coexist
instead of overwriting one another. Because a descriptor is compiler-synthesized
rather than a checked source child, overload selection permits only exact or
descriptor-family coercion for it; arbitrary user `@implicit` construction is
rejected before HIR/MIR. Ordinary index operands remain recursively checked
source expressions and retain their selected executable conversions.

The arena deliberately does not encode constructs as a closed expression opcode
ABI. Non-exhaustive declaration/category/adjustment families and independent child
edges allow future pattern binders, class declarations and dispatch, coroutine
suspension, generators, or continuation resume edges without pretending those are
ordinary calls or exceptions.

The older compatibility query remains available to focused tests:

```rust
checker::resolve_overload_targets(program: &[Stmt]) -> Result<HashMap<Span, String>, TypeError>
```

The checker consumes the parsed AST and rejects programs that the later compiler
does not want to reason about.

It is responsible for:

- names and local scopes
- builtin scalar types
- struct declarations and field layouts
- function and method signatures
- overload sets and selected overload targets
- trait declarations and a limited trait-conformance model
- trait receiver conventions and associated compile-time facts
- type parameters and value parameters
- call argument matching
- default, keyword, and variadic arguments where supported
- `var`, `mut`, `ref`, and `deinit` conventions
- compile-time integer constants used as value parameters
- nominal collection, private runtime-pack, Variant, string, and SIMD type rules
- typed and parametric raising effects, inferred handler types, and the `Never`
  bottom type
- borrow checking for call arguments
- rejecting parse-only syntax whose semantics are deferred

The checker is deliberately conservative. If a construct is parsed but not
semantically implemented, this is where it should normally become
`TypeError::Unsupported`.

An implicit assignment introduced inside a branch or loop is allocated in the
enclosing function's stable binding scope, not the transient block scope. The
checker separately carries a set of maybe-uninitialized owner identities across
CFG-shaped source joins: the name remains resolvable after the block, but a read
is rejected unless every reachable predecessor initialized it. Loop joins retain
the zero-iteration path.

Explicit declarations instead keep one runtime slot per stable checked binding,
including same-spelled declarations in sibling scopes produced by compile-time
unrolling. HIR may suffix the internal slot name, but downstream place lookup is
by owner identity; independently inferred sibling types therefore never merge
in `MirFunction::var_tys`. A substituted local `ref` alias has no runtime handle
payload, yet its analytical slot retains the checked `Ty::Ref` capability used
by every `MirPlace::through` access. Opaque structured statements may retain a
source spelling after HIR has assigned a shadowed declaration a suffixed slot,
so MIR resolves identifier writes through the checked `OwnerId` exactly as it
does reads; the two halves of augmented assignment cannot select different
same-spelled bindings.

Public collection types, including heterogeneous `Tuple[*Ts]`, cross the checked
boundary as `Ty::Struct` with their concrete type arguments. `Ty::ComptimeList`
exists only while materializing `CtValue::List`; `Ty::Tuple(Vec<Ty>)` is the
compiler-private heterogeneous pack carrier and is never the type of a public
tuple expression. `Ty::Variant(Vec<Ty>)` retains alternative order at the
checked boundary. Variant construction and the
parameterized `isa[T]`, projection, and `set[T]` operations record the selected
alternative as a `SemanticAdjustment`; MIR therefore receives a numeric tag and
never guesses one from source spelling. `Value::Variant` repeats the checked
alternative list beside its active tag as a defensive runtime consistency check.

Trait refinement is flattened during checking: inherited method and associated
compile-time requirements become part of the refined contract, and a refined
bound satisfies its ancestors. Before checking, executable trait defaults are
materialized as ordinary methods on each conforming struct. An explicit struct
method wins; unresolved defaults from multiple paths are rejected. MIR and the
VM retain static dispatch and need no trait-object representation.

Associated compile-time members may be monomorphic or parameterized.
`TraitComptime` and `StructComptime` retain a name and requirement/value plus an
optional member-local parameter list (type, value, and origin parameters, with
the `//` infer-only boundary), and `Type::Assoc`/`Ty::Assoc` carry application
arguments (`TyArg::Ty`/`Val`/`Origin`, the origin erased from the runtime ABI like
a pointer origin). A type-parameterized member instantiated by a conforming struct
resolves concretely by substituting those arguments into the member's lowered
template. The bundled owned iterator protocol uses the monomorphic
`IteratorOwnedType` member. Current Mojo's borrowed
`IteratorType[iterable_mut: Bool, //, iterable_origin: Origin[mut=iterable_mut]]`
and the dependent application `Self.IteratorType[origin_of(self)]` parse, check,
and arity-validate.

An `origin_of(self)` argument on a trait method's *abstract* signature has no
bound `self` place, so it lowers to the symbolic `Origin::SelfParam` — the
`Origin`-level analogue of the signature contract's `SigOrigin::Self_` — which
carries the receiver origin through the associated-type application and, like
every non-`Place` origin, erases from the runtime ABI (collapsing to the single
mangling marker). A conforming struct then resolves the origin-parameterized
member concretely, so a requirement returning `Self.IteratorType[origin_of(self)]`
is satisfiable and conformance succeeds. The borrowed `Iterable` proof protocol
uses this origin-parameterized member, and the bundled List/Set iterator
carries its origin as an erased struct parameter (with an infer-only `Bool`
binding its `mut=`, likewise erased), borrows its source through a `ref`
field, and yields element references whose mutability the checker resolves
from the source at each loop site. Concrete List/Set/Dict borrowed iteration
keeps its checker-attached interior `element` loan; the mapping iterators
remain snapshot/copy bridges until mapping invalidation lands.

Trait method requirements retain `raises` and an optional concrete error type.
A nonraising implementation may satisfy a raising requirement; a raising
implementation must not widen the requirement's error family. Bounded method
selection substitutes that contract, records it on the checked call expression,
and passes it through HIR to the call instruction just like direct dispatch.

Mojo defines one additional directional refinement for methods named
`__next__`: a concrete `ref[o] T` return may satisfy an abstract value return
`T` when the referent is identical and `T: Copyable`. It does not apply in the
reverse direction or to linear referents. Trait registration retains the
concrete reference ABI, while the abstract call contract carries
`CheckedResultAdapter::CopyIteratorReference`. The adapter survives HIR and MIR;
after runtime dispatch selects the concrete declaration, the VM tests that
declaration's return ABI and, only for a reference ABI, performs the semantic
read and `__copyinit__`. This ABI test is essential because a value-returning
method may legitimately return a reference-valued element. MIR verification
requires the adapter on abstract value-result `__next__` calls and forbids it on
concrete calls. The VM temporarily restores both the executing caller and the
identified just-completed iterator frame while user copy code runs; reference
handles nested inside a `Copyable` result therefore resolve against real storage
and any permitted write-through is preserved.

Opaque trait-bounded collection indexing uses the requirement signature for its
index and result types, then executes through concrete dunder dispatch after
erasure. The standard `Indexer` contract is modeled separately. When overload
selection needs an `Int` index and no direct subscript overload accepts the
source index type, the checker records the exact `__mlir_index__() -> Int`
normalization. Lowering evaluates the source expression once, emits that selected
call explicitly, and gives the later index operation its `Int` result; a backend
does not rediscover the conversion from the runtime value. The VM uses `Int` for
that result because it has no distinct MLIR index representation.

Scalar operators, comparisons, conversions, and rounding are typed through
checked operation traits, not ad-hoc numeric rules. Each binary/prefix operator
names a trait (`Addable`/`Subtractable`/…/`Comparable`/`Equatable`/`Negatable`,
the bitwise and shift ones integer-only) whose dunder the checker resolves; the
same traits serve as generic bounds, so `def f[T: Addable]` type-checks and a
struct declaring an operation trait must define its dunder. Conversions
(`Int`/`Float64`/`Bool` via `__int__`/`__float__`/`__bool__`) and `abs`/`round`
(`__abs__`/`__round__`) route the same way for concrete structs, matching the
paths opaque parameters already used. Builtin scalars keep primitive execution
(`apply_infix`/`apply_prefix`) behind the protocol; a struct operand dispatches
through its dunder (`apply_binop`/`apply_prefix`, and the `abs`/`round`/`Int`
builtins). Recording the resolved operator dunder as a MIR adjustment for the
textual schema is deferred to that milestone, where its shape can be validated
against a real consumer.

Augmented assignment (`place OP= rhs`) on a user-defined value dispatches to the
dedicated in-place dunder rather than the binary operator: the checker selects
`__iadd__`/`__isub__`/… as a full `mut self` `CheckedCallContract` and records it
as an `AugmentedInPlace` adjustment on the place, so lowering emits an ordinary
receiver-committing `MethodCall` (the mutation writes back through the receiver's
slot, alias, or reference handle) instead of a `BinOp` read-modify-write. There
is no fall-through to `__add__`; a missing in-place dunder is a checker error.
Native scalar targets keep the primitive `BinOp` path. A user-struct
nominal-subscript element dispatches the same way, recorded on the
`CheckedAugmentedSubscript`: lowering materializes the element into a mutable
temporary and sends the mutated result through `__setitem__` (value getter) or
reads it through the reference handle (mutable-reference getter), applies the
in-place dunder to that temporary, and commits the result.

Printing and `String()` require `Writable`. A custom `write_to` or
`write_repr_to` receives `Some[Writer]`; `Writer.write` accepts heterogeneous
Writable values and ultimately feeds UTF-8 strings to `write_string`. The VM's
buffer writer uses `String` as the `StringSlice` representation. When a Writable
method is absent, field reflection produces the default display or repr form.
`String.format` supports automatic/manual fields, repr selection, escaped braces,
and accepts format-spec syntax; the scalar width/alignment mini-language remains
representation-level work rather than a return to deprecated `__str__` hooks.

Hashable values contribute to a caller-provided `Some[Hasher]` through
`__hash__`; `Hasher.update` accepts any checked Hashable and incrementally mixes
it into state. The bundled IncrementalHasher exposes `__init__`, `update`, and
`finish`; its VM word-sized `UInt` result represents the standard `UInt64`
contract in the current target-independent VM.

Examples of syntax that may parse before it is fully implemented include richer
trait features, `with`, and advanced expression/declaration forms that the VM
does not yet execute.

### Overload Resolution

Top-level functions, methods, trait requirements, and constructors may form
overload sets. The checker represents a repeated top-level `def` name as
`Ty::Overload(Vec<Ty>)`; struct and trait methods use a per-name list of
`MethodSig`s.

Duplicate-equivalent signatures are rejected. Distinct arities are naturally
different signatures, and same-arity signatures are allowed when their parameter
types differ.

At a call site the checker:

1. collects the candidates for the source name
2. filters candidates by call shape, explicit type/value arguments, and argument
   type compatibility
3. ranks surviving candidates lexicographically by conversion count, variadic
   use, parameter-signature length, and generic/concrete tie-break
4. accepts the unique lowest-score candidate
5. rejects no-match and tied-best cases

This first-pass ranking includes validated, nonraising user-defined `@implicit`
constructors. The checker records the uniquely selected converting constructor
at the source expression and MIR emits that call before its consumer. For
example, a typed `String` value selects `f(x: String)` over `f(x: Int)`, and an exact
`Int` argument selects `f(x: Int)` over a candidate requiring widening. A bare
integer literal passed to both `f(Int)` and `f(Float64)` is ambiguous because it
can materialize as either. Alpha-equivalent generic declarations are rejected,
while generic overloads with genuinely different bounds receive distinct lowered
symbols that include those bounds. Nested-def overload sets are rejected until
the lifting path can preserve their selected identities safely.

The important architecture point is that overload selection is static. The VM
does not inspect runtime value tags to choose between same-arity candidates.
The checker records the result on the checked node as `ResolveCallable` or the
full `SelectedCall` adjustment; recursively checked HIR carries it into MIR,
which preserves the selected callee through execution. A source-span map remains
only for compatibility queries and is not a semantic lowering input.

### Generic Implicit Conversions

Implicit conversion lookup first resolves the contextual target struct. For a
parameterized target such as `Box[Int]`, Mojito substitutes those checked
arguments into converting constructors before testing source compatibility and
constraints. It diagnoses ambiguity after substitution and records the selected
specialized constructor identity for MIR lowering. The same deterministic
ranking compares exact/coercing candidates, user conversions, variadic use,
signature length, and generic/concrete specialization; the VM never repeats
overload or conversion selection dynamically.

### Borrow Checking In The Checker

The borrow checker currently lives with call checking because the checked
operation is local to one call expression.

For each argument, the checker classifies the operation:

- ordinary read/shared borrow
- `mut` or `ref` exclusive borrow
- consuming move via `^`

It then applies the mutable-XOR-shared rule by root/place. The checker is
place-sensitive enough to allow disjoint field borrows such as:

```mojo
f(mut p.a, mut p.b)
```

but reject conflicting uses of the same root/place such as:

```mojo
f(mut p, p)
f(mut p, p^)
```

This early borrow check complements, rather than replaces, MIR ownership
analysis. The checker handles local aliasing at call boundaries; MIR analysis
handles move state across control flow.

### Multi-Element Pointer Origins And View Loans

An origin-bearing pointer to a precise place designates exactly one value and
dereferences only at offset 0. A pointer origin **projected into an
interior-generation domain** (`origin._get_owned_interior["tag"]`, whether a
concrete place tail or a projected origin parameter) is the multi-element
form: offsets are legal (the VM's arena bounds check is the dynamic
backstop), the projection is part of the type so the discriminator also fixes
the runtime representation (allocation arithmetic, never a frame/slot
handle), and the origin's loan engages the ordinary interior-generation
staleness machinery. `unsafe_origin_cast` rebinds provenance without a runtime
operation (lowering forwards the receiver register; MIR verify compares
pointer ABI modulo origin) and never upgrades a statically immutable
capability.

Borrowed views (`Span`, `StringSpan`) are pointer-plus-length structs over
that capability. Their loans flow through three checked channels: a
constructor's `ref [origin]` parameter records `BorrowRefArguments` (the
binding loans each lent place), a view-typed subscript result records
`BorrowViewResult` (the result inherits the receiver's loans, falling back
to the receiver's own place for a plain owner), and `unsafe_origin_cast`/
`unsafe_offset` results carry their rebound or forwarded origins through the
aggregate-origin walks. The loans are whole-place and shared, so reads
coexist while structural mutation of the source conflicts with any live
view.

The **conservative subtree form** (`origin._subtree`, current Mojo's
experimental `Origin._subtree`) is a third origin shape beside precise
places and named interior generations: a terminal `OriginSeg::Subtree` on a
place path (or the `subtree` flag on a symbolic `Param`/`SelfPlace` pointer
origin, appended to the solved place after the interior tags). It
deliberately forgets which descendant of its base the pointer designates,
which drives every rule downstream. It is single-element (`multi_element()`
is false, offsets reject), the deref-place substitution refuses it
(`pointer_deref_place` — the runtime handle carries the true projection, so
the checked origin must never re-derive one), and for overlap the terminal
segment is a wildcard over every descendant. Its loan is a lazy interior
generation whose staleness predicate (`interior_origin_invalidated_by`)
drops both directional requirements the named-generation rule keeps: a
mutation base above, at, or below the subtree base invalidates it, and no
`Interior` segment is required below the mutation base. First-write
self-invalidation costs no new dataflow state: a pointer store records an
invalidation at the pointer's own source place with no protected handle,
that base carries the subtree tail, and because use-checking runs before
the transfer function the writing instruction still sees the live
generation while every later use observes the invalidation. The `except`
handle that ordinarily protects a reference from its own mutation is
carved out for subtree generations for the same reason. MIR verify accepts
a domain loan whose path ends in — but never continues past — the subtree
segment, and rejects subtree segments in transfer destination domains.

## Stage 4: HIR CFG Lowering

Module:

```rust
src/hir/mod.rs
```

Main type:

```rust
hir::Cfg
```

HIR is the first control-flow-aware representation. It is a graph of basic
blocks backed by `petgraph::StableGraph`. Expression-bearing instructions and
terminators carry `HirExpr`: diagnostic syntax paired with a stable checked node
identity, resolved type, value category, and explicit semantic adjustments.
Opaque compatibility statements carry the checked expression identities within
their source extent. HIR retains responsibility for statement control flow.
The CFG design admits additional edge kinds for suspension/resumption and pattern
failure without baking them into expression lowering.

Each HIR block has:

```rust
pub struct BasicBlock {
    pub instrs: Vec<HirInstr>,
    pub term: Option<Terminator>,
}
```

Each block is sealed with exactly one terminator:

```rust
pub enum Terminator {
    Jump(BlockId),
    Branch { cond: Expr, then_b: BlockId, else_b: BlockId },
    Return(Option<Expr>),
    ReturnWithCleanup { value: Option<Expr>, cleanup: Vec<VarId> },
    FallOff,
    EscapeJump(BlockId),
}
```

Iterator-driven loops use `ReturnWithCleanup` when a source `return` bypasses
their common exit. The return expression is materialized first, then the current
loop binding and synthetic iterator owners are destroyed from the innermost
loop outward.

The core invariant is:

> Every block has one terminator, and terminators own the outgoing control-flow
> shape.

That invariant makes later MIR and analysis passes graph-driven rather than
syntax-driven.

### Variable Slots

HIR also interns variables into stable `VarId`s.

```rust
pub type VarId = u32;
```

Function parameters are seeded first, so parameter slots are stable:

```text
vars[0..n_params]
```

This becomes the VM call ABI later. A callee frame receives argument values by
writing them into the first `n_params` variable slots.

A free-function named `out` result is deliberately outside that ABI prefix. MIR
seeds it as an uninitialized callee-local slot, rewrites fallthrough and bare
returns to read that slot, and exposes its declared type as the function result.
The caller therefore invokes a named-result function exactly like an ordinary
returning function and never supplies the `out` argument.

Trailing `where` clauses compile once into the checked `GenericConstraint`
algebra. The `(condition, "message")` form becomes a `WithMessage` wrapper:
evaluation, implication, conformance assumptions, and specialization-closure
checks recurse through the semantic condition, while a failed call reports the
retained message. This keeps diagnostics attached without making message text
part of generic identity or proof semantics. The same wrapper is retained on
struct declarations, conditional conformances, method availability, associated
and trait comptime members, and comptime declarations.
Associated-member constraints are validated at concrete projection; a
conditional-conformance failure feeds its retained reason into lifecycle and
trait diagnostics. Origin-mutability-only function constraints survive the
ordinary generic erasure in `CallableOriginSignature` and are discharged after
call-origin solving recovers the inferred Bool binding. The checked constraint
contract is plural: every declaration family stores one compiled constraint
per trailing `where` clause, the first failing clause reports its own message,
and truth-only operations (implication, inherited-requirement merging) fold a
clause list into one conjunction without erasing the stored per-clause
messages. Per-trait conditional-conformance conditions stay single-clause.
Generic top-level comptime aliases lower once into the checker's alias
registry (classified `ParamDecl`s plus a symbolic template shared with
parameterized associated members) and expand per application in type
resolution through the same `resolve_use_params` contract as a struct
application; an alias is a pure type declaration with no runtime form, skipped
at MIR program assembly like a struct or trait.

A variadic type parameter retains a leading `*` in the checked parameter name.
Generic-call inference recognizes the matching `*args: *Pack` element type and
checks each overflow argument independently against the pack bounds instead of
forcing all arguments to unify to one type. Specialization records the concrete
sequence as the internal checked `Ty::RuntimePack([T0, ...])` call ABI; the VM
materializes that collector in private `Value::Tuple` storage. An ordinary
homogeneous `*args: Tuple[...]` instead remains a nominal `List` whose repeated
element type is the public nominal `Tuple`. This supports heterogeneous calls
and pack length queries without confusing a public tuple with the private
`Ty::Tuple` carrier. Specialization infers literal
and directly constructed call-argument types, binds the pack's type tuple into
the compile-time environment, and unrolls `args.__len__()`-driven loops.
Because specialization consumes this generic call before whole-program checking,
it queries a declaration-only checker conformance oracle first. Every inferred
element is tested against every declared pack bound at the requesting call site;
a failure names the one-based element number, concrete type, pack, and trait.
The oracle reuses the authoritative conformance rules and records trait
refinement, nominal/conditional conformances, field types, and lifecycle-method
presence without checking method bodies early. Full conformance verification
still runs on the elaborated program.
Each unrolled static index substitutes its concrete element type while retaining
the declared common bound for operations that are not specialized to one index.
Pack expansion necessarily runs before checked `OwnerId`s exist, so it owns a
small lexical resolver with private monotonic `ElabBindingId`s and independent
value/type namespaces. A runtime pack is identified by its specialized
`$pack[T0, ...]` parameter declaration and its length is keyed by that binding;
concrete type-pack substitutions are likewise keyed by the specialization's
type binding. The walker mirrors declaration order and the lexical scopes of
branches, loops, handlers, comprehensions, nested definitions, generic
parameters, and individual struct methods. A same-spelled local therefore gets
a different identity, leaving an unsupported ordinary spread for the checker,
while leaving the scope restores the outer pack. Presence in the binding map —
not a nonzero length — identifies an empty pack. HIR loop lowering follows the
same rule by assigning the loop target a scoped runtime slot instead of
reusing/leaking an outer same-named slot.

After top-level specialization, a contextual nested pass gives every
specializable nested declaration a private scope-qualified marker. Calls resolve
that marker through lexical value scopes, request independent specializations,
and emit the generated declarations at the template's original source site.
The parent specialization is part of the marker identity, so the same nested
syntax in two outer instances cannot collide. A parallel lexical runtime-pack
environment snapshots explicitly captured outer packs, masks them under local,
parameter, loop, comprehension, import, and walrus bindings, and restores them
when a scope ends. Generated declarations retain defaults, keywords, named
results, capture lists, effects, and their concrete variadic ABI when nested
function lowering registers the lifted MIR declaration.

Pack forwarding first flattens the one known spread into a virtual positional
type sequence and runs the shared call-slot matcher; only positional overflow
becomes the target pack's inferred type list. The spread must follow every fixed
positional argument, while parameters after the variadic collector are supplied
as keywords or defaults. Specialization gives that target instance a regular
private runtime-pack collector slot and moves the complete collector as one
value. This
preserves linear elements without inventing an illegal move through `args[i]`.
Both the top-level and lexical nested specialization passes reject a second
spread, explicit positional overflow after the spread, or a non-pack target;
those are current-Mojo rejections, not deferred concatenation features.

A variadic **struct** template (`struct S[*Ts: Bound]`) is specialized the same
way: compile-time elaboration keeps the template verbatim, resolves every
explicit instantiation (calls, `TypeApply` expressions, and type annotations) to
a mangled fully concrete struct, resolves pack-applied member annotations such
as `Tuple[*Ts]` to the corresponding concrete nominal specialization, rewrites
a pack-typed method
parameter to the `$pack` element list, unrolls current Mojo's dependent
accessor `__getitem_param__[i: Int] -> Ts[i]` into per-element concrete methods,
and drops the template. The earlier `__getitem__` spelling follows the same
path only as an explicit compatibility fallback. Every specialization reuses
the template's spans (correct
provenance), so struct annotation sites are identified by the struct's unique
name and each specialization's subtree — and each unrolled accessor body — is
stamped with a distinct source tag, keeping span-keyed checked facts separate.
If an element is a function value, its exact checked `Func`/`GenericFunc`
contract cannot be losslessly respelled as a source `def(...)` annotation:
defaults, variadics, generic declarations, capture origins, and typed errors are
richer semantic facts. Generated Tuple AST therefore carries only an opaque,
parser-unconstructible callable-type id. The compiler seeds the final checker
pass with the corresponding `Ty`; the source AST never embeds or reconstructs
that semantic type.
The checker resolves a subscript on such a struct at a compile-time-constant
index and records its complete selected accessor contract. MIR `Index.call`
carries the resulting `MirSubscriptCall`, and the VM dispatches ordinary value
reads, explicit reference bindings, and chained receiver/place uses without
name or effect derivation.

A non-pack struct may also define `__getitem_param__[i: Int]`: subscript
checking supplies the source index as a compile-time value parameter, retains
the exact generic-method target, and reuses the already evaluated source-index
register for the value-parameter ABI. This is separate from runtime
`__getitem__(index)` dispatch and does not add an ordinary positional argument.

The implicit prelude exposes `List`, `Set`, `Dict`, `Optional`, `Range`, and
`Tuple` as ordinary bundled structs (upstream requires an explicit import for
the `Set` name — a recorded divergence pending display-lowering identity
plumbing). List, set, and dictionary displays lower to the
selected nominal constructor; comprehension leaves lower to ordinary
`append`, `add`, or `__setitem__` calls. Range syntax is an ordinary overload of
the bundled `range` function. Tuple displays request a concrete specialization
of `Tuple[*Ts]`, whose private `__RuntimeTuple[*Ts]` field is the only source
construct that lowers to `MakeTuple`. Indexing, sizing, containment, comparison,
reversal, concatenation, and tuple-element consumption therefore use checked
methods just as they do on user structs. List, Set, Dict, and Range borrowed
iteration likewise follows selected nominal methods; consuming bundled
iteration is currently List-specific. Public Tuple has no runtime `__iter__`
contract.
Conditional lifecycle conformances are folded per specialization: a `deinit`
method may implicitly copy a named tuple only when every element is
`ImplicitlyCopyable`; otherwise the call must transfer the tuple with `^`.

Borrowed-source iteration lowers uniformly, in statements and comprehensions
alike. The checker records a borrowed origin for a named source — a concrete
List/Set/Dict place gains an `element` `Interior` segment, while a named
user-struct iterable records the *whole* source place — and HIR `BorrowIter`
preserves that place instead of invoking its value copy lifecycle. MIR always
binds the retained source as a genuine reference (`MakeRef` into a
`Ty::Ref`-typed retained-source slot; a borrowed `__iter__(ref self)` re-roots
at the source, so the handle is read only by `GetIter` and dropped afterward
as a no-op) and normalizes the iterator into a distinct iterator-object slot.
Whole-source versus interior borrowing is expressed only as loan granularity:
`EstablishLoans` carries either a whole-place shared loan or an interior
`element` generation, re-established on the long-lived iterator-object slot so
the source stays live through the loop and mutation of it during iteration is
rejected. List iteration additionally observes element replacement and rejects
structural invalidation before a later iterator use; `for ref` binds the
yielded reference handles of the ordinary protocol, with the iterator's source
loans re-established on each binding. The remaining collection-specific
checker rules (the interior-`element` attachment and the mapping snapshots)
stay narrow until generic-bound origin derivation lands with mapping
invalidation. The only method-free
collection behavior in the VM is the explicitly CTFE-only `ComptimeList`
bridge; the separate method-free tuple-shaped path is compiler-private
runtime-pack storage, not a public collection.

Tuple specialization is a closed-set, two-phase handoff. The discovery check
collects every public Tuple element sequence and only the transforms actually
called on each receiver; flexible numeric literal types are default-materialized
at this runtime-storage boundary so a constructor and its later receiver cannot
request different specializations. The elaborator then emits that complete set
of concrete declarations. Before checking their members, the checker
predeclares each generated Tuple symbol together with the fixed arguments
retained in its materialized `element_types` member. Method signatures and
compiler-owned constructors may refer to those predeclared identities, but the
gate does not enable forward references in user source.

This predeclaration is necessary for reciprocal transforms. If both
`Tuple[Int, String].reverse()` and `Tuple[String, Int].reverse()` occur, each
generated method returns and constructs the other specialization, so no linear
declaration order can place both callees first. Reverse-result edges are
therefore forward-safe and are not hard edges in specialization ordering;
dependencies that need a declaration's checked storage layout, such as the
right operand of generated concatenation, remain topologically ordered.

### If

An `if`/`elif`/`else` chain lowers to a diamond or chain of diamonds:

```text
current -> branch
          /      \
       then      else/next-elif
          \      /
           join
```

Branches that already returned, broke, or continued are sealed, so the lowerer
does not add spurious join edges.

### While

A `while` lowers to:

```text
preheader -> header -> body -> header
                    \-> exit
```

`break` targets `exit`; `continue` targets `header`.

### For

A `for` lowers to the same control-flow shape as a `while`, with explicit
iterator protocol instructions. Current typed-raising iterators call
`__next__` exactly once per trip and treat only the checked `StopIteration` type
as normal exhaustion:

```text
bind iterator
initialize iterator through selected __iter__ chain
header:
    try_next(iterator) -> (loop variable, yielded)
    branch yielded, body, exit
body:
    user body
    jump header
exit:
```

The compatibility path for a nonraising iterator instead executes
`has_next(iterator)` through its selected `__len__`, then `next(iterator)` in the
body. For a concrete borrowed List, Set, or Dict place, the preheader carries its
checked interior origin through `BorrowIter`; `for ref` binds the protocol's
yielded reference handles directly — the former List-only indexed-place
desugaring is gone.

Concrete iterator selection retains an exact checked `__next__` operation:
target, raising effect, executable result type, and (when present) the
origin-bearing reference result. HIR and MIR carry that contract unchanged, and
MIR verification checks it against the selected declaration. Thus a
reference-yielding iterator writes a `Ty::Ref` register and binding; the VM is
never relied on to smuggle a reference handle through a register typed as its
referent. Compiler-private iterator carriers alone omit the nominal operation
contract.

Abstract iteration instead retains the value type promised by the trait. Its
checked `Next`/`TryNext` operation carries the same explicit copy-reference
adapter as an ordinary bounded `iterator.__next__()` call. Runtime retargeting
leaves a concrete value return unchanged, or materializes and lifecycle-copies a
concrete reference return before writing the value-typed destination. A
reference into the iterator receiver is read from the identified just-completed
callee frame before that temporary frame storage is discarded.

### Try Regions

`try` is represented structurally rather than fully inlining all exceptional
edges into the surrounding CFG.

HIR can emit a special `HirInstr::Try` that carries the original `try` statement
plus a snapshot of enclosing function-level loop targets. This is needed for
source like:

```mojo
for i in range(10):
    try:
        break
    finally:
        print(i)
```

The `break` targets a loop outside the `try` region. A seeded try-region CFG can
therefore produce:

```rust
Terminator::EscapeJump(target)
```

where `target` is a block in the enclosing function CFG, not the local region CFG.
The VM later propagates this as a non-local jump while running `finally` blocks on
the way out.

## Stage 5: MIR Lowering

Module:

```rust
src/mir/mod.rs
```

Main entry points:

```rust
lower_cfg(cfg: &hir::Cfg) -> MirFunction
lower_program(program: &[Stmt]) -> Result<MirProgram, TypeError>
lower_checked_program(program: &CheckedProgram) -> MirProgram
```

Normal compilation uses `lower_checked_program`. The compatibility
`lower_program` entry point performs semantic checking and propagates any
`TypeError`; it never manufactures unchecked semantic data. VM-backed CTFE
fragments also pass through `check_program`; the former source-type approximation
path has been removed.

MIR is the stable waist of the compiler. HIR still has nested expressions; MIR
flattens them into A-normal form / three-address code.

For example:

```mojo
foo(bar(x + 1))
```

becomes a sequence of register-producing instructions:

```text
r0 = use x
r1 = const 1
r2 = r0 + r1
r3 = call bar(r2)
r4 = call foo(r3)
```

Every intermediate value gets a virtual register:

```rust
pub struct Reg(pub u32);
```

Every variable remains a `VarId` slot:

```rust
pub type VarId = u32;
```

The VM frame has both:

```text
regs: Vec<Value>
vars: Vec<Value>
```

Registers hold temporaries. Variable slots hold source-level locals,
parameters, and synthetic locals such as iterators.

### MIR Program Shape

A lowered program contains:

- one synthetic `__toplevel__` function
- one `MirFunction` per top-level `def`
- one `MirFunction` per lowered struct method
- lifted nested functions where the compiler can safely lift them

Production compilation rejects executable file-scope source statements. The VM
runs the synthetic module-initialization function and then calls zero-argument
`main()` if it exists; the same synthetic function supports explicitly opted-in
legacy statement snippets in phase-level tests.

### MIR Blocks And Terminators

MIR blocks are simple:

```rust
pub struct MirBlock {
    pub instrs: Vec<MirInstr>,
    pub term: MirTerm,
}
```

Terminators are:

```rust
pub enum MirTerm {
    Jump(MirBlockId),
    Branch { cond: Reg, then_b: MirBlockId, else_b: MirBlockId },
    Return(Option<Reg>),
    ReturnWithCleanup { value: Option<Reg>, cleanup: Vec<VarId> },
    FallOff,
    EscapeJump { target: MirBlockId, cleanup: Vec<VarId> },
}
```

Function bodies should not normally end with `FallOff`; that is for try
sub-regions. `EscapeJump` is for a `break`/`continue` inside a try region whose
target belongs to the enclosing function. `ReturnWithCleanup` carries
loop-owned cleanup through structured regions; the VM runs every pending
`finally` before destroying those owners and completing the return.

The continuation-driven VM frame path binds reference parameters directly to
caller frame/slot handles. Structured `try` sub-regions execute calls
synchronously, so the VM temporarily pushes a mirror containing the retained
caller's real `FrameId`, registers, and variables. Direct, indirect, method,
callable-struct, and constructor calls can then use the ordinary handle ABI:
mutations update the mirror even when the child raises, and projected or
aggregate reference returns already name caller storage. The mirror is copied
back and removed on every outcome; there is no separate result-rebasing rule.

### Places

MIR separates rvalues from writable places.

```rust
pub struct MirPlace {
    pub root: VarId,
    pub root_ty: Option<Ty>,
    pub proj: Vec<Proj>,
    pub projection_tys: Vec<Ty>,
    pub ty: Option<Ty>,
    pub through: Option<VarId>,
}

pub enum Proj {
    Field(String),
    Index(Reg),
    ConstIndex(usize),
    Variant(usize),
}
```

A place is something that can be read, written, moved from, or borrowed:

```text
x
p.field
p.items[i].x
xs[i]
```

This is one of the key architecture choices. Mojo-like ownership and borrowing
need to know the difference between "the value computed by an expression" and
"the storage location this expression names." MIR makes that difference explicit.

Production checked lowering requires `root_ty`, one result type per projection,
and the final stored `ty`; the optional wrapper exists only for the deliberately
unchecked phase-test API. HIR carries the same information earlier as
`HirPlace`: stable `OwnerId`, root type, typed field/index projections, and
final storage type.

Dynamic `Index(Reg)` projections conservatively overlap every index. A
`ConstIndex` is emitted only for an exact nonnegative literal selecting an
element of compiler-private heterogeneous `Ty::Tuple` storage. This gives each
pack element a distinct ownership path without changing nominal collection
subscript dispatch.

When an accessor returns a reference used immediately as another receiver or
place, lowering evaluates it once into a hidden `Ty::Ref` local. `DefVar` stores
the handle and `EstablishLoans` retains its owner generation; the derived place
names that local in `through`. Later chained loads, projections, and calls
therefore preserve provenance without rerunning the accessor or treating its
referent as owner storage.

Every register is typed. Expression results record their checked type as they
lower; synthetic registers (handles, markers, short-circuit and iterator
temporaries) are typed at their emission site; and a `close_register_types`
pass fills the remaining results by copying facts already present in the
instruction stream — operand register types, place storage types, inline
element types, slot types, and declaration returns — never by re-implementing
checker inference. Loan and consumption markers, which hold no runtime value,
are typed `Ty::None` by convention. Functions additionally carry their checked
`ret_ty`, raising contract (`raises`/`error_ty`), and per-slot `var_tys`.

`mir::verify` is the standalone semantic verifier of record. From MIR plus
`MirDeclarations` alone it checks place completeness and projection
consistency, register bounds and register-type completeness, store/binding/
return/call-argument type consistency (through the checker's coercion
predicate — never re-derived rules), CFG-edge validity (jump-target bounds per
region, `FallOff`/`EscapeJump` only inside `try` sub-regions), effect
protection (a raising site in a nonraising function must sit under a handler),
and reference invariants (`StoreRef` targets reference storage; declared
write-back parameters receive caller places). For method-dispatched nominal subscripts it also
verifies the selected target against its declaration, exact positional/keyword/
default and variadic source binding, operand and collector types, generic value
arguments, receiver and argument place requirements, capture slots,
checker-selected executable result (including exact reference origin and
permission), and protection of every raising subscript form. Function
declarations retain whether a receiver exists, its declared convention, aligned
explicit-parameter conventions, positional- and keyword-variadic collector
conventions, and the reference-return ABI. The verifier permits only declared
`Ref` to effective `Read` narrowing. An abstract trait-bound target has no
concrete body declaration until runtime retargeting; its complete selected
requirement is therefore retained as typed call-local MIR metadata (including
argument conventions/place requirements, result ABI, effects, generic
arguments, and any result adapter) and verified as the abstract declaration of
record. Concrete targets are additionally checked against `MirDeclarations`. The
pipeline composes it with
`analysis::check_ownership_program`, which owns the ownership dataflow; the
compiler rejects findings as `CompilerError::Verify`, and the VM re-verifies
the drop-elaborated program it actually executes.

A place's storage type is distinct from its expression value type. For a field
declared `ref[origin] T`, the place stores `Ty::Ref`, while an ordinary load
produces `T`. The VM now chooses reference read/write behavior from this checked
place type; it does not inspect a runtime `Value::Ref` to rediscover semantics.

### Important MIR Instructions

Representative instructions:

```rust
Const
UseVar
MovePlace
DefVar
UnOp
BinOp
Call
MethodCall
GetField
Index
Slice
MultiIndex
MultiSet
Store
LoadPlace
MakeTuple
MakeVariant
VariantIs
VariantGet
VariantSet
VariantSetInitWith
VariantTake
VariantDeinitWith
VariantReplace
MakeSimd
Raise
Try
DropVar
HasNext
Next
Unsupported
```

Public collection displays and comprehensions use ordinary `Call` and
`MethodCall` instructions. `MakeTuple` is reserved for the compiler-private
heterogeneous pack behind `__RuntimeTuple`; constructing the public nominal
`Tuple` uses an ordinary constructor call.

Every method-dispatched nominal subscript carries a `MirSubscriptCall`
containing the exact target,
typed raising effect and executable result type, receiver place requirement and
effective convention,
source-bound argument types/conventions/places, capture accesses,
reference-result origin, and source-ordered compile-time value arguments plus
their declarations. `Index` carries exactly one nominal call or a checked
`MirIntrinsicSubscript` discriminator for Tuple/runtime-pack storage, variadic
storage, SIMD, pointers, or the CTFE-only compile-time-list bridge. `Slice`
similarly carries exactly one nominal call or the temporary VM String intrinsic.
`MultiIndex` and `MultiSet` always require a nominal call contract. Checked
assignment, including the single-index spelling
`value[index] = replacement`, retains whether the right-hand side binds as the
last positional argument or the keyword-only `value` after a variadic index
pack. A mutable receiver place may be rooted in a `mut`/`ref` parameter, so the
VM commits setter write-back through the same caller handle instead of
flattening the access into raw backing storage.

One narrow representation bridge is intentionally call-less despite its
nominal checked result: `Slice.indices()` currently produces transient private
`Value::Tuple` storage typed as the public result Tuple. Its subsequent indexing
is explicitly tagged `MirIntrinsicSubscript::TupleStorage`; this exception is
not permission to infer dispatch from an arbitrary nominal runtime value.

A projection below a nominal reference-returning accessor is rooted in a hidden
caller handle. Lowering executes the selected accessor once, materializes its
typed `ref` result and loans, then appends ordinary field/private-storage
projections to that handle. It must not recursively turn
`container[index].field` into a raw `MirPlace` rooted at the nominal container;
the extended handle is also the caller place passed to a later `mut` or `ref`
parameter or forwarded from a reference-returning function. Pointer, SIMD, and
private Tuple steps below that field retain typed dynamic/constant projections.
Verification distinguishes a place that forwards the `ref T` handle slot from
one that addresses its `T` referent, validates the `through` slot and every
concrete projected element type, and treats nominal `owner[index]` paths in
`EstablishLoans` as analytical origins rather than executable VM navigation.
Register-type closure makes the same root-sensitive distinction: borrowing an
ordinary field whose stored value is `ref T` produces an outer `ref (ref T)`
handle, while a projection through an existing `ref` or origin-bearing Pointer
forwards that capability and can never recover stronger mutability.
Assignment through a runtime alias likewise preserves the local slot's checked
`ref` type: the right-hand side types the referent write, never the handle
storage. This matters for union-origin free-function results, whose one handle
may designate any of several caller places.

Reference-valued List elements add one deliberate handle layer: indexing the
List first produces a handle to the element slot, whose stored value is itself a
reference handle. Lowering peels that outer slot handle before an augmented
write or chained method receiver is formed. The operation therefore reaches the
ultimate referent and never replaces the reference stored in the List element.
Relatedly, a chained subscript's loaded base register may legitimately stay
reference-typed one level above its place — the VM's `LoadPlace` second
dereference resolves it at runtime — so the verifier peels exactly one
`Ty::Ref` level on both the storage and the loaded register when checking a
subscript receiver, symmetric by design. No consumer needs the register
retyped (analysis and drops read place types, not the loaded base register),
so this one-level tolerance is the sanctioned contract rather than a gap.

Augmented nominal subscripts cross the checked boundary with call-local
adaptation and invalidation snapshots. For a value result this includes both
complete `CheckedCallContract`s plus the computed-result setter slot. MIR
evaluates the receiver and raw subscript operands once, evaluates the RHS,
applies getter-specific conversions and calls the getter, performs the operator,
then independently reloads any getter-mutated caller place and applies the
setter conversions before `MultiSet`. A mutable-reference getter instead is the
complete operation: MIR invokes it before the RHS, reads and updates its handle,
and emits `WriteRef` without selecting or calling a setter. The getter uses
`Index`/`Slice`/`MultiIndex` in either path, so neither source expressions nor
overloads are reconstructed.

Checked membership on a nominal struct likewise lowers to the selected
`__contains__` `MethodCall`, with a retained container place and the source value
as its argument. It does not use the value-only `BinOp` form: doing so would lose
the borrow and let a short-lived shallow receiver appear to own pointer-backed
fields. `not in` negates that checked call's Boolean result.

Intrinsic `print`, `String`, and `repr` formatting retain a nominal argument's
place, when one exists, through its `Writable` call. This is a liveness fact,
not a runtime ownership guess: drop elaboration cannot release the original
pointer-backed owner after loading the value but before formatting finishes.

`UseVar` is tagged with a `UseMode`:

```rust
pub enum UseMode {
    Copy,
    Move,
    BorrowShared,
    BorrowMut,
}
```

This lets later analysis distinguish ordinary reads, ownership transfers, and
borrows without reparsing expressions.

### Partial Moves

Whole-variable moves use:

```rust
UseVar { mode: UseMode::Move, ... }
```

Field moves and constant-index moves from compiler-private Tuple storage use:

```rust
MovePlace { place, ... }
```

This allows the ownership analysis to understand:

```mojo
var x = p.a^
print(p.b)
```

as valid when `a` and `b` are distinct fields, while rejecting a later read of
`p.a` or a whole-value move of `p` before `p.a` is reinitialized.
The same rule lets generated heterogeneous-pack code move `storage[0]` and
`storage[1]` independently; runtime-indexed places remain conservatively
overlapping.

### Calls

MIR calls keep the information the VM needs for Mojo-style conventions:

- positional argument registers
- keyword argument registers
- simple caller places aligned with both positional and keyword `mut`/`ref`
  arguments
- an optional callable-value place for a nominal `mut`/`ref __call__` receiver
- source-ordered compile-time parameter arguments, retaining an optional name
  separately from the optional value register (type arguments have no register)
- the resolved lowered callee name when the checker selected an overload
- the checker-selected concrete error type when the call may raise

Non-capturing functions also have a runtime `Value::Function` representation.
Calls through a function-typed local or a general callable expression lower to
`CallIndirect`, whose callee register is resolved to a MIR function symbol by the
VM before it pushes the ordinary explicit frame. It retains positional and
keyword argument places and, for nominal callable values, the callee place plus
the exact checker-selected `__call__` symbol. A `def(...)`-typed parameter
retains the same signature as an abstract dispatch symbol; if its runtime value
is a nominal callable, the VM retargets that suffix to the concrete struct.
Consequently same-arity `__call__` overloads do not fall back to runtime value
tags or source-name/arity selection. The checker has already matched the
arguments against the callable signature, so indirect execution shares the same
function/reference ABI without adding dynamic overload ranking.

For a generic indirect call, `CallIndirect` also carries the checked anonymous
callable's `ParamDecl` list. The VM first binds source-ordered positional and
named bracket arguments to that contract, then evaluates omitted scalar and
symbolic callable defaults in declaration order. Only after the contract has
produced concrete value slots are they renamed positionally to the
alpha-equivalent implementation binders. Contract defaults therefore govern
partial and named calls even when the runtime function value declares different
defaults. Origin/OriginSet arguments remain semantic-only and never enter this
runtime vector. When checking has concretely instantiated a generic callable,
MIR additionally retains its complete declaration-order `TyArg` list (including
defaults) and the resulting monomorphic contract. This pair is semantic-only:
the verifier reconstructs type/value substitution from it rather than searching
earlier `Const` instructions. A dependent indexed type must thereby resolve to
one concrete operand/result type before executable argument verification. A
call which remains inside an unspecialized generic body may retain a symbolic
dependent type only when every referenced compile-time name is owned by an
explicit enclosing value binder. The MIR verifier also checks the retained
declaration list against the callee register type, validates every value
register, and rejects malformed raising and statically decidable
reference-result metadata.

`Ty::Func` and `Ty::GenericFunc` retain both the `raises` effect and its optional
error type. A source function type also retains a `var **kwargs: T` parameter as
the checked `kw_variadic` collector rather than flattening it into the ordinary
parameter vector, so indirect calls preserve structural keyword binding.
`SignatureKey` likewise stores a distinct keyword-variadic type key; ordinary,
positional-variadic, and keyword-variadic slots with the same element type
therefore cannot collide in lowered callable identity. Direct
overload resolution carries the selected candidate's effect
alongside its lowered symbol, and an indirect call reads the effect from its
callable type. Generic substitution includes error types, with a nonraising
callable inferring `Never`. `Never` is the bottom type, and `raises Never` is
treated as nonraising. A protected `try` records the errors its body can raise
and gives the `except` binding that type. Effect checking therefore happens
after candidate selection instead of conservatively attaching one effect to
every declaration sharing a source name, and before lowering. The same path is
used for trait requirements and bounded method calls. `Call`, `CallIndirect`,
`MethodCall`, `Index`, `Slice`, `MultiIndex`, and `MultiSet` retain the selected
optional error type for MIR verification; the VM does not rediscover an effect
from a source declaration.

When a `Ty::GenericFunc` appears where a concrete `Ty::Func` is expected, the
checker solves its type parameters from the expected parameter and result types
and validates the resulting monomorphic signature. When both sides are generic,
it alpha-normalizes their binder names, preserves arity and bounds, and compares
the resulting callable shapes directionally. Binder defaults and the infer-only
marker govern calls through the contract but are not part of current Mojo's
generic-callable conformance identity.

There are two source forms that otherwise look similar in a declaration's
compile-time parameter list:

- `F: def(T) -> T` declares a type parameter with a checked anonymous callable
  constraint. Its `ParamDecl::Type { callable_bound, ... }` retains the complete
  dependent `Ty::Func` or `Ty::GenericFunc`, so a call through `F` inside the
  template is checked against that signature. A monomorphic contract requires
  one monomorphic function type or a struct with nominal `def(...)` conformance;
  a contract with its own `def[...]` binders accepts an alpha-equivalent generic
  function. A shape-compatible `__call__` and an unresolved overload set do not
  satisfy either form. A nonraising/read-only implementation may satisfy a
  raising/mutable contract because it demands no more capability, while binder
  bounds, ownership-changing conventions, and reference origins remain exact.
- `callback: def(...) thin` and `callback: def(...) capturing[...]` declare
  callable **value** parameters. `Origin` and `OriginSet` binders are
  semantic-only, and `//` separates infer-only binders from explicitly supplied
  ones. The bracket argument is evaluated to a callable register, then reified
  under the parameter name as a hidden typed MIR local; it is not counted among
  the function's ordinary source-call ABI parameters. Calls through that local
  use `CallIndirect` like any other callable value. The callable type may have
  its own nested `def[...]` binder scope; ordinary runtime parameters cannot use
  that parametric type directly and must use one of these compile-time forms.

An omitted callable-value argument is represented by `CallableDefault`, not by
`CtValue`. `Symbol` retains the checker-selected function specialization,
`Parameter` aliases an earlier reified callable, and `If` selects recursively
between those plans using a dependent compile-time condition. The VM evaluates
the plan in declaration order after preceding scalar and callable parameters
have been reified. The generic identity carries only a symbolic occupied-slot
marker, so neither a static function nor a captured environment is serialized
as a compile-time closure payload.

An explicit `function[origin_of(place)]` expression is the current-Mojo spelling
for materializing an origin-generic function as a value without erasing its loan
contract. The checker binds the function's semantic-only Origin parameters to
the caller's checked origins and produces a concrete `Ty::Func`; its parameter
reference signatures and reference-result signature are then ordinary indirect-
call facts. The runtime value still needs only the resolved MIR symbol because
the origin substitution has already constrained legal calls and escapes. A
non-overloaded stateless nested function combines the same substitution with its
resolved lifted symbol, so top-level and lexical function values share indirect
lowering. Origin arguments are split from ordinary compile-time arguments before
candidate checking. They therefore participate in direct overload selection,
compose with inferred generic arguments on direct calls, and compose with
explicit ordinary generic arguments on function values. Specialization binds
the complete source parameter layout: a variadic type/value pack consumes only
the positional segment available before required suffix parameters, while a
named Origin may follow the pack and remains on the rewritten call. Origins,
OriginSets, and callable-value parameters stay symbolic on generated
specializations instead of shifting the evaluated type/value arguments. An
overloaded function value requires an expected `def(...)` type; the checker
specializes every candidate, retains the unique compatible lowered symbol, and rejects an
uncontextualized or still-ambiguous set. Mojito can also combine a captured
nested environment with an explicit-Origin contract; the pinned nightly rejects
materializing that form, so it is classified as an acceptance divergence.

Overloaded function values remain overload sets until an expected `def(...)`
type selects one candidate; the selected lowered symbol and its raising effect
are retained in checked data. A struct is callable only when its declaration
nominally lists a `def(...)` conformance and its `__call__` method matches that
contract. Function-type generic parameters, `mut`/`ref` conventions, reference
origins/results, and raising effects are retained while checking that contract.
This applies both to parameters declared inside `def[...]` and to a dependent
outer `F: def(...)` bound. Indirect VM calls dispatch such values to the checked
method—including a signature-qualified same-arity overload—and carry the
receiver place when `__call__` needs writable/reference access. Typed-MIR
verification checks an abstract target against the callable type, then checks a
concrete nominal target against its declaration and argument types whenever the
receiver is statically known.

The environment is semantic callable-type data. `CallableEnvironment`
distinguishes an unqualified contract, an explicitly `thin` function, and a
`capturing` contract. A capture set is one of `Infer` (`capturing[_]`), a stable
declaration-order `OriginSet` parameter, or a canonical concrete list of
`CaptureOrigin { origin, access }` entries. The latter records read versus write
access, flattens origin unions, removes duplicates, and lets a write subsume a
read of the same origin. An `OriginSet` is deliberately not a reference-origin
union: it describes storage retained and accessed by a callable environment,
not storage a returned reference may designate. Substitution and nominal
callable conformance preserve the environment contract, and origin escape
checking recursively visits concrete environment dependencies.

Escape enforcement is a checker origin rule at *both* frame boundaries. The
return boundary checks a returned value's aggregate origins and a `ref[o]`
contract's actual origin; the store boundary applies the same predicate at
place writes whose destination root outlives the frame (a parameter or
`self` owner from the per-body escape context, including variadic collector
parameters): a store fires only when the stored value carries loans — or the
destination is `ref`-typed storage, whose rebound handle becomes the loan —
and some origin is rooted at frame-local storage. Parameter-rooted loans
store outward freely, so origin-parameter-bound flows such as
`self.field = make_iter(self.data)` stay accepted. Ownership analysis remains
the lazy within-frame layer (interior origins are invalidation generations,
not exclusive loans; eager whole-loan exclusivity is not promised). Mapping
key yields are declaration-level immutable via the `ref` signature's
immutable-origin cast (`Origin[mut=False].cast_from[...]`), which
`lower_ref_sig` unwraps and pins to `SigMutability::Immutable` so the loop
site's parametric-mutability upgrade never applies; the upgrade direction is
rejected.

Nested functions are lifted with explicit closure environments. Capture lists
appear directly as `{...}` after effects; the removed `unified {...}` spelling
is rejected with a contextual parse error. `imm`, `mut`,
and `ref` captures become stable frame/slot handles with their checked
permissions, `var` clones the value when the declaration executes, and move
capture transfers the value at that same point. Direct nested calls use the same
materialized closure and `CallIndirect` path as closure values rather than
rebuilding an environment at each call.

The stored reference handle is not itself a persistent loan spanning declaration
to invocation. The enclosing code may access or update its owner between those
events, and an `imm` capture observes that current value. Loan conflicts apply
when the closure is used under its checked capture convention; owned copy/move
captures instead operate on their independent environment slot.

The checker resolves every explicit, defaulted, and intermediate forwarding
capture to a stable `OwnerId`, exact storage `Ty`, and capture convention. This
includes explicit captures unused by the body: copy/move still happens when the
declaration executes. Checked call, identifier, type-application, and declaration
occurrence identities then select a nested registry keyed by `OwnerId`; same-name
declarations in disjoint blocks cannot alias. Lifted capture parameters and MIR
declarations use the retained storage type rather than an opaque environment
placeholder.

Each checked capture also carries the origins retained by its stored value and,
for `imm`/`ref`/`mut`, the read or write access to its source place. Finalizing a
nested declaration folds those facts into its concrete callable environment.
At every direct, indirect, method, constructor, or nominal subscript call, the
checker collects the concrete environment accesses of both the callee and
non-escaping callable arguments into
`SemanticAdjustment::CallableCaptureAccesses`. Checked HIR keeps that
adjustment; MIR maps stable owners to local places and stores the result on the
call instruction as `capture_accesses`. Verification checks the typed call,
and persistent-loan analysis treats each entry as an access only for the call's
duration. This is why a mutable closure invocation conflicts with a live alias
of its captured owner even across a downward-funarg boundary, while ordinary
owner mutation between closure declaration and invocation remains valid.

Capture discovery builds a lexical tree and lifts descendants recursively at
arbitrary depth. Each lifted symbol encodes its complete lexical path; the
nearest declaration wins under shadowing, and each intermediate environment
forwards only captures permitted by its own explicit/default policy. Capturing a
sibling captures the already materialized sibling closure slot, not a rebuilt or
flattened copy of its transitive environment, so declaration-time snapshots and
moved state remain stable. `KeepAlive` retains exact existing callable/reference
slots and never fabricates a slot for a transitive capture. Lifted
specializations retain their exact regular/default/variadic/keyword/marker/effect
ABI; reference returns and a named `out` result remain part of that checked ABI.
The checker rejects undeclared captures and every path that would let an
environment outlive its defining function.

Compile-time specialization distinguishes evaluated parameters from retained
runtime/semantic parameters. Scalar values, ordinary types, and packs may drive
`comptime if` or `comptime for` and are baked into the generated body. Origin,
OriginSet, and explicit callable-value parameters remain on its signature, and
their source arguments remain on the rewritten call. A scalar-controlled branch
can therefore select code that later invokes a captured callback without asking
the compile-time universe to own that callback. `CtValue` intentionally has no
function or closure variant: this residualization is not arbitrary callable
CTFE, and it does not make closures escaping. A capturing closure binds only
to a contract that states `capturing[...]`: unqualified `def(...)` value
positions reject it (matching current Mojo), while comptime callable bounds
still ground capturing values.

Runtime `for`, tuple-unpack, and `except` targets likewise retain checked owner
identity and storage type across HIR and structured-region lowering. Unpack uses
each target expression's occurrence/owner facts; loop and handler declarations
seed exact owner slots. A handler may therefore shadow an outer same-name value,
host a nested closure over its `Error`, and restore the differently typed outer
binding afterward without name-based slot reuse. Future pattern and coroutine
binders can extend this declaration/binder boundary instead of forging source
locations.

For method calls, MIR also records whether the receiver was a writable place.
That lets a `mut`/`ref self` method bind directly to caller storage, including
when the call executes synchronously inside a structured region.
For overloaded method calls, MIR also carries the resolved function name, such as
`Counter.bump$ov$Int`, so the VM does not have to reconstruct type-directed
dispatch dynamically.

### Overloaded Names In MIR

Overloaded definitions lower to stable signature-based names. The source name is
kept for non-overloaded declarations, but an overload set uses names of the form:

```text
function$ov$ParamType$OtherParamType
Struct.method$ov$ParamType
Struct.__init__$ov$ParamType
```

For example:

```mojo
def choose(x: Int) -> Int: ...
def choose(x: String) -> String: ...
```

lowers to functions named roughly:

```text
choose$ov$Int
choose$ov$String
```

This is intentionally a lowered compiler name, not source syntax. It gives MIR
and the VM a stable identity for each candidate, including same-arity overloads.
It also keeps arity overloads and type overloads on one mechanism instead of
special-casing `name#arity`.

Signature identity and this name scheme are owned by one canonical module,
`src/symbol.rs`: a signature is typed data (`SignatureKey`, built from either
the declared `ast::SourceType` or the checker-resolved `Ty` — both spell a type
from its annotation, e.g. `Point`, `Pair$Int`) and only the module formats the
final symbol. Checker, MIR, and VM all route through it, so the recorded
callee always names the emitted function; `tests/symbol_test.rs` pins the
spellings and scans `src/` for stray hand-built `$ov$` strings.

The signature stores a keyword-variadic collector separately from positional
types and emits a `$kwv$Type` component when present. Thus
`def route(value: Int)` and `def route(var **options: Int)` have distinct ABI
identities even though their sole declared element type is `Int`; definition-
and resolution-side symbol construction use the same field.

Across phase boundaries, Mojito names source syntax and checked semantics
explicitly. `SourceType` is parser-owned syntax; `Ty`, `binding_ty`, and MIR
`param_types` are checker-resolved facts. `AnnotationSite` keys declaration
annotations at the checked boundary, after which HIR, MIR, and the VM carry only
the resolved `Ty`. In particular, variable initialization and function-entry
coercion no longer reinterpret source annotations in the backend. Missing checked
facts are compiler invariant diagnostics carried by the lowering/backend
result—not permission for compatibility APIs to guess from unchecked input.

### Try In MIR

`MirInstr::Try` contains mini-CFGs:

```rust
Try {
    body: Vec<MirBlock>,
    handler: Option<(Option<VarId>, Vec<MirBlock>)>,
    orelse: Option<Vec<MirBlock>>,
    finalbody: Option<Vec<MirBlock>>,
    cleanup: Vec<VarId>,
}
```

Those mini-CFGs share the enclosing function's register and variable spaces. They
have local block numbers, but their instructions address the same `regs` and
`vars` vectors as the outer function frame.

The structure mirrors source-level exception semantics:

- body runs first
- `except` handles a raised error
- `else` runs only when the body completes normally
- `finally` runs on every path
- `return`, `break`, and `continue` crossing the region are represented as
  non-normal flows so `finally` can run before control leaves

### Spans

MIR records source spans for generated registers:

```rust
pub struct SpanTable(pub HashMap<u32, (SourceSpan, Option<VarId>)>);
```

This is what lets ownership diagnostics point back to the original source even
though expressions have been flattened into temporaries.

## Stage 6: Ownership Analysis

Module:

```rust
src/analysis/mod.rs
```

Production entry point:

```rust
check_ownership_checked(program: &CheckedProgram) -> Result<(), OwnershipError>
```

`check_ownership(&[Stmt])` is a compatibility wrapper for unchecked callers; it
rechecks through compatibility lowering. Production compilation lowers the
existing `CheckedProgram` and runs move/init and persistent-loan analysis on each
function.

The core state is:

```text
Owned
Moved
MaybeMoved
```

Analysis is forward over the MIR CFG.

Rules:

- defining a variable makes it `Owned`
- moving a variable makes it `Moved`
- using a `Moved` variable is a use-after-move error
- merging `Owned` and `Moved` at a join produces `MaybeMoved`
- using a `MaybeMoved` variable is a conditional-move error
- moving a field marks that field moved but leaves sibling fields usable
- reassigning a moved variable or moved field reinitializes it

This is why control-flow lowering happens before ownership analysis. A move
inside an `if` or loop only has the right meaning once joins and back-edges are
explicit.

### Persistent local loans

Local `ref name = place` bindings are checked references, not copied referent
values. MIR emits one grouped `EstablishLoans` operation for each fresh binding
generation; every `MirLoan` retains the executable owner `MirPlace`, permission,
and optional canonical `MirInteriorOrigin`. Statically resolvable aliases use
the frozen place, while cross-call aliases use explicit reference operations.
`MirPlace::through` records which reference authorized an access. The VM erases
both loan and interior-generation metadata after checking.

Reference variables participate in backward CFG liveness. A loan is active from
its binding through its last use, including joins and loop back-edges. While it
is active, direct or differently-authorized overlapping mutation, replacement,
move, drop, or mutating call is rejected. Projection overlap is field-sensitive
and index-conservative. Drop elaboration combines that liveness with the
forward reaching `EstablishLoans` generation: every possible owner of a live
union/reference value remains alive, but rebinding a reference-bearing aggregate
retires only its old generation instead of permanently retaining every owner
ever stored in that variable slot.

Reference handles can temporarily leave a variable slot while one MIR
expression is evaluated. Drop elaboration therefore propagates owner provenance
through the SSA registers of that expression (`MakeRef` through `ReadRef` and
the consuming call/operator). The owner dies after the final consuming
instruction, while `DefVar` is an explicit handoff to a newly established
variable generation. This preserves ASAP destruction without dropping storage
between creation and use of a transient handle.

Cross-call aliases lower to explicit `MakeRef`, `ReadRef`, and `WriteRef`
operations. Runtime handles contain a monotonic frame identity, variable slot,
and captured field/index projection. Returning a reference forwards the caller's
handle, so a union return preserves whichever argument was selected dynamically.
An ordinary value context requires a `Copyable` referent and follows `ReadRef`
with typed `CopyValue`, invoking the referent's copy lifecycle instead of
aliasing pointer-backed storage. An explicit `ref` binding retains the handle,
omits that copy, and therefore remains legal for a linear referent.
The checker uses the same explicit `CopyValue` boundary for a projected place in
a consuming context such as a new binding, assignment, or return. This fact is
recorded while conditional `Copyable` constraints are in scope; plain
`LoadPlace` remains handle-preserving for borrowed method receivers, formatting,
and iteration.
The checker marks only actual arguments selected for `mut`/`ref` parameters as
caller-place dependencies; MIR retains those places through the call while an
ordinary copied place argument remains eligible for ASAP destruction after its
value is evaluated.

A bare `ref` parameter or receiver has parametric mutability. Its checked body
may read and return/reborrow it, and the call substitutes the actual caller
capability into a reference result, but the body cannot assume write access.
Writing requires an explicit `ref[origin]` whose `Origin` is mutable (or the
ordinary `mut` convention). Call solving rejects any attempt to pass an
immutable reference to that mutable contract, so capability cannot be escalated.

### Cross-call transfer effects

A callee that stores a loan-carrying value into `self` or a `mut`/`ref`
parameter changes the caller's loan picture, so the checker owns that fact and
replays it at every call. When the store-outward escape rule accepts such a
store, the body records a `TransferEffect { dest, src, src_is_place, mutable }`
in signature-origin terms (`Self_`/`Param(k)`) on the current callable's frame;
the bundled `List.append`/`insert`/`__setitem__` are seeded directly because
their pointer-mediated stores never reach that acceptance point. Effects live in
a name-keyed side map (`"name"` / `"Struct.method"`), and visibility is
declaration-order independent: `check_program` reruns the whole check — a
fresh checker seeded with the prior round's committed map merged over the
bundled seeds — until no call site has observed a stale callee entry.
Staleness is exact rather than structural: `apply_transfer_effects` records
the first-seen effects per queried callee (including "none"), and a round
converges when every observation matches the final committed map, so a
program whose effects were all committed before any call site consulted them
— every stdlib-only compile included — finishes in one round. Effects grow
monotonically over a finite per-callable lattice; a four-round cap surfaces
`TransferEffectDivergence`, which indicates a checker defect, not a user
error.

The store-outward acceptance point itself is shared: the SetPlace guard
(escape check plus transfer recording) is the `check_outward_store` helper,
which unpack-into-place targets also run (per tuple-display element, or
conservatively with the whole right-hand side's origins). Outward storage
covers both the frame's outliving owners (`self`, `mut`/`ref` parameters and
the capture-reachable extensions of the escape context) and ANY
enclosing-frame binding a nested def reaches through captures — storing a
closure-locally rooted loan into a captured enclosing local dangles just as
surely once the closure returns. A store through a captured owner records a
concrete `SigOrigin::Bound` destination (owner ids are checker-global):
invocation sites ground it directly, intermediate frames propagate it
verbatim, and the frame whose signature covers the owner re-abstracts it, so
a method whose closure stores into captured `self` carries the effect to the
method's own callers. Augmented assignment needs no dedicated guard: the
in-place dunder rides ordinary method selection, so its callee effects
replay at the `+=` site.

Effects also ride checked function types. A `def` name in value position
bakes its committed effects into the produced `Ty::Func`/`Ty::GenericFunc`
as an identity-transparent `TransferSet` (never part of type equality or
acceptance — a `def(...)` contract cannot spell effects, so soundness comes
from call-site replay, not acceptance filtering), with a fixpoint
observation so a later-grown entry re-bakes. Indirect calls replay from the
value's type; a callable-struct call replays its `Struct.__call__` entry
with the callee binding as the receiver; overloaded call sites replay the
shared bare-name entry; and a trait-method call on a bounded receiver —
which has no concrete body — replays the union of effects over every
conforming implementation of the method, one observation per conformer key.
The one genuinely higher-order shape is a body calling through its own
callable parameter (a runtime `def(...)` param or a compile-time callable
value param, which specialization retains symbolically): the body records a
`CallThroughEffect` carrying the signature abstraction of every actual, and
each call site — which knows the concrete callable — translates that
callable's effects through the recorded mapping into effects of the callee
and replays them, rejecting a frame-local source flowing into a signature
destination; when the supplied callable is itself a callable parameter of
the calling frame, a composed residue is derived instead, so two-level
forwarding chains resolve at the outermost concrete call. Call-through
visibility shares the two-phase pass through its own seed and observation
channel.

Each call site with a matching effect substitutes the source actual's caller
origins (its carried aggregate/reference origins, plus — only when
`src_is_place`, i.e. the callee parameter is borrowed rather than owned — the
actual's own place), enforces the store-outward escape rule across the boundary,
merges the result into the destination actual's aggregate-origin bookkeeping so
the checker's own return-escape analysis sees callee-installed loans, derives a
transitive effect onto the enclosing callable when the destination roots at its
parameter or receiver, and records a span-keyed
`CheckedCallTransfer { dest, dest_path, sources, mutable }` on
`CheckedProgram` for MIR. Destinations are interior-precise: the store's
path below the destination root survives on the effect
(`SigOrigin::Projected`), composes with the actual's own projection at each
call site, and lowers as the generation's destination domain —
`EstablishLoans { reference, loans, marker, dest_interior }`, where `None`
means the whole root. A root-domain generation replaces every prior
generation; an interior-domain generation replaces only overlapping interior
domains, so sibling fields keep independent generations, and a `Store`
through a concrete field prefix releases the domains it covers — rebinding
`t.a` frees `t.a`-rooted transferred loans while `t.b`'s and the root's
survive. Repeated transfers into one domain still merge (union, never
replacement), so a second `append` extends that generation. Lowering skips
destinations rooted at the current function's own parameters, which the
derived effect covers at the caller where the storage lives; a `Bound`
destination resolves through the owner-variable map only in the frame that
owns the storage. Ownership and drop analysis then reject mutating or
dropping the loan root while the stored alias lives and keep borrowed
sources alive under carrier collections with no transfer-specific analysis
code. A closure value flowing into storage additionally loans its REFERENCE
captures' owners (`imm` immutably, `mut`/`ref` mutably) — the stored
environment retains their frame slots — while direct nested calls keep the
loan-free declaration-to-call capture model. The `FieldInvocation`
adjustment (an indirect call whose callee place is the storage's, so a
closure environment rehydrates from stable storage) is retained internal
machinery: `def(...)`-typed fields and elements are rejected at declaration
to match current Mojo, so no production path reaches it from a field today,
and it stays only as the invocation shape for supported callable storage
channels.

The deliberate residues, frozen with the schema: effects erase when an
effectful callable value is stored into explicitly annotated `def(...)`
storage (a plain function value carries no loans of its own, so this stays
permissive rather than unsound); a call through a bare `callable_bound` with
no value provenance carries nothing; a call-through destination that is
frame-local to the higher-order body is invisible to that body's own
return-escape analysis; transfer via a returned `self` belongs to the
return-origin path; and source loans stay root-abstracted — only
destinations carry interior paths.

### Nominal String and the literal bridges

The self-hosted `String` (stdlib/std/string.mojo) is an ordinary struct — a
UTF-8 byte buffer over `UnsafePointer[Byte]` in the List storage pattern —
with exactly two deliberate intrinsic crossings, both keyed on the linked
declaration identity (`symbol::is_stdlib_string_struct`): the `@implicit`
literal constructor (`String("...")`) never executes its declared body — the
VM fills the byte buffer from the literal's UTF-8 bytes — and
`_as_string_literal(self) -> StringLiteral` reads the buffer back into a
builtin string value for the Writer path. Everything else (comparison,
concatenation, membership, hashing, decoding, slicing) is pure library code.

The `StringLiteral`-vs-`String` split is realized at the type level. The
compile-time literal type is `Ty::StringLiteral` (spelled `StringLiteral` in
source, mirroring `Ty::IntLiteral`); string literals and literal-only
operations stay on it, as do comptime strings, `[text: String]` value
parameters, kwargs keys, and Writer payloads inside the stdlib. A source
`String` annotation is not a builtin type keyword: it parses as an ordinary
name, the prelude rewrite qualifies it, and ordinary struct lookup resolves
the nominal struct — no dedicated checker path. A literal converts wherever
the nominal String is expected through the ordinary implicit-conversion
engine (the `@implicit` literal constructor), including operator operands
(mixed literal/nominal comparisons and concatenation normalize onto the
struct's dunders), tuple-display and fieldwise-construction elements, and
specialized-pack constructor arguments. Builtin string producers retarget:
`String(x)` stringify, `input()`, `repr(x)`, and `.format(...)` type as the
nominal String by routing the underlying call to the VM's conversion builtin
(a `ResolveCallable` adjustment where needed) and wrapping the buffered
result through the literal constructor. `Error(msg)`/`raise` and the Writer
`write_string` contract accept either spelling; the VM bridges read a
nominal message back and materialize a nominal payload for a
nominally-declared `write_string`. Overload symbols keep the stable
`String` spelling for both types, so an overload set differing only in
StringLiteral-vs-String collides and is rejected at declaration. In unlinked
seam programs (no prelude) a bare `String` annotation fails explicitly as an
unknown type, and the bare `String(...)` builtin keeps the literal result.

Keyword subscripts are the general feature the String's explicit index forms
ride on: a named bracket argument over a lowercase (value) base parses as a
`MultiIndex` with `SubscriptArg::Keyword`, selected against keyword-only
`__getitem__` parameters through ordinary structural call binding and carried
to the VM through the subscript instruction's keyword channel; a named
bracket over a capitalized type name stays compile-time parameter
application. Keyword-only parameter names are part of callable identity —
`same_method_shape` compares them at declaration and `SignatureKey` appends a
`$kw$...` suffix (only when keyword-only names exist) in both symbol
producers, the checker's call-target mangling and MIR's declaration-side
mangling.

### Collection-owned interior origins

An origin path may contain `Interior("tag")`, distinct from an unknown runtime
index. It names storage owned behind a container, such as
`values["element"]`, `mapping["value"]`, or a Variant payload. Each
`EstablishLoans` marker is a fresh generation and may retain several possible
origins for a union-valued return. Multiple overlapping interior references may
coexist, ordinary owner reads remain legal, and a direct List element write
updates the storage those references designate.

The checker, rather than MIR syntax inspection, records every operation that may
redefine an interior. Lowering preserves those facts as
`InvalidateInteriors { base, except, include_base_generation, marker }`
immediately before the operation.
Whole-owner replacement (including writes through references, reference-valued
aggregate fields, and origin-bearing pointers), replacement of an interior that
owns deeper interiors, structural List mutation, mutable/ref calls, Dict lookup
or replacement, and Variant tag replacement invalidate matching old
generations. `except` preserves the generation used to mutate through an
interior reference while still invalidating any nested interiors below it.
Ordinary mutation leaves the exact base generation valid;
`include_base_generation` instead records Mojo's owned-interior refresh, where a
new generation replaces that exact named region. Dict `__getitem__` uses this
mode for `mapping["value"]`, invalidating an earlier value reference without
invalidating the sibling `mapping["element"]` generation retained by key
iteration.

A separate forward may-analysis carries generation sets through CFG joins and
loop back-edges. An invalidation on any incoming path makes a later use of that
generation an error; the diagnostic identifies the canonical origin and points
both to the stale use and the invalidating operation. Prefix matching is
field-sensitive, so mutating `pair.left` cannot invalidate
`pair.right["element"]`. Structured `try` regions carry distinct normal,
raising, and return/escape channels: handlers join only actual raising sites,
`finally` is checked on every channel, and only normal fallthrough reaches the
instruction after the region. This generation analysis is deliberately
separate from ordinary shared/exclusive loans: interior invalidation governs
storage identity, while ordinary loans continue to govern direct place access.

Origin-parametric aggregate fields store those same handles rather than reading
and copying their referents. A normal field access reads through the handle;
assignment writes through it. MIR transfers the originating loan to the
aggregate binding. Reaching-generation-aware drop liveness keeps the current
owner alive through aggregate handle use, releases the old owner when the
aggregate is rebound, and retains the replacement generation independently.
A stored `MutUnsafeAnyOrigin` reference is rejected because it would hide an untracked mutable
capability behind an otherwise safe value.

`UnsafePointer(to=place)` rides the same machinery. The checker infers
`PointerOrigin::Place` from the source place — mutability follows the owner
binding — and the checked pointer type retains that provenance through HIR and
MIR while the VM value stays an origin-free frame/slot handle. Construction
lowers to `MakeRef` plus `EstablishLoans` on the pointer binding; a stably bound
pointer's `p[0]` deref substitutes the frozen owner place (`MirPlace::through`
names the pointer), so owner liveness, ASAP destruction, and loan conflicts stay
exact, while reassigned or field-loaded pointers read and write through their
runtime handles. Aggregates that store place-origin pointers carry the owner
loan exactly like reference-valued aggregates. Because an origin-bearing
pointer designates one checked value rather than an allocation, the checker
rejects non-zero offsets, pointer arithmetic and comparison, `free()`, writes
through immutable provenance, and returns that would escape the origin
(`returned pointer escapes storage outside its declared origin`). A method may,
however, return the *dereference* of an origin-bearing pointer field whose origin
is a struct/callable parameter (`def get(self) -> ref[o] Int: return self.p[0]`):
the returned `ref[o]` stays within that parameter, so the return-boundary
re-rooter (`canonical_reference_parts`) follows the field handle to the single
pointee and retains the residual offset-0 index, which the runtime projection
walkers forward as the identity deref of that pointee — an immutable origin reads,
a mutable origin writes through the caller's storage.

### Loops

Loops matter because a move in one iteration can affect the next iteration.

For example:

```mojo
var x = Box(1)
for i in range(3):
    var y = x^
```

The back-edge makes the moved state flow to the next iteration. The analysis can
therefore reject the second iteration's attempted move.

Borrowed iteration, and consuming iteration for a type with `__iter__(var self)`,
first execute the checker-selected nominal `__iter__` normalization chain. Every
borrowed source is retained in its own slot — statement loops and comprehension
clauses share the rule — and `GetIter` writes the normalized iterator into a
distinct iterator-object slot, so the source stays live in its own slot through
the loop rather than being overwritten during normalization. A borrowed
**temporary** — the only owner of its storage — is `Bind`-bound and kept live by
a `KeepAlive` liveness anchor at the loop exit, then destroyed exactly once
after the loop. A borrowed **named** source is instead `MakeRef`-bound (a
genuine reference, no copy) and its dependency is recorded as a loan — a
whole-place shared loan, or an interior `element` generation for a concrete
collection place — re-established on the long-lived iterator-object slot: the
loan keeps the source live through the loop (no `KeepAlive` needed) and rejects
conflicting mutation of the source during iteration; the reference slot is read
only by `GetIter` and dropped afterward as a no-op. Owned iteration keeps the
single slot: `__iter__(var self)` consumes the source into the iterator.
Bundled borrowed paths cover List, Set, Dict, and Range; the bundled owned path
is currently List-specific. For a current typed-raising iterator, `TryNext`
invokes `__next__(mut self)`, writes the mutated iterator back, and branches on
whether the call returned an element or raised the exact checked
`StopIteration`. The older `HasNext`/`Next` pair remains for nonraising iterators
whose selected `__len__` reports exhaustion. Bundled Range/list/set/dict
iterators and concrete user iterators use those nominal paths. The only
method-free fallbacks are the CTFE-only `ComptimeList` carrier and the
compiler-private heterogeneous runtime-pack carrier; public `Tuple` values are
nominal and do not use that fallback.

Borrowed concrete List, Set, and Dict place iteration retains an `element`
interior-origin loan for the live iterator. For List, nonstructural replacement
remains visible, while a structural mutation invalidates that generation and a
subsequent iterator use is rejected. Concrete List `for ref` similarly creates
write-through indexed element references. A user-defined reference-yielding
iterator can now satisfy an abstract value `__next__` contract for a `Copyable`
element and execute through the checked copy adapter. Generic code still cannot
derive a borrowed source loan, yielded-reference origin, or abstract `for ref`
binding from the associated iterator contract until the bundled protocol and
loop source/binding modes are migrated.

Consuming `for var item in collection^` moves the source once into the iterator
slot. Each `Next` transfers one element, so the current loop binding and the
residual iterator state have disjoint ownership. Normal exhaustion leaves no
residual elements; return, raise, and `break` paths run the ordinary edge
cleanup, and the exit edge drops the iterator slot. Protocol-driven owned
iteration carries current Mojo's element bounds: the yielded element must be
`Movable & Deinitable`, enforced both by the bundled `__iter__(var self)`
where clauses (an unavailable declaration rejects with the bound named) and
by a checker gate over user-declared owned iterators, so the implicitly
dropped iterator always has its residual-destroying `__deinit__` available.

Variadic packs are not library iterators, and linear whole-pack forwarding
remains supported under guaranteed exhaustion. For that channel the checker
still rejects every abandoning path when the element type is not
`Deinitable`: the syntactic body walk (break/return/raise), an observation
frame that flags any raising call whose `try` handler sits outside the loop
(callable bodies push barrier frames so nested `def`s never mark an enclosing
loop; the frame records the `handled_raise_depth` at loop entry, so a `try`
inside the body contains its error), and — in comprehensions — filter clauses
over a linear binder, since a skipped element would be abandoned. Each
diagnostic names the element's `@explicit_destroy` obligation.

### Partial Move Tree

The analysis tracks places at field granularity. This is stricter and more useful
than only tracking whole variables.

It can distinguish:

```mojo
var a = p.left^
print(p.right)   # ok
print(p.left)    # error
```

Dynamic indexed moves are more conservative because arbitrary indices can alias.
Compiler-private heterogeneous Tuple storage is the narrow exception: a
compile-time element index lowers to `Proj::ConstIndex`, so the move tree can
distinguish element 0 from element 1. This lets `Tuple(*args^)` and
`consume_elements` relocate linear pack elements exactly once while public
runtime-varying indexed transfers remain rejected.

## Stage 7: Liveness And ASAP Destruction

Same module:

```rust
src/analysis/mod.rs
```

Entry point:

```rust
elaborate_drops_program(prog: MirProgram) -> MirProgram
```

ASAP destruction is implemented as a MIR rewrite. The analysis computes where
owned variables stop being live and conservatively splices explicit:

```rust
MirInstr::DropVar { var }
```

after each variable's last use.

The VM does not need to discover last uses dynamically. It just executes
`DropVar` where the compiler placed it.

### What Gets Dropped

Drop roots are selected independently of type: every owned local and consuming
parameter receives drop glue. This conservative policy releases heap-backed
runtime storage at its last use even when no user `__deinit__` call is observable,
and it naturally covers destructor-less structs containing aggregate storage.
Ownership is limited to:

- locals
- consuming `var` parameters

Borrowed parameters are not dropped by the callee. They are owned by the caller.
`self` is handled carefully to avoid destructor recursion and to support method
write-back.

### Drop Order

When several variables die at the same point, they are dropped in reverse
declaration order. Struct destruction runs:

1. the struct's `__deinit__(deinit self)`, if present
2. fields in reverse declaration order

The compiler-private heterogeneous pack carrier drops elements left-to-right,
matching current Mojo's pack-storage lifecycle. Public collections, including
`Tuple`, are nominal structs and otherwise follow the ordinary reverse
declaration order for fields; their library destructors own any element-specific
teardown.

Types whose `Deinitable` conformance is explicitly unavailable, such
as `Deinitable where False`, are excluded from this automatic path.
A declared conditional `Movable` conformance is likewise effective:
`is_movable` evaluates the struct's own `Movable where ...` predicate, so a
false condition rejects `^` transfers, `var` parameters and receivers, and
move/copy captures at the checker's consuming positions (`check_consuming_as`
distinguishes an ownership `Move` from `Deinit` consumption, so destructors
and named destructors still consume a non-Movable value).
The optional `@explicit_destroy("message")` decorator does not control
linearity; it supplies the required user-facing diagnostic when an obligation
is violated and is inert on an implicitly deletable type. A checked,
stable-binding obligation analysis requires every initialized linear value to
reach exactly one named `deinit self` method on every exit. It rejects
abandonment, overwrite, and inconsistent branch or loop states.
MIR retains the resulting declaration metadata. The checked obligation ensures
that an intact linear value is consumed before it can reach automatic
`DropVar`; the VM therefore does not guess concrete deletability from an open
generic struct name. If any aggregate field has already moved, drop glue skips
the whole-value `__deinit__` for every struct and recursively destroys only its
initialized residual fields.

A named explicit destructor is lowered as a call followed by `ConsumeVar` for a
whole binding or `ConsumePlace` for a projected field. Drop elaboration treats a
pending `ConsumeVar` as the variable's teardown — the variable stays live up to
it and counts as moved there — so no competing ordinary `DropVar` is spliced
between the call and the consumption (which would re-run the whole-value
`__deinit__` the named destructor replaced). The call retains its receiver
place, and the VM writes the callee's final `self` state back before the
consumption runs, so residual-field destruction sees exactly what the body
left: moved fields are tombstones (no re-drop), and a drained pointer-backed
container field is empty rather than a stale pre-call clone (no double free).
Consumption then destroys those residual fields in reverse order without the
whole-value `__deinit__`.
Because consumption occurs only after a successful return, a raising destructor
leaves the source slot live on the exceptional edge so an `except` handler can
invoke a fallback destructor.

### Explicit-Destruction Partial Moves

An intact linear binding begins with one whole-value obligation. Moving a field
decomposes that obligation into stable paths for its directly linear child
fields. A moved linear field carries a new obligation at its destination;
ordinary residual fields remain eligible for automatic reverse-order dropping.
The aggregate's whole-value destructor is unavailable while any field is moved,
but a projected linear field can use its own named destructor. Reinitializing all
moved fields reconstructs the intact state and restores the whole destructor.

Obligation paths are part of structured control-flow state. Branches,
exceptional exits, and loop backedges must agree on both the remaining
obligations and moved fields, preventing a partial state from being hidden by a
join. MIR `ConsumePlace` preserves sibling storage, and the VM drops only
ordinary residual fields of an incomplete linear aggregate.

### Edge Drops

Some values die on control-flow edges rather than immediately after an
instruction. The liveness pass handles these by inserting drops:

- at the end of the predecessor when there is only one successor
- at the start of the successor when there is only one predecessor
- in a fresh split block for critical edges

This keeps ASAP destruction precise across branches, both in a function's
top-level CFG and inside each `try` region's mini-CFG.

### Try Region Drops

Region interiors get the same per-instruction and edge drop elaboration as
top-level blocks: each of a `try`'s four regions is a mini-CFG whose liveness
is seeded from the enclosing walk at the instruction, per exit kind — the
normal `FallOff` continuation (`else` entry, then `finally` entry, then the
code after the block), `Return`/`ReturnWithCleanup`/`EscapeJump` edges (the
`finally` still runs after them, and a crossing return's cleanup values are
torn down after it), and `EscapeJump` targets bounded by the enclosing
effective live-in. Because any potentially-raising instruction can transfer
control to the raise edge's observer (the handler, or the `finally` and the
enclosing handler when there is none), a **raise seed** — that observer's entry
liveness — is unioned into the live set at every instruction not on a minimal
allowlist of provably silent operations (`DefVar`, `UseVar`, `DropVar`,
`ConsumeVar`, `KeepAlive`). Backward propagation therefore places each
normal-flow drop after the last potentially-raising instruction preceding the
death: a handler can never observe a vacated slot, and an outer variable
rebound in the body runs the overwritten value's destructor between the
constructing call and the rebind — skipped exactly when that call raises. A
value live into the `try` that no region path can observe (an unconditional
silent rebind precedes every potential raise) dies on the entry edge,
immediately before the `try`. The loan machinery participates: generation and
register-loan fixpoints run over each region with entry states replayed from
the enclosing walk (regions entered by raise or completion use the union of
every state the preceding region can reach — pure over-retention), so owner
retention and retirement behave identically inside regions.

Per-instruction drops own all *normal-flow* deaths inside regions. The raise
edge cannot host per-instruction drops, so the scope-exit cleanup lists remain
and act as raise-edge/scope-exit backstops. The VM's `DropVar` and cleanup
teardown on an already-vacated (`None`) slot are no-ops, and that idempotency
is load-bearing: a value may legitimately be listed on a cleanup edge it
already died before.

### Try Cleanup

Try regions need cleanup for the values only the body can still observe. The
drop elaboration pass fills `MirInstr::Try.cleanup` so the VM can destroy them
when the body exits through normal completion, raise, return, break, or
continue. The set holds the body's *locals* — variables whose every `DefVar` in
the function lies within the body region (a reassignment is also a `DefVar`, so
an outer variable merely rebound inside the body is not a local and survives
the exit) — plus the liveness-guarded rebound outer variables that cannot be
observed after the body is left: dead on the normal continuation, unused by the
handler/`else`/`finally` regions, and dead at every escape target. That second
set is the raise-edge backstop for rebound values whose per-instruction drop
sits on the normal path: a raise landing between the rebind and that drop
would otherwise leak the value.

`EscapeJump` also carries cleanup for cross-region loop escapes. This makes
hidden try-region exits explicit enough for the VM to run destructors before
jumping to the enclosing loop target.

Iterator-driven loops also place an explicit drop at their common exhaustion or
`break` exit. A `return` cannot reach that block, so HIR and MIR retain its
current binding and iterator owners on `ReturnWithCleanup`; this preserves
return-value evaluation order and carries destruction through nested
`try/finally` regions.

## Stage 8: Register VM

Module:

```rust
src/backend/vm.rs
```

The register VM executes verified MIR. It is structured rather than
byte-addressable:

- registers hold rich `runtime::Value`s
- variables are frame slots
- public collections are ordinary nominal struct values; their storage uses the
  same pointer arena and struct fields available to user code
- private heterogeneous packs, CTFE-only lists, strings, errors, variants,
  slices, and SIMD values retain dedicated runtime carriers
- field and index operations work through high-level value navigation
- calls allocate a new VM frame

Frames are owned by an explicit VM stack and have monotonic identities. Direct
user-function calls push a frame plus a return/write-back continuation; returns
pop and resume the caller in the iterative dispatcher. This makes ordinary deep
source recursion independent of the Rust call stack. The frame shape is:

```text
id: FrameId
function/block/instruction cursor
registers: Vec<Value>
variables: Vec<Value>
return continuation
```

`regs` are temporaries. `vars` are source variables, parameters, and compiler
synthetic locals.

### Program Metadata

The VM builds a `Prog` containing:

- lowered MIR
- struct definitions and field layouts
- method mutability information
- function signatures
- default arguments
- value-parameter declarations
- signature-mangled overload definitions and fallback lookup for unique arity
  protocol calls

The checked entry point is the production path. It normalizes declaration facts
into `MirDeclarations`: struct field layouts and callable parameters use checked
`Ty`, defaults use `CheckedConst`, and overload names come from `CheckedProgram`.
The VM builds compact registries from this metadata rather than rescanning AST
annotations, reevaluating default expressions, or recomputing overload choices.

### Function Calls

Calling a function:

1. resolves the function index
2. matches arguments to parameters
3. coerces arguments to parameter types
4. creates a new frame
5. writes arguments into parameter variable slots

For a homogeneous `var **kwargs: T` collector, the checker leaves explicit parameter
binding unchanged and type-checks unmatched keyword values as `T`. The same
logic participates in generic inference and in free, instance, static, and
bounded-trait method selection. The ABI preserves unmatched pairs in call-site
order. The VM constructs the implicitly linked, self-hosted `StringDict[T]`
directly in the collector's callee slot; it is an owned mutable local and never
participates in caller write-back. `callee(**kwargs^)` consumes that dictionary,
moves its ordered entries back into the shared binder, and retains ordinary
duplicate/missing-keyword diagnostics and the checker-selected effect contract.
6. binds value parameters into frame locals
7. runs the callee's block loop
8. returns the result and, where needed, final variable slots for write-back

`mut` and explicitly mutable `ref[origin]` parameters receive handles to the
simple caller places retained in MIR. Keyword places are resolved after argument
binding by the selected parameter name, so reordering and ordinary defaults do
not lose identity; a value synthesized by `**kwargs^` has no writable source
place. Inside a structured region, a temporary caller-frame mirror makes these
the same frame/slot handles used by continuation-driven calls and is committed
on both normal and raising outcomes. Bare `ref` remains parametrically mutable
and may not write in an unspecialized body.

Overloaded function calls arrive at the VM already resolved to a lowered
signature name. For constructor overloads, a direct resolved callee such as
`Box.__init__$ov$String` still enters the constructor path: the VM creates the
uninitialized `self` skeleton, binds the remaining arguments through the same
positional/default/keyword matcher used by ordinary calls, invokes the selected
`__init__`, and returns the initialized struct. Internal dunder/protocol paths that do not have a source call
span can still ask for a unique overload by source name and arity; this is a
fallback for compiler-generated calls, not the general overload-ranking engine.

### Method Calls

Method calls are normal function calls with a receiver convention:

- `self` is parameter slot 0
- ordinary arguments use the same positional/keyword/default/variadic slot
  matcher as free functions before `self` is prepended
- `mut self` writes the final receiver back to the caller place
- ordinary `mut` and explicitly mutable `ref[origin]` method parameters also
  bind through retained positional or keyword caller places
- nominal collection mutators commit through a reference-aware receiver place,
  including pointer-backed self-hosted `List` fields

Method-dispatched nominal `Index`, `Slice`, `MultiIndex`, and `MultiSet` enter
this same selected-method dispatcher and argument binder. They never choose a dunder by
runtime name or arity: the retained `MirSubscriptCall` supplies the lowered
target, value-parameter arguments, typed raising contract, caller places, and
reference-result metadata. The VM only materializes slice descriptors, invokes
that contract, performs selected write-back, and returns or propagates its
result. The call-less `Slice.indices()` Tuple-storage bridge described above is
the sole nominally typed exception.

### Moves At Runtime

Static ownership analysis should reject invalid moves before execution. The VM
still models move effects:

- moving a variable transfers the value out of the source slot
- the source slot becomes moved/empty
- moving a field leaves a moved marker in that field
- using a moved slot at runtime is a loud error, not silent behavior

This makes the VM a useful backstop and executable model for ownership semantics.

### DropVar At Runtime

`DropVar` removes the value from a variable slot and recursively destroys it.

Dropping is observably a no-op for scalars and destructor-less leaf values. For
structs with `__deinit__`, it calls the destructor and then drops fields. A
destructor-less struct still recursively destroys aggregate fields; elements
inside the compiler-private heterogeneous pack carrier are visited
left-to-right. Moved-out fields are skipped so partial moves do not double-drop.

### Exceptions And Non-Normal Flow

`raise` propagates the original runtime value as a `Raised` error until a `Try`
catches it. This preserves fields on user-defined typed error structs; string
raise shorthand is normalized to the builtin `Error` value.

Inside try sub-regions, the VM uses a control-flow enum conceptually like:

```rust
Normal
Return(Value)
Jump(MirBlockId)
```

This lets `return`, `break`, and `continue` cross a `try` boundary while still
running cleanup and `finally`.

The rule is:

- body raise goes to `except`, if present
- `else` runs only after normal body completion
- `finally` always runs
- non-normal flow from `finally` overrides the pending body/handler/else outcome

## Runtime Values And Builtins

Module:

```rust
src/runtime/mod.rs
```

The VM operates on `runtime::Value`, the shared representation for supported
runtime values:

- integers, unsigned integers, floats, booleans
- strings
- `None`
- structs, including public `List`, `Set`, `Dict`, `Range`, and `Tuple`
- CTFE-only `ComptimeList` values
- compiler-private heterogeneous pack tuples
- variants (checked alternative list, active tag, and payload)
- slice descriptors (contiguous, strided, or general, with optional bounds)
- SIMD-like lane vectors
- errors
- moved/tombstone markers

Runtime helpers implement:

- arithmetic and comparison
- prefix operators
- coercion and numeric conversion
- string display
- list methods
- SIMD construction and lane access
- builtin functions such as `print`, `len`, `range`, numeric conversions,
  `abs`, `min`, `max`, and `round`

Keeping value-level behavior in `runtime` prevents the VM from baking every
operation directly into the backend. The VM should be a consumer of checked MIR
plus runtime primitives, not a second checker.

## Unsupported Constructs

Unsupported constructs should be explicit.

Preferred behavior:

- parser accepts Mojo-like syntax when possible
- checker rejects unsupported semantics early when it can
- MIR may contain `MirInstr::Unsupported` for late-discovered backend gaps
- VM reports a clean `RuntimeError::Unsupported`
- tests assert unsupported behavior instead of allowing panics

This is important because mojito parses more syntax than it fully implements.
A clean unsupported error is part of the architecture.

## Fixture And Test Relationship

The architecture is reflected in test layout:

- parser tests check AST shape
- checker tests check type and semantic acceptance/rejection
- HIR tests check CFG shape
- MIR tests check lowering shape
- ownership tests check move analysis
- drops tests check ASAP destruction
- VM tests check execution
- `assets/` fixtures exercise whole-pipeline behavior

Accepted `.mojo` programs belong in:

```text
assets/ok/
```

Ownership-specific fixtures belong in:

```text
assets/ownership_ok/
assets/ownership_error/
```

The asset harness turns examples into executable documentation. A feature is more
real when it has a fixture.

## Architectural Boundaries

### Checker vs MIR Analysis

The checker should answer questions that are local to declarations,
expressions, types, and calls.

MIR analysis should answer questions that require control flow:

- has this value been moved on all paths?
- has it been maybe-moved on one path?
- where is the last use?
- where should destruction occur?
- which branch edge needs a cleanup block?

### HIR vs MIR

HIR owns statement-level control flow while expressions remain nested.

MIR owns expression flattening, register allocation, places, and instruction
semantics.

If a feature needs to know the order of subexpression evaluation, it belongs in
MIR or later. If it needs only branch/loop shape, HIR is the right layer.

### MIR vs VM

MIR should preserve enough semantic facts that the VM does not need to infer
language rules from source syntax.

The VM may still hold runtime metadata such as struct field layouts and function
signatures, but the direction should be toward checked declarations and MIR
metadata becoming the source of truth.

### Runtime vs Backend

The runtime module owns value operations. The VM owns execution order, frame
management, calls, jumps, drops, and exception flow.

This separation makes it possible to add another backend later without
reimplementing every scalar/list/string/SIMD rule from scratch.

### Textual MIR/VM assembly boundary

The normative version-1 grammar, compatibility policy, canonical ordering, and
serialization inventory live in [`mir-text-format.md`](mir-text-format.md).
`mir::text` owns the matching version constants, closed mnemonic vocabulary,
and the canonical `disassemble` entry point. The printer rejects MIR invariant
or verifier findings before emitting any text, serializes checked semantic
metadata directly without AST or `Debug` reconstruction, and normalizes
unordered tables through sorted borrowed views. The later parser must share
this vocabulary.

`mir::text::parse_artifact` owns the inverse syntax boundary. It validates UTF-8
bytes, parses a Mojo-independent spanned schema tree, and decodes structural MIR
while retaining an `ArtifactSourceMap` separate from serialized Mojo source
locations. Parsing does not invoke semantic MIR verification:
`mir::text::verify_artifact` composes the canonical `mir::verify` pass on the
returned `ParsedArtifact` and maps each finding to its assembly span (block,
then function, then artifact-root precision) by parsing the verifier's
canonical `MIR function '<name>' [block <n>]` message prefixes — the one
sanctioned consumer of that spelling. `load_artifact` bundles parse-then-verify
as the loading gate artifact execution sits behind; verification policy itself
never moves out of `mir::verify`.

The compiler exposes a human-readable, flattened, versioned serialization
of verified MIR and the metadata needed to execute it. The format must support:

- deterministic printing suitable for review and golden tests (implemented)
- parsing with source-located diagnostics (implemented for the seed
  instruction subset)
- structural and semantic verification before execution (implemented)
- lossless print/parse/print round trips
- disassembly of verified in-memory programs (implemented)
- execution by the register VM without reconstructing source AST semantics
- consumption by future native backends (LLVM and the MLIR-family targets first)

This is a Mojito format, not a generic interchange standard. The in-memory MIR
remains authoritative; textual assembly is its stable inspection and artifact
boundary. A compact binary encoding may later share the same schema.

## Current And Future Pressure Points

The main pressure points are:

- CTFE function-body execution uses restricted MIR/VM execution, while nested
  generic requests return to the same structural specialization worklist
- compile-time declaration generation is deliberately structural: reflection
  selects parsed declarations through `comptime` control flow rather than a
  string-to-AST macro channel
- ABI-sensitive reflection such as byte offsets belongs to the future native
  backends; VM reflection exposes semantic field indexes and checked projections
- MIR is fully register-typed and semantically verified; checked capture and
  binder facts cross HIR without source-name/span reconstruction. The remaining
  compatibility boundaries are name-based callee fallbacks kept only for the
  unchecked phase-test path, nominal callable-conformance facts in
  `MirDeclarations`, and caching the verified `MirProgram` in `CompiledProgram`
  to avoid the compiler/VM double lowering
- the backend-ready MIR checkpoint is closed: abstract erased-dispatch
  requirements live in typed call-local contracts, callable-value requirements
  live in their stored `Ty::Func`/`Ty::GenericFunc` contracts, variadic
  conventions are explicit declaration fields, and reference loans are checked
  against their executable capability permission and canonical interior owner
- source modules and packages are flattened after lexical namespace resolution;
  compiled `.mojoc` artifacts remain future distribution tooling
- trait support is intentionally incomplete; in particular, associated
  compile-time types are monomorphic, so origin-parameterized iterator families
  must cross the checked/MIR boundary before its textual schema freezes
- abstract trait-dispatch subscripts are verified from their complete
  checker-retained argument/result requirement and retargeted to a concrete
  method only at execution
- exception modeling is structured, not a fully general unwind-edge MIR
- nested-function and capture support should match Mojo's non-escaping patterns
  without growing into a general escaping-closure system
- more library types can migrate from runtime/compiler support into self-hosted
  modules as the language subset gets stronger
- diagnostics should continue moving from "correct" to "pleasant"

## Mental Model

Read the compiler from the middle outward:

1. MIR is the contract.
2. HIR exists to make control flow explicit before MIR.
3. Module linking assembles imported declarations into one program.
4. Comptime elaboration erases compile-time control and materializes constants
   before runtime checking.
5. The checker prevents unsupported or ill-typed programs from reaching MIR.
6. Analysis proves ownership and inserts destruction.
7. The VM executes what MIR says.

That is the core architecture of mojito after parsing.
