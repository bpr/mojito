# Parameter expressions as typed canonical attributes

A value argument such as the `n + 1` of `Buf[n + 1]` is a **parameter
expression**. The front end represents one as a typed, immutable, canonical
node — `mojito_types::param_expr::ParamExpr` — so two expressions it can prove
equal are the same node, and `Buf[n + 1]` is the type `Buf[1 + n]` before any
value is supplied. This note records the representation, the pin evidence
behind each rule, and how the payload re-homes into a parametric dialect.

Code anchors: `crates/mojito-types/src/param_expr.rs` (`ParamContext`,
`ParamExpr`, `ParamKind`, `ParamOp`, `MetaTy`, `ParamId`, `ParamBindings`,
`ParamEval`, `ParamError`, `ConstraintVerdict`, `ParamConstraint`),
`param_expr/fold.rs` (`fold_infix`, `fold_neg`, `compare`), and
`types.rs` (`DependentType::Parameter`, `TyRewrite`, `rewrite_ty`,
`replace_parameters`, `referenced_parameters`).

## What a node is

A `ParamExpr` is an `Arc` handle to `{ fingerprint, meta, kind }`. Every node
has a checked meta-type and is built by a canonicalizing constructor on a
`ParamContext`; there is no unchecked construction path.

| Kind | Meta-type | Contract |
|---|---|---|
| `Constant(CtValue)` | from the value | Closed: no free parameter in the value, its type, or below it. All sixteen concrete `CtValue` forms project here. |
| `DeclRef(ParamRef)` | the declaration's | A declared parameter of an enclosing declaration. Identity is `ParamId { owner, slot }`; the spelling is diagnostic metadata. |
| `IndexRef { depth, index }` | the slot's | Slot `index` of the signature binder `depth` levels out. |
| `Op { op, operands }` | checked per opcode | A primitive in canonical form. |
| `Identical(a, b)` | `Bool` | Whole-value identity, distinct from numeric `==`. |
| `Conforms`, `Trivial` | `Bool` | Contextual queries over a Type-meta-type subject. |
| `TypeShape(Ty)` | `Type` | A type with free typed references; a closed type folds to a Type constant. |
| `Select { elements, index }` | `Type` | Finite type selection, the payload of `DependentType::Parameter`. |
| `PackQuery` | `Bool` / `Int` | Transport for today's bound-pack constraint leaves. It adds no symbolic pack support. |
| `Hole { kind, token }` | explicit | Reserved unknown/unbound. No source construct produces one; it is a boundary error at MIR. |

`MetaTy` is `Value(Ty)`, `Type`, `ReflectedType`, the compile-time aggregates
(`Tuple`, `List`, `Dict`, `Set`), and the reserved `ParamList`. `Type` is not a
runtime `Ty`, so it has no place in that lattice.

`CtValue` stays the concrete transport. `CtValue::Param(String)` is gone:

- `CtValue::Expr(ParamExpr)` is a **residual** — never a constant. A folded
  expression is its ordinary concrete variant (`ParamExpr::into_value`), so a
  type argument has one representation of each constant.
- `CtValue::Deferred(String)` is a slot whose value arrives later and takes no
  part in generic identity: a callable-value parameter the VM reifies, a value
  argument only the elaborator can fold under source validation, or an
  elaborator-private marker. It is not a parameter reference and never enters
  a specialization key.

`CtExpr` is gone too: a value parameter's default, a conditional callable
default's condition, and a dependent index are `ParamExpr`s.

## Interning, equality, hashing

`ParamContext::new()` is a per-compilation hash-consing context; cloning
shares it. The compilation's context rides in `TemplateCatalog`
(`param_context()`), which already travels through source validation, every
discovery and transfer round, and finalization, so each checker run of one
compilation interns into one table. `ParamContext::detached()` canonicalizes
without interning, for pure helpers in `mojito-types` (`substitute`,
`canonical_generic_signature`) that have no compilation at hand; its nodes are
structurally equal to interned ones, and `intern` re-homes them.

