# Symbol-Level Architecture Map

This map answers “where does this rule live?” It names the production entry
points and the symbols that own cross-phase contracts. Keep it synchronized with
refactors; implementation details belong in `docs/architecture.md`.

## Production Path

| Stage | Owning symbols | Output / invariant |
|---|---|---|
| Driver | `compiler::Compiler::{compile_path, compile_source, execute}`, `compiler::CompiledProgram::{mir, elaborated_mir, emit_mir}` | The only whole-program stage ordering; caches the ownership-verified pre-drop MIR and, lazily, the drop-elaborated re-verified artifact that backend execution and canonical emission share. |
| Lex | `lexer::Lexer`, crate-level `lex` | Spanned token stream. |
| Parse | `parser::Parser::{parse_program, parse_program_diagnostic}`, crate-level `parse` | Spanned AST; diagnostic partial AST is quarantined. |
| Link | `module::{link_with_options, link_source_with_options, LinkOptions, ModuleError}` | Dependency-first flat program with `SourceSpan` module identity, explicit-binding collision checks, canonical self-import checks, and provisional exports for mutual cycles. `builtin_module_exports` gives the docstring-only `std.traits`/`std.origin` homes their builtin identity exports. |
| Comptime | `comptime::elaborate`, `ct::CtValue` | Ordinary AST with compile-time control resolved. |
| Check | `checker::{check_program, Checker}`, `checked::CheckedProgram` | Authoritative semantic handoff and side tables. |
| HIR | `hir::Cfg::build_checked_fn` (unchecked `build`/`build_fn` are phase-test compatibility) | Statement CFG with nested expressions. |
| MIR | `mir::lower_checked_program`, `mir::MirProgram` | Fully register-typed A-normal IR, places, declaration metadata, source table. |
| MIR text | `mir::text::{disassemble, parse_artifact, verify_artifact, load_artifact, ParsedArtifact, ArtifactSourceMap, ArtifactReport}` | Canonical serialization, source-located Mojo-independent artifact parsing, and the parse-then-verify loading gate that maps canonical `mir::verify` findings to artifact spans. |
| Verify | `mir::verify::verify` | Semantic verification of typed MIR: register/place types, concrete and inline abstract call contracts, variadic ABI conventions, CFG edges, effects, reference capabilities, loans, and interior origins. |
| Ownership | `analysis::check_ownership_program` (checked wrapper `check_ownership_checked`) | Move/init and loan validation over lowered MIR. |
| Drops | `analysis::elaborate_drops_program` | MIR with explicit `DropVar` operations; re-verified before execution. |
| Execute | `compiler::Compiler::execute`, `backend::Backend::{run, run_elaborated}`, `backend::vm::VmBackend` | Production execution consumes the cached `CompiledProgram::elaborated_mir` artifact; `run_elaborated` executes it or a loaded artifact without rewriting it. |
| Artifacts | `artifact::{run_artifact, ArtifactRunError}` | Load-then-execute composition for textual MIR artifacts: the `load_artifact` gate plus `Backend::run_elaborated`, shared by the CLI `exec` subcommand and tests. |

## Cross-Phase Contracts

