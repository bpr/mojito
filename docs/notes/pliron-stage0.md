# Pliron Stage 0: Feasibility, Exact Pin, and Dependency Isolation

Outcome record for the roadmap section 4 "Pliron Stage 0" task (completed
2026-08-18). The spike lives in `spikes/pliron-stage0/` as a standalone crate
outside the production build; its gate is `scripts/check-pliron-spike`.

**Verdict: GO for Stage 1.** All acceptance criteria pass and none of the four
material-failure conditions triggered:

- A pinned Linux gate emits and executes `main -> i32` (exit code and JIT
  result both 42): `t01_construct_and_jit.rs`, `t06_export_object_exec.rs`.
- Invalid IR reports a source-associated `Result` diagnostic and never
  panics: `t03_verify_failure.rs`.
- The default VM lane needs no native toolchain: `spikes/` is not a workspace
  member, `scripts/check` is unchanged, and
  `tests/backend_isolation_test.rs` guards the root lockfile.
- No required API needed any Pliron fork; every gap found is minor and
  locally bridgeable (details below).

## Pin Record

| Component | Pin | Evidence |
| --- | --- | --- |
| `pliron` | `=0.17.0` (crates.io, released 2026-08-07) | sha256 `9e51eb8628227038376c81dcb1752db90a90af24a1e38b27582c7dab235149d7` |
| `pliron-llvm` | `=0.17.0` (lockstep: it pins `pliron =0.17.0`) | sha256 `511535a077e4f72f83c437cde2636a6f525809a370b3078c0d1d5d53612ad052` |
| `pliron-derive` | `0.17.0` (transitive) | sha256 `f878ccd6bfd39a9373175774b96785daab296dc1870bc72ff7d285151f613a56` |
| `llvm-sys` | `221.0.1` (pliron-llvm requires `^221` → LLVM 22) | sha256 `2abcc34a3b190f03c2a61b555f218f529589ff13657bdd2ff8ac3e85f2abe6bb` |
| LLVM | 22.1.8 (`llvm-22-dev`, `/usr/lib/llvm-22`) | `llvm-config --version` |
| Rust | 1.96.1 (repo `rust-toolchain.toml`; pliron needs edition 2024 ≥ 1.85, stable, no nightly) | |
| Cargo features | defaults only (`pliron-llvm`'s default `llvm-sys` feature enables the native path) | spike `Cargo.toml` |

The spike's committed `Cargo.lock` (79 packages) is the authoritative pin;
`.gitignore` was adjusted (`!spikes/*/Cargo.lock`) so it stays tracked.

- Discovery: `LLVM_SYS_221_PREFIX=/usr/lib/llvm-22` (exported with that
  default by `scripts/check-pliron-spike`); llvm-sys falls back to
  `llvm-config` on PATH, which is also LLVM 22 on this machine.
- Required system packages (spike lane only): `llvm-22-dev`, `clang`
  (`clang-22` preferred, plain `clang` fallback; both are 22.1.8 here).
- Upstream: repo moved to `pliron-org/pliron`; Apache-2.0; ~609 commits; very
  active (pushed 2026-08-17) but effectively one maintainer (vaivaswatha,
  461 commits; #2 contributor is a CubeCL developer). Monthly breaking 0.x
  minors with no CHANGELOG. docs.rs for pliron-llvm is broken (upstream
  issue #61) — read vendored sources under
  `~/.cargo/registry/src/*/pliron{,-llvm}-0.17.0/` instead; the published
  crates ship their integration tests, which are the best API models
  (`pliron-llvm/tests/compile_run.rs`, `pliron/tests/ir_construct.rs`).

## Ecosystem Audit (2026-08-18)

- **Kaleidoscope**: in-repo workspace member (`kaleidoscope/`), the primary
  end-to-end tutorial, built by upstream CI against head. The README also
  documents compiling bzip2 and lua end-to-end via `llvm-opt` bitcode round
  trips — stronger evidence than the tutorial.
- **CUDA Oxide (NVlabs)**: NVIDIA's experimental Rust→PTX rustc backend
  consumes the upstream `pliron-llvm` crate with exactly the architecture
  Mojito plans: a custom `dialect-mir` modeling rustc MIR, lowered through
  `dialect-llvm`/`dialect-nvvm` to textual LLVM IR. Their stated reason for
  Pliron over MLIR is the pure-cargo build (no C++/CMake/tablegen).
- **CubeCL (Tracel)**: connected through `pliron-spirv` and `tracel-rspirv`
  (both depend on `pliron ^0.17`); whether shipped mainline CubeCL uses them
  yet is unconfirmed. A CubeCL developer is the #2 pliron contributor.
- Other reverse dependencies: `plirun` (third-party pure-Rust interpreter for
  pliron dialects), `pliron-inspect-driver`, `tensor-wasm-jit`.

## Facility Classification Matrix

| Facility | Class | Evidence |
| --- | --- | --- |
| Operations (define, build, mutate) | supported | `#[pliron_op]` derive; `t01`, `t05`; `spike_dialect.rs` |
| Types | supported | `#[pliron_type]`; LLVM dialect reuses builtin `IntegerType` — integer/float types live in the *builtin* dialect, `llvm.func`/pointers/aggregates in the LLVM dialect |
| Attributes | supported | `IntegerAttr`, op attr dictionaries, outlined `!N` attributes; `t01`, `t02` |
| Blocks / SSA | supported | MLIR-style block arguments (no phi); def-use chains with `replace_some_uses_with`; `t01`, `const_fold.rs` |
| Dominance | supported (not exercised) | upstream `DomTree` + `fast_ssa_liveness`; mem2reg depends on them; open #99 (post-dominators) |
| Pass invalidation | supported | `PassManager`/`Pass`/`AnalysisManager` with `IRStatus`-driven invalidation; `t04` |
| Conversion legality | locally bridgeable | `irbuild::dialect_conversion` (`DialectConversion` + `PassWrapper`) drives rewrites, but there is no MLIR-style legality/target declaration — totality is asserted by hand (`count_ops_in_dialect == 0`, `t05`) |
| LLVM dialect coverage | supported (scalar subset) | full integer arith/icmp/branch/call/func/return/memory set; caveats: no LLVM-dialect float types (builtin FP only), `llvm.constant` limited to int/float, ConstFold coverage tracked upstream (#118) |
| Data layout | upstream gap / bridgeable | no data-layout modeling in pliron core; layout is decided by LLVM at emission. Mojito's shared native layout contract (roadmap section 4) must own layout anyway, so this stays a producer responsibility |
| JIT / targets | supported (JIT), locally bridgeable (objects) | ORC LLJIT wrapper executes in-process (`t01`, `t05`); no shipped object-emission API — bitcode → `clang` produces the executable (`t06`); `llvm_sys/target.rs` bindings exist for direct object emission later |
| Diagnostics | supported | `Result` + `ErrorKind` + `Location`; parsing attaches line/column locations rendered as `[<file>: line: L, column: C] Compilation error: ...`; `t03` |
| Parsing / printing | supported with caveat | see finding 2; canonical form requires name erasure (`canonical_text`), then byte-stable (`t02`, `t05`) |

## Findings

1. **The dialect-conversion framework exists** (0.17.0:
   `pliron::irbuild::dialect_conversion`, plus pliron-llvm's `ToLLVMDialect`
   op interface and `builtin_to_llvm` as the model pass). Earlier repo
   browsing that concluded "no conversion framework, hand-written passes
   only" is out of date; only the *legality declaration* layer is missing.
2. **Plain `parse -> print` is not a fixpoint**: the parser stores each
   parsed block label as a given name and the printer re-suffixes it with the
   internal id, so block labels grow every round (`block1v1` →
   `block1v1_block1v1` → …). Value names and locations stabilize after one
   round. Byte-stable canonical text requires
   `debug_info::erase_given_names` before printing (spike `canonical_text`);
   upstream issue candidate.
3. **`llvm.constant` is missing from the `SideEffects`-false list**
   (`pliron-llvm/src/interface_impls.rs`), so built-in DCE conservatively
   keeps dead constants (`t04` pins this). One-line upstream patch candidate.
4. **First parse attaches locations** as outlined `!N` attributes, so a
   constructed module and its reparse print differently on round one; this is
   also what makes verifier diagnostics source-associated (`t03`).
5. **`to_llvm_ir` folds constant operand chains at emission**: the exported
   module for `40 + 2` is directly `define i32 @main() { ret i32 42 }`.
6. **expect-test needs `CARGO_WORKSPACE_DIR`** pinned for `UPDATE_EXPECT=1`
   runs in the spike (otherwise it resolves snapshot paths against the outer
   mojito repo); `scripts/check-pliron-spike` exports it.

## Spike Map (test → facility)

- `t01_construct_and_jit` — programmatic construction, verification, LLVM
  module conversion, LLJIT host execution (42).
- `t02_roundtrip` — print snapshot; canonical parse → print byte stability.
- `t03_verify_failure` — constructed-invalid non-panic `Result`; parsed
  invalid IR with located diagnostic; located parse error.
- `t04_passes` — hand-written `FoldConstAdd` + built-in DCE in one pipeline;
  pins the dead-constant DCE gap.
- `t05_conversion` — `spike.answer` textual syntax, verifier positive and
  negative, total lowering via the dialect-conversion framework, legality
  walk, JIT of the converted module.
- `t06_export_object_exec` — textual `.ll` snapshot, bitcode export,
  `clang`-linked native executable exiting 42.

Main crate: `tests/backend_isolation_test.rs` asserts the root `Cargo.lock`
contains no `llvm-sys`/`pliron`/`pliron-llvm`. **Stage 1 caveat**: once
`pliron` becomes an optional mojito dependency it will appear in the root
lockfile even for default builds; replace the guard then with a feature-gated
check (e.g. `cargo tree --no-default-features` must show no `llvm-sys`).
`Cargo.toml` reserves the empty `backend-pliron` feature as that seam.

## CI Recipe (Linux)

Not yet wired to any hosted CI (the repo has none); `scripts/check-pliron-spike`
is the committed, CI-ready gate. A future workflow is a transcription of:

```sh
apt-get install -y llvm-22-dev clang    # clang-22 preferred at runtime
rustup toolchain install 1.96.1
export LLVM_SYS_221_PREFIX=/usr/lib/llvm-22
scripts/check-pliron-spike              # fmt + nextest + clippy -D warnings
```

The default lane stays `env RUSTC_WRAPPER= scripts/check` with no LLVM.

## Pin-Update Policy

Upstream releases breaking 0.x minors roughly monthly, in `pliron` +
`pliron-llvm` lockstep, without a CHANGELOG. Policy: stay on the exact pin;
consider a bump only deliberately (at a stage boundary, or to pick up a needed
fix), re-running the full spike gate as the upgrade rehearsal the promotion
decision requires. An LLVM major bump rides along with llvm-sys and needs the
matching `llvm-XX-dev` + `LLVM_SYS_XX?_PREFIX` update in
`scripts/check-pliron-spike` and this record. This recurring cost feeds the
"version/update burden" material-failure axis and is accepted for now.
