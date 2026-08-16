# VM Artifact Execution Plan

## Objective

Close roadmap §3's "VM artifact execution": run verified textual MIR artifacts
directly from the CLI. This milestone owns the elaborated-MIR execution entry
(`VmBackend::run_elaborated`/`Backend::run_elaborated`), the load-then-execute
composition (`artifact::run_artifact`), and the `mojito exec [FILE]`
subcommand. It does **not** own compiler dump flags or artifact-based
conformance contracts; those remain the roadmap §3 compiler/test-integration
task.

## Deliverables

- `VmBackend::run_elaborated(mir: MirProgram)` (src/backend/vm.rs) and the
  matching `Backend::run_elaborated` static-enum method: build the VM's
  `Prog` (structs/sigs from `mir.declarations`) and execute via `run_prog`,
  refusing only a non-empty `invariant_errors`.
- `artifact::run_artifact(input, source_name, backend) -> Execution`
  (src/artifact.rs) with `ArtifactRunError { Load, Backend, Runtime }` — the
  composition of `mir::text::load_artifact` with backend execution, shared by
  the CLI and tests.
- CLI `mojito exec [FILE]` (src/main.rs): reads raw bytes (file or stdin) so
  the artifact parser owns UTF-8/BOM validation, prints the program's output
  verbatim, and renders loading diagnostics with `label:line:col`, the
  offending line, a caret, and the artifact-path context (byte ranges when
  the input is not valid UTF-8).
- `tests/artifact_exec_test.rs`: snapshot smoke runs, the frozen executable
  `tests/snapshots/mir/exec_print.mir` (generated from `def main():
  print(42)` through the unlinked seam — prelude linking would embed
  machine-specific stdlib paths), output/bindings equivalence between
  artifact execution and the direct VM run on four fixtures, and
  loading-gate/backend-refusal negatives.

## Decisions

- **Execution runs the serialized program exactly as written.** The artifact
  entry never calls `elaborate_drops_program` (it is not idempotent — it
  would splice a second `DropVar` schedule over the serialized one), never
  re-runs `mir::verify` (nothing rewrites the program between the loading
  gate and execution; `verify_artifact` already folds `invariant_errors`
  into its findings), and never runs `check_ownership_program` (a pre-drop
  analysis with no meaning on elaborated MIR).
- **Trust boundary: verify-at-load is the consumer gate.** Ownership
  analysis and drop elaboration are producer obligations the schema cannot
  re-check; canonical artifacts serialize only analyzed, drop-elaborated
  programs. The format doc's closing contract sentence now states this
  split precisely.
- **`Backend::run(&CheckedProgram)` is untouched.** The stage-composed test
  seam keeps its pre-drop ownership contract; artifact execution is a
  separate entry, not a contract change. The `CompiledProgram` MIR-caching
  follow-up is retargeted to the compiler/test-integration task — the
  artifact path bypasses the `Compiler` entirely.
- **`exec` never echoes bindings.** Artifacts are compiled programs; output
  parity mirrors file-based `run`'s differential rationale. `Execution`
  bindings stay available to library callers and tests.
- **The helper lives in a new `src/artifact.rs`.** `Compiler` owns the
  source pipeline (AGENTS invariant 3) and the artifact path deliberately
  bypasses it; `mir::text` cannot host the composition without inverting
  layering onto `backend`.
