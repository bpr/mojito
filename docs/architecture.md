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

## Workspace Layout

Since the 2026-09 crate split, each pipeline phase is its own workspace crate
under `crates/`, and the root `mojito` crate is the facade: the driver
(`compiler::Compiler`, `artifact`), the CLI (`main.rs`), and a `lib.rs` that
re-exports every crate at its historical module path (`mojito::checker`,
`mojito::mir`, ...), so consumers and tests are unaffected by the split.

Dependency direction is enforced by Cargo. Bottom-up: `mojito-common`
(token/literal/errors) → `mojito-ast` (+ structural call binding) →
`mojito-types` (Ty/origins/ct) → `mojito-symbol` → `mojito-checked` →
{`mojito-lexer` → `mojito-parser` → `mojito-module`, `mojito-hir`,
`mojito-native-core`} → `mojito-checker` → `mojito-mir` → `mojito-analysis` →
`mojito-vm` → {`mojito-native` → `mojito-pliron` (feature-gated),
`mojito-comptime`} → the root facade. Note that the crate DAG is not the
pipeline order: `mojito-comptime` sits **above** the VM because CTFE compiles
and executes sub-programs through `VmBackend`, which itself re-runs checking
(the stage-composed seam contract). `crates/mojito-runtime` remains the
independently versioned native C-ABI runtime, depending on nothing above.

## Big Picture

The post-parse pipeline is:

```text
Vec<Stmt>
  -> module link
  -> source validation
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

`CompiledProgram` privately retains the `CheckedProgram`, the single
ownership-verified, pre-drop `MirProgram` lowered by the driver, and — computed
lazily from it exactly once — the drop-elaborated, re-verified `MirProgram`
exposed as `CompiledProgram::elaborated_mir`. That cached post-drop program is
the exact backend artifact: execution hands it to `Backend::run_elaborated` and
`CompiledProgram::emit_mir` serializes it to canonical text, so both consumers
observe one retained artifact rather than re-deriving elaboration per call.
Post-drop verification findings fold into its `invariant_errors`, which every
consumer refuses when non-empty. Production backends therefore never re-lower
checked syntax. `CompilerError` identifies the failing
stage. Individual stage functions and `Backend::run(&CheckedProgram)` remain
public compatibility seams for tests and diagnostic tools; that stage-composed
entry re-checks pre-drop ownership but is non-authoritative for whole-program
discovery and specialization (a generic body forwarding its own `H` into
`hash[H](x)` binds the declaration default there, while `Compiler` binds the
caller's; multi-lane hasher clones exist only through the discovery loop's
`hash_leaf_types` channel).

The design is an hourglass:

```text
parsed AST
   |
   v
module linker
   |
   v
source validation (every comptime arm, parameters symbolic)
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

mojito does not today reproduce Mojo's production architecture, and that
is now regarded as a mistake that must be corrected (`docs/pliron-future.md`).
Resembling Mojo's own implementation as closely as a small compiler can is a
goal, pursued in stages over a long horizon; the arrangements described in
this document are where the code stands, not an argument that the distance
from Mojo is right. Whether MLIR — or any backend IR — becomes the compiler's
internal layer is one of those staged questions
(`docs/pliron-backend-pivot-plan.md`), no longer a standing exclusion.

What the current pass does not attempt is scope, not architecture: it targets
single-threaded CPU language semantics and leaves out parallelism, distributed
execution, Python interoperability, legacy `fn`/`owned` and other removed
source spellings beyond clear rejection diagnostics, and escaping closures with
the removed `escaping` effect — closure parity targets Mojo's current
non-escaping capture-list model. Two of those lines are drawn differently.
Concurrency in its most primitive form is expected: not tasks or parallel
execution, but the smallest primitives the language needs, on no schedule yet.
GPU is a stretch goal: we intend to reach it, but it is not on the immediate
horizon. Distributed execution and Python interoperability are the ones we do
not intend to pursue. The register VM is
the executable specification. A versioned textual MIR/VM assembly form is the
next representation boundary; the prioritized native backends are the
Rust-native, MLIR-inspired [Pliron](https://github.com/pliron-org/pliron) (its
LLVM dialect emits LLVM IR; `docs/roadmap.md` contains the staged adoption and
fallback plan) and Cranelift, with a C or C++ source backend as a possible addition.

Native-backend work is isolated from the default build as an invariant: the
default `mojito` build and `scripts/check` resolve no LLVM or Pliron
dependency (`tests/backend_isolation_test.rs` guards the default feature
graph). The supported backend lives in the workspace crate `crates/mojito-pliron/` behind the
optional `backend-pliron` feature — a compile path
(`mojito compile --backend pliron`, plus `run --backend pliron` for the
advertised subset) consuming the cached post-drop `elaborated_mir` artifact,
with its own gate (`scripts/check-pliron`, which also chains the Stage 0
spike gate) and a separate memory-heavy corpus lane
(`scripts/check-pliron-heavy`, `tests/heavy/`). Production execution stays on the register VM. The current pin
and promotion rationale live in `docs/notes/pliron-promotion.md`; the per-stage designs and recorded
VM/native divergence policies in `docs/notes/pliron-stage1.md` through
`docs/notes/pliron-stage4.md`.

The backend-independent half of that work has one owner: the un-gated
`native` surface (`crates/mojito-native-core/` for target/layout/runtime-ABI
below the MIR waist, re-exported with monomorphization and mangling by
`crates/mojito-native/`) holds the checked build configuration (target triple
with a pinned data-layout string, CPU features, optimization level, output
kind), the layout engine, the injective symbol mangler, and the runtime ABI
contract table, and the workspace crate `crates/mojito-runtime` implements
the versioned `mjrt_*` C ABI every produced executable links (never
depending on the `mojito` crate or the VM's `Value`). The normative contract
— scalar semantics, layouts, calling convention, reserved symbol namespaces,
per-symbol runtime rules, and the mechanical Rust/LLVM agreement checks — is
[`docs/native-abi.md`](native-abi.md); any later backend (Cranelift
included) consumes the same module and runtime.

Native backend support and its required gates are Linux-only, currently for the
single target defined in `docs/native-abi.md`. Supporting additional hosts or
targets requires a separate, explicitly resourced decision; it is not a Pliron
promotion condition.

### Native Backend Contract

The preferred native architecture sits entirely below the verified-MIR waist:

```text
source -> Compiler -> ownership-verified, post-drop verified MIR
                            |                 |
                            |                 +-> VM / canonical .mir artifact
                            v
              backend-private monomorphization
                            |
                    Pliron lowering
                            |
             Mojito ops only where justified
                            |
                    Pliron LLVM dialect
                            |
            LLVM IR / bitcode / object / executable
                            |
                    versioned runtime ABI
```

Pliron does not replace Mojito MIR in the current architecture: it is an
optional lowering, verification, transformation, and optimization framework
below the serialized MIR handoff. Making it a required IR framework is an
open, staged decision (`docs/pliron-future.md`,
`docs/pliron-backend-pivot-plan.md`), and until a stage lands the contract
here is the rule. A backend consumes `MirProgram`; it does not import AST,
HIR, checker, ownership, or call-binding policy. The native interface accepts a
`&MirProgram`, a target description, and an output kind, and returns textual
IR, bitcode, an object, or an executable plus structured diagnostics;
`run --backend pliron` executes only the advertised subset natively and
rejects everything else with a contextual diagnostic — never a silent VM
fallback. Origin and ownership facts erase after validation, while explicit
drop and cleanup instructions remain executable behavior. Errors and
`try`/`finally` lower as tagged outcomes and explicit CFG edges; platform
unwinding stays out until it has its own semantic, ABI, and portability
specification.

Variant storage lowering (the `__VariantStorage` primitive behind the
self-hosted `std.utils.Variant`) uses the shared native tag/payload layout
and dispatches owning operations and destruction from the runtime tag. Multi-lane SIMD is
stored as a contiguous lane-aligned scalar aggregate and computed as LLVM
fixed vectors in SSA between loads and stores; the vector representation is
backend-private, and the VM's lane semantics remain the contract. User `__moveinit__` and `__deinit__` bodies remain
ordinary compiled MIR calls, with a `deinit self` callee owning residual-field
teardown.

Optimization policy lives in one profile-to-pipeline table
(`backend/pliron/pipeline.rs`, snapshot-pinned): every profile runs the same
pliron cleanup stage between whole-module verifications, and only `release`
adds the pinned external LLVM pipeline. External tools are resolved and
version-checked up front as a reportable toolchain object; the runtime
archive validates by provenance, digest, and its embedded ABI version before
linking. Emission is deterministic and failure-atomic: clean builds are
byte-reproducible at both profiles, emitted objects carry a validating link
manifest consumed by the CLI `link` verb, and executables default to DWARF
line-level debug information attached below the conversion boundary. The
policy record is `docs/notes/pliron-stage6.md`.

Dialect policy: lower directly to Pliron's LLVM dialect, and introduce a
narrow `mojito` Pliron dialect only for demonstrated needs such as runtime
calls, checked traps, explicit error propagation, target-independent
aggregate constants, or lifecycle normalization. Do not reproduce the MIR
schema as a second operation set. That policy governs the backend as it
exists. The `mojito.semantic`/`mojito.core`/`mojito.abi` dialects in
`docs/pliron-backend-pivot-plan.md` would supersede it, and only a landed
migration stage does so. Every custom operation needs textual syntax, a
verifier, negative coverage, and a total conversion rule; LLVM
emission rejects any residual illegal operation. Missing Pliron facilities
are upstreamed as narrowly scoped patches rather than accumulating an
untracked local fork.

The front end's parameter-expression layer (`mojito_types::param_expr`, Stage
2 *Parameter Expressions*) is independent Rust data shaped like a dialect's
uniqued typed attributes, so moving it into a parametric dialect is a
re-homing of storage and not a redesign. It needs no Pliron dependency, and it
does not move `CheckedProgram`, the MIR waist, or the VM's authority.

Every backend stage is tested through four complementary layers:

1. IR unit tests for each lowering, verifier, rewrite, conversion, ABI type,
   and unsupported diagnostic.
2. Canonical Pliron/LLVM snapshots with UTF-8, LF, and one trailing newline.
3. VM/native differential tests comparing stdout bytes, bindings or result,
   error category, and ordered lifecycle events at `O0` and optimized levels.
4. Artifact tests proving `.mir -> VM` and `.mir -> native` consume the same
   serialized program.

