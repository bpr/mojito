# The Shared Native Target, Layout, and Runtime ABI

This is the normative contract every native backend (Pliron today, Cranelift
or any later backend) compiles against. It has one code owner, the
`crate::native` module — `target.rs` (checked build configuration),
`layout.rs` (the layout engine), `mangle.rs` (symbol mangling), and
`rt_abi.rs` (the runtime contract table) — and one runtime implementation,
the independently versioned `crates/mojito-runtime` C-ABI library. Backends
consume these; they never re-derive layout from Rust's unspecified
`repr(Rust)`, from LLVM defaults, or from the VM.

Three artifacts stay in lockstep: this document, the `rt_abi` contract table,
and the `mojito-runtime` implementation. The agreement is mechanical, not
aspirational — see [Mechanical checks](#mechanical-checks).

The VM's internal `Value` representation (`src/runtime.rs`) is explicitly not
part of any native ABI, and `mojito-runtime` must never depend on the
`mojito` crate.

## ABI versioning

- `ABI_VERSION` (currently **1**) is a monotonic `u32` declared identically in
  `mojito_runtime::ABI_VERSION` and `native::rt_abi::MJRT_ABI_VERSION`. Bump
  it on any change to an exported symbol's signature or semantics, an
  exported `#[repr(C)]` type's layout, a trap category's meaning, or any rule
  in this document that generated code depends on.
- Every linked runtime exports the inspectable `u32` data symbol
  `mjrt_abi_version` and the function `mjrt_version() -> u32`; the
  synthesized executable wrapper references `mjrt_version`, so every produced
  binary carries the version symbol (visible to `llvm-nm` and friends).
- Mismatch handling: the runtime is linked statically, so a version mismatch
  is structurally impossible in produced executables today. When Stage 3
  introduces runtime startup, the wrapper's version call becomes a startup
  check that traps with a diagnostic on disagreement; dynamic linking, if
  ever offered, requires that check first.

## Checked build configuration

All native build knobs are checked types in `native::target`; backends never
receive raw strings:

- **Target triple** — `Triple`, currently exactly
  `x86_64-unknown-linux-gnu`. Each triple carries its canonical name and its
  pinned LLVM data-layout string:

  ```text
  e-m:e-p270:32:32-p271:32:32-p272:64:64-i64:64-i128:128-f80:128-n8:16:32:64-S128
  ```

  Both are stamped as `target datalayout`/`target triple` on every emitted
  LLVM module, and every `clang` invocation passes `--target=<triple>`.
  The string is pinned against the installed LLVM 22 toolchain by test.
- **CPU features** — `CpuFeatures`; only the target baseline is accepted.
- **Optimization level** — `OptLevel`: `O0` (backend baseline cleanup only)
  or `O1` (LLVM `default<O1>` over the emitted bitcode).
- **Output kind** — `EmitKind`: `plir | ll | bc | obj | exe`; binary kinds
  require an output path (`BuildConfig::validate`).
- **CLI** — `--target TRIPLE` (default: the build host; error when the host
  is unsupported), `--native-opt 0|1`, `--emit KIND`, `-o PATH`.

JIT execution additionally requires `target == host`.

## Scalar semantics

| Checked type | Native type | Rules |
| --- | --- | --- |
| `Int` | `i64` (signless) | Signed operator selection (`sdiv`/`srem`/`ashr`, signed predicates). **Overflow is defined two's-complement wrapping** for `+ - *`, unary `-`, and `**`; shifts mask the amount `& 63`. `//`/`%` trap on a zero divisor (exit category 1) and define the single overflowing case `MIN // -1 == MIN`, `MIN % -1 == 0` (native lowering sanitizes the divisor because LLVM `sdiv`/`srem` are poison there). `**` traps unless the exponent is in `0 ..= u32::MAX` (category 2) and computes square-and-multiply over wrapping multiplication (the `mjrt_pow` helper). |
| `UInt` | `i64` (signless) | Unsigned operator selection (`udiv`/`urem`/`lshr`, unsigned predicates); **wrapping mod 2^64** for `+ - *` and `**`; same trap rules. |
| `Bool` | `i1` in SSA | At ABI boundaries (returns read through the JIT, future by-value fields) the storage unit is one byte and producers zero-extend; consumers may rely only on the low bit. Layout is size 1, align 1. |
| `Float64` | `double` | IEEE 754 binary64, no fast-math flags. `!=` is the **UNE** predicate and every other comparison is ordered — so `NaN != x` is True (`Bool(NaN)` is True via `fcmp une 0.0`) and other NaN comparisons are False. Signed zero follows IEEE (`0.0 == -0.0`; displays keep the sign). `//` and `%` are `floor`-based expansions (`fdiv` + `llvm.floor`, `x - y*floor(x/y)`) with **no zero trap** — infinities and NaN flow through. `**` lowers to `llvm.pow`; cross-libm bit-exactness is an explicit **non-claim** (VM and native may differ in the last ulp on some hosts; fixtures avoid such inputs). |

Conversions: `Int(f)`/`UInt(f)` are the saturating `llvm.fptosi.sat.i64.f64`/
`llvm.fptoui.sat.i64.f64` intrinsics (NaN → 0) matching the VM's `as`-cast
semantics; integer→float are `sitofp`/`uitofp`; `Bool(x)` is `x != 0` under
the type's own `!=`; `Int(b)`/`Float64(b)` zero-extend/convert the i1.

The VM implements the same wrapping definitions (`src/runtime.rs`
`int_op`/`uint_op`/`floor_div`/`floor_mod`), so integer overflow is no longer
a recorded VM/native divergence.

## Layout

Owned by `native::layout::LayoutCx`, keyed off checker `Ty` plus the
program's `MirDeclarations` struct index (MIR-level struct identity is the
specialized name). Rules:

