# Changelog

All notable changes to Mojito will be documented in this file. The project uses
Semantic Versioning while its public Rust API and supported Mojo subset continue
to evolve under the `0.x` compatibility rules.

## [Unreleased]

### Added

- Current Mojo consuming parameters use `var`; the removed `owned` spelling is
  rejected, and the convention is represented as `Var` throughout the compiler.
- Unified `__init__(out self, *, copy: Self)` and
  `__init__(out self, *, move: Self)` lifecycle declarations drive copy and move
  construction through the existing checked MIR and VM lifecycle machinery.
- Calls materialize Copyable read arguments before overlapping `mut`/`ref`
  access, allowing calls such as `f(mut x, x)` while retaining alias errors for
  non-Copyable values and multiple exclusive accesses.
- Current `ImplicitlyDestructible` lifecycle vocabulary replaces the obsolete
  `ImplicitlyDeletable` spelling in bundled sources and generic checking.
- Validated, nonraising `@implicit` constructors now provide explicit MIR-lowered
  conversions for typed bindings, arguments, returns, and overload selection.
- `@explicit_destroy` retains its diagnostic and named destructors in checked/MIR
  metadata. Path-sensitive obligations reject abandonment, overwrite, partial,
  double, and conditional destruction; raising destructors preserve the value
  for an `except` fallback, and automatic VM destruction is suppressed.

## [0.1.0] - 2026-07-15

Initial crates.io release.

### Added

- Indentation-sensitive lexer, Pratt parser, semantic checker, HIR and flattened
  MIR pipeline, ownership analysis, drop elaboration, and register VM.
- Functions, methods, structs, traits, generics, overloads, compile-time
  elaboration and VM-backed CTFE for the supported subset.
- Move checking, partial moves, ASAP destruction, stable origins, persistent
  loans, local and cross-call references, reference returns, and frame/slot
  runtime handles.
- Scalar, string, list, tuple, range, exception, iterator, unsafe-pointer, and
  VM-emulated `SIMD[...]` lane-vector semantics needed by the bundled self-hosted
  standard-library proofs. The VM executes lanes serially; hardware SIMD and
  native vector code generation are not included.
- Dotted, relative, qualified, and aliased source-module imports; package
  `__init__.mojo` discovery and re-exports; collision-free linked identities;
  and bundled `std` search roots.
- CLI stages for lexing, parsing, checking, ownership verification, and running
  `.mojo` source files.
- A versioned CPU-parity manifest and Pixi-driven differential harness for
  matching execution output and matching compiler rejection against a pinned
  Mojo reference build.
- A validated Mojo 1.0.0b2 manual inventory that distinguishes parity,
  strict-subset gaps, divergences, representation differences, exclusions, and
  stretch goals; every recorded divergence has an executable differential case.
- An expanded differential corpus covering the implemented first-pass parity
  surface with matching execution, matching rejection, strict-subset,
  acceptance-divergence, and output-divergence modes. The comparison also pins
  lowercase Bool formatting and Mojito's conservative same-place mutable-call
  rejection as known differences from the reference build.
- Mojo-compatible module-scope validation: production compilation rejects
  executable file-scope statements and enters runtime code through `main()`.
- Source package namespace completion includes wildcard privacy for
  underscore-prefixed declarations and isolates same-named declarations and
  overload sets from different modules.
- Module namespaces now preserve lexical shadowing, support imports inside
  functions and nested blocks, resolve unaliased full dotted paths and exported
  types, and implement dots-only relative sibling-module imports.
- User-defined static methods now type-check, participate in overload selection,
  lower without an implicit receiver, and execute with default and keyword arguments.
- `raise` now requires a surrounding handler or a `raises` function/method, and
  direct calls to raising free functions must be handled or propagated.
- Raising instance and static methods now retain their effect through method
  overload selection, so calls must likewise be handled or propagated.
- Non-capturing functions are runtime values with checked function types and can
  be stored, passed as arguments, and invoked through MIR indirect calls.
- Function types retain their `raises` effect; selected free-function overloads
  and indirect callable calls now require effect handling or propagation.
- Typed and parametric errors now survive parsing and checking through direct,
  overloaded, method, and indirect calls. Handlers receive the inferred typed
  error value, and `Never` acts as the bottom and nonraising error type.
- Free functions support a single named `out` result with caller-transparent
  invocation, checked initialization, and direct VM return-slot execution.
- Generic free functions accept heterogeneous `*args: *ArgTypes` packs, check
  every supplied type against the pack bound, and execute type-erased pack
  length queries. Compile-time loops can specialize literal/constructed packs,
  query `args.__len__()`, and index elements through their common bound.
- Expected function types contextually specialize non-overloaded generic function
  values for checked indirect invocation. Hand-written constructors now share
  default and keyword argument binding with free and method calls.
- Overload selection now follows first-pass Mojo precedence across conversion
  counts, fixed versus variadic candidates, signature length, and generic ties;
  defaulted and variadic declarations can participate in overload sets while
  overlapping defaulted calls retain ambiguity.
- Trait refinement now inherits method and associated-member requirements, and
  executable defaults are statically materialized with override/ambiguity rules.
- Lifecycle definite initialization follows normal, returning, raising,
  branching, looping, and protected exceptional paths instead of collecting
  assignments flow-insensitively.
- Opaque trait-bounded indexing dispatches through `__getitem__`; the self-hosted
  library includes an incremental hasher proof; and user-defined printed values
  must opt into Writable/Representable formatting. Bool output is `True`/`False`.

### Scope

- Targets an evolving single-threaded CPU subset of Mojo.
- GPU execution, concurrency/parallelism, distributed execution, Python
  interoperability, MLIR, and optimized native code generation are not included.

[0.1.0]: https://github.com/bpr/mojito/releases/tag/v0.1.0
