# The Compile-Time Request Path

How a compile-time evaluation will be served by the elaborator's worklist,
designed for roadmap entry "Compile-time evaluation has no path to the
elaborator's worklist" (2026-10-02, pin `Mojo 1.2.0.dev2026092105`,
`26cfe94f`). It is the third P3 prerequisite of
[`docs/parametric-mir-plan.md`](../parametric-mir-plan.md) and records the
owner's decisions D2 and D3. No code moves here; the entries named at the
end move it.

## Today

A compile-time call is evaluated above the check, by the AST elaborator in
`crates/mojito-comptime`, once per discovery round:

```text
src/compiler.rs  compile_linked
  prepare -> validate_comptime_templates -> elaborate_prepared  (round 0)
  -> check_program_carrying -> [requests grew?] -> elaborate_prepared (round n ≤ 5)
  -> lower to MIR -> ownership -> drops -> native::mono::specialize -> VM

comptime/elab.rs  Elab::stmt / comptime if / comptime for / resolve_ct_arg / …
  -> comptime/eval.rs  Elab::eval
     -> comptime/ctfe.rs  ctfe_call | ctfe_struct_entry | ctfe_generic_def_entry | ctfe_expr_entry
        1. vm_ctfe_safe_fn        effect walk over the AST (print/input reject)
        2. vm_ctfe_declaration_closure, vm_ctfe_subprogram
                                  clone the needed defs, structs, traits, literal
                                  constants from the prepared program; stub
                                  compile-time-keyed methods; mint hasher leaves
        3. [expr entry] check_program_with_templates on a `$ctfe$probe` whose
                                  body binds the expression, read its type,
                                  source_type_from_ty to spell it back
        4. VmBackend::run_function_value(&[Stmt], …)   crates/mojito-vm/src/backend/vm.rs
                                  check_program_with_templates again
                                  -> mir::lower_checked_program -> elaborate_drops_program
                                  -> verify -> call_function on the ERASED body
        5. freeze_vm_result / vm_value_to_ct / vm_to_ct    Value -> CtValue
```

What that costs, beyond the checker run inside the elaborator that P4
cannot keep:

- The subprogram is checked twice on the expression path and once on the
  others, and every evaluation repeats in every discovery round, since each
  round builds a fresh `Elab`. Nothing is cached.
- The VM runs the erased body. A value parameter is reified into the frame
  (`value_params`), so `rep[n - 1]()` under a runtime `if` *runs* where the
  pin expands without end (`conformance/probes/ctfe_plain_keyed_recursion.mojo`).
- The subprogram cannot mint an instance: every compile-time-keyed `def` is
  excluded (`conformance/probes/ctfe_keyed_recursion.mojo`, roadmap 3.121)
  and a keyed method is a trap stub
  (`conformance/probes/ctfe_calls_comptime_if_struct_method.mojo`, 3.120).
- Fuel is `const FUEL: usize = 100_000` in `comptime.rs`, reset per round,
  burned by `Elab::burn` per entry and per `comptime for` iteration and by
  the VM per instruction, frame, and block (`burn_ctfe`).
- The effect rule is `vm_ctfe_effectful_builtin`: `print` and `input`, plus
  a nested `struct`, `trait`, or `import`. Raising is caught only by the
  probe's non-raising signature, and only on the expression path.
- Crossing: `ct_to_vm` (`comptime.rs`) admits scalars, literals, `Bool`,
  `Str`, tuples, compile-time lists, fieldwise structs, SIMD values, and
  `DType`; `vm_to_ct` and `freeze_vm_result` (`ctfe.rs`) bring the same set
  back, a nominal `String` as `Str`, and reject pointer-backed values.

The crate graph the route runs on:

```text
mojito-vm ────────── mojito-checker, mojito-mir, mojito-analysis   (re-checks; lowers)
    ▲
mojito-native ────── mojito-mir, mojito-checked                     (no VM, no checker)
mojito-comptime ──── mojito-vm, mojito-checker                      (CTFE runs VmBackend)
    ▲
mojito (root) ────── everything; calls native::mono for both backends
```

