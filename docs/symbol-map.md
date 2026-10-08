# Symbol-Level Architecture Map

This map answers “where does this rule live?” It names the production entry
points and the symbols that own cross-phase contracts. Keep it synchronized with
refactors; implementation details belong in `docs/architecture.md`.

Module paths below (`checker::…`, `mir::…`) are the root `mojito` facade's
paths; since the crate split each phase lives in its own workspace crate
(`crates/mojito-<phase>/`) and the facade re-exports it unchanged. The crate
map and dependency DAG live in `docs/architecture.md` §Workspace Layout.

## Production Path

| Stage | Owning symbols | Output / invariant |
|---|---|---|
| Driver | `compiler::Compiler::{compile_path, compile_source, execute}`, `compiler::CompiledProgram::{mir, drop_elaborated_mir, concrete_mir, emit_mir}`, `compiler::VmInstantiation` | The only whole-program stage ordering. Holds the three MIR phases apart, each computed once: parametric MIR (`mir`), drop-elaborated MIR (`drop_elaborated_mir`, what `emit_mir` serializes), and concrete MIR (`concrete_mir`, the elaborator's cached output every backend consumes). |
| Lex | `lexer::Lexer`, crate-level `lex` | Spanned token stream. |
| Parse | `parser::Parser::{parse_program, parse_program_diagnostic}`, crate-level `parse` | Spanned AST; diagnostic partial AST is quarantined. |
| Link | `module::{link_with_options, link_source_with_options, LinkOptions, ModuleError}` | Dependency-first flat program with `SourceSpan` module identity, explicit-binding collision checks, canonical self-import checks, and provisional exports for mutual cycles. `builtin_module_exports` gives the docstring-only `std.traits`/`std.origin` homes and the `std.hashlib` `Hashable`/`Hasher` homes their builtin identity exports. |
| Comptime | `comptime::elaborate`, `ct::CtValue` | Ordinary AST with compile-time control resolved. |
| Check | `checker::{check_program, Checker}`, `checked::CheckedProgram` | Authoritative semantic handoff and side tables. |
| HIR | `hir::Cfg::build_checked_fn` (unchecked `build`/`build_fn` are phase-test compatibility) | Statement CFG with nested expressions. |
| MIR | `mir::lower_checked_program`, `mir::MirProgram` | Fully register-typed A-normal IR, places, declaration metadata, source table. |
| MIR text | `mir::text::{disassemble, parse_artifact, verify_artifact, load_artifact, ParsedArtifact, ArtifactSourceMap, ArtifactReport}` | Canonical serialization, source-located Mojo-independent artifact parsing, and the parse-then-verify loading gate that maps canonical `mir::verify` findings to artifact spans. |
| Verify | `mir::verify::{verify, verify_concrete, concrete_function_findings, instruction_named_types}` | `verify_concrete` is the mode for elaborated MIR: `verify` plus the rules of `verify/concrete.rs`, which reject symbolic types, compile-time parameters, layout queries, and compile-time argument slots that forward a binder or compute an expression over one; `verify/scope.rs` is the parametric mode's binder-scope and kind rule for every type a body names (`docs/notes/param-expr-attributes.md` §Register types), and `instruction_named_types` the types an instruction names beside its places; `concrete_function_findings` is the same rules for one body, which `native::mono` asks while materializing an instance. `verify` is semantic verification of typed MIR: register/place types, concrete and inline abstract call contracts, variadic ABI conventions, CFG edges, effects, reference capabilities, loans, and interior origins. `verify/instr.rs` owns `verify_instruction`: the place and register checks every instruction gets, then a dispatch on the instruction's family over one `InstrCx`. Each family's checks sit with the rules they use: loans and interior invalidation in `loans.rs`, the iterator protocol in `iteration.rs`, subscripts in `subscripts.rs`, calls in `calls.rs`, and references, value construction, private storage, SIMD, and raise/`try` effects in `instr.rs`. |
| Ownership | `analysis::check_ownership_program` (checked wrapper `check_ownership_checked`) | Move/init and loan validation over lowered MIR; `analysis/moves.rs` walks `try` regions per channel (`walk_region`/`walk_try`). |
| Drops | `analysis::elaborate_drops_program` | MIR with explicit `DropVar`/`ConsumeVar` operations, plus `DropPlace`s before writes that overwrite an initialized droppable place — a field store, and a whole place written through a reference (a `mut` parameter's or `mut self` receiver's `WriteRef`, a place pointer's or `ref` binding's `Store`) — from `analysis/store_drops.rs` (`elaborate_store_drops`, whose `out self` receiver test is `mojito_symbol::symbol::is_initializer_symbol`) and per-field `DropPlace`s for `deinit` parameters from `analysis/field_drops.rs` (`refine_deinit_fields`); re-verified before execution. |
| Execute | `compiler::Compiler::{execute, with_vm_instantiation}`, `backend::Backend::{run, run_concrete, run_elaborated}`, `backend::vm::VmBackend`, `artifact::{run_artifact, run_artifact_as}` | Production execution runs the cached `CompiledProgram::concrete_mir` through `run_concrete`; a loaded artifact is elaborated the same way first. `run_elaborated` runs drop-elaborated MIR erased, as the differential oracle (`VmInstantiation::Erased`, `MOJITO_VM_ERASED`, `--erased`). `run` is the stage-composed test seam, which stays erased. |
| Artifacts | `artifact::{run_artifact, ArtifactRunError}` | Load-then-execute composition for textual MIR artifacts: the `load_artifact` gate plus `Backend::run_elaborated`, shared by the CLI `exec` subcommand and tests. |

## Cross-Phase Contracts

| Concern | Sole owner | Consumers |
|---|---|---|
| Structural call binding | `call::{match_call_slots, match_fieldwise_slots, ArgSlot, CallSlots}` | Checker and VM call adapters; `match_fieldwise_slots` binds a `@fieldwise_init` construction's keywords to fields for the checker, template facts, and MIR, which lists the registers in field order from `OverloadSets::fieldwise_parameters`. |
| Parser-to-call marker normalization | `call::{regular_marker_index, effective_keyword_only_index}` | Checker and MIR declaration lowering. |
| Callable identity, overload/dispatch, and native instance names | `symbol::{SignatureKey, VariadicKey, InstanceArg, OverloadSets, resolve_callable_symbol, resolve_method_symbol, instance_symbol, canonical_specialization_type, unqualified_instance_name, lowered_def_name, lowered_method_name, static_method_symbol}` | Checker, MIR, VM, native monomorphization, symbol tests. Runtime and native method retargeting share one declaration-view policy. A receiver-overloaded static (`Tuple.__len__()` beside the instance `__len__`) keys as `static_method_symbol`; a minted Tuple instance spells bare in overload keys and with its elements in instance names (`KeyMode`). An `instance_symbol` whose argument spelling passes 96 characters names its arguments by a SHA-256 digest (`W$mono$H…`), and a nested instance argument spells as its own symbol, so a symbol's length is bounded at any nesting depth. A struct's variadic runtime-pack constructor beside a same-arity nullary one is selected by the VM's `constructor_name`, the monomorphizer's `runtime_pack_constructor`, and the native `constructor_init` alike. |
| Checked semantic facts | `checked::{CheckedProgram, CheckedTables, CheckedConst, AnnotationSite, CheckedCallContract, CheckedIteratorCall, CheckedResultAdapter}` | MIR, ownership driver, backends. `CheckedProgram::tables` is the one `Arc<CheckedTables>` (expressions, declarations, span indexes) that every `hir::Cfg` and MIR `Flatten` shares — lowering never copies program-wide tables. |
| Source annotation syntax | `ast::SourceType` (alias of the AST `Type` node) | Parser, checker input, HIR/MIR source metadata. |
| Source location/provenance | `token::{Span, SourceSpan}` | AST, checker side tables, MIR diagnostics. |
| Compile-time values | `ct::CtValue` | Elaborator, specialization, checked constants. `CtValue::Expr` is a residual parameter expression, `CtValue::Deferred` the slot of a binder (named by `ParamRef`) outside generic identity, and `CtValue::Marker` an elaborator-private `CtMarker` classification of a name; none is a constant. |
| Parameter expressions | `param_expr::{ParamContext, ParamExpr, ParamKind, ParamOp, MetaTy, ParamId, ParamRef, ParamBindings, ParamEval, ParamError, ConstraintVerdict, ParamConstraint, identity_eq, SIZE_OF_FUNCTION, builtin_application_value}` and `param_expr::fold::{fold_infix, fold_neg, fold_invert, compare}` (crate `mojito-types`) | The typed, canonical, per-compilation-interned form of a value argument, a value default, a callable default's condition, and a dependent type (`types::DependentType::Parameter`). `ParamContext` owns construction, canonical form, `rebuild` (an operator rebuilt from its parts as the constructors kept it, for the A1 payload's export and the MIR text reader), `replace` (type identity) and `evaluate`/`fold` (a required value); `fold` is the one implementation of compile-time scalar operators. `ParamKind::Apply` (`ParamContext::{apply, apply_evaluated, size_of, dtype_float_query}`) is a compile-time application, never folded by `replace` and equal by structure (`evaluate` answers a builtin one, `answer_builtin_applications`), carrying the value the compile-time route established when one has; the checker builds one for a module constant whose initializer applies a function (`Checker::applied_constant_expr`, kept by `TemplateCatalog::applied_constants`), and the elaborator keeps such a constant's name in every type argument (`CtMarker::Applied`). Users: the checker (`constraints.rs`), the elaborator (`eval.rs`), MIR text and the verifier, native monomorphization (`mono/symbolic.rs`), the VM's default resolution, and `symbol::mangle`. `types::{TyRewrite, rewrite_ty, replace_parameters, referenced_parameters}` is the one type traversal behind replacement. `Ty::Simd`'s slots are `types::{SimdDtype, SimdWidth}`; a lane gather's mask is `types::LaneMask`, known or a template's `shuffle`/`slice`/`join` form, and `LaneMask::{close_with, resolve}` are the one closing and constraint rule the elaborator (`close_lane_masks`) and the erased VM share; `simd_ty_from_slots` is the only constructor of a symbolic vector type, and `simd_shape`, `simd_slots`, `is_scalar_simd`, `scalar_simd_dtype`, and `simd_lane` read the slots. A type binder is the same identity: `Ty::Param { binder: ParamRef, .. }` and `ParamDecl::{Type, Value}.id` carry the declaration's `ParamId`, `types::TySubst` (`HashMap<ParamId, Ty>`) is every type substitution's key, and `types::CONTRACT_BINDER_OWNER` owns a canonicalized contract's slots. The identity crosses the waist on `MirInstr::ConstructTypeParam.param` and `MirParamArg.binder` (a forwarded enclosing binder), and native monomorphization's `Bindings.types` and `Bindings.values` are keyed by `ParamRef`. Design: `docs/notes/param-expr-attributes.md`. |
| Semantic types | `types::{Ty, TyArg, ParamDecl}` | Checker, checked data, MIR declarations, VM coercion. |
| Runtime values/operations | `runtime::{Value, coerce_checked, apply_infix, apply_prefix}` | VM and VM-backed CTFE. |
| Backend contract | `backend::{Backend, BackendKind}` | Compiler driver and CLI. |
| Phase timing (`--timings`) | `timing::{enable, enabled, span, round, count, report}` (crate `mojito-common`) | Every phase crate records spans; the CLI enables collection and prints the report; `scripts/bench-compile` and `tools/bench` parse it. Disabled, a span is one relaxed atomic load. |
| Instantiation census (`--instantiation-census`) | `census::{InstantiationCensus, CloneCensus, CloneClass, ErasedServed}` (crate `mojito-checked`) | The elaborator classifies what it mints (`comptime/census.rs`, `Elaborated::clones`); the checker records distinct inferred and derived instance bodies in `TemplateStats::{inferred_instances, derived_instances}`; `native::mono` reports `SpecializedProgram::parametric` (the template of each instance) and `parametric_bodies`; `CompiledProgram::instantiation_census` assembles them and the CLI prints them. `CloneCensus::minted` says whether the cloner minted a lowered symbol, from the source names `comptime/census.rs` records, which is how a clone that keeps a parameter is counted apart from an erased template. |
| `print` keywords | `infer_print` (`checker/builtins.rs`) | The VM's `print` arm (`dispatch.rs`; `file=` writes through `host_write_bytes`), pliron's `lower_print` (`lower/print.rs`; a `print_sink` descriptor makes `write_stdout` call libc `write`), `mojito_types::types::is_stdlib_file_descriptor_struct`. |
| Constructed defaults (`dir: Optional[String] = None`) | `CheckedConst::Construct` in the callee's declaration | The VM's `bind_for_call`; the native monomorphizer's `instantiate_constructed_defaults` (enqueues the constructor instance for the parameter type and respells the default's target), pliron's `reachable_set` (follows the default's target) and `bind_call_slots` (runs the instance over fresh storage). |
| Evaluated defaults (`s: String = String("a")`) | `mir::lower_default` lowers the default as the zero-parameter function `$default$<owner>$<parameter>` recorded as `CheckedConst::Evaluate`; the checker's `dynamic_default_reference` rejects one naming runtime storage and the elaborator's `check_default_effects` (`comptime/ctfe.rs`) one that does I/O | The VM's `bind_for_call` runs the function per call; pliron calls it from the caller (`evaluated_default_value` in `lower/calls.rs`), releasing a borrowed slot's value through `default_temps`. `native::mono` (`instantiate_constructed_defaults`) and pliron's `reachable_set` carry the edge no call instruction spells; a default function declaring the binders it reads is instantiated under its owner instance's arguments (`default_function_bindings`), and the erased VM refuses it. |
| Keyword-pack collectors (`var **kwargs: T`) | The callee declaration's `kw_variadic`, collected into the `StringDict` instance `instance_symbol("StringDict", [T])` | The VM's `make_kwargs_dict`; `native::mono`'s `enqueue_keyword_collector` keeps the instance's empty constructor and `__setitem__`, pliron's `reachable_set` follows them, and `build_keyword_collector` (`lower/calls.rs`) runs them from the direct (`bind_call_slots`) and indirect (`bind_contract_slots`) caller. |
| Narrow float lanes (`Float16`, `Float32`) | `ast::Dtype::{is_narrow_float, round_lane, float_literal_lane}` over `common::float::{f16, round_f16, f16_bits}` and `literal::FloatLiteral::{to_f16, to_f32}` (crate `mojito-common`) | The VM's lane rounding, literal materialization, `to_bits`, and `__fma__` (`runtime.rs`); `CtLane::from_value` (`mojito-types/ct.rs`); pliron's `widen_float_lane`/`round_float_lane` (`lower/arith.rs`), `lower_narrow_float_binop` (`lower/binops.rs`), `narrow_float_constant` (`lower/emit.rs`), and `simd_round_float_vector` (`lower/simd.rs`). |
| `DType` values | `ast::Dtype::{code, is_integral, is_floating_point, is_signed, is_unsigned, is_numeric, is_float8, is_half_float, predicate, float_query, repr_alias}` with `DTYPE_PREDICATES`, `DTYPE_FLOAT_QUERIES`, the `DTYPE_*_MASK` bits, and `UPSTREAM_ONLY_DTYPE_NAMES` (crate `mojito-ast`) | The checker's `dtype_constant` (`checker/indexing.rs`, recording `SemanticAdjustment::DtypeConstant` for `DType.<name>` and a `SIMD` type operand's `dtype` via `member_type_operand`; `infer_member` records it for a value's `dtype` beside `length`), its `dtype_float_query` (recording `SemanticAdjustment::ParamValue`: the answer at a known dtype, `ParamContext::dtype_float_query`'s application over a binder), and its `Ty::Dtype` method arm (`infer_dtype_predicate` in `method_calls/simd_receivers.rs`); MIR's `Const::Dtype` and `CheckedConst::Dtype`; the elaborator's predicate and float-query folds (`comptime/eval.rs`), a `SIMD` type's `dtype` in `associated_value` (`comptime/elab.rs`), and dtype equality (`compare_numeric_values`); the VM's `Value::Dtype` (display, `scalar_repr`, `hash_leaf_value`, the receiver arm in `invoke.rs`); pliron's `ScalarTy::Dtype` (`dtype_constant`, `dtype_text`, `lower_dtype_predicate`). A `DType` hashes as its `UInt8` code through `types::hash_leaf_ty`. |
| Scalar splat into a vector | `types::scalar_splats_into` (crate `mojito-types`) and `symbol::SCALAR_SPLAT_CONVERSION` (crate `mojito-symbol`) | The checker's `implicit_conversion_constructor` selects the splat as an implicit conversion; `checked.rs` turns its target into `SemanticAdjustment::SplatScalar` and `checked_call_boundary` (`method_calls/selection.rs`) into `CheckedCallValueAdjustment::SplatScalar`; MIR's `Flatten::splat_scalar` (`lower_expr/calls.rs`) emits the one-element `MakeSimd`. |
| Host call allowlist (`external_call`) | `mojito_types::ffi::{CType, FfiCallee, CALLEES, callee, accepts_arg, accepts_ret}` | The checker's `infer_external_call` (rejects any other callee, checks arguments and the declared return type), the VM's `backend/vm/libc.rs` table (std-only execution: descriptor table, `errno` slot, environment overlay, glibc `dirent` byte images, `_c_stat` fills by field name), and pliron's `lower/externs.rs` (on-demand `llvm.func` declarations, `open` variadic, real C calls). The callee travels as the call's first parameter argument; the result type is the destination register's checked type. |
| Compiler-called Mojo bodies | `stdlib/std/_intrinsics.mojo` (`_pow_int`, `_int_digits`, `_uint_digits`), named by `mojito_symbol::symbol::{POW_INT_SYMBOL, INT_DIGITS_SYMBOL, UINT_DIGITS_SYMBOL, DIGITS_BUFFER_BYTES, calls_pow_int}` | The Mojo implementations behind `**` on `Int`/`UInt` and integer display, which the builtin has no struct to hang a method on. `std.prelude` imports the module so every linked program carries them, and they stay out of `PRELUDE_EXPORTS`. Callers: the VM's `apply_binop`/`format_value`, the native monomorphizer's roots, and pliron's `reachable_set` plus `lower_pow`/`format_scalar`. |
| Generated string tables | `stdlib/std/_string_tables.mojo` (written by `scripts/gen-string-tables` from the pinned upstream `_unicode_lookups.mojo` and `_parsing_numbers/constants.mojo`) | Fixed-width hex records in string literals: the Unicode 16 case-mapping tables behind `StringSpan.upper`/`lower`/`isupper`/`islower` and the Eisel-Lemire power-of-five table behind `atof`; `string.mojo`'s `_hex_at`/`_hex_u64_at`/`_table_find` decode and binary-search them. Regenerate at every re-pin; never edit by hand. |
| String formatting | `stdlib/std/string.mojo` (`_FormatUtils`, `_FormatCurlyEntry`, `_PrecompiledEntriesRuntime`, and the `format` methods of `String` and `StringSpan`), the literal stand-in named by `mojito_symbol::symbol::{STDLIB_FORMAT_UTILS_STRUCT, FORMAT_LITERAL_METHOD}` | Upstream's `collections/string/format.mojo`, kept beside `String` because `format` builds a `String`. A string literal's `format` reaches `_FormatUtils.format_literal` through `infer_literal_format` (`checker/method_calls/intrinsic_receivers.rs`) and `SemanticAdjustment::LiteralFormat`; nothing in the VM or the native backend formats a template. |
| Native compile (`backend-pliron`) | `backend::pliron::{compile, compile_mir, CompileOptions, NativeModule, EmitKind, OptLevel, NativeTarget, JitValue, TrapCategory, PlironError, runtime_declarations}` | CLI `compile`/`run --backend pliron` and the capability-manifest differential harness. `compile` takes the driver's cached concrete graph and elaborates nothing; `compile_mir` elaborates drop-elaborated MIR from the entries a caller names. |
| Shared native ABI | `native::target::{Triple, CpuFeatures, NativeTarget, BuildConfig, OptLevel, EmitKind}`, `native::layout::{LayoutCx, StructFieldIndex, LayoutError, compose}`, `native::mangle::mangle`, `native::rt_abi` | Every native backend, the elaborator's answer to a layout query (`native::mono`, under the compilation's target; `LayoutError::Symbolic` is the one refusal of a type that still names a parameter), the erased oracle's `SizeOf` on the host, the CLI, `crates/mojito-runtime` agreement tests, and the LLVM cross checks. |
| The elaborator | `native::mono::{specialize, entry_roots, SpecializedProgram, MonoError, MonoErrorKind}`, `mir::ConcreteMir`, `mono/specializer.rs`: `InstanceState`, `Specializer::{demand_application, materialize_closure, resolve_applications, select_comptime_branches, demand_layout}`, `LayoutOracle`; `mono/rebind.rs`: `Specializer::discharge_rebinds`; `mono/failure.rs`: `Specializer::discharge_instantiation_failures` (a reached method clone whose instantiation failed above MIR), `reached_failure` (a parameter constant an unrolled iteration could not compute, reported once the instance reaches it); `mono/gather.rs`: `Specializer::close_lane_masks`; `mir::prune_unreachable_blocks` | The driver, for the VM and the native backend alike, and artifact execution. Clones an entry-rooted concrete MIR graph from drop-elaborated MIR, which it leaves unchanged. `entry_roots` is the one definition of a whole program's roots (`main`, `__toplevel__`). Its output is a `ConcreteMir`, whose only constructor runs `mir::verify::verify_concrete`, which owns the concreteness rules. `mono/promote.rs` owns the one shape whose instance signature differs from its template's: a callable parameter bound to a closure that captures becomes the instance's last runtime parameter, renumbering the variable slots it displaces (`mir::verify::instruction_places_mut` is the shared place inventory it walks). `mono/availability.rs` owns the verdict of a member's `where` clause (`MirFunctionDeclaration.availability`) under an instance's bindings, read from `MirStructDeclaration.conformances` and `MirDeclarations.traits`; the checker builds those rows in `Checker::conformance_facts` (`checker/traits.rs`), handed over as `CheckedProgram::conformances`. `mojito_types::conformance::leaf_conforms` is the one rule for a compiler-known type's conformance to a built-in trait, shared by the checker's `conforms_to` and the elaborator. `INSTANCE_BUDGET` (`mono.rs`) is the one elaboration bound, checked in `Specializer::enqueue`; `specialize` runs the elaborator on its own deep-stack thread. |

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
  call-effect/coercion helpers; the `ConformanceOracle` impl lives in
  `checker/conformance.rs`, and the free overload/signature and trait-support
  helpers live in `checker/overload_support.rs` and
  `checker/traits_support.rs`. Submodules extract by responsibility, not by
  line count; a moved method is `pub(super)` so siblings and the parent can call
  it.
