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
Mojo leaves undefined. The 2026-09-12 error-folder sweep measured the gap at 25
fixtures the pin compiles and runs while Mojito rejects or traps
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
- `assets/type_error/var_less_introduction.mojo` rejects a var-less
  introduction that the pin only deprecates, with a warning, and runs.

The 26 fixtures in the same sweep where Mojito's rejection is *not* deliberate
are the opposite case and stay on `docs/roadmap.md` as `divergence` rows.

### Divergences retained across re-pins

The behavioral-divergences task in `docs/roadmap.md` §2 burns divergences to
zero. These three are exempt and are re-probed at every nightly re-pin rather
than fixed.

- `subtree-origin-cast`: a cited bridge to upstream's `#lit.origin.subtree`
  experiment; its fixtures live under `assets/extensions/` (above).
- `reference-valued-aggregate`: the `ref` field extension above.
- `deinit-body-field-loop-read` (`output-diff`): the pinned Mojo destroys a
  `deinit self` field that is read only inside a loop body at the
  destructor's entry and then reads the destroyed value. That is an upstream
  bug Mojito does not reproduce.

### Rust runtime services that stay in Rust

`docs/roadmap.md` §2 lists the Mojito stdlib shortcuts to port back to pure
Mojo. These four are not on it: upstream draws the same boundary, so a Mojo
implementation would be the wrong shape, not a better one.

- `_mojito_abort` (`os.abort`)
- `mjrt_read_line` (`input`)
- allocation
- traps

## Not To Be Fixed

### Native SIMD: signed-zero min/max is unspecified

Float `reduce_min` / `reduce_max` fold through `llvm.minnum` / `llvm.maxnum`,
which is the VM's `f64::min` / `max`, and neither side specifies the result
of `min(-0.0, +0.0)`.

- Fixtures avoid mixed-sign zeros in min/max reductions.
- Revisit only if upstream pins a rule.

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
