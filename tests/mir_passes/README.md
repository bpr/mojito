# MIR pass tests

`tests/mir_pass_test.rs` runs one trial per `<pass>/<case>.mir` here, after
MLIR's `mlir-opt` golden tests. The directory names a registered pass
(`analysis::pass_named`). A trial checks the input is canonical, runs the
pass through `analysis::opt` (which verifies after the pass), prints the
`functions:` section with `mir::text::disassemble_functions`, and compares
it with `<case>.expected`. Only that section is pinned, so a MIR format bump
breaks no expected file; the reader still loads an input written at an older
minor version.

## Error cases

A case with a `<case>.error` file beside it expects failure, after
`mlir-opt -verify-diagnostics`: the trial runs the pass through
`analysis::opt`, skips the canonical check, and compares the error's
report with `<case>.error`. `opt` verifies its input when it loads it, so
an input that does not verify fails at load, before the pass runs
(`elaborate-drops/double_drop.mir` is the doubled-drop reproducer). A pass
whose output does not verify is pinned by a unit test in
`analysis::opt` instead. Create an empty `<case>.error` and bless it as
below. Labels are `<pass>/<case>.mir`, so the reports hold no machine path.

## Adding a case

1. Write a small Mojo program outside the repository and print its MIR with
   `mojito emit-mir`. A pass in `analysis::PIPELINE` takes this post-drop
   form; emit it with `MOJITO_PRE_ELABORATION=0`, or the pipeline will
   already have run (`eliminate-dead-symbols` would have removed what its
   case is about). The CLI prints that form structured, every region one
   block, as every pipeline pass reads it. Only `canonicalize`'s generic
   folds and erasures and `cse` take a lowered body, which only their
   existing cases still hold; `elaborate-drops`, `sccp`,
   `mem-2-reg`, `lower-arg-conventions`, and `simplify-cf` refuse one,
   as Mojo has no `cf` dialect. This form is also past `lower-lit`, so it holds no
   `lifetime.keep_alive` or `interiors.invalidate`. `elaborate-drops` takes
   pre-drop MIR in its structured form (every region one block, as
   `verify_structured_shape` checks; `opt` refuses a lowered body), which
   the CLI does not print, so for it emit with drop elaboration skipped in a
   scratch build, or rebuild the printed body's regions by hand (`if`,
   `loop`, `break`/`continue`/`yield`, as in
   `simplify-cf/*.mir`) and remove the `drop.*` lines (when no copy
   elision or deinit refinement is in play). Its output keeps the
   regions, as every pipeline pass reads them. A
   `lower-lit`
   case needs the markers, so emit it with a binary from before the pass or
   from pre-drop text, as for `elaborate-drops`.
2. Trim the artifact by hand to the case's own functions, their `decls`
   rows, and the `structs` rows they name; empty `traits`. Rename the
   program's own `files` path to a bare file name.
3. Make it canonical: `mojito opt case.pre.mir > <pass>/<case>.mir`.
4. Bless the expectation:
   `UPDATE_EXPECT=1 cargo nextest run --test mir_pass_test`, then read the
   new `<case>.expected` and check it shows what the case is about.

`mojito opt --pass NAME [--pass NAME]… FILE` prints the whole artifact after
the passes, for inspecting a case by hand.