- Fields lay out in **declaration order** with C-style padding: each field at
  the next offset aligned for it; aggregate alignment is the maximum field
  alignment (at least 1); total size pads to that alignment. No reordering.
- A zero-sized type has size 0, alignment 1, and forces no padding.
- Scalars: `Int`/`UInt`/`Float64` are 8/8, `Bool` is 1/1, `None` is a ZST.
- `Pointer`/`Ref` are one target pointer (8/8); origins and ownership facts
  erase after validation. A reference adds runtime metadata only when its
  checked type requires it — no checked type does today. Dangling ZST-style
  pointers are aligned sentinels, never null.
- `Tuple`/`RuntimePack`/nominal structs are ordinary aggregates of their
  element/field types.
- `Variant[T1..Tn]` is a `u32` tag at offset 0 (the alternative's index in
  the type — the ordering is part of the type) with every payload overlaid at
  the first offset aligned to the widest alternative; size pads to
  `max(4, payload align)`.
- Types with no defined native representation (SIMD until its lowering stage,
  packs, callables, unmaterialized literal types) reject with a contextual
  diagnostic — backends never guess.

### Strings and the constant pool

- A **string literal** (`Ty::StringLiteral`) is the borrowed descriptor
  `MjStrDesc { data: *const u8, len: u64 }` (16/8): `len` bytes of UTF-8, not
  NUL-terminated, never owned by the consumer. Literal bytes live in private
  unnamed constant globals named `mjstr.<n>`, numbered in deterministic
  lowering order and deduplicated by content within a module.
- The **nominal `String`** is its declared stdlib fields laid out by the
  ordinary aggregate rules — `{ data: *mut u8, size: i64, cap: i64 }` (24/8),
  the runtime's `MjString`. `data` is `size` initialized bytes of UTF-8 in a
  `cap`-byte allocation obtained from `mjrt_alloc`.

### Errors and exceptional control flow

- The built-in error value is `MjError { message: MjString }` (24/8).
- A raising call produces a **tagged outcome** laid out by the ordinary
  aggregate rules: `{ tag: u32, ok: T, err: MjError }` with `tag` 0 (`MJ_TAG_OK`)
  or 1 (`MJ_TAG_ERR`); exactly one payload is initialized, selected by the
  tag. Success, error, return, and `try`/`finally` cleanup paths lower as
  **explicit CFG edges** — platform unwinding is not used and would require
  its own semantic/ABI/portability specification first.

## Calling convention

- The C calling convention (`ccc`) everywhere; specialized MIR functions map
  1:1 to deterministically mangled native symbols.
- Scalars pass and return by value (`Bool` as i1 in SSA, byte-sized at ABI
  boundaries as above).
- Aggregates pass **by reference** and return through an sret-style out
  pointer — deterministic and target-independent by construction; the ABI
  deliberately does not adopt SysV register classification for aggregates.
  (The runtime C ABI itself passes only primitives and pointers, so this rule
  binds generated-code-to-generated-code calls from Stage 3 on.)
