# Pliron Stage 2: Complete Scalar Execution and Conversion Legality

Outcome record for the roadmap section 4 "Pliron Stage 2" task (completed
2026-08-19). Builds on the Stage 1 backend (`docs/notes/pliron-stage1.md`);
pins are unchanged (`pliron`/`pliron-llvm` `=0.17.0`, llvm-sys 221, LLVM 22,
`LLVM_SYS_221_PREFIX=/usr/lib/llvm-22`, clang-22 and opt-22 subprocesses).
The gate remains `scripts/check-pliron`.

All roadmap acceptance criteria pass:

- Every eligible scalar `run` conformance case matches VM output and
  result/trap category at `O0` and `O1`
  (`tests/pliron_backend_test.rs::scalar_capability_manifest_and_differential`).
- The generated capability manifest `conformance/pliron-scalar.tsv` names
  every `assets/ok` and `assets/runtime_error` fixture exactly once with its
  status (`differential`, `trap-differential`, `excluded` with the first
  rejection diagnostic, or `ineligible`).
- Guard assertions fail when differential coverage drops below its floor,
  trap coverage drops below four categories-with-fixtures, or a
  pliron-named fixture regresses to `excluded`.
- The bindings criterion is vacuous by construction: production compilation
  rejects executable module-scope statements, so file-based programs always
  finish with empty `__toplevel__` bindings. Recorded here so the acceptance
  reading is explicit rather than silently skipped.

## Scalar surface

`ScalarTy` is now `Int | UInt | Float64 | Bool` (`Int`/`UInt` share the
signless i64 representation and differ only in operator selection; `Float64`
is `builtin.fp64`). Width-1 SIMD scalar aliases (`Int32`, `UInt8`, `Byte`,
...) stay excluded — they route through `MakeSimd`/`SimdCast` and per-dtype
wrapping, deferred with the SIMD surface.

Operator lowering matches `src/runtime.rs` exactly (the VM is the oracle):

| operator | Int | UInt | Float64 |
| --- | --- | --- | --- |
| `+ - *` | wrapping `add/sub/mul` | same ops | `fadd/fsub/fmul` |
| `/` | `sitofp` both + `fdiv` (Float64 result) | `uitofp` + `fdiv` | `fdiv` |
| `//` | zero trap, `sdiv` + floor select | zero trap, `udiv` | `fdiv` + `llvm.floor` (no trap) |
| `%` | zero trap, `srem` + divisor-sign select | zero trap, `urem` | `x - y*floor(x/y)` (NOT `frem`, no trap) |
| `**` | exponent trap, `mjrt_pow` | same | `llvm.pow.f64` |
| `<< >>` | mask `&63`; `shl`/`ashr` | mask `&63`; `shl`/`lshr` (logical) | — |
| `& \| ^` | `and/or/xor` | same | — |
| compare | signed `icmp` | unsigned `icmp` | `fcmp` OEQ/**UNE**/OLT/OLE/OGT/OGE |
| unary `-` | `0 - x` | rejected (checker rejects) | `fneg` |

Float comparison honors Rust f64 semantics: `!=` is unordered-or-unequal
(`fcmp une` — true for NaN operands), the ordered comparisons are false for
NaN. `Bool` still supports only `== != & | ^` and `not`.

## Conversions

`Int(x)`/`UInt(x)`/`Float64(x)`/`Bool(x)` intercept by name at the call site
when no MIR function of that name exists — the same by-name policy as the VM
(`vm.rs` builtin dispatch → `runtime::builtin_convert`):

- Float→integer is Rust `as`: **saturating, NaN → 0** — lowered as the
  `llvm.fptosi.sat.i64.f64` / `llvm.fptoui.sat.i64.f64` intrinsics (plain
  `fptosi` is poison out of range and would miscompile).
- `Int(u)`/`UInt(i)` reinterpret bits (no-op alias); integer→float is
  `sitofp`/`uitofp`; Bool widens by `zext`/`uitofp`.
- `Bool(x)` is a non-zero test: `icmp ne 0` / `fcmp une 0.0` (so
  `Bool(NaN)` is `True`, matching `x != 0.0`).