Equality is pointer identity first and canonical structure otherwise, so it
holds across contexts. Hashing is the cached structural fingerprint, never an
address. Operand order (`Ord`) is kind, then payload — never a pointer, an
allocation order, a randomized hash, or a source location.

`Ty` and everything it reaches derive `Hash`. `TransferSet` is
equality-transparent by design, so its `Hash` hashes nothing; a type's hash
never sees metadata its identity ignores. A node whose payload holds a callable
type with transfer effects is **not interned**: it is equal to its undecorated
twin, and sharing one node would hand one occurrence another's effects
(`equal_types_hash_equally_and_interning_keeps_decorations`).

`TyArg` equality is parameter identity (`identity_eq`/`identity_hash`): a
residual by its node, a constant by its value, a dictionary's or set's display
spelling left out.

A CTFE subprogram is a standalone checker boundary and makes its own context
(`--timings` reports `param_expr.contexts`). Nothing depends on sharing it:
cross-context equality is structural.

## The normal form, and why it is this small

The normal form must be **no stronger than the pinned Mojo's**: Mojito may
reject what the pin accepts, never the reverse. These were probed against
`Mojo 1.1.0.dev2026082605 (dd957314)` on 2026-09-20, each as a generic
`def f[...](var x: Buf[A]) -> Buf[B]: return x^`:

| `A` → `B` | Pin | | `A` → `B` | Pin |
|---|---|---|---|---|
| `n + 1` → `1 + n` | accepts | | `-n + n` → `0` | **rejects** |
| `(n + 1) * 4` → `n * 4 + 4` | accepts | | `0 - n` → `-n` | **rejects** |
| `n + n` → `2 * n` | accepts | | `-n` → `-1 * n` | **rejects** |
| `n - 1 + 1` → `n` | accepts | | `-(-n)` → `n` | **rejects** |
| `n * 1 + 0` → `n` | accepts | | `-(n + 1)` → `-n - 1` | **rejects** |
| `n - n` → `0`, `0 * n` → `0` | accepts | | `n - m` → `-m + n` | **rejects** |
| `2 * n - n` → `n` | accepts | | `n // 1` → `n` | **rejects** |
| `(n + 1) * (n + 1)` → `n * n + 2 * n + 1` | accepts | | `(n * 2) // 2` → `n` | **rejects** |
| `n * m` → `m * n`, `n + m - m` → `n` | accepts | | `n * n` → `n ** 2` | **rejects** |
| `n * n * n` → `n * (n * n)` | accepts | | `n + 1` → `n + 2` | **rejects** |
| `n << 1` → `2 * n` | accepts | | | |

So typed integer `+`, binary `-`, and `*` form a sum of products with
collected coefficients (`Polynomial` in `param_expr.rs`), and a left shift by
a constant is a multiplication. **Everything else is an opaque atom of that
polynomial**: unary `-` on a symbol, `//`, `%`, `**`, `>>`, and the bitwise
operators. In particular unary negation is *not* multiplication by `-1`, and a
power is never expanded — the planning pass expected both, and the pin refutes
both. `assets/ok/param_expr_normal_form.mojo` pins the accepted column;
`assets/type_error/param_expr_{unequal,opaque_negation,power_unequal}.mojo`
pin rejections.

Rules the constructors keep:

1. Machine `Int`/`UInt` coefficients wrap at 64 bits; literal coefficients are
   exact. A literal constant beside a machine operand takes the machine type.
2. A zero coefficient drops its monomial, unless the monomial holds a partial
   atom (`//`, `%`, `**`, a shift), whose possible evaluation error a
   cancellation must not erase.
3. Expansion is budgeted at `MAX_MONOMIALS` (4,096). Past it the constructor
   returns `ParamError::Budget`; no half-normalized node exists.
