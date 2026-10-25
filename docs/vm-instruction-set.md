# Mojito VM Instruction Set

This document describes the instruction set executed by mojito's register VM in
the style of an assembly-language manual.

The VM does not currently decode a packed bytecode stream. It executes the
structured `MirInstr` and `MirTerm` values defined in `src/mir/ir.rs`. The
mnemonics below are a textual assembly spelling for those existing
operations; they do not define a separate implementation or binary encoding.
The normative artifact grammar, versioning, metadata, and canonical spellings
are defined by [`mir-text-format.md`](mir-text-format.md); where this execution
manual is illustrative, that schema is authoritative.

## Machine Model

A function owns two indexed storage spaces:

- **registers**, written `%r0`, `%r1`, and so on, hold expression temporaries
- **variable slots**, written `$v0`, `$v1`, and so on, hold parameters, source
  variables, and compiler-generated locals

Registers are function-local virtual registers. They are allocated densely but
are not physical machine registers. Variable slots have stable identities across
the function's body and its regions.

A function body is one block, as each region of a region op is:

```text
fn example(%parameters...) {
bb0:
    const.i64 %r0, 1
    var.store $v0, %r0 : Int
    var.copy %r1, $v0
    return %r1
}
```

Every block contains zero or more ordinary instructions followed by exactly one
terminator. Terminators transfer control and are documented separately.

## Assembly Notation

| Notation | Meaning |
|---|---|
| `%rN` | virtual register `N` |
| `$vN` | variable slot `N` |
| `bbN` | basic block `N` |
| `@name` | function, constructor, builtin, or resolved method symbol |
| `: Type` | optional type annotation used for coercion |
| `[$v0.field]` | a place rooted at a variable slot |
| `[$v0.items[%r2].x]` | a place with field and index projections |
| `[$v0.storage[2]]` | a statically selected compiler-private Tuple element |
| `{...}` | structured metadata, not an evaluated operand |
| `[...]` | a list of operands or optional operands |

A **place** identifies mutable storage. Its root is a variable slot and its
projection chain contains named fields, register-valued indices, and static
indices for compiler-private heterogeneous Tuple storage:

```text
[$v0]
[$v0.field]
[$v0.items[%r3].value]
[$v0.storage[2]]
```

A place is different from the value currently stored there. Place instructions
can read, write, or move from that storage without reevaluating its index
expressions. Static Tuple indices are distinct ownership paths; dynamic indices
conservatively overlap every static index. A place may also carry static
`through` metadata naming the retained reference slot through which it is
accessed; the compact notation omits this analysis-only field.

## Instruction Summary

### Constants and variable transfer

| Mnemonic | MIR operation | Purpose |
|---|---|---|
| `const.*` | `Const` | Load a literal into a register |
| `var.copy` | `UseVar(Copy)` | Copy a variable value into a register |
| `value.copy` | `CopyValue` | Run checked `Copyable` value semantics for an SSA value produced by a consuming reference or projected-place read |
| `value.rebind` | `Rebind` | Read a generator's `rebind[Dest](x)` operand at `Dest`; erased per instance by the elaborator, a verifier finding on concrete MIR |
| `var.move` | `UseVar(Move)` | Move a variable value into a register |
| `var.forward` | `UseVar(Forward)` | Hand a variable's value to an owned parameter or a conditional's consumer |
| `var.borrow` | `UseVar(BorrowShared)` | Read through a shared borrow |
| `var.borrow_mut` | `UseVar(BorrowMut)` | Read through an exclusive borrow |
| `var.store` | `DefVar` | Define or redefine a variable slot |

### Scalar computation

| Mnemonic | MIR operation | Purpose |
|---|---|---|
| `neg` | `UnOp(Neg)` | Arithmetic negation |
| `not` | `UnOp(Not)` | Logical negation |
| `add` through `not_in` | `BinOp` | Binary arithmetic, comparison, logic, or membership |
| `literal.materialize` | `MaterializeLiteral` | Convert an exact numeric literal once to its checked runtime scalar type |

### Calls

| Mnemonic | MIR operation | Purpose |
|---|---|---|
| `call` | `Call` | Call a free function, constructor, or builtin |
| `closure.make` | `MakeClosure` | Materialize a lifted function's checked capture environment |
| `call.indirect` | `CallIndirect` | Invoke a function, closure, or nominal callable value |
| `call.method` | `MethodCall` | Invoke a method on a receiver |

### Places and aggregate access

