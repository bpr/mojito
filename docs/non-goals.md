# Non-Goals And Deliberate Limits

Decisions *not* to act, and why. Nothing here is scheduled work:
[`docs/roadmap.md`](roadmap.md) is the task list and holds only what we
intend to do next. An item lands here when we have considered it and chosen
to leave it alone — because upstream behaves the same way, because the cost
is not justified, or because the capability is kept on purpose.

An item leaves here only when the reason expires: upstream pins a rule,
upstream lands a feature, or the cost calculation changes. Then it becomes a
roadmap checkbox.

## Deferred Options

### Cranelift alternate backend

Not planned: Pliron is Mojito's supported route to LLVM and optimized
binaries. The verified-MIR waist deliberately permits a Cranelift backend, so
this stays an option rather than a plan.

- Build it only if portability, build cost, or upstream risk justifies it.
- It must implement the same acceptance slices over the shared
  target/layout/runtime ABI and the differential corpus.
- It must not fork language semantics or become a required compiler layer.

### Pliron as the required post-check IR framework (A1 rejected)

Not planned: the Stage A1 shadow core passed four of its five falsifiers and
failed the fifth, overhead. The record is
[`docs/notes/pliron-a1.md`](notes/pliron-a1.md).

- The failed condition is compile time on the predeclared stress row. Gate
  ×64 compiles 1.62 times as slowly under the shadow, 1.54 to 1.69 at three
  median absolute deviations, against a limit of 1.20.
- `stdlib_heavy` fails too, at 1.45. Gate ×16 straddles the limit at 1.19.
  Peak memory is unchanged on every row.
- Measured 2026-09-30 at `1967fe1d`, Pliron `477e6b0`, LLVM 23.1.0, release
  build, ten alternating pairs, on an Intel Core i7-10875H.
- The cost is the canonical text round trip: 79 to 85% of the added time on
  the failing rows is one print and one parse of the core module.
- The corpus aggregate was not measured, because it could not reverse a
  failing stress row.
- What is rejected is this bridge as built. Representation, locations,
  lifecycle, conversion totality, and default-build isolation all held.
- Optional Pliron-to-LLVM emission stays the supported native backend, and
  the parametric front-end work of roadmap section 1 does not depend on
  this.
- Reopen when a canonical form needs no print and parse at the pin, or the
  pinned printer and parser are several times faster, and the same focused
  lane then passes.

## Kept On Purpose

### Direct `ref` struct fields stay a Mojito extension

