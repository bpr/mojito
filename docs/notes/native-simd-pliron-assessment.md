# Native SIMD Through Pliron: Capability Assessment

Date: 2026-09-03 (assessment); 2026-09-08 (as built, below)\
Mojito target: current `master` after the crate split  
Examined dependency: `pliron = 0.17.0`, `pliron-llvm = 0.17.0`,
`llvm-sys = 221.0.1` / LLVM 22 — re-verified equivalent at the promoted pin
(Pliron revision `477e6b0`, `llvm-sys 231` / LLVM 23.1): `VectorType`,
`InsertElementOp`/`ExtractElementOp`/`ShuffleVectorOp`, `SelectOp` over
`<N x i1>`, vector-shaped comparisons and casts, and `CallIntrinsicOp` are
all present and round-trip through the canonical text.

## As Built (2026-09-08)

The dual representation below is implemented in
`crates/mojito-pliron/src/lower/simd.rs`. Storage and the call ABI are the
unchanged lane-aligned `LayoutCx` aggregate (`docs/native-abi.md`); every
multi-lane operation loads its operands as `<N x lane>` (lane alignment
declared; Bool lanes load as bytes and compare non-zero into `<N x i1>`),
computes in vector SSA, and stores the result into a fresh typed vector
alloca at lane alignment (Bool lanes zero-extend to bytes). No SSA cache
exists: pliron's mem2reg forwards the whole-vector store to the loads at
`O0`, and SROA does the rest at release. The invariant that makes mem2reg
safe: a value's storage is touched at its base only by whole-vector typed
loads and stores (mem2reg does not compare access types), so lane reads —
dynamic subscripts, formatting, the hashing fold — extract from the loaded
vector, and a lane *write* is that read's mirror: load the whole vector
from the place's base address, `insertelement`, store it back. A
whole-value read or write is the same shape — `copy_value` in
`lower/abi.rs` moves a multi-lane value as one typed vector load and store
rather than the aggregate `llvm.memcpy`. No lane is addressed by a byte
offset and no `memcpy` touches a SIMD slot, so the slot stays promotable:
pliron's `prune_candidates` drops any allocation whose pointer has a use
that is not a plain load or store, and both a `getelementptr` and a
`memcpy` argument are exactly such a use.

This is why a multi-lane slot is allocated at its storage vector type
(`value_storage` → `simd_storage_slot`) and never as bytes: mem2reg types
an inserted block argument at the *allocation's* pointee type, so a byte
slot carrying vector loads and stores would grow a byte-typed phi fed by
vector values and fail verification. The two halves must move together;
`simd_lowering_emits_vector_code` pins both (a `phi <8 x i32>` for the
lane-written `prefix`, and no `alloca` or `memcpy` at all in the
whole-vector-moving `carry`). What remains in memory is the call boundary:
SIMD still passes by pointer and returns through sret (roadmap §1).

