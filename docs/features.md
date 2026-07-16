# Mojito Feature Matrix

This is the authoritative summary of language support. Update it in the same
change that alters a feature's status. [`grammar.md`](../grammar.md) remains authoritative for
surface syntax; the tests remain authoritative for detailed behavior.
The machine-checkable comparison with pinned Mojo—including explicit divergences
and exclusions—lives in [`conformance/parity.tsv`](../conformance/parity.tsv).

Status meanings:

- **Run** — accepted by the production compiler and executed by the register VM.
- **Check** — represented and checked, but has no independent runtime behavior.
- **Parse** — preserved in the AST but deliberately rejected before execution.
- **No** — not reliably accepted even as syntax.

| Area | Feature | Status | Boundary / notes |
|---|---|---:|---|
| Frontend | Indentation, continuations, comments, literals, Mojo escapes | Run | Diagnostic parsing can report multiple statement-level errors. |
| Frontend | T-strings | Parse | Interpolations are parsed; semantic lowering is unsupported. |
| Frontend | Walrus `:=` | Parse | Typed as its value, then rejected by MIR execution. |
| Modules | File-scope declarations and runtime entry | Run | Runtime statements at file scope are rejected; execution begins in zero-argument `main()`. |
| Bindings | Typed/inferred `var`, var-less introduction, assignment | Run | Same-scope redeclaration and type changes are rejected. |
| Bindings | Field/index places, augmented assignment, tuple unpacking | Run | Place expressions evaluate indexes once. |
| Bindings | Local `ref name = place` | Run | Aliases variable/field/index storage; indexed places bind once; persistent CFG loans enforce exclusivity through last use. |
| Calls | Defaults, keywords, `/`, `*`, homogeneous `*args` | Run | One structural contract serves free, method, static-method, and hand-written constructor calls in the checker and VM. |
| Calls | Homogeneous free-function `**kwargs` | Run | Materialized as self-hosted `HashDict[String, T]`. |
| Calls | Heterogeneous `*args: *ArgTypes` | Run | Per-element bound checking, type-erased runtime collection, and compile-time `args.__len__()` iteration/indexing run for literal and directly constructed arguments. Indexes expose the common pack bound, not a reflected per-index concrete type. |
| Calls | Generic callable values | Run | An expected monomorphic `def(...) -> ...` type contextually instantiates a non-overloaded generic function; Mojito models this as a runtime value, while current Mojo uses a compile-time specialized alias. |
| Calls | Method/generic argument-binding parity | Run | Includes ordinary-parameter write-back. |
| Calls | User-defined static methods | Run | Uses the same positional/default/keyword ABI without a receiver slot. |
| Calls | Callable expressions / first-class function values | Run | Non-capturing functions can be stored, passed, and indirectly invoked through checked function types; generic/overloaded values and captures remain incomplete. |
| Calls | Named `out` results | Run | A free function's single `out` parameter is a caller-transparent result slot and must be initialized before fallthrough or bare return. |
| Conventions | `read`, `var`, `mut`, `ref` parameters and `ref self` | Run | Removed `owned` syntax is rejected. Copyable reads overlapping a `mut` argument are materialized before exclusive access; non-Copyable aliases remain errors. Origin-checked reference paths use frame/slot handles; ordinary non-reference `mut` paths may use value write-back. |
| Conventions | `out self`, named `out` results, `deinit self` | Run | Lifecycle receivers and a single free-function named result are supported. |
| Lifecycle | `ImplicitlyDestructible`, unified copy/move initialization, and `@implicit` constructors | Run | The obsolete `ImplicitlyDeletable` spelling is rejected. A unique nonraising `@implicit __init__` may satisfy typed bindings, arguments, and returns and participates in overload ranking. |
| Lifecycle | `@explicit_destroy` | Run | Named `deinit self` methods discharge path-sensitive obligations. Abandonment, overwrite, partial/double/conditional destruction are rejected; raising destruction preserves the value for an `except` fallback and automatic `DropVar` destruction is suppressed. |
| Functions | Recursion and conservative return checking | Run | Sibling forward references and mutual recursion are rejected. |
| Functions | Non-escaping nested `def` captures | Run | Lifted downward funargs currently capture implicitly; current Mojo requires an explicit capture list, so capture-list checking remains open. |
| Functions | Escaping closures | Excluded | Current Mojo does not support closures that outlive their enclosing scope; this is not a parity target. |
| Types | Scalars, strings, lists, tuples, ranges, SIMD and slices | Run | SIMD is value-level, not machine-vector code generation. |
| Types | Origin-carrying parameter reference types | Check | Named/place origins, semantic-only `Origin` parameters, and fixed/symbolic mutability declarations are checked. |
| Types | Origin-carrying return reference types | Run | Parameter/receiver projections, unions, call substitution, invalid-local escape rejection, and returned alias execution are implemented. |
| Structs | Fields, fieldwise/manual construction, methods, copy/move/drop | Run | Structs have value semantics. |
| Generics | Bounded type parameters and `Int`/`Bool` value parameters | Run | Type parameters erase; value parameters participate in identity; origin parameters are compile-time semantic facts. |
| Traits | Requirements, nominal conformance, associated comptime facts | Run | Includes the protocols exercised by the self-hosted library. |
| Traits | Refinement and default method bodies | Run | Requirements/capabilities inherit; defaults are statically materialized, explicit methods override, and ambiguity is rejected. |
| Overloading | Functions, methods, constructors | Run | Ranking minimizes conversions, then prefers fixed arity, shorter signatures, and concrete ties; invalid conventions are filtered and ambiguity is retained. User-defined `@implicit` conversion remains incomplete. |
| Comptime | Constants, `comptime if`, `comptime for`, type facts | Run | Elaborated before checking. |
| Comptime | Pure top-level CTFE through MIR/VM | Run | Fuel bounded; generated declarations remain unsupported. |
| Control flow | `if`, `while`, `for`, `break`, `continue`, ternary | Run | User iterator protocol is supported. |
| Exceptions | `raise`, `try`/`except`/`else`/`finally` | Run | Direct, selected overload/method, and indirect callable effects are checked; typed/parametric errors, inferred `except` bindings, and `Never` run. |
| Contexts | `with` statements | Parse | Context-manager protocol is unsupported. |
| Modules | Source modules/packages, wildcard/selective, dotted/relative linking | Run | Lexical imports, dots-only siblings, `__init__.mojo` re-exports, and declaration provenance survive flattening. |
| Modules | Qualified `import module`, module/member aliases | Run | Aliased and full dotted calls, values, and types resolve without merging same-named declarations. |
| Ownership | `^` moves, partial moves, use-after-move analysis | Run | MIR dataflow owns these rules. |
| Borrowing | Local loans and origin-bearing cross-call references | Run | Frame/slot handles execute free and receiver-projected returns, unions, and captured indexes; unsafe/static origin forms remain deferred. |
| Destruction | ASAP `__del__`, edge/try cleanup, reverse field order | Run | Liveness rewrites MIR with explicit drops. |
| Standard library | Self-hosted collections, algorithms, math, hashing | Run | Proof subset under `stdlib/`, not Mojo's full standard library. |
| Backend | Register VM | Run | Sole backend and runtime; direct calls use an explicit continuation-driven frame stack with monotonic frame identities. |
| Tooling | Textual MIR/VM assembly, parser, verifier, disassembler | No | Planned as a versioned Mojito-owned serialization and debugging format. |
| Backend | Cranelift, then LLVM | No | Planned native backends after the textual MIR contract and VM semantics stabilize. |
| Stretch backend | eBPF and MLIR | No | Explicit stretch goals, not first-pass parity requirements. |
| Out of scope | GPU, concurrency/parallelism, distributed execution, Python interop | No | Intentionally excluded from the first Mojito parity target. |

For planned semantic work, see [`roadmap.md`](../roadmap.md). For exact VM
operations, see [`vm-instruction-set.md`](vm-instruction-set.md).