Track compile time, peak memory, IR size at each boundary, object size,
execution time, and supported/excluded MIR counts (the generated
`conformance/pliron-parity.tsv` and `conformance/pliron-capability.tsv`
manifests). Every stage is removable by disabling its optional feature and
backend modules without changing MIR, VM, or source semantics. The default
response to a correctness failure is to disable the offending optimization or
native path until parity is restored. Upstream or distribution risk can justify
the Cranelift alternate recorded in `docs/roadmap.md`, never erosion of the
MIR/VM contracts.

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
  `declarations`, `generics`, and `places`; `conformance`,
  `overload_support`, and `traits_support` hold the extracted conformance
  oracle and free helpers, and `method_calls`/`origins` are themselves
  directory modules split by responsibility. See `docs/symbol-map.md` for
  the per-file responsibility map.
- `mir/mod.rs` lowers ordinary code; `mir/ir.rs` defines the MIR data model and
  `mir/nested.rs` owns capture analysis and nested-function lifting.
- `backend/vm.rs` drives execution; `backend/vm/calls.rs` owns runtime argument
  binding and construction, while `backend/vm/places.rs` owns projected storage
  navigation and access; further `impl VmBackend` clusters live in
  `backend/vm/{values,adapters,invoke,dispatch}.rs`.
- `comptime.rs` runs elaboration and specialization;
  `comptime/rewrite.rs` owns AST substitution and value materialization, and
  the root's extracted clusters live in
  `comptime/{elab,synth,ctfe_calls,packs,params,simd_width}.rs`.

Child modules expose only the phase-internal operations their coordinator needs.
The public entry points for each compiler phase remain in its root module.

## Stage 1: Module Linking

Module:

```rust
crates/mojito-module/src/module.rs
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
crates/mojito-comptime/src/comptime.rs
```

Entry points:

```rust
comptime::prepare(program: Vec<Stmt>) -> Result<Vec<Stmt>, ComptimeError>
checker::validate_comptime_templates(prepared: &[Stmt]) -> Result<(), TypeError>
checker::validate_comptime_templates_into(prepared: &[Stmt], catalog: &mut TemplateCatalog) -> Result<(), TypeError>
comptime::elaborate_prepared(prepared: Vec<Stmt>, …requests) -> Result<Elaborated, ComptimeError>
comptime::elaborate(program: Vec<Stmt>) -> Result<Vec<Stmt>, ComptimeError>
```

`comptime` is implemented as a phase distinction before type checking. The
elaborator rewrites compile-time constructs into ordinary AST so the checker,
HIR, MIR, and VM do not need to carry special `comptime if` or `comptime for`
semantics.

**Source validation comes first.** `prepare` normalizes declarations
without selecting an arm, unrolling a loop, stubbing a template, or minting a
clone (pack qualification, the synthesized `copy`/`__hash__` methods, the
SIMD-keyed method desugar, SIMD alias-bound folding). The checker then
validates the prepared source (`validate_comptime_templates`,
`checker/comptime_validation.rs`): every function or method body holding a
`comptime if`/`comptime for`, or a `rebind` over its own parameters, is
checked once with its declaration's parameters left symbolic — each condition typed as a compile-time `Bool`
(a generic constraint over the parameters in scope, a concrete conformance,
or a `Bool` value), each arm and loop body in its own scope, no arm assumed
selected — so a type error in an untaken arm rejects as upstream rejects it,
a guard such as `T == Int` narrows nothing, and an unused template still
checks. A `rebind` takes its target on faith here, where the operand's type
is still symbolic, exactly as upstream does; the equality is asserted on each
clone. Bodies without such constructs are declared but left to the
executable check; a struct keyed on a struct-typed value parameter registers
as a template shell and keeps its per-instantiation check. Validation then runs the
explicit-destruction analysis over exactly those bodies
(`explicit_destroy::check` with `DestroyScope::ValidatedTemplates`): a
compile-time-keyed template is a trapping stub by the time the executable
check sees it, so this is the only place its abandoned values are judged with
the parameters symbolic. An arm whose condition names one of the body's
parameters may assume nothing about the instantiation, so such arms join as
`if` branches do: a value one arm alone destroys is abandoned, or
uninitialized if used afterwards, even when every instantiation selects that
arm, as upstream (`explicit_destroy::check_comptime_if`,
`assets/type_error/comptime_if_arm_conditional_destroy.mojo`). A condition
naming no parameter folds to one arm, and its arms join as alternatives. A
`comptime for` body must leave every outer obligation as it found it.

**A variadic pack is symbolic too.** A body keyed on a pack — a `def`'s or
method's own `*Ts`, or any method of a variadic struct — is validated like any
other, whether or not it holds a `comptime` construct. The element at a
compile-time index (`args[i]`, `self.storage[i]`, `Self.Ts[i]`, a constant
index included) has the dependent type `param_list.get(Ts, i)`
(`ParamKind::ListGet` behind `DependentType::Parameter`; a `comptime for`
variable is the index's binder). That type is the element's identity: it
substitutes when `Tuple.__getitem_param__[index]` is applied at `i`, renames
when one struct's pack is forwarded as another's (`Tuple[*Self.Ts]`), and
folds to the element once the pack binds to a list
(`Checker::close_pack_elements`). Its capabilities are read through a bounded
`Ty::Param` view (`Checker::opaque_element`): the pack's declared bounds plus
every trait a conjunctive `conforms_to(Ts.values, Trait)` or
`Ts.all_conforms_to[Trait]()` of the enclosing declarations guarantees, and
nothing else — a struct-header conditional conformance, a disjunction, and a
`comptime if` guard license nothing, as at the pin. Storage over an unbound
pack is a list that spreads it (`Ty::Tuple`, `Ty::Variant`, or a `Tuple[...]`
application whose one argument is the starred `Ty::Param`,
`types::pack_spread`); a spread beside other arguments is rejected. A
`TypeList` proposition over the pack is a `Bool` no instantiation has fixed,
so both arms of `comptime if Self.Ts.contains[T]()` check, and
`__VariantStorage` operations type from `T` without deciding membership. A
variadic struct applied to concrete types inside a validated body
(`Tuple(1, "one")`, `a == b`, `t[0]`) resolves through the same signatures
with the pack bound to the element list. An explicit construction of a user
variadic struct there (`Pair[Int, Bool](1, True)`, `Bag[T, String](x^, "tail")`)
is matched against the template's constructor with the pack bound from its
`[...]` arguments (`infer_construction`), the path a bare construction takes
once its pack is solved: a `*args: *Self.Ts` collector must take exactly the
bound elements, and the fieldwise constructor its one tuple. The public
`Tuple` alone types as the tuple it spells — bare as its display, explicit
against its element list (`infer_validated_variadic_construction`) — because
its identity is the element-by-element nominal spelling, not the template's
bound pack. A call that forwards the pack whole
(`inner(*a)`, `collect(30, *items^, tail=10)`, `self.take(*a)`, `print(*a)`)
binds the callee's own pack to the caller's (`forwarded_pack_argument`,
`bind_forwarded_pack`): the spread is the last positional argument and lands
alone in a collector that is itself a pack (`call.rs:spread_position`,
`bind_spread`), every bound the callee's pack declares must hold of the
caller's, the two collectors' ownership must agree (`var` needs the `^`, a
read pack cannot be transferred), and a result naming the callee's elements
closes over the caller's pack. Where there is still no symbolic rule — a
method other than `__len__` on the pack, a spread outside a call argument —
the checker raises `TypeError::SymbolicBoundary`, and that one body gets
no verdict (`Checker::symbolic_verdict`, counted as `templates.no_verdict`):
it keeps its per-instantiation check, leaves the destruction walk, and the
run goes on. `docs/notes/param-expr-attributes.md` records the design.

**A SIMD lane is symbolic too.** A body keyed on a `DType` parameter, or on a
vector width naming its own parameter (`def total[dt: DType, width:
Int](v: SIMD[dt, width])`, a struct field `SIMD[Self.dt, Self.width]`), is
validated like any other. `Ty::Simd` holds typed slots (`SimdDtype`,
`SimdWidth`): a slot is a known dtype or width, or the parameter expression
the template names (`dtype_from_arg`, `simd_width` in the checker), and the
one constructor of a symbolic vector type (`types::simd_ty_from_slots`) folds
a closed slot to its constant and two known slots to the canonical type, so
slot equality is type identity in the pin's normal form (`SIMD[dt, n + 1]`
is `SIMD[dt, 1 + n]`; `v.join(v)` is `SIMD[dt, 2 * width]`; a guard narrows
nothing). A symbolic lane's capabilities are the SIMD surface itself, not a
trait bound: every dtype-gated operator and method is licensed through
`SimdDtype::licenses` (the constraint is the instantiation's to check, as
upstream defers `constrained[...]`), an integer or float literal splats into
it where a concrete scalar does not (`types::splats_to`), and a lane fact the
checker records for lowering (`SimdLength`, `DtypeConstant`, a cast or
shuffle adjustment) is recorded only when the slot is known — such a body
keeps its clone check. Applying a `DType`-keyed struct in a validated body
closes its `Scalar[Self.dt]` members under the application's values, to a
concrete lane or to the caller's own symbolic `dt`. A symbolic slot never
crosses the MIR waist: `validate_dependent_bindings` refuses it, the
elaborator still clones every such body per call or per application, and
lowering reads only known slots.

**A reflected field is symbolic too.** A body reading `reflect[T]` over a
parameter — a `def`'s type parameter, or `Self` in a generic struct's method —
is validated like any other (`checker/reflection.rs`). A query over a
registered struct, at concrete or symbolic arguments, is answered from the
struct table as the elaborator answers it; a query over a subject that is
still a parameter is a `ParamKind::Reflect` node: `field_count()`,
`field_index[name]()`, and `len(field_names())` are compile-time `Int`s,
`is_struct()` a `Bool`, `field_names()[i]` a string literal, and a field
type — `types[i]`, `r.field_at[i].T`, `r.field["x"].T` — the dependent
element `ListGet` of a `field_types()` list, viewed through the same bounded
`Ty::Param` a pack element is (`opaque_element`, now keyed by the element's
spelling). A field type has no declared bound: `T`'s own bound says nothing
about a field, so a trait use of it is rejected until proved, and a
`comptime if conforms_to(X, A & B)` condition proves exactly those traits of
exactly that operand for the arm it guards (`conformance_arm_assumptions`,
pushed on `assumed_conformances` around the arm), which is what the pin
licenses for a plain parameter, a pack element, and a field type alike; a
proof on another index, after the use, in another loop, or in an `or` proves
nothing. A reflection body's certificate is incomplete by rule
(`template_certificate`), so its instances keep the clone check and the
elaborator's own field facts. A `Reflect` node never crosses the MIR waist.