Not to be removed: upstream rejects `var f: ref[o] T` fields (`'ref' patterns
are only valid on the left side of an assignment`) and spells reference
storage through `Pointer[T, origin]`, but upstream has signalled that `ref`
fields may arrive, so Mojito keeps the capability (2026-09-09 decision; the
stdlib's six `ref`-field structs stay as they are).

### `Origin._subtree` casts stay under `assets/extensions/`

Not to be removed: `origin_of(self)._subtree` is upstream's own experimental
conservative origin form, and the pinned build parses it but rejects every use
("use of a never-initialized interior reference
'origin_of(self_is_origin).subtree'"). Mojito implements it, so a program that
casts to a subtree origin is accepted here and rejected there — the
`subtree-origin-cast` divergence below. The 2026-09-12 assets sweep moved its
fifteen fixtures to `assets/extensions/<folder>/`, because the ordinary folders
hold only programs the pinned Mojo compiles.

These are the two admitted extensions under the match-or-subset rule
(`AGENTS.md` invariant 1).

- Fixtures that use `ref` fields live under `assets/extensions/<folder>/`;
  the ordinary `assets/` folders hold only programs the pinned Mojo compiles
  (`assets/README.md`).
- Where a `ref`-field fixture has a Mojo-valid twin, the twin spells the
  storage through `Pointer[T, origin]` under the same name with "ref_field"
  replaced by "pointer_field" in the ordinary folder.
- `conformance/fixtures/reference_valued_aggregate.mojo` stays a
  `mojito-only` row until upstream decides.
- If upstream lands `ref` fields, re-probe the extension fixtures against the
  new spelling and promote them back — that is a roadmap task, not this one.

### Interior-reference and unsafe-memory strictness exceeds the pin

Not to be relaxed: Mojito tracks interior references through `Pointer` and
through views into a container, and traps on unsafe-memory misuse the pinned
Mojo leaves undefined. The 2026-09-12 error-folder sweep, with its 2026-09-13
divergence triage, measured the gap at 33 fixtures the pin compiles and runs
while Mojito rejects or traps; the 2026-09-21 re-pin left 32, upstream having
made a var-less introduction an error of its own
(`conformance/assets-mojo-errors.tsv`, family `subset`). Invariant 1 permits it:
Mojito may reject valid Mojo.

- Eleven `ownership_error`/`origin_error` fixtures report `invalidated interior
  reference` or `conflicts with live reference` where the pin tracks nothing and
  runs. `assets/ownership_error/reference_row_iteration_invalidated_by_row_append.mojo`
  is the one to cite: the pin runs it to completion printing dangling addresses,
  so the strictness is catching real undefined behavior rather than rejecting a
  sound program.
- Six `runtime_error` fixtures trap on use-after-free, double-free, and reads of
  uninitialized `MaybeUninit` storage; the pin runs each one to exit 0.
- Five more trap on integer division or modulo by zero and on a negative `**`
  exponent, which the pin leaves to the hardware.
- `assets/type_error/comptime_dict_result_not_freezable.mojo` and
  `external_call_unknown_callee.mojo` name Mojito's own mechanisms — a VM-CTFE
  value that cannot cross back, and the libc allowlist — which have no upstream
  counterpart to agree with.
- `mapping_key_ref_write.mojo` and `set_element_ref_write.mojo` reject a write
  through a `for ref` key or set element. The pin accepts it and then answers
  membership from a stale hash index (`set-ref-write-gap`).
- `pointer_to_place_offset_rejected.mojo` rejects offset 1 of a pointer to a
  single local, which the pin reads from the neighbouring stack slot.
- `span_implicit_return_escape.mojo` rejects returning a frame-local `List` as a
  `Span`; the pin returns a dangling view.
- `pointer_take_tracked_nontrivial.mojo` rejects `unsafe_take_pointee()` of a
  `String` through `Pointer(to=s)`. The pin moves the value out and leaves `s`
  to be destroyed again; Mojito allows a tracked take only for a trivially
  destructible element.
- `typelist_index_out_of_range.mojo` bounds-checks a `TypeList` index. The pin
  has no check and yields the malformed type `Int, Bool[2]`, failing only at a
  later use.
- `generic_ctfe_impure.mojo` keeps VM-CTFE pure. The pin lets a compile-time
  callee `print`, but that output has no stable home: it appears under
  `mojo run` and is absent from a `mojo build` binary. A parameter default
  that prints or reads input rejects for the same reason, since the pin
  evaluates a default at compile time.
- `assets/runtime_error/nominal_string_justify_fillchar.mojo` traps on an
  `assert` in upstream's own `_justify`. Mojito evaluates every
  standard-library assert, as the pin does under `-D ASSERT=all`; the pin's
  default level skips it.

The fixtures in the same sweep where Mojito's rejection is *not* deliberate are
the opposite case and stay on `docs/roadmap.md` as `divergence` rows.

### Divergences retained across re-pins

The behavioral-divergences task in `docs/roadmap.md` §3 burns divergences to
zero. These five are exempt and are re-probed at every nightly re-pin rather
than fixed.

- `subtree-origin-cast`: a cited bridge to upstream's `#lit.origin.subtree`
  experiment; its fixtures live under `assets/extensions/` (above).
- `reference-valued-aggregate`: the `ref` field extension above.
- `deinit-body-field-loop-read` (`output-diff`): the pinned Mojo destroys a
  `deinit self` field that is read only inside a loop body at the
  destructor's entry and then reads the destroyed value. That is an upstream
  bug Mojito does not reproduce.
- `defined-shift-overflow` (`mojito-only`): Mojito masks a shift amount to
  the word (`& 63`), so `UInt(1) << UInt(70)` has an answer. The pin's
  over-wide shift is poison, which it cannot even fold. Mojito's result is a
  deliberate contract in [`docs/native-abi.md`](native-abi.md) that both
  backends keep, and any defined value is a valid refinement of poison.
- `defined-float-to-int-edges` (`output-diff`): `Int(f)` saturates out of
  range and converts a NaN to zero, the same `docs/native-abi.md` contract.
  The pin's conversion is poison there, so it folds those branches to
  arbitrary values.

### Rust runtime services that stay in Rust

`docs/roadmap.md` §3 lists the Mojito stdlib shortcuts to port back to pure
Mojo. These four are not on it: upstream draws the same boundary, so a Mojo
implementation would be the wrong shape, not a better one.

- `_mojito_abort` (`os.abort`)
- `mjrt_read_line` (`input`)
- allocation
- traps

The integer arms of the VM's `Display for Value` stay Rust too, for a
different reason: every program-visible text path, sized `SIMD` lanes
included, formats integers through the bundled `_int_digits`/`_uint_digits`
bodies, and what is left has no VM to run them and is not program output.
Those callers are runtime diagnostics, which upstream's C++ compiler writes,
and the CLI's stdin binding echo, which runs after execution and is excluded
from differential comparison.

## Not To Be Fixed

### Native SIMD: signed-zero min/max is unspecified

Float `reduce_min` / `reduce_max` fold through `llvm.minnum` / `llvm.maxnum`,
which is the VM's `f64::min` / `max`, and neither side specifies the result
of `min(-0.0, +0.0)`.

- Fixtures avoid mixed-sign zeros in min/max reductions.
- Revisit only if upstream pins a rule.

### A compile-time `//` or `%` by zero stays an error

The pinned Mojo folds `comptime a = Int(7) // Int(0)` (and the literal and `%`
forms) to `0`. Mojito reports `division by zero`, a rejection the subset rule
allows.

- The pin's `0` reads as an artifact of its folder, not as language
  semantics; nothing documents it.
- The shared folder (`mojito_types::param_expr::fold`) keeps the structured
  error, and a closed partial operator in an untaken branch stays unevaluated.
- A cancellation keeps the error only in the node it builds: an operator
  built over that node (`(1 // 0 - 1 // 0) + 1`), and `fold`, drop the
  zero-coefficient term again, which lands on the pin's answer.
- Revisit only if upstream documents the fold as the rule.

### `Pointer(to=<temporary>)` is unsupported

`View(Pointer(to=make_list()), 0)` reports `Pointer(to=...) requires a place
expression`.

- A temporary bound to an explicit `ref[Self.o]` constructor parameter covers
  the same fixture point
  (`assets/ok/pointer_field_ctor_temporary_explicit_init.mojo`).

### An untracked origin cast frees its source

Casting a tracked pointer to an untracked origin at its source's last use
(`s.unsafe_ptr().unsafe_origin_cast[ImmUntrackedOrigin]()`) frees the source
before the pointer is read.

- Upstream does the same, so this is parity, not a defect.

### An owned pack outlives a literal-index element read

`print(a[0])` as the last use of `var *a: *Ts` prints the element and then
destroys the pack. The pinned Mojo destroys the whole pack first and then
prints the element it read out of it.

- The pin's order reads a destroyed value: an element holding a `String`
  prints that string after the element's `__deinit__` ran and its field was
  freed (2026-10-02). It is an upstream defect, not a rule to copy.
- The same happens at the pin for `a[1]` and for an element handed to a read
  parameter (`show(a[0])`).
- A `comptime for` over the pack, a homogeneous `var *a: T` collector, and a
  read `*a: *Ts` pack consume the element first on both sides, which is the
  order Mojito keeps everywhere.
- Probe:
  `conformance/probes/owned_pack_literal_index_destroyed_before_use.mojo`.
- Revisit when a re-pin prints the element first, and promote the probe to
  `assets/ok/` then.

### A store over an element stales an iterator taken from it

`var it = ys[0].codepoints()`, then `ys[0] = String("zz")`, then a loop over
`it` is rejected with "use of invalidated interior reference 'it' to
'ys["element"]~'". The pinned Mojo compiles it and prints the replaced
element's codepoints.

- The pin's program reads bytes the store destroyed: with a heap-sized string
  as the element it prints the freed string (2026-10-02). It is an upstream
  defect, not a rule to copy.
- The pin stales a view only when its return origin names an interior below
  the element. `codepoints()` returns `origin_of(self)`, which names the
  element itself, and a store over the element does not stale that at the
  pin.
- Mojito lends such a view the whole subtree below the element, so a store
  over the element stales it. A view that names an interior
  (`ys[0].rstrip()`) lends exactly `ys["element"]["bytes"]`, as at the pin.
- The same shape over a local (`var it = s.codepoints()`, then
  `s = String("zz")`) runs at the pin and is rejected by Mojito as a loan
  conflict, for the same reason.
- Growing the list under the iterator is rejected on both sides.
- Probe: `conformance/probes/list_element_iterator_after_store.mojo`.
- Revisit when a re-pin rejects the store, and move the probe to
  `assets/ownership_error/` then.

### A nested callable contract's binders are not told from its enclosing contract's

Every anonymous callable contract declares its binders under the owner
`$callable` and canonicalizes them to `$contract` slots, which is the
alpha-equivalence contracts need at depth 0. A contract binder bounded by
another contract that names the outer binder would collide one level down.

- The pinned Mojo rejects every shape that reaches it (2026-09-26): a runtime
  `f: def[U: Writable](U, T) -> None` fails with "value cannot be converted
  from type value '$0' to an instance of '$0'", and the compile-time
  `f: def[U: Writable](U, T) -> None` parameter with "missing required
  argument: 'move'".
- Revisit when the pin accepts a nested generic contract that names an
  enclosing binder.

### A schema 1.0/1.1 MIR artifact binds by spelling

A binder record in a 1.0 or 1.1 artifact carries only its spelling, so the
parser gives each spelling one identity (`$mir-1.1:<name>`) and the
artifact's own uses still find their declaration.

- Nothing else is recorded to recover: a later schema writes `owner`/`slot`.
- A 1.2 artifact's forwarded type argument (`hash[Self.H](key)`) records no
  binder either. Native monomorphization resolves it when exactly one
  enclosing binder carries the spelling, and leaves the slot to its default
  otherwise.
- A `where` operand or a callable default's parameter older than schema 1.4
  is a spelling as well. It reads as the binder of that spelling in the
  parameter list declaring it, and as an unbound reference when that list
  declares none, so an operand naming an enclosing declaration's binder is
  bound by nothing.
- A deferred slot older than schema 1.5, and a pack query's pack older than
  schema 1.6, are spellings too. Each reads as an unbound reference, which no
  enclosing binder closes.
- Revisit only if a 1.0/1.1 artifact must be read with two same-spelled
  binders told apart.