`mojito-native` and `mojito-vm` do not depend on each other.
`native::mono::Specializer` (`crates/mojito-native/src/native/mono.rs`)
holds the only worklist: `queue: VecDeque<(InstanceKey, Bindings)>`,
`instances: Vec<(InstanceKey, String)>`, `output_functions`. Its identity is
`InstanceKey { template, arguments: Vec<InstanceArg>, owner }` with origins
erased, named by `instance_symbol`. Its one bound is `output_functions.len()
>= 4096` ("polymorphic recursion exceeded the 4096-instance budget"). It has
no state enum (a key is pending while queued, done when output), no VM, no
compile-time entry, and `ParamKind` has no node for a call: a parameter
expression that applies a function is folded by the AST elaborator before
MIR exists.

## Upstream

`Mojo/lib/Elaborator/` is one worklist that serves calls, parameter
evaluation, and layout alike.

- A compile-time application is the parameter operator `kgen.param.apply`.
  `IREvaluator::evaluateApplyLike` (`IREvaluator.cpp`) first calls
  `Elaborator::getConcreteFunction`, which is a worklist demand:
  `getOrCreateNode(vals, gen)` then `concretizeCallee`. If the callee is not
  concrete yet, the demander is suspended (`skipNode`) and resumed when it
  is. Then it looks up the interpreter cache keyed `(FuncOp,
  ParameterExprArrayAttr)` (`lookupCachedInterpretation`), evaluates the
  concrete function on the `BytecodeInterpreter`, and writes the result back
  (`writeGlobalCachedInterpretation`, which asserts determinism).
- A node is `FRESH`, `IN_PROGRESS`, or `DONE` (`ParamNodeState`,
  `IREvaluatorContext.h`), and separately `NOT_DONE`, `DONE`, or `ERROR`.
- Two edge kinds (`ImplNode::dependencies` and `::blockers`,
  `IREvaluator.h`). A call in a body is a *dependency*: the caller finishes
  without waiting, and a strongly connected component of dependencies is
  simply broken (`diagnoseAndBreakRecursion`), so recursion is valid. A
  parameter-domain need — an application, a struct layout — is a
  *blocker*: the demander waits. An SCC that contains a blocker edge is the
  error "function instantiation in parameter domain that recursively
  requires itself", with the path as its causes (`buildRecursionError`).
- The instantiation depth bound `--elaboration-max-depth`
  (`include/Mojo/ToolCommon/CLOptions.h`) defaults to unlimited. The
  interpreter has no step budget.
- The parser never runs the interpreter. A module `comptime A_plus_one =
  add(A, 1)` is `lit.alias.decl` with the *expression* as its value
  (`LITOps.td`); operator expressions over literals fold as attributes
  (`KGENAttrsFolders.cpp`), an application stays "an unevaluated
  constructor apply that rebinding alone cannot fold".

## The request path

**A compile-time application is a parameter expression the elaborator
evaluates by demanding a concrete instance and running it on the VM.**

- `ParamKind::Apply { function: String, args: Vec<ParamExpr> }` is the one
  new node, specified with the register types over parameter expressions
  (roadmap 1.1). `function` is a callable symbol; `args` are its
  compile-time arguments, the binders in scope that the body reads.
- The checker lowers a compile-time expression that applies anything — a
  call, a constructor, a static method, a method chain on a compile-time
  value — as a zero-parameter thunk whose compile-time parameters are the
  enclosing binders, the way `CheckedConst::Evaluate { function }` already
  lowers a non-literal default argument. The expression's symbolic value is
  `Apply(thunk, binders)`, typed by the thunk's declared result. Runtime
  arguments (`f(7)`) live in the thunk's body; a generic callee's solved
  arguments are on the thunk's `Call`, as on any call (schema 1.13).
- `eval_ct` on `Apply` under the instance's bindings substitutes the
  arguments, builds the thunk's `InstanceKey`, and **demands** it: the
  instance is materialized now, and its *reference closure* — every
  instance its body and theirs enqueue — is drained on a nested worklist
  before the VM runs, since the VM needs every callee it may reach. The
  closure's functions, with those already completed, form a verified
  `ConcreteMir` fragment. `VmBackend::call_concrete(&ConcreteMir, name,
  args, fuel) -> (Value, fuel)` replaces the AST-taking
  `run_function_value`. The result crosses through `vm_to_ct` unchanged and
  is validated at the thunk's declared result type
  (`materialize_ct_value`), so a value the VM cannot freeze is the same
  error as today.
