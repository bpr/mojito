# Owned-`var` transfer convention

Upstream Mojo (pinned head `a79fbdf59f2`) refuses to turn a *place* — a named
variable, parameter, field projection, subscript, or other reference result —
into an *owned value* unless its type is `ImplicitlyCopyable`. The escapes are
`^` (transfer) and `.copy()` (explicit copy). The rule lives in one front-end
helper (`KGEN/lib/MojoParser/IREmitter.cpp:906 emitCopyOfValue`) and has no
last-use analysis: `take(a)` on a `List` errors even when `a` is never used
again. Mojito implements the same convention; this note records the anchors
and the decisions that are not derivable from the code.

## Pinned verdicts (2026-08-30 probes, since deleted)

- `return result` of a `List` local: **rejects**, suggesting both `^` and
  `.copy()` (`conformance/fixtures/implicit_copy_return_local.mojo`).
- `take(values)` at the last use of `values`: **rejects**; no last-use move
  (`conformance/fixtures/implicit_copy_owned_argument.mojo`).
- `struct S(ImplicitlyCopyable)` with a non-`ImplicitlyCopyable` field and a
  hand-written `__init__(out self, *, copy: Self)`: **accepts** — the explicit
  copy initializer is the conformance (`struct_implicitly_copyable_conformance_ok`).
- `bump(mut x: String, y: String)` called as `bump(s, s)`: **rejects**
  although `String` is `ImplicitlyCopyable`. Nominal memory values never
  disarm within-call exclusivity by copying (`call_read_is_independent_copy`).

## Where the decision lives

Every consuming position funnels through `Checker::check_consuming_as`
(`src/checker/traits.rs`): a place that is not `Copyable` is the existing
`NonCopyable` error; a place that is `Copyable` but not `ImplicitlyCopyable`
is `TypeError::ImplicitCopy`; an `ImplicitlyCopyable` place records
`copy_place_value_uses` → `SemanticAdjustment::CopyPlaceValue`. The
diagnostic mirrors upstream: the `^` note appears only when the value is
`Movable` and the place's root binding is mutable or owned
(`consider transferring the value with '^'`); the `.copy()` note whenever the
type is `Copyable`.

Binary operators previously bypassed the funnel (`struct_dunder` is
type-only). `infer_infix` now resolves the dunder signature
(`struct_dunder_signature`) and routes a `var`/`deinit` operand through the
funnel, which is why `p + q` on `List` rejects and `p + q^` accepts.
Speculative overload candidates save and restore `copy_place_value_uses` and
`implicitly_copied_consuming_receivers` so a rejected candidate leaves no
copy marks behind.

Reference-result reads (`check_reference_result_reads`) use the same
predicate. Method-call receivers and invoked callables reached through a
reference result are retained borrows (`borrowed_reference_receivers`), never
value reads; a consuming receiver over a reference result is gated on
`ImplicitlyCopyable` like a place.

## Deliberate exceptions

- **Unregistered structs.** On the unlinked seam (`check_program` without a
  linked stdlib) `is_implicitly_copyable` treats an unregistered nominal as
  implicitly copyable except the bundled collection shapes the checker
  recognizes structurally (`List`, `Dict`, `Set`, `Optional`, `Array`,
  `OwnedPointer`). The linked pipeline sees the real declarations and is
  authoritative.
- **Iterator refinement.** A `ref`-returning `__next__` satisfying a by-value
  `Self.Element` requirement is read as a checked copy
  (`CopyIteratorReference`). The generic body only sees the by-value
  contract, so the monomorphized re-check records the read as copyable
  rather than demanding `ImplicitlyCopyable` where upstream sees no copy.
- **Borrowing view conversions.** `var s: Span[Int, _] = xs` converts through
  an `@implicit` constructor that borrows its source; the consumed value is
  the conversion temporary (`storage_conversion_borrows_source`).
- **Built-in `copy()`.** `x.copy()` on a scalar, literal, tuple, or variant has
  no callee: the checker resolves it to the value read and MIR lowers it as
  one (`builtin_copy_is_value_read`); a place receiver records
  `CopyPlaceValue` like an implicit copy of an `ImplicitlyCopyable` value.

## Ownership analysis and VM changes the convention forced

`MirInstr::MethodCall` gained `recv_writes` (textual `call.method` field
`recv_writes`): the loan analysis classified every retained receiver place as
an exclusive write, so `self.items[i].copy()` — an immutable `self` receiver
over the hidden `$call_ref` slot — reported a false self-conflict. The
checker's effective receiver convention (`Imm` for a `ref self` reached
through an immutable reference) decides the flag; unchecked lowering paths
stay conservatively exclusive. The VM's `load_index_dunder` reads a
pointer-subscript parent through the reference walk when the parent crosses
a `ref`-typed field (`self.src.data[i]` in `_OptionalIter.__next__`), and
the parser binds an adjacent postfix `^` tightest (`p + q^` transfers `q`).