- Literal arguments fold at compile time through the same wrapping/truncating
  paths as `builtin_convert`'s literal branches (`wrapping_signed(64)`,
  `wrapping_unsigned(64)`, `FloatLiteral::trunc_to_int`, `to_f64`).

Literal materialization itself now mirrors `runtime::materialize_literal`
exactly: integer literals **wrap modulo 2^64** into `Int`/`UInt` (Stage 1's
`to_i64` range rejection was a recorded divergence, now fixed — at 64-bit
targets the VM's wrapping conversion never fails; `LiteralOutOfRange`
survives only as a defensive impossible-case diagnostic). Pending literals
(`IntLiteral` and now `FloatLiteral`) stay exact until a consumer fixes their
target kind; direct consumers of literal registers materialize at the
consumer's operand kind (the other operand's concrete type for `BinOp`s).

## Checked traps (no Mojito dialect op)

Two trap categories exist (`TrapCategory` in `src/backend/pliron.rs`):

| category | code | exit status | VM message |
| --- | --- | --- | --- |
| DivModZero | 1 | 65 | `integer division or modulo by zero` |
| PowExponent | 2 | 66 | `'**' exponent must be a non-negative Int that fits in 32 bits` |

A guard compare branches to a lazily created per-function, per-category trap
block that calls the C `exit(64 + code)` and ends in `llvm.unreachable`. This
closes Stage 1's recorded div-by-zero UB divergence. The roadmap's "checked
traps" boundary needed **no `mojito` dialect operation**: guards, trap
blocks, the `exit` declaration, and the intrinsic calls are all existing
LLVM-dialect ops with textual syntax and verifiers; a custom op would add a
dialect, a conversion rule, and negative coverage for zero semantic gain.

The `**` exponent guard is one unsigned compare (`icmp ugt exp, u32::MAX`
covers negative-as-i64 and oversized in one test — `runtime::pow_exp`'s
accepted range). `mjrt_pow` is a lazily emitted module-private wrapping
square-and-multiply helper shared by Int and UInt (wrapping i64/u64
multiplication is bit-identical). Runtime helper names (`exit`, `mjrt_*`,
wrapper `main`) sit outside the injective `mj_` mangle image.

## Recorded VM/native divergence policies

- **Int/UInt `+ - *` overflow**: *closed by the shared native ABI milestone*
  (`docs/native-abi.md`) — defined two's-complement wrapping on both
  backends, including wrapping `**` (VM `wrapping_pow` = native `mjrt_pow`)
  and the defined `i64::MIN // -1 == i64::MIN` / `MIN % -1 == 0` case (the
  native lowering sanitizes the `sdiv`/`srem` poison divisor). Pinned by the
  `assets/ok/pliron_wrap_*` differential fixtures at `O0`/`O1`.
- **Float `**`**: VM `f64::powf` and native `llvm.pow.f64` both resolve to
  the host libm `pow` — exact match on one host, unspecified across libms.
  Cross-platform bit-exactness is deliberately not claimed.
- **Literal-literal arithmetic**: the VM computes exactly (BigInt/rational)
  and wraps at materialization; native materializes each literal operand at
  the consumer kind first. Comptime folding makes runtime literal-literal
  `BinOp`s rare; in-range values are identical. Same policy family as
  Stage 1's literal shift amounts.

## kwargs and constant defaults

MIR `Call`s keep keyword arguments and defaults unflattened; the backend now
binds them with `crate::call::match_call_slots` — the same structural policy
the VM uses (`src/call.rs` owns call binding; the backend duplicates
nothing). `ArgSlot::Default` folds the declaration's pre-folded
`CheckedConst` literal at the parameter's scalar type through the
pending-literal machinery. Variadics, keyword variadics, `param_arg_regs`,
place-carrying arguments, and capture effects stay rejected.

## for-range stays excluded

In linked production programs `range(...)` builds the self-hosted stdlib
range structs (`_ZeroStartingRange` et al.) whose iteration protocol is
ordinary methods: struct fields (width-1 SIMD scalars), a borrowed
`__iter__` receiver, and a raising `__next__`. The checker's scalar-triple
fast path (`checker/iteration.rs`) fires only in discovery/compatibility
mode, never in lowered production MIR. Lowering ranges natively would mean
reimplementing stdlib Mojo semantics inside the backend — the exact
anti-pattern the MIR-waist invariant forbids — so `GetIter`/`HasNext`/`Next`
stay rejected (manifest-recorded) until the struct/method stage. Supported
scalar control flow: `if`/`elif`/`else`, `while`, ternary, short-circuit
`and`/`or`, early return, recursion — all plain CFG.

