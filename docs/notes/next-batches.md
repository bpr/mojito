# Next Batches: The Pliron Gate Lanes

The batches to run after the pmir, comptime, and call-op batches, which
have all landed. Each batch is one `scripts/claude_loop.py` command.
`--task` runs each named entry after its open prerequisites, so a batch may
run a few entries it does not name. Preview one with `--list` or
`--dry-run`. `scripts/run-batches` reads the commands below, one batch
each, and runs focused and the instantiation ratchet after every batch.
Delete a batch here once it has landed.

The goal is a green `check-pliron` and `check-pliron-heavy` in
`scripts/run-overnight-gates`. The 2026-11-15 gate run (at 9e4431676)
passed every other step except the instantiation ratchet, which is left for
later. A fixture that is red until its entry lands is commented out in
`tests/focused/list.txt` behind the entry's ID, and the session that lands
the entry restores the line.

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

## 1. Gate repair: the Pliron lanes (4 entries)

What `check-pliron` and `check-pliron-heavy` failed on 2026-11-15:
- R2231: the Pliron capability tables no longer match the MIR instruction
  and type vocabulary, failing two `mojito-pliron` unit tests. This is
  `check-pliron`'s only failure. Its bench-smoke step, which the failure
  skipped, passes.
- R2232: the parity sweep requires empty native stderr, but
  `print_library_keywords` prints to stderr as the pin does. The VM must
  capture fd 2 so that stderr can be compared with it.
- R2233: `maybe_uninit_deinit_uninitialized` stopped compiling natively
  ("struct 'Bool' has no MIR declaration to lay out"). The panic in
  R2232 hides it today.
- R2234: then run both lanes once. Their runtime-error half, manifest
  comparison, and coverage ratchets have not run since 2026-11-01. It
  files anything still red as a new batch 2.

Focused line to restore: `maybe_uninit_deinit_uninitialized` (R2233).
After the batch, run `scripts/check-pliron` and
`scripts/check-pliron-heavy` to confirm the goal. The two take about 20
minutes together, and `run-batches` does not run them.

```sh
scripts/claude_loop.py --model opus --plan-fable-on-opus -n 0 \
  --task R2231 --task R2232 --task R2233 --task R2234
```

## 2. Gate repair: what the 2026-11-15 heavy lane run left red (2 entries)

R2234 ran both lanes. `check-pliron` failed only on a Clippy lint in
`tests/heavy/main.rs`, now fixed. The heavy lane's parity sweep showed:
- 11 `assets/ok` rows that no longer compiled natively. R2234 fixed both
  causes (native layout refused an instance keyed by a closed application,
  and `inline-parametric` wrote `None` into a reference to `None`).
- R2235: a pending error that a `finally` or `__exit__` swallows is freed
  natively through a poison value. `pliron_finally_overrides` and
  `tempfile_mkdtemp` stop in Pliron's verifier.
- R2236: the sweep requires a trap category from
  `simd_element_list_too_few`, whose read past the element pack is
  undefined. Its SIGSEGV panics the sweep before the manifest comparison.

Focused lines to restore: `pliron_finally_overrides` and
`tempfile_mkdtemp` (R2235), and `simd_element_list_too_few` (R2236).
After the batch, run `scripts/check-pliron-heavy` to confirm the goal.

```sh
scripts/claude_loop.py --model opus --plan-fable-on-opus -n 0 \
  --task R2235 --task R2236
```