**Validation is a template producer.** `Compiler::compile_linked` runs
validation once, on the prepared program that every discovery round then
re-elaborates, and lends it the compilation's `TemplateCatalog`
(`validate_comptime_templates_into`). A module-level body validation checks
is retained there as a `CheckedTemplate`: its facts keyed by its own syntax
occurrences, with a coverage certificate. The elaborator stubs such a
template, so validation is the only check it gets, and its instances inherit
the facts of the arms the elaborator selects instead of being inferred
(Stage 3, *Checked Templates*). A run that ends without a verdict — it
reached a member of a struct-value-keyed template shell — withdraws every
certificate it produced. A pack-, `DType`-, or vector-keyed body's
certificate is always incomplete, and so is a reflection-reading body's, so
their instances keep the clone check. The verdict-only
`validate_comptime_templates` remains for clients without a catalog, and
`comptime::elaborate`, the composed-stage seam, runs the same three steps
through it.

Compile-time values are represented by:

```rust
CtValue::Int
CtValue::Bool
CtValue::Str
CtValue::Tuple
CtValue::List
CtValue::Type
CtValue::Expr      // a residual parameter expression, never a constant
CtValue::Deferred  // a slot whose value arrives later; not part of identity
```

### Parameter Expressions

A value argument such as the `n + 1` of `Buf[n + 1]` is a typed, immutable,
canonical node: `mojito_types::param_expr::ParamExpr`, built only by the
canonicalizing constructors of a per-compilation `ParamContext`. Two
expressions the front end can prove equal are the same node, so type equality
over value arguments is decided before any value is supplied. A value
parameter's default, a conditional callable default's condition, and a
dependent type (`DependentType::Parameter`) are the same nodes; there is no
second expression tree.

- The context rides in `TemplateCatalog`, so source validation and every
  discovery and transfer round of one compilation share it. Pure helpers in
  `mojito-types` canonicalize on a detached context; equality is structural
  across contexts.
- The normal form is no stronger than the pinned Mojo's: integer `+`, binary
  `-`, and `*` are a sum of products, a left shift by a constant is a
  multiplication, and unary `-`, `//`, `%`, and `**` are opaque atoms.
- Replacement (type identity) re-folds the polynomial and leaves an opaque
  atom unfolded; evaluation (a default, a dependent index, native
  monomorphization) folds it. The pin draws the same line.
- A reference is owned by its declaration (`ParamId { owner, slot }`), so
  same-spelled parameters of unrelated declarations differ and a `$` clone
  shares its template's. An overloaded module-level `def` is owned by the
  signature-qualified symbol MIR names it by (`tally$ov$…`), so two overloads
  of one name never share a binder. A type binder is the same identity: `ParamDecl`
  carries its `id`, `Ty::Param { binder: ParamRef }` compares by it, and every
  type substitution is a `TySubst` keyed by it; a canonicalized callable
  contract's binders are `$contract` slots, so contracts differing only in
  spelling are one identity and a trait witness may spell its binder as it
  likes.
- `param_expr::fold` is the one implementation of compile-time scalar
  operators; the checker, the elaborator, and native monomorphization call it.

`docs/notes/param-expr-attributes.md` is the design record, with the pin
evidence behind each rule.

The implemented forms are:

- `comptime NAME = expr`: evaluates `expr` immediately, records the result in
  the compile-time environment, and keeps a folded declaration whose value is a
  literal.
- `comptime if`: evaluates each condition as a compile-time `Bool` and keeps
  only the selected branch. The dropped branches were already checked by
  source validation, so elimination hides no type error; a dropped branch's
  compile-time evaluation failure (a `comptime k = 1 // 0` there) is not
  an error, as upstream.
- `comptime for`: evaluates the iterable as either `range(...)` or a
  compile-time tuple/list, substitutes the loop variable with a literal, and
  splices a fresh elaborated copy of the loop body for each element. A
  zero-step range has no elements in both this direct evaluator and the
  VM-backed CTFE path; the shared loop predicate simply never enters.
- CTFE calls: a compile-time expression may call a pure top-level `def`,
  including value-parameterized helpers and helpers whose type parameters are
  used only for compile-time facts. The elaborator clones the needed helper call
  graph, folds compile-time-only operations such as a type comparison
  (`T == U`) and `T.size` out of the cloned bodies, and executes the resulting helper through
  HIR, MIR, and the register VM in compile-time mode.
- Materialization: module-level `comptime` constants are inlined as runtime
  literals into later code, so a function can use a constant computed at module
  elaboration time.

Generic `def` templates monomorphize in two classes sharing one worklist,
mangling, and clone generator. A **comptime-class** template (a `comptime
if`/`for` body, a `rebind` over the declaration's own parameters, or a type
pack) must specialize at every reference: resolution
failure is an error, and the template is replaced by its clones (a dead
template is dropped unchecked). A `rebind` keys the class because it asserts
that a parametric operand type resolves to its target, which only an
instantiation can settle; a generic struct's method holding one is stubbed on
the template for the same reason, and the assertion is made on every clone.
Three comptime-class subsets take an inferred
call through discovery instead: a type pack whose element types are not
statically evident, a **compile-time-keyed** `def`
specialized only for its `comptime if`/`for` body or `rebind` (no pack,
`DType`, or SIMD-width parameter; `comptime_generic_template_names`) called
without an argument for a required parameter, and a **`DType`-keyed** `def`
(`dtype_generic_template_names`) whose call omits only its lane: the lane is
the argument's own, which the checker reads off a `Scalar[dt]` slot and the
elaborator cannot. A call that omits a SIMD width is not in that subset —
the pin does not infer one either. An overloaded name with a
compile-time-keyed or type-pack declaration among its overloads is a *family*
(`collect_overload_families`): no call to it is
ever resolved syntactically, since explicit `[...]` arguments name type
arguments rather than an overload and overload selection is the checker's.
Every call is served from the checker's recorded instantiation, which names
the selected overload by its runtime parameter names, and where two overloads
share those by their mangled parameter types. A variadic parameter is
caller-visible but is spelled by neither key, so where two share both the
request's `symbol::VariadicKey` tells them apart — the `*args` collector's
position among the regular parameters and its element key, which carries a
pack's bounds — and last whether the request's own arguments bind the
declaration's parameters at all. Two type-pack overloads of one name are
therefore ordinary members of a family, never resolved against the first
declaration. One call has no recorded instantiation to serve it: a clone
forwarding its own specialized pack whole to a sibling (`tally(*rest)`),
which the checker sees only once the spread is expanded.
`forwarded_family_target` binds it to the one declaration whose positional
collector the spread follows, with the whole-pack ABI; where several would
bind it, the call stays on the abstract path.

A family may mix specialization classes, because the class is a property of a
*declaration* and not of a name: a keyed `def kind[T](a: T)` beside a type
pack `def kind[*Ts](*xs: *Ts)` is two templates of two classes at one name,
and the request's selected declaration index is the only thing that tells a
call which class serves it. The registries above stay name-keyed only as the
question "is this name a template at all"; admission, routing, and the
program rebuild's choice of stub all decide per declaration.

Two declarations specialized at
the same values mangle to one clone name and become an ordinary overload set
there, which `$ov$` qualifies at lowering exactly as it qualifies the sources;
a plain overload sharing the family's name is not a template and survives the
rebuild unchanged. Until the checker's request is served,
such a template survives as a signature-only `template_stub` whose body
traps, so the discovery check can type the call against it. The stub stays
in the program for a call from a retained bound-generic body over that body's
own parameters (`show(x)` or `show[T](x)` in `def forward[T]`), and that call
lowers to a call of the stub. It never runs. A generic struct's
erased method body is such a body too (`Box[T].f` calling `show(self.x)`): it
runs only on the paths listed below, so the references it leaves are owned by
the method (`Struct.method`) rather than unserved. The elaborator lists
every reference it leaves on an abstract path
(`Elaborated::unserved_template_uses`) that can reach a stub: a reference to a
compile-time-keyed template, or to a bound-generic `def` or struct method
whose own body reaches one, transitively — through a call it left abstract, or
through a by-name method edge, since the receiver's type is the checker's to
solve. A reference made inside such a body is not listed, because that body
runs only through a listed reference. At the fixpoint
`reject_unserved_template_calls` rejects a listed call the checker still
records against its template, and any listed function-value use, so no
accepted program reaches the trap.

Such a method is served, or rejected, on the paths its erased body can run. A
closed receiver reaches the method's clone, and construction, copying, moving
and destruction reach the instance's lifecycle clones. An instance the
elaborator queued but could not clone the method for — a bound violation, an
argument with no source spelling, a clone whose own walk fails — reports that
method's references as unserved (`Mono::unclonable_methods`); an instance that
merely withholds the method, because a `where` clause or a conditional
conformance makes it unavailable there, does not, since no call can reach that
body either. An instance the fixpoint discovers at the round cap reports
`SpecializationDivergence` rather than converging on the erased path
(`Elaborated::stub_reaching_structs`). A bundled struct's method, and a struct
whose parameters mint no clones at all (value, callable-bounded, or origin
binders), keep no owner, so their references stay unserved as before. What is
left is the erased dispatch no checked static type reaches — an operator or
protocol dunder called from another erased body, an instance the checker
records no instantiation for (a type parameter inferred as the compile-time
`StringLiteral`), and CTFE, which runs before any clone exists — where such a
method still traps at run time (`docs/roadmap.md`).

A **bound-generic** template — a plain
trait-bound generic `def` with no comptime constructs and a unique top-level
name — resolves softly: only an explicit application whose arguments resolve
concretely monomorphizes, while inferred calls, symbolic arguments, and
function-value uses stay on the template's abstract erased-dispatch path. The
template itself always survives the rebuild, instantiated or not: what a
parametric body demands of its own parameters is a fact about the template,
so its Mojo-style abstract pre-check must not depend on which arguments some
call happened to supply. In both classes each clone bakes
its concrete type arguments into every remaining type position — annotations,
compile-time argument lists, and constructor heads — and drops them from the
residual signature and the rewritten calls, so the clone checks concretely.
Because the checker never re-validates a dropped argument, the resolver
enforces each dropped parameter's declared trait bounds at the requesting call
through the conformance oracle. Type packs, callable-value bindings, and
types that do not round-trip to source syntax remain symbolic on the residual
signature. A specialization is walked whole — signature and body — for further
template uses, so a clone's expanded `-> Variant[Int, String]` requests that
concrete struct exactly as its body's calls do.