## Optimization pipeline and typed JIT

- `OptLevel::O0` (default) keeps the Stage 1 pliron pipeline (per-function
  `mem2reg` + `dce`). `OptLevel::O1` additionally round-trips the emitted
  bitcode through subprocess `opt -passes='default<O1>'` (candidates
  `opt-22`, `opt` — the same policy as clang discovery). pliron-llvm 0.17
  keeps `LLVMModuleRef` private, so the in-process new-pass-manager is
  unreachable; the subprocess mirrors the established clang seam. The JIT's
  `O1` path reparses the optimized bitcode in a fresh `LLVMContext` kept
  alive alongside the LLJIT.
- `NativeModule::jit_value(entry, opt) -> JitValue {Int, UInt, Float64,
  Bool}` types the harness by the entry's checked return kind. LLVM `i1`
  returns leave the upper register bits undefined, so Bool entries read as
  `extern "C" fn() -> u8` masked to the low bit. `jit_i64` survives as an
  Int-asserting `O0` wrapper.
- CLI: `--native-opt {0|1}` (default 0) on `compile` and
  `run --backend pliron`.

## `run --backend pliron`

`run --backend pliron` now executes the advertised subset natively: the
production pipeline's cached post-drop MIR compiles from `main` (plus
`__toplevel__`), links a temporary executable, runs it, and forwards stdout
and the exit status. There is still no native `print` (Stage 3 owns the
runtime), so real printing programs reject at the `print` call site with the
contextual diagnostic — never a silent VM fallback. Trap exit codes map back
to the VM's `Type error: ...` text for user-facing parity. The `Backend`
enum stays VM-only: the native path consumes `MirProgram` below the waist
and is dispatched in the CLI, not through `Backend::run(&CheckedProgram)`.

## Capability manifest and differential harness

One test pass (`scalar_capability_manifest_and_differential`) shares a single
production compile per fixture between manifest generation and both
differentials, because the stdlib-linked compile dominates the cost
(~25 eligible fixtures; the gate is minutes-long and lives only in the
dedicated LLVM lane):

- Eligibility is a text scan first (zero-arg value-returning `def compute()`
  entry in `assets/ok`, or `pliron_trap_*` main-pattern in
  `assets/runtime_error`); only eligible fixtures compile natively, keeping
  the pass to ~25 compiles instead of ~250.
- Value fixtures compare the VM's single printed line (parsed at the entry's
  return kind; floats compare by bits with NaN-class equality — the VM
  prints shortest-round-trip text) against `jit_value` at `O0` **and** `O1`.
- Trap fixtures run only as native executables in subprocesses (an
  in-process JIT trap would `exit` the test runner): exit status must be the
  category's `64 + code` at both levels with empty stdout, and the VM error
  must carry the same category's message.
- `conformance/pliron-scalar.tsv` is asserted byte-exact via
  `expect_file!` (`UPDATE_EXPECT=1` with `CARGO_WORKSPACE_DIR=$PWD`
  regenerates); the Stage 1 ">= 7 fixtures" shrink guard is superseded by
  the manifest floors (differential >= 20, trap-differential >= 4, no
  `pliron_*` fixture may be `excluded`).

## Findings

- `llvm.fptosi.sat.i64.f64`-style type-mangled intrinsic names pass through
  `pliron_llvm`'s `CallIntrinsicOp` emission unchanged
  (`llvm_lookup_intrinsic_id` resolves the base id; the declaration is added
  under the full mangled name), so saturating conversions need no custom
  scaffolding.
- Trap guards introduce real control flow into the formerly branch-free
  arithmetic expansions: the lowering splits the current block and appends
  continuation/trap blocks to the function region; `mem2reg` handles the new
  block structure unchanged.
- `expect_file!`-based manifests inherit the in-repo expect-test caveat:
  `UPDATE_EXPECT=1` needs `CARGO_WORKSPACE_DIR=$PWD`.