- `checker/statements.rs` owns `check_program`, block scoping, and the
  `check_stmt` statement dispatcher, including generic comptime alias lowering
  (`check_generic_comptime_alias` fills the `Checker.comptime_aliases`
  registry of `ComptimeAlias` entries — classified `ParamDecl`s plus an
  `AliasBody`: a symbolic type template or, for a Bool-bodied predicate
  alias, a symbolic `GenericConstraint`; `check_program` pre-registers
  module-level aliases like struct shells). A local `comptime` binding whose
  value names a compile-time value in scope (`comptime lane = dt`,
  `Checker::names_value_binder`) is no generic alias: it binds as an alias
  of that parameter expression (`local_comptime_parameters`, and the
  block-scoped `Checker.comptime_dtypes` read through
  `Checker::comptime_dtype` for a `DType`). `check_def` checks one function declaration — the
  `StmtKind::Def` arm delegates to it, and lambda mode (`lambda = true`)
  applies the lambda-specific capture-default/thinness/diagnostic deltas.
- `checker/with_stmt.rs` owns the `with` statement: `check_with` classifies
  the manager into a `WithForm` (`context_manager_form`: consuming
  `__enter__`, plain and error-taking `__exit__` overloads, upstream's
  rejections), synthesizes the desugar (`Synth` node factory, whose node
  identities are `SyntaxId::derived` from the statement's, hidden
  `$with<id>_*` names) and checks it in a block scope, recording a
  `WithDesugar` (form and statements); `with_desugar` builds the same
  statements from a statement and a form without a check, for a derived
  instance (`template_facts/capture.rs:instance_with_desugars`); and
  `splice_with_desugars` replaces every checked `with` in the final tree
  before `explicit_destroy`; `KEEP_ALIVE_BUILTIN` names the
  `_mojito_keep_alive` liveness anchor the call inference accepts and MIR's
  `lower_stmt.rs` lowers to `KeepAlive`.
- `checker/bound_defaults.rs` owns the requirement defaults bound at a call
  through a trait bound: `record_bound_default_arguments` (called from the
  bound branch, `resolve_bound_method` in `method_calls/resolution.rs`)
  records, by the call's
  origin, each omitted requirement default some conformer's witness
  declares otherwise (`witnesses_default_alike`), the catalog keeps them
  across discovery rounds (`TemplateCatalog::bound_default_arguments`,
  `BoundDefaultArguments`), and `bind_bound_default_arguments` spells them
  as keyword arguments of the call and of every clone of it, before a pass
  and again, with another pass, when a pass found new ones
  (`check_program_carrying`). The spelled defaults are literals, or
  expressions over the method's value parameters: `requirement_default` in
  `checker/traits.rs` folds a requirement's default over module constants
  (`comptimes`, `comptime_literals`, both registered before the trait pass)
  when the trait is declared, keeping the parameters it reads
  (`reads_value_parameter`), and `spell_parameters` replaces them with the
  call's compile-time arguments. A generic call (`ExprKind::Invoke`) binds
  them as a plain method call does.
- `checker/inference.rs` owns expression inference (`infer`/`infer_impl`)
  and list/tuple/variant construction. `checker/template_string.rs` owns
  t-string typing: `infer_template_string` spells a `t"…"` as the call of
  `template_string_entry()` (`std.format.tstring`'s `__make_tstring`, by
  linked name from `module::linked_item_name`) under identities derived from
  the literal's, with the snapshot capture policy (a place that is not
  `ImplicitlyCopyable` wrapped in `String(...)`), and records
  `SemanticAdjustment::SpelledConstruction`. `infer_variant_storage_method`
  is the `__VariantStorage` primitive's operation dispatch (`isa`/`set` — both
  value and `init_with=` placement forms — `unwrap`/`unsafe_unwrap`,
  `replace`/`unsafe_replace`, and the consuming `deinit_with`), reachable only
  on a `Ty::Variant` receiver (the bundled `Variant`'s `_storage` field)
  through `infer_method_call`, which the parameterized `Invoke(Member)`
  spelling and the ordinary method call both dispatch to after the static
  receiver arms (a type-name receiver is never inferred as a value); the
  public API is
  `stdlib/std/utils/variant.mojo`, whose type-keyed methods specialize per
  call, and `type_keyed_projection` routes a `v[T]` subscript to the struct's
  `__getitem_param__[T]` clone in value and place positions (the
  `type_keyed_accessor_call` lowering in MIR). `check_lambda` runs a lambda
  expression's hidden definition through `check_def` during statement-root
  registration and caches the finalized function-value type under the
  expression span (the comprehension pattern); `ast::lambdas_in_expr`/
  `lambdas_in_stmt` are the shared lambda-discovery walkers used by the
  checker, `checked.rs`, and `mir/nested.rs`.
- `checker/indexing.rs` owns place validation, subscript/index inference and
  assignment (including keyword slices and the `BorrowViewResult` marking for
  view-typed slice results), pointer offset/write checks (the single-place
  rule and its multi-element interior-domain lift), the pointer-write
  capability (`pointer_write_capability`, the per-binding resolution of a
  symbolic origin binder — through a call result's recorded
  `call_result_origins` and an `ImmOrigin` cast's field path too — also the
  source of a dereference place's mutability in `origins/actuals.rs`, where
  `record_call_result_origins` resolves a callee's `ViewReturnOrigin`
  contract and `materialize_temporary_holder` gives a call-result holder
  its hidden slot), the positional
  String-slice rejection hint, and member access.
- `checker/method_calls.rs` (split across `method_calls/`) owns method-call
  inference. `mc_infer.rs` keeps `infer_method_call` as a sequence of stages
  over one `MethodCallSite`: `type_receivers.rs` types a receiver that
  spells a type, `intrinsic_receivers.rs` and `simd_receivers.rs` the
  receivers that answer without a declared signature (a string literal's
  `format` types there as the static call of the bundled stand-in
  `_FormatUtils.format_literal`, recorded as
  `SemanticAdjustment::LiteralFormat`, which
  `mir/lower_expr/expr_method.rs` lowers as that static `Call` with the
  literal first), `resolution.rs`
  selects a signature per receiver family (struct, bound, builtin) and
  retargets to a minted clone, `receiver_effects.rs` checks the receiver
  and argument conventions and builds the reference result, and
  `call_contract.rs` records the `CheckedCallContract`. `selection.rs`,
  `statics.rs`, and `builtin_types.rs` hold the scoring, static, and
  pointer/List helpers, and `mlir_op.rs` the one `__mlir_op` statement
  the bundled modules spell. `mc_infer.rs:infer_selected_method_call` is the
  tail every selected signature takes (clone retarget, effects, receiver and
  argument contracts, result). A member of `Tuple` or `TString` is an
  ordinary struct method of its declaration in `std/builtin/tuple.mojo` or
  `std/format/tstring.mojo`; no Rust surface types one. A method's named `out` result is read off
  `mojito-ast`'s `named_result` / `FnParam::is_named_result` by
  `declarations.rs:method_sig`, `check_method_inner`, and MIR's method
  lowering alike. `checker/initializer_list.rs` owns the
  initializer list: `infer_initializer_list` spells `{}` / `{a, b}` at the
  contextual type as that type's construction under an identity derived
  from the brace's, checks it, and records
  `SemanticAdjustment::SpelledConstruction` (`record_spelled_construction`,
  shared with t-strings), which `mojito-checked`'s arena builder answers at
  the sugar's location and `mojito-hir` (`substitute_spelled_constructions`)
  lowers in the sugar's place. Together they own method-call inference (including the inverted write:
  `x.write_to(writer)` on a bounded parameter, a builtin, or the
  nominal String records `SemanticAdjustment::InvertedWrite`, which
  `mir/lower_expr/expr_method.rs` lowers as the writer's host primitive
  `mojito-symbol`'s `WRITE_FORMATTED` (`$write_formatted`, the formatted
  text through `write_string`), and `x.write_repr_to(writer)` records
  `InvertedReprWrite`, lowered as the same primitive over `repr(x)`; a
  plain `writer.write(...)` resolves to the bundled `Writer` trait's
  declared `write` like any method; an instance method called through its type with
  the receiver as the first argument — `infer_type_receiver_instance_call`
  in `statics.rs` — records `ReceiverFromFirstArgument`, and MIR lowers the
  first argument as the receiver), overload scoring
  (score ties on receiver-overloaded methods break by the call's explicit `^`
  transfer), and static/pointer/uninit-storage/List/Tuple method inference
  (`infer_struct_static_method` dispatches statics on parameterized structs —
  explicit `TypeApply`/subscript-parsed receivers, whose origin slots
  partition out through `partition_struct_origin_args` as construction does,
  and bare receivers with struct parameters inferred from argument types via
  `resolve_use_params`; `finish_static_call` completes every static, on both
  the parametric and the bare non-parametric path — binding `ref [Self.o]`
  slots and explicit origins through the constructor's
  `bind_constructor_origins`/`record_constructor_reference_borrows`, retaining
  `mut`/`ref` argument places via `solve_call_origins`, and alias-checking;
  both method paths and the constructor paths record
  `checked::MethodInstantiation`s and retarget to an existing per-call
  clone through `specialized_method_clone`, whose value list must agree with
  the specializer's `method_request_values` (a retargeted static names the
  clone through `record_static_clone_target`); the method-call, static, and
  constructor paths also record every closed generic-struct application
  reached from a non-bundled source (`record_struct_instantiation` →
  `checked::StructInstantiation`) and retarget a closed receiver's call to
  its per-instantiation clone by exact name (`instance_method_clone`,
  `generics.rs`, over `symbol::instance_method_clone_name`); `operators.rs`,
  `indexing.rs` (`resolve_struct_setitem`), and `iteration.rs` (the
  `__iter__` prepare symbol) apply the same lookup, and
  `call_inference.rs::existing_def_clone` retargets an inferred bound-generic
  call to an already-declared def clone; `unify_through_callable_bounds`
  solves an infer-only type parameter through a callable-bounded sibling)
  (`infer_uninit_storage_method` types the compiler-private
  `__UninitStorage[T]` write/take/destroy crossings behind
  `MaybeUninit`).