| Mnemonic | MIR operation | Purpose |
|---|---|---|
| `field.get` | `GetField` | Read a named field from a register value |
| `index.get` | `Index` | Execute the complete selected `__getitem__` contract, or read an intrinsic indexed element |
| `index.multi` | `MultiIndex` | Execute the complete selected `__getitem__` contract with mixed index/slice arguments |
| `index.multi_set` | `MultiSet` | Execute the complete selected `__setitem__` contract (including one-index assignment) and write back its mutable receiver, even through a caller reference handle |
| `place.load` | `LoadPlace` | Read a place without reevaluating it |
| `place.store` | `Store` | Write a value through a place |
| `place.store_ref` | `StoreRef` | Initialize reference-valued storage with an existing handle |
| `place.move` | `MovePlace` | Move a value out of a place |
| `pointer.take` | `PointerStorageTake` | Move an initialized element out of private pointer storage |
| `pointer.destroy` | `PointerStorageDestroy` | Destroy an initialized element in private pointer storage |

### Aggregate construction

| Mnemonic | MIR operation | Purpose |
|---|---|---|
| `tuple.make` | `MakeTuple` | Construct compiler-private heterogeneous pack storage |
| `variant.make` | `MakeVariant` | Construct a checked tagged-union alternative, moving its payload in |
| `variant.is` | `VariantIs` | Test a checker-selected Variant tag |
| `variant.get` | `VariantGet` | Project a checked Variant alternative |
| `variant.set` | `VariantSet` | Replace and destroy a Variant payload through a place, moving the new one in |
| `variant.take` | `VariantTake` | Move a Variant payload out of an owned value |
| `variant.replace` | `VariantReplace` | Replace a Variant payload, moving the new one in, and return the old payload |
| `simd.make` | `MakeSimd` | Construct or splat a SIMD value |

### Iteration

| Mnemonic | MIR operation | Purpose |
|---|---|---|
| `iter.init` | `GetIter` | Call a value's `__iter__` once, into an iterator |
| `iter.has_next` | `HasNext` | Test compiler-private iterator storage for another element |
| `iter.next` | `Next` | Advance compiler-private iterator storage |

### Exceptions and structured regions

| Mnemonic | MIR operation | Purpose |
|---|---|---|
| `try` | `Try` | Execute structured try/except/else regions, a raise naming the `try` entering its handler |
| `unsupported` | `Unsupported` | Report an explicitly unsupported operation |

`if`, `loop`, `comptime.if`, and `comptime.for` (`If`, `Loop`, `ComptimeIf`,
`ComptimeFor`) are the structured `hlcf` region ops, and `match` (`Match`)
is one too. Concrete MIR keeps `if`, `loop`, and `match`, which the VM steps
with each frame's region cursor; the elaborator decides every `comptime.if`
and `comptime.for`, and the VM rejects one that survives as unsupported.

### Lifetime operations