- The synthesized executable wrapper is the plain C `main() -> i32`: it calls
  `mjrt_version`, then the mangled `__toplevel__` (when present), then the
  mangled `main`, then returns 0.

## Symbols and mangling

Owned by `native::mangle`. MIR names are already globally unique and
deterministic (`src/symbol.rs`); mangling is a purely mechanical injective
escape: prefix `mj_`, then per byte — `[A-Za-z0-9]` passes through, `_`
becomes `_u`, any other byte becomes `_hh` (two lowercase hex digits).

Reserved namespaces, all outside the `mj_` image: the wrapper `main`, the C
`exit`, the runtime family `mjrt_*` (the `mojito-runtime` exports plus
backend-emitted helpers like `mjrt_pow`), and the constant-pool family
`mjstr.*`.

## Output

`mjrt_write_stdout(data, len)` writes exactly the given bytes (interrupt
retries included) and traps on failure — byte-exact output parity with the VM
is the differential contract. The formatting family `mjrt_fmt_i64/u64/f64`
produces the same text as the VM's display (`f64` is Rust's `{:?}` shortest
round trip — `3.0`, `1e300`, `NaN`, `inf`), so Stage 3 print parity is
structural rather than re-implemented.

## The runtime library

`crates/mojito-runtime` — a small, independently versioned Rust library,
`crate-type = ["rlib", "staticlib"]`, zero dependencies, linked into every
produced executable (discovery: `MOJITO_RUNTIME_LIB`, then the compiler
executable's directory and ancestors). Its complete export surface is the
`rt_abi` contract table; per symbol the table records the C signature,
ownership and nullability of every pointer, allocation responsibility, and
failure behavior. Summary (authoritative rows in `native::rt_abi`):

| Symbol | Contract |
| --- | --- |
| `mjrt_abi_version` (`u32` data), `mjrt_version() -> u32` | The inspectable ABI version. |
| `mjrt_alloc(size, align) -> *mut u8` | Caller owns; never null; zero size returns an aligned dangling pointer; traps (category 3) on exhaustion or invalid align. |
| `mjrt_dealloc(ptr, size, align)` | Consumes an `mjrt_alloc` allocation; null/zero-size no-op; size/align must match the allocation. |
| `mjrt_write_stdout(data, len)` | Borrows; full write with interrupt retry; traps (category 4) on failure. |
| `mjrt_fmt_i64/u64(value, out) -> u64` | Borrows `out` (≥ 20 bytes); returns bytes written; no NUL. |
| `mjrt_fmt_f64(value, out) -> u64` | Borrows `out` (≥ 32 bytes); VM display text. |
| `mjrt_trap(category) -> !` | Reports on stderr, exits `64 + category` (clamped to 127); runs no destructors. |

Trap categories (shared with the backend's `TrapCategory` codes and exit
codes `64 + category`): 1 div/mod by zero (exit 65), 2 `**` exponent range
(exit 66), 3 allocation failure (exit 67), 4 stdout failure (exit 68).
Categories 1–2 reuse the VM's runtime-error message text so both backends
diagnose identically; `run --backend pliron` maps trap exit codes back to the
VM diagnostic.

## Mechanical checks

- **Default lane** (`scripts/check`, no LLVM): `tests/native_abi_test.rs`
  coerces every `mojito-runtime` export to the exact `extern "C"` fn-pointer
  type its table row implies, checks every `#[repr(C)]` type's
  `size_of`/`align_of`/`offset_of!` against the table layout, and pins the
  version and trap/tag constants across the two crates; `native::layout` and
  `mojito-runtime` unit tests pin the layout arithmetic and runtime behavior.
- **Pliron lane** (`scripts/check-pliron`): `tests/pliron_backend_test.rs`
  `native_abi_cross_checks` builds LLVM target data from the pinned
  data-layout string alone and asserts `LLVMABISizeOfType`/
  `LLVMABIAlignmentOfType`/`LLVMOffsetOfElement` agreement with the layout
  engine for every exported runtime type and a representative checked-type
  matrix; snapshot-pins the table's LLVM declaration rendering
  (`pliron::runtime_declarations`); pins the data-layout string against the
  installed clang; and inspects produced executables with `llvm-nm` for the
  version symbol and the absence of unspecified `mjrt_*` exports. All checks
  are target-only — no generated foreign code executes.
