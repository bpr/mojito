# Pliron Stage 3: Runtime, Strings, Aggregates, Allocation, and Errors

Stage 3 grew the feature-gated Pliron backend from the scalar subset to the
runtime-calling subset: string-literal constant pools, `print` through the
shared ABI, target-layout aggregates (structs, internal tuple storage,
methods, constructors, drops), heap allocation, the nominal `String`, and
explicit unhandled-error reporting. The normative contract is
[`docs/native-abi.md`](../native-abi.md) (ABI version 2); this note records
the design decisions, bridges, and divergences.

## The aggregate memory model

- An aggregate-typed register holds a **pointer to function-local storage**
  (an entry-block `i8` alloca of the type's `LayoutCx` size/alignment).
  Storage allocas hoist to the entry block so registers redefined inside
  loops reuse one slot.
- Every offset comes from the shared layout engine (`native::layout`) —
  field and tuple-element addresses realize as constant `i8` GEPs; LLVM
  struct types are never consulted for layout.
- Aggregates pass **by pointer** and return through a **prepended sret
  out-pointer** (void LLVM return). In a callee, an aggregate parameter's
  variable slot *aliases the incoming pointer* (write-through), which is what
  makes `out`/`mut` receiver conventions work: `__init__` writes through the
  caller's destination storage, and a `mut self` method mutates the caller's
  receiver copy directly.
- Method receivers are **copy-in/copy-out**: the receiver register holds a
  place-loaded copy (`LoadPlace` — the VM's plain clone-on-read, a byte
  copy); after a `mut self` (per the struct's `mut_self_methods` set, keyed
  by resolved or source name) or `deinit self` callee returns, the receiver
  storage copies back to `recv_place` — the VM's `store_at_call_place`
  write-back.
- Constructor calls (`Call` to a struct name — MIR has no MakeStruct opcode)
  dispatch like the VM: the `Type(copy=value)` fieldwise copy form, the
  compiled `__init__` (exact name, else the unique arity overload — the VM's
  `overload_name` policy), else fieldwise per-field stores for
  `@fieldwise_init` structs (positional arguments only).
- `DropVar` mirrors the VM's `drop_value`: run the compiled `__deinit__` when
  the struct defines one, else destroy fields in reverse declaration order.
  Combinations whose residual state is only dynamically knowable reject
  contextually: a destructor on a struct with independently droppable
  fields, a partially-moved variable (tracked from `MovePlace` projections),
  and `ConsumeVar`/`ConsumePlace` over droppable fields.
- `UseVar` copies run the VM's `clone_value`: the compiled `__copyinit__`
  when defined (nested-only `__copyinit__` rejects), else a byte copy — which
  intentionally *shares* heap like the VM's plain clone, so byte-copy
  temporaries are never released. Moves are byte copies (ownership analysis
  guarantees no use after move); types with a user `__moveinit__` reject,
  except the nominal String whose move-init is an identity field move.

## Reachability

`reachable_set` grew beyond direct `Call` edges: checker-resolved
`MethodCall` targets, constructor `__init__` overloads, and lifecycle edges —
every struct type a reachable function mentions (transitively through
declared field types) pulls its compiled `__deinit__`/`__copyinit__`,
because drops and copies execute those bodies without any call instruction
naming them. Intercepted names are *not* edges: the `std.memory`
`unsafe_alloc` entry points (their inner bodies allocate through
element-erased builtins), pointer-receiver methods, the nominal String
constructor, and String's `__copyinit__`.

## Strings

- Compile-time literals stay register-bound bytes (`str_consts`); the pool
  interns `mjstr_<n>` private globals on actual use only, so fold-only
  literals (literal `+`/`==`/`!=` fold at compile time) never emit a global.
- Runtime strings are `(data, len)` SSA pairs (`str_runtime`), the in-flight
  `MjStrDesc`. `String(scalar)` formats through `mjrt_fmt_*` into a dedicated
  allocation (an *owned* pair); the consuming nominal-String constructor
  **steals** an owned pair's allocation instead of copying.
- The nominal String's literal constructor and `__copyinit__` are native
  bridges (see `docs/native-abi.md`); `__deinit__` compiles from real MIR
  because `Pointer.unsafe_free()` lowers to `mjrt_free`.

## The owned-temporary release rule

The VM never destroys register temporaries (its arena makes that free), so a
native temporary that owns heap (a `clone_value` String copy consumed by
`print`, a constructed String the ASAP drop pass orphans) must be released
invisibly — no user destructor runs, exactly matching the VM's observable
behavior — by freeing its buffers directly after the temporary's **final
operand appearance** (a precomputed per-function map). Transfers unmark:
`DefVar`, `Store`, `Return`, consuming (`owned`/`deinit`) call arguments, and
the constructor steal. Temporaries used across blocks reject (no liveness
analysis); releasable types are the String and byte-copied aggregates over
String fields — a user `__copyinit__` type other than String may own
arbitrary resources and simply is not marked (a recorded leak residue;
no current fixture produces one).

## Allocation

`unsafe_alloc[T](count, *, alignment = 0)` intercepts at its (concrete)
call sites: `mjrt_alloc(count * sizeof(T), align)` with a pre-multiplication
guard that traps (category 3) on excessive counts — a recorded divergence:
the VM raises a `TypeError` for a negative count. Pointer subscripts
(`Index` with the pointer intrinsic, and `Store` through `Proj::Index` on a
pointer root) address `p + i * sizeof(element)` unchecked, like the VM's
arena reads; `free`/`unsafe_free` lower to the size-less `mjrt_free`.
`Ty::Pointer` itself lowers as an opaque-pointer scalar (`ScalarTy::Ptr`)
with `==`/`!=` address identity and everything else rejected.

## Errors (the pre-Stage-4 contract)

No `try` lowering exists, so every runtime raise is dynamically unhandled:
raising functions compile with unchanged signatures, `Error(x)` lowers as its
message string pair, and `Raise` calls `mjrt_unhandled_error` (stderr
`unhandled error: <message>`, exit 69) with lowering continuing into a dead
block. `run --backend pliron` re-renders the executable's stderr as the CLI
diagnostic for byte parity with the VM's `RuntimeError::Raised` display.
Stage 4 replaces this with tagged outcomes and explicit cleanup edges.

## Recorded divergences

- Negative/excessive allocation counts: native traps category 3; the VM
  raises a `TypeError` (`value_as_index`).
- On an unhandled raise, the VM's `run` discards buffered partial stdout
  while native executables stream it — the raise differential compares exit
  category and stderr, not stdout.
- Owned temporaries of user-`__copyinit__` types other than String are not
  released (leak residue until Stage 4's real destruction machinery).
- Alloc-failure and stdout-failure traps (categories 3/4) have no VM analog.

## The acceptance harness

`tests/pliron_backend_test.rs::stage3_exe_manifest_and_differential`
generates `conformance/pliron-stage3.tsv`: every `assets/ok` fixture with a
`main` either compiles natively — exe stdout at `O0`/`O1` must equal the
VM's execution output byte-for-byte, and the `O0` AddressSanitizer/
LeakSanitizer build must run clean — or records `excluded` with its first
diagnostic; `assets/runtime_error/pliron_raise_*` fixtures compare the
unhandled-raise exit category and stderr. Shrink guards pin the eligible
counts, and `pliron_*` fixtures may never regress to `excluded`. The
Stage 2 scalar manifest (`conformance/pliron-scalar.tsv`) and its JIT value
lane stay unchanged as the cheap in-process scalar oracle. The symbol
surface check inspects both a scalar and a Stage 3 executable with `llvm-nm`
for the contract-table allowlist and asserts the `mjstr_*` pool never
exports.