4. `and`/`or` flatten, fold constants, drop duplicates, and sort. `not x` is
   `x ^ True`, so double negation cancels. `!=` is `not ==`; `>`/`>=` swap.
5. Numeric `==` of identical total operands is `True`; an integer `==` whose
   difference is a nonzero constant is `False`. Anything else is residual.
6. Machine floats are never reassociated; outside the integers an operator
   folds constants and is otherwise opaque with its operand order kept.
7. A closed operator whose fold fails (a division by zero) stays a node, so an
   untaken branch retains it unevaluated and `require_constant` reports it.

Different nodes mean **not established equal**, never *proved unequal*.

## Replacement is not evaluation

`ParamContext::replace` is what type identity sees; `evaluate` is what a
required value uses. They differ, because the pin does:

| Program | Pin |
|---|---|
| `def grow[n: Int](var x: Array[Int, n + 1])`, called `grow[3](Array[Int, 4])` | accepts — the polynomial re-folds |
| `def halve[n: Int](var x: Buf[n // 2])`, called `halve[8](Buf[4](7))` | **rejects**: `Buf[Int(4)]` is not `Buf[(Int(8) // Int(2))]` |
| `var b: Buf[8 // 2] = Buf[4](1)` | accepts — a literal expression folds when written |
| `where n // -2 == -4`, applied at `n = 7` | **rejects**: lacking evidence |
| `struct Holder[n: Int, m: Int = n // -2]`, `Holder[7]` | accepts, `m` is `-4` |

So replacement rebuilds the polynomial through the canonicalizing constructors
and rebuilds an opaque atom **without folding it**, even once its operands are
constants. A type argument and a `where` operand replace. A default, a
dependent index, the MIR verifier's dependent instantiation, and native
monomorphization evaluate (`replace`, then `fold`).
`assets/type_error/param_expr_{unfolded_atom,where_unfolded_atom}.mojo` and
`assets/ok/param_expr_fold_routes.mojo` pin the three behaviours.

Replacement is simultaneous and capture-avoiding. A nested generic callable's
own binders mask same-spelled name bindings and sit one signature level
further in; a replacement value's free signature indices shift past the
binders it crossed. `ParamBindings` binds by `ParamId`, by signature frame, and
— the source-lookup adapter for callers holding a declaration's own name map —
by name, with an identity entry always winning.

## Binder identity

A checker-produced reference is owned by its declaration: a struct's
parameters by the template's name, a `def`'s by its name, a method's by
`Struct.method` (`binder_owner`/`method_binder_owner` in
`checker/annotations.rs`). A `$` clone demangles to its template, so it shares
its template's parameters rather than minting fresh ones, and repeated
discovery rounds agree. Same-spelled parameters of unrelated declarations are
different parameters.

That exposed the sites that used to work because two spellings coincided:
fieldwise construction now binds the struct's value parameters into its field
types, and a non-parameterized associated alias binds the full instance
environment (`substitute_at`), which is what closes `S[Int, 3].Alias`.

A generic callable contract's own value binders become `IndexRef` slots in
`canonical_generic_signature`, so `def[w: Int](…)` and `def[n: Int](…)` are
one identity by construction rather than by renaming.

The checker opens a declaration's value parameters as a scope level beside its
type parameters (`push_param_scope`; `tparams.pop()` closes both), which is
what lets a free `def`'s bare `n` resolve in `Buf[n + 1]`. `Ty::Param` is still
identified by spelling; see the roadmap entry *Type parameters are identified
by spelling*.

Two readers have no owner to give: the elaborator's declaration metadata
(`$elaborated`) and a schema 1.0 MIR artifact (`$mir-1.0:<name>`). Both bind
by name, as they always did.

## One folder

