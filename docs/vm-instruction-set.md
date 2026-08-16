# Mojito VM Instruction Set

This document describes the instruction set executed by mojito's register VM in
the style of an assembly-language manual.

The VM does not currently decode a packed bytecode stream. It executes the
structured `MirInstr` and `MirTerm` values defined in `src/mir/ir.rs`. The
mnemonics below are a human-readable assembly spelling for those existing
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
the function's control-flow graph.

A function is a list of basic blocks:

```text
fn example(%parameters...) {
bb0:
    const.i64 %r0, 1
    var.store $v0, %r0 : Int
    jump bb1

bb1:
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
| `var.move` | `UseVar(Move)` | Move a variable value into a register |
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
| `slice.get` | `Slice` | Execute the complete selected `__getitem__` contract with a checked slice descriptor, or use the temporary VM String intrinsic |
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
| `variant.make` | `MakeVariant` | Construct a checked tagged-union alternative |
| `variant.is` | `VariantIs` | Test a checker-selected Variant tag |
| `variant.get` | `VariantGet` | Project a checked Variant alternative |
| `variant.set` | `VariantSet` | Replace and destroy a Variant payload through a place |
| `variant.take` | `VariantTake` | Move a Variant payload out of an owned value |
| `variant.replace` | `VariantReplace` | Replace a Variant payload and return the old payload |
| `simd.make` | `MakeSimd` | Construct or splat a SIMD value |

### Iteration

| Mnemonic | MIR operation | Purpose |
|---|---|---|
| `iter.init` | `GetIter` | Normalize a value into an iterator |
| `iter.try_next` | `TryNext` | Advance a typed-raising iterator, treating checked `StopIteration` as exhaustion |
| `iter.has_next` | `HasNext` | Test a legacy nonraising iterator for another element |
| `iter.next` | `Next` | Advance a legacy nonraising iterator |

### Exceptions and structured regions

| Mnemonic | MIR operation | Purpose |
|---|---|---|
| `raise` | `Raise` | Raise an error value |
| `try` | `Try` | Execute structured try/except/else/finally regions |
| `unsupported` | `Unsupported` | Report an explicitly unsupported operation |

### Lifetime operations

| Mnemonic | MIR operation | Purpose |
|---|---|---|
| `loans.establish` | `EstablishLoans` | Establish one grouped reference/aggregate loan generation; a VM no-op after ownership checking |
| `interiors.invalidate` | `InvalidateInteriors` | Invalidate matching collection-owned interior generations, optionally replacing the exact named generation; a VM no-op after ownership checking |
| `ref.make` | `MakeRef` | Materialize or extend a verified frame/slot reference handle |
| `ref.read` | `ReadRef` | Read through a runtime reference handle |
| `ref.write` | `WriteRef` | Write through a runtime reference handle |
| `lifetime.keep_alive` | `KeepAlive` | Extend an owner's MIR live range without copying it; a VM no-op after checking |
| `drop.var` | `DropVar` | Destroy the value in a variable slot |
| `consume.var` | `ConsumeVar` | Consume an explicitly destroyed aggregate after its named destructor succeeds; then destroy its fields in reverse order |
| `consume.place` | `ConsumePlace` | Consume one projected subobject after its named destructor succeeds |
| `drop.reg` | `Drop` | Reserved register-drop operation |

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
| `jump` | `Jump` | Unconditional block transfer |
| `branch` | `Branch` | Conditional block transfer |
| `return` | `Return` | Return from a function or structured region |
| `return.cleanup` | `ReturnWithCleanup` | Return after pending `finally` regions, then destroy retained loop owners |
| `falloff` | `FallOff` | Complete a try sub-region normally |
| `escape` | `EscapeJump` | Leave a try region for an enclosing loop target |

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
var.copy %dest, $source
```

Copies the value in `$source` into `%dest` without emptying the variable slot.
For lifecycle-aware structs this may invoke the checked
`__init__(out self, *, copy: Self)` path; aggregate copies recursively copy their
contents. Reading a moved slot is a runtime error and should already have been
rejected by ownership analysis.

### `value.copy` — Copy Register Value

```text
value.copy %dest, %source
```

Runs the checked value-semantics copy operation for an SSA register. In
particular, an ordinary value use of a reference-returning expression emits
`ref.read` followed by `value.copy`, so a pointer-backed nominal collection runs
its copy lifecycle instead of becoming an accidental alias. An explicit
`ref alias = expression` retains the reference handle and does not emit this
copy. A checker-proven consuming field read also emits `place.load` followed by
`value.copy`; borrowed receiver, formatting, and iteration loads stay
handle-preserving and omit it.

