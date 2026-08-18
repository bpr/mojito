# AGENTS.md

Repository guidance for all coding agents. Tool-specific setup (e.g. Claude
Code's context imports) lives in the per-tool file that points here, such as
`CLAUDE.md`.

## Start Here

Mojito is a Rust compiler for a strict, executable subset of current Mojo. The
register VM is the sole runtime and executable oracle; there is no tree-walking
execution path.

**Backend direction.** The end goal is native code below the verified-MIR waist.
The prioritized native backends are
[Pliron](https://github.com/pliron-org/pliron) — a Rust-native, MLIR-inspired IR
framework whose LLVM dialect emits LLVM IR; `docs/pliron_plan.md` is the staged
adoption plan — and Cranelift, with a C or C++ source backend as a possible
additional target. Direct LLVM or MLIR lowering and eBPF are no longer
prioritized. No backend IR is a *required internal* compiler layer: backends sit
below the MIR waist, MIR remains the serialized backend-independent handoff, and
the VM remains the executable semantic oracle. See `roadmap.md` section 6 for
the ordering.

Read these documents before changing behavior:

- `docs/features.md` — authoritative support matrix.
- `docs/symbol-map.md` — symbol-level ownership and navigation map.
- `docs/architecture.md` — pipeline invariants and phase design.
- `grammar.md` and `docs/frontend.md` — accepted syntax and parser design.
- `roadmap.md` — current direction, pending work, and task lifecycle policy.

Do not copy the feature inventory into this file. Update `docs/features.md` when
support changes and update `docs/symbol-map.md` when ownership or entry points
move.

## Non-Negotiable Invariants

1. Mojito is a strict subset of current Mojo. Accepted programs must use valid
   Mojo syntax and semantics; Mojito may reject valid Mojo but must not invent a
   different language.
2. Unsupported semantics fail explicitly. Prefer an early, contextual checker
   error; use `MirInstr::Unsupported` or `RuntimeError::Unsupported` only for a
   genuine later-phase boundary.
3. `Compiler` owns the production pipeline:

   ```text
   source -> lex -> parse -> link -> comptime elaboration -> CheckedProgram
          -> HIR CFG -> MIR -> ownership/liveness -> drop elaboration -> VM
   ```

4. `CheckedProgram` is the semantic handoff. Later phases consume checked facts;
   they do not silently re-check or recover unchecked execution.
5. MIR is the stable waist. Backends consume register-typed MIR that has passed
   `mir::verify` plus ownership analysis, with checked declaration metadata,
   rather than rediscovering language rules from AST syntax.
6. `src/call.rs` owns structural call binding and `src/symbol.rs` owns callable
   identity. Do not duplicate either policy in the checker, MIR, or VM.
7. Preserve source/module provenance on every AST and lowered location.

## Working Practices

- Inspect the worktree before editing. Existing staged and unstaged changes belong
  to the user; preserve unrelated work.
- Order items public-first, strictly. Rust ignores item order, but readers should
  not: put every public item at the top so a top-down read shows what the file
  exports. Layout within a file: module docs and `use`/`mod` imports; then public
  items (`pub`, then `pub(crate)`/`pub(super)`), with a public type immediately
  followed by its own `impl` blocks; then private items; then `#[cfg(test)]`
  modules last. Public functions bubble to the top even when that separates them
  from a private helper only they use. Within the private section, order by
  fan-in: a helper called by many items sits higher, and the more solitary a
  helper is the lower it sinks.
- Prefer ripgrep (`rg`) for repository searches, and make focused, reviewable
  edits.
- Add positive and negative tests at the phase that owns the rule.
- Parser support is not semantic support. Keep those states distinct in code,
  diagnostics, and `docs/features.md`.
- When syntax changes, update `grammar.md` first and parser tests with the code.
- When public pipeline or symbol ownership changes, update
  `docs/architecture.md` and `docs/symbol-map.md`.
- Keep comments about current invariants. Historical comparisons belong in design
  notes or commit history, not production-code commentary.

## Commands

- Required gate: `env RUSTC_WRAPPER= scripts/check`
- Full suite: `cargo nextest run`; iteration loop:
  `cargo nextest run --profile quick` (excludes the per-fixture corpus
  binary). Plain `cargo test` remains a working fallback.
- One integration target: `cargo nextest run --test vm_test`
- One named test: `cargo nextest run test_name`
- CLI: `cargo run -- <lex|parse|check|own|run> [FILE]`
- Module roots: repeat `--module-path PATH` / `-I PATH`; use `--stdlib PATH`
  to replace the bundled standard-library root.

Do not report a task complete until formatting, tests, Clippy with warnings denied,
and `git diff --check` pass.

## Test and Fixture Ownership

Integration tests are grouped by phase: lexer, parser, checker, comptime, HIR,
MIR, ownership, drops, VM, modules, symbols, compiler driver, and self-hosted
stdlib. `tests/evaluator_test.rs` is a historical filename; it exercises the
compiler-and-VM execution path.

Files under `assets/<outcome>/` run through the whole pipeline as one
generated test per fixture in the `tests/corpus_test.rs` binary
(`harness = false`, libtest-mimic), grouped as `assets_*`, `vm_ok`,
`verify::*`, `origin_*`, and `ownership_*` — each group pinning a distinct
pipeline entry path; the phase-grouped files keep only targeted tests. The
outcome folders are `ok`, `parse_error`, `type_error`, `runtime_error`,
`ownership_ok`, and `ownership_error`. See `assets/README.md`.

The stage-composed test seam (`link`/`parse` → `elaborate` →
`check_program` → `backend.run`) enforces the same pre-drop ownership
contract as the production `Compiler`: `VmBackend::run` runs the ownership
analysis before executing. It remains non-authoritative only for the
whole-program discovery/specialization handoff, which `Compiler`-based
helpers (`run_compiled`) still own.
