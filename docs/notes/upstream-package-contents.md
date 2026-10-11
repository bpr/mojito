# What upstream's precompiled package holds

Status: verified against the pinned build `mojo-2026092105` on 2026-10-10,
while landing R582. Settles which stage of a function body Mojito's `std`
package should store (`crates/mojito-mir/src/mir/text/package.rs`).

## Finding

Upstream's precompiled package (`.mojoc`, written by `mojo precompile`,
the pin's replacement for `mojo package`) holds each function body as
checked, **before** its lifetime pass inserts destructors. Its importer runs
that pass on each body it materializes.

- `mojo precompile --help` says the package "includes only non-elaborated
  code".
- A package `a` declaring `struct Zork` with `__init__` and
  `__del__(deinit self)` stores the destructor under `__deinit__`, and its
  op set carries `lit.ownership.mark_destroyed` (from the source-level
  `deinit self`).
- A package `b` holding only

  ```mojo
  from a import Zork

  def useit():
      var s = Zork()
      print(s.x)
  ```

  names `Zork` and `__init__` (the call it makes), but contains no
  `__deinit__` string and no destroy op: its `lit` ops include `call`,
  `callee`, `var.decl`, `ref.load`, `ref.store`, `ref.immut`,
  `ref.struct.ger`, `ref.pack.create`, and `load.consume`, and nothing
  named `destroy` or `ownership`. The body of `useit` is the `lit` IR
  before the lifetime pass, whose destructor call for `s` would name
  `__deinit__` as the constructor call names `__init__`.
- `precompile` still runs the lifetime check: a package body that uses a
  value after `take(s^)` is rejected at package build time ("use of
  uninitialized value 's'"). The check's rewrite is not what it stores.

So storing drop-elaborated bodies in Mojito's package would diverge from
upstream.

## Finding: the stored body's control flow is still structured

Verified against the same pin on 2026-10-10, while landing R546. A package
`b` whose one function has a `while` with an `if`/`else` and a `break`, a
`for` over `range`, and a `return`:

- holds the ops `hlcf`, `loop`, `loop.yield`, `loop.continue`,
  `loop.break.else`, `yield`, `try.yield`, and `return`;
- holds no `br` and no `cond_br`.

So the stored body is the parser's IR with its semantic control flow, which
the checks validated without storing a rewrite: `lit`'s `return` and the
structured `hlcf` loops, which LowerSemanticCF lowers to terminators. The
compiler walkthrough agrees: package building rebuilds "a `lit.package` op
whose regions carry the full post-parse IR". An importer therefore lowers a
materialized body's control flow as well as running its lifetime pass.

Mojito matches it: `src/package.rs::build` runs the check and the ownership
check, and writes each body as the check left it, one record per lowering
unit (its syntax and the check's facts for it,
`mojito_checked::unit::CheckedUnit`); `PackageBodies::get` lowers a body's
unit (checked arena, HIR CFG, MIR) and inserts its drops when the
elaborator first reaches it. What remains is representation: Mojito's
checked body is syntax beside side tables, upstream's typed `lit` ops
(R604).

## Recipe

To read what a pinned package holds:

1. Write a package directory (`b/__init__.mojo`) and build it with the
   pinned toolchain: `mojo precompile b -o b.mojoc` (the pinned
   environment is the one `scripts/sweep-assets-mojo
   --mojo-pixi-manifest` uses).
2. Strip the 120-byte header and decompress the rest, which is zstd:
   `tail -c +121 b.mojoc | zstd -d -o b.raw`.
3. The result is MLIR bytecode (it starts `ML\xefR`, producer `MLIR24.0.0git`).
   Its dialect/op-name and string sections are plain text:
   `strings b.raw | grep -E '^(call|fn|var\.decl|ref\.|load\.|ownership)'`
   lists the ops, `strings b.raw | grep -c __deinit__` asks whether any
   destructor is named, and `strings b.raw | grep -E '^(hlcf|loop|br|cond_br)'`
   whether the control flow is still structured.

What the importer does with an already-checked body beyond reinserting
destructors (whether it reports lifetime diagnostics again) is not
observable this way.