- The result is cached by the thunk's instance key, which is upstream's
  `(FuncOp, operands)`: the same application under the same bindings runs
  once per compilation.
- The path is entered wherever the AST elaborator calls `Elab::eval` on an
  application today: a `comptime if` or `comptime for` condition or range,
  a value argument (`scale[f(3)]()`), a module constant, a local `comptime`
  binding, an associated compile-time value, a `where` operand, and a
  `comptime(e)` crossing. A `comptime if` whose condition is an `Op` over
  binders (`n == 0`) never reaches the VM: the folder decides it.
- An evaluation may demand further instances without restarting anything:
  a demand inside a demand nests the worklist, as the pin's three `twice`
  instances behind `comptime K = k()` show
  (`conformance/probes/ctfe_demands_instances.mojo`).

## Keys and states

- The key stays `InstanceKey { template, arguments, owner }`, origins
  erased, with a `HashMap<InstanceKey, InstanceId>` beside the vector that
  today is scanned linearly. Canonical form: arguments substituted under
  the demander's bindings and value arguments materialized at their
  declared types, so `Apply(f, [IntLiteral 7])` and `Apply(f, [Int 7])`
  are one key.
- `enum InstanceState { Pending, Active, Completed, Failed }`. A key is
  `Pending` from enqueue, `Active` while `materialize` runs on it or on a
  demand it made, `Completed` when its function is output, `Failed` with
  its `MonoError` so that a second demand reports the same error without
  running again. Elaboration still stops at the first failure; the state
  exists for the cache and for the cycle check.
- The evaluation cache is `HashMap<InstanceKey, CtValue>` over thunk keys.
  A `Failed` thunk is a failed evaluation.

## Edges and cycles

- A **reference** edge is a `Call`, `MethodCall`, `MakeClosure`, or implicit
  enqueue inside a body: `enqueue` returns the instance's name at once and
  never waits. Recursion through reference edges is valid, as today
  (`assets/ok/vm_backed_ctfe_recursive_call.mojo`).
- A **demand** edge is a compile-time need: an `Apply` to evaluate, a
  layout to answer, a module constant to read. The demander cannot continue
  until the demanded key is `Completed`, so the demanded instance is
  materialized at once and its closure drained, nested.
- A demand on a key that is `Active` is a cycle, reported with upstream's
  words — "function instantiation in parameter domain that recursively
  requires itself" — and the demand stack as the path. A reference to an
  `Active` key is not. `comptime A = f(1)` whose `f` reads `comptime B = A
  + 1` is the cycle (`conformance/probes/ctfe_const_cycle.mojo`); today
  Mojito reports `Undefined variable 'B'` because constants evaluate in
  source order.
- Module constants are evaluated on demand, by the first reader, so their
  order is dependency order. A forward reference is valid, as at the pin.

## Bounds and fuel

- **Expansion**: no depth bound by default, as upstream (owner decision,
  2026-10-02). The instance count stays the one elaboration bound, as a
  named constant, counted at enqueue so that a wide explosion and a deep
  one both stop at it. It must trigger in seconds:
  `conformance/probes/mono_polymorphic_recursion.mojo` spins for more than
  five minutes in `enqueue → instance_symbol → encode_identifier` because
  each nested `W[W[…]]` name is built from the last, which roadmap 2.4
  fixes before the path lands. The pin runs out of memory on the same
  program.
- **Fuel** stays shared and separate from expansion: one counter per
  compilation, held by the elaborator, burned per request and handed to
  each `call_concrete`, which burns per instruction, frame, and block and
  returns the remainder. The per-round reset goes with the rounds. The
  quota error keeps its text.

## Effects, crossing, raising

- The effect rule is unchanged — `print`, `input`, and the three nested
  declarations — but decided on MIR: each instance records whether its
  body calls an effectful builtin, and a demand rejects a closure in which
  any instance does, with today's "is not safe for VM-backed compile-time
  execution". A trap or an uncaught raise inside the VM is today's "VM CTFE
  failed" error.