Per-call clones of a method with its own compile-time parameters share one
minting path (`per_call_method_clones`): the elaborator's struct walk mints
them for a named owner and `generate_instance_clones` for an instance
(instance values first, then the call's), a pack binding expands `*args:
*Ts` to the `$pack[...]` element list inside `specialize_method_clone`
exactly as a def specialization does, and the template's method body
becomes the `unspecialized_method_stub` trap when it only elaborates with
the struct's or its own parameters bound. `specialize_method_clone` bakes
the clone's *value* bindings into its signature types before its type
bindings, as a def specialization does, so a value parameter standing in a
type position (`a: Scalar[dt]`, `-> SIMD[DType.int32, w]`) spells its bound
value on a clone that no longer declares the binder. In the checker,
`instantiate_method_generics` resolves an inferred pack's variadic element to
the heterogeneous `RuntimePack` so each overflow argument scores and converts
against its own element, and the call retargets through
`instance_call_method_clone` / `specialized_method_clone` by the composed
name. An instance call records that name as its target; a static call,
whose syntax still names the template, records it through
`record_static_clone_target`. Every clone's body gets its own source tag
after the uniform module stamp, so span-keyed facts (including the
requests of calls inside it) stay separate across a template's clones: an
instance clone is found by its `self_ty`, a clone on a named owner through
`Elab::per_call_clones`.

Temporaries borrow for their statement through three anchors. A temporary
receiver of a `ref[self]`-returning method is materialized like a temporary
bound to a `ref` parameter (`materialized_reference_actual`; when the
expression already carries `BorrowRefArguments` or a view result's
`BorrowViewResult`, the owner rides on that adjustment's `materialized` slot,
and `materialized_borrow_owner` is the one reader), and `lower_call_receiver`
stores it in its `$mat_r` slot so the result handle roots at real frame
storage. The `$mat_r` slot establishes the temporary's own loans
(`aggregate_borrows_unmaterialized`, the loan funnel without the slot's
self-loan), so storage a materialized view borrows outlives every borrower of
the slot (`it.peek_next().value()`). The slot is lent with the capability the
borrow that forced the materialization needs, which
`MaterializeBorrowSource` records and `materialized_borrow_mutability` reads:
materializing changes where the storage lives, never what the loan permits,
so a read receiver's two views coexist exactly as they would over a named
local. A subscript view temporary
(`MultiIndex`/`Slice`/`Index` with a `BorrowViewResult`) in any argument
list anchors in `$arg_loan_r` like a loan-carrying call temporary in a plain
call — no consumer channel retains it, so the anchor never duplicates a
loan. The builtin `String(view)` conversion lets its arguments anchor like a
plain call, because its result is a fresh owned string carrying no loans, and
a call temporary at a place-retaining argument slot, which has no place to
retain, anchors through the same `anchor_temporary_argument` decision. A
binding's argument anchors stay live through its store, so assigning a call
straight back to the local a temporary view argument borrows
(`head = f(head[byte=1:4])`) is an ownership conflict, as upstream rejects it.
Upstream keys that rejection on the argument's owned-interior origin rather
than on temporary lifetime, so it also rejects a named view and accepts a
plain-origin one (`s = f(StringSpan(s))`); the roadmap tracks both gaps. A tuple unpack
gives every loan-carrying element (`CheckedTupleUnpackElement::carries_loans`)
the unpacked value's loans, as binding the whole value would. A discarded
reference result (an expression statement, `_ = e`) is
recorded in `discarded_reference_results` and is not a value read. On the
VM, `Writer.write` formats its arguments with the caller frame mirrored so a
nested `write_to` can read a pointer into the caller. The empty-subscript
store `p[] = v` types the pointer as a value read and gates on the pointer
origin's mutability alone, so a plain `self` method writes through a
`Pointer[T, Origin[mut=True]]` field as upstream does.

A variadic struct applied over an enclosing declaration's own type parameters
(`Variant[T, String]` in `def wrap[T]`, `Variant[*Ts]` in `def f[*Ts]`, a
`Variant[*Self.Ts]` field of `struct Outer[*Ts]`) cannot specialize eagerly.
`Mono::symbolic_type_params` tracks the parameters of the declarations being
walked; `resolve_struct_spec_args_if_ready` leaves such an application
symbolic (as the public `Tuple` always was) and retains the template. The
program rebuild then emits the template as a **shell**
(`StmtKind::Struct::template_shell`): its parameters, declared conformances
(taken unconditionally — the concrete specialization re-verifies them), the
method signatures that resolve symbolically, and — for a variadic template,
whose members resolve over its pack — its fields and associated members
when they all resolve, register in the checker (`check_struct_shell` skips
the variadic-template rejection, `register_struct_method_signatures` skips a
signature that fails, `check_struct_types` installs a variadic shell's
members all-or-nothing, and the completion phase is skipped), so the
retained abstract body checks against it, its constructions included;
`explicit_destroy` and MIR skip a shell entirely. A concrete application that
fails to resolve keeps its eager diagnostic, and the raw seam still rejects
an unmarked variadic template.

A bare construction of a single-pack template (`Pair((1, True))`,
`Bag(7, "x")`) also survives elaboration with the template retained as a
shell (`mono_expr`, `Elab::single_pack_template`). The discovery check types
it against the shell's constructor and solves the pack there
(`solve_value_args` binds a spread `Tuple[*Self.Ts]` to the display's
element list; the `*args: *Self.Ts` collector's elements collect as a
method-level pack does), rejects a constructor naming the pack nowhere
(`Checker::reject_unconstrained_pack`), records the pack as a
`GenericInstantiation` on the struct's own name at the call occurrence and
— once the specialization is declared — types the call as that concrete
struct's construction (`finish_variadic_construction`). `Compiler::compile_linked` turns the
recording into a constructor-rewrite request (`variadic_struct_requests`,
the scalar-range shape), which `seed_def_call_targets` files under
`Mono::struct_call_targets` and `mono_expr` serves by rewriting the call to
`mangle(template, [pack])` — the same symbol an explicit application mints.

Inferred applications reach the same clones through the compiler's discovery
fixpoint. Each round's check stops at a `DiscoveryResult` (Stage 3): the
requests below are read from it, every round but the converged one is
discarded without its checked arena being built, and a clone the round's
`DefInstanceTrace`s tie to a certified template takes its facts from the
compilation's `TemplateCatalog` rather than being inferred; a pack-keyed
clone's trace carries the pack's element types, and each unrolled copy of
its body fixes its element from the loop index the elaborator folded there.
`Compiler::compile_linked` iterates elaborate→check, deriving
`DefSpecializationRequest`s from the checker's recorded generic
instantiations (a bound, pack, compile-time, or `DType`-keyed `def`; a scalar
`range` family; a bare variadic-struct construction — the last two are
constructor rewrites on a struct template) and `MethodSpecializationRequest`s from its recorded generic
*method* instantiations — on a specialized variadic struct, on a closed
instance of an ordinary generic struct (the request owner is the instance
key `mangle(template, arguments)` and the instance's arguments bake before
the call's: `kind$y3:Int$y4:Bool`), on a user-declared struct, or from any
call site outside the unstamped bundled stdlib (user code and `$`-stamped
clone bodies: `List[Int].write_repr_to` reaching `FormatStruct.params`),
never for a variadic template's own shell name (its round-1 recording would
only conflict with the specialization's); a method-level pack inferred from
the overflow arguments is a closed `CtValue::Tuple` of materialized element
types (closed arguments only, keyed by occurrence span with the phase-local
syntax id stripped) and
re-elaborating the original linked program with the accumulated monotone
request set until a round discovers nothing new; a hard round cap reports inferred polymorphic recursion as a
dedicated divergence diagnostic. Request seeding records each occurrence's
target without queuing work; the clone job queues lazily when the soft
resolution path fails on source arguments and consults the request — so a
request can only upgrade a call from the abstract path, and a drifted or
conflicting request (a `comptime for` unrolling duplicates one source
occurrence) leaves the call abstract. A bound-generic template is
emitted before its specializations because a clone may still reference the
template abstractly (an inferred recursive call) and the checker binds
top-level names sequentially.

Methods with their own compile-time parameters inside a *specialized
variadic struct* (`def isa[T: AnyType](self)`, `def unwrap[T: Movable](deinit
self) -> T`, a constructor `__init__[T: AnyType, //, F: def() -> T](out self,
*, init_with: F)`) specialize per call through the same fixpoint. The
checker's method paths record a `MethodInstantiation` (receiver struct,
method, declaration-order arguments; an infer-only `T` also solves through a
callable-bounded sibling's contract) and, once the variadic-struct
specializer has minted the clone for that instantiation — `isa$y3:Int`, the
method name mangled with the baked type/value arguments, callable-bounded
parameters folded into concrete `capturing[_]` contracts — retarget the
call to it by exact name (`specialized_method_clone`), or for constructors
select it as a concrete same-name overload that wins over the generic
template. The template itself survives as a declaration; a template body
that only elaborates with its parameters bound (a `comptime if` on `T`)
becomes an `_mojito_abort` stub that no specialized call reaches. A call
whose arguments stay symbolic (inside an unspecialized generic body) keeps
the erased path and therefore cannot use such a method.

An *ordinary* generic struct (`struct Optional[T: AnyType]`, every parameter
a plain type parameter) keeps its template in the program — checked once
with `T` as `Ty::Param` (Mojo's generic pre-check) and still the fallback for
symbolic receivers — and gains **per-instantiation method clones** appended
to its own method list. The checker records every closed application it
reaches from a non-bundled source as a constructor target or method-call
receiver (`StructInstantiation`; instances seen only inside unstamped
bundled stdlib bodies keep the erased path, so a program without its own
instantiations mints nothing); the driver replays them as
`StructInstanceRequest`s, and the specializer also mints, within one
elaboration, every closed application it meets while walking user code and
the clones themselves (annotations, constructor calls, static receivers, and
the instance's substituted field types), reporting that minted set back so
the checker's recordings of it are not new discoveries. A clone
(`get$y3:Int`, the method name mangled with the baked struct arguments, one
per same-name overload) has every `T`/`Self.T` respelled concretely, its
`comptime if` folded, and an explicit receiver type (`Method::self_ty`,
`self: Optional[Int]`) the checker binds `self`/`Self` to instead of the
struct scope's parametric `Self` (`AnnotationSite::MethodSelf` types the MIR
receiver); a method whose `where` clause is false — or unevaluable — for the
instance, a requirement of a trait whose conditional conformance is false
for the instance (`Iterator where conforms_to(T, Movable)` withholds
`__next__` from `_ListOwnedIter[Pinned]`: Mojo instantiates it only through
the conformance), and an overload family that collapses to one shape on the
instance stay uncloned. A user struct's lifecycle methods clone like any
other, under the name they are registered and dispatched by — a copy
constructor as `__copyinit__$y3:Int` — and the whole pipeline recovers the
source name through `symbol::instance_clone_base` at the gates that test for
one (`out self`, `@implicit`, the copy/move shapes, the named-destructor
list). A construction retargets in the checker like any other call, but by
signature rather than by name, because a constructor family's clones do not
share the template's `$ov$` symbols: `constructor_clone_target` takes the
member whose signature is the selected template signature with the instance's
arguments substituted and records its lowered name, for a written
construction and for an `@implicit` conversion alike. A family therefore
clones as one overload set (`__init__$y3:Int$ov$Int` beside
`__init__$y3:Int$ov$Int$Bool`), minted whole or not at all, and a clone keeps
the struct's parameter list as its compile-time interface, as the template
does. Copying, moving and destruction retarget nowhere: the VM selects the
instance's `__copyinit__`/`__moveinit__`/`__deinit__` clone from the checked
static type of the value being copied, moved or destroyed
(`Prog::lifecycle_symbol`, with `instance_field_types` carrying the
substituted field types into a whole-value drop or copy), and the native
monomorphizer emits such a clone under the instance's plain lifecycle symbol
(`lifecycle_clone_instance_symbol`), which is what the Pliron lowering
composes by name — a signature-qualified constructor clone keeps its own
symbol there, since its siblings answer to the same base name. A bundled
template's constructors stay on the erased path (its `__deinit__` clones,
which the elaborator already minted, are now reached like any other). An
instantiation whose argument mentions `StringLiteral` mints no clones and
names none (`instance_method_clone_name`; a method-level type argument
inferred from a string literal still materializes `String`, as the call
argument does): its values keep the literal runtime
representation while an un-annotated binding of the type materializes
`String`, so a clone body would neither type against its own `Self.T` nor
share values with the nominal-`String` instance. Type identity is
unchanged (`Optional[Int]` is still `Ty::Struct("Optional", [Int])` and the
runtime struct name stays `Optional`), so a call whose receiver instance has
no clone simply keeps today's erased path; a call on a closed receiver
retargets to the clone by exact name through the `specialized_method_clone`
funnel (`instance_method_clone`): method and static calls, operator dunders
(re-selected among the clone family by operand), subscript assignment, and
`for` iteration (`__iter__` by receiver convention) all record the clone
symbol. The by-name dispatches the backends perform on a runtime struct
name — `print`/`String(x)`/`repr(x)`/`Writer.write` through
`write_to`/`write_repr_to`, and `len`/`abs`/`Bool`/`Int` and the prefix
operators through their dunders — select the clone from the argument's
*checked static type* instead (the VM's `format_value`/`call_typed_dunder`
read the caller's register types; the native monomorphizer's
`enqueue_display_instance`/`instance_dunder_target` read `reg_types`), all
through the symbol crate's one identity helper `instance_method_clone_name`;
with no clone the runtime-name path serves the call exactly as before. An
inferred application of a bound-generic `def` whose clone already exists
(`existing_def_clone`: `mangle(name, values)` declared as a concrete
function) retargets to it directly and records no request, so a clone body
calling an already-cloned def costs no discovery round. A clone that fails
to check reports `TypeError::PostInstantiation` naming the instance and the
source method. A
template method whose body only elaborates with the struct's parameters
bound becomes an `_mojito_abort` stub, exactly as in a variadic
specialization.

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
remaining expression evaluator in `crates/mojito-comptime/src/comptime.rs` is not a second function
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

for an instantiation such as `capacity[Buffer[8]]()`. Similarly, a type
comparison `T == Int` is replaced with a `Bool` literal. After this rewrite,
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

- `crates/mojito-comptime/src/comptime.rs` handles language-level `comptime` declarations, branch
  selection, loop unrolling, materialization, and CTFE.
- `crates/mojito-checker/src/checker.rs` still validates type/value-parameter positions and folds the
  small expression subset it needs for those positions.

`ParamDecl::Value` retains its declared checked type and optional compile-time
default, a `ParamExpr`. The shared `CtValue` model carries integers, booleans,
strings, tuples/lists, types, residual parameter expressions, and zero-sized
reflection handles. Both mechanisms fold through `param_expr::fold`.
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
checker::check_program_for_discovery(program, materialized_callables, catalog) -> Result<DiscoveryResult, TypeError>
checker::check_program_carrying(program, materialized_callables, catalog, previous: Option<PassCarry>) -> Result<PassCarry, TypeError>
checker::check_program_with_templates(program, materialized_callables, catalog) -> Result<CheckedProgram, TypeError>
```

`check` is the compatibility validation wrapper, and `check_program` the
whole-handoff entry the stage-composed seam, CTFE, and direct clients use.
The compiler driver calls `check_program_for_discovery`, which runs every
rejecting check — the transfer-effect fixpoint, reference-result reads,
context-manager splicing, explicit destruction — and stops before the checked
arena is assembled. A `DiscoveryResult` is `CheckedProgram::new`'s inputs,
owned. The driver's request collectors read it directly, and the two that
need every expression's types use `DiscoveryResult::scan_expressions`, which
is the arena builder's own traversal with node construction switched off, so
they see exactly the expressions the arena would hold. Only the round that
converges is finalized (`DiscoveryResult::finalize`). A `DiscoveryResult` is
not executable and never reaches HIR or MIR.

The driver actually calls `check_program_carrying`, which returns the
`DiscoveryResult` inside a `PassCarry`: the pass's fact stores, one record
per body site, and its binding-identity watermark. Every fact store the
checker writes during a body is a logged store (`mojito-checked`'s
`fact_store.rs`: a map, set, or vector plus the log of keys written), so a
site — a module-level `def` or a struct method — records the log ranges it
spans, the effect entries it read exactly as read, and a hash of its syntax
(`checker/body_carry.rs`). The next pass, whether a transfer pass over the
same tree or the first pass of the next discovery round, starts from the
previous pass's committed effect maps and, at a site whose record is clean
and whose reads still equal the committed entries, copies the logged entries
instead of inferring. The driver marks a record dirty before the next round
when a request the body recorded was served (`compiler.rs:ServedRequests`),
and a rewritten call changes the syntax hash. Binding identities never
collide: a pass starts its counter at the previous watermark, and a body
inferred again allocates inside the range it had (`scopes.rs:reserve_owners`)
so an unchanged prefix keeps its identities. The copy is exact by
construction — nothing outside the body window is skipped — and
`tests/compiler_test.rs` pins that a compilation with carry-over lowers the
same MIR as one without (`MOJITO_BODY_FACT_REUSE=0`), up to identity
numbering. The catalog's template derivation is not consulted for a carried
body; verification mode (`MOJITO_VERIFY_TEMPLATE_FACTS`) disables carry-over.

The checked handoff owns the elaborated AST for diagnostics and
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
synthesized entry against the checked CTFE subprogram; that subprogram
carries variadic templates other than the public `Tuple`/`TString` as the
shells pre-check elaboration emits, so a retained body's symbolic
application checks, and stubs a struct method whose comptime body only
elaborates with its own parameters bound, as the pre-check does). Both monomorphize
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
unrolling: an unrolled `comptime for` copy or a selected `comptime if` arm
that declares a binding is an elaborator-emitted `StmtKind::Scope`, a straight-line block every phase treats as
a lexical scope, and a template derivation counts each copy of a declaration as a
local of its own (`renumber_locals`). HIR may suffix the internal slot name, but downstream place lookup is
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
tuple expression. `Ty::Variant(Vec<Ty>)` is the compiler-private tagged-union
storage of the bundled `__VariantStorage[*Ts]` field behind `std.utils.Variant`
(gated on `bundled_stdlib_declaration`, never the type of a public expression);
it retains alternative order at the checked boundary. Storage construction and
the parameterized `isa[T]`, projection, and `set[T]` operations record the
selected alternative as a `SemanticAdjustment`; MIR therefore receives a
numeric tag and never guesses one from source spelling. The public `Variant`
API, its conditional conformances, and its `__eq__`/`__hash__`/`write_to`/
`write_repr_to` bodies are Mojo in `stdlib/std/utils/variant.mojo`, specialized
per instantiation and per type-keyed call like any variadic struct. `Value::Variant` repeats the checked
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