| Concern | Sole owner | Consumers |
|---|---|---|
| Structural call binding | `call::{match_call_slots, ArgSlot, CallSlots}` | Checker and VM call adapters. |
| Parser-to-call marker normalization | `call::{regular_marker_index, effective_keyword_only_index}` | Checker and MIR declaration lowering. |
| Callable identity and overload names | `symbol::{SignatureKey, OverloadSets, lowered_def_name, lowered_method_name, function_symbol, method_symbol}` | Checker, MIR, VM registries, symbol tests. `SignatureKey` retains positional types, keyword-only names, and the keyword-variadic collector role. |
| Checked semantic facts | `checked::{CheckedProgram, CheckedConst, AnnotationSite, CheckedCallContract, CheckedIteratorCall, CheckedResultAdapter}` | MIR, ownership driver, backends. |
| Source annotation syntax | `ast::SourceType` (alias of the AST `Type` node) | Parser, checker input, HIR/MIR source metadata. |
| Source location/provenance | `token::{Span, SourceSpan}` | AST, checker side tables, MIR diagnostics. |
| Compile-time values | `ct::CtValue` | Elaborator, specialization, checked constants. |
| Semantic types | `types::{Ty, TyArg, ParamDecl}` | Checker, checked data, MIR declarations, VM coercion. |
| Runtime values/operations | `runtime::{Value, coerce_checked, apply_infix, apply_prefix}` | VM and VM-backed CTFE. |
| Backend contract | `backend::{Backend, BackendKind}` | Compiler driver and CLI. |
| Native compile (experimental, `backend-pliron`) | `backend::pliron::{compile, CompileOptions, NativeModule, EmitKind, OptLevel, NativeTarget, JitValue, TrapCategory, PlironError, runtime_declarations}` | CLI `compile`/`run --backend pliron` and the capability-manifest differential harness. |
| Shared native ABI | `native::target::{Triple, CpuFeatures, NativeTarget, BuildConfig, OptLevel, EmitKind}`, `native::layout::{LayoutCx, StructFieldIndex, compose}`, `native::mangle::mangle`, `native::rt_abi` | Every native backend, the CLI, `crates/mojito-runtime` agreement tests, and the LLVM cross checks. |

## Source Versus Checked Naming

Names crossing a phase boundary must say which representation they contain:

- `SourceType`, `source_annotation`, and `param_annotations` preserve syntax
  written in the source program. They are not proof that a type is valid.
- `Ty`, `checked_type`, and `param_types` are checker-produced semantic facts.
- `AnnotationSite` identifies a source annotation; `CheckedProgram::checked_type_at`
  retrieves the semantic `Ty` resolved for that site.

Do not use an unqualified `type` field for source syntax in HIR or MIR. Compiler
invariant failures—such as checked metadata missing at a required annotation
site—must be returned as diagnostics, never encoded with `expect`, `unwrap`, or
`unreachable!` at a phase boundary.

## Internal Responsibility Boundaries

### Checker

- `checker::Checker` is one type whose ~250 methods are split across
  `impl Checker` blocks in the `checker/` submodules below; `checker.rs` retains
  the struct, constructors, `check_program` glue, the shared prelude types
  (`StructInfo`, `MethodSig`, overload helpers, `ConformanceOracle`), and the
  call-effect/coercion helpers. Submodules extract by responsibility, not by
  line count; a moved method is `pub(super)` so siblings and the parent can call
  it.
- `checker/statements.rs` owns `check_program`, block scoping, and the
  `check_stmt` statement dispatcher, including generic comptime alias lowering
  (`check_generic_comptime_alias` fills the `Checker.comptime_aliases`
  registry of `ComptimeAlias` entries — classified `ParamDecl`s plus an
  `AliasBody`: a symbolic type template or, for a Bool-bodied predicate
  alias, a symbolic `GenericConstraint`; `check_program` pre-registers
  module-level aliases like struct shells). `check_def` checks one function declaration — the
  `StmtKind::Def` arm delegates to it, and lambda mode (`lambda = true`)
  applies the lambda-specific capture-default/thinness/diagnostic deltas.
- `checker/inference.rs` owns expression inference (`infer`/`infer_impl`),
  list/tuple/variant construction, and t-string typing (the lazy `TString`
  element list and its snapshot capture policy). `infer_variant_method` is the
  shared Variant-intrinsic dispatch (`isa`/`is_type_supported`/`set` — both
  value and `init_with=` placement forms — `unwrap`/`unsafe_unwrap`,
  `replace`/`unsafe_replace`, and the consuming `deinit_with`), reached from
  the parameterized `Invoke(Member)` spelling via `infer_variant_invoke` and
  from ordinary method calls via `infer_method_call`. `check_lambda` runs a lambda
  expression's hidden definition through `check_def` during statement-root
  registration and caches the finalized function-value type under the
  expression span (the comprehension pattern); `ast::lambdas_in_expr`/
  `lambdas_in_stmt` are the shared lambda-discovery walkers used by the
  checker, `checked.rs`, and `mir/nested.rs`.