### `var.move` — Move Variable

```text
var.move %dest, $source
```

Transfers the value from `$source` into `%dest`. The source slot becomes a moved
tombstone. If the value's type defines
`__init__(out self, *, deinit move: Self)`, the VM performs that custom move
initialization. A later read or second move from the source is an error.

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
- an optional checker-selected concrete error type for a raising call

Caller places bind `mut` and `ref` parameters to frame/slot handles. Keyword
places remain aligned with their source arguments and are selected by parameter
name after structural binding. Type parameters are erased at runtime; value
parameters can be reified as function locals or struct metadata.

Calls may dispatch to builtins, user functions, fieldwise constructors,
hand-written `__init__`, or copy constructors. Argument binding handles required,
default, positional-only, keyword-only, and variadic parameters. Homogeneous
keyword collectors use the same ABI for free, generic, instance, static, and
bounded-trait calls; a `**kwargs^` entry expands an owned `StringDict` before
structural binding.

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
value, and moved slots transfer it. Materialization occurs when the nested
declaration executes, so an owned snapshot is independent of later outer
assignment and a moved outer binding is unavailable immediately. A retained
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
- an optional writable receiver place
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

### `slice.get` — Slice Value

```text
slice.get %dest, %object, [%lower:%upper:%step]
```

Constructs a slice descriptor and executes the selected nominal accessor, or
slices a VM-backed String. The accessor result, not the descriptor, is stored in
`%dest`. Each bound is either a register or `_` for an omitted, direction-aware
default:

```text
slice.get %r4, %r0, [%r1:%r2:_]
slice.get %r5, %r0, [_:_:%r3]
```

A source slice is represented by its checker-selected `ContiguousSlice` or
`StridedSlice` descriptor. User receivers receive that first-class descriptor
through their selected `__getitem__`; this includes the bundled nominal List and
does not bypass its ordinary method. Checker-created descriptors may widen only
within the descriptor family; they never carry an arbitrary user `@implicit`
conversion for the VM to reconstruct. The VM-backed String path normalizes
bounds directly. A zero step or invalid bound produces a runtime error.

### `index.multi` / `index.multi_set` — Mixed Subscripts

```text
index.multi %dest, %object, [%row, stride(%lo:%hi:%step)]
index.multi_set [$grid], [%row, stride(%lo:%hi:%step)], %value
```

Every method-dispatched nominal variant carries the same `MirSubscriptCall` payload: exact lowered
target, executable result type, and error type; independent ABI place
requirements and effective
receiver/argument conventions, source-slot mapping for positional, keyword, and
default arguments, capture accesses, reference-result origin, and source-ordered
compile-time value arguments plus declarations. `Index.call` and `Slice.call`
are absent only when the instruction carries an explicit
`MirIntrinsicSubscript`: Tuple/runtime-pack storage, variadic storage, SIMD,
pointer, or CTFE compile-time-list indexing for `Index`, and temporary VM String
slicing for `Slice`. Verification requires exactly one call or intrinsic;
runtime values never select the family. `MultiIndex.call` and `MultiSet.call`
are mandatory because the VM has no intrinsic multidimensional operation. The
source evaluation order for assignment is receiver, index and slice-bound
expressions, then right-hand side. Fixed setters receive the assignment value last,
while variadic
`*indices, *, value` methods receive it in the keyword-only `value` slot.

`MirIntrinsicSubscript::TupleStorage` also names the narrow ABI bridge for a
nominally typed `Slice.indices()` result that is still carried as transient
private `Value::Tuple` storage. This exception is explicit in MIR and does not
enable runtime family inference for arbitrary nominal values.

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
`__next__`, with a defensive iteration-depth limit. Bundled ranges and
collections take this same path as concrete user structs.

### `iter.try_next` — Advance A Typed-Raising Iterator

```text
iter.try_next %dest, %yielded, $iterator, @method, StopIteration
```

Calls the checker-selected typed-raising `__next__(mut self)` exactly once and
writes the final receiver back into `$iterator`. On success, `%dest` receives the
element and `%yielded` is `True`. Raising exactly the retained exhaustion type
sets `%yielded` to `False`; any other raised value propagates. Current bundled
iterators use this path. An abstract value-result call may carry the verified
`CopyIteratorReference` adapter: runtime dispatch consults the selected concrete
declaration's ABI and, when it returns `ref T`, reads and lifecycle-copies the
`Copyable` referent before writing `%dest`. Exhaustion is never interpreted as an
adapter failure.

### `iter.has_next` — Test A Legacy Iterator

```text
iter.has_next %dest, $iterator
```