- A callee that may raise, applied in a compile-time position outside a
  `try`, is a checker error on the expression ("cannot call raising
  function in comptime initializer"), since the checker types the thunk
  body. The probe that finds it today goes.
- `ct_to_vm`, `vm_to_ct`, and `freeze_vm_result` move to `mojito-vm`,
  unchanged in what they admit. The residues under "Compile-time
  evaluation residues" in `docs/roadmap.md` stay residues.

## D2: where the executor lives

Decided 2026-10-02: **`mojito-native` depends on `mojito-vm`.** The
elaborator owns its executor, as upstream's `Elaborator` owns its
`BytecodeInterpreter`. The edge is allowed by the crate order in
`AGENTS.md`, where `mojito-native` is already listed below `mojito-vm`; it
is new, and it makes `mojito-checker` a transitive dependency of the
elaborator until the VM's `Backend::run(&CheckedProgram)` seam goes at P5.
`mojito-comptime`'s edge to `mojito-vm` is the same relationship one level
up and goes with the AST route. No `mojito-comptime → mojito-native` edge
is ever added: the AST route is deleted at the P4 entry, not bridged.

```text
mojito-vm ────────── mojito-mir, mojito-analysis, (mojito-checker until P5)
    ▲
mojito-native ────── mojito-vm, mojito-mir, mojito-checked    the elaborator + executor
    ▲
mojito (root) ────── calls native::mono from the driver, the artifact runner, and the CLI
```

The alternative not taken: a `ComptimeExecutor` trait declared in
`mojito-native` and implemented by the root crate, which keeps the crate
graph as it is at the cost of wiring an executor through `specialize`'s
three callers and every elaborator test.

## D3: module-scope `comptime` values

Decided 2026-10-02: **match the pin.** The evidence
(`conformance/probes/ctfe_const_*.mojo`):

| Probe | Pin | Mojito today |
| --- | --- | --- |
| `comptime M = f(7)`; `def g(x: SIMD[DType.float32, M])`; `g(SIMD[DType.float32, 8](…))` | rejects: `SIMD[.float32, 8]` cannot convert to `SIMD[.float32, f(Int(7))]` | rejects: `not a compile-time Int constant: M` |
| `comptime B = f(A)`; `comptime C = B * 2`; `SIMD[.., C]` on both sides | runs, `4 8` and `1` | rejects: `not a compile-time Int constant: C` |
| `comptime A = f(1)`, `f` reads `comptime B = A + 1` | rejects: cycle in the parameter domain | rejects: `Undefined variable 'B'` |

The boundary:

- A module constant is **folded before the check** when the `ParamExpr`
  folder closes its initializer: literals, other folded constants,
  operators, `DType` members, type names and applications, predicates.
  That is upstream's attribute folding, and it is what the checker's
  `eval_ct` and `register_module_alias` do today.
- A module constant whose initializer **applies anything** stays a typed
  request: the check types the initializer and declares the constant at
  that type; its value is `Apply(thunk)`, symbolic, equal by structure, so
  `SIMD[DType.float32, C]` matches itself and not `SIMD[DType.float32, 8]`,
  as at the pin. The elaborator evaluates it on first demand. Such a
  constant in a type position needs the register types over parameter
  expressions of roadmap 1.1; until they land, the source-validation
  rejection ("not a compile-time Int constant") stays, and roadmap 3.122
  records the program the pin runs.
- Today's early folding of applied constants by the AST elaborator stays as
  an implementation until the P4 entry deletes the route. It is contained:
  the validation pass rejects the one place where a folded `8` would type
  what the pin's symbolic `f(Int(7))` rejects.
- A struct's associated `comptime` member is not a module constant; it is
  a member of the generator, evaluated under the instance's bindings as
  today.

## What each entry takes from this

- **Roadmap 1.1** (register types over parameter expressions) specifies
  `ParamKind::Apply` and the symbolic type of an applied module constant.
- **Roadmap 1.2** (`comptime if`) lands the first code: the
  `mojito-native → mojito-vm` edge, `InstanceState`, the `HashMap` index,
  the demand stack and cycle error, the evaluation cache, the shared fuel,
  `VmBackend::call_concrete`, and the thunk lowering, with the condition
  as the first consumer. The MIR text schema bump for `Apply` is part of
  that entry's bump.
- **Roadmap 1.9** (clones minted during CTFE) and **3.120**, **3.121**
  close when a demand serves a keyed instance.
- **Roadmap 1.11** (P4) deletes the AST route, the early folding of applied
  constants, and the per-round fuel reset; `run_function_value` goes with
  it. **Roadmap 2.4** makes the instance budget reachable first.