- `checker/indexing.rs` owns place validation, subscript/index inference and
  assignment (including keyword slices and the `BorrowViewResult` marking for
  view-typed slice results), pointer offset/write checks (the single-place
  rule and its multi-element interior-domain lift), the positional
  String-slice rejection hint, and member access.
- `checker/method_calls.rs` owns method-call inference, overload scoring
  (score ties on receiver-overloaded methods break by the call's explicit `^`
  transfer), and static/pointer/uninit-storage/List/Tuple method inference
  (`infer_uninit_storage_method` types the compiler-private
  `__UninitStorage[T]` write/take/destroy crossings behind
  `UnsafeMaybeUninit`).
- `checker/call_inference.rs` owns free-function and callable-value call
  inference (`infer_call`, generic-call instantiation).
- `checker/type_resolution.rs` resolves source annotations into checked `Ty`
  (builtin type-argument forms, dependent/associated projection, and generic
  comptime alias expansion via `resolve_comptime_alias`). It also owns pointer
  origin arguments (`pointer_origin_arg`/`pointer_origin_expr`: origin
  parameters, `origin_of(place)`, `._get_owned_interior["tag"]` projections
  in both annotation and expression shapes — the multi-element pointer
  marker — and the terminal conservative `._subtree` projection via
  `append_subtree`) and the annotation alias table (`StringSlice` →
  `StringSpan`, `MutPointer`/`ImmPointer`).
- `checker/traits.rs` owns trait/struct declaration checking, conformance
  (nominal and built-in), and type-capability queries (`is_deinitable`,
  `is_movable`, `is_copyable`, …). Deprecated lifecycle spellings
  (`ImplicitlyDeletable`/`__del__`) normalize to the canonical
  `Deinitable`/`__deinit__` vocabulary via `ast::canonical_trait_name` /
  `ast::canonical_destructor_name`, applied by the parser at the semantic
  positions and by the checker where trait names are extracted from
  expressions.
- `checker/origins.rs` owns origin/reference-handle derivation, interior and
  aggregate-origin tracking, capture-origin collection, origin-signature
  lowering, and cross-call transfer effects (`abstract_body_origin`,
  `record_transfer_effect`, the `apply_transfer_effects` name-keyed wrapper
  over the `replay_transfer_effects` core, value-position effect baking
  (`bake_value_transfer_effects`), the higher-order call-through channel
  (`record_call_through`, `apply_call_through_effects`,
  `translate_call_through`), and the span-keyed `CheckedCallTransfer`
  handoff to MIR, destination interior paths included).
- `checker/scopes.rs` owns lexical scope, binding declaration/mutability, and
  nested-def capture-access checks.