`param_expr::fold::fold_infix` is the only implementation of compile-time
scalar operators. The checker's `eval_ct` and `eval_associated_ct_infix`, the
elaborator's `eval_infix` (which no longer evaluates each operand three times,
and no longer manufactures identifier ASTs in `eval_infix_values`), native
monomorphization's `eval_ct`, and the canonicalizing constructors all call it.
Name resolution, lazy `and`/`or`, membership, CTFE, reflection, and
diagnostics stay with their owners.

Pin-confirmed corrections it carries (`assets/ok/param_expr_arithmetic.mojo`,
`param_expr_overflow.mojo`):

- Machine `Int` `//` and `%` floor toward the divisor's sign. The elaborator
  and native monomorphization used Euclidean division, so `Int(7) // Int(-3)`
  was `-2` and is now `-3`.
- Machine `Int` and `UInt` `+`, `-`, `*` wrap at 64 bits instead of reporting
  an overflow; `Int.MIN // -1` is `Int.MIN`.
- A literal beside a machine operand converts to the machine type and wraps
  (`Int(9223372036854775807) + 1` is `Int.MIN`). True division still promotes
  to an exact float.

Observed and deliberately **not** adopted: at the pin a compile-time `//` or
`%` by zero yields `0`. Mojito keeps the structured error — a rejection the
subset rule allows.

## Three-valued constraints

`Checker::constraint_verdict` returns `ConstraintVerdict::{Proven, Disproven,
Residual(proposition)}`. `GenericConstraint` remains the source-facing adapter;
`constraint_proposition` lowers it under an environment, and the connectives
are the context's, so there is one `and`/`or`/`not`. A leaf the bindings
decide is a constant, a leaf over a residual value is its canonical
expression, and a leaf whose parameter has no binding is an unknown
proposition. `ConstraintOperand::Expr` carries an arithmetic operand
(`n + 1`).

The pin requires evidence at **every** application, symbolic ones included
(`invalid call to 'below': lacking evidence to prove correctness`). So a
residual is neither accepted nor reported as false:

- `eval_generic_constraint`, the Bool every selecting consumer uses, is
  `verdict.is_proven()`. `not` over a residual is a residual, so a missing
  binding no longer turns into an acceptance under negation.
- `validate_constraint_in_environment` accepts a residual only when the
  enclosing declaration's own `where` assumes the same canonical proposition
  (`assume_declared_propositions`, `assumptions_prove`). Canonical operand
  order makes `where k == n + 1` evidence for `n + 1 == k`. There is no solver.

`assets/ok/param_expr_where_assumption.mojo`,
`assets/type_error/param_expr_where_{false,residual}.mojo`, and the checker
test `param_expr_residual_is_not_false` pin this.

## Boundaries

- `TyArg::Val` may hold a residual under a checked declaration binder.
- `Ty::Simd` still holds a concrete dtype and width.
  `conformance/probes/param_expr_simd_hooks.mojo` is the follow-on shape.
- MIR keeps scoped references and pure expressions that instance binding
  supports. `validate_dependent_bindings` rejects a hole, an out-of-range or
  mistyped signature slot, and an unbound dependent reference.
  `types_compatible` refuses two distinct residuals over the same parameters
  (`residual_arguments_conflict`) instead of treating a shared parameter as a
  wildcard.
- The VM materializes no residual or deferred value.
- Native monomorphization closes a residual under the mono environment or
  reports the contextual unsupported boundary; lowering sees concrete types.
- `symbol::mangle` returns `Result<String, NonConstantSpecialization>` and
  validates the whole key before writing it. `specialized_method_values`
  returns `None` for a residual value and skips only a deferred callable slot.
  The erased-origin owner marker is `SpecializationKeyPart::ErasedOrigin`, not
  a value. Every concrete encoding is unchanged
  (`param_expr_closed_specialization_keys`).
- The driver's `closed_generic_argument` separates *replayable* (a deferred
  callable slot, an origin) from *closed*; a residual value names no instance.

## MIR text schema 1.1

The writer prints schema 1.1; the reader accepts 1.0 and 1.1
(`docs/mir-text-format.md`). Concrete `ct_*` spellings are unchanged.

