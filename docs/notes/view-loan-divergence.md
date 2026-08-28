# Borrowing Views: Why Mojito Rejects What Mojo Accepts

Status: implemented behavior as of the ref-field-struct-return work
(2026-08). Book destination: source material for *Mojito Internals*
chapters on borrow rules and runtime iteration, and a Part VIII case
study in choosing a subset position.

Mojito rejects this program; the pinned upstream Mojo accepts it and
prints `1`, `2`, `9`:

```mojo
def main() raises:
    var d: Dict[Int, Int] = {1: 10, 2: 20}
    var kv = d.keys()
    d[9] = 90          # Mojito: access to 'd' conflicts with live reference 'kv'
    for k in kv:
        print(k)
```

The divergence is recorded as the `dict-view-source-mutation-gap`
strict-subset conformance case
(`conformance/fixtures/dict_view_source_mutation_gap.mojo`), beside the
analogous `set-ref-write-gap`. This note explains why the two compilers
disagree, why the disagreement is structural rather than accidental, and
why Mojito's side of it is a deliberate policy choice.

## What upstream's views are

Upstream's `_DictKeyIter` does not hold a language-level reference. It
holds a pointer:

```mojo
var src: Pointer[Dict[Self.K, Self.V, Self.H], Self.origin]
```

built with `Pointer(to=dict)`. The `origin` parameter records
provenance, but Mojo's current checker does not convert that stored
pointer into an enforced loan on the dictionary for the lifetime of the
iterator value. Once `keys()` returns, `d` is — as far as upstream's
exclusivity checking is concerned — unborrowed. `d[9] = 90` therefore
compiles, and at runtime the view reads whatever the dictionary's
storage looks like at each `__next__`: it observes the concurrent
insertion, hence `1 2 9`.

Mutation during a live view sits in upstream's
"programmer error, unenforced" zone. A plain insertion happens to be
benign; anything that triggers a rehash, or a write *through* a view,
is not. The Set variant of the same gap demonstrates the corruption
concretely: upstream accepts `for ref x in s: x += 10` and then its
hash index is desynchronized from element storage — `11 in s` prints
`False` even though an element now holds 11. Upstream's own source
carries TODOs (better iterator traits, origin enforcement for
`Pointer(to=...)`) pointing toward eventually tightening this.

## What Mojito's views are

Mojito's `_DictKeyIter` holds a checked reference field:

```mojo
var src: ref[iterable_origin] List[DictEntry[Self.K, Self.V]]
```

and the ref-field-struct-return machinery is precisely what makes that
reference *tracked across an ordinary method return*. When a
non-consuming method returns a struct whose storage contains a
reference, the checker records `SemanticAdjustment::BorrowViewResult`
at the call span, and MIR lowering establishes an immutable whole-place
loan on the receiver that lives exactly as long as the returned view
does. A later write to the receiver is a write to a loaned place, and
the ownership analysis rejects it: `access to 'd' conflicts with live
reference 'kv'`.

Before that machinery existed, Mojito "accepted" such programs only in
the sense that the loan was missing entirely — the same missing loan
also let the source be dropped while the view was still alive (a
dangling-reference crash at runtime) and let mutation corrupt an
iteration invisibly. The loan is one mechanism with two consequences:
it keeps the source alive, and it excludes mutation. There is no way to
keep the first and discard the second without inventing a third
ownership state.

## Why Mojito does not imitate the accept

Matching upstream here would mean deliberately not installing the loan
for mapping views. That purchases a worse position than rejection:

1. **The accepted programs' output is layout-dependent.** A view that
   observes concurrent mutation exposes internal storage behavior.
   Mojito's dictionary is dense insertion-ordered entries plus a
   nested-list bucket index; upstream's is a swiss table. They do not
   rehash at the same points or in the same way, so every
   mutate-during-iteration program would become a potential
   output-divergent accept — an unbounded family of the eager-drain
   divergence this same work retired, tied permanently to
   implementation details neither side promises.
2. **The precedent already exists.** The Set write-through case showed
   that when upstream's accept produces corrupted or layout-dependent
   observations, matching it means matching corruption byte-for-byte.
   Mojito instead rejects (`set-ref-write-gap`), and this case takes
   the same shape.
3. **The subset policy permits exactly this.** Mojito may reject valid
   Mojo; it may not change what accepted programs mean. A clean early
   rejection with a loan-conflict diagnostic is a subset position; an
   accept with different output is a fork.

## Convergence

The gap retires by upstream movement, not ours: if Mojo's checker
starts enforcing origins on `Pointer(to=...)`-backed iterator storage,
both compilers reject, the conformance case flips to `reject`, and the
divergence note in `conformance/parity.tsv` (`types.collections`)
disappears.

## Code anchors

- Mojito view structs and loans: `stdlib/std/collections/dict.mojo`
  (`_DictKeyIter`/`_DictValueIter`/`_DictEntryIter`,
  `_TakeDictEntryIter`); adjustment install in
  `src/checker/method_calls.rs` (`infer_method_call` finalization);
  consumer arms in `src/checker/origins.rs` (`aggregate_origins`) and
  `src/mir/facts.rs` (`aggregate_borrows`); conflict rule in
  `src/analysis.rs` (write vs. live loan).
- Upstream shapes: `mojo/stdlib/std/collections/dict.mojo` at the
  pinned revision (`_DictKeyIter`/`_DictEntryIter` with
  `Pointer[Dict, origin]` fields).
- Conformance: `dict-view-source-mutation-gap` and `set-ref-write-gap`
  in `conformance/cases.tsv`; narrative notes in the
  `types.collections` row of `conformance/parity.tsv`.