- `checker/constraints.rs` owns compile-time evaluation and generic-constraint
  compilation/evaluation. `compile_where_clause` retains an optional source
  diagnostic around the semantic constraint compiled by
  `compile_generic_constraint`; declarations compile one constraint per
  trailing `where` clause. `lower_parameterized_member` lowers the symbolic
  template shared by parameterized associated members and generic comptime
  aliases. Predicate aliases live here too: `compile_predicate_alias_body`
  lowers a Bool body, and `predicate_alias_application`/`apply_predicate_alias`
  recognize and inline an application (shared by constraint compilation and
  `traits.rs`'s raw conformance-condition evaluator); the elaborator's
  `comptime if` path applies module-scope aliases via `Elab::apply_generic_alias`
  (`comptime/eval.rs`). The `TypeList` vocabulary shares both homes:
  `compile_typelist_proposition`/`typelist_receiver` lower the constraint
  forms (`PackPredicate`/`PackContains`/`PackLength` in `types.rs`), and the
  elaborator evaluates compile-time TypeList values (`eval_typelist_of`,
  `eval_typelist_method`, the `make_typelist` marker in `comptime/eval.rs`).
- `checker/operators.rs` and `checker/iteration.rs` own operator/SIMD inference
  and iterator-protocol selection respectively.
- `checker/calls.rs` adapts neutral call matching to `TypeError` and validates
  checker-only signature rules.
- `checker/places.rs` owns call-site place classification and alias rejection.
- `checker/generics.rs` owns unification, substitution, and callable/method
  specialization.
- `checker/declarations.rs` owns parameter classification and method/function
  signature and body checking.
- `checker/annotations.rs` converts AST annotations into checked `Ty` values.
- `checker/builtins.rs` owns built-in typing/coercion rules and builtin
  free-function inference (`print`/`len`/`range`/…).

### MIR

- `mir/ir.rs` defines `MirInstr`, `MirTerm`, `MirPlace`, `MirFunction`, and
  `MirProgram`.
- `mir/text.rs` owns the public disassembler, the verified artifact-loading
  entry points (`verify_artifact`/`load_artifact`, including the mapping of
  canonical `mir::verify` finding prefixes to artifact spans), textual-schema
  version constants, reserved words, canonical escaping, and exhaustive
  instruction/terminator/type spellings (with the canonical
  `INSTRUCTION_MNEMONICS`/`TYPE_SPELLINGS` inventories the native capability
  matrix pins against); `mir/text/write.rs` owns structural
  serialization and ordering.
- `mir/text/parse.rs` owns UTF-8 validation, the recoverable spanned schema
  parser, full-schema typed reconstruction (every instruction, terminator,
  type, origin, and declaration-metadata form, including nested try-region
  block namespaces), structural diagnostics, and artifact source mapping. It
  does not own semantic MIR verification.
- `mir.rs` owns the `Flatten` ANF-lowering driver, core emission primitives, and
  the `lower_cfg`/`lower_program` entry points. `Flatten`'s methods are split by
  responsibility across `impl Flatten<'_>` blocks in the submodules below.
- `mir/facts.rs` reads `CheckedProgram` facts during lowering (checked types,
  call contracts, adjustments, capture accesses).
- `mir/calls.rs` owns call-site lowering (arguments, keywords, receiver,
  reference results, checked-call boundaries, interior-origin invalidations).
- `mir/lower_expr.rs` owns expression lowering (the `expr_unconverted`
  dispatcher, collections/comprehensions, nested closures, the
  field-invocation indirect-call branch), and installs merged caller-side
  `EstablishLoans` — domain-keyed for interior-precise destinations — for
  checked call-transfer records (`install_call_transfers`) after free,
  method, indirect, and nested calls.
- `mir/lower_stmt.rs` owns statement, place, subscript-assignment, `try`-region,
  and terminator lowering, plus the borrowed-iteration source binding and loan
  re-establishment helpers shared with comprehension lowering.
- `mir/nested.rs` owns capture analysis and nested-function lifting.

### VM and Comptime

- `native.rs` (un-gated; normative contract `docs/native-abi.md`) owns the
  shared native target, layout, and runtime ABI consumed by every native
  backend: `native/target.rs` (checked build configuration — `Triple` with
  the pinned data-layout string, `CpuFeatures`, `NativeTarget`,
  `BuildConfig`, `OptLevel`, `EmitKind`), `native/layout.rs` (the layout
  owner — `LayoutCx`, `StructFieldIndex`, `Layout`/`StructLayout`/
  `VariantLayout`, `compose`), `native/mangle.rs` (injective C-safe `mj_`
  symbol escaping; `exit`/`mjrt_*`/`mjstr.*`/`main` sit outside the mangle
  image), and `native/rt_abi.rs` (the runtime C ABI contract table —
  `MJRT_ABI_VERSION`, trap categories, `RT_SYMBOLS`/`RT_DATA_SYMBOLS`/
  `RT_TYPES`, `type_layout`).
- `crates/mojito-runtime` (workspace member, independently versioned,
  dependency-free) implements that contract as the linked `mjrt_*` C ABI:
  version symbols, `mjrt_alloc`/`mjrt_dealloc`, `mjrt_write_stdout`, the
  VM-display `mjrt_fmt_*` formatters, and `mjrt_trap`. It must never depend
  on the `mojito` crate (the VM `Value` stays out of the ABI);
  `tests/native_abi_test.rs` pins the Rust-side agreement.
- `backend/pliron.rs` (feature `backend-pliron`) owns the experimental native
  backend: `compile` orchestration (reachable closure, verify, mem2reg/DCE,
  canonical text), `NativeModule` emission/JIT entry points,
  `runtime_declarations` (the contract table's LLVM rendering), `JitValue`,
  and the `TrapCategory` exit-code/VM-message contract (`OptLevel`/
  `EmitKind`/`NativeTarget` re-export from `native::target`). Its
  submodules: `backend/pliron/lower.rs` (MIR-to-LLVM-dialect lowering —
  scalar operators/conversions with keyword/default call binding via
  `call::match_call_slots`, trap guard blocks, the sanitized `MIN // -1`
  divisor, the `mjrt_pow` helper, aggregates/strings/allocation, Stage 4's
  tagged-outcome raising ABI, structural `try`/`finally` flattening with
  per-variable initialization flags and pending-outcome dispatch, references
  as place addresses, `mjrt_trace` lifecycle emission under
  `CompileOptions::trace_lifecycle`, and the exe wrapper that references
  `mjrt_version` and consumes a raising entry's outcome),
  `backend/pliron/emit.rs` (target stamping onto every LLVM module, LLVM
  IR/bitcode/object/exe via clang `--target`, runtime-archive discovery and
  linking, plus the `opt`-subprocess `O1` pipeline), and
  `backend/pliron/jit.rs` (host-only ORC LLJIT execution typed by
  `RetKind`), and `backend/pliron/capability.rs` (the generated capability
  matrix behind `conformance/pliron-capability.tsv`, pinned against the
  textual-MIR schema vocabulary).
- `backend/vm.rs` owns the `VmBackend` core: heap, value operations, method-call
  and named-call execution, drops, formatting, and the test-only ordered
  lifecycle-event log (`enable_lifecycle_log`/`lifecycle_log`) the native
  trace differential compares against. Its remaining methods are
  split across `impl VmBackend` blocks in the submodules below.
- `backend/vm/frames.rs` owns call-frame construction and the `drive_frames`
  dispatch loop (`call_frame`/`make_frame`/`prepare_direct_call`).
- `backend/vm/references.rs` owns runtime reference handle read/write/projection.
- `backend/vm/exec.rs` owns the `exec_instr` instruction dispatcher and
  `try`-region execution.
- `backend/vm/calls.rs` turns `CallSlots` into runtime values and frame slots.
- `backend/vm/places.rs` navigates projected runtime storage, including the
  `UninitPayload` projection into inline uninit storage (a final payload store
  initializes-or-overwrites raw; reads trap while uninitialized).
- `comptime.rs` owns the `Elab` elaboration driver (`block`/`stmt`), type
  resolution, and the free-function/`Mono` support code; `Elab`'s remaining
  methods are split across `impl<'a> Elab<'a>` blocks in the submodules below.
- `comptime/eval.rs` owns compile-time expression evaluation (the `eval`
  dispatcher, reflection methods, infix/iteration folding).
- `comptime/ctfe.rs` owns VM-driven compile-time function evaluation and the
  VM-CTFE program rewrite and safety analysis.
- `comptime/specialize.rs` owns monomorphization and `def`/`struct`
  specialization synthesis (`generate_struct_spec`, tuple-spec ordering, and
  Tuple/TString request seeding).
- `comptime/mono.rs` owns the monomorphizing AST rewrite (`mono_type` and
  friends), struct-specialization argument resolution, and the t-string
  desugar into its `TString` specialization's construction.
- `comptime/rewrite.rs` owns AST substitution and value materialization.

## Change Routing

| If you change… | Start at… | Also inspect… |
|---|---|---|
| Syntax or AST shape | [`grammar.md`](grammar.md), `parser.rs`, `ast.rs` | Parser tests, `frontend.md`, feature matrix. |
| Argument binding | `call.rs` | Checker/VM adapters and call-parity tests. |
| Overload identity | `symbol.rs` | Checker selection, MIR declarations, symbol/rejection tests. |
| Type rules | `checker.rs` or focused checker child | `CheckedProgram`, negative checker tests. |
| Ownership/destruction | `analysis/mod.rs` | MIR place/use forms, ownership and drop tests. |
| Runtime behavior | `backend/vm.rs` or `runtime/mod.rs` | VM tests and file fixtures. |
| Pipeline ordering | `compiler.rs` | CLI, architecture doc, compiler tests. |
| Support status | `docs/features.md` | Roadmap/todo only if future work changes. |
