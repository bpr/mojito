# Next Batches: Toward Mojo's Architecture

The batches to run after the Pliron gate repair, which has landed. Each
batch is one `scripts/claude_loop.py` command. `--task` runs each named entry
after its open prerequisites, so a batch may run a few entries it does not
name. Preview one with `--list` or `--dry-run`. `scripts/run-batches` reads
the commands below, one batch each. After every batch it runs the entries
the batch's sessions filed, round by round while each round is smaller than
the last, then focused and the instantiation ratchet. A rule that holds
across many sites is planned sites-first: an entry that enforces it lists
every site, found by running the check, rather than leaving later sessions
to find them one construct at a time.
Delete a batch here once it has landed.

The goal is structure, not features. Each batch closes one place where
Mojito's representation departs from Mojo's own and other code works around
it. They are ordered so that each batch lands on the representation the
earlier ones leave. Batch 1 finishes the MIR call op, which every phase
below the check reads, by running what its first pass left. Batches 2 to
4 make origins and `Pointer` the library declarations upstream has, which
the memory model rests on. Batches 5 to 7 move the trait, the parser, and
the check's handoff closer to upstream's shapes.

Four entries are planning entries. Their session writes a plan, files
the plan's slices as chained entries, and deletes the planning entry
instead of landing code: R2139 (batch 2), R2211 (batch 4), R769 (batch
5), and R2137 (batch 6). Their slices run in a later batch, or come back
as a new batch here.

## Checking between batches by hand

`scripts/run-batches` runs these checks itself. To run a batch by hand and
check it before the next:

1. Run the batch's `scripts/claude_loop.py` command below. It commits each
   entry it lands.
2. Run the focused tier, about four minutes:

   ```sh
   export LLVM_SYS_231_PREFIX="$(scripts/llvm-prefix)"
   export PATH="$LLVM_SYS_231_PREFIX/bin:$PATH"
   scripts/focused              # VM and native; `--vm-only` needs no LLVM
   ```

   It exits 0 when every entry passes. On a failure it prints
   `FAIL fixture <path>` or `FAIL test <target>::<name>`, and the full report
   is in `target/focused/last.log`. `scripts/focused --filter NAME` reruns
   just the entries matching `NAME` after a fix.
3. Check that the batch's newly green items stay green. A batch below lists
   the commented-out `tests/focused/list.txt` lines it should restore. Make
   sure the landed sessions uncommented them (`git diff HEAD~N --
   tests/focused/list.txt`), then rerun `scripts/focused --filter` on them.
4. Check the instantiation ratchet:

   ```sh
   scripts/instantiation-report --check
   ```

   "stale" or a small intended "growth" means you should commit the refreshed
   baseline: `cp target/instantiation.tsv benchmarks/instantiation.tsv`.
   A "lost" fixture is a regression to triage before the next batch.
5. Optionally, for a broad check in under a minute: `scripts/smoke`.

The overnight gate (`scripts/run-overnight-gates`) remains the full check.
`scripts/focused --from-gate target/overnight-gates/<stamp>/check-default.log`
prints the list lines a gate run's new failures add.

## 1. What the call-operand batch left (14 entries)

The call-operand batch (R2185–R2191, R1931) landed on 2026-11-16, and its
sessions filed these entries as they went. A batch's residue now runs at
once, inside the batch (`scripts/run-batches` step 2). These were filed
before that existed, so they run here, ahead of anything else.
- By-address `read` arguments that still pass a value, and the verifier
  rule that would catch them: R2244, R2248, R2249, R2250, R2251.
- `deinit` receivers: R2272, R2273, R2274.
- Results that keep a struct's origin parameter or pass a pointer
  straight through: R2262, R2263.
- `VariadicPack`'s mutability: R2264.
- MIR pass cases that fail at HEAD: R2275.

```sh
scripts/claude_loop.py --model opus --plan-fable-on-opus -n 0 \
  --task R2244 --task R2248 --task R2249 --task R2250 --task R2251 \
  --task R2262 --task R2263 --task R2264 \
  --task R2272 --task R2273 --task R2274 --task R2275
```

## 2. Origin parameters are ordinary declared parameters (7 entries)

Upstream's origin parameter is a value parameter whose type is the library
`Origin` struct, declared in place in the parameter list. Mojito classifies
one by its bound's spelling, leaves it out of function and method parameter
lists, and keeps a struct's origin arguments in a tail after the others.
- R2045 goes first, though it is not about origins. It re-emits seven MIR
  pass test inputs that the verifier now rejects, so `scripts/smoke
  --suites` checks every pass case again before the batch starts.
- R2139 plans the classification change and files its slices. It
  repoints R2209 at them, so batch 4 runs them.
- R2126 and R2127 declare origin binders in place in the check and in
  MIR. R2128 then gives `mut=` binders a value in the body.