For the compatibility nonraising protocol, writes whether `$iterator` can
produce another element by calling the checker-selected `__len__()` and testing
whether its `Int` result is positive. Method-free fallbacks are restricted to
`Value::ComptimeList` while the VM is running in CTFE mode and private
`Value::Tuple` runtime-pack storage. A public Tuple is a nominal struct and
cannot reach either path.

### `iter.next` — Advance A Legacy Iterator

```text
iter.next %dest, $iterator
```

On the compatibility nonraising path, calls the checker-selected
`__next__(mut self)`, writes the produced element to `%dest`, and writes the
final nominal receiver back into the iterator slot. The CTFE-only
`Value::ComptimeList` and private runtime-pack `Value::Tuple` fallbacks instead
remove their first element.

Abstract value-result calls use the same verified `CopyIteratorReference`
adapter described for `iter.try_next`; concrete calls retain their exact value
or reference result and never carry an adapter.

Calling this instruction when no element remains is invalid; the generated loop
tests `iter.has_next` first.

## Exceptions and Structured Regions

### `raise` — Raise Error

```text
raise %source
```

Raises the value in `%source`. An `Error` or `String` supplies its message;
another value reports its runtime type. The exceptional outcome propagates to
the nearest enclosing `try` handler or out of the current function.

### `try` — Structured Exception Region

```text
try {
    body      { ... }
    except $error { ... }
    else      { ... }
    finally   { ... }
    cleanup   [$v3, $v4]
}
```

Executes structured mini-CFG regions that share the enclosing function's
registers and variable slots.

Semantics:

1. Execute `body`.
2. If `body` raises and an `except` region exists, drop the body-local cleanup
   slots, optionally bind the error, and execute the handler.
3. Execute `else` only if the body completed normally.
4. Execute `finally` on normal completion, raise, return, break, or continue.
5. A non-normal result from `finally` overrides the pending result.

The body, handler, else, and finally components are each local basic-block
graphs whose entry is block zero.

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
drop.var $variable
```

Removes the value from `$variable`, leaving `None`, and destroys the removed
value. For a struct this can invoke `__deinit__`; fields are then dropped in reverse
declaration order. Public collections are nominal structs and follow that rule.
Elements of the compiler-private heterogeneous pack carrier use Mojo's
left-to-right order. Moved fields and relocated pack storage are skipped at
their old owner, preventing double destruction after a partial move. This is
also how an early exit destroys the residual state of an owned iterator when
its elements are implicitly deletable.

Drop elaboration conservatively inserts this instruction for every owned root at
the variable's last use or on an appropriate control-flow edge. Runtime teardown
then follows the value recursively; this also releases heap-backed storage whose
destruction has no user-visible output.

### `drop.reg` — Reserved Register Drop

```text
drop.reg %register
```

Represents destruction of a register temporary. It is reserved for a future
operation or assembler VM and is currently rejected by the register VM as
unsupported. Current lifetime elaboration uses `drop.var`.

## Block Terminators

Terminators appear only as the final operation of a basic block.

### `jump` — Unconditional Transfer

```text
jump bb_target
```

Continues execution at `bb_target`.

### `branch` — Conditional Transfer

```text
branch %condition, bb_true, bb_false
```

Tests `%condition` using runtime truth-value semantics. Control transfers to
`bb_true` when true and `bb_false` otherwise.

### `return` — Return Value

```text
return %value
return.none
return.cleanup %value, [$v3, $v2]
```

Ends the current function and returns a register value or `None`.

Within a try sub-region, `return` becomes a non-normal flow value. It propagates
through enclosing regions so cleanup and `finally` execute before the function
actually returns. `return.cleanup` additionally retains the listed loop-owned
variables while the return value is materialized, carries them through every
pending `finally`, then destroys them in the recorded inner-to-outer order.

### `falloff` — Complete Region

```text
falloff
```

Marks normal completion of a try sub-region. It is not a valid ordinary function
terminator. The region runner translates it to normal flow and allows the
surrounding `try` instruction to continue with `else` or `finally` as required.

### `escape` — Escape Structured Region

```text
escape bb_target cleanup [$v3, $v4]
```

Represents `break` or `continue` leaving a try sub-region for a loop block in the
enclosing function. Before propagating the jump, the VM destroys the listed
region-local variables. Every intervening `finally` executes before control
reaches `bb_target`.

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

The categorized tables in [Instruction Summary](#instruction-summary) are the
human-readable execution inventory. The frozen textual vocabulary and canonical
printer live in `mir::text`; `MirInstr` and `MirTerm` in `src/mir/ir.rs` remain
the in-memory authority. Keeping another flat opcode list here would drift when
the structured MIR evolves.