- `checker/call_inference.rs` owns free-function and callable-value call
  inference (`infer_call`, generic-call instantiation).
- `checker/type_resolution.rs` resolves source annotations into checked `Ty`
  (builtin type-argument forms, dependent/associated projection, and generic
  comptime alias expansion via `resolve_comptime_alias`), partitions a struct
  application's origin slots out of its arguments
  (`partition_struct_origin_args`, the funnel shared by annotations and
  `infer_construction`; `resolve_storage_annotation_with_origins` reports the
  explicit origins a local's annotation demands), and resolves the bare
  `Pointer[T, _]` parameter placeholder as the immutable alias. It also owns pointer
  origin arguments (`pointer_origin_arg`/`pointer_origin_expr`: origin
  parameters, `origin_of(place)`, `._get_owned_interior["tag"]` projections
  in both annotation and expression shapes — the multi-element pointer
  marker — and the terminal conservative `._subtree` projection via
  `append_subtree`) and the annotation alias table (`StringSlice` →
  `StringSpan`, `MutPointer`/`ImmPointer`). `size_of_operand` validates the
  layout query shared by call inference and compile-time expression checking;
  `constraints::eval_associated_ct` keeps it as `ParamContext::size_of` in a
  SIMD width, and `types::substitute` replaces type binders inside the slots.
- `checker/traits.rs` owns trait/struct declaration checking, conformance
  (nominal and built-in), and type-capability queries (`is_deinitable`,
  `is_movable`, `is_copyable`, …). A type parameter is movable or
  destructible only where its bounds prove it (`bounds_prove_movable`,
  `bounds_prove_deinitable`), and `overload_support::builtin_trait_implies`
  is the one table of which built-in trait proves which, for an assumed
  conformance and for a `where` premise alike. Deprecated lifecycle spellings
  (`ImplicitlyDeletable`/`__del__`) normalize to the canonical
  `Deinitable`/`__deinit__` vocabulary via `ast::canonical_trait_name` /
  `ast::canonical_destructor_name`, applied by the parser at the semantic
  positions and by the checker where trait names are extracted from
  expressions.
- `checker/origins.rs` (split across `origins/{actuals,binders,construct,
  exclusivity,transfer,solve,sig,subst,interior,ref_params,result_alias}.rs`)
  owns origin/reference-handle derivation, constructor origin binding
  (`origins/construct.rs`:
  `bind_constructor_origins` and `bind_fieldwise_origins` bind a struct's
  origin binders from a `Pointer[Self.T, Self.origin]` argument, a
  `ref [Self.origin]` argument, or a struct-typed argument carrying the binder
  in its origin tail, check an explicitly applied origin, and return
  `ConstructorOriginBindings`, whose `substitute` rewrites the parameter types
  and whose `bind_tail` fills the constructed value's origin tail
  (`constructed_type`); `bind_callee_origins` does the same for a free
  callee's own binders), the argument exclusivity rule
  (`origins/exclusivity.rs`: `check_argument_origin_exclusivity` judges every
  pair of arguments — receiver included — by their own places and the origins
  their types carry, both the declared parameter type's and the bound one's,
  which alone shows an origin received through a type argument; an
  `ExclusivityCallee` names the callee and whether it is
  `@__unsafe_nested_origins_read_only`
  (`MethodSig::nested_origins_read_only`, or
  `Checker::nested_origins_read_only_functions` for a free function), an
  `ExclusivityReceiver` the receiver; `infer_pointer_method` runs the rule
  for `Pointer.unsafe_write`; reporting upstream's
  `AliasingArguments`), the return-tail check
  (`origins/solve.rs`: `reconcile_return_origin_tails` judges a returned
  struct's origin tail against the body's `return_annotations` entry resolved
  over the body's own places, so `origin_of(self.items)` stays a field, and
  rewrites a fitting tail to the signature origin the declared return names),
  the call-result tail (`origins/actuals.rs`:
  `bind_call_result_tail` writes the `call_result_origins` a view-returning
  call resolved into the result's origin tail),
  the annotation-demand verdict for locals (`check_storage_origin_demands`,
  replayed on reassignment from the per-binding
  `storage_origin_demand_scopes` table),
  interior and
  aggregate-origin tracking, capture-origin collection, origin-signature
  lowering (including the shared `SigOrigin` instantiation helpers
  `instantiate_sig_origin`/`instantiate_bound_origin` used by the iteration
  protocol and delegated-call expression-origin resolution,
  `lower_ref_sig_resolved`), and cross-call transfer effects (`abstract_body_origin`,
  `record_transfer_effect`, which a symbolic outward store reaches as a
  latent effect the frame keeps unpublished (`TransferFrame::record`), the
  `apply_transfer_effects` callable-keyed wrapper
  over the `replay_transfer_effects` core, value-position effect baking
  (`bake_value_transfer_effects`), the higher-order call-through channel
  (`record_call_through`, `apply_call_through_effects`,
  `translate_call_through`), and the span-keyed `CheckedCallTransfer`
  handoff to MIR, destination interior paths included), and the call-result
  aliasing rule (`origins/result_alias.rs`:
  `check_result_aliases_destination`, called from the `Assign` and `SetPlace`
  arms, rejects `D = f(args)` when a direct
  argument of the outermost call borrows an owned interior of `D`;
  `check_result_carries_container_interior` judges a nominal `__setitem__`
  store by the argument values' carried origins only; a
  view-returning method call projects its carried origins through the tags
  `MethodSig::view_return_interior` lowers from its return annotation, kept
  per call in the checker's `view_result_interiors` table and handed to MIR
  on `SemanticAdjustment::BorrowViewResult::interior`, where
  `aggregate_borrows` names them in an element receiver's loan, and
  `call_parameters` names each argument), plus its `mut self` twin
  (`check_mutable_receiver_carried_aliases`, called from
  `infer_method_call`: a read argument whose value borrows the receiver's
  storage, `s += s.rstrip()`, is rejected).
- `checker/scopes.rs` owns lexical scope, binding declaration/mutability, and
  nested-def capture-access checks (a compile-time binding of a validated
  body — a `comptime for` variable or a value parameter — captures nothing).
  It also owns binding-identity allocation: `reserve_owners` hands out
  identities from the cursor, or from the fresh region above every range a
  previous pass recorded once a body inferred again would cross the ceiling
  of the range it had (`owner_range_split` then refuses template capture).
- `checker/body_carry.rs` owns carrying a body's facts from one checker pass
  to the next. `PassCarry` is what `checker.rs:check_program_carrying`
  returns (the `DiscoveryResult`, the checker-internal stores, one
  `BodyRecord` per site, the identity watermark); `carried_stores!` lists
  every store a site is measured and copied over; `Checker::carry_body`
  serves a site whose record is clean and whose effect reads (recorded by
  `note_body_effect_read` beside each `effect_observations` write) are still
  current, and `enter_body_site`/`leave_body_site` bracket an inferred one
  (from `statements.rs:check_def` at module level and
  `declarations.rs:check_method_inner`). `def_syntax_hash`/`method_syntax_hash`
  are the syntax fingerprints a later round compares, and
  `PassCarry::for_next_round`/`sites` are the driver's dirtiness hooks
  (`compiler.rs:ServedRequests::dirty_sites`).
- `checker/comptime_validation.rs` owns source validation of compile-time
  control flow: `validate_comptime_templates_into` (in `checker.rs`) runs a
  checker in `source_validation` mode over the prepared program, lending it
  the compilation's template catalog (`validate_comptime_templates` is the
  verdict-only wrapper),
  where `check_comptime_condition` types a `comptime if` condition (a
  generic constraint, a concrete `conforms_to`, or a `Bool` value),
  `check_comptime_for` checks a loop body under its element type,
  `bind_local_comptime` binds function-local `comptime` aliases and
  compile-time-only values, `validates_body` draws the per-instantiation
  boundary (a body keyed on the `Hasher` wildcard vector binder is
  validated), and the body `value_keyed_def` names is validated, with no
  verdict where it cannot be typed (a `def` keyed on a
  `DType` binder or using a parameter as a lane width, which the executable
  pass checks again as a bound generic
  — `mojito_ast::simd_width::def_uses_layout_dependent_param`, shared with
  the elaborator, is the lane-width scan), and
  `validate_comptime_method_bodies` checks a method body
  `mojito_ast::simd_width::method_constructs_at_own_lane` names (one
  constructing a vector at its own binder's lane, stubbed by the
  elaborator) the same way,
  `conformance_arm_assumptions` collects what a `comptime if`'s
  `conforms_to` atoms prove for the arm `statements.rs:check_conditional`
  guards with them. A `DType`- or width-keyed body is
  validated with its lane symbolic: `annotations::dtype_from_arg` and
  `type_resolution::simd_width` resolve a binder in scope (bare, or `Self.x`
  through `constraints::self_param_value`) to a `SimdDtype::Expr`/
  `SimdWidth::Expr` slot, `annotations::simd_of` builds the type through
  `types::simd_ty_from_slots`, and the SIMD sites in `operators.rs`,
  `method_calls/simd_receivers.rs`, `indexing.rs`, and `builtins.rs` gate on
  `SimdDtype::licenses` and record each lane fact as its slots, known or
  symbolic (a shuffle, slice, or join only at a known width). `checker.rs`
  re-exports `validates_body` as `validates_comptime_body`, the body gate
  `explicit_destroy::check` reuses for its `DestroyScope::ValidatedTemplates`
  run. The same page owns a pack that is still a parameter:
  `pack_reference`/`unbound_pack_named` resolve it (scopes in
  `Checker::pack_params`, `annotations::pack_scope`), `pack_element_type`
  builds the dependent element (`ParamContext::list_get`,
  `DependentType::pack_element` in `mojito-types`), `opaque_element` is its
  bounded `Ty::Param` view and `restore_pack_elements` the way back,
  `infer_pack_element_construction` types an element's `Ts[i]()` and, over
  a symbolic pack, records `SemanticAdjustment::ConstructPackElement`, which
  `lower_expr/expr_call.rs:pack_element_construction` lowers as
  `MirInstr::ConstructTypeParam` with its `element` index (a reflected
  field type's `types[i]()` or `FT()`, typed by
  `checker/reflection.rs:construct_dependent`, records
  `SemanticAdjustment::ConstructType` over the type expression instead,
  which `Flatten::lower_call` lowers as `MirInstr::ConstructType`, a
  template-only form the concrete verifier rejects) and
  `native::mono`'s `default_construct_parameters` (`mono/substitute.rs`,
  through `constructed_type` and `default_construction`, beside the
  unroller's per-copy call) writes as the element's default construction (each
  instance's construction is `comptime/rewrite.rs:pack_element_construction`,
  reached for a `def` through `fold_pack_uses`; `Tuple.__init__(out self)`
  writes it through `Pointer(to=self[i]).unsafe_write({})`, the initializer
  list `initializer_list.rs:infer_initializer_list` spells as the element's
  construction, after the `lit.ownership.mark_initialized` statement
  `method_calls/mlir_op.rs:infer_mlir_op` records as
  `SemanticAdjustment::MarkInitialized`, which `lower_expr/expr_method.rs`
  lowers as `MirInstr::MarkInitialized` and `analysis/moves.rs` defines in
  the place-tree flow `store_drops.rs` replays; the consuming members
  (`reverse`, `concat`, `consume_elements`) move each element out with
  `Pointer(to=self[i]).unsafe_take_pointee()` and end the storage with
  `lit.ownership.mark_destroyed`, `SemanticAdjustment::MarkDestroyed`
  lowered as `MirInstr::MarkDestroyed`, which `analysis/moves.rs` moves,
  `field_drops.rs` counts as the field's transfer, the VM tombstones, and
  Pliron's `lower/drops.rs:clear_presence` clears the leaf flag for; an instance derives a
  `print` argument or a local's value through
  `template_facts/realization_folds.rs:element_construction_facts`, a copy with no folded
  index taking its loop index from `constructed_element_indices`, which
  `index_indifferent` lets pick among elements sharing the constructed type),
  `forwarded_pack`/`forwarded_pack_argument` recognize a spread of it as one
  call argument (its placement through `call.rs:spread_position` and
  `bind_spread`) and `bind_forwarded_pack` binds it whole to a callee's pack
  collector (bounds, ownership, and the recorded `owned_packs`/
  `owned_collectors`), at the overflow loops of
  `call_inference.rs:infer_generic_call`, `generics.rs:instantiate_method_generics`,
  `method_calls/selection.rs:score_method_call`, and `builtins.rs:infer_print`
  (`rebind.rs:rebinds_by_value` keeps a validated template's selection in a
  clone through the transfer frame `template_facts.rs:mark_symbolic_selection`
  marks),
  `close_pack_elements` and `positional_pack_binding` bind a callee's pack at
  a use (`types::expand_pack_spread` for a spread such as `other: Self`),
  `infer_unbound_pack_construction` types the private `__RuntimeTuple`
  over a spread of an unbound pack and
  `infer_validated_variadic_construction` a public `Tuple` (every other
  variadic struct, and every construction spreading a forwarded pack, is
  matched against its declared constructor by
  `declarations.rs:infer_construction`),
  `reject_mixed_spread` refuses `Tuple[Int, *Self.Ts]`, and `symbolic_verdict`
  turns a `TypeError::SymbolicBoundary` into that one body's no-verdict.
  `types::pack_spread` owns the spread convention.
- `checker/reflection.rs` owns `reflect[T]` under the checker.
  `eval_reflection` answers a query from the struct table for a registered
  struct, through `ReflectQuery::answer` (`mojito-types`, `param_expr.rs`,
  the one closed-subject policy the elaborator shares), and as a
  `ParamKind::Reflect` node (`ParamContext::reflect_query`) for a subject
  still a parameter; `infer_reflection` types a query read as a value
  (hooked at the top of `inference.rs:infer_impl`) and records its answer
  as `SemanticAdjustment::ParamValue` (`record_reflection_value`), which MIR
  lowers as `Const::Param`; `eval_reflection_expr` serves
  `constraints.rs:eval_associated_ct` and `compile_dependent_ct_expr`, and
  `reflected_type_operand` / `reflected_type_annotation` resolve `types[i]`,
  `r.field_at[i].T`, and `r.field["x"].T` in `comptime_type_operand` and
  `type_resolution.rs:ty_from_anno` to the dependent element (a `ListGet`
  over the `field_types()` node, viewed through `opaque_element`).
  Type-level pack algebra lives beside it: `constraints.rs:typelist_receiver`
  reads a pack, a `TypeList` construction, a `.reverse()`, and a
  `TypeList._concat[A.values, B.values]()` as a `TypeListReceiver`
  (`Derived` for a list computed from packs still open),
  `type_list_operand` gives the `ParamExpr` a spread's operand denotes, and
  `comptime_validation.rs:type_list_element` its `[i]`. The lists are
  `mojito-types`' `ParamKind::ListTabulate` and `ParamKind::ListConcat`
  (`ParamContext::list_tabulate`, `list_concat`, `list_reverse`,
  `list_length`, `list_elements`, `type_list`); a spread of one is the sole
  argument of its struct type (`types.rs:list_spread_argument`,
  `spread_arguments`, `expand_pack_spread`), which
  `type_resolution.rs:tuple_element_types` and
  `declarations.rs:spread_pack_values` build,
  `generics.rs:expand_solved_packs` closes at a call, and
  `native::mono`'s `substitute_ty` closes per instance. The cloner's twins
  over source types are `comptime/packs.rs:type_list_source_types`,
  `bind_type_list_packs`, and `expand_spread_argument`, and
  `comptime/eval.rs` reverses and concatenates closed `TypeList` values. The
  elaborator's `comptime/eval.rs` answers the same queries per instance;
  `template_facts/certificate.rs:template_certificate` keeps every reflection-reading
  body's instances on the clone check (`reads_reflection`).
- `checker/template_facts.rs` owns checked templates on the checker side:
  the shared types (`BodySite`, `Occurrence`, `BodyShape`, `GrammarNotes`),
  the helpers more than one seam uses, and the body entry points. The rest
  is one file per seam under `checker/template_facts/`:
  - `capture.rs` — `capture_body_facts`, `body_transfers`,
    `occurrences_over`, `record_template`, `retain_template`, `span_table`.
  - `certificate.rs` — `template_certificate`, `method_certificate`,
    `validated_struct_binders`, `FUNCTION_FEATURES`, and the binder
    predicates they read.
  - `grammar.rs` — the `BodyShape` core walk (`statement`, `expression`,
    `whole_value`, the local and receiver predicates). Each certificate
    class's productions are a `grammar_*.rs` beside it: `grammar_calls.rs`,
    `grammar_builtins.rs`, `grammar_constructions.rs`,
    `grammar_control.rs`, `grammar_operators.rs`, `grammar_packs.rs`,
    `grammar_references.rs`, `grammar_simd.rs`, `grammar_stores.rs`.
  - `realization.rs` — `derivable_facts`, `derive`,
    `instance_substitution`, `realize_instance_facts`, `realize_transfers`,
    `substituted_facts`. `realization_calls.rs` holds the per-call
    `realize_*` recipes (method, static, direct, callable, operator,
    conversion, `print`/`repr`/`len`), and `realization_folds.rs` what the
    elaborator folds per instance (pack elements, folded literals and
    arithmetic, `SIMD` lanes).
  - `install.rs` — `install_body_facts`, `replace_body_facts`,
    `install_transfers`, `remove_occurrence_facts`.
  - `verify.rs` — `capturable`, `census`, `transfer_residue`, and the
    unkeyed-store and hash-leaf growth checks.
  `Checker::check_def_body` is every `def` body's entry and
  `check_method_body` every struct method's (from
  `declarations.rs:bind_and_check_method`, under `Checker::method_site`); both
  build a `BodySite` for `check_body`, which serves a clone
  from a certified template (`derivable_facts`, `realize_instance_facts`,
  `install_body_facts`), otherwise infers the body, and retains a
  module-level generic body's facts (`capture_body_facts`,
  `template_certificate` — whose `FUNCTION_FEATURES` allowlist bounds a
  `FunctionBody` — `method_certificate`, `record_template`).
  `BodyShape::simd_intrinsic` admits the lane reads of a closed or
  lane-shaped vector, `instance_substitution` folds a wildcard vector
  binder's hidden slots, and
  `realize_simd_intrinsics` records each instance's reinterpretation and
  lane-count shapes. `BodyShape::struct_lane_simd` admits a `DType`-keyed
  struct's symbolic lane (`Scalar[Self.dtype]`) as a scalar, and
  `realize_lane_literals` materializes a literal beside such a lane where an
  instance folds it to `Int` or `Float64`. `BodyShape::lane_float_method`
  admits a float lane's rounding dunder or `__fma__` on such values, and
  `realize_lane_float_methods` records the borrows the native `Float64`'s
  method takes. `BodyShape::lane_comparison` admits a comparison of such
  values as a condition or `Bool(...)`'s argument, and
  `realize_lane_comparisons` re-types its mask to `Bool` where the lane is
  native. `BodyShape::struct_vector` admits a
  closed vector binder read as `Self.key`, whose clone construction
  `fold_vector_values` tells from the template's syntax and
  `construct_folded_vectors` records; `operators.rs:vector_alias` types a
  vector alias's call (`U256(...)`) as the `SIMD` it spells.
  `realize_method_call` retargets a closed method call to the clone member
  `declarations.rs:method_clone_target` finds (the helper
  `constructor_clone_target` shares; its `realize_method_contract` half also
  realizes an element store's embedded value getter,
  `realize_element_getters`, and `realize_element_dunders` re-selects an
  embedded in-place dunder dispatched through a bound,
  `bound_dispatch.rs:realize_embedded_dispatch`; `realize_inplace_updates`
  realizes an augmented assignment's in-place dunder kept at its place in
  the `inplace_updates` table, through the same two helpers, and
  `install_body_facts` writes it back as the `AugmentedInPlace` adjustment),
  `realize_builtin_len` takes the `len`
  witness, `realize_operator` repeats an operator's type-driven dispatch
  (through `operators.rs:struct_infix_dispatch`, the type-level half of
  `infer_infix`, `operators.rs:struct_reflected_dispatch`, its reflected
  half, and `operators.rs:scalar_operator_result`, its primitive half), `plain_data` is the instance-argument obligation of a
  `MethodBody` and `loan_free` its transfer obligation (`body_transfers`
  keeps each replayed transfer by template owner with its source's binding
  type, `transfer_residue` tells a replay from a residue no recipe covers,
  `realize_transfers` replays them for an instance, and
  `install_transfers` installs them), `residue_plain` its call-through
  obligation (a callee's
  residue is read as `EffectRead::CallThrough`, kept as
  `call_through_reads`, and rekeyed by `note_realized_callee`;
  `realize_callable_call` takes a call through a `def(...)` parameter from
  the instance's own binding of it), `realize_conversion` re-selects an
  implicit conversion from the substituted source and target types while
  `install_conversions` writes the four conversion tables back and
  `realize_repr_call` re-proves `repr`'s argument `Writable`, and `census`
  reports what keeps each
  generic body from being
  captured, with `grammar_features` naming the constructs it holds. The
  submodule `template_facts/bound_dispatch.rs` re-selects a call through a
  bound for an instance: `bound_witness` is the by-types resolver from a
  receiver type, its convention, a method name, and the recorded argument
  types to the requirement's witness (a place read, a hashed leaf, or a
  struct's own method with its binders bound, of an overload set the one
  member witnessing the requirement (`traits.rs:requirement_witnesses`),
  and retargeted to a baked binder's per-call clone, keyed by the
  instance too when the struct is generic), with
  `witness_binders` judging one declaration, `realize_bound_dispatch` rewrites the
  abstract contract with it (a receiver typed by a binder the instance keeps
  stays a dispatch, reading the summaries of every conformer
  `mc_infer.rs:dispatch_conformers` names) (and marks a witness that is a named `deinit self`
  destructor as an explicit-destroy call, refusing a copied receiver whose
  witness does not consume it), `realize_inverted_writes` turns an inverted
  `write_to` on a struct instance into that call, and
  `realize_bound_builtin` re-proves a `hasher.update`/`writer.write`
  argument's bound. The submodule `template_facts/constructions.rs`
  re-selects a struct construction's constructor for an instance
  (`realize_construction`): the template's member must still bind every
  recorded argument type exactly (`exact_binding`, over
  `call.rs:match_call_slots`; a fieldwise struct's fields likewise, over
  `match_fieldwise_slots`), and
  the target becomes the instance's clone of it through
  `declarations.rs:constructor_clone_target`. The submodule
  `template_facts/iterations.rs` keeps a runtime `for`'s protocol as the
  inputs it is selected from (`captured_iterations`, a `TemplateIteration`
  proven by rebuilding the recorded protocol), selects it again from an
  instance's substituted iterable type (`realize_iterations`), and resolves
  it against the instance's own source binding (`install_iterations`), all
  through `iteration.rs:loop_site_protocol`, the path the `for` statement
  and comprehensions share. The submodule `template_facts/comprehensions.rs`
  keeps each comprehension binder as its owner and its clause's iterable
  (`captured_comprehension_bindings`, a `TemplateComprehensionBinding`
  proven against the recorded binding and the checker-only
  `comprehension_iterables` table `inference.rs:check_comprehension`
  fills), and declares it again for an instance from the protocol
  installed at that iterable (`install_comprehension_bindings`). The
  submodule `template_facts/tuple_unpacks.rs`
  keeps a tuple unpacking's plan as the value's type, the reference the
  unpacked place yields, and the named targets (`captured_tuple_unpacks`, a
  `TemplateTupleUnpack` proven by rebuilding the recorded plan), and builds
  the plan again for an instance (`realize_tuple_unpacks`,
  `install_tuple_unpacks`) through `statements.rs:tuple_unpack_plan`, the
  path the unpack statement itself takes. A `with` statement's form
  (`WithForm`, the `WithDesugars` table's recipe) is kept at the statement;
  `instance_with_desugars` builds an instance's desugars from it before its
  occurrences are walked (`occurrences_over`, which reads each desugar in its
  statement's place), and installation hands them to the final splice. A
  nested `def` has no derivation recipe: a body holding one is served by its
  template, and a clone kept for another reason infers it. A capturing
  callable's environment is kept like a struct's origin slots
  (`map_struct_origins`). A retained struct type that
  names a binding in an origin argument is kept by template owner
  (`unbound_struct_origins`/`bind_struct_origins` over `map_struct_origins`,
  the bundle's `typed_origins`: only a slot naming a binding is unbound and
  refilled, so a slot an instance's argument brings in, such as a clone
  binder, stays as it stands; a pointer to a place, `Pointer(to=self.items)`,
  keeps its provenance the same way, first, flagged by `TypedOrigins::pointer`:
  `typed_origins`/`bind_typed_origins`), and the return annotation an inference
  re-resolves at each `return` is resolved once for a derived instance
  (`annotation_spans`, `grew_outside_body`). `struct_application_frames`, pushed in
  `generics.rs:record_struct_instantiation`, is how a template keeps the
  struct applications its instances must request. `span_table` maps every
  `FactTable` onto the checker's storage, `BodyShape` is the grammar of the
  derivation classes (`method_direct_calls` the direct calls a method body
  may make), and `overload_rebinding_only` is the one difference
  verification mode accepts. `note_effect_query` records the callee effect
  summaries a body read, which are its dependencies. The vocabulary
  (`CheckedTemplate`, `CheckedBodyFacts`, `TemplateCatalog`, `InstanceTrace`,
  `OccurrenceId`, `TemplateObligation`, `MethodFeatures`, the template-local
  `TemplateCallContract` with `closed_method_contract` and its consuming
  siblings `consuming_method_contract` (through a bound) and
  `consuming_nominal_contract` (a nominal receiver's `^` transfer, or a
  place the call copies, `BodyShape::copied_consuming_call`), and the
  exhaustive
  `derive_adjustment`) is `crates/mojito-checked/src/templates.rs`. The
  declaration-level trace is `comptime.rs`'s `DefInstanceTrace` (type,
  value, and pack bindings), recorded by `specialize.rs:generate_def_spec`,
  and `MethodInstanceTrace`, recorded by `generate_instance_clones` and by
  `per_call_method_clones` (`trace_per_call_clone`) for a per-call clone;
  `GeneratedDeclarations` lists what an elaboration generated; the
  occurrence-level trace is `ast.rs:rekey_syntax`'s `SyntaxOrigins` (which
  traces a `mojito-common` `token.rs:SyntaxId::derived` node through its
  parent) plus
  `comptime.rs:rebuilt` and the identity a folded `comptime for` variable
  or pack `TypeList` use keeps (`rewrite.rs:rewrite_expr`). A pack struct's
  `__RuntimeTuple(*args^)` initializer, which each instance writes as
  `args^`, is laid over the instance by `template_facts/realization_folds.rs:relocate_packs`
  from the template's `PackRelocation`; a user struct's or `TString`'s
  `Tuple(*args^)`, which each instance expands per element with
  `mojito-checked` `templates.rs:PackElementNode` identities, by
  `template_facts/realization_folds.rs:spread_packs` from its `PackSpread`. `mojito-comptime`'s
  `comptime.rs:instance_traces` carries one to the other. A variadic struct's
  member instance substitutes per copy through `mojito-types`' `types.rs:substitute_packs`.
  The design record is `docs/notes/instantiation-from-template.md`.
  Where each instance obligation goes once clones are gone is
  `docs/notes/generator-contract.md`.
