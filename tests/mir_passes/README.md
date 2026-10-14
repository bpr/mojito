# MIR pass tests

`tests/mir_pass_test.rs` runs one trial per `<pass>/<case>.mir` here, after
MLIR's `mlir-opt` golden tests. The directory names a registered pass
(`analysis::pass_named`). A trial checks the input is canonical, runs the
pass through `analysis::opt` (which verifies after the pass), prints the
`functions:` section with `mir::text::disassemble_functions`, and compares
it with `<case>.expected`. Only that section is pinned, so a MIR format bump
breaks no expected file; the reader still loads an input written at an older
minor version.

## Adding a case

1. Write a small Mojo program outside the repository and print its MIR with
   `mojito emit-mir`. A pass in `analysis::PIPELINE` takes this post-drop
   form. `elaborate-drops` takes pre-drop MIR, which the CLI does not print,
   so for it either remove the `drop.*` lines (when no copy elision or
   deinit refinement is in play) or emit with drop elaboration skipped in a
   scratch build.
2. Trim the artifact by hand to the case's own functions, their `decls`
   rows, and the `structs` rows they name; empty `traits`. Rename the
   program's own `files` path to a bare file name.
3. Make it canonical: `mojito opt case.pre.mir > <pass>/<case>.mir`.
4. Bless the expectation:
   `UPDATE_EXPECT=1 cargo nextest run --test mir_pass_test`, then read the
   new `<case>.expected` and check it shows what the case is about.

`mojito opt --pass NAME [--pass NAME]… FILE` prints the whole artifact after
the passes, for inspecting a case by hand.