| Mnemonic | MIR operation | Purpose |
|---|---|---|
| `loans.establish` | `EstablishLoans` | Establish one grouped reference/aggregate loan generation; absent from concrete MIR (erased by `lower-lit`), so the VM refuses one |
| `interiors.invalidate` | `InvalidateInteriors` | Invalidate matching collection-owned interior generations, optionally replacing the exact named generation; absent from concrete MIR (erased by `lower-lit`), so the VM refuses one |
| `ref.make` | `MakeRef` | Materialize or extend a verified frame/slot reference handle |
| `ref.read` | `ReadRef` | Read through a runtime reference handle |
| `ref.write` | `WriteRef` | Write through a runtime reference handle |
| `lifetime.keep_alive` | `KeepAlive` | Extend an owner's MIR live range without copying it; absent from concrete MIR (erased by `lower-lit`), so the VM refuses one |
| `ownership.mark_initialized` | `MarkInitialized` | Mark a place initialized without writing it; absent from concrete MIR (erased by `lower-lit`), so the VM refuses one. A struct or tuple slot gets its unwritten storage where an instruction first stores into, reads, addresses, or passes it by address |
| `ownership.mark_destroyed` | `MarkDestroyed` | End a place's value without destroying it; absent from concrete MIR (erased by `lower-lit`), so the VM refuses one |
| `drop.var` | `DropVar` | Destroy the value in a variable slot, running the destructor the drop names |
| `consume.var` | `ConsumeVar` | End a slot whose parts drop elaboration has already destroyed (a `deinit` receiver at each exit, upstream's exit `mark_destroyed`); destroys nothing |
| `drop.reg` | `Drop` | Destroy an owned register's value |

Verification requires every `ref.make` destination to be a reference capability
whose target and permission are available from the source place. `ref.read` must
produce the capability's referent type, and `ref.write` requires mutable
permission plus a compatible value. Borrowing ordinary reference-valued storage
therefore creates an outer handle, while extending a handle rooted in an
existing `ref` or origin-bearing Pointer preserves that root's permission.
Every analytical `MirPlace::through` also names a checked reference-capability
slot; a substituted local alias retains that type even when the VM stores no
independent handle payload for it. Indexing `List[ref T]` initially produces an
outer handle to reference-valued element storage; lowering peels that layer
before augmented write-through or chained receiver dispatch, preserving the
stored handle and operating on its referent.

### Terminators

| Mnemonic | MIR terminator | Purpose |
|---|---|---|
| `return` | `Return` | Return from a function or structured region |
| `try.yield` | `TryYield` | Complete a try sub-region normally |
| `raise` | `Raise` | Raise an error value to the handler of the `try` its label names, or out of the function |

The structured region terminators — `yield`, `if.elifcond.yield`, `break`,
`continue`, `match.next`, `match.complete`, `comptime.yield`,
`comptime.for.break`, `comptime.for.continue`, and `unreachable` — end only
the regions of the ops above. The VM's region cursor runs those of runtime
control flow, and rejects a compile-time one, or a reached `unreachable`,
as unsupported.

## Constants and Variable Transfer

### `const.*` — Load Constant

```text
const.i64  %dest, integer
const.f64  %dest, float
const.int_literal   %dest, arbitrary-precision-integer
const.float_literal %dest, exact-finite-float
const.bool %dest, true|false
const.str  %dest, "text"
const.none %dest
```

Loads a constant into `%dest`. Source numeric literals use the exact
`IntLiteral`/`FloatLiteral` classes; concrete `Int`/`Float64` constants remain
available for compiler-synthesized runtime values. Boolean, String, function,
and None constants are concrete.

Examples:

```text
const.i64  %r0, 42
const.f64  %r1, 3.5
const.int_literal   %r2, 1606938044258990275541962092341162602522202993782792835301376
const.float_literal %r3, 1.000000059604644775390625
const.bool %r4, true
const.str  %r5, "mojito"
const.none %r6
```

### `literal.materialize` — Materialize Exact Numeric Literal

```text
literal.materialize %dest, %literal, target-type
```

Converts an exact `IntLiteral` or `FloatLiteral` register at the contextual
boundary selected by the checker. Integer targets use destination-width
two's-complement wrapping; floating targets round directly from the exact value
to binary32 or binary64. The verifier rejects a concrete source, a literal-only
target, or an incompatible integer/float target combination.

### `var.copy` — Copy Variable

```text
var.copy %dest, $source [constructor @symbol]
```

Copies the value in `$source` into `%dest` without emptying the variable slot.
A copy of a struct with a `__copyinit__` names it, as upstream's copy
insertion leaves a call to the copy constructor at each copy, and the VM
calls that function, `__init__(out self, *, copy: Self)`; a copy naming none
recursively copies the value's contents, each field by its own type. Reading a moved slot is a runtime error and should already have been
rejected by ownership analysis.

### `value.copy` — Copy Register Value

```text
value.copy %dest, %source [constructor @symbol]
```

Runs the checked value-semantics copy operation for an SSA register, through
the copy constructor it names as `var.copy` does. In
particular, an ordinary value use of a reference-returning expression emits
`ref.read` followed by `value.copy`, so a pointer-backed nominal collection runs
its copy lifecycle instead of becoming an accidental alias. An explicit
`ref alias = expression` retains the reference handle and does not emit this
copy. A checker-proven consuming field read also emits `place.load` followed by
`value.copy`; borrowed receiver, formatting, and iteration loads stay
handle-preserving and omit it.

### `value.rebind` — Rebind a register to its target type

```text
value.rebind { dest: %r1, value: %r0 }
```

`rebind[Dest](x)` read as a value in a generator: `dest` is typed `Dest`,
`value` the operand's own type, as upstream's `kgen.rebind`. Only a template
holds one. The elaborator asserts the two types equal for each instance a
call reaches, after deciding its `comptime if`s, and replaces `dest` by
`value`; a mismatch fails the instance ("rebind input type 'String' does not
match result type 'Int'"). Concrete MIR, the VM, and the native backend
never see it: `verify_concrete` rejects one, and the VM reports it as an
invariant error. A rebound place needs no instruction: its terminal type
is `Dest`.

### `var.move` — Move Variable

```text
var.move %dest, $source [constructor @symbol]
```

Transfers the value from `$source` into `%dest`. The source slot becomes a moved
tombstone. A move of a struct with a `__moveinit__` names it, as upstream's
move insertion does, and the VM calls that function,
`__init__(out self, *, deinit move: Self)`; a move naming none relocates the
value as it is. A later read or second move from the source is an error.

### `var.forward` — Forward Variable

```text
var.forward %dest, $source
```

Transfers the value from `$source` into `%dest` for an owned (`var`/`deinit`)
parameter of the call that consumes `%dest`, or from a conditional
expression's result temporary to its consumer, as `var.move` does, but runs
no custom move initializer: the consumer takes the source's storage.

### `var.borrow` — Shared-Borrow Read

```text
var.borrow %dest, $source
```

Reads `$source` under the MIR `BorrowShared` use mode. The current VM represents
this value similarly to a non-moving read; the mode exists so ownership and
borrow analysis can distinguish the operation before execution.

### `var.borrow_mut` — Mutable-Borrow Read

```text
var.borrow_mut %dest, $source
```

Reads `$source` under the MIR `BorrowMut` use mode. Static analysis enforces
exclusive access. Runtime mutation through reference-bearing `mut` and `ref`
parameters uses frame/slot handles. Ordinary non-reference mutable parameters
may still use caller-place write-back as a value-ABI implementation detail.

### `var.store` — Define Variable

```text
var.store $dest, %source
var.store $dest, %source : Type
```

Defines or redefines `$dest` from `%source`.

With a type annotation, the VM coerces the value to that declared type. Without
one, it coerces like the value already in the slot when applicable. In ownership
analysis this is a definition: storing into a moved variable reinitializes it.

## Scalar Computation

### `layout.size_of` — Target-layout byte size

```text
layout.size_of { dest: %r0, type: Padded }
```

Produces an `Int` from the shared native ABI layout engine. The checked type is
part of the instruction, and verification rejects types without a concrete
runtime layout; the VM does not infer layout from a runtime value. Only a
generator carries the query: the elaborator answers it under the compilation's
native target (`native::mono`), so concrete MIR holds the constant; the
instruction is a verifier finding on concrete MIR.

### `type.name` — Unqualified spelling of a parametric type

```text
type.name { dest: %r0, type: Box[T] }
```

Produces a `StringLiteral`: the unqualified spelling of a type that names a
compile-time parameter (`_unqualified_type_name[Self]()` in a generic
struct's method). Only a template holds one, and verification rejects a
concrete type, whose name lowers to a `const`. The elaborator substitutes the
type and replaces the instruction with that constant, so concrete MIR and
the native backend never see it; it is a verifier finding on concrete MIR.

### Unary instructions

```text
neg %dest, %operand
not %dest, %operand
```

`neg` performs arithmetic negation. `not` performs logical negation according to
the runtime truth-value rules.

### Binary instructions

All binary operations have the form:

```text
opcode %dest, %left, %right
```

| Mnemonic | Source operation | Struct dispatch |
|---|---|---|
| `add` | `a + b` | `a.__add__(b)` |
| `sub` | `a - b` | `a.__sub__(b)` |
| `mul` | `a * b` | `a.__mul__(b)` |
| `div` | `a / b` | `a.__truediv__(b)` |
| `floor_div` | `a // b` | `a.__floordiv__(b)` |
| `mod` | `a % b` | `a.__mod__(b)` |
| `pow` | `a ** b` | `a.__pow__(b)` |
| `eq` | `a == b` | `a.__eq__(b)` |
| `ne` | `a != b` | `a.__ne__(b)` |
| `lt` | `a < b` | `a.__lt__(b)` |
| `gt` | `a > b` | `a.__gt__(b)` |
| `le` | `a <= b` | `a.__le__(b)` |
| `ge` | `a >= b` | `a.__ge__(b)` |
| `and` | `a and b` | no dunder dispatch |
| `or` | `a or b` | no dunder dispatch |
| `in` | `a in b` | `b.__contains__(a)` |
| `not_in` | `a not in b` | negated `b.__contains__(a)` |

Primitive values are handled by the shared runtime arithmetic implementation.
For most operators, a struct in the left operand dispatches to its corresponding
dunder method. Membership dispatches on the right operand.

Short-circuit evaluation of source `and` and `or` is normally expressed through
control flow before MIR execution. The binary opcode remains part of the
underlying operation set.

## Function and Method Calls

### `call` — Free Call or Construction

```text
call %dest, @function(%r0, %r1)
call %dest, @function(%r0, name=%r1)
call %dest, @Generic[value=%r2](%r0)
```

Invokes a free function, builtin, or struct constructor and stores its result in
`%dest`.

The encoded operation carries more information than the compact spelling shows:

- ordered positional argument registers
- named keyword argument registers
- an optional caller place corresponding to each positional and keyword
  argument
- optional compile-time value-parameter registers
- the checker-resolved lowered symbol for overloaded calls
- for a raising call, its raise edge: the checker-selected concrete error
  type, and the error flag and error registers the call defines

A raising call does not raise itself: as upstream's throwing call returns
its error flag, it sets its flag to `True` and its error to the raised
value when the callee raises, and the flag to `False` when it returns, its
result then defined. Its error region follows it in the block, an `if` on
the flag whose `then` region raises the error; a child frame's `raise`
pops into the caller's call so. Every raising instruction — `call.indirect`,
`call.method`, an operator whose dunder raises, a subscript whose accessor
does — has a raise edge the same way.

Caller places bind `mut` and `ref` parameters to frame/slot handles. Keyword
places remain aligned with their source arguments and are selected by parameter
name after structural binding. Type parameters are erased at runtime; value
parameters can be reified as function locals or struct metadata.

Calls may dispatch to builtins, user functions, fieldwise constructors,
hand-written `__init__`, or copy constructors. Argument binding handles required,
default, positional-only, keyword-only, and variadic parameters. Homogeneous
keyword collectors use the same ABI for free, generic, instance, static, and
bounded-trait calls: the call passes the owned `StringDict` it builds, or the
one a `**kwargs^` forwards, as its one `**` keyword operand, which binds the
collector slot whole.

A specialized heterogeneous `*args^` forwarding call may carry one private
heterogeneous-pack register after its fixed positional prefix. The target specialization
binds that register directly to its concrete pack slot, so forwarding relocates
the complete collector rather than issuing projected moves for its elements.
Keyword/default tail arguments remain ordinary named slots. Multiple spreads
and explicit positional overflow after the spread are rejected before MIR.

### `closure.make` — Materialize Closure Environment

```text
closure.make %dest, @outer$middle$inner, [$x: imm, $y: mut, $z: var]
```

Builds a non-escaping closure value for a recursively lifted MIR function. The
compact spelling shows source capture conventions; the structured instruction
contains the checker-resolved owner place, exact storage type, and storage mode
for every environment slot. Lifted capture parameters carry those concrete
types even when a capture is used only by a descendant or is explicitly unused.
`imm`, `mut`, and `ref` slots retain frame/slot handles, `var` slots copy their
value, and moved slots transfer it. Materialization occurs at a first-class
use of the nested `def` (a function value passed or stored, a type
application), or when the declaration executes for a body that copies or
moves a capture or that a sibling captures, so an owned snapshot is
independent of later outer assignment and a moved outer binding is
unavailable immediately; a call by name is a direct `call` of the lifted
function passing the captured places as its leading arguments and makes no
closure. A retained
reference handle does not by itself establish a declaration-to-call loan; its
access convention is enforced when the closure is used. The verifier checks each
capture place and the lifted function's leading environment slots. A captured
sibling slot contains its already materialized closure; it is not reconstructed
from or expanded into the sibling's transitive captures.

### `call.indirect` — Callable Value

```text
call.indirect %dest, %callee(%r0, %r1)
```

Invokes the function, closure, or nominal callable value in `%callee`. Function
values carry a resolved MIR symbol; arguments use the target's normal signature
and execution pushes the same explicit frame as a direct user-function call.
The instruction retains positional and keyword caller places, an optional
callable place for a `mut`/`ref __call__` receiver, the callable type's selected
error contract, source-ordered compile-time arguments, the selected generic
callable contract declarations, and a checker-selected nominal target. A
compile-time argument retains its optional source name independently of its
optional register, so a named value can skip an earlier default while an erased
type argument still occupies its selected source slot. Omitted values are
resolved from the anonymous contract—not the runtime implementation—including
partial scalar defaults and symbolic selected-function, conditional, and
earlier-callable defaults. A concretely specialized instruction also carries a
semantic-only declaration-order type/value argument witness and its checked
monomorphic callable contract. The verifier reapplies that witness and requires
dependent indexed parameter/result types to resolve before checking executable
registers; it never infers compile-time values by scanning preceding constant
instructions. Symbolic dependent types are permitted only for calls which stay
under their generic callable's explicit value binders. The verifier checks this
parameter ABI before the VM normalizes it to declaration order. For a statically known
callable struct this is the exact lowered `Type.__call__$ov$Signature` symbol; a
`def(...)` value carries an abstract symbol with the same signature suffix,
which is retargeted only if the runtime value is nominal. Same-arity overloads
therefore never depend on arity-only lookup. Origin-specialized function values
and nominal callable contracts retain reference parameter/result signatures as
checked metadata; the VM consumes their already-resolved caller handles without
dynamically reconstructing origins.

### `call.method` — Method Call

```text
call.method %dest, %receiver, method(%r0, %r1)
call.method %dest, %receiver, @Resolved.method$ov$Type(%r0)
```

Invokes a method on `%receiver` and stores the result in `%dest`.

The instruction carries:

- the source method name
- an optional statically resolved overload symbol
- ordinary argument registers
- an optional writable receiver place, and the callee's receiver
  convention (`receiver_convention`), which says whether the call consumes
  the place or the receiver register
- optional writable places for positional and keyword ordinary arguments
- an optional concrete error type selected from the method or trait requirement
- an optional `reference_result` describing the selected concrete reference ABI
  independently from `%dest`'s value type
- an optional verified `result_adapter` for an abstract result convention

For a `mut`/`ref self` method, the receiver place becomes a frame/slot handle.
`mut` and `ref` ordinary parameters likewise bind through their argument
places. Nominal prelude collections and user-defined structs share this checked
method instruction; only genuine scalar/runtime intrinsics take a separate
dispatch path.

A call whose `receiver_convention` is `deinit` consumes its receiver place, as upstream's call consumes a `DeinitMem`
operand: the callee owns the receiver and destroys its fields, and the VM
stores its ended-place tombstone into the place at the call, as a move
leaves one. No marker after the call ends the receiver.

Calls made by a structured `try` region use a temporary mirror with the caller's
real frame identity. This preserves the same handles, including nested projected
returns and mutations on raising paths, until the synchronous child completes.

An abstract value-result `__next__` carries `CopyIteratorReference`. Runtime
retargeting checks the concrete declaration ABI: a value return passes through,
while a `ref T` return is read and lifecycle-copied before reaching `%dest`.
Concrete method calls carry their exact `reference_result` and no adapter.

## Places and Aggregate Access

### `field.get` — Read Field

```text
field.get %dest, %base, field_name
```

Reads the named field of the struct-like value in `%base` into `%dest`. This is
an rvalue read. Writes and read-modify-write operations use place instructions.

### `index.get` — Read Element

```text
index.get %dest, %base, %index
```

Reads an indexed element from `%base`.

If overload selection chose an `Int` subscript for a non-Int `Indexer`, the
index expression has already been evaluated once and converted by an explicit
checker-selected `__mlir_index__` call. A direct accessor overload accepting the
source index type bypasses that normalization.

Supported checked/runtime paths include:

- nominal List/Tuple and user-struct indexing through selected accessors
- compiler-private Tuple/runtime-pack and variadic-argument storage
- VM-backed SIMD indexing
- pointer arena loads
- compile-time-list indexing during restricted CTFE execution

Bare String positional indexing is intentionally absent, matching current
Mojo's requirement to choose byte, code-point, or grapheme indexing explicitly;
those Unicode-aware forms belong to the self-hosted String roadmap task.

For a method-dispatched nominal receiver the instruction retains the complete selected-call
payload: target, executable result type, and typed error; receiver and argument
conventions/places,
capture effects, compile-time arguments, reference-result origin, and
write-back requirements. A parameter-only `__getitem_param__` has no ordinary
index argument; the already evaluated index register is reused in its
compile-time value-parameter ABI. An ordinary value consumer reads and copies a
returned referent, while an explicit `ref` binding retains its handle.

### Slice literals

A source slice is a call of its bound parameter type's `__slice_literal__`
constructor, as upstream's literal constructs its parameter's type: the
library `Slice`, `ContiguousSlice`, `StridedSlice`, or a user struct. The
subscript passes the constructed value as an ordinary `index.multi`
argument, so a selected `__getitem__` receives it like any index, and no
instruction builds a slice value of its own.

### `index.multi` / `index.multi_set` — Mixed Subscripts

```text
index.multi %dest, %object, [%row, %slice]
index.multi_set [$grid], [%row, %slice], %value
```

Every method-dispatched nominal variant carries the same `MirSubscriptCall` payload: exact lowered
target, executable result type, and error type; independent ABI place
requirements and effective
receiver/argument conventions, source-slot mapping for positional, keyword, and
default arguments, capture accesses, reference-result origin, and source-ordered
compile-time value arguments plus declarations. `Index.call` is absent only
when the instruction carries an explicit `MirIntrinsicSubscript`:
Tuple/runtime-pack storage, variadic storage, SIMD, pointer, or CTFE
compile-time-list indexing. Verification requires exactly one call or intrinsic;
runtime values never select the family. `MultiIndex.call` and `MultiSet.call`
are mandatory because the VM has no intrinsic multidimensional operation. The
source evaluation order for assignment is receiver, index and slice-bound
expressions, then right-hand side. Fixed setters receive the assignment value last,
while variadic
`*indices, *, value` methods receive it in the keyword-only `value` slot.

Verification treats retained `raises` as an ordinary protected or propagating
call site and checks exact argument-source binding, types, collectors, places,
captures, and reference results. Loan analysis classifies immutable conventions
as reads and mutable conventions as writes. When a returned reference becomes a
chained receiver/place, MIR stores its handle once in a hidden reference slot,
establishes its owner loans, and uses that slot as the place's `through` root.

Augmented nominal subscripts retain two distinct verified paths. A value getter
evaluates receiver and raw indices once, then the RHS, getter-specific argument
adaptation and getter call, the operator, setter-specific adaptation, and the
setter call. A mutable-reference getter instead runs before the RHS to establish
the lvalue, after which MIR reads and writes its handle directly and emits no
setter call.

### `place.load` — Read Place

```text
place.load %dest, [$v0.field[%r1]]
```

Reads a previously formed place into `%dest`. It is used for the read half of a
read-modify-write operation so index expressions are evaluated exactly once.

An indexed nominal struct, including the bundled List, dispatches through its
checker-selected `__getitem__`; a pointer place reads the heap arena. Ordinary
fields, compiler-private heterogeneous pack slots, and SIMD lanes use direct
place navigation.

### `place.store` — Write Place

```text
place.store [$v0.field[%r1]], %source
```

Writes `%source` through the destination place.

An indexed nominal struct, including the bundled List, dispatches through its
checker-selected `__setitem__` and writes the mutated receiver back. An indexed
pointer writes the VM heap arena. Ordinary variable, field,
nominal-struct-storage, and SIMD places are updated directly.

### `place.move` — Partial Move

```text
place.move %dest, [$v0.field]
place.move %dest, [$v0[1]]
```

Transfers a value out of a projected place into `%dest`. The source location is
replaced with a moved tombstone. This permits a field to be moved while leaving
sibling fields usable and ensures later destruction skips the moved field.
A compile-time element of compiler-private heterogeneous Tuple storage uses a
static `ConstIndex` projection, allowing distinct pack elements to move without
collapsing them into one dynamic-index alias class.

Ownership analysis must prove the place is initialized and not used again in an
invalid way. Moving an already moved place is a runtime error.

## Aggregate Construction

Public `List`, `Set`, `Dict`, `Range`, and `Tuple` values are nominal bundled
structs, so the VM has no public collection-construction instructions or native
collection values. Displays lower to ordinary constructor calls. Comprehension
leaves invoke the checked `append`, `add`, or `__setitem__` method after their
surrounding MIR blocks have encoded generator nesting and filters.

### `tuple.make` — Construct Private Heterogeneous Pack

```text
tuple.make %dest, [%r0, %r1, %r2]
```

Constructs compiler-private heterogeneous pack storage from the supplied
register values. Element types may differ. Public tuple construction is an
ordinary call to a concrete specialization of the nominal `Tuple[*Ts]` struct;
its private `__RuntimeTuple[*Ts]` backing field is the only source construct
that lowers to this operation.

### `simd.make` — Construct SIMD Value

```text
simd.make %dest, DType.Int32, 4, [%r0, %r1, %r2, %r3]
simd.make %dest, DType.Float64, 8, [%r0]
```

Constructs a SIMD value with the specified element type and width. Supplying one
element splats it across all lanes; otherwise the element count must match the
width. Scalar aliases can also lower through this operation with width one.

## Iteration Instructions

The iterator instructions mutate a variable slot because nominal iterator
structs carry iteration state. For consuming `for var item in collection^`, the
source has already moved into this slot; producing the next element transfers it
to the loop binding and leaves the slot owning only the residual iterator.
Concrete borrowed List, Set, and Dict place iteration first uses ordinary place-load
and loan instructions to retain the live source's `element` interior generation.
That is a checked concrete bridge, not a VM inference of the current generic
`IteratorType[origin]` contract.

### `iter.init` — Normalize Iterator

```text
iter.init $iterator
```

Normalizes `$iterator` for a value or owned `for` loop by following the
checker-selected `__iter__()` chain until it yields a nominal struct with
`__next__`, with a defensive iteration-depth limit. The iterator lands in a
slot of its own; an owned chain (`__iter__(var self)`) moves the source out
of its slot, leaving it moved. Bundled ranges and
collections take this same path as concrete user structs. A raising
`__iter__` is no `iter.init`: MIR calls it as a raising `call.method` with its
error region, as upstream's `emitForStmt` does, and stores the iterator into
the slot.

A typed-raising iterator has no `iter.*` advance either: a `for` loop calls
its `__next__(mut self)` as a raising `call.method` on the iterator slot,
which writes the final receiver back, inside a `try` whose handler runs the
loop's `else` body and breaks, as upstream's `emitForStmt` does. The `try`
body stores the element into the loop's `$yield` slot, so the exhausted edge,
which raises before the store, never fills it. An abstract value-result call
carries the verified `CopyIteratorReference` adapter, as any `call.method`
does: runtime dispatch consults the selected concrete declaration's ABI and,
when it returns `ref T`, reads and lifecycle-copies the `Copyable` referent
before writing the destination.

### `iter.has_next` — Test Compiler-Private Iterator Storage

```text
iter.has_next %dest, $iterator
```

Writes whether `$iterator` holds another element. The operand is restricted to
`Value::ComptimeList` while the VM is running in CTFE mode and private
`Value::Tuple` runtime-pack storage an owned loop (`for v in xs^`) consumes;
every nominal iterator, generic or concrete, advances through its raising
`__next__` call instead, and a borrowed loop over a runtime pack indexes it in place through
`ref.make` with no `iter.*` instruction. A public Tuple is a nominal struct
and cannot reach either path.

### `iter.next` — Advance Compiler-Private Iterator Storage

```text
iter.next %dest, $iterator
```

Removes the first element of the CTFE-only `Value::ComptimeList` or an owned
loop's private runtime-pack `Value::Tuple` in `$iterator` and writes it to
`%dest`.

Calling this instruction when no element remains is invalid; the generated loop
tests `iter.has_next` first.

## Exceptions and Structured Regions

### `try` — Structured Exception Region

```text
try try0 {
    body      { ... }
    except $error { ... }
    else      { ... }
}
```

Executes structured regions that share the enclosing function's registers
and variable slots. A source `finally` has no region here: MIR lowering
copied it before every exit of the parts, as upstream's `lowerTryFinally`
does, so it runs as ordinary instructions on each way out.

Semantics:

1. Execute `body`.
2. If a raise in `body` names this `try`'s label, optionally bind the error
   and execute the handler. A raise naming another `try`, or none, leaves
   the `try` untouched, as a `return` or a region exit does.
3. Execute `else` only if the body completed normally.

The body, handler, and else components are each one block, `bb0`.

### `unsupported` — Explicit Backend Failure

```text
unsupported "description"
```

Stops execution with a clean unsupported-operation error. Lowering emits this
instruction for recognized syntax whose runtime semantics are not implemented,
instead of panicking or silently executing the wrong behavior.

## Lifetime Instructions

### `drop.var` — Destroy Variable

```text
drop.var $variable [destructor @symbol]
```

Removes the value from `$variable`, leaving `None`, and destroys the removed
value. A drop of a struct with a `__deinit__` names it, as upstream's
destructor insertion leaves a call to the `__del__` at each death, and the
VM calls that function on the whole value; a struct naming none has its
fields dropped, each by its own type's destructor, in declaration order. A partly moved struct drops only its residual fields. `drop.place`
destroys one projected place the same way. Public collections are nominal structs and follow that rule.
Elements of the compiler-private heterogeneous pack carrier use Mojo's
left-to-right order. Moved fields and relocated pack storage are skipped at
their old owner, preventing double destruction after a partial move. This is
also how an early exit destroys the residual state of an owned iterator when
its elements are implicitly deletable.

Drop elaboration conservatively inserts this instruction for every owned root at
the variable's last use or on an appropriate control-flow edge. Runtime teardown
then follows the value recursively; this also releases heap-backed storage whose
destruction has no user-visible output.

### `drop.reg` — Register Drop

```text
drop.reg { reg: %register }
drop.reg { reg: %register, destructor: "S.__deinit__" }
```

Destroys the owned value in a register, upstream's direct `ValueToDestroy`,
and leaves the register moved. A named destructor is lowered before
execution to the `deinit`-receiver call on the register; the VM runs an
unnamed one by dropping the value's fields by their types, as `drop.var`
does. Drop elaboration places one at the death of each owned register no
operand takes (`docs/architecture.md` §What Gets Dropped).

## Block Terminators

Terminators appear only as the final operation of a basic block.

### `return` — Return Value

```text
return %value
return.none
```

Ends the current function and returns a register value or `None`.

Within a try sub-region, `return` becomes a non-normal flow value that
propagates through the enclosing regions to the function. Each `finally` it
crosses already ran before it, copied there by MIR lowering, and drop
elaboration destroyed before it what it still held.

### `try.yield` — Complete Region

```text
try.yield [%values...]
```

Marks normal completion of a try sub-region. It is not a valid ordinary function
terminator. The region runner translates it to normal flow and allows the
surrounding `try` instruction to continue with `else` or complete, binding
its values to the `else`'s arguments or the `try`'s results.

### `raise` — Raise Error

```text
raise %source [try0] [%values...]
```

Raises the value in `%source`. An `Error` or `String` supplies its message;
another value reports its runtime type. It ends any region, as upstream's
`raise` ends its block: the exceptional outcome leaves every enclosing
region for the handler of the nearest `try` carrying its label whose body
holds it (`lit.try.raise`), binding its values to that handler's
arguments, or, with no label, out of the current function
(`lit.error_return`). Drop elaboration has already destroyed, before it,
what that handler does not read.

## Call ABI and Function Metadata

The instruction stream is accompanied by function and declaration metadata.
This is part of the VM contract even though it is not an opcode.

Each function records:

- block list
- register count
- variable-slot count and diagnostic names
- number and types of leading parameter slots
- which parameters are owned
- which parameters are owned or reference-bearing and how arguments are passed
- source spans associated with generated registers

Program declaration metadata records:

- struct field layouts
- mutating method identities
- fieldwise-construction status
- function parameter names and types
- defaults and argument markers
- generic type and value parameter declarations

The VM constructs a frame by allocating the recorded register and variable-slot
counts, placing arguments in the leading variable slots, and reifying generic
value parameters into their named slots.

## Worked Example

For source shaped like:

```mojo
def add_one(x: Int) -> Int:
    return x + 1

def main():
    var n: Int = 4
    print(add_one(n))
```

a simplified assembly rendering is:

```text
fn @add_one($v0: Int) {
bb0:
    var.borrow %r0, $v0
    const.i64 %r1, 1
    add %r2, %r0, %r1
    return %r2
}

fn @main() {
bb0:
    const.i64 %r0, 4
    var.store $v0, %r0 : Int
    var.borrow %r1, $v0
    call %r2, @add_one(%r1)
    call %r3, @print(%r2)
    return.none
}
```

The exact use mode selected for an operand is determined by checking and
lowering. The example is explanatory rather than a golden dump format.

## Opcode Inventory

The categorized tables in [Instruction Summary](#instruction-summary) are this
manual's execution inventory. The frozen textual vocabulary and canonical
printer live in `mir::text`; `MirInstr` and `MirTerm` in `src/mir/ir.rs` remain
the in-memory authority. Keeping another flat opcode list here would drift when
the structured MIR evolves.
