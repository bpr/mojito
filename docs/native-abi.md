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

- `ABI_VERSION` (currently **6**) is a monotonic `u32` declared identically in
  `mojito_runtime::ABI_VERSION` and `native::rt_abi::MJRT_ABI_VERSION`. Bump
  it on any change to an exported symbol's signature or semantics, an
  exported `#[repr(C)]` type's layout, a trap category's meaning, or any rule
  in this document that generated code depends on. Version 2 introduced the
  headered allocator (zero-size requests allocate, `mjrt_dealloc` validates
  against the header), the size-less `mjrt_free`, and
  `mjrt_unhandled_error`. Version 3 made raising functions return tagged
  `{tag, ok, err}` outcomes through an outcome out-pointer (a generated-code
  rule — see [Errors and exceptional control
  flow](#errors-and-exceptional-control-flow)) and added the
  lifecycle-event reporter `mjrt_trace`. Version 4 added the `input()`
  line reader `mjrt_read_line` and its stdin-failure trap category (6).
  Version 5 made zero-size `mjrt_alloc` requests return the aligned dangling
  sentinel (no allocation) with sentinel-address frees as no-ops — the
  stdlib's `unsafe_alloc[T](0)` neutralization idiom abandons nothing.
  Version 6 added runtime layout/lifetime tracking alongside allocation
  headers and added `mjrt_pointer_status`,
  dynamic-message `mjrt_abort`, and trap categories 7–13 for abort, pointer
  lifetime, and `UnsafeMaybeUninit` failures.
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

Native development, testing, and release gates cover the supported Linux target
only. Adding another host or target is not a promotion requirement and requires
its own explicit support decision and testing infrastructure.

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
- `SIMD[dtype, width]` with `width > 1` is a contiguous scalar aggregate of
  `width` lanes, aligned like one lane. Width-one SIMD aliases retain their
  scalar ABI. This is the semantic fallback representation; mapping completed
  SIMD semantics to native vector types is a later backend optimization.
- A **retained callable** (`Ty::Func`) is the two-word value
  `{ invoke: ptr, env: ptr }` (16/8). `invoke` is a backend-interned thunk
  (`mjthunk_<n>`, outside the `mj_` mangle image); `env` points at the
  frame-local environment record of the creating `MakeClosure`, or is null
  for a bare function value or empty-capture closure. Generic callable
  values (`Ty::GenericFunc`) have no representation and reject.
- Types with no defined native representation (unspecialized packs and
  unmaterialized literal types) reject with a contextual diagnostic —
  backends never guess.

### Strings and the constant pool

- A **string literal** (`Ty::StringLiteral`) is the borrowed descriptor
  `MjStrDesc { data: *const u8, len: u64 }` (16/8): `len` bytes of UTF-8, not
  NUL-terminated, never owned by the consumer. Literal bytes live in private
  constant globals named `mjstr_<n>` (an underscore — pliron identifiers
  admit no `.`), numbered in deterministic first-use order and deduplicated
  by content within a module.
- The **nominal `String`** is its declared stdlib fields laid out by the
  ordinary aggregate rules — `{ data: *mut u8, size: i64, cap: i64 }` (24/8),
  the runtime's `MjString`. `data` is `size` initialized bytes of UTF-8 in a
  `cap`-byte allocation obtained from `mjrt_alloc`.
- Two stdlib String bodies are **bridged natively** rather than compiled
  (their byte loops need machinery beyond this stage), mirroring the VM's own
  literal bridge: the literal constructor fills `{data: mjrt_alloc(len, 1),
  size: len, cap: len}` from the constant pool, and the copy constructor
  allocates `cap` bytes and copies `size`, preserving `size`/`cap` exactly
  like the stdlib body. `String.__deinit__` compiles from its real MIR
  (`Pointer.unsafe_free()` lowers to `mjrt_free`).

### Errors and exceptional control flow

- The built-in error value is `MjError { message: MjString }` (24/8): the
  message is `size` initialized bytes of UTF-8 in a `cap`-byte `mjrt_alloc`
  allocation, owned by whoever holds the `MjError`.
- A `raises` function compiles as `void f(outcome*, params...)`: one
  prepended **outcome out-pointer** (caller-allocated, replacing the plain
  sret slot when the return type is an aggregate — a function never receives
  both). The outcome is laid out by the ordinary aggregate rules:
  `{ tag: u32, ok: T, err: MjError }` (`native::layout::outcome_layout`) with
  `tag` 0 (`MJ_TAG_OK`) or 1 (`MJ_TAG_ERR`); exactly one payload is
  initialized, selected by the tag. Success, error, return, and
  `try`/`finally` cleanup paths lower as **explicit CFG edges** — platform
  unwinding is not used and would require its own semantic/ABI/portability
  specification first.
- **MjError ownership per edge**: the raise site materializes an owned
  `MjError` (a literal message copies into a fresh allocation; an owned
  runtime string or `String` buffer is stolen; a re-raise moves). A handler
  that binds the error owns it through the bound variable, whose ordinary
  drop frees `message.data` invisibly — the built-in error has no user
  destructor. A handler without a binder frees the message after the
  cleanup-edge drops. Propagation byte-copies the `MjError` from the callee
  outcome into the caller's own outcome error slot (ownership transfers). An
  error reaching the executable wrapper passes its message to
  `mjrt_unhandled_error(data, len)` — `unhandled error: <message>` on
  stderr, trap category 5 (exit 69); the CLI re-renders that stderr text as
  its diagnostic for byte parity with the VM's `RuntimeError::Raised`
  display.
- **The propagation path abandons locals**: matching the VM (which truncates
  raising frames without running destructors — its arena reclaims the memory
  invisibly), no user destructor runs on the error-propagation path out of a
  function beyond the explicit `try` cleanup lists in the MIR. Generated code
  frees the *buffers* of still-initialized releasable locals (the String and
  built-in-error family) on that path without running user code; abandoned
  locals of other droppable types are a recorded leak residue.

### Retained callables

- The `invoke` thunk of a `{ invoke, env }` callable value has the contract's
  physical signature with the environment pointer prepended **after** the
  out-pointer: `[outcome*|sret*], env*, params...` — parameter kinds classify
  exactly as a compiled callee's (scalars by value, aggregates and `mut`/`ref`
  places by pointer, zero-sized parameters skipped). A raising contract's
  tagged outcome flows through the thunk untouched: the thunk forwards its
  out-pointer to the target and the **caller** branches on the tag.
- One thunk is interned per (compiled target, capture-mode vector). The thunk
  rebuilds the target's leading capture arguments from the environment
  record and calls the target directly; every capture parameter of a lifted
  body is a reference parameter, so a `Reference` slot passes its stored
  place address and an owned (`copy`/`move`) slot passes the slot's own
  address — the record is the stable storage whose in-place mutation across
  repeated invocations the VM achieves by re-referencing the closure value.
- The **environment record** is frame-local storage of the creating
  `MakeClosure` (one entry allocation per site, slots re-stored on each
  execution): `{ drop: ptr, slots... }` laid out by the ordinary aggregate
  rules. A `Reference` capture slot holds the captured place's address; an
  owned capture slot holds the value inline. `drop` is null unless some
  owned slot needs drop work, in which case it names a per-site teardown
  thunk that destroys owned slots in reverse order and then nulls the header
  (drops are idempotent per record — a two-word copy aliases the record, so
  the first teardown wins; the VM's deep-copying closure clones are a
  recorded divergence). No runtime symbol participates: the value, record,
  and thunks are entirely compiler-side.

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

Reserved namespaces, all outside the `mj_` image: the wrapper `main`, the
runtime family `mjrt_*` (the `mojito-runtime` exports plus backend-emitted
helpers like `mjrt_pow`), and the constant-pool family `mjstr_*` (private
linkage — never exported from produced objects).

## Output

`mjrt_write_stdout(data, len)` writes exactly the given bytes (interrupt
retries included) and traps on failure — byte-exact output parity with the VM
is the differential contract. The formatting family `mjrt_fmt_i64/u64/f64`
produces the same text as the VM's display (`f64` is Rust's `{:?}` shortest
round trip — `3.0`, `1e300`, `NaN`, `inf`), so print parity is structural
rather than re-implemented: `print` lowers to one write per piece — each
argument's display bytes, a single `" "` between arguments, and a trailing
`"\n"` — composing the VM's `format_value` join byte-for-byte.

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
| `mjrt_alloc(size, align) -> *mut u8` | Caller owns; never null. Nonzero allocations have a hidden layout header plus a runtime lifetime record. Zero size returns an aligned dangling sentinel. Traps (category 3) on exhaustion or invalid alignment. |
| `mjrt_free(ptr)` | Consumes any live nonzero `mjrt_alloc` allocation; null and zero-size sentinels are no-ops; a repeated free traps (category 10). |
| `mjrt_pointer_status(ptr) -> u32` | Borrows pointer identity; returns 0 for live/ordinary, 1 for a dangling sentinel, and 2 for an address within a freed allocation. Generated dereferences map those states to categories 8 and 9. |
| `mjrt_dealloc(ptr, size, align)` | Consumes a live nonzero `mjrt_alloc` allocation, validating `size`/`align` against its header (mismatch traps, category 3); null and zero-size sentinels are no-ops; a repeated deallocation traps (category 10). |
| `mjrt_write_stdout(data, len)` | Borrows; full write with interrupt retry; traps (category 4) on failure. |
| `mjrt_fmt_i64/u64(value, out) -> u64` | Borrows `out` (≥ 20 bytes); returns bytes written; no NUL. |
| `mjrt_fmt_f64(value, out) -> u64` | Borrows `out` (≥ 32 bytes); VM display text. |
| `mjrt_trap(category) -> !` | Reports on stderr, exits `64 + category` (clamped to 127); runs no destructors. |
| `mjrt_unhandled_error(data, len) -> !` | Borrows the raised UTF-8 message; reports `unhandled error: <message>` on stderr and exits `64 + 5`; runs no destructors. |
| `mjrt_abort(data, len) -> !` | Borrows the abort's UTF-8 message; reports `abort: <message>` on stderr and exits `64 + 7`; runs no destructors. |
| `mjrt_trace(kind, data, len)` | Borrows the UTF-8 payload; reports one ordered lifecycle event (`mjtrace <kind> <payload>`) on stderr. Emitted only by trace-instrumented builds, never by default emission; write errors are ignored so tracing cannot perturb behavior. Kinds: 1 drop, 2 consume, 3 cleanup, 4 raise, 5 catch. |
| `mjrt_read_line(out)` | Borrows `out` (≥ 24 bytes) and writes an `MjString` whose `data` is a fresh caller-owned `mjrt_alloc` allocation (`size == cap ==` line length, trailing `\n` then `\r` stripped). EOF yields size 0 with a valid header-only allocation (uniformly freeable, never blocks noninteractive runs); a read error traps (category 6). |

Trap categories (shared with the backend's `TrapCategory` codes and exit
codes `64 + category`): 1 div/mod by zero (exit 65), 2 `**` exponent range
(exit 66), 3 allocation failure (exit 67), 4 stdout failure (exit 68),
5 unhandled error (exit 69), 6 stdin failure (exit 70), 7 abort (exit 71),
8 dangling-pointer dereference (exit 72), 9 use after pointer deallocation
(exit 73), 10 double free (exit 74), 11 uninitialized storage read (exit 75),
12 uninitialized storage take (exit 76), and 13 uninitialized storage destroy
(exit 77). Categories 1–2 and 8–13 reuse the VM's runtime-error message text;
dynamic categories 5 and 7 preserve their executable stderr message.

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