```text
ct_expr(<param-expr>)          a residual value argument
ct_deferred(name)              a deferred slot
dependent_parameter(<param-expr>)

param_constant(<ct-value>)
param_decl_ref  { owner: "sym", slot: N, name: n, type: <meta> }
param_index_ref { depth: N, index: N, type: <meta> }
param_expr      { op: add, type: <meta>, operands: [ ... ] }
param_identical { left, right }
param_conforms  { subject, trait }
param_trivial   { lifecycle, subject }
param_type_shape(<type>)
param_select    { elements: [ ... ], index }
param_pack_query { pack, query }
operand_expr(<param-expr>)     an arithmetic constraint operand
```

`<meta>` is `meta_value(<type>)`, `meta_type`, `meta_reflected`,
`meta_tuple([...])`, `meta_list([...])`, `meta_set([...])`,
`meta_dict([meta_entry { key, value }])`, or `meta_param_list(<meta>)`.

Parsing re-enters the constructors, so a parsed expression is canonical
whatever order the text spelled it in, and a `param_expr`'s recorded type must
be the type it builds to. A 1.0 artifact's `ct_param(name)` translates through
the value parameters the artifact declares: exactly one declared type gives a
typed reference, none gives a deferred slot in value position and an error in
expression position, and several are an error — a 1.0 reference carries
nothing that chooses. 1.0 forms in a 1.1 artifact are errors.

## Re-homing into a parametric dialect

Payloads and canonicalization are independent of storage, so the later move is
a re-homing, not a redesign:

| Here | Parametric dialect |
|---|---|
| `ParamKind` payloads | `#[pliron_attr]` wrapper types holding a `UniquedKey<ParamExprPayload>` and delegating access and printing to the payload. |
| `ParamContext::intern` | `uniqued_any::save(ctx, payload)` after canonical construction. |
| `MetaTy` | The dialect's scalar/meta types; `TypedAttrInterface::get_type`. |
| `DependentType::Parameter` | The equivalent of `!kgen.param<expr>`, folded away when it becomes a type constant. |
| `ParamConstraint` | A constraint attribute plus separate location/message metadata. |
| folding, ordering, replacement, query descriptors | Unchanged pure Rust. |
| `CtValue`, `CtLane`, transfer facts | Stay the compiler/VM boundary. |

A `UniquedKey` is an untagged index, so raw keys from two contexts are never
compared; cross-context clone re-interns. No upstream Pliron addition is a
prerequisite. The KGEN definitions consulted are `KGENAttrs.td`,
`KGENAttrInterfaces.td`, `KGENEnums.td`, `KGENTypes.td`,
`ParameterEvaluator.h`, `ParameterReplacer.h`, and `UnifiedFolding.h`.

Hooks left for adjacent work: the reserved `MetaTy::ParamList` and the
`Select`/`DependentType::Parameter` wrapper for packs (`param_list.get`);
typed value/dtype expressions for the `Ty::Simd` migration; the residual
discharge API for the by-value `rebind` gap; the four-outcome unifier
(`solve_value_args` binds a direct reference and leaves any other residual as
an equation) for variadic-constructor inference.

## Measurements

Release build, `rustc 1.96.1`, Intel i7-10875H, `hyperfine --warmup 1 --runs
5`, baseline `74a3d40` against this change, 2026-09-20:

| Program | Baseline | This change | Peak RSS |
|---|---|---|---|
| `benchmarks/compile/hello.mojo` | 2.032 s ± 0.037 | 1.947 s ± 0.023 | 283.9 MB → 273.2 MB |
| `benchmarks/compile/generic.mojo` | 2.246 s ± 0.059 | 2.136 s ± 0.010 | |
| `benchmarks/compile/tuple.mojo` | 2.065 s ± 0.028 | 2.010 s ± 0.028 | |

`generic.mojo` interns 21 nodes over 744 replacements in one context.