- R762 orders a struct's arguments by its declared list, with origins in
  place. R763 then drops the parameter syntax the checker keeps to
  validate them.
- This batch comes before the second `Pointer` batch, because
  `Pointer[mut, T, origin]` is spelled in this order.

```sh
scripts/claude_loop.py --model opus --plan-fable-on-opus -n 0 \
  --task R2045 --task R2139 --task R2126 --task R2127 --task R2128 --task R762 \
  --task R763
```

## 3. `struct Pointer` is declared in the library, part 1 (6 entries)

Upstream's `Pointer` is a library struct over one `!kgen.pointer` field,
and its members are bodies written in `lit.ref` and `pop` ops. This
continues the built-ins batch, whose R1681 planned it.
- R2203 and R2204 make those ops and `AddressSpace` spellable.
- R2205 declares the struct and its aliases. A member with no checker
  rule then resolves to its declared body.
- R2206 moves the casts and conversions into the declared bodies.
- R1665 and R2196 fix the VM's place pointers, which R2207 needs. A
  pointer over a place gets an address. A pointer to a place that holds
  a pointer addresses that place, not its pointee. R2196 is a
  wrong-output bug on both backends today.

```sh
scripts/claude_loop.py --model opus --plan-fable-on-opus -n 0 \
  --task R2203 --task R2204 --task R2205 --task R2206 --task R1665 \
  --task R2196
```

## 4. `struct Pointer` is declared in the library, part 2 (6 entries)

The remaining checker rules become declared bodies, and the type becomes
the struct.
- R2024 constructs `Tuple(init_with=)` elements in place.
- R2207 moves the address and memory members into the struct. R2208
  moves `Pointer(to=)` and `p[]`.
- R2209 makes a pointer's origin an `Origin` value. It waits on R2139's
  classification slices (R2339, R2340, R2341), which `--task` runs first.
- R2210 deleted `TyKind::Pointer` with a package bump and no MIR schema
  bump. R2486 and R2487 carry the struct rules and binding passes it
  left as the compiler's.
- R2211 gave a `!lit.ref` the `RefTy` record a `ref` holds. R2488–R2491
  move the read-through off `TyKind::Ref` at each binding position.

```sh
scripts/claude_loop.py --model opus --plan-fable-on-opus -n 0 \
  --task R2024 --task R2207 --task R2208 --task R2209 --task R2210
```

## 5. A trait's members are its methods (6 entries)

Upstream checks a trait default as a method of its trait, binds `_Self` as
the trait's binder, and keys overloads by resolved signatures. Mojito
checks a default as a module-scope `def` it builds, and finds requirement
defaults and forwarding calls by syntax.
- R2215 and R2216 check a member as its trait's method and make `_Self`
  the trait's binder.
- R2217 keys a default by its typed signature and deletes the AST
  spelling path in `mojito-symbol`.
- R758 gives a requirement its checked signature. That fixes a
  convention misread that R1329's register drops would turn into a
  double destroy.
- R640 reads the wrapper's call node instead of a span-keyed lookup.
- R769 plans materializing a requirement's default at the call, which
  replaces a syntax rewrite and a second check pass.

```sh
scripts/claude_loop.py --model opus --plan-fable-on-opus -n 0 \
  --task R2215 --task R2216 --task R2217 --task R758 --task R640 \
  --task R769
```

## 6. Annotations are expressions (4 entries)

Upstream parses a type annotation and a parameter bound as an ordinary
expression, and sema evaluates it in a type context. Mojito has two more
grammars for them.
- R2134, R2135, and R2136 parse annotations and bounds with
  `parse_expression`. The stop condition is a parse dump of `assets/` and
  `stdlib/` that differs only in `SyntaxId` lines.
- R2137 plans storing the expression in the AST and deleting `Type` as a
  parser product. That is about 350 use sites.
- When syntax changes, `docs/grammar.md` is updated first.

```sh
scripts/claude_loop.py --model opus --plan-fable-on-opus -n 0 \
  --task R2134 --task R2135 --task R2136 --task R2137
```

## 7. The check emits its typed nodes as it checks (5 entries)

Upstream's parser emits typed ops while it checks. Mojito records facts in
a span-keyed map, then walks the syntax again (`finalize`,
`ArenaBuilder`) to build `CheckedProgram`. This is the largest structural
gap left above the waist. The MIR text and the package bytes must not
change.
- R764 to R767 cite plan files that are no longer in the tree
  (`R624-…-plan.md`, `R625-…-plan.md`). `--plan-fable-on-opus` writes a
  new plan before each Fable session. Expect the first session to do
  mostly planning.
- R768 deletes `CheckedDeclaration`, which repeats the statement nodes.

```sh
scripts/claude_loop.py --model opus --plan-fable-on-opus -n 0 \
  --task R764 --task R765 --task R766 --task R767 --task R768
```