Per operation: construction is `poison` plus `insertelement` per lane (one
element splats via `shufflevector`); a lane write is a bounds-guarded
`insertelement` into the loaded vector, stored back at the base; `+ - *` carry no `nsw`/`nuw`; shift
counts are masked lane-wise (`and` with `bits - 1`) before `shl`/`ashr`/
`lshr`; `//` and `%` reuse the scalar select chain with splat constants (a
zero divisor selects a zero lane, `MIN // -1` wraps); float `+ - * /` run at
the lane width with no fast-math (a `Float32` result computed at f32 equals
the VM's f64 computation rounded once); comparisons produce `<N x i1>`
directly; `select` blends over the mask; `shuffle` is one `shufflevector`
(a one-lane mask extracts the scalar); casts are vector `trunc`/`sext`/
`zext`/`sitofp`/`uitofp`/`fpext`/`fptrunc` (a `Float32` target rounds
through f64, as the VM does) except float→int, which extracts each lane
through `llvm.fptosi.sat.i128.f64` and re-inserts (the `v{N}i128` vector
form's legalization is unverified); `to_bits` is a vector `bitcast` plus
`zext`; reductions call `llvm.vector.reduce.{add,mul,smin,smax,umin,umax}`,
`llvm.vector.reduce.{and,or}.v{N}i1`, and the *ordered*
`llvm.vector.reduce.{fadd,fmul}` from the exact identity (`-0.0`, `1.0`)
with no reassociation, while float min/max fold lane by lane through
`llvm.minnum`/`llvm.maxnum` (Rust's `f64::min`/`max`, the VM's step).

Testing rule: vectorization evidence is object code, not IR. Vectors wider
than a register legalize by splitting, so legal vector IR proves nothing
about the instructions actually selected —
`simd_lowering_emits_vector_code` inspects the `llvm-objdump` listing, and
that is the assertion to keep green. The same test pins what the backend
*emits* (a lane write's `insertelement`) at `O0`, where the optimizer has
not yet rewritten it.

Evidence: 20 `assets/ok/simd_*` fixtures agree VM/`O0`/release
(`conformance/pliron-parity.tsv`), the release IR of
`tests/pliron_backend_test.rs::simd_lowering_emits_vector_code` keeps
`mul <8 x i32>`, `shl`, `fcmp ogt <8 x float>`, `select <8 x i1>`, and the
`llvm.vector.reduce.add.v8i32`/`fadd.v8f32` calls, and its object code
selects `paddd`, `pmuludq`, `pslld`, `cmpltps`, `pshufd`, and `movups`
(baseline x86-64: no `pmulld`). `LayoutCx` is unchanged and now pinned for
SIMD against LLVM target data (multi-lane storage realized as `[N x lane]`
arrays, never vector types).

## Conclusion

Pliron's LLVM dialect exposes the essential LLVM vocabulary Mojito needs for
native SIMD lowering. There is no identified dialect-level blocker and no
present reason to introduce a Mojito SIMD dialect, fork Pliron, or bypass
Pliron with direct LLVM construction.

The difficult part is not expressing vector operations. It is preserving
Mojito's exact VM semantics and normative storage/runtime ABI while introducing
an LLVM vector computation representation. Native-vector lowering should
therefore distinguish storage representation from computation representation:

```text
verified MIR SIMD value
        |
        +-- storage/call ABI: existing layout-compatible scalar-lane storage
        |
        `-- computation: LLVM fixed vector `<N x lane-type>` in SSA
```

Width-one SIMD scalar aliases should remain scalars. Multi-lane values should
enter vector SSA for computation and return to the established storage form
only at memory and ABI boundaries. LLVM may split or legalize vectors wider
than a physical target register; source-level SIMD width must not be confused
with the number of registers selected by a target.

## Current Mojito State

The Pliron backend already implements Mojito's supported multi-lane SIMD
semantics in `crates/mojito-pliron/src/lower/simd.rs`, but wider values are
memory-resident scalar aggregates. `lower_ty` in
`crates/mojito-pliron/src/lower/types.rs` deliberately classifies width-one
aliases as SSA scalars and wider SIMD values as aggregates. Constructors,
casts, shuffles, elementwise operations, selection, indexing, reductions, and
formatting operate lane by lane through loads and stores.

This is a valuable executable fallback and VM-parity baseline. Native SIMD
should optimize the representation below the verified-MIR waist rather than
change MIR or remove the scalar implementation prematurely.

The supported source semantics include:

- width-one scalar aliases and multi-lane `SIMD[dtype, width]` values;
- integer and floating construction, including scalar splats;
- bit-accurate wrapping integer arithmetic;
- unary and elementwise arithmetic and bitwise operations;
- integer and floating comparisons;
- Boolean mask selection;
- lane access;
- compile-time-mask shuffle;
- casts between supported lane types;
- `reduce_add`, `reduce_mul`, `reduce_min`, `reduce_max`, `reduce_and`, and
  `reduce_or`; and
- formatting and conversion back to canonical scalar results.

## Pliron LLVM-Dialect Coverage

The pinned `pliron-llvm 0.17.0` source provides:

| Requirement | Pliron/LLVM facility |
|---|---|
| Fixed or scalable vector types | `pliron_llvm::types::VectorType` and `VectorTypeKind` |
| Integer vector arithmetic/bitwise operations | Existing LLVM integer operations, whose type interfaces admit vectors |
| Floating vector arithmetic | Existing LLVM floating operations over vector types |
| Integer comparisons | `ICmpOp`, producing an equal-width Boolean vector |
| Floating comparisons | `FCmpOp`, producing an equal-width Boolean vector |
| Per-lane selection | `SelectOp` with `<N x i1>` condition and equal-width value vectors |
| Dynamic lane insertion | `InsertElementOp` |
| Dynamic lane extraction | `ExtractElementOp` |
| Compile-time lane permutation | `ShuffleVectorOp` and `ShuffleVectorMaskAttr` |
| Vector casts | Integer and floating cast operations with vector-shape checks |
| Vector memory traffic | `LoadOp` and `StoreOp` over vector types |
| Vector constants/building | LLVM vector constants plus `UndefOp`/`PoisonOp` and insertion |
| Reductions and overloaded intrinsics | `CallIntrinsicOp` |

`VectorType` supports both fixed and scalable forms and converts to LLVM's
corresponding native vector types. Mojito should initially use only fixed
vectors. Its lane width is a compile-time, source-observable fact used by
indexing, shuffling, type identity, and layout; scalable vectors represent a
different model in which the runtime lane count depends on `vscale`.

`CallIntrinsicOp` validates an intrinsic name and function signature against
LLVM and is sufficient for the `llvm.vector.reduce.*` families and specialized
conversion intrinsics. Pliron need not have a distinct Rust operation type for
every reduction.

## Recommended Representation Boundary

Changing every multi-lane `Ty::Simd` directly from the current aggregate to an
LLVM vector would risk silently changing:

- struct field size, alignment, and padding;
- function parameter and result ABI;
- the `LayoutCx` contract;
- runtime-library interoperability;
- Boolean-lane storage;
- object compatibility across compiler versions; and
- behavior for vectors larger than a target's physical vector registers.

The safer first implementation is a dual representation:

1. Keep the existing `LayoutCx` result and aggregate representation at stored
   fields, addressable variables, runtime calls, and externally visible call
   boundaries.
2. Materialize a fixed LLVM vector when a multi-lane value enters an operation
   chain.
3. Keep successive SIMD operations in vector SSA form.
4. Spill or unpack only where an address, established aggregate layout, scalar
   formatter, or ABI crossing requires it.
5. Retain the lane-by-lane lowering as a correctness fallback when the selected
   target or operation cannot use vector lowering.

This design lets LLVM's legalization and instruction selection decide whether
`<16 x float>` becomes one instruction, several physical vectors, or scalar
code. "Native SIMD" means presenting legal vector computation to LLVM, not
promising one physical instruction for every Mojito value.

If later measurements justify exposing LLVM vector layout directly in the
native ABI, that must be an explicit `docs/native-abi.md` decision with an ABI
version change, not an incidental consequence of changing `lower_ty`.

## Semantic Requirements and Traps

### Width-One Aliases

Keep `SIMD[dtype, 1]` aliases in their current scalar representation. Mapping
them to `<1 x T>` adds ABI conversions and can inhibit ordinary scalar
optimization without providing parallel execution.

### Construction and Splat

An element-list constructor can start from `poison`/`undef` and use
`InsertElementOp` per lane. A splat can insert one converted lane and use
`ShuffleVectorOp` to broadcast it. Every source conversion must retain the
current exact-literal and lane-wrapping behavior.

### Integer Arithmetic

LLVM vector integer `add`, `sub`, and `mul` without `nsw` or `nuw` flags match
Mojito's modular lane arithmetic. Do not attach overflow flags unless the
source contract is changed. Signedness remains an operation-selection fact;
LLVM integer vector storage is signless.

### Shifts

Mojito masks shift counts to match the VM's wrapping shifts. Raw LLVM vector
shifts do not supply that policy. Mask the right-hand vector lane-wise before
emitting `shl`, `ashr`, or `lshr`, preserving signedness selection.

### Comparisons and Masks

LLVM comparison results are `<N x i1>`. Mojito's stored Boolean SIMD lanes have
a byte-oriented layout. Keep `<N x i1>` inside computation chains and convert
to or from the byte-lane storage form only at boundaries. `SelectOp` directly
accepts a vector-of-`i1` condition when its width matches the value vectors.

### Lane Access

`ExtractElementOp` and `InsertElementOp` accept dynamic integer indexes, but an
out-of-range LLVM index can yield poison. Emit Mojito's checked bounds/trap
behavior before the LLVM element operation. Do not rely on later LLVM passes to
preserve a source-level bounds failure.

### Shuffle

Mojito shuffle masks are compile-time values already verified by MIR. They map
directly to `ShuffleVectorOp`. Preserve the current rule that result width is
the mask length and every selected source lane is in range. Do not introduce
LLVM undef-mask lanes unless Mojito gains corresponding semantics.

### Casts

Straight integer widening/truncation and floating widening/rounding can use the
vector forms of Pliron's cast operations. The current VM contract has two
nontrivial requirements:

- `Float32` rounds at the same boundary as the VM; and
- float-to-integer conversion truncates toward zero, saturates at the i128
  intermediate, and then wraps to the requested lane width.

Plain LLVM `fptosi`/`fptoui` is not a substitute for the latter because
out-of-range conversion may produce poison. Use the appropriate saturating
LLVM intrinsic through `CallIntrinsicOp`, vectorized at the correct
intermediate width, then perform the existing rewrap. Keep NaN-to-zero and
signed/unsigned behavior aligned with the VM tests.

### Reductions

Integer add/multiply/and/or and signed/unsigned min/max can use the matching
`llvm.vector.reduce.*` intrinsic when its exact semantics agree. Floating
reductions require special care:

- reassociation changes rounding;
- min/max intrinsic families differ in NaN handling; and
- reduction order may be observable.

Do not set reassociation or broad fast-math flags by default. Select an LLVM
intrinsic only after comparing its ordering and NaN contract with the VM.
Where LLVM's intrinsic contract differs, retain or generate the deterministic
scalar reduction tree required by Mojito.

### Memory and Alignment

Vector loads and stores must use alignment no stronger than the address is
proven to have. The established aggregate ABI may provide only lane alignment,
not the preferred alignment of an LLVM vector type. An unaligned vector load is
still useful if expressed honestly; falsely claiming stronger alignment is
undefined behavior.

## Suggested Implementation Slices

### 1. Boundary Conversion and Construction

- Introduce an internal compute-value distinction in Pliron lowering.
- Keep width-one aliases scalar and stored multi-lane values ABI-compatible.
- Add aggregate-to-vector load/assembly and vector-to-aggregate spill/unpack.
- Lower explicit constructors and splats to vector SSA.
- Pin Pliron verification and LLVM IR snapshots.

### 2. Elementwise Operations

- Integer arithmetic, bitwise operations, and masked shifts.
- Floating arithmetic with existing fast-math policy (normally no flags).
- Integer/floating comparisons and vector mask selection.
- Keep chains in vector SSA so repeated operations do not spill each result.

### 3. Indexing and Shuffle

- Emit explicit dynamic-index bounds checks.
- Use extract/insert for lane access and mutation.
- Use `ShuffleVectorOp` for compile-time masks and splats.
- Cover result widths differing from source widths where MIR permits them.

### 4. Casts

- Integer width/sign conversions.
- Integer/float and float/float conversions.
- VM-exact `Float32` rounding.
- Saturating float-to-i128 followed by destination-width wrapping.
- Negative tests for every poison-sensitive edge.

### 5. Reductions

- Integer and mask reductions first.
- Signed and unsigned min/max with explicit operation selection.
- Floating reductions only after order and NaN parity are pinned.
- Preserve scalar lowering for any reduction without an exactly matching
  intrinsic contract.

### 6. ABI and Optimization Validation

- Confirm unchanged `LayoutCx` results and native ABI snapshots.
- Exercise stored vectors in fields, variables, arguments, results, and nested
  aggregates.
- Inspect emitted object code on every supported target to prove representative
  cases select vector instructions.
- Confirm oversized fixed vectors legalize correctly rather than being rejected
  or miscompiled.

## Acceptance Criteria

Completion should require all of the following:

- VM/native differential coverage at `O0` and release for every supported
  dtype, width, elementwise operation, conversion edge, shuffle, and reduction;
- exact output, trap category, and edge-value parity;
- sanitizer-clean storage and ABI crossings;
- unchanged `LayoutCx` and `docs/native-abi.md` contracts, or an explicit,
  versioned ABI revision with migration evidence;
- no fast-math or overflow flags that strengthen the source contract;
- bounds checks before poison-producing dynamic lane operations;
- a documented scalar fallback for unsupported target/operation combinations;
- capability-manifest updates that distinguish legal scalar fallback from
  actual vector lowering; and
- target-code inspection proving representative cases use vector instructions,
  rather than merely producing legal LLVM vector IR.

## Risks

1. **ABI drift disguised as optimization.** Changing `lower_ty` globally could
   alter public layout even when source semantics appear unchanged.
2. **Poison introduction.** Out-of-range extraction, shifts, and unchecked
   floating conversion can turn defined VM behavior into LLVM poison.
3. **Floating semantic drift.** Reassociation, fast-math, reduction order, and
   NaN differences can pass ordinary examples while violating exact parity.
4. **Mask representation mismatch.** `<N x i1>` is ideal for computation but is
   not automatically the established stored `SIMD[DType.bool, N]` layout.
5. **Excessive boundary traffic.** Correct vector operations can still produce
   poor code if every MIR instruction spills to aggregate storage. The lowering
   needs a local representation cache or equivalent SSA discipline.
6. **Mistaking IR for machine code.** LLVM may scalarize a legal vector. Codegen
   inspection and benchmarks are required in addition to IR verification.
7. **Target over-specialization.** The first implementation should use portable
   LLVM vector operations, not architecture intrinsics, unless a measured case
   cannot be expressed portably.

## Source References

- Mojito current SIMD lowering:
  `crates/mojito-pliron/src/lower/simd.rs`
- Mojito Pliron type classification:
  `crates/mojito-pliron/src/lower/types.rs`
- Mojito native ABI contract: `docs/native-abi.md`
- Mojito supported surface: `docs/features.md`
- Mojito roadmap item: `docs/roadmap.md`
- Pinned Pliron LLVM vector types:
  `pliron-llvm-0.17.0/src/types.rs` (`VectorType`, `VectorTypeKind`)
- Pinned Pliron LLVM vector operations:
  `pliron-llvm-0.17.0/src/ops.rs` (`InsertElementOp`, `ExtractElementOp`,
  `ShuffleVectorOp`, `SelectOp`, comparisons, casts, and `CallIntrinsicOp`)
- LLVM vector and intrinsic semantics:
  <https://llvm.org/docs/LangRef.html>

## Decision

Proceed with Pliron for native SIMD if it becomes the preferred backend. Treat
the task as a backend-private fixed-vector compute optimization over the stable
MIR and native ABI, with scalar aggregate storage and lowering retained until
the differential and target-code evidence justifies replacing any boundary.