- `checked.rs`'s `DiscoveryResult` is what a discovery round's check returns
  (`checker.rs:check_program_for_discovery`, inside the `PassCarry` of
  `check_program_carrying`): `CheckedProgram::new`'s inputs, owned, with
  `scan_expressions` for the request collectors and `finalize` for the round
  that converges. Its tables are `fact_store.rs`'s logged stores.
- `fact_store.rs` owns `FactMap`, `FactSet`, and `FactVec`: a `HashMap`,
  `HashSet`, or `Vec` that logs every key written, read through `Deref`,
  with `mark`/`logged` for a range of the log. Every checker fact store is
  one, so `checker/body_carry.rs` can copy exactly what a body wrote.
- `explicit_destroy.rs` owns the explicit-destruction analysis, run from
  `checker.rs`'s `run_explicit_destroy` twice: once over the elaborated
  program (`DestroyScope::Program`) and once over source validation's
  symbolic template bodies (`DestroyScope::ValidatedTemplates`), where
  `check_comptime_if` joins the arms of a condition naming a parameter as
  branches (`join_parametric`) and the arms of any other condition as
  alternatives (`join_comptime`). `LINEAR_TYPE_PARAMETER` (`$linear`) keys
  the obligation of a value typed by a non-`Deinitable` type parameter;
  `Env.linear_temporaries` carries the call results the checker recorded as
  owned-but-unconsumed (`places.rs`'s `record_linear_temporary` and
  `record_unconsumed_temporary`, intersected in `run_explicit_destroy`),
  reported as `'(expression temporary)'`. A write through a reference (a
  `ref` binding, a reference-returning call) owns no obligation of its own,
  so the checker rejects one over a non-`Deinitable` referent at the store
  (`places.rs`'s `check_overwritten_referent`)). A `^` whose source is rooted at an
  immutable value binding is rejected where the transfer is inferred
  (`places.rs`'s `check_transfer_source`, `TypeError::ImmutableTransfer`).
- `mojito-types`' `types::is_symbolic`/`ct_value_is_symbolic` answer whether a
  type still mentions something only an instantiation can resolve. MIR's
  `lower_expr/expr.rs` uses it to turn `size_of` of an unspecialized type into
  `MirInstr::Unsupported`, and `mojito-vm`'s abstract-call adapters use it
  where `vm_type_is_symbolic` used to. Its neighbour `types::has_free_parameters`
  answers the different *closedness* question `ParamContext::type_shape` turns
  on: a generic callable value binds the parameters its own signature
  mentions, so `def[T](T) -> T` folds to a compile-time type constant instead
  of staying an open shape.
- `checker/rebind.rs` owns `rebind[Dest](value)`: `erase_rebinds` replaces
  each well-formed call by its operand before checking and records the
  retyping in `Checker.rebind_targets`; `apply_rebind_target` (from
  `infer`, the `ref` binding, and `check_place`) takes `Dest` on faith under
  validation or where either side is symbolic and demands equality between
  closed types, recording each judged rebind in `Checker.rebind_assertions`,
  which `CheckedProgram.rebind_assertions` hands to MIR as
  `SemanticAdjustment::Rebind`; `check_rebind_place`
  rejects writing through a by-value (`TrivialRegisterPassable`) rebind,
  outside `$`-mangled clones. The eraser turns `rebind[Dest](x) = value`
  into the plain `Assign` of `x`, whose retyping `rebind_assignment_target`
  applies from the `Assign` arm and `record_assignment_rebind` records,
  reversed, on the assigned value. MIR lowering reads a rebound operand
  through `Flatten::rebind_value` (`MirInstr::Rebind`) or
  `Flatten::rebound_place` (the place's terminal type), both in
  `lower_expr/entry.rs`, and `native::mono`'s `discharge_rebinds`
  (`mono/rebind.rs`) judges and erases them per instance. `RebindTargets::target_spans` names the
  spans a body's erased targets embed, which template capture tolerates as
  it does a return annotation's. The parser admits the call as an
  assignment and augmented-assignment target (`parser/stmts.rs`).
  `rebind_keyed_bodies`, scanned before the erasure removes the calls, names
  the bodies source validation must check for this reason
  (`Checker.rebind_keyed_bodies`, read through `body_keys_rebind` by
  `validates_body` and `explicit_destroy::walks_body`); the elaborator's twin
  is `comptime::block_has_rebind`, which keys a compile-time evaluation's
  method on it.
- `checker/constraints.rs` owns compile-time evaluation and generic-constraint
  compilation/evaluation. `compile_where_clause` compiles a clause,
  `bind_constraint`/`bind_declared_constraints`/`compile_condition` bind its
  operands to their binders (`GenericConstraint::bind`), and
  `ConstraintEnvironment` is the identity-keyed argument environment every
  verdict reads. Its evaluators resolve names and delegate operator
  semantics to `param_expr::fold`; `compile_dependent_ct_expr` builds a
  `ParamExpr` through the compilation's `ParamContext`
  (`Checker::param_context`, handed over by `TemplateCatalog::param_context`),
  and `value_parameter_in_scope`/`push_param_scope` resolve a bare value
  parameter to its owned reference (`annotations.rs`: `binder_owner`,
  `Checker::method_binder_owner` over `symbol::MethodBinderOwners`, which
  the elaborator's `elaborated_binder` shares,
  `value_parameter`, `params_as_args` over `ParamDecl::own_argument`).
  The same table's `call_qualifier` names the overload a per-call method
  clone request selects (the qualifier of the symbol `lowered_method_name`
  declares, which a call records verbatim): the elaborator mints only that
  overload's clone (`MethodSpecializationRequest::selects`), and the checker
  retargets a call only to a clone family holding it
  (`Checker::clone_serves_overload`).
  `constraint_verdict`/`constraint_proposition` are the three-valued
  evaluator, `assume_declared_propositions`/`assumptions_prove` the evidence
  an enclosing `where` supplies, and `eval_generic_constraint` its
  `is_proven()` projection. `compile_where_clause` retains an optional source
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
  and iterator-protocol selection respectively. `iteration.rs` accepts only a
  `StopIteration`-raising `__next__` (upstream's `__has_next__` rejection for a
  nonraising one) and records `IterationProtocol::source_retained`, which HIR
  (`hir.rs` loop lowering) and MIR (`lower_stmt.rs` `GetIter`,
  `lower_expr/ctrl.rs` comprehensions) read to decide whether a loop keeps its
  source alive and loaned.
- `checker/calls.rs` adapts neutral call matching to `TypeError` and validates
  checker-only signature rules.
- `checker/places.rs` owns call-site place classification and alias rejection
  (the pointer keyword subscript `p[unsafe_offset=i]` is a place:
  `pointer_offset_keyword_subscript`), and how a selected call's read
  parameters bind their arguments (`record_argument_borrows`: the borrowable
  place reads and the owned temporaries the caller destroys after the call,
  `read_temporary_arguments`; the arguments a read `*args` collector gathers
  bind the same way).
- `checker/generics.rs` owns unification, substitution, and callable/method
  specialization; `solve_value_args` solves value binders from argument
  types, a vector's width slot solving only a binder declared `SIMDLength`
  (`Checker::simd_length_binders`, recorded by `classify_params`);
  `substitute_variadic_at` types a member's struct-pack collector
  (`*b: *Self.Ts`) at a receiver as the runtime pack of its elements, a
  forwarded caller pack retyped with the struct pack's bounds.
- `checker/declarations.rs` owns parameter classification and method/function
  signature and body checking.
- `checker/annotations.rs` converts AST annotations into checked `Ty` values.
- `checker/builtins.rs` owns built-in typing/coercion rules and builtin
  free-function inference (`print`/`len`/`range`/…), and the `Hasher`
  requirement `_update_with_simd` as a call through an `H: Hasher` bound
  sees it (`hasher_simd_update_requirement`, generic over its dtype and
  width binders).

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
- `mir/text/parse.rs` (split across
  `parse/{reader,decls,instrs,operands,types,origins,prims}.rs`) owns UTF-8
  validation, the recoverable spanned schema
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
- `mir/lower_expr.rs` (split across
  `lower_expr/{entry,ctrl,expr,expr_call,expr_method,expr_access,calls}.rs`)
  owns expression lowering (the `expr_unconverted`
  dispatcher, collections/comprehensions, nested closures, the
  field-invocation indirect-call branch). `expr_unconverted` in `expr.rs` is
  a dispatch table over `ExprKind`: each arm calls one `Flatten` method.
  `expr.rs` keeps the variable-read, operator, and literal-aggregate arms
  (a t-string reaching it is an explicit `Unsupported` boundary); `expr_call.rs` owns direct calls (`call_expr`,
  `direct_call`) and callable-value invocations (`invoke_expr`,
  `variant_operation`, `parameterized_method_call`); `expr_method.rs` owns
  method calls (`method_call_expr`, its value and storage special forms,
  `ordinary_method_call`, `type_receiver_name`); `expr_access.rs` owns
  member, subscript, slice, and variant-projection reads. The module
  installs merged caller-side
  `EstablishLoans` — domain-keyed for interior-precise destinations — for
  checked call-transfer records (`install_call_transfers`) after free,
  method, indirect, and nested calls.
- `mir/lower_stmt.rs` owns statement, place, subscript-assignment, `try`-region,
  and terminator lowering, plus the borrowed-iteration source binding and loan
  re-establishment helpers shared with comprehension lowering.
- `mir/nested.rs` owns capture analysis and nested-function lifting; each
  lifted declaration names the declaration it is nested in
  (`MirFunctionDeclaration::enclosing`, set by `lower_nested_node`), and the
  nested frame declares its enclosing callable and type binders' locals
  (`Enclosing`). `verify/scope.rs` (`Scope::of_function`) walks that chain
  for the binders a body may name; `native::mono`
  (`Specializer::instantiate_nested_bodies`, `nested_bindings`,
  `binder_scope`) instantiates a nested body under its enclosing instance;
  the erased VM carries the enclosing values on the closure
  (`Value::Closure::parameters`, `Prog::inherited_parameters`).

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
  VM-display `mjrt_fmt_f64` formatter, and `mjrt_trap` (`mjrt_fmt_i64`,
  `mjrt_fmt_u64`, and `mjrt_repr_string` are dead rows awaiting the batched
  ABI bump: integer display and a String's `repr` are bundled Mojo now). It must never depend
  on the `mojito` crate (the VM `Value` stays out of the ABI);
  `tests/native_abi_test.rs` pins the Rust-side agreement.
- `backend/pliron.rs` (feature `backend-pliron`) owns the supported native
  backend: `compile` orchestration (reachable closure, verify, mem2reg/DCE,
  canonical text), `NativeModule` emission/JIT entry points,
  `runtime_declarations` (the contract table's LLVM rendering), `JitValue`,
  and the `TrapCategory` exit-code/VM-message contract (`OptLevel`/
  `EmitKind`/`NativeTarget` re-export from `native::target`). Its
  submodules: `backend/pliron/lower.rs` (MIR-to-LLVM-dialect lowering,
  itself split by instruction domain across the `backend/pliron/lower/`
  submodules —
  scalar operators/conversions with keyword/default call binding via
  `call::match_call_slots`, trap guard blocks, the sanitized `MIN // -1`
  divisor, aggregates/strings/allocation, Stage 4's
  tagged-outcome raising ABI, structural `try`/`finally` flattening with
  per-variable initialization flags and pending-outcome dispatch, references
  as place addresses, Variant tag/payload operations with dynamic payload
  destruction, fixed-vector multi-lane SIMD lowering over lane-aligned
  storage, compiled user
  `__moveinit__` transfers and residual-field destructor ownership,
  `mjrt_trace` lifecycle emission under
  `CompileOptions::trace_lifecycle`, and the exe wrapper that references
  `mjrt_version` and consumes a raising entry's outcome),
  `backend/pliron/emit.rs` (target stamping onto every LLVM module, LLVM
  IR/bitcode/object/exe via clang `--target`, runtime-archive discovery and
  linking, plus the `opt`-subprocess release pipeline),
  `backend/pliron/pipeline.rs` (the profile-to-pipeline table owning
  optimization policy: the shared pliron cleanup stage and each profile's
  LLVM pipeline selection, snapshot-pinned),
  `backend/pliron/toolchain.rs` (`ResolvedToolchain`: PATH-resolved
  absolute clang/`opt` paths version-checked against the LLVM 23.1 pin,
  ordered runtime-archive discovery — `--runtime-lib` via
  `set_runtime_override`, `MOJITO_RUNTIME_LIB`, installation bundle,
  development tree — with provenance/sha256/embedded-ABI-version
  validation, the `toolchain_report` behind `--print-toolchain`, and the
  fail-fast `check_toolchain` CLI front door),
  `backend/pliron/artifact.rs` (`write_atomic`: failure-atomic
  temp-and-rename artifact writes that preserve existing outputs),
  `backend/pliron/debug.rs` (`DebugTable` harvested from the cleaned
  module + the llvm-sys DIBuilder attach over the reparsed LLVM module:
  DWARF subprograms and call-granular line locations with a per-function
  count assertion that degrades to subprogram-only rather than mislabel;
  corpus test pins zero degradations),
  `backend/pliron/inspect.rs` (pure-Rust `object`-crate ELF inspection:
  per-section digests and diffs for reproducibility failures, symbol
  surfaces, machine/PIE/exec-stack/DT_NEEDED facts, byte scans),
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
  dispatch loop (`call_frame`/`make_frame`/`prepare_direct_call`); `make_frame`
  binds a method frame's receiver value parameters among its reified ones,
  which `exec.rs` closes a symbolic lane by (`vm.rs`'s `erased_closed_ty`).
- `backend/vm/references.rs` owns runtime reference handle read/write/projection,
  including `place_crosses_reference`/`place_handle`, which decide whether a
  place reaches a stored handle at or below its root and must therefore be
  accessed through the handle walk rather than frame storage.
- `backend/vm/exec.rs` owns the `exec_instr` instruction dispatcher and
  `try`-region execution.
- `backend/vm/calls.rs` turns `CallSlots` into runtime values and frame slots.
- `backend/vm/places.rs` navigates projected runtime storage, including the
  `UninitPayload` projection into inline uninit storage (a final payload store
  initializes-or-overwrites raw; reads trap while uninitialized).
- `backend/vm/values.rs` owns operator application, place stores,
  construction, String and StringSpan literal materialization, cloning, and
  moves.
- `backend/vm/adapters.rs` owns checked-result adapters and dunder-backed
  index loads.
- `backend/vm/invoke.rs` owns writeback/synchronous call machinery, argument
  binding, kwargs collection, and method dispatch.
- `backend/vm/dispatch.rs` owns named-call dispatch (`print` with its
  `sep`/`end`/`flush`/`file` keywords through `descriptor_value` and
  `libc.rs`'s `host_write_bytes`), drops, slice bounds, and value formatting.
- `builtins.rs` (`mojito_vm::builtins`) names what that dispatch answers
  without a program function: `BUILTIN_CALLEES`, `INTRINSIC_METHODS`, and
  `TRAIT_DISPATCH_PREFIX`; its tests pin each name to the dispatch source. A
  consumer deciding whether a call target resolves (the A1 legality rule)
  reads these tables.
- `backend/vm/libc.rs` owns the `external_call` libc table: `HostState`
  (descriptors, directory streams, the `errno` allocation, the environment
  overlay) and the per-callee marshaling between VM values and Rust's
  standard library.
- `comptime.rs` owns the staged entry points (`prepare` normalizes
  declarations without selecting or cloning — among them each conformer's
  inherited trait defaults, through `checker::expand_trait_defaults`, `elaborate_prepared` is the
  already-validated request-driven route the driver re-elaborates each
  discovery round, its `ElaborationInputs::new` requiring the catalog that
  holds validation's verdict, `elaborate` composes prepare → validate → elaborate for
  the stage seam), the `Elab` elaboration driver (`block`/`stmt`; its
  `keep_template_comptime_if` keeps a `comptime if` over a generic `def`'s
  own binders (`Elab::template_binders`) for the check, where every other
  one is selected), type
  resolution, the template classifications (`bound_generic_template_names`,
  `pack_generic_template_names` for type-pack defs whose non-evident calls
  specialize from checker-recorded instantiations, and
  `comptime_generic_template_names` for defs keyed only by a `comptime
  if`/`for` body, whose inferred calls do the same, gated by
  `omits_required_param` in `comptime/mono.rs`; a `DType`- or lane-keyed
  def is in none of them unless its body keys a clone — each
  classification is a per-declaration predicate
  (`comptime_keyed_declaration`, `pack_keyed_declaration`) that an
  overloaded name admits one declaration at
  a time, so one family may hold two classes, or two type packs, and
  `Elab::family_declaration` picks the declaration a request selected by its
  parameter names, parameter types, and `symbol::VariadicKey`
  (`recorded_overloads` in `comptime.rs`, shared with
  `unserved_template_parameter`, whose `RecordedKeys` carry the three keys),
  `seed_family_selections` (`comptime/specialize.rs`) records the
  checker's selection at an unclosed family call
  (`ElaborationInputs::def_selections`, which the driver's
  `def_family_selections` collects over `overload_family_names`) into
  `Mono::family_selections`, which `Elab::family_call_is_served` consults
  after the closed `Mono::def_call_targets`, and
  `Elab::forwarded_family_target` (`comptime/mono.rs`) the one declaration a
  clone's whole-pack forward to a sibling binds; `template_stub` in
  `comptime/specialize.rs` stands in for either deferred template;
  `Mono::retain_abstract` records each abstract reference and
  `Mono::record_method_edge` each by-name method call from an abstract body,
  the specializer's `stub_reaching_bodies` closes both over
  `Mono::abstract_owner` (a bound-generic `def`, a struct method as
  `method_owner`'s `Struct.method`, or a generic nested `def` as
  `nested_body_owner`'s declaration site), `unserved_template_uses` keeps the
  references that can reach a stub from outside a stub-reaching body — plus
  those of a method no instance could clone (`Mono::unclonable_methods`) — as
  `Elaborated::unserved_template_uses` (`UnservedTemplateUse`),
  `Elaborated::stub_reaching_structs` stops the driver's round cap from
  converging on such an instance, and `unserved_template_parameter` names the
  parameter for the driver's `reject_unserved_template_calls`;
  `clone_source_tag` stamps each method clone's body before it is walked, so
  its span-keyed requests find the checker's records for that
  instantiation), the
  origin-slot guards (`ty_mentions_origin_slotted_struct` finds such type
  arguments, a pointer whose origin is a place
  `PointerOrigin::clone_bindable_place` admits among them, whose
  `CloneBinderProjection` the binder's pointer re-applies below the place
  it binds (`PointerOrigin::without_projection` peels it at the call);
  `clone_binding` rebinds an instance's slots to the
  `CloneOriginBinders` a clone declares, named by
  `symbol::CLONE_ORIGIN_BINDER_PREFIX`, for a bundled template's instance
  or `def` call as for a user template's, which
  `Checker::clone_origin_binder_ids` leaves unbound for the argument
  exclusivity rule; `pack_element_source_type` spells
  erased slots as `_`), and the free-function/`Mono` support code; `Elab`'s remaining
  methods are split across `impl<'a> Elab<'a>` blocks in the submodules
  below (`comptime/elab.rs` holds the root driver's own cluster), and the
  root's helper clusters live in
  `comptime/{synth,ctfe_calls,packs,params}.rs`.
- `comptime/pack_qualification.rs` owns upstream's spelling rule for a
  variadic struct's own pack (`qualify_struct_packs`, the first step of
  `elaborate_with_requests`): a member naming the pack bare reports
  `ComptimeError::UnqualifiedStructParam`, the header keeps the bare name,
  and the parser's qualified spread `Type::SelfParam("*Ts")` is folded onto
  the bare `Named("*Ts")` node every later fold expands. It walks members
  with `mojito_ast::visit` (`Visitor` + `walk_*`), the crate's read-only AST
  traversal with a scope hook for shadowing binders.
- `comptime/eval.rs` owns compile-time expression evaluation (the `eval`
  dispatcher, set/dictionary displays and the explicit literal constructors,
  the structural collection folds `len`/`in`/`keys`/`values`, reflection
  methods, infix/iteration folding).
- `comptime/ctfe.rs` owns VM-driven compile-time evaluation — `ctfe_call`,
  `ctfe_struct_entry`, `ctfe_generic_def_entry`, and the general
  `ctfe_expr_entry` (collection bindings as display-initialized locals, a
  checked typing probe, then the typed entry) — the VM-CTFE program rewrite
  (`vm_ctfe_subprogram`, which also returns the catalog its checks derive
  its traced clones from: `TemplateCatalog::for_subprogram` over the
  driver's catalog that `elaborate_prepared` receives), and the effect walk (`vm_ctfe_safe_*`: deterministic bodies run, only
  `print`/`input` reject).
- `comptime/requests.rs` owns the module constants evaluated on demand
  (R7): `Elab::defer_constant` records one whose initializer applies a
  callable (`Elab::applies_callable`, `comptime/elab.rs`, the line every
  in-body request is drawn by, with its checker twin in
  `comptime_validation.rs`), `Elab::force_constant` evaluates it for a
  reader above the check (`Elab::eval`'s identifier fallback, and
  `Elab::pending_lookup` inside `materialize_block`), and
  `Elab::request_pending_reads` rewrites a body's value read to the request
  `comptime(<initializer>)`, `Elab::restore_forced_constants` putting back
  only the declarations something forced.
- `comptime/crossing.rs` owns the compile-time → runtime crossing fold
  (`fold_runtime_crossings`: `materialize[X]()` and `comptime(e)` become
  literals, and so does a reflection query over a closed handle
  (`reflection_query`: `r.field_count()`, `reflect[T].field_index["x"]()`);
  a bare runtime use of a compile-time collection is rejected with
  upstream's `ImplicitlyCopyable` text; runtime locals shadow).
- `comptime/unparse.rs` owns the source spelling of a `where` clause for
  diagnostics (`render_where_clause`, `violated_constraint_message`: the
  clause as declared with `Self.` dropped, upstream's note text).
- `comptime/specialize.rs` owns monomorphization and `def` specialization
  synthesis (`generate_def_spec`, request seeding), and the per-instantiation method clones of
  ordinary generic structs (`generate_instance_clones`, driven by
  `StructInstanceRequest`s and by the in-elaboration instance worklist
  `mono.rs` feeds through `instance_template`/`request_instance`; the clone
  carries `ast::Method::self_ty`, which the checker binds `self`/`Self` to
  and records as `AnnotationSite::MethodSelf` for MIR; the body check reads
  that record back, and `MethodSig::receiver` keeps it so a call binds the
  clone's origin binders from its receiver, `bind_clone_receiver_origins` in
  `checker/origins/construct.rs`). `keyed_methods` there says which methods
  of an instance still clone: the ones whose template body is the trap stub,
  the stub-reaching ones, and the driver-reported ones
  (`ElaborationInputs::keyed_methods`). Every other method mints no clone,
  whatever the instance's arguments carry, and a method with compile-time
  parameters of its own, a type pack among them, mints no per-call clone
  unless it is keyed;
  `template_serves_method` says the same for a non-generic struct's method,
  judged on its elaborated body, whose `comptime if`/`comptime for` over its
  own binders stays in the template (`Elab::def_body`). `per_call_stubs`
  seeds `stub_reaching_bodies` with the methods whose template is still the
  trap stub, so a served body calling one over its own binders is
  stub-reaching. A clone of a stubbed method that fails to elaborate is
  minted by `failed_method_clone` with `instantiation_failure_stub`'s body,
  the compiler-private `_mojito_instantiation_failed("…")` the checker types
  like `_mojito_abort` (`call_inference.rs`, diverging in
  `declarations.rs:is_diverging_intrinsic`), so `native::mono` reports it
  where it is reached (`mono/failure.rs`); the erased VM raises it as
  unsupported. `template_serves_def` says
  which generic `def` keeps its template at every closed call: a plain
  trait-bound one, with type parameters and scalar or `DType` value
  parameters (`template_serves_binders`, a value inferred from any argument
  type included), and no such construct; a `DType`- or lane-keyed `def`,
  overloaded or not, is an ordinary generic `def`, specializable only when
  its body keys a clone (`is_specializable_declaration`). An
  associated type its body names is solved below
  the waist, from `MirStructDeclaration.associated_types`
  (`declared_associated_type` over `struct_instance`,
  `native/mono/substitute.rs`).
  `checker/origins/transfer.rs`
  owns the carried source that lets both (`SigOrigin::Carried`, recorded by
  `record_transfer_effect` and closed by `replay_transfer_effects` with the
  receiver's arguments and the call's own bindings, `call_closed`).
- `src/compiler/template_reach.rs` owns what a template-served body reaches
  once its owner's parameters are bound (`TemplateReach`): a method at an
  instance, and a generic `def` at a closed call (`closed_def_calls`). It is
  read from one check's facts: the
  closed instances its checked types name once the struct's parameters are
  bound (`instances`, requested like checker-recorded ones), and the methods
  whose checked bodies only an instance's own check can serve
  (`keyed_methods`; a `def` is listed with an empty method name). A
  method with compile-time parameters of its own is read at each closed
  call the checker recorded (`closed_method_calls`, `method_call`), its
  struct's and its own binders bound: the instances its body applies are
  requested, and one applying a tuple over its own binders is keyed, as is a method the last elaboration found reaching a
  compile-time-keyed stub (`TemplateDemand::note_stub_reaching`, from
  `Elaborated::stub_reaching_methods`). `compile_linked` consults it every discovery round, and
  a body that reached a struct whose method becomes keyed is inferred again
  (`ServedRequests::keyed_templates`).
- `comptime/mono.rs` owns the monomorphizing AST rewrite (`mono_type` and
  friends) and struct-specialization argument resolution.
- `comptime/rewrite.rs` owns AST substitution and value materialization.
- `comptime/synth.rs` also owns the `SIMD[_, _]` parameter desugar
  (`desugar_simd_wildcard_parameters`: an infer-only `DType` and
  `SIMDLength` binder pair per wildcard parameter of a `def`, method, or
  trait requirement), the vector-alias bound fold, and the value reading
  of a `Self.`-spelled `materialize` operand
  (`read_materialize_self_operands`: `materialize[Self.n]()`,
  `materialize[Self.values[i]]()`).
- `crates/mojito-symbol/src/symbol.rs` owns specialization keys. `mangle`
  returns `Result<String, NonConstantSpecialization>`: `mangle_parts`
  validates the whole key (`specialization_value_is_closed`: no residual or
  deferred value in an aggregate child, a struct field, or a type payload's
  value argument) before writing any of it, so no symbolic spelling enters a
  key. `SpecializationKeyPart::ErasedOrigin` is the owner-key marker an erased
  origin argument contributes. `specialized_method_values` refuses a residual
  value and skips only a deferred callable-value slot.
- A scalar `__hash__` leaf calls its hasher's `_update_with_simd`
  instance at the leaf's vector type (`mojito_types::types::hash_leaf_ty`):
  `native::mono`'s `enqueue_hash_leaf_instances` instantiates it, and the
  VM (`Prog::hash_leaf_update`, which an erased run answers with the
  template and the lane binders `lane_binders_from_arguments` reads off the
  argument values) and Pliron (`hash_leaf_update_instance`) select it by
  the lane shape of its value parameter.
- `crates/mojito-symbol/src/symbol.rs` owns the value-specialization demangler
  (`demangle_specialization`, which rebuilds every key but a type or
  reflected one, and `unqualified_instance_name`) behind
  `_unqualified_type_name`'s spelling of a minted clone.
- `crates/mojito-symbol/src/symbol.rs` also owns the string identities
  (`is_stdlib_string_struct`/`is_stdlib_string_span_struct`, re-exported from
  `mojito-types`; `nominal_string_literal_ctor_symbol`, the literal→`String`
  wrap MIR emits) and the
  instance-clone identity shared by the checker and both backends:
  `specialized_method_values`,
  `materialized_instantiation_argument`, and `instance_method_clone_name`
  (the VM's `instance_dunder_symbol` in `backend/vm.rs` and the native
  monomorphizer's `instance_dunder_target`/`enqueue_display_instance` in
  `native/mono/instances.rs` select clones through it from checked register
  types, as does `instance_method_target`, which hands a template body's
  method call on a closed receiver to that instance's clone where one
  exists, and `dispatched_overload_target`, which selects the receiver's
  own overload for a bound dispatch whose qualifier spells the requirement's
  parameter otherwise than the witness
  (`__hash__$ov$Some$u5B$Hasher$u5D$$Hasher` against `Twin.__hash__$ov$H$Hasher`),
  and `enqueue_display_instance`'s `protocol_overload`, which picks the
  `Writer`-taking `write_to` beside a same-arity rival;
  `mono/infer.rs:bind_static_receiver` binds a struct's parameters
  from the receiver type a static call records, which
  `checker/method_calls/statics.rs:record_static_receiver` types and MIR
  lowering copies into `MirInstr::Call::receiver` (a static method's
  `Self.n` reads a receiver-less `self` slot `Flatten::intern_static_self`
  types as the struct at its own binders; the erased VM binds that slot,
  and each of the struct's type parameters by name, from the same field,
  `static_receiver_binding`; an erased construction reifies an inferred
  type argument from the call's result type,
  `VmBackend::constructed_parameter_arguments`, and a binder of the caller
  from its frame, `CallerBindings::comptime_binding`), and `infer_call` binds a
  `def`'s own type parameters from the arguments the checker solved, which
  `SemanticAdjustment::InstantiatedArguments` carries into
  `MirInstr::Call::instantiated_args`, and a method's own from a
  `checked::MethodInstantiation` into `MirInstr::MethodCall::instantiated_args`,
  its inferred value parameters (`MethodInstantiation::inferred_values`) passed
  as named arguments as a `def`'s are); the checker closes a method's own value
  binders in its result (`Checker::close_method_values`) and types a spelled
  `DType` argument (`record_dtype_parameter_arguments`); `instance_clone_base` recovers a clone's source method name for the
  exact-name lifecycle gates, `split_method_symbol` splits a lowered method
  symbol from its receiver at the last `.` outside brackets (a clone's baked
  `SIMD[DType.float32, 2]` keeps its `.`; the MIR verifier, template facts,
  and the monomorphizer parse method symbols through it), and `lifecycle_constructor` recognizes a
  construction through a clone or an overload. The VM's `lifecycle_symbol`
  and `instance_field_types` (`backend/vm.rs`) pick an instance's
  `__init__`/`__copyinit__`/`__moveinit__`/`__deinit__` clone from a checked
  static type, substituting `types::struct_argument_substitution` into the
  fields a whole-value drop or copy reaches; the native side names such a
  clone by its instance (`lifecycle_clone_instance_symbol` in
  `native/mono/specializer.rs`), which is the symbol Pliron's lowering
  composes.
- A closed bracket argument is compile-time data on its call: the checker's
  `folded_parameter_arguments` (`checker/call_inference.rs`, over
  `bracket_argument_slots`, the one bracket-to-declaration binding
  `unsupplied_value_parameters` also reads) fills
  `GenericInstantiation::folded_arguments` and
  `MethodInstantiation::folded_arguments` with the spans of arguments solved
  to `CtValue::is_folded_parameter_argument` values, carried to MIR as
  `SemanticAdjustment::FoldedParameterArguments`; `Flatten::param_arg_regs`
  (`Flatten::param_arg_has_no_runtime_form`, `mir.rs`) lowers no code for
  them; `verify_param_arguments` (`verify/subscripts.rs`) accepts the absent
  register against the call's `instantiated_args`; and
  `VmBackend::supplied_parameter_arguments` fills the erased slot through
  `VmBackend::thaw` (`backend/vm.rs`), `freeze`'s inverse.

## Change Routing

| If you change… | Start at… | Also inspect… |
|---|---|---|
| Syntax or AST shape | [`grammar.md`](grammar.md), `parser.rs`, `ast.rs` | Parser tests, `frontend.md`, feature matrix. |
| Argument binding | `call.rs` | Checker/VM adapters and call-parity tests. |
| Which declarations use a parameter as a lane width | `mojito-ast/simd_width.rs` | The elaborator's per-call specialization (`comptime.rs`, `comptime/synth.rs`'s method stubs) and `checker/comptime_validation.rs` (`value_keyed_def`, `validate_comptime_method_bodies`). |
| An AST walk (read-only `Visitor`, in-place `MutVisitor`) | `mojito-ast/visit.rs` | `comptime/pack_qualification.rs`, `ast::stamp_source`, `checker/rebind.rs`. |
| Overload identity | `symbol.rs` | Checker selection, MIR declarations, symbol/rejection tests. |
| Type rules | `checker.rs` or focused checker child | `CheckedProgram`, negative checker tests. |
| Ownership/destruction | `analysis.rs` and its `analysis/` submodules | MIR place/use forms, ownership and drop tests. |
| Runtime behavior | `backend/vm.rs` or `runtime/mod.rs` | VM tests and file fixtures. |
| Pipeline ordering | `compiler.rs` | CLI, architecture doc, compiler tests. |
| Support status | `docs/features.md` | Roadmap/todo only if future work changes. |

## The compile-time loop (2026-10-03)

- `MirTerm::ComptimeFor { binder, slot, source, body, exit }`
  (`mir/ir.rs`) is the `comptime for` header: HIR's
  `Terminator::ComptimeLoop { iter, var, binding, index, body, exit }`
  (`hir.rs`, lowered in `Lower::stmt` with the loop variable declared as a
  `for` variable is), lowered by `Flatten::lower_term` (`mir/lower_stmt.rs`)
  from the checker's `SemanticAdjustment::ComptimeIteration` (recorded by
  `Checker::record_comptime_iteration` inside `check_comptime_for`,
  `checker/comptime_validation.rs`, a range's bounds compiled by
  `compile_dependent_ct_expr`, a literal display's elements by
  `closed_iteration_elements`, a value pack by `value_pack_named`, a
  reflected name list by `reflection_list`, and a display over the binders
  left as `ComptimeSource::Evaluated` when `evaluated_display` admits its
  elements, carrying the display's own construction, which the record
  displaces on the display's span and `Flatten::display_adjustments`,
  `mir/lower_expr/ctrl.rs`, reads back). `lower_term` turns an evaluated
  source into the application of a thunk
  (`ComptimeThunks::request_sequence`, `mir.rs`, lowered with the condition
  thunks at each request's own result type), which `verify/scope.rs` admits
  as a loop sequence of list meta. A local `comptime` binding of such a
  display is typed by `Checker::evaluated_display_binding` from
  `bind_template_comptime`, which declares the name at the display's type,
  records `SemanticAdjustment::ComptimeDisplay` on the display with the
  sequence the binding denotes (`record_display_binding`,
  `display_sequence` over `binders_in_scope`), and keeps a `BoundDisplay` in
  `local_comptime_displays` (source validation notes one too,
  `note_validated_display`); an element or the length in a type or a
  parameter argument is a parameter expression over that sequence
  (`Checker::display_read`, `display_element_argument` for the capitalized
  spelling, `checker/constraints.rs`, from `eval_associated_ct` and
  `compile_dependent_ct_expr`; `BoundDisplay::element` and `length` over
  `ParamContext::list_get` and `list_length`, whose
  `LIST_LENGTH_FUNCTION` application `builtin_application_value` answers);
  a loop over the name records
  `ComptimeSource::Bound` (`record_bound_iteration`), a `range` bound the
  check does not close records `ComptimeSource::EvaluatedRange`
  (`record_comptime_iteration`, admitted by `reads_compile_time_alone` over
  `comptime_binding_owners`), a condition naming an evaluated binding is
  left uncompiled (`names_evaluated_binding`), and a runtime read is
  rejected (`reject_display_crossing`, from `inference.rs`).
  `Flatten::lower_comptime_binding` (`mir/lower_stmt.rs`) records the
  binding with its checked facts (`DisplayBinding`,
  `ComptimeThunks::bind_display`, `Flatten::copy_facts`) and gives it no
  runtime form, lifting the display under the name and over the binders
  the check's sequence spells; `lower_term` reads a header's sequence back
  (`ComptimeThunks::bound_sequence`), and lifts an evaluated bound
  (`ComptimeThunks::request_value`). A bracket argument that reads a
  display is lowered by `Flatten::param_arg_reg` through `display_read`, or
  `display_element_argument` for the capitalized spelling, and an inferred
  one by `inferred_value_register`. An `Int` or a `Bool` such an
  argument, or a local `comptime` value, computes some way the parameter
  domain does not express (`h(L[0])`, `n > 2`) is lifted by
  `Checker::lifted_application` (`checker/comptime_validation.rs`), reached
  from `eval_associated_ct` and `compile_dependent_ct_expr`
  (`positioned_application`) inside a `Checker::lifting_position`
  (`checker/scopes.rs`), which a value parameter argument, a SIMD width
  (`simd_width`), and a local `comptime` value
  (`comptime_value_expression`) open: it names the function, applies it to
  the binders the expression reads (`binders_read`), shares it between
  equal occurrences (`LiftedApplication`), and keeps the application by the
  expression (`lifted_expressions`, a body-carried store) for
  `record_lifted_applications`, which `into_carry` runs to record
  `SemanticAdjustment::ComptimeApplication` beside the expression's own
  operation. The arena builder gives such an expression inside an
  annotation a node (`type_applications`, `checked.rs`). A call of a module
  `def` or a static method over `Int` and `Bool` values is instead the
  application of the callable itself (`Checker::called_application`, tried
  first by `lifted_application` and alone in a signature or a field type):
  `applicable_functions` (`checker/comptime_validation.rs`, run by
  `check_program` and `ConformanceOracle::from_program`) collects every
  overload under the name a call spells (`ApplicableFunction`, built by
  `ApplicableShape::applicable`) before any declaration is checked,
  `selected_application` picks the one overload the arguments bind and
  type, `applied_owner` resolves a static call's struct and instance
  (`S.f(n)`, `G[n].f()`, `Self.f()`), `applied_arguments` orders the
  compile-time and runtime arguments through `ast::call::match_call_slots`
  with `applied_default` for an omitted one, `calls_raising_application`
  backs the raising-callee rejection in `eval_associated_ct`,
  `solve_applied_args` (`checker/generics.rs`, from `solve_value_args`)
  solves a value parameter through one, and
  `ComptimeThunks::request_applications` lifts nothing for it. `not` and a
  conditional are compiled by `fold_ct_not` and `fold_ct_cond`
  (`checker/constraints.rs`). A static call through `Self` in a body is
  typed by `infer_type_receiver_call`
  (`checker/method_calls/type_receivers.rs`) as the call on the enclosing
  struct applied to its own parameters, and the parser spells the type
  argument forms (`Self.w()`, `G[n].w()`) in `parse_param_arg`
  (`parser/types.rs`).
  `ComptimeThunks::request_applications` (`mir.rs`, from `lower_fn_nested`
  and the nested-`def` lowering once the body is lowered) lifts each named
  function over the narrowest recorded scope (`enter_scope`) that declares
  its binders, `ComptimeThunks::lower` skipping a name another owner
  already lifted, and `Flatten::comptime_application` reads the expression
  back as its application in `display_read` and `value_binder_expr`.
  `Flatten::display_read` (`mir.rs`)
  lowers an expression the check lifted as the parameter constant of its
  application, and a compile-time expression that reads a display
  (`displays_read`): an `Int` or `Bool` as a parameter constant over its
  thunk's application, any other value in place after `build_display`. A
  local `comptime` binding or a `comptime(e)` operand that applies a
  callable at a type no parameter expression spells (a `String`, a tuple, a
  struct) is lifted by name at its own type
  (`Checker::requested_binding`, `checker/comptime_validation.rs`), in any
  body; the erased oracle runs such an application by calling its function
  (`VmBackend::erased_application`, `backend/vm.rs`);
  `Flatten::crossing_operand` (`mir/lower_expr/expr_access.rs`) is the
  operand of a kept `materialize[X]()` or `comptime(e)`
  (`Checker::infer_template_materialize`, `infer_template_comptime`), a
  whole display through `crossing_display`. Every thunk begins with the
  local `comptime` bindings its expression reads that denote no parameter
  expression (`ComptimeThunks::bind_evaluated`, `thunk_prologue`,
  `bindings_read`, lowered by `lower_expression_thunk` under
  `EnclosingBinders::lifted`). `native::mono` evaluates an application
  wherever it meets one: `Specializer::applied`
  (`mono/specializer.rs`, over `ParamContext::answer_applications`) serves
  `answer_param_constants` (a parameter constant, and a bracket argument's
  expression through `mojito_mir::mir::instruction_param_args_mut`), a
  condition operand in `resolve_application`, and a range bound in
  `trip_elements`. An application in a type or a parameter argument is
  answered where substitution evaluates it: `eval_ct` (`mono/symbolic.rs`)
  reads `Bindings::applications` (`Applications`, `mono.rs`), which records
  one it has no value for, and `Specializer::materialize_body` demands
  those and substitutes the template again (`substituted`,
  `answer_pending`). A thunk reads an enclosing local
  `comptime` value (`comptime k = n + 1`) as its parameter expression:
  `lower_comptime_binding` records it (`ComptimeThunks::bind_value`), each
  request carries the bindings so far (`EnclosingBinders::comptime_bindings`),
  and `Flatten::identifier_read` (`mir/lower_expr/expr.rs`) answers the read.
  The same holds for such a binding as a method receiver
  (`lane.is_floating_point()` over `comptime lane = dt`):
  `Flatten::lower_call_receiver` (`mir/calls.rs`) reads it as a value, not
  a place, and `type_receiver_name` never takes a checked binding for a
  type name. The header's `source` is the
  `mojito_checked::checked::ComptimeSequence` (`Range { start, stop, step }`
  or `Elements(expr)`, whose `binder_meta` types the binder; the elements a
  value yields are `CtValue::comptime_iteration_elements`, `ct.rs`, which the
  AST elaborator's `as_sequence` shares), its slot the checked binding's
  (the slot HIR declared for the binding, typed by `binder_meta`).
  `mir.rs::loop_index_scopes` puts each loop's index among the
  `EnclosingBinders` of its body blocks, with its checked binding
  (`EnclosingBinders::with_loops`, `loop_bindings`), so a bracket argument
  built from it (`g[i]()`) resolves. A loop binder's owner starts
  `mojito_types::param_expr::COMPTIME_FOR_OWNER`
  (`ParamId::is_comptime_for_binder`), which the checker mints
  (`comptime_index_binder`) and `mono/substitute.rs::bound_parameter_locals`
  skips. `verify/scope.rs` (`Scope::declare_loop_binders`)
  brings the binders into scope and checks the sequence; `verify/concrete.rs`
  rejects a survivor; the text form is `comptime_for` for a range (schema
  1.16) and `comptime_for.elements` for any other sequence (schema 1.25). A
  `Bool` loop binder is a `comptime if` condition of its own
  (`is_bool_loop_binder`, `check_ct_bool`).
- Reflection in a served loop: `Checker::eval_reflection_expr`
  (`checker/reflection.rs`) answers a reflected list's length (`len(names)`,
  through `reflection_list_count`) for a loop bound;
  `bind_template_comptime` inlines a bound reflected list;
  `infer_template_materialize` (`checker/comptime_validation.rs`) types
  `materialize[X]()` over a binder, which `Flatten::expr`
  (`mir/lower_expr/expr.rs`) lowers as its operand; `check_ct_bool` compiles
  `conforms_to` over a dependent element to `ParamKind::Conforms`
  propositions, which `Specializer::resolve_application`
  (`mono/specializer.rs`) decides through `type_conforms`; and
  `substitute_ty` (`mono/substitute.rs`) closes a reflected field type by
  evaluating its expression. A runtime read through a bound name list
  (`print(names[i])`) is `TypeError::ComptimeCrossing`
  (`reject_bound_list_crossing`), outside the compile-time positions
  `Checker::comptime_position` opens (`checker/scopes.rs`): a `comptime`
  binding's value, a `comptime if` condition, a `comptime for` iterable, a
  `materialize` operand. `substitute_identifiers`
  (`checker/comptime_validation.rs`) keeps each rebuilt node's syntax id, so
  a fact recorded on an inlined local `comptime` value is the original
  occurrence's.
- `mir/ir.rs` gained `instruction_regs_mut`, `terminator_regs_mut`,
  `terminator_targets`, and `terminator_targets_mut`, the register and
  target visitors a copied block is renumbered through, and
  `block_successors` is public.
- `native::mono::unroll` (`mono/unroll.rs`): `unroll_comptime_loops` runs
  before `substitute_function`; `outermost_loop` and `loop_body` (dominators)
  pick a loop and its body, `trip_elements` evaluates its sequence — a
  thunk's application through `Specializer::resolve_application`, whose
  result `VmBackend::freeze` (`freeze_collection`, `backend/vm.rs`) reads
  out of a nominal `Array`, `List`, `Set`, or `Dict`, freezing a nominal
  `Tuple` element (`is_nominal_tuple`) to its elements and any other struct
  to its fields — and
  `copy_body` appends one finished copy per
  iteration (fresh registers, a fresh slot from `fresh_slots` for each slot
  whose type names the index (`mojito_types::types::names_binder`),
  `substitute_value_parameter_reads` with the index among the locals,
  `substitute_blocks_metadata`, nested `unroll_in`,
  `select_comptime_branches_in`), `retarget` chains the copies, and
  `forget_registers` drops the dead body's tables; the template's
  index-typed slots leave the instance through `retire_slots`.
- `native::mono` slot renumbering (`mono/slots.rs`): `renumber_slots`,
  `renumber_blocks`, `addressed_slots`, and `retire_slots`, shared by
  runtime promotion (`mono/promote.rs`) and unrolling; `reification_slots`
  names a type parameter's erased-oracle slot, which `materialize_body`
  retires from each instance. The comptime
  elaborator's walk (`Elab::mono_stmt`, `comptime/mono.rs`) treats a kept
  `comptime for`'s index as a symbolic parameter, so a struct applied over
  it (`Lanes[i]`) stays for the checker. `comptime_for_next`
  (`backend/vm.rs`) runs the header on the erased path from the slot and a
  per-frame cursor (`VmBackend::comptime_cursors`).
- The cloner keys a top-level `def` on a `comptime for` its template does
  not serve (`comptime_for_is_template_served`, `comptime.rs`, over the
  `LoopNames` of the `def`: its packs, its value packs
  (`def_value_pack_names`), its
  local bindings of a display over the binders
  (`served_display_bindings` over `mojito_ast::visit::display_bindings`,
  none when `display_in_unserved_argument` finds a type or parameter
  argument that is a display itself or a collection built from one, which
  the elaborator holds per open template body in
  `TemplateLoopNames`), and
  which bare names are closed collections, `def_bound_names` telling a
  module constant from a name the `def` binds; `parameter_shaped`,
  `scalar_shaped`, `display_read_shaped`, `reflection_count`, and
  `reflected_names` are the admitted spellings, a call or method call
  admitted by `ScalarReads::call` from the verdict source validation
  recorded alone (no declaration fallback), `Checker::scalar_calls` (`checker/comptime_validation.rs`) into
  `TemplateCatalog::scalar_calls`; a closed-aggregate display element by
  `literal_tuple` or by `aggregate_shaped` over `ScalarReads::aggregate`,
  from `Checker::aggregate_elements` into
  `TemplateCatalog::aggregate_elements`, the checker's own test being
  `Checker::parameter_aggregate_element`; a named collection by
  `CtValue::is_parameter_value_collection`), on a reflected list materialized whole
  (`ReflectedLists::materialized_in`), or a nested `def` holding a
  `rebind`. The crossing pass spells a named collection as its display in
  a kept header (`cross_stmt`, `comptime/crossing.rs`), and leaves a
  `comptime(e)` it cannot evaluate in a generic body for the check
  (`in_template_body`);
  `Elab::keep_template_comptime_for` keeps the served loop and
  `Elab::unroll_comptime_for` unrolls the rest, refusing a compile-time
  `break`/`continue` it would splice into the wrong loop (`comptime/elab.rs`).
- A type pack the template serves: `served_pack_defs` (a fixpoint over
  `pack_def_shape_served`, `pack_spread_callees` with its `SpreadCallee`,
  `pack_collector_methods`, `pack_collector_constructors` (both over
  `collects_type_pack`), `def_pack_names`, and
  `def_body_keys_specialization`, `comptime.rs`) names the served `def`s and
  `pack_def_template_served` reads it; `PackRewriter::served_callees`
  (`comptime/rewrite.rs`) spells a clone's spread into a served callee
  element by element; `comptime_for_is_template_served` admits a pack's
  length as a bound; a whole-pack spread is `MirInstr::Call::spread` or
  `MirInstr::MethodCall::spread`
  (`lower_pack_spread_argument`, `mir/calls.rs`; `verify_pack_spread`,
  `verify/calls.rs`; `splice_pack_spread`, `backend/vm/calls.rs`, for the
  erased oracle) and `native::mono::spread` (`mono/spread.rs`:
  `expand_pack_spreads`, after `substitute_function`; `spread_operands` reads
  either call form) replaces it with the bound pack's element places; `Checker::pack_length_binder`/`pack_length_query`
  (`checker/constraints.rs`) read `args.__len__()`, `Ts.length`, and
  `len(Ts)` as `PackQuery::Length` for the loop bound and as a
  `ConstraintOperand::PackLength` in a `comptime if`; a pack element's
  literal argument converts to the element the call solved
  (`call_inference.rs`); `verify_param_arguments` (`verify/subscripts.rs`)
  and `matched_parameter_arguments` (`mono/unify.rs`) let a pack take every
  positional compile-time argument; `bind_pack` (`mono/unify.rs`) binds the
  pack from the call's `TyArg::Val(CtValue::Tuple)` or the overflow's types,
  `ct_bindings` (`mono/symbolic.rs`) carries the solution into `eval_ct` and
  `substitute_ty` (a `Dependent` element closes through
  `replace_parameters`; a `VariadicPack` over a bound pack is the elements'
  `Tuple`), and `ParamContext::replace` (`mojito-types`) answers a bound
  pack's length; `erased_parameter_values` (`backend/vm.rs`) gives the
  erased frame each pack's arity, from the collector or the call's recorded
  elements.

## Struct generators (P3d, 2026-10-05)

- No struct is a template the cloner keeps: `is_specializable_declaration`
  (`comptime.rs`) admits only a `def`, and every struct — keyed on a
  `DType`, a lane width, a vector or struct value, or a type pack — is a
  generator `native::mono` instantiates.
- `Variant` is a generator (2026-10-06). Its storage operations carry
  upstream's `_get_type_index[T, *Ts]()` as `types::VariantIndex`
  (`mojito-types/src/types.rs`; `PackQuery::IndexOf` in `param_expr.rs`,
  text `pack_index_of`, schema 1.31): the checker records it
  (`inference.rs:variant_index`), `mojito-checked`'s Variant adjustments,
  `HirPlaceProjectionKind::Variant`, the MIR `Variant*` instructions, and
  `Proj::Variant` carry it, `mono/substitute.rs:close_variant_index` closes
  it per instance (failing a non-member), `verify/concrete.rs` rejects a
  symbolic one, `analysis/moves.rs` keys it as `Key::Variant(None)`, and the
  VM (`known_variant_index`, `backend/vm.rs`) and Pliron
  (`lower/variants.rs`) read only a known one.
- `Tuple` and `TString` are generators (2026-10-06). The checker spells a
  tuple's type as the nominal application (`types::tuple_type`,
  `types::canonical_pack_arguments` in `struct_instance_type`), reads an
  unpacked element through `Tuple.__getitem_param__`
  (`statements.rs:tuple_unpack_plan`, `CheckedTupleUnpackElement::{param_decls,
  temporary}`), assumes a conformance's `where` clause for a pack field
  (`overload_support.rs:binder_conformance_assumed`), and binds an open
  computed index as its parameter expression (`type_resolution.rs:resolve_param_arg`).
  `mir/facts.rs:subscript_call_contract` records that expression on the
  subscript call, `mono/substitute.rs:substitute_subscript_call` folds it in
  each loop copy, and `mono/infer.rs:rewrite_subscript_call` binds the index
  before the receiver is inferred. `mono/unify.rs:owner_instance_ty` and
  `substitute_ty` name an instance element by element, the empty tuple
  included. The checker builds a `t"…"` occurrence's `__make_tstring` call
  itself (`checker/template_string.rs`); the driver derives no t-string
  request.
- A read of a closed vector, struct, or tuple binder folds to
  `Const::Value(CtValue)` (`mir/ir.rs`, text `value(...)`, schema 1.27):
  `Specializer::value_param_constant` (`mono/instances.rs`) and
  `value_parameter_constant` (`mono/substitute.rs`) build it, a whole
  place read and the field-projection loads folding through the bound value
  (`substitute_value_parameter_reads`'s `LoadPlace` arm and
  `projected_parameter_constant`, and the `LoadPlace` arm of the
  specializer's rewrite); `CtValue::is_closed_parameter_value`
  (`mojito-types/src/ct.rs`, called from `verify/instr.rs`) admits it. A
  value with a `String` leaf (`CtValue::is_constructed_parameter_value`)
  is not folded: `substitute_function` constructs it at each read of its
  slot (`construct_parameter_reads`, `mono/substitute.rs`, through
  `parameter_value_construction`), the slot holding nothing, and
  `seed_captured_parameter_slots` stores a captured folded slot's constant
  at entry. The VM materializes a folded one through
  `ct_value_as_runtime` (`backend/vm.rs`) and, a tuple at any depth as the nominal `Tuple` its
  checked type names, `materialize_parameter_value`
  (`backend/vm/adapters.rs`), and Pliron through `lower_parameter_value`
  and `store_parameter_value` (`lower/consts.rs`). A `comptime for`
  element with a `String` leaf is constructed at each read of its binder
  the same way (`construct_parameter_reads`, from
  `Specializer::copy_body`, `mono/unroll.rs`), and the erased VM
  materializes the element it binds at the slot's type
  (`VmBackend::materialize_comptime_binder`, `backend/vm/adapters.rs`). A
  borrow of a compile-time value (a value parameter, a `comptime for`
  variable) is a temporary the checker materializes
  (`materialized_reference_actual`, and for a read of a tuple or struct
  that is not trivially register-passable, any place read,
  `materialize_parameter_read`, both `checker/origins/actuals.rs`), which
  MIR roots any place read at (`Flatten::materialized_parameter_place`,
  `mir/calls.rs`); a consumed read hands the value over instead
  (`consume_parameter_read`). A field chain off a compile-time binding
  (`Checker::is_parameter_read`, `checker/places.rs`) is a `ParamValue`
  that `Checker::infer_member` records, its root kept from materializing
  (`parameter_field_objects`); a borrowed one materializes through
  `ParamValue.materialized`, as `ConstructCollection.materialized` does,
  and the AST unroller folds one off a substituted binder
  (`substituted_field`, `comptime/rewrite.rs`). The erased oracle reads
  one off the frame slot (`projected_frame_parameter`,
  `backend/vm/exec.rs`). A compile-time parameter's slot is no
  drop root: a `comptime for` binder's (`comptime_binder_slots`,
  `analysis/scan.rs`) and a value parameter's
  (`materialized_parameter_slots`, `mojito-mir/src/mir.rs`, over the
  declarations `binder_scope` walks, which `Specializer::binder_scope`
  shares), both excluded by `elaborate_drops` and
  `RegionDropCtx::droppable`.
- `Self.e.rows` in a compile-time position is `ParamKind::Field { base, name }`
  (`mojito-types/src/param_expr.rs`, `ParamContext::field`, text
  `param_field`), built by `Checker::struct_value_field`
  (`checker/constraints.rs`) for `compile_dependent_ct_expr`,
  `eval_associated_ct`, and a `SIMD` width spelled `Self.e.rows`
  (`simd_width`, `type_resolution.rs`); `infer_member` (`indexing.rs`)
  types `Self.e` at its declared struct, and a method receiver's place for
  it is `Flatten::receiver_value_parameter_place` (`mir.rs`).
- `eval_associated_ct` freezes a fieldwise construction of a non-generic
  struct from compile-time values (`Extent(2, 3)`) to `CtValue::Struct`;
  `Elab::freeze_struct_value_arguments` (`comptime/mono.rs`) rewrites any
  other struct-typed argument of a struct or a uniquely named `def` to that
  construction, evaluated by CTFE.
- `bind_ty_args` (`mono/unify.rs`) binds a struct's pack element by element,
  whole (`CtValue::Tuple`), or forwarded (`Ty::RuntimePack`) through
  `bind_pack`.
- The erased oracle reifies a struct instance over value arguments as a
  type token (`reified_type_value`/`type_token`, `backend/vm.rs`), a fieldless
  `Value::Struct` that `ConstructTypeParam` constructs at its arguments;
  `align_parameter_arguments` collects a type pack's arguments and
  `constructed_parameter_arguments` reads a pack off the checked result
  type; a constructor's own solved parameters reach its frame as
  `ConstructorParameters::own` (`backend/vm/values.rs`); a `comptime if`
  over a pack element reads its spelling (`comptime_branch_holds`).

## The parameter constant (2026-10-04)

- `Const::Param(ParamExpr)` (`mir/ir.rs`) is a parameter expression read as a
  runtime value, the text form `param(...)` (schema 1.20). The checker
  records it as `SemanticAdjustment::ParamValue { value }`
  (`mojito-checked`, `checked.rs`) through `Checker::record_pack_query_value`
  and `typelist_proposition_query` (`checker/comptime_validation.rs`), from
  `infer_member` (`Ts.length`, `indexing.rs`), `infer_len` (`len(Ts)`,
  `builtins.rs`), and the `Invoke` arm of `infer` (`Ts.contains[X]()`,
  `Ts.all_conforms_to[T]()`, `inference.rs`); `derive_adjustment`
  (`templates.rs`) substitutes it for an instance, and a struct clone's
  derivation drops it where the clone folded the query
  (`substituted_facts`, `folded_literals`). A value pack that is still a
  parameter (`*values: Int`) is a `ParamList` of its element
  (`annotations::value_parameter_expr`, `verify::declared_kind`):
  `Checker::value_pack_named` resolves it — a struct's own `Self.values`
  through `self_value_pack`, which `infer_member` types as the pack — and
  `infer_value_pack_element` records `values[i]` as a `ListGet`, while
  `infer_len`, the intrinsic `__len__` receiver, and `pack_length_binder`
  record its length; `value_packs_read_as_parameters` (`comptime.rs`) keeps
  the clone for any other read, and `LoopNames::value_packs` admits a
  `comptime for` over `Self.values` to a method's template. A pack spread
  whole into brackets (`Pack[*vs]`, `total[*vs]()`) binds the callee's pack
  to it (`Checker::value_pack_spread`, read by the bracket binding in
  `checker/declarations.rs`); MIR
  passes it as the list's parameter constant in a `VariadicPack` register
  (`Flatten::value_pack_spread`, `EnclosingBinders::value_pack`),
  `verify_param_arguments` accepts it in the pack's slot, mono folds it and
  retires it once the call names its instance (`retire_forwarded_packs`,
  `mono/spread.rs`), and the erased oracle splices it into the callee's
  tuple (`align_parameter_arguments`); and mono binds the pack from the call
  (`bind_instantiated_arguments`, `mono/infer.rs`;
  `bind_explicit_value_arguments`, `mono/unify.rs`) and folds the constants
  in each unrolled copy too (`copy_body`, `mono/unroll.rs`). The erased
  oracle reifies it as a tuple (`align_parameter_arguments`,
  `backend/vm.rs`) that `const_value` reads from the frame's `comptime`
  bindings. `Flatten::param_value` and
  `param_value_register` (`mir/lower_expr/expr_access.rs`) lower it; the
  pack operand is never lowered. `verify/scope.rs` checks its binders and
  `verify/concrete.rs` rejects a survivor.
- `Specializer::answer_param_constants` and `param_constant`
  (`mono/specializer.rs`) fold it per instance, a value owning a `String`
  constructed instead (`constructed_parameter_constant`,
  `parameter_value_construction`, `mono/substitute.rs`): a length through `eval_ct`,
  a membership or conformance as a `GenericConstraint` through
  `constraint_holds` (`mono/availability.rs`). `eval_ct`
  (`mono/symbolic.rs`) answers a reflection query through
  `ParamContext::answer_reflections` with `reflection_answer`, which reads
  the bound struct's fields off `Bindings::struct_shapes`
  (`struct_instance`, `mono/substitute.rs`) and applies
  `ReflectQuery::answer`; an unanswerable query
  (`ParamError::Reflect`) is a `MonoErrorKind::Instantiation` error that
  `param_constant` propagates. It then answers a builtin application
  through `ParamContext::answer_builtin_applications` and
  `builtin_application_value` (`param_expr.rs`): a `DType` float-format
  query at a float dtype is its `Int`, and at any other is
  `ParamError::Constraint`, the same instantiation error.
  `ParamContext::evaluate` answers them too, so the erased oracle does;
  `ParamContext::replace` does not, since an application stays symbolic in
  a type. The checker's producer is
  `record_reflection_value` (`checker/reflection.rs`). The erased oracle evaluates it
  in `const_value` (`backend/vm.rs`) against `erased_parameter_values`.
- On the cloner, `fold_pack_uses` (`comptime/rewrite.rs`, called from
  `generate_def_spec`) folds a `def` clone's own pack uses — `Ts[k]()` and the
  `TypeList` queries `fold_pack_typelist_use` answers — honouring a nested
  declaration's same-named type parameter.

## The compile-time branch and the request path (2026-10-03)

- `MirTerm::ComptimeBranch { cond: GenericConstraint, .. }` (`mir/ir.rs`) is
  the `comptime if`: HIR's `Terminator::ComptimeBranch`, lowered by
  `Flatten::comptime_condition` (`mir/lower_expr/entry.rs`) from the
  checker's `SemanticAdjustment::ComptimeCondition` (recorded by
  `Checker::check_comptime_condition`, `checker/comptime_validation.rs`, per
  leaf under `not`/`and`/`or`) or, for a leaf with no constraint, from the
  thunk `ComptimeThunks::request` registers with the binders in scope at
  the condition and `lower_expression_thunk` lowers over them (`mir.rs`,
  shared with `lower_default`); the thunk reads an enclosing loop's index
  as a parameter reference (`Flatten::parameter_reads`, filled from
  `EnclosingBinders::loop_bindings`, read in `Flatten::identifier_read`). `verify/scope.rs`
  (`ScopeCx::constraint`) checks its binders; `verify/concrete.rs` rejects a
  survivor; the text form is `comptime_branch` (schema 1.15).
- `mojito_vm::crossing` owns `ct_to_vm`/`vm_to_ct` and `CTFE_FUEL`;
  `VmBackend::{call_concrete, freeze}` run a verified fragment for the
  elaborator; `comptime_branch_holds` (`backend/vm.rs`) decides a value
  condition on the erased path from the frame's reified parameters
  (`Frame::comptime`).
- `native::mono`: `InstanceState`, `Specializer::drain`,
  `demand_application` (the demand edge: materialize the instance's
  reference closure — `Specializer::materialize_closure` over the names each
  materialization enqueued, `Specializer::references`, and the lifecycle
  members of the structs its types name, `lifecycle_instances`; a member
  still `Active` is the parameter-domain cycle — verify that closure as the
  fragment, refuse an effectful callee reached through any reference edge
  (`collect_referenced_functions`), burn fuel, run, freeze, cache by
  instance name and runtime arguments; the arguments
  past the callee's compile-time parameters are its runtime ones, for a
  `def` the check applied by name; a demand on an `Active` instance is
  the parameter-domain cycle), `resolve_applications` over a branch
  condition, `demand_layout` and `LayoutOracle` (`Bindings::layout`, read by
  `eval_ct` for `size_of[T]()`), `select_comptime_branches`, and
  `FunctionFrame` around a nested materialization. The cloner's
  `comptime if` class is gone: `def_body_keys_specialization` keys a
  top-level `def` on an unserved `comptime for` or a nested `def`'s
  `rebind` only, and
  `template_serves_binders` admits a scalar value parameter an application
  binds (`Int`, `UInt`, `Bool`, `Float64`, `StringLiteral`, `DType`), and
  native `mono::substitute`'s `scalar_parameter_ty` types its read at the
  declared type (`comptime.rs`, `comptime/specialize.rs`); `CtMarker::Layout` keeps
  a layout constant symbolic through elaboration.