A struct's own origin arguments are part of its checked identity: `Ty::Struct`
carries its declared type and value arguments first and then an **origin tail**,
one `TyArg::Origin` per explicit `Origin`/`OriginSet` source parameter in source
order (`P[origin_of(xs)]` and `P[origin_of(ys)]` are different types, as
upstream). `Origin::Unbound` marks a slot the use site infers (a bare or `_`
parameter or initialized-local annotation, an alias body); `Origin::Param` in a
tail is the struct's own binder, rigid inside its methods. `Origin::coerces_to`
is the tail rule: unbound accepts anything, everything else demands identity or
containment. The tail erases below the checker exactly like a pointer origin —
mangling emits nothing for it, MIR verification compares struct prefixes only,
monomorphization treats origin-differing instances as one (the native instance
key erases its arguments' origin tails, since the instance symbol spells none —
keying on them would mint a second instance under the first one's symbol), and
the checker's
instantiation records reset it to unbound so a place origin (a per-check owner
id) never makes an instantiation look new across discovery rounds.

An `origin_of(self)` argument on a trait method's *abstract* signature has no
bound `self` place, so it lowers to the symbolic `Origin::SelfParam` — the
`Origin`-level analogue of the signature contract's `SigOrigin::Self_` — which
carries the receiver origin through the associated-type application and, like
every non-`Place` origin, erases from the runtime ABI (collapsing to the single
mangling marker). A conforming struct then resolves the origin-parameterized
member concretely, so a requirement returning `Self.IteratorType[origin_of(self)]`
is satisfiable and conformance succeeds. The borrowed `Iterable` proof protocol
uses this origin-parameterized member, and the bundled List/Set iterator
carries its origin in its origin tail (with an infer-only `Bool` binding its
`mut=`, which erases), iterates itself through upstream's
`IteratorType[...] = Self` alias, borrows its source through a `ref` field, and
yields element references whose mutability the checker resolves from the
source at each loop site. Concrete List/Set/Dict borrowed iteration
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
and any permitted write-through is preserved. The native backend applies the
same test at both sites: a `for` loop's own advance copies the element out in
`lower_try_next_reference`, and a direct `iterator.__next__()` through the
contract copies it in `copy_reference_result` (`lower/methods.rs`), keyed on
the resolved callee's compiled `ResultForm`.

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
A value of a bare type parameter whose bound requires the dunder dispatches
through the bound the same way, as the call `x.__iadd__(y)` would; a parameter
whose bounds do not require it keeps the operator path. The call's result
register takes the contract's `None` result, since a dispatched target names no
declaration. Native scalar targets keep the primitive `BinOp` path. A user-struct
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

Hashable values contribute to a caller-provided hasher through
`__hash__(self, mut hasher: Some[Hasher])`; a conformer that omits it receives
a synthesized field-by-field default at elaboration (next to the `copy`
synthesis). `hash`, `Dict`'s bucketing, and the bundled `AHasher`/`Fnv1a` are
ordinary stdlib code in `std.hashlib`. The compiler contributes only the leaf:
a scalar or literal receiver's `__hash__(hasher)` is an intrinsic that
normalizes the value to one `UInt64` (its unsigned bit pattern, `-0.0` folded
to `0.0`; a `StringLiteral` materializes to `String` and hashes as that
struct) and calls the hasher's `_update_with_simd` — a runtime dispatch on the
VM, a monomorphized call natively — and a constructible type parameter (`H()`
under a `Hasher`/`Defaultable` bound) is reified at runtime as the bound
struct's name (`MirInstr::ConstructTypeParam`), bound into a def's frame or a
struct's value parameters with the declaration default when unsupplied. An
erased body that forwards its own binder (`hash[Self.H](key)` in
`DictEntry.__init__`) reifies the binder's spelling (`Const::Str("H")`), which
the VM resolves through the caller frame's bindings at the call
(`runtime_parameter_arguments`), so an explicit `Dict[K, V, SumHasher]`
hashes under `SumHasher` on insertion as well as lookup; the native
monomorphizer does not yet honor that forwarding (a `docs/roadmap.md`
native-lane residue).

Examples of syntax that may parse before it is fully implemented include richer
trait features and advanced expression/declaration forms that the VM does not
yet execute.

### Checked Templates

A generic body is inferred once, with its parameters symbolic, and the
instances it covers inherit that result instead of being inferred again as
clones. The vocabulary is `crates/mojito-checked/src/templates.rs`; capture,
certification, realization, and installation are
`checker/template_facts.rs`. The design record, with every certificate class
and its soundness argument, is
[`docs/notes/instantiation-from-template.md`](notes/instantiation-from-template.md).

- **What is a template.** A module-level generic `def`, or a method of a
  generic struct. A method has no source range of its own, so it is named by
  its struct and the range of its body's first statement. Both reach the
  mechanism through one `BodySite` (`check_def_body`, `check_method_body`).
- **One producer per body.** The executable check retains the template of a
  trait-bound generic that survives elaboration. Source validation retains
  the template of a body it checks and the elaborator then stubs.
- **Capture is total or it refuses.** `FactTable` enumerates every
  occurrence-keyed fact table. A body that recorded into a table without a
  derivation recipe, keyed a fact outside its own occurrences, grew a store
  that is not keyed by occurrence, or read a callee effect summary that was
  not empty is retained as `TemplateCoverage::Incomplete`, and its instances
  keep the clone check.
- **The trace is explicit.** The elaborator records which prepared declaration
  each `def` clone instantiates and what each compile-time parameter became
  (`DefInstanceTrace`), the same for each per-instantiation method clone
  (`MethodInstanceTrace`), and everything it generated
  (`GeneratedDeclarations`). Only that list, or a clone's explicit receiver
  type, says a declaration is generated: a module-qualified source name
  carries a `$` too. A clone node keeps the syntax identity of the template
  node it was copied from, and `rekey_syntax` returns the identity each
  re-keyed node had before (`SyntaxOrigins`). No correspondence is inferred
  from a mangled name.
- **An instance still owes its obligations.** Realization substitutes, then
  discharges the `rebind` equalities, the implicit copies, `Movable` at each
  transfer, the deletability of each local of a parameter type, the clone
  lookup of each retained application, the built-in `len` witness, the
  retarget of each closed method call (`method_clone_target`), and the empty
  effect summaries. A method beyond a scalar getter also owes that every
  instance argument is plain data. It also records the generic-struct applications the body
  reaches, substituted, so a derived clone requests the instances an inferred
  one would. A failed obligation refuses the derivation, so the clone
  check reports it in its own words. A refusal never accepts a program.
- **Selection is bound once.** A derived instance inherits the overload its
  template selected and never ranks the set again, as the pinned Mojo binds a
  call while it checks the generic body.

`MOJITO_VERIFY_TEMPLATE_FACTS=1` infers every derivable body as well and
requires the two fact bundles to agree.

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
   use, a non-empty collector, fewer implicit copies into `var` parameters,
   parameter-signature length, then how a variadic candidate binds the rest of
   the arguments (more bound by value, then fewer `ref` parameters), and the
   generic/concrete tie-break. The copy a place costs a `var` parameter is
   charged to every candidate — a method's and a constructor's as much as a
   free function's — while the collector, by-value and `ref` terms decide
   between variadic candidates alone, since outside a pack the pinned Mojo
   follows different rules for a trivial value handed to a `var` parameter. A
   method is ranked by its own compile-time parameters, not its owner's. A
   string literal a type pack absorbs counts the conversion its
   materialization to `String` costs a regular parameter
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
the implicit copies a place costs a `var` parameter, signature length, and
generic/concrete specialization; the VM never repeats overload or conversion
selection dynamically.

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
view — including a `for ref` loop over the source, whose element loan is
mutable and therefore exclusive against the view (a value loop's element
loan is shared and coexists). A local annotated with an explicit origin
argument (`var v: Span[Int, origin_of(xs)]`) keeps that demand for every
later assignment, not only its initializer.

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
crates/mojito-hir/src/hir.rs
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

Evaluation is three-valued. `Checker::constraint_verdict` lowers a constraint
under an environment to a `Bool` parameter expression and classifies it as
`Proven`, `Disproven`, or `Residual`: a leaf the bindings decide is a constant,
an arithmetic operand (`ConstraintOperand::Expr`, the `n + 1` of `where n + 1
== m`) stays its canonical expression, and a leaf whose parameter has no
binding is unknown. The connectives are the parameter-expression context's, so
`not` over a residual is a residual and a missing binding never becomes an
acceptance under negation. As at the pin, an application needs evidence even
with its arguments symbolic: a residual passes only when the enclosing
declaration's own `where` assumes the same canonical proposition, and is
otherwise reported as lacking evidence rather than as false. A selecting
consumer (an overload, a conditional conformance, method availability) reads
`eval_generic_constraint`, which is `is_proven()`.
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

A nested declaration is specializable either by its own shape — compile-time
control flow, a pack, a `DType` parameter — or because the top-level walk found
its body able to reach a compile-time-keyed stub, which it reports by the
nested body's declaration site. Such a body has no concrete argument to select
the callee's arm with, so only an instance per call can run it. The nested pass
therefore also joins the discovery fixpoint. A call whose compile-time
arguments only the checker can solve leaves its template standing — verbatim,
or as a stub when the body holds compile-time control flow — so the discovery
check types the call against it and records the instantiation the next round
serves; a template with any such call mints no instances that round, or an
instance minted for a sibling call would be the clone the checker retargets the
unsolved one to, consuming the recording. Calls the fixpoint never serves are
reported like top-level ones and rejected there. A generated instance's calls
to top-level templates are rewritten from the requests recorded against that
instance's own source tag, and the clones they name are queued by the top-level
walk, which alone can mint them.

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
The checker owns the implicit-copy decision in one funnel
(`Checker::check_consuming_as`): a *place* reaching a consuming position —
`var`/`deinit` parameters and receivers, operator `var` operands, storage
initialization, assignment, field stores, returns, displays — is an implicit
copy only when its type is `ImplicitlyCopyable`, and MIR receives
`CopyPlaceValue` only for such places; a `Copyable`-only place must spell `^`
or `.copy()`, exactly as upstream, and there is no last-use move. Conditional
lifecycle conformances are folded per specialization: a `deinit` method may
implicitly copy a named tuple only when every element is `ImplicitlyCopyable`;
otherwise the call must transfer the tuple with `^`. A borrowed `self` method
receiver over a reference result reads its retained place — `MethodCall`
records `recv_writes` for a `mut`/mutable-`ref`/consuming receiver — so a live
shared loan on that place does not conflict with the call.

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

A nonraising `__next__` has no loop shape: the checker rejects it with upstream's
`__has_next__` diagnostic, and only compiler-private carriers (the CTFE
`ComptimeList` and the runtime pack) test `has_next` before a method-free `next`.
For a concrete borrowed List, Set, or Dict place, the preheader carries its
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
checked `TryNext` operation, which catches the `StopIteration` that the bundled
`Iterator.__next__` requirement declares, carries the same explicit copy-reference
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
crates/mojito-mir/src/mir.rs
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
place — including the target of a field store or augmented assignment
(`xs[i].field = v`) — lowering evaluates it once into a hidden `Ty::Ref`
local. `DefVar` stores the handle and `EstablishLoans` retains its owner
generation; the derived place names that local in `through`. Later chained
loads, projections, stores, and calls therefore preserve provenance without
rerunning the accessor or treating its referent as owner storage; a nominal
subscript never becomes a raw `Index` projection on the collection.

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
the drop-elaborated program it actually executes. The loan analysis reads
each call's retained argument places against the callee's declaration
(a program-wide `CalleeRefParams` table): a place retained at a slot whose
`param_writes` mask is set — a `mut` parameter, or a `ref` whose declared
origin is statically mutable — is an exclusive write, while a place lent to
any other slot — a read convention, or a `ref` under an immutable,
parametric, or bare contract, which no body can write through (a
borrowing-view constructor, `Named`) — is a shared read. A single-`__init__`
construction calls its bare struct name and resolves to the
`<name>.__init__` declaration; a callee without a declaration falls back to
its function's `ref_params` mask, and an unknown callee stays exclusive.

A place's storage type is distinct from its expression value type. For a field
declared `ref[origin] T`, the place stores `Ty::Ref`, while an ordinary load
produces `T`. The VM now chooses reference read/write behavior from this checked
place type; it does not inspect a runtime `Value::Ref` to rediscover semantics.

### Important MIR Instructions

Representative instructions:

```rust
Const
SizeOf
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

A place that reaches a stored handle *below* its root — through a `ref`-typed
field, or a single-pointee pointer field (`p.src[].v`) — is likewise not raw
frame storage, because the handle designates storage the executing frame may
not own. Reads, writes and drops of such a place all route through the
reference walk, which chases stored handles mid-projection and re-roots at the
storage they designate; frame-place navigation owns only handle-free storage,
plus a final dynamic index, which is a heap allocation's element or a nominal
`__setitem__` receiver.

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
therefore cannot collide in lowered callable identity. A free function's
signature walks its parameters in declaration order on both the definition and
the call-resolution side, and the names of the parameters after a `*args`
collector — keyword-only by position — join the key, so `f(a: Int, *rest: Int)`
and `f(*rest: Int, a: Int)` are two declarations with two symbols. Direct
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
specializations instead of shifting the evaluated type/value arguments; a
checked type's origin tail spells as its binder where the clone has one in
scope and as upstream's `_` placeholder otherwise. An
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
immutable-origin cast (`ImmOrigin(...)`), which
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
`crates/mojito-symbol/src/symbol.rs`: a signature is typed data (`SignatureKey`, built from either
the declared `ast::SourceType` or the checker-resolved `Ty` — both spell a type
from its annotation, e.g. `Point`, `Pair$Int`) and only the module formats the
final symbol. Checker, MIR, and VM all route through it, so the recorded
callee always names the emitted function; `tests/symbol_test.rs` pins the
spellings and scans the facade's `src/` and every `crates/*/src/` for stray
hand-built `$ov$` strings.

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
crates/mojito-analysis/src/analysis.rs
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
- moving a field marks that field moved; a later use of any disjoint part of
  the value is an error until the field is written back
- a whole redefinition of the variable, or the function's exit, while a field
  is moved out is an error (the value cannot be destroyed as a whole)
- a `deinit` parameter's direct fields are independent values: each may be
  moved out and is destroyed at its own last use
- reassigning a moved variable or moved field reinitializes it

This is why control-flow lowering happens before ownership analysis. A move
inside an `if` or loop only has the right meaning once joins and back-edges are
explicit.

### Persistent local loans

Local `ref name = place` bindings are checked references, not copied referent
values. MIR emits one grouped `EstablishLoans` operation for each fresh binding
generation; every `MirLoan` retains the executable owner `MirPlace`, permission,
optional canonical `MirInteriorOrigin`, and whether it is a shared alias (a
`Pointer(to=place)` loan: two shared loans of overlapping storage coexist,
while owner accesses and exclusive loans treat each as an ordinary borrow).
Statically resolvable aliases use the frozen place, while cross-call aliases
use explicit reference operations.
`MirPlace::through` records which reference authorized an access. The VM erases
both loan and interior-generation metadata after checking.

Reference variables participate in backward CFG liveness. A loan is active from
its binding through its last use, including joins and loop back-edges. While it
is active, direct or differently-authorized overlapping mutation, replacement,
move, drop, or mutating call is rejected. Establishing an interior-generation
loan (a `for ref` loop's element loan) beside a whole-place loan of the same
storage, or the reverse, conflicts when either side is mutable; two interior
loans stay generation-tracked rather than exclusive. Projection overlap is
field-sensitive and index-conservative. Drop elaboration combines that
liveness with the
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
their pointer-mediated stores never reach that acceptance point. Effects live
in a callable-identity-keyed side map: bare names for unique callables and
signature-qualified symbols for overloaded methods. This keeps a consuming
candidate from inheriting a borrowing sibling's loan effects (and vice versa).
Visibility is
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
with the callee binding as the receiver; overloaded method calls replay the
selected signature-qualified entry (free-function overloads retain their
shared bare-name entry); and a trait-method call on a bounded receiver —
which has no concrete body — replays the union of effects over every
conforming implementation and overload of the method, one observation per
conformer key.
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
and its literal constructors are ordinary Mojo bodies. The one intrinsic is
upstream's pair of `StringLiteral` byte primitives, `byte_length()` and
`ptr() -> Pointer[Byte, ImmStaticOrigin]` (`pop.string.size` /
`pop.string.address`): the VM addresses a never-freed allocation interned
per literal text, the native backend the interned `mjstr_<n>` constant or a
runtime descriptor's bytes. `String(literal)` allocates and `memcpy`s from
`ptr()`; `StringSpan(literal)` (upstream's `StaticString` initializer) views
`ptr()` through `unsafe_mut_cast[True]()` and `unsafe_origin_cast`. Writing
goes the other way through upstream's `Writer.write_string(mut self, string:
StringSlice)`: `String.write_to` hands the writer `StringSpan(self)` and
`StringSpan.write_to` hands itself. The VM's builtin text accumulator and the
native print path read the view's bytes; a user writer's `write(...)` gets
each formatted argument as a view over a buffer freed after the call.
Everything else (comparison, concatenation, membership, hashing, decoding,
slicing, and the result APIs, which live on the view with `String`
forwarding through `StringSpan(self)`) is pure library code.

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
nominally-declared `write_string`. Overload symbols spell the two types
distinctly (`String` and `StringLiteral`), so `def f(x: StringLiteral)`
beside `def f(x: String)` is a legal overload set and a literal argument
selects the exact `StringLiteral` member (exact matches rank above
conversions), as upstream. In unlinked
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
through immutable or unresolved symbolic provenance
(`pointer_write_capability`: a pointer field's `Origin[mut=m]` binder resolves
through the holder's construction-time origins — for a call result, the
origins its callee's view-return contract (`ViewReturnOrigin`, resolved into
`call_result_origins` at the call) bound with the callee's own capability —
and the dereference place carries that capability rather than the holder
binding's; a call result holding the dereferenced pointer materializes as an
anonymous owned slot, `materialize_temporary_holder`), and returns that
would escape the origin
(`returned pointer escapes storage outside its declared origin`). A method may,
however, return the *dereference* of an origin-bearing pointer field whose origin
is a struct/callable parameter (`def get(self) -> ref[o] Int: return self.p[0]`):
the returned `ref[o]` stays within that parameter, so the return-boundary
re-rooter (`canonical_reference_parts`) follows the field handle to the single
pointee and retains the residual offset-0 index, which the runtime projection
walkers forward as the identity deref of that pointee — an immutable origin reads,
a mutable origin writes through the caller's storage. A read of the bound pointer
variable itself — a copy into another local, an annotated binding, or a call
argument — is a plain `UseVar` value read of the stored handle, which both
backends interpret as the pointee's address.

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
the loop rather than being overwritten during normalization. Whether the loop
keeps that source alive is the checked `IterationProtocol::source_retained`
fact: the normalized iterator may refer to its source only when its declaration
binds an `Origin`/`OriginSet` parameter, holds a reference or pointer field, or
its `__next__` returns a reference. When it cannot, nothing extends the
source's lifetime and ASAP destruction runs its `__deinit__` right after
`GetIter`, as upstream does. Otherwise a borrowed **temporary** — the only
owner of its storage — is `Bind`-bound and kept live by a `KeepAlive` liveness
anchor at the loop exit, then destroyed exactly once after the loop. A borrowed
**named** source is instead `MakeRef`-bound (a genuine reference, no copy) and
its dependency is recorded as a loan — a whole-place shared loan, or an
interior `element` generation for a concrete collection place — re-established
on the long-lived iterator-object slot: the loan keeps the source live through
the loop (no `KeepAlive` needed) and rejects conflicting mutation of the source
during iteration; the reference slot is read only by `GetIter` and dropped
afterward as a no-op. Owned iteration keeps the
single slot: `__iter__(var self)` consumes the source into the iterator.
Bundled borrowed paths cover List, Set, Dict, and Range; the bundled owned path
is currently List-specific. For a current typed-raising iterator, `TryNext`
invokes `__next__(mut self)`, writes the mutated iterator back, and branches on
whether the call returned an element or raised the exact checked
`StopIteration`; a generic `Iterable` bound takes the same path through the
abstract `__iterator_dispatch.__next__`. Bundled Range/list/set/dict iterators
and concrete user iterators use that nominal path. `HasNext`/`Next` serve only
the method-free fallbacks: the CTFE-only `ComptimeList` carrier and the
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

`try` regions are walked with three channels per region (normal fall-off,
raise points, `return`/escape exits), the interior-origin analysis's shape:
the `except` arm starts from the join of the states at every potentially
raising instruction of the body (after that instruction's effects, with a
named destructor's trailing `ConsumeVar` folded in, since the receiver is
consumed at the call), `else` from the body's completion, and a `finally` is
checked from the join of all three and re-summarized per entering channel.
The states after the `try` are the join of the handler's and `else`/body's
completions, so a value consumed before a raising call is uninitialized in
the handler and after the `try`, while one consumed after the body's last
raise point is initialized in the handler. Definite and path-dependent
transfers report upstream's `use of uninitialized value 'x'`.

## Stage 7: Liveness And ASAP Destruction

Same module:

```rust
crates/mojito-analysis/src/analysis.rs
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

Three timing rules complete the last-use model, each pinned against the
current Mojo:

- A call result no expression consumes (an expression statement; the unbound
  result of a non-consuming `__enter__`) is an owned temporary: MIR lowering
  binds it to a hidden `$discard_r` slot that is dead at its definition, so
  its `DropVar` follows the call immediately — before the next statement.
- An owning temporary receiver of a method (`B(3).show()`, `print(B(2).get())`)
  is bound to a hidden `$tmp_recv_r` slot whose place the call retains, as a
  named receiver's is (`lower_method_receiver`): the call is the slot's last
  use, so the temporary's `DropVar` follows the call, before the enclosing
  expression continues. A `var`/`deinit` receiver convention binds no slot;
  the callee owns the temporary. A receiver whose result borrows it takes the
  `$mat_r` path above instead, and lives as long as the borrower.
- An owning temporary at a read parameter (`take(B(2))`) is bound to a
  hidden `$tmp_arg_r` slot and the call reads that slot, the shape a named
  local takes there (`bind_temporary_argument`); the loaded register retains
  the slot through the call, so the `DropVar` follows it. The checker decides
  which arguments qualify: `read_temporary_arguments` (`checker/places.rs`)
  marks each non-place argument bound to a read convention with
  `ReadTemporaryArgument`, so a `var`/`deinit` slot — which moves the
  temporary into the callee — carries no marker, and MIR never guesses a
  callee's conventions.
- A field read off an owning temporary (`print(B(1).x)`, `var n = B(9).name`)
  binds the temporary to a hidden `$tmp_field_r` slot and loads the projected
  place (`load_temporary_field`), as `p.x` does off a named local: the load
  retains the slot through the instruction consuming the field, and a
  consuming context's `CopyPlaceValue` copies an owning field out before the
  slot dies. A reference-typed field keeps the register `GetField`.
- A scalar `LoadPlace` (`t.n` fed to `print`) keeps its owner alive through
  the one instruction consuming the loaded register (the register-loan
  dataflow's single-hop retention), so the owner's `DropVar` follows the call
  that reads the field rather than the load; an aggregate load retains its
  owner through every consumer as before.
- A consuming parameter (`var`, `deinit`, receiver or not) is a drop root of
  the callee: it is destroyed — for `deinit`, consumed — at its last use in
  the body, and one the body never uses dies at the function's entry, before
  the first statement runs.

The VM does not need to discover last uses dynamically. It just executes
`DropVar` where the compiler placed it.

### What Gets Dropped

Drop roots are selected independently of type: every owned local and consuming
parameter receives drop glue. This conservative policy releases heap-backed
runtime storage at its last use even when no user `__deinit__` call is observable,
and it naturally covers destructor-less structs containing aggregate storage.
Ownership is limited to:

- locals
- consuming `var` and `deinit` parameters, receivers included

Borrowed parameters are not dropped by the callee. They are owned by the caller,
as is a `self` outside the leading parameter range of a generated method CFG.

### Drop Order

When several variables die at the same point, they are dropped in reverse
declaration order. Struct destruction runs:

1. the struct's `__deinit__(deinit self)`, if present — its body owns the
   receiver as independently dying direct fields: each field is destroyed
   at its own last use inside the body (an unused one at the destructor's
   entry, before its first statement; a nested projection keeps its whole
   direct field alive; a transferred field is its new owner's), and the
   receiver's `ConsumeVar` at its last use destroys only the survivors
2. otherwise the fields, in declaration order

The compiler-private heterogeneous pack carrier likewise drops elements
left-to-right, matching current Mojo's pack-storage lifecycle. Public
collections, including `Tuple`, are nominal structs and follow the ordinary
declaration order for fields; their library destructors own any
element-specific teardown. Per-field liveness for `deinit` parameters is a
refinement pass after the variable-granular elaboration
(`analysis/field_drops.rs`): for every `deinit` parameter of struct type it
runs a backward liveness over `(parameter, field)` keys — a depth-one field
projection uses that field, a bare receiver touch uses every field, a loaded
register carries its field to every consumer as the register-loan rule does
— with the same fall-off/raise/`finally` seeds and edge splitting as the
variable pass, recursing into `try` regions, and splices `DropPlace` (a
whole-value destruction of one field that tombstones it) after each field's
last use, on block 0 for the unused, and on edges. The receiver's existing
`ConsumeVar` (in-block, cleanup lists, and the native exit backstop) then
skips the tombstoned fields, so raise edges stay sound. Fields that root a
loan, are moved below depth one, or need no destructor work are left to the
`ConsumeVar`. A field read only inside a loop dies after the loop; the pinned
Mojo destroys it at entry (recorded as an `output-diff` row).

A store into a field destroys the value it replaces at the store, as Mojo's
assignment does. A whole-variable reassignment needs nothing extra (the old
value dies at its last use before the redefining `DefVar`), but a field is not
a variable, so a pass ahead of the variable-granular elaboration
(`analysis/store_drops.rs`) splices a `DropPlace` immediately before each
`Store` into a static field or constant-index element of droppable type whose
subtree is initialized there. A whole place written *through a reference* has
no redefining `DefVar` either — its old value is the caller's or another
slot's — so the same pass drops it first: a `WriteRef` through the handle a
`mut` parameter or `mut self` receiver makes of itself, and a `Store` through
a place pointer (`q[] = v`) or a whole-variable `ref` binding. A `WriteRef`
names only its handle register, so the place comes from the `MakeRef` that
defined it. Such a write qualifies only where the whole subtree is intact:
dropping a partially moved value whole would free a hole, and a reference-
rooted place has no leaf flag to guard it natively (reassigning a partially
moved value is a checker error in any case). It replays the move checker's place-tree flow
(`observe_move_states`) from a different entry: parameters initialized, every
other slot uninitialized, and an initializer's `out self` receiver with each
declared field uninitialized, so a constructor's first store into a field
destroys nothing and a second destroys the first. A field wholly moved out is
reinitialized without a drop; a depth-one field moved on only some paths drops
under the VM's tombstone and the native leaf flag. The `DropPlace` touches the
same root at the same position as its `Store`, so liveness, loans, and the
variable pass are unchanged by it. On the VM it follows a reference root (a
`mut` receiver or parameter) into the caller's storage, and equally a handle
the place reaches below its root (the value `p.src[].v = …` replaces lives
wherever `p.src` points); natively a nested or reference-rooted place drops
unguarded at its address.

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
`__deinit__` the named destructor replaced). The callee owns its `deinit`
receiver from the call on: its own elaborated `ConsumeVar` destroys the
residual fields at the receiver's last use inside the body (moved fields are
tombstones, so nothing re-drops), the VM vacates the caller's receiver place
before the call, and the caller's trailing consumption therefore finds
nothing left to destroy. Consumption is unconditional: a raising named
destructor leaves the source uninitialized on both edges (upstream's rule),
and the checker rejects an `except` arm that re-consumes it.

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
crates/mojito-vm/src/backend/vm.rs
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
crates/mojito-vm/src/runtime.rs
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

`size_of[T]()` is deliberately not a VM-value helper. Checking records `T`, MIR
retains it in `SizeOf`, verification requires it to have a native layout, and
both the VM and native lowering ask the shared `native::layout::LayoutCx` for
the byte count. This keeps target layout below the checked-MIR waist without
duplicating layout policy in either backend.

`external_call["callee", ReturnType, num_fixed_args=n](args...)` is the one
host boundary below the stdlib: a builtin over the closed libc callee table
`mojito_types::ffi::CALLEES`. The checker (`checker/ffi_calls.rs`) accepts
only an allowlisted callee (any other name is a contextual error), checks
each argument and the declared return type against the C prototype, and
types the call as never raising; MIR carries it as an ordinary named `Call`
whose first parameter argument is the callee string constant, and every
backend reads the result type from the destination register. The VM
(`backend/vm/libc.rs`) executes each callee on Rust's standard library with
libc's observable contract — return value, an `errno` slot behind
`__errno_location`, bytes written through pointer arguments (`readdir`
produces glibc `dirent` images, `__xstat` fills the caller's `_c_stat` by
field name) — over a descriptor table where fd 1 appends to the captured
stdout and `setenv`/`unsetenv` write a per-VM overlay; the native backend
declares the C function on demand and calls it. No compiler-private
`_mojito_*` crossing exists for I/O. `print(sep=, end=, flush=, file=)` is the
same builtin with keywords: the VM joins the cells and writes them to the
captured stdout or, for `file=`, through its descriptor table; the native
backend routes every piece of such a call through libc `write` on the
`FileDescriptor`'s value instead of `mjrt_write_stdout`.

The `with` statement is a checker desugar (`checker/with_stmt.rs`), not a
lowering construct: the manager's declared `__enter__`/`__exit__` methods
select one of a few ordinary statement shapes (`VarDecl`, `Try`, `If`,
`Raise`, `Expr`), the checker checks that desugar in a block scope, and after
the last transfer round it splices the desugar into the checked tree in the
statement's place. HIR, MIR, ownership analysis, and the backends therefore
never see a `With` node. Each synthesized node's identity is derived from the
statement's (`SyntaxId::derived`), so every rebuild of one statement's desugar
names the same occurrences, and an instance derived from a checked template
builds its own from the template's recorded form. The one compiler-private spelling it emits is
`_mojito_keep_alive(name)`, a statement MIR lowers to the existing
`KeepAlive` liveness anchor (resolved through the argument's checked binding,
so a later block rebinding the same `as` name anchors its own slot): it keeps
the manager — or a consuming `__enter__`'s result standing in for it — alive
to the end of the block without a copy or a move, while an `as` binding is an
ordinary local destroyed at its last use, as the pinned Mojo does. The unbound
result of a non-consuming `__enter__` is not anchored: it is a discarded
temporary destroyed before the body runs.

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
bytes, parses a Mojo-independent spanned schema tree, and decodes the complete
schema — every instruction, terminator, type, origin, and declaration-metadata
form the printer emits — while retaining an `ArtifactSourceMap` separate from
serialized Mojo source locations. Nested `try` regions decode through their
own dense per-region block namespaces and are deliberately absent from the
source map: the enclosing instruction path brackets them, and the canonical
verifier resolves only function-level block paths. Parsing does not invoke semantic MIR verification:
`mir::text::verify_artifact` composes the canonical `mir::verify` pass on the
returned `ParsedArtifact` and maps each finding to its assembly span (block,
then function, then artifact-root precision) by parsing the verifier's
canonical `MIR function '<name>' [block <n>]` message prefixes — the one
sanctioned consumer of that spelling. `load_artifact` bundles parse-then-verify
as the loading gate artifact execution sits behind; verification policy itself
never moves out of `mir::verify`. `artifact::run_artifact` composes that gate
with `Backend::run_elaborated` (the CLI `exec` subcommand's engine): the
loaded program executes exactly as serialized, with no re-elaboration,
re-verification, or ownership re-analysis. The composition lives in
`src/artifact.rs`, beside rather than inside `Compiler`: the artifact path
deliberately bypasses the source pipeline, and `mir::text` cannot host it
without layering onto `backend`.

The compiler exposes a textual, flattened, versioned serialization of
verified MIR and the metadata needed to execute it. The format is built for
determinism and tooling rather than human reading — even trivial programs
serialize to long artifacts. It must support:

- deterministic printing suitable for review and golden tests (implemented)
- parsing with source-located diagnostics (implemented for the full schema)
- structural and semantic verification before execution (implemented)
- lossless print/parse/print round trips (implemented; enforced byte-for-byte
  per executable corpus fixture by the `roundtrip::*` group of
  `tests/corpus_test.rs`)
- disassembly of verified in-memory programs (implemented)
- execution by the register VM without reconstructing source AST semantics
  (implemented: `artifact::run_artifact` composes `load_artifact` with
  `Backend::run_elaborated`, which runs the serialized, already
  drop-elaborated program as-is — re-running `elaborate_drops_program` is
  unsound because elaboration is not idempotent, and the pre-drop ownership
  analysis has no meaning post-drop; verify-at-load is the consumer gate)
- consumption by future native backends (Pliron and Cranelift first)

`mojito emit-mir [FILE]` exposes the producer boundary for files or standard
input. It prints only the canonical post-drop artifact accepted by `mojito
exec`, so the two commands compose directly in a pipe. Every shared runnable
conformance case executes both from the cached compiler MIR and after textual
serialization/loading; output and observable bindings must agree.

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
  unchecked phase-test path and nominal callable-conformance facts in
  `MirDeclarations`
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
